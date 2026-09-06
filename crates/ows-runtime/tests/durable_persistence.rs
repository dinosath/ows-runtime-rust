//! Verifies that the execution engine persists workflow execution state through
//! the configured `ExecutionStore` (create on start, update to a terminal phase,
//! and lifecycle events). This makes durable execution real: a store wired via
//! `RuntimeBuilder::with_store` (e.g. `ows-runtime-stores`) now receives records.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ows_runtime::Runtime;
use ows_runtime_core::{ExecutionRecord, ExecutionStore, Phase, StoredEvent, WorkflowError};
use serde_json::Value;

/// A recording store capturing every create/update/event.
#[derive(Clone, Default)]
struct RecordingStore {
    records: Arc<Mutex<Vec<ExecutionRecord>>>,
    events: Arc<Mutex<Vec<StoredEvent>>>,
}

impl RecordingStore {
    fn snapshot_records(&self) -> Vec<ExecutionRecord> {
        self.records.lock().unwrap().clone()
    }
    fn snapshot_events(&self) -> Vec<StoredEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait]
impl ExecutionStore for RecordingStore {
    async fn create(&self, record: ExecutionRecord) -> Result<(), WorkflowError> {
        self.records.lock().unwrap().push(record);
        Ok(())
    }
    async fn load(&self, _execution_id: &str) -> Result<Option<ExecutionRecord>, WorkflowError> {
        Ok(self.records.lock().unwrap().last().cloned())
    }
    async fn update(&self, record: &ExecutionRecord) -> Result<(), WorkflowError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
    async fn append_event(&self, event: StoredEvent) -> Result<(), WorkflowError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
    async fn events(&self, _execution_id: &str) -> Result<Vec<StoredEvent>, WorkflowError> {
        Ok(self.events.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn completed_execution_is_persisted() {
    let store = RecordingStore::default();
    let rt = Runtime::builder()
        .with_store(Arc::new(store.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: persist, version: '1.0.0' }
do:
  - a: { set: { value: 1 } }
"#,
    )
    .unwrap();
    let wf = rt.register_definition(&def).unwrap();
    rt.run(wf, Value::Null).await.unwrap();

    let records = store.snapshot_records();
    assert!(!records.is_empty(), "expected at least a start record");
    assert_eq!(records[0].phase, Phase::Running);
    assert_eq!(records.last().unwrap().phase, Phase::Completed);

    let events = store.snapshot_events();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(types.contains(&"workflow.started"));
    assert!(types.contains(&"workflow.completed"));
}

#[tokio::test]
async fn faulted_execution_is_persisted_with_error() {
    let store = RecordingStore::default();
    let rt = Runtime::builder()
        .with_store(Arc::new(store.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: persist-fault, version: '1.0.0' }
do:
  - boom:
      raise:
        error:
          type: https://open-workflow-specification.org/spec/1.0.0/errors/runtime
          status: 500
          title: Boom
"#,
    )
    .unwrap();
    let wf = rt.register_definition(&def).unwrap();
    assert!(rt.run(wf, Value::Null).await.is_err());

    let records = store.snapshot_records();
    assert_eq!(records[0].phase, Phase::Running);
    assert_eq!(records.last().unwrap().phase, Phase::Faulted);
    assert!(records.last().unwrap().error.is_some());

    let events = store.snapshot_events();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(types.contains(&"workflow.started"));
    assert!(types.contains(&"workflow.faulted"));
}
