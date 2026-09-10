//! Data-driven workflow tests.
//!
//! Each `*.yaml` file under `tests/fixtures/cases/` is a self-contained case
//! describing a workflow (inline or a reference to a vendored official OWS
//! example), the workflow input, any secrets, and the expected outcome. The
//! runner below executes every case so adding coverage means adding data, not
//! code.
//!
//! Case schema:
//!
//! ```yaml
//! name: human readable name
//! workflow: { ... an OWS definition ... }     # or: workflow_file: relative/path.yaml
//! input: { ... }                              # optional (default: null)
//! secrets: { name: value }                    # optional
//! expect:
//!   output: { ... }                           # exact output (optional)
//!   contains: { ... }                         # deep-subset of output (optional)
//!   error: true                               # expect a fault (optional)
//!   error_kind: semantic                      # expected ErrorKind (optional)
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ows_runtime::dsl_models::WorkflowDefinition;
use ows_runtime::Runtime;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    #[serde(default)]
    workflow: Option<Value>,
    #[serde(default)]
    workflow_file: Option<String>,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    secrets: HashMap<String, String>,
    expect: Expect,
}

#[derive(Debug, Deserialize)]
struct Expect {
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    contains: Option<Value>,
    #[serde(default)]
    error: bool,
    #[serde(default)]
    error_kind: Option<String>,
}

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cases")
}

fn load_definition(case: &Case, dir: &Path) -> WorkflowDefinition {
    let raw: Value = match (&case.workflow, &case.workflow_file) {
        (Some(w), None) => w.clone(),
        (None, Some(file)) => {
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("case `{}`: read {}: {e}", case.name, path.display()));
            serde_yaml::from_str(&text)
                .unwrap_or_else(|e| panic!("case `{}`: parse {}: {e}", case.name, path.display()))
        }
        _ => panic!(
            "case `{}` must set exactly one of `workflow` / `workflow_file`",
            case.name
        ),
    };
    // Route through the DSL so OWS default normalization is applied.
    let json = serde_json::to_string(&raw).unwrap();
    ows_runtime_dsl::from_json(&json)
        .unwrap_or_else(|e| panic!("case `{}`: definition invalid: {e}", case.name))
}

fn deep_contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e
            .iter()
            .all(|(k, ev)| a.get(k).map(|av| deep_contains(av, ev)).unwrap_or(false)),
        (Value::Array(a), Value::Array(e)) => {
            a.len() >= e.len() && e.iter().zip(a).all(|(ev, av)| deep_contains(av, ev))
        }
        _ => actual == expected,
    }
}

async fn run_case(case: &Case, dir: &Path) {
    let def = load_definition(case, dir);
    let report = ows_runtime_dsl::validate(&def);
    assert!(
        report.is_valid(),
        "case `{}`: validation failed: {:?}",
        case.name,
        report.issues
    );

    let runtime = Runtime::builder()
        .with_secrets(case.secrets.clone())
        .build()
        .unwrap();
    let wf = runtime
        .register_definition(&def)
        .unwrap_or_else(|e| panic!("case `{}`: compile failed: {e}", case.name));

    let result = runtime.run(wf, case.input.clone()).await;

    if case.expect.error {
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("case `{}`: expected a fault", case.name),
        };
        if let Some(kind) = &case.expect.error_kind {
            assert_eq!(
                err.kind.as_str(),
                kind,
                "case `{}`: wrong error kind",
                case.name
            );
        }
        assert!(
            case.expect.output.is_none() && case.expect.contains.is_none(),
            "case `{}`: fault cases cannot assert output",
            case.name
        );
    } else {
        let out = result.unwrap_or_else(|e| panic!("case `{}`: faulted: {e}", case.name));
        if let Some(expected) = &case.expect.output {
            assert_eq!(out, *expected, "case `{}`: output mismatch", case.name);
        }
        if let Some(expected) = &case.expect.contains {
            assert!(
                deep_contains(&out, expected),
                "case `{}`: output {out} does not contain {expected}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn data_driven_cases() {
    let dir = cases_dir();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yaml") | Some("yml") | Some("json")
            )
        })
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no data-driven cases found in {}",
        dir.display()
    );

    let mut ran = 0;
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        let case: Case = serde_yaml::from_str(&text)
            .unwrap_or_else(|e| panic!("parse case {}: {e}", path.display()));
        run_case(&case, &dir).await;
        ran += 1;
    }
    eprintln!("data-driven cases ran: {ran}");
    assert!(
        ran >= 8,
        "expected at least 8 data-driven cases, found {ran}"
    );
}
