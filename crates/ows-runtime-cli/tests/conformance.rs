//! Conformance test for the OWS CTK.
//!
//! This test runs the deterministic (non-network) CTK scenarios against the
//! runtime. It needs a checkout of the OWS specification repository, supplied
//! via the `CTK_DIR` or `OWS_SPEC_REPO` environment variables. If neither is
//! set, the test is skipped so it does not fail in offline CI.

use std::path::PathBuf;

fn ctk_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CTK_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(repo) = std::env::var("OWS_SPEC_REPO") {
        return Some(PathBuf::from(repo).join("ctk").join("features"));
    }
    // The official CTK features are vendored so this data-driven suite runs in
    // offline CI. See `tests/ctk/README.md`.
    let vendored = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("ctk")
        .join("features");
    if vendored.exists() {
        return Some(vendored);
    }
    let local = PathBuf::from("specification/ctk/features");
    if local.exists() {
        return Some(local);
    }
    None
}

#[tokio::test]
async fn deterministic_ctk_scenarios_pass() {
    let Some(dir) = ctk_dir() else {
        eprintln!("CTK not found; skipping conformance test");
        return;
    };
    let report = ows_runtime_cli::conformance::run_ctk_with(&dir, false).await;
    eprintln!(
        "Conformance: {} pass, {} fail, {} skip",
        report.passes, report.failures, report.skips
    );
    assert_eq!(
        report.failures, 0,
        "deterministic CTK scenarios must pass, failures: {report:#?}"
    );
    assert!(report.passes > 0, "at least one scenario should have run");
}
