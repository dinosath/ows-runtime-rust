#![allow(clippy::result_large_err)]
//! Integration tests covering additional task dispatch paths to raise coverage.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ows_runtime::Runtime;
use ows_runtime_core::{EventPublisher, ProcessResult, ProcessRunner, WorkflowError};
use serde_json::{json, Value};

/// A process runner that fails a configurable number of times then succeeds.
#[derive(Clone)]
struct FlakyRunner {
    failures: Arc<AtomicUsize>,
    fail_count: usize,
}

impl FlakyRunner {
    fn new(fail_count: usize) -> Self {
        Self {
            failures: Arc::new(AtomicUsize::new(0)),
            fail_count,
        }
    }
    fn maybe_fail(&self) -> Result<ProcessResult, WorkflowError> {
        let n = self.failures.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            Err(WorkflowError::new(
                ows_runtime_core::ErrorKind::Communication,
                ows_runtime_core::ProblemDetails::standard(
                    ows_runtime_core::StandardErrorType::Communication,
                ),
            ))
        } else {
            Ok(ProcessResult {
                code: Some(0),
                stdout: Some("ok".into()),
                stderr: None,
            })
        }
    }
}

#[async_trait::async_trait]
impl ProcessRunner for FlakyRunner {
    async fn run_shell(
        &self,
        _c: &str,
        _a: &[String],
        _e: &HashMap<String, String>,
        _s: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        self.maybe_fail()
    }
    async fn run_script(
        &self,
        _l: &str,
        _c: &str,
        _a: &[String],
        _e: &HashMap<String, String>,
        _s: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        self.maybe_fail()
    }
    async fn run_container(
        &self,
        _i: &str,
        _a: &[String],
        _e: &HashMap<String, String>,
        _s: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        self.maybe_fail()
    }
}

#[tokio::test]
async fn try_retry_recovers_after_failures() {
    let runner = FlakyRunner::new(2);
    let runtime = Runtime::builder()
        .with_process(Arc::new(runner))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: retry
  version: '1.0.0'
do:
  - attempt:
      try:
        - runIt:
            run:
              shell:
                command: echo hi
      catch:
        retry:
          delay:
            milliseconds: 0
          limit:
            attempt:
              count: 5
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!("ok"));
}

#[tokio::test]
async fn for_with_while_condition() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: for-while
  version: '1.0.0'
do:
  - loop:
      for:
        each: n
        in: '[1, 2, 3, 4]'
        at: idx
      while: '$idx < 2'
      do:
        - acc:
            set:
              collected: '${ .collected + [ $n ] }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({ "collected": [] })).await.unwrap();
    // The body input advances per iteration; while stops after idx 2.
    assert_eq!(out["collected"][0], json!(1));
}

#[tokio::test]
async fn call_reusable_function() {
    // A local identity function invoker.
    struct Identity;
    #[async_trait::async_trait]
    impl ows_runtime::service::FunctionInvoker for Identity {
        async fn invoke(
            &self,
            req: ows_runtime::service::FunctionRequest<'_>,
        ) -> Result<Value, WorkflowError> {
            Ok(req.context.input.clone())
        }
    }
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: reuse
  version: '1.0.0'
use:
  functions:
    mySet:
      call: mySetImpl
do:
  - useIt:
      call: mySet
"#,
    )
    .unwrap();
    let runtime = Runtime::builder()
        .register_function("mySetImpl", Arc::new(Identity))
        .build()
        .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({"a": 1})).await.unwrap();
    assert_eq!(out, json!({"a": 1}));
}

#[tokio::test]
async fn run_shell_returns_stdout() {
    #[derive(Clone)]
    struct Fixed;
    #[async_trait::async_trait]
    impl ProcessRunner for Fixed {
        async fn run_shell(
            &self,
            _: &str,
            _: &[String],
            _: &HashMap<String, String>,
            _: Option<String>,
        ) -> Result<ProcessResult, WorkflowError> {
            Ok(ProcessResult {
                code: Some(7),
                stdout: Some("out".into()),
                stderr: Some("err".into()),
            })
        }
        async fn run_script(
            &self,
            _: &str,
            _: &str,
            _: &[String],
            _: &HashMap<String, String>,
            _: Option<String>,
        ) -> Result<ProcessResult, WorkflowError> {
            Ok(ProcessResult {
                code: Some(0),
                stdout: Some("s".into()),
                stderr: None,
            })
        }
        async fn run_container(
            &self,
            _: &str,
            _: &[String],
            _: &HashMap<String, String>,
            _: Option<String>,
        ) -> Result<ProcessResult, WorkflowError> {
            Ok(ProcessResult {
                code: Some(0),
                stdout: Some("c".into()),
                stderr: None,
            })
        }
    }
    let runtime = Runtime::builder()
        .with_process(Arc::new(Fixed))
        .build()
        .unwrap();

    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1' }
do:
  - runIt:
      run:
        shell: { command: x }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!("out"));
}
#[tokio::test]
async fn switch_continue_and_end_directives() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: sw
  version: '1.0.0'
do:
  - pick:
      switch:
        - a:
            when: '.kind == "a"'
            then: continue
  - after:
      set: { done: true }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({ "kind": "a" })).await.unwrap();
    assert_eq!(out, json!({ "done": true }));
}

#[tokio::test]
async fn set_direct_expression() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - pick:
      input:
        from: .user.claims.subject
      set: '${ . }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime
        .run(wf, json!({"user":{"claims":{"subject":"abc"}}}))
        .await
        .unwrap();
    assert_eq!(out, json!("abc"));
}

#[tokio::test]
async fn try_catch_with_do_handler() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - attempt:
      try:
        - fail:
            raise:
              error:
                type: https://open-workflow-specification.org/spec/1.0.0/errors/runtime
                status: 500
                title: boom
      catch:
        errors:
          with: { status: 500 }
        as: err
        do:
          - handle:
              set:
                caught: '${ $err.title }'
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({"caught": "boom"}));
}

#[tokio::test]
async fn emit_uses_defaults_for_id_and_source() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: emit, version: '1.0.0' }
do:
  - e:
      emit:
        event:
          with:
            type: com.example.thing
            data: { x: 1 }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert!(!out["id"].as_str().unwrap().is_empty());
    assert!(out["source"].as_str().unwrap().contains("emit"));
    assert_eq!(out["data"]["x"], 1);
}

#[tokio::test]
async fn fork_compete_returns_single_winner() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - race:
      fork:
        compete: true
        branches:
          - a: { set: { v: 1 } }
          - b: { set: { v: 2 } }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert!(
        out.is_object(),
        "compete should return a single object, got {out}"
    );
    assert!(out["v"] == json!(1) || out["v"] == json!(2));
}

#[tokio::test]
async fn fork_empty_branches() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - fanout:
      fork:
        compete: false
        branches: []
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!([]));
}

#[tokio::test]
async fn run_script_and_container_modes() {
    #[derive(Clone)]
    struct R;
    #[async_trait::async_trait]
    impl ProcessRunner for R {
        async fn run_shell(
            &self,
            _: &str,
            _: &[String],
            _: &HashMap<String, String>,
            _: Option<String>,
        ) -> Result<ProcessResult, WorkflowError> {
            Ok(ProcessResult {
                code: Some(0),
                stdout: Some("s".into()),
                stderr: None,
            })
        }
        async fn run_script(
            &self,
            _l: &str,
            _c: &str,
            _a: &[String],
            _e: &HashMap<String, String>,
            _s: Option<String>,
        ) -> Result<ProcessResult, WorkflowError> {
            Ok(ProcessResult {
                code: Some(0),
                stdout: Some("script-out".into()),
                stderr: None,
            })
        }
        async fn run_container(
            &self,
            _i: &str,
            _a: &[String],
            _e: &HashMap<String, String>,
            _s: Option<String>,
        ) -> Result<ProcessResult, WorkflowError> {
            Ok(ProcessResult {
                code: Some(0),
                stdout: Some("container-out".into()),
                stderr: None,
            })
        }
    }
    let runtime = Runtime::builder()
        .with_process(Arc::new(R))
        .build()
        .unwrap();
    let script = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - runIt:
      run:
        script: { language: js, code: 'console.log(1)' }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&script).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!("script-out"));

    let container = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - runIt:
      run:
        container: { image: alpine }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&container).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!("container-out"));
}

#[tokio::test]
async fn try_catch_title_filter_and_when() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - attempt:
      try:
        - fail:
            raise:
              error:
                type: https://open-workflow-specification.org/spec/1.0.0/errors/runtime
                status: 500
                title: Boom
      catch:
        errors:
          with:
            title: Boom
        when: '$error.status == 500'
        do:
          - handle:
              set: { caught: true }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({"caught": true}));
}

#[tokio::test]
async fn emit_extension_and_missing_type_error() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: emit, version: '1.0.0' }
do:
  - e:
      emit:
        event:
          with:
            type: com.example.thing
            myext: somevalue
            data: { x: 1 }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out["myext"], "somevalue");
    assert_eq!(out["data"]["x"], 1);

    // Missing type must fault.
    let def2 = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: emit, version: '1.0.0' }
do:
  - e:
      emit:
        event:
          with:
            source: https://x
"#,
    )
    .unwrap();
    let wf2 = runtime.register_definition(&def2).unwrap();
    assert!(runtime.run(wf2, Value::Null).await.is_err());
}

#[tokio::test]
async fn raise_reference_error_with_detail() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: raise, version: '1.0.0' }
use:
  errors:
    myErr:
      type: https://open-workflow-specification.org/spec/1.0.0/errors/runtime
      title: Reference Error
      status: 503
      detail: service down
do:
  - r:
      raise:
        error: myErr
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.problem.title, "Reference Error");
    assert_eq!(err.problem.detail.as_deref(), Some("service down"));
}

#[tokio::test]
async fn reusable_function_referencing_unknown_fails() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: reuse, version: '1.0.0' }
use:
  functions:
    mySet:
      call: does-not-exist
do:
  - useIt:
      call: mySet
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ows_runtime_core::ErrorKind::Semantic);
}

#[tokio::test]
async fn listen_any_multiple_filters() {
    let broker = ows_runtime_events::InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(std::sync::Arc::new(broker.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: listen, version: '1.0.0' }
do:
  - waitEvent:
      listen:
        to:
          any:
            - with: { type: com.example.a }
            - with: { type: com.example.b }
"#,
    )
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let runtime2 = runtime.clone();
    let handle = tokio::spawn(async move { runtime2.run(wf, Value::Null).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    broker
        .publish(
            &ows_runtime_core::EventMessage::new("1", "src", "com.example.b")
                .with_data(json!({"n":2})),
        )
        .await
        .unwrap();
    let out = handle.await.unwrap().unwrap();
    assert_eq!(out, json!([{ "n": 2 }]));
}
