//! Durable checkpoint/resume tests.

use std::sync::Arc;

use ows_runtime::Runtime;
use ows_runtime_core::{ExecutionRecord, ExecutionStore, InMemoryExecutionStore, Phase};
use serde_json::{json, Value};

fn definition() -> &'static str {
    r#"
document: { dsl: '1.0.3', namespace: resume, name: wf, version: '1.0.0' }
do:
  - a: { set: { step: a } }
  - b: { set: { step: b } }
  - c: { set: { step: c } }
"#
}

fn crash_record(pointer: Value, phase: Phase) -> ExecutionRecord {
    ExecutionRecord {
        execution_id: "exec-1".into(),
        workflow: "resume/wf/1.0.0".into(),
        namespace: "resume".into(),
        name: "wf".into(),
        version: "1.0.0".into(),
        phase,
        context: json!({}),
        pointer,
        error: None,
        started_at: 0,
    }
}

#[tokio::test]
async fn resume_continues_from_checkpoint_and_completes() {
    let store = Arc::new(InMemoryExecutionStore::new());
    // Simulate a crash after the first task: resume at index 1 with its output.
    store
        .create(crash_record(
            json!({ "next": 1, "input": { "step": "a" } }),
            Phase::Running,
        ))
        .await
        .unwrap();

    let runtime = Runtime::builder()
        .with_store(store.clone())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(definition()).unwrap();
    runtime.register_definition(&def).unwrap();

    let out = runtime.resume("exec-1").await.unwrap();
    assert_eq!(out, json!({ "step": "c" }));

    let record = store.load("exec-1").await.unwrap().unwrap();
    assert_eq!(record.execution_id, "exec-1");
    assert_eq!(record.phase, Phase::Completed);
}

#[tokio::test]
async fn resume_falls_back_to_context_when_pointer_has_no_input() {
    let store = Arc::new(InMemoryExecutionStore::new());
    store
        .create(crash_record(json!({ "next": 2 }), Phase::Running))
        .await
        .unwrap();

    let runtime = Runtime::builder()
        .with_store(store.clone())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(definition()).unwrap();
    runtime.register_definition(&def).unwrap();

    // Only the final task runs.
    let out = runtime.resume("exec-1").await.unwrap();
    assert_eq!(out, json!({ "step": "c" }));
}

#[tokio::test]
async fn resume_rejects_terminal_executions() {
    let store = Arc::new(InMemoryExecutionStore::new());
    store
        .create(crash_record(Value::Null, Phase::Completed))
        .await
        .unwrap();

    let runtime = Runtime::builder()
        .with_store(store.clone())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(definition()).unwrap();
    runtime.register_definition(&def).unwrap();

    let err = runtime.resume("exec-1").await.unwrap_err();
    assert!(err.problem.detail.unwrap_or_default().contains("already"));
}

#[tokio::test]
async fn resume_unknown_execution_errors() {
    let runtime = Runtime::builder().build().unwrap();
    let err = runtime.resume("does-not-exist").await.unwrap_err();
    assert!(err
        .problem
        .detail
        .unwrap_or_default()
        .contains("unknown execution"));
}

#[tokio::test]
async fn resume_requires_the_workflow_to_be_registered() {
    let store = Arc::new(InMemoryExecutionStore::new());
    store
        .create(crash_record(
            json!({ "next": 1, "input": { "step": "a" } }),
            Phase::Running,
        ))
        .await
        .unwrap();

    let runtime = Runtime::builder()
        .with_store(store.clone())
        .build()
        .unwrap();
    let err = runtime.resume("exec-1").await.unwrap_err();
    assert!(err
        .problem
        .detail
        .unwrap_or_default()
        .contains("not registered"));
}
