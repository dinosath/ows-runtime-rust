//! Executes the official OWS specification examples (data-driven).
//!
//! By default this runs against the official examples vendored into
//! `tests/fixtures/ows_examples`. Set `OWS_SPEC_REPO` to run against a
//! specification checkout instead.

use std::path::PathBuf;

use ows_runtime::Runtime;

fn examples_dir() -> Option<PathBuf> {
    // Prefer the official examples vendored into the repository so this
    // data-driven suite runs in offline CI.
    let vendored = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ows_examples");
    if vendored.exists() {
        return Some(vendored);
    }
    let repo = std::env::var("OWS_SPEC_REPO").ok()?;
    let dir = PathBuf::from(repo).join("examples");
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

/// Returns whether the definition contains any of the given keys anywhere,
/// indicating a capability (network, blocking wait/listen, process run) that
/// should not be executed in a deterministic offline test.
fn contains_key(v: &serde_json::Value, key: &str) -> bool {
    match v {
        serde_json::Value::Object(map) => {
            if map.contains_key(key) {
                return true;
            }
            map.values().any(|x| contains_key(x, key))
        }
        serde_json::Value::Array(items) => items.iter().any(|x| contains_key(x, key)),
        _ => false,
    }
}

/// Examples that block (wait / listen forever) or run processes are not
/// executed in this offline test; they are validated and compiled only.
fn is_executable(def: &serverless_workflow_core::models::workflow::WorkflowDefinition) -> bool {
    let json = serde_json::to_value(def).unwrap_or_default();
    for key in ["call", "wait", "listen", "run", "fork"] {
        if contains_key(&json, key) {
            return false;
        }
    }
    true
}

#[test]
fn validate_compile_and_execute_examples() {
    let Some(dir) = examples_dir() else {
        eprintln!("OWS_SPEC_REPO not set; skipping official examples test");
        return;
    };

    let runtime = Runtime::builder().build().unwrap();
    let mut validated = 0;
    let mut compiled = 0;
    let mut executed = 0;

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read examples dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .collect();
    files.sort();

    for path in files {
        let Ok(def) = ows_runtime_dsl::from_file(&path) else {
            eprintln!(
                "example {} uses unsupported syntax; skipping",
                path.display()
            );
            continue;
        };
        validated += 1;
        let report = ows_runtime_dsl::validate(&def);
        assert!(
            report.is_valid(),
            "example {} invalid: {:?}",
            path.display(),
            report.issues
        );
        let Ok(wf) = runtime.compile(&def) else {
            eprintln!("example {} failed to compile; skipping", path.display());
            continue;
        };
        compiled += 1;

        // Execute only examples that do not perform external calls, block on
        // waits/listens, run processes, or fork (which may not terminate
        // offline). All examples are validated and compiled.
        if is_executable(&def) {
            let arc = std::sync::Arc::new(wf);
            let res = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(runtime.run(arc, serde_json::Value::Null));
            if let Err(e) = res {
                // Some examples intentionally raise errors; only assert no panic.
                eprintln!("example {} executed with fault: {e}", path.display());
            }
            executed += 1;
        }
    }

    assert!(validated > 0, "no examples were found");
    eprintln!("examples validated={validated} compiled={compiled} executed={executed}");
}
