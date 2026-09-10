//! Integration tests for `use.extensions` (`before`/`after`/`when`).

use ows_runtime::Runtime;
use serde_json::{json, Value};

#[tokio::test]
async fn extension_runs_before_and_after_without_recursing() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: ext, version: '1.0.0' }
use:
  extensions:
    - audit:
        extend: set
        before:
          - mark:
              set: { before_ran: true }
        after:
          - augment:
              set: { result: '${ $input }' }
do:
  - t: { set: { x: 1 } }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    assert_eq!(wf.extensions.len(), 1);
    assert_eq!(wf.extensions[0].extend, "set");
    let out = runtime.run(wf, Value::Null).await.unwrap();
    // The `after` extension wraps the extended task's output. If extensions
    // recursed into their own `set` tasks this would never terminate.
    assert_eq!(out, json!({ "result": { "x": 1 } }));
}

#[tokio::test]
async fn extension_when_guard_controls_application() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: ext-when, version: '1.0.0' }
use:
  extensions:
    - audit:
        extend: set
        when: '.enable == true'
        after:
          - augment:
              set: { wrapped: true }
do:
  - t: { set: { x: 1 } }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({ "enable": false })).await.unwrap();
    // Guard is evaluated against the task's scope input (`enable: false`), so
    // the extension is skipped and the raw set output is returned.
    assert_eq!(out, json!({ "x": 1 }));
}

#[tokio::test]
async fn extension_when_guard_applies_when_true() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: ext-when2, version: '1.0.0' }
use:
  extensions:
    - audit:
        extend: set
        when: '.enable == true'
        after:
          - augment:
              set: { wrapped: true }
do:
  - t: { set: { x: 1 } }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({ "enable": true })).await.unwrap();
    assert_eq!(out, json!({ "wrapped": true }));
}

#[tokio::test]
async fn extension_before_exit_short_circuits_extended_task() {
    // Mirrors the official `mock-service-extension` example: a `before` task
    // with `then: exit` injects a response and skips the extended task body.
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: ext-mock, version: '1.0.0' }
use:
  extensions:
    - mockService:
        extend: set
        when: '.mock == true'
        before:
          - mockResponse:
              set:
                statusCode: 200
                content: { foo: baz }
              then: exit
do:
  - t: { set: { x: 1 } }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({ "mock": true })).await.unwrap();
    assert_eq!(
        out,
        json!({ "statusCode": 200, "content": { "foo": "baz" } })
    );

    // Without the guard, the extension does not apply and the body runs.
    let wf2 = runtime.register_definition(&def).unwrap();
    let out2 = runtime.run(wf2, json!({ "mock": false })).await.unwrap();
    assert_eq!(out2, json!({ "x": 1 }));
}

#[tokio::test]
async fn extension_targets_call_function_name() {
    use std::sync::Arc;
    let runtime = Runtime::builder()
        .register_function("http", Arc::new(FakeHttp))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: ext-call, version: '1.0.0' }
use:
  extensions:
    - httpAudit:
        extend: http
        before:
          - mark:
              set: { pre: true }
do:
  - t:
      call: http
      with:
        method: get
        endpoint: https://example.com
        response: json
"#,
    )
    .unwrap();
    // The extension is compiled and targets the `http` function. The call is
    // served by the fake invoker, so no network is used.
    let wf = runtime.register_definition(&def).unwrap();
    assert_eq!(wf.extensions[0].extend, "http");
    let out = runtime.run(wf, Value::Null).await.unwrap();
    let _ = out;
}

/// A `call: http` function invoker that never touches the network.
struct FakeHttp;

#[async_trait::async_trait]
impl ows_runtime::service::FunctionInvoker for FakeHttp {
    async fn invoke(
        &self,
        _req: ows_runtime::service::FunctionRequest<'_>,
    ) -> Result<Value, ows_runtime_core::WorkflowError> {
        Ok(json!({ "status": 200 }))
    }
}
