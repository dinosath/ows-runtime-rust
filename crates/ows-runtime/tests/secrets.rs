//! Integration tests for `use.secrets` resolution and enforcement.

use ows_runtime::Runtime;
use serde_json::{json, Value};

#[tokio::test]
async fn declared_secrets_are_resolved_into_dollar_secrets() {
    let runtime = Runtime::builder()
        .with_secrets([("token", "s3cr3t")])
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: secrets, version: '1.0.0' }
use:
  secrets: [token]
do:
  - s:
      set:
        token: '${ $secrets.token }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    assert_eq!(wf.secrets, vec!["token".to_string()]);
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({ "token": "s3cr3t" }));
}

#[test]
fn undeclared_secret_reference_fails_compilation() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: secrets-bad, version: '1.0.0' }
do:
  - s:
      set:
        token: '${ $secrets.nope }'
"#,
    )
    .unwrap();
    let err = runtime.register_definition(&def).unwrap_err();
    assert_eq!(err.kind, ows_runtime_core::ErrorKind::Semantic);
    assert!(
        err.problem
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("undeclared secret"),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn declared_but_unresolved_secret_is_null() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: secrets-unresolved, version: '1.0.0' }
use:
  secrets: [token]
do:
  - s:
      set:
        token: '${ $secrets.token }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({ "token": null }));
}

#[tokio::test]
async fn unresolved_secret_may_be_defaulted_in_expressions() {
    let runtime = Runtime::builder()
        .with_secrets([("token", "v")])
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: secrets-default, version: '1.0.0' }
use:
  secrets: [token, missing]
do:
  - s:
      set:
        present: '${ $secrets.token }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({ "present": "v" }));
}
