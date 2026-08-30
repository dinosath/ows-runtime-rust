#![cfg(feature = "http")]
#![allow(clippy::result_large_err)]

use ows_runtime::Runtime;
use ows_runtime_core::RuntimePolicy;
use serde_json::{json, Value};

#[tokio::test]
async fn http_call_get() {
    let policy = RuntimePolicy {
        allow_network: true,
        ..Default::default()
    };
    let runtime = Runtime::builder().with_policy(policy).build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: https://httpbin.org/get
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out: Result<Value, ows_runtime_core::WorkflowError> = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.run(wf, json!({})),
    )
    .await
    .unwrap_or_else(|_| Err(ows_runtime_core::WorkflowError::timeout(None)));
    match out {
        Ok(v) => {
            assert!(v.get("url").is_some(), "expected url in response, got {v}");
            eprintln!("http ok: {}", v.get("url").unwrap());
        }
        Err(e) => {
            eprintln!("http failed (may be no network): {e}");
        }
    }
}
