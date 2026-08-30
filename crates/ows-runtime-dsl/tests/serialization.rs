//! Serialization round-trip tests: YAML → Rust → JSON → Rust → YAML.

use ows_runtime_dsl::{from_json, from_yaml, to_json, to_yaml, validate};

const WORKFLOW: &str = r#"
document:
  dsl: '1.0.3'
  namespace: test
  name: roundtrip
  version: '0.1.0'
use:
  errors:
    myError:
      type: https://example.com/errors/thing
      title: Thing Failed
      status: 500
  retries:
    myRetry:
      limit:
        attempt:
          count: 3
do:
  - stepOne:
      set:
        a: 1
        b: '${ .input.x }'
  - stepTwo:
      call: http
      with:
        method: get
        endpoint: https://example.com
"#;

#[test]
fn yaml_to_json_to_yaml_roundtrip() {
    let def = from_yaml(WORKFLOW).expect("parse yaml");
    let report = validate(&def);
    assert!(report.is_valid(), "validation: {:?}", report.issues);

    let json = to_json(&def).expect("to json");
    let from_json_def = from_json(&json).expect("parse json");

    let yaml_again = to_yaml(&from_json_def).expect("to yaml");
    let from_yaml_again = from_yaml(&yaml_again).expect("parse yaml again");

    // The re-parsed definition should be semantically equal.
    assert_eq!(from_json_def.document.name, def.document.name);
    assert_eq!(from_yaml_again.do_.entries.len(), def.do_.entries.len());
}

#[test]
fn parse_and_validate_valid_definitions() {
    let def = from_yaml(WORKFLOW).unwrap();
    let report = validate(&def);
    assert!(report.is_valid());
}

#[test]
fn malformed_yaml_is_parse_error() {
    let err = from_yaml("do: [").unwrap_err();
    assert!(err.is_parse());
}

#[test]
fn unknown_task_is_parse_error() {
    // A task with no recognized type keyword fails deserialization.
    let err = from_yaml(
        r#"
document:
  dsl: '1.0.3'
  namespace: t
  name: w
  version: '0.1.0'
do:
  - weird:
      frobnicate: 42
"#,
    )
    .unwrap_err();
    assert!(err.is_parse());
}
