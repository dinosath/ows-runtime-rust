//! A durable [`ExecutionStore`] backed by PostgreSQL.
//!
//! Execution records and lifecycle events are stored in `executions` and
//! `events` tables. The schema mirrors the SQLite backend so a deployment can
//! choose its durable backend without changing how the runtime stores state.

use std::sync::Arc;

use async_trait::async_trait;
use ows_runtime_core::{
    ErrorKind, ExecutionRecord, ExecutionStore, Phase, StandardErrorType, StoredEvent,
    WorkflowError,
};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS executions (
    execution_id TEXT PRIMARY KEY,
    workflow     TEXT NOT NULL,
    namespace    TEXT NOT NULL,
    name         TEXT NOT NULL,
    version      TEXT NOT NULL,
    phase        TEXT NOT NULL,
    context      TEXT NOT NULL,
    pointer      TEXT NOT NULL,
    error        TEXT,
    started_at   BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    seq          BIGSERIAL PRIMARY KEY,
    execution_id TEXT NOT NULL,
    sequence     BIGINT NOT NULL,
    event_type   TEXT NOT NULL,
    payload      TEXT NOT NULL,
    timestamp    BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_execution ON events(execution_id);
"#;

/// A [`ExecutionStore`] persisted in a PostgreSQL database.
#[derive(Clone)]
pub struct PostgresExecutionStore {
    client: Arc<Mutex<Client>>,
}

fn db_error(context: &str, err: impl std::fmt::Display) -> WorkflowError {
    let mut e = WorkflowError::standard(ErrorKind::Runtime, StandardErrorType::Runtime);
    e.problem.detail = Some(format!("postgres store {context}: {err}"));
    e
}

fn encode_json(value: &Value) -> Result<String, WorkflowError> {
    serde_json::to_string(value).map_err(|e| db_error("encoding value", e))
}

fn decode_json(s: &str) -> Result<Value, WorkflowError> {
    serde_json::from_str(s).map_err(|e| db_error("decoding value", e))
}

impl PostgresExecutionStore {
    /// Connects to a PostgreSQL database using a libpq connection string.
    pub async fn connect(connection_string: &str) -> Result<Self, WorkflowError> {
        let (client, connection) = tokio_postgres::connect(connection_string, NoTls)
            .await
            .map_err(|e| db_error("connect", e))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let store = Self {
            client: Arc::new(Mutex::new(client)),
        };
        store.init().await?;
        Ok(store)
    }

    async fn init(&self) -> Result<(), WorkflowError> {
        let client = self.client.lock().await;
        client
            .batch_execute(SCHEMA)
            .await
            .map_err(|e| db_error("schema", e))?;
        Ok(())
    }
}

fn parse_phase(s: &str) -> Result<Phase, WorkflowError> {
    Ok(match s {
        "pending" => Phase::Pending,
        "running" => Phase::Running,
        "waiting" => Phase::Waiting,
        "suspended" => Phase::Suspended,
        "cancelled" => Phase::Cancelled,
        "faulted" => Phase::Faulted,
        "completed" => Phase::Completed,
        other => return Err(db_error("load", format!("unknown phase `{other}`"))),
    })
}

#[async_trait]
impl ExecutionStore for PostgresExecutionStore {
    async fn create(&self, record: ExecutionRecord) -> Result<(), WorkflowError> {
        let context = encode_json(&record.context)?;
        let pointer = encode_json(&record.pointer)?;
        let error = record.error.as_ref().map(encode_json).transpose()?;
        let client = self.client.lock().await;
        client
            .execute(
                "INSERT INTO executions
                    (execution_id, workflow, namespace, name, version, phase,
                     context, pointer, error, started_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (execution_id) DO UPDATE SET
                     workflow = EXCLUDED.workflow,
                     namespace = EXCLUDED.namespace,
                     name = EXCLUDED.name,
                     version = EXCLUDED.version,
                     phase = EXCLUDED.phase,
                     context = EXCLUDED.context,
                     pointer = EXCLUDED.pointer,
                     error = EXCLUDED.error,
                     started_at = EXCLUDED.started_at",
                &[
                    &record.execution_id,
                    &record.workflow,
                    &record.namespace,
                    &record.name,
                    &record.version,
                    &record.phase.to_string(),
                    &context,
                    &pointer,
                    &error,
                    &record.started_at,
                ],
            )
            .await
            .map_err(|e| db_error("create", e))?;
        Ok(())
    }

    async fn load(&self, execution_id: &str) -> Result<Option<ExecutionRecord>, WorkflowError> {
        let client = self.client.lock().await;
        let row = client
            .query_opt(
                "SELECT execution_id, workflow, namespace, name, version, phase,
                        context, pointer, error, started_at
                 FROM executions WHERE execution_id = $1",
                &[&execution_id],
            )
            .await
            .map_err(|e| db_error("load", e))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let phase: &str = row.get(5);
        let context: String = row.get(6);
        let pointer: String = row.get(7);
        let error: Option<String> = row.get(8);

        Ok(Some(ExecutionRecord {
            execution_id: row.get(0),
            workflow: row.get(1),
            namespace: row.get(2),
            name: row.get(3),
            version: row.get(4),
            phase: parse_phase(phase)?,
            context: decode_json(&context)?,
            pointer: decode_json(&pointer)?,
            error: match error {
                Some(e) => Some(decode_json(&e)?),
                None => None,
            },
            started_at: row.get(9),
        }))
    }

    async fn update(&self, record: &ExecutionRecord) -> Result<(), WorkflowError> {
        self.create(record.clone()).await
    }

    async fn append_event(&self, event: StoredEvent) -> Result<(), WorkflowError> {
        let payload = encode_json(&event.payload)?;
        let client = self.client.lock().await;
        client
            .execute(
                "INSERT INTO events (execution_id, sequence, event_type, payload, timestamp)
                 VALUES ($1, $2, $3, $4, $5)",
                &[
                    &event.execution_id,
                    &(event.sequence as i64),
                    &event.event_type,
                    &payload,
                    &event.timestamp,
                ],
            )
            .await
            .map_err(|e| db_error("append_event", e))?;
        Ok(())
    }

    async fn events(&self, execution_id: &str) -> Result<Vec<StoredEvent>, WorkflowError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT execution_id, sequence, event_type, payload, timestamp
                 FROM events WHERE execution_id = $1 ORDER BY seq",
                &[&execution_id],
            )
            .await
            .map_err(|e| db_error("events", e))?;

        let mut out = Vec::new();
        for row in rows {
            let execution_id: String = row.get(0);
            let sequence: i64 = row.get(1);
            let event_type: String = row.get(2);
            let payload: String = row.get(3);
            let timestamp: i64 = row.get(4);
            out.push(StoredEvent {
                execution_id,
                sequence: sequence as u64,
                event_type,
                payload: decode_json(&payload)?,
                timestamp,
            });
        }
        Ok(out)
    }
}
