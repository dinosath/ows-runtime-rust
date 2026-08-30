//! Runtime behavior tests: cancellation, workflow timeout, and execution handles.

use ows_runtime::Runtime;
use ows_runtime_core::ErrorKind;
use serde_json::{json, Value};

#[tokio::test]
async fn cancellation_stops_workflow() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: cancel
  version: '1.0.0'
do:
  - waitForever:
      wait:
        seconds: 1000
  - after:
      set: { x: 1 }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let handle = runtime.execute(wf, Value::Null).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle.cancel();
    let err = handle.wait().await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
}

#[tokio::test]
async fn workflow_timeout_faults() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: wf-timeout
  version: '1.0.0'
do:
  - waitForever:
      wait:
        seconds: 1000
timeout:
  after:
    milliseconds: 50
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
}

#[tokio::test]
async fn execution_handle_waits_for_output() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: handle
  version: '1.0.0'
do:
  - setVal:
      set: { v: 42 }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let handle = runtime.execute(wf, Value::Null).await.unwrap();
    assert!(!handle.execution_id.is_empty());
    let out = handle.wait().await.unwrap();
    assert_eq!(out, json!({ "v": 42 }));
}

#[tokio::test]
async fn task_order_tracks_execution() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: order
  version: '1.0.0'
do:
  - first: { set: { a: 1 } }
  - second: { set: { b: 2 } }
  - third: { set: { c: 3 } }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    runtime.run(wf.clone(), Value::Null).await.unwrap();
    let order = runtime.take_task_order();
    assert_eq!(order, vec!["first", "second", "third"]);
    // Re-run uses a fresh order.
    runtime.run(wf, Value::Null).await.unwrap();
    let order = runtime.take_task_order();
    assert_eq!(order.len(), 3);
}
