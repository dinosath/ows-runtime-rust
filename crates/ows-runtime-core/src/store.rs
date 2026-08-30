//! Execution state storage abstraction.
//!
//! The runtime is designed so that persistence can be added later without
//! changing workflow semantics. All execution state mutations go through the
//! [`ExecutionStore`] trait. An in-memory implementation is provided for tests
//! and local development; PostgreSQL / Redis / SQLite adapters can be layered
//! on top without touching the engine.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::WorkflowError;
use crate::phase::Phase;

/// A single stored workflow execution record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionRecord {
    /// The stable execution id.
    pub execution_id: String,
    /// The workflow identity (qualified name).
    pub workflow: String,
    /// The workflow namespace.
    pub namespace: String,
    /// The workflow name.
    pub name: String,
    /// The workflow version.
    pub version: String,
    /// The current phase.
    pub phase: Phase,
    /// The current workflow context.
    pub context: Value,
    /// The current task index / pointer (implementation specific).
    pub pointer: Value,
    /// Errors, if any.
    pub error: Option<Value>,
    /// The execution start epoch seconds.
    pub started_at: i64,
}

/// A single stored event appended to an execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredEvent {
    /// The execution id.
    pub execution_id: String,
    /// The event sequence number.
    pub sequence: u64,
    /// The event type (e.g. `workflow.started`).
    pub event_type: String,
    /// The event payload.
    pub payload: Value,
    /// The event timestamp (epoch seconds).
    pub timestamp: i64,
}

/// Storage for workflow execution state.
///
/// Implementations must be `Send + Sync`. All methods are asynchronous to allow
/// database-backed implementations.
#[async_trait::async_trait]
pub trait ExecutionStore: Send + Sync {
    /// Creates a new execution record.
    async fn create(&self, record: ExecutionRecord) -> Result<(), WorkflowError>;

    /// Loads an execution record by id.
    async fn load(&self, execution_id: &str) -> Result<Option<ExecutionRecord>, WorkflowError>;

    /// Updates an existing execution record.
    async fn update(&self, record: &ExecutionRecord) -> Result<(), WorkflowError>;

    /// Appends an event to the execution's event log.
    async fn append_event(&self, event: StoredEvent) -> Result<(), WorkflowError>;

    /// Lists events for an execution.
    async fn events(&self, execution_id: &str) -> Result<Vec<StoredEvent>, WorkflowError>;
}

/// An in-memory [`ExecutionStore`] for tests and local development.
#[derive(Debug, Clone, Default)]
pub struct InMemoryExecutionStore {
    records: Arc<Mutex<HashMap<String, ExecutionRecord>>>,
    events: Arc<Mutex<Vec<StoredEvent>>>,
}

impl InMemoryExecutionStore {
    /// Creates a new empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ExecutionStore for InMemoryExecutionStore {
    async fn create(&self, record: ExecutionRecord) -> Result<(), WorkflowError> {
        let mut records = self.records.lock().await;
        records.insert(record.execution_id.clone(), record);
        Ok(())
    }

    async fn load(&self, execution_id: &str) -> Result<Option<ExecutionRecord>, WorkflowError> {
        let records = self.records.lock().await;
        Ok(records.get(execution_id).cloned())
    }

    async fn update(&self, record: &ExecutionRecord) -> Result<(), WorkflowError> {
        let mut records = self.records.lock().await;
        records.insert(record.execution_id.clone(), record.clone());
        Ok(())
    }

    async fn append_event(&self, event: StoredEvent) -> Result<(), WorkflowError> {
        let mut events = self.events.lock().await;
        events.push(event);
        Ok(())
    }

    async fn events(&self, execution_id: &str) -> Result<Vec<StoredEvent>, WorkflowError> {
        let events = self.events.lock().await;
        Ok(events
            .iter()
            .filter(|e| e.execution_id == execution_id)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_store_roundtrip() {
        let store = InMemoryExecutionStore::new();
        let record = ExecutionRecord {
            execution_id: "exec-1".into(),
            workflow: "test".into(),
            namespace: "default".into(),
            name: "wf".into(),
            version: "1.0.0".into(),
            phase: Phase::Running,
            context: Value::Null,
            pointer: Value::Null,
            error: None,
            started_at: 0,
        };
        store.create(record.clone()).await.unwrap();
        let loaded = store.load("exec-1").await.unwrap().unwrap();
        assert_eq!(loaded.execution_id, "exec-1");
        assert_eq!(loaded.phase, Phase::Running);

        store
            .append_event(StoredEvent {
                execution_id: "exec-1".into(),
                sequence: 1,
                event_type: "workflow.started".into(),
                payload: Value::Null,
                timestamp: 0,
            })
            .await
            .unwrap();
        let events = store.events("exec-1").await.unwrap();
        assert_eq!(events.len(), 1);
    }
}
