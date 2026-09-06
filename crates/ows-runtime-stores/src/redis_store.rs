//! A durable [`ExecutionStore`] backed by Redis.
//!
//! Execution records are stored as JSON under `ows:exec:<id>` and lifecycle
//! events are appended to the list `ows:exec_events:<id>`. This keeps reads and
//! writes simple while allowing an execution's state and event log to survive a
//! runtime restart.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use async_trait::async_trait;
use ows_runtime_core::{
    ErrorKind, ExecutionRecord, ExecutionStore, StandardErrorType, StoredEvent, WorkflowError,
};
use redis::aio::MultiplexedConnection;

/// A [`ExecutionStore`] persisted in Redis.
#[derive(Clone)]
pub struct RedisExecutionStore {
    conn: Arc<MultiplexedConnection>,
}

fn db_error(context: &str, err: impl std::fmt::Display) -> WorkflowError {
    let mut e = WorkflowError::standard(ErrorKind::Runtime, StandardErrorType::Runtime);
    e.problem.detail = Some(format!("redis store {context}: {err}"));
    e
}

fn record_key(id: &str) -> String {
    format!("ows:exec:{id}")
}

fn events_key(id: &str) -> String {
    format!("ows:exec_events:{id}")
}

impl RedisExecutionStore {
    /// Connects to a Redis instance at the given URL (e.g. `redis://127.0.0.1/`).
    pub async fn connect(url: &str) -> Result<Self, WorkflowError> {
        let client = redis::Client::open(url).map_err(|e| db_error("open", e))?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| db_error("connect", e))?;
        Ok(Self {
            conn: Arc::new(conn),
        })
    }
}

#[async_trait]
impl ExecutionStore for RedisExecutionStore {
    async fn create(&self, record: ExecutionRecord) -> Result<(), WorkflowError> {
        let json = serde_json::to_string(&record).map_err(|e| db_error("encoding", e))?;
        let mut conn = self.conn.as_ref().clone();
        redis::cmd("SET")
            .arg(record_key(&record.execution_id))
            .arg(&json)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| db_error("create", e))?;
        Ok(())
    }

    async fn load(&self, execution_id: &str) -> Result<Option<ExecutionRecord>, WorkflowError> {
        let mut conn = self.conn.as_ref().clone();
        let json: Option<String> = redis::cmd("GET")
            .arg(record_key(execution_id))
            .query_async::<Option<String>>(&mut conn)
            .await
            .map_err(|e| db_error("load", e))?;
        match json {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| db_error("decoding", e)),
            None => Ok(None),
        }
    }

    async fn update(&self, record: &ExecutionRecord) -> Result<(), WorkflowError> {
        self.create(record.clone()).await
    }

    async fn append_event(&self, event: StoredEvent) -> Result<(), WorkflowError> {
        let json = serde_json::to_string(&event).map_err(|e| db_error("encoding", e))?;
        let mut conn = self.conn.as_ref().clone();
        redis::cmd("RPUSH")
            .arg(events_key(&event.execution_id))
            .arg(&json)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| db_error("append_event", e))?;
        Ok(())
    }

    async fn events(&self, execution_id: &str) -> Result<Vec<StoredEvent>, WorkflowError> {
        let mut conn = self.conn.as_ref().clone();
        let values: Vec<String> = redis::cmd("LRANGE")
            .arg(events_key(execution_id))
            .arg(0)
            .arg(-1)
            .query_async::<Vec<String>>(&mut conn)
            .await
            .map_err(|e| db_error("events", e))?;
        values
            .iter()
            .map(|v| serde_json::from_str(v).map_err(|e| db_error("decoding event", e)))
            .collect()
    }
}
