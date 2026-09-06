//! A durable [`ExecutionStore`] backed by a SQLite database file.
//!
//! Execution records and their appended lifecycle events are stored in two
//! tables (`executions` and `events`). SQLite is embedded via `rusqlite`'s
//! `bundled` feature, so no system SQLite installation is required.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use ows_runtime_core::{
    ErrorKind, ExecutionRecord, ExecutionStore, Phase, StandardErrorType, StoredEvent,
    WorkflowError,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use tokio::sync::Mutex;

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
    started_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_id TEXT NOT NULL,
    sequence     INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    payload      TEXT NOT NULL,
    timestamp    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_execution ON events(execution_id);
"#;

/// A [`ExecutionStore`] persisted in a local SQLite database.
#[derive(Clone)]
pub struct SqliteExecutionStore {
    conn: Arc<Mutex<Connection>>,
}

fn db_error(context: &str, err: impl std::fmt::Display) -> WorkflowError {
    let mut e = WorkflowError::standard(ErrorKind::Runtime, StandardErrorType::Runtime);
    e.problem.detail = Some(format!("sqlite store {context}: {err}"));
    e
}

fn encode_json(value: &Value) -> Result<String, WorkflowError> {
    serde_json::to_string(value).map_err(|e| db_error("encoding value", e))
}

fn decode_json(s: &str) -> Result<Value, WorkflowError> {
    serde_json::from_str(s).map_err(|e| db_error("decoding value", e))
}

impl SqliteExecutionStore {
    /// Opens (creating if needed) a SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorkflowError> {
        let conn = Connection::open(path).map_err(|e| db_error("open", e))?;
        Self::init(conn)
    }

    /// Opens an ephemeral, in-memory SQLite database.
    pub fn in_memory() -> Result<Self, WorkflowError> {
        let conn = Connection::open_in_memory().map_err(|e| db_error("open in-memory", e))?;
        Self::init(conn)
    }

    /// Returns every persisted execution record, ordered by start time then id.
    pub async fn all_records(&self) -> Result<Vec<ExecutionRecord>, WorkflowError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT execution_id, workflow, namespace, name, version, phase,
                        context, pointer, error, started_at
                 FROM executions ORDER BY started_at, execution_id",
            )
            .map_err(|e| db_error("all_records prepare", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .map_err(|e| db_error("all_records", e))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(record_from_row(
                row.map_err(|e| db_error("all_records row", e))?,
            )?);
        }
        Ok(out)
    }

    fn init(conn: Connection) -> Result<Self, WorkflowError> {
        conn.execute_batch(SCHEMA)
            .map_err(|e| db_error("schema", e))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

/// A row of the `executions` table.
type ExecutionRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
);

/// Decodes an `executions` row tuple into an [`ExecutionRecord`].
fn record_from_row(row: ExecutionRow) -> Result<ExecutionRecord, WorkflowError> {
    let (
        execution_id,
        workflow,
        namespace,
        name,
        version,
        phase,
        context,
        pointer,
        error,
        started_at,
    ) = row;
    let phase = match phase.as_str() {
        "pending" => Phase::Pending,
        "running" => Phase::Running,
        "waiting" => Phase::Waiting,
        "suspended" => Phase::Suspended,
        "cancelled" => Phase::Cancelled,
        "faulted" => Phase::Faulted,
        "completed" => Phase::Completed,
        other => return Err(db_error("load", format!("unknown phase `{other}`"))),
    };
    Ok(ExecutionRecord {
        execution_id,
        workflow,
        namespace,
        name,
        version,
        phase,
        context: decode_json(&context)?,
        pointer: decode_json(&pointer)?,
        error: match error {
            Some(e) => Some(decode_json(&e)?),
            None => None,
        },
        started_at,
    })
}

#[async_trait]
impl ExecutionStore for SqliteExecutionStore {
    async fn create(&self, record: ExecutionRecord) -> Result<(), WorkflowError> {
        let context = encode_json(&record.context)?;
        let pointer = encode_json(&record.pointer)?;
        let error = record.error.as_ref().map(encode_json).transpose()?;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO executions
                (execution_id, workflow, namespace, name, version, phase,
                 context, pointer, error, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(execution_id) DO UPDATE SET
                 workflow=excluded.workflow, namespace=excluded.namespace,
                 name=excluded.name, version=excluded.version,
                 phase=excluded.phase, context=excluded.context,
                 pointer=excluded.pointer, error=excluded.error,
                 started_at=excluded.started_at",
            params![
                record.execution_id,
                record.workflow,
                record.namespace,
                record.name,
                record.version,
                record.phase.to_string(),
                context,
                pointer,
                error,
                record.started_at,
            ],
        )
        .map_err(|e| db_error("create", e))?;
        Ok(())
    }

    async fn load(&self, execution_id: &str) -> Result<Option<ExecutionRecord>, WorkflowError> {
        let conn = self.conn.lock().await;
        let row = conn
            .query_row(
                "SELECT execution_id, workflow, namespace, name, version, phase,
                        context, pointer, error, started_at
                 FROM executions WHERE execution_id = ?1",
                params![execution_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| db_error("load", e))?;

        let Some((
            execution_id,
            workflow,
            namespace,
            name,
            version,
            phase,
            context,
            pointer,
            error,
            started_at,
        )) = row
        else {
            return Ok(None);
        };

        let phase = match phase.as_str() {
            "pending" => Phase::Pending,
            "running" => Phase::Running,
            "waiting" => Phase::Waiting,
            "suspended" => Phase::Suspended,
            "cancelled" => Phase::Cancelled,
            "faulted" => Phase::Faulted,
            "completed" => Phase::Completed,
            other => {
                return Err(db_error("load", format!("unknown phase `{other}`")));
            }
        };

        Ok(Some(ExecutionRecord {
            execution_id,
            workflow,
            namespace,
            name,
            version,
            phase,
            context: decode_json(&context)?,
            pointer: decode_json(&pointer)?,
            error: match error {
                Some(e) => Some(decode_json(&e)?),
                None => None,
            },
            started_at,
        }))
    }

    async fn update(&self, record: &ExecutionRecord) -> Result<(), WorkflowError> {
        self.create(record.clone()).await
    }

    async fn append_event(&self, event: StoredEvent) -> Result<(), WorkflowError> {
        let payload = encode_json(&event.payload)?;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO events (execution_id, sequence, event_type, payload, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.execution_id,
                event.sequence as i64,
                event.event_type,
                payload,
                event.timestamp,
            ],
        )
        .map_err(|e| db_error("append_event", e))?;
        Ok(())
    }

    async fn events(&self, execution_id: &str) -> Result<Vec<StoredEvent>, WorkflowError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT execution_id, sequence, event_type, payload, timestamp
                 FROM events WHERE execution_id = ?1 ORDER BY seq",
            )
            .map_err(|e| db_error("events prepare", e))?;
        let rows = stmt
            .query_map(params![execution_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| db_error("events", e))?;

        let mut out = Vec::new();
        for row in rows {
            let (execution_id, sequence, event_type, payload, timestamp) =
                row.map_err(|e| db_error("events row", e))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str) -> ExecutionRecord {
        ExecutionRecord {
            execution_id: id.into(),
            workflow: "test".into(),
            namespace: "default".into(),
            name: "wf".into(),
            version: "1.0.0".into(),
            phase: Phase::Running,
            context: json!({"x": 1}),
            pointer: Value::Null,
            error: None,
            started_at: 0,
        }
    }

    #[tokio::test]
    async fn sqlite_store_roundtrip() {
        let store = SqliteExecutionStore::in_memory().unwrap();
        store.create(record("e1")).await.unwrap();
        let loaded = store.load("e1").await.unwrap().unwrap();
        assert_eq!(loaded.phase, Phase::Running);
        assert_eq!(loaded.context, json!({"x": 1}));

        store
            .append_event(StoredEvent {
                execution_id: "e1".into(),
                sequence: 1,
                event_type: "workflow.started".into(),
                payload: json!({"a": 1}),
                timestamp: 5,
            })
            .await
            .unwrap();
        let events = store.events("e1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "workflow.started");
        assert_eq!(events[0].payload, json!({"a": 1}));
    }

    #[tokio::test]
    async fn sqlite_store_update_and_missing() {
        let store = SqliteExecutionStore::in_memory().unwrap();
        assert!(store.load("missing").await.unwrap().is_none());
        store.create(record("e2")).await.unwrap();
        let mut r = record("e2");
        r.phase = Phase::Completed;
        store.update(&r).await.unwrap();
        assert_eq!(
            store.load("e2").await.unwrap().unwrap().phase,
            Phase::Completed
        );
    }
}
