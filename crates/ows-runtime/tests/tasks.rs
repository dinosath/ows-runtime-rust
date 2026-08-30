//! Integration tests for the OWS task types and runtime behaviors using the
//! deterministic testing helpers.

use ows_runtime_core::ServiceResponse;
use ows_runtime_testing::{test_runtime, FakeServiceInvoker};
use serde_json::{json, Value};

async fn run_workflow(
    runtime: &ows_runtime::Runtime,
    yaml: &str,
    input: Value,
) -> Result<Value, ows_runtime_core::WorkflowError> {
    let def = ows_runtime_dsl::from_yaml(yaml).expect("parse");
    let report = ows_runtime_dsl::validate(&def);
    assert!(report.is_valid(), "validation: {:?}", report.issues);
    let wf = runtime.register_definition(&def).expect("compile");
    runtime.run(wf, input).await
}

#[tokio::test]
async fn do_sequential_subtasks() {
    let runtime = test_runtime();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: do
  version: '1.0.0'
do:
  - composite:
      do:
        - setRed: { set: { colors: '${ .colors + ["red"] }' } }
        - setGreen: { set: { colors: '${ .colors + ["green"] }' } }
        - setBlue: { set: { colors: '${ .colors + ["blue"] }' } }
"#,
        json!({ "colors": [] }),
    )
    .await
    .unwrap();
    assert_eq!(out, json!({ "colors": ["red", "green", "blue"] }));
}

#[tokio::test]
async fn for_loop_accumulates() {
    let runtime = test_runtime();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: for
  version: '1.0.0'
do:
  - loop:
      for:
        each: color
        in: '.colors'
      do:
        - mark:
            set:
              processed: '${ { colors: (.processed.colors + [ $color ]), indexes: (.processed.indexes + [ $index ]) } }'
"#,
        json!({ "colors": ["red", "green", "blue"] }),
    )
    .await.unwrap();
    assert_eq!(
        out,
        json!({ "processed": { "colors": ["red", "green", "blue"], "indexes": [0, 1, 2] } })
    );
}

#[tokio::test]
async fn fork_parallel_non_compete() {
    let runtime = test_runtime();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: fork
  version: '1.0.0'
do:
  - fanout:
      fork:
        compete: false
        branches:
          - a: { set: { value: 1 } }
          - b: { set: { value: 2 } }
          - c: { set: { value: 3 } }
"#,
        Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(out, json!([{ "value": 1 }, { "value": 2 }, { "value": 3 }]));
}

#[tokio::test]
async fn switch_selects_case() {
    let runtime = test_runtime();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: switch
  version: '1.0.0'
do:
  - pick:
      switch:
        - a:
            when: '.kind == "a"'
            then: doA
        - b:
            when: '.kind == "b"'
            then: doB
        - default:
            then: doC
  - doA: { set: { result: 'A' }, then: end }
  - doB: { set: { result: 'B' }, then: end }
  - doC: { set: { result: 'C' }, then: end }
"#,
        json!({ "kind": "b" }),
    )
    .await
    .unwrap();
    assert_eq!(out, json!({ "result": "B" }));
}

#[tokio::test]
async fn try_catches_error() {
    let runtime = test_runtime();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: try
  version: '1.0.0'
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
            status: 500
        as: err
        do:
          - handle:
              set:
                caught: ${ $err.title }
"#,
        Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(out, json!({ "caught": "Boom" }));
}

#[tokio::test]
async fn raise_uncaught_faults() {
    let runtime = test_runtime();
    let err = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: raise
  version: '1.0.0'
do:
  - boom:
      raise:
        error:
          type: https://open-workflow-specification.org/spec/1.0.0/errors/runtime
          status: 418
          title: Teapot
"#,
        Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(err.problem.status, 418);
    assert_eq!(err.problem.title, "Teapot");
}

#[tokio::test]
async fn data_flow_input_output_transform() {
    let runtime = test_runtime();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: dataflow
  version: '1.0.0'
do:
  - pick:
      input:
        from: .user.claims.subject
      set:
        playerId: ${ . }
"#,
        json!({ "user": { "claims": { "subject": "abc123" } } }),
    )
    .await
    .unwrap();
    assert_eq!(out, json!({ "playerId": "abc123" }));
}

#[tokio::test]
async fn call_uses_fake_service() {
    let service = FakeServiceInvoker::new();
    service.stub(
        "GET",
        "https://example.com/pet/1",
        ServiceResponse {
            status: 200,
            headers: [("content-type".to_string(), "application/json".to_string())]
                .into_iter()
                .collect(),
            body: Some(json!({ "id": 1, "name": "rex" })),
            raw: None,
            content_type: Some("application/json".to_string()),
        },
    );
    let runtime = ows_runtime::Runtime::builder()
        .register_function(
            "http",
            std::sync::Arc::new(ows_runtime_testing::FakeHttpFunction::new(service)),
        )
        .build()
        .unwrap();
    let out = run_workflow(
        &runtime,
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: call
  version: '1.0.0'
do:
  - getPet:
      call: http
      with:
        method: get
        endpoint: https://example.com/pet/{petId}
"#,
        json!({ "petId": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(out.get("id"), Some(&json!(1)));
    assert_eq!(out.get("name"), Some(&json!("rex")));
}
