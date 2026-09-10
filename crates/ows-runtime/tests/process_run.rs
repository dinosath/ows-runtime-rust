//! Integration tests for the opt-in `TokioProcessRunner` (`run` shell/script).

use std::sync::Arc;

use ows_runtime::Runtime;
use ows_runtime::TokioProcessRunner;
use ows_runtime_core::ErrorKind;
use serde_json::{json, Value};

#[tokio::test]
async fn run_shell_executes_with_opt_in_runner() {
    let runtime = Runtime::builder()
        .with_process(Arc::new(TokioProcessRunner::new().allow_scripts()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: proc, name: shell, version: '1.0.0' }
do:
  - say:
      run:
        shell:
          command: echo
          arguments: [hello]
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!("hello\n"));
}

#[tokio::test]
async fn run_script_executes_with_opt_in_runner() {
    let runtime = Runtime::builder()
        .with_process(Arc::new(TokioProcessRunner::new().allow_scripts()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: proc, name: script, version: '1.0.0' }
do:
  - say:
      run:
        script:
          language: bash
          code: 'echo scripted'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!("scripted\n"));
}

#[tokio::test]
async fn run_shell_is_denied_by_default() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: proc, name: denied, version: '1.0.0' }
do:
  - say:
      run:
        shell:
          command: echo
          arguments: [nope]
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Policy);
}
