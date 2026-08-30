//! Basic integration tests exercising the public runtime API against the OWS
//! CTK `set` scenario.

use ows_runtime::Runtime;
use serde_json::{json, Value};

fn build_runtime() -> Runtime {
    Runtime::builder().build().expect("runtime should build")
}

#[tokio::test]
async fn set_task_scenario() {
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: set
  version: '1.0.0'
do:
  - setShape:
      set:
        shape: circle
        size: ${ .configuration.size }
        fill: ${ .configuration.fill }
"#,
    )
    .expect("should parse");
    let report = ows_runtime_dsl::validate(&def);
    assert!(report.is_valid(), "validation issues: {:?}", report.issues);

    let runtime = build_runtime();
    let wf = runtime.register_definition(&def).expect("should compile");

    let input = json!({
        "configuration": {
            "size": { "width": 6, "height": 6 },
            "fill": { "red": 69, "green": 69, "blue": 69 }
        }
    });

    let output = runtime.run(wf, input).await.expect("should run");
    let expected: Value = serde_yaml::from_str(
        r#"
shape: circle
size:
  width: 6
  height: 6
fill:
  red: 69
  green: 69
  blue: 69
"#,
    )
    .unwrap();
    assert_eq!(output, expected);
}

#[tokio::test]
async fn implicit_sequence_flow() {
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: implicit-sequence
  version: '1.0.0'
do:
  - setRed:
      set:
        colors: '${ .colors + [ "red" ] }'
  - setGreen:
      set:
        colors: '${ .colors + [ "green" ] }'
  - setBlue:
      set:
        colors: '${ .colors + [ "blue" ] }'
"#,
    )
    .expect("parse");
    assert!(ows_runtime_dsl::validate(&def).is_valid());

    let runtime = build_runtime();
    let wf = runtime.register_definition(&def).expect("compile");
    let output = runtime.run(wf, json!({ "colors": [] })).await.expect("run");
    assert_eq!(output, json!({ "colors": ["red", "green", "blue"] }));
}

#[tokio::test]
async fn raise_faults_workflow() {
    let def = ows_runtime_dsl::from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: raise-custom-error
  version: '1.0.0'
do:
  - raiseError:
      raise:
        error:
          status: 400
          type: https://open-workflow-specification.org/errors/types/compliance
          title: Compliance Error
"#,
    )
    .expect("parse");
    assert!(ows_runtime_dsl::validate(&def).is_valid());

    let runtime = build_runtime();
    let wf = runtime.register_definition(&def).expect("compile");
    let err = runtime
        .run(wf, Value::Null)
        .await
        .expect_err("should fault");
    assert_eq!(err.problem.status, 400);
    assert_eq!(
        err.problem.type_,
        "https://open-workflow-specification.org/errors/types/compliance"
    );
    assert_eq!(err.problem.title, "Compliance Error");
}
