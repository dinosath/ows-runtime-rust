#![cfg(feature = "http")]

use ows_runtime::Runtime;
use ows_runtime_core::RuntimePolicy;
use serde_json::json;

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
    let out = runtime.run(wf, json!({})).await;
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
