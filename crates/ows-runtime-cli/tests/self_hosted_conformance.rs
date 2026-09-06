//! Self-hosted deterministic conformance scenarios.
//!
//! The official OWS CTK requires an external specification checkout (see the
//! `deterministic_ctk_scenarios_pass` test). To keep conformance coverage
//! meaningful in offline CI, this suite runs the same Gherkin runner over a set
//! of in-repo `.feature` files that exercise core deterministic behaviors.
//! Keeping these green guards the conformance runner and the runtime's
//! deterministic task semantics without any external dependency.

use std::path::PathBuf;

fn features_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("features")
}

#[tokio::test]
async fn self_hosted_deterministic_scenarios_pass() {
    let dir = features_dir();
    let report = ows_runtime_cli::conformance::run_ctk_with(&dir, false).await;
    eprintln!(
        "Self-hosted conformance: {} pass, {} fail, {} skip",
        report.passes, report.failures, report.skips
    );
    assert_eq!(
        report.failures, 0,
        "self-hosted deterministic scenarios must pass, report: {report:#?}"
    );
    assert!(report.passes > 0, "at least one scenario should have run");
}
