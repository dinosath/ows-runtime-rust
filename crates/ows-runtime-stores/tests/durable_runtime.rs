//! End-to-end durable execution against the real SQLite store.
//!
//! A workflow is run through the runtime with `SqliteExecutionStore` wired as
//! the durable backend, then the persisted record is reloaded from the database
//! to confirm the terminal phase and error were written durably.
#![cfg(feature = "sqlite")]

use std::sync::Arc;

use ows_runtime::Runtime;
use ows_runtime_core::Phase;
use ows_runtime_stores::SqliteExecutionStore;
use serde_json::Value;

fn sqlite_store() -> SqliteExecutionStore {
    SqliteExecutionStore::in_memory().unwrap()
}

#[tokio::test]
async fn runtime_persists_completed_execution_to_sqlite() {
    let store = sqlite_store();
    let rt = Runtime::builder()
        .with_store(Arc::new(store.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: durable, version: '1.0.0' }
do:
  - a: { set: { value: 42 } }
"#,
    )
    .unwrap();
    let wf = rt.register_definition(&def).unwrap();
    rt.run(wf, Value::Null).await.unwrap();

    let records = store.all_records().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].name, "durable");
    assert_eq!(records[0].namespace, "default");
    assert_eq!(records[0].phase, Phase::Completed);
    assert!(records[0].error.is_none());
}

#[tokio::test]
async fn runtime_persists_faulted_execution_to_sqlite() {
    let store = sqlite_store();
    let rt = Runtime::builder()
        .with_store(Arc::new(store.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: durable-fault, version: '1.0.0' }
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

    let records = store.all_records().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, Phase::Faulted);
    assert!(records[0].error.is_some());
}

#[tokio::test]
async fn resume_continues_a_checkpointed_execution_across_runtime_instances() {
    use ows_runtime_core::{ExecutionRecord, ExecutionStore};
    use serde_json::json;

    let store = sqlite_store();
    // Simulate an interrupted (crashed) execution persisted durably.
    store
        .create(ExecutionRecord {
            execution_id: "durable-exec-1".into(),
            workflow: "default/durable-resume/1.0.0".into(),
            namespace: "default".into(),
            name: "durable-resume".into(),
            version: "1.0.0".into(),
            phase: Phase::Running,
            context: json!({ "seen": ["a"] }),
            pointer: json!({ "next": 1, "input": { "step": "a" } }),
            error: None,
            started_at: 0,
        })
        .await
        .unwrap();

    let definition = r#"
document: { dsl: '1.0.3', namespace: default, name: durable-resume, version: '1.0.0' }
do:
  - a: { set: { step: a } }
  - b: { set: { step: b } }
"#;

    // First runtime instance resumes the checkpoint...
    let rt = Runtime::builder()
        .with_store(Arc::new(store.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(definition).unwrap();
    rt.register_definition(&def).unwrap();
    let out = rt.resume("durable-exec-1").await.unwrap();
    assert_eq!(out, json!({ "step": "b" }));
    drop(rt);

    // ...and a fresh runtime instance can observe the durable terminal state.
    let records = store.all_records().await.unwrap();
    let record = records
        .iter()
        .find(|r| r.execution_id == "durable-exec-1")
        .unwrap();
    assert_eq!(record.phase, Phase::Completed, "resume must persist completion");
}
