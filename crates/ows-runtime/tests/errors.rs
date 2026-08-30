//! Error-path integration tests: timeouts, retries, invalid expressions,
//! unknown functions, and policy violations.

use ows_runtime::Runtime;
use ows_runtime_core::ErrorKind;
use serde_json::{json, Value};

async fn run(
    runtime: &Runtime,
    yaml: &str,
    input: Value,
) -> Result<Value, ows_runtime_core::WorkflowError> {
    let def = ows_runtime_dsl::from_yaml(yaml).expect("parse");
    let wf = runtime.register_definition(&def).expect("compile");
    runtime.run(wf, input).await
}

#[tokio::test]
async fn task_timeout_faults() {
    let runtime = Runtime::builder().build().unwrap();
    let err = run(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: timeout
  version: '1.0.0'
do:
  - slow:
      wait:
        seconds: 100
      timeout:
        after:
          milliseconds: 10
"#,
        Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
    assert_eq!(err.problem.status, 408);
}

#[tokio::test]
async fn invalid_expression_faults() {
    let runtime = Runtime::builder().build().unwrap();
    let err = run(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: expr
  version: '1.0.0'
do:
  - bad:
      set:
        x: '${ .this is [ not valid }'
"#,
        Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Expression);
}

#[tokio::test]
async fn unknown_function_faults() {
    let runtime = Runtime::builder().build().unwrap();
    let err = run(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: fn
  version: '1.0.0'
do:
  - callMissing:
      call: does-not-exist
"#,
        Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Semantic);
}

#[tokio::test]
async fn network_denied_by_default() {
    let runtime = Runtime::builder().build().unwrap();
    let err = run(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: net
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: https://example.com
"#,
        Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Policy);
}

#[tokio::test]
async fn retry_exhaustion_then_catch() {
    // A retry policy with 0 attempts must skip retrying and run the catch handler.
    let runtime = Runtime::builder().build().unwrap();
    let out = run(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: retry
  version: '1.0.0'
do:
  - attempt:
      try:
        - fail:
            raise:
              error:
                type: https://open-workflow-specification.org/spec/1.0.0/errors/runtime
                status: 500
                title: transient
      catch:
        errors:
          with:
            status: 500
        retry:
          limit:
            attempt:
              count: 0
        do:
          - handle:
              set:
                caught: true
"#,
        Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(out, json!({ "caught": true }));
}

#[tokio::test]
async fn flow_directive_to_unknown_task_faults() {
    let runtime = Runtime::builder().build().unwrap();
    let err = run(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: bad-flow
  version: '1.0.0'
do:
  - first:
      set: { x: 1 }
      then: missing
"#,
        Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Semantic);
}
