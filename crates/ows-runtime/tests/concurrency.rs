//! Concurrency tests: concurrent workflow executions and parallel fork branches.

use ows_runtime::Runtime;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_workflow_executions() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: concurrent
  version: '1.0.0'
do:
  - inc:
      set:
        n: '${ .n + 1 }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();

    let mut handles = Vec::new();
    for i in 0..50 {
        let wf = wf.clone();
        let runtime = runtime.clone();
        handles.push(tokio::spawn(async move {
            let out = runtime.run(wf, json!({ "n": i })).await.unwrap();
            out
        }));
    }

    for (i, handle) in handles.into_iter().enumerate() {
        let out = handle.await.unwrap();
        assert_eq!(out, json!({ "n": i + 1 }));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_high_parallelism() {
    let runtime = Runtime::builder().build().unwrap();
    // Build a workflow with 50 fork branches dynamically.
    let mut branches = String::new();
    for i in 0..50 {
        branches.push_str(&format!("          - b{i}: {{ set: {{ v: {i} }} }}\n"));
    }
    let yaml = format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: parallel-fork
  version: '1.0.0'
do:
  - fanout:
      fork:
        compete: false
        branches:
{branches}
"#
    );
    let def = ows_runtime_dsl::from_yaml(&yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    let arr = out.as_array().expect("fork output is an array");
    assert_eq!(arr.len(), 50);
}
