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
