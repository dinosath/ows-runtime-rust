# Vendored OWS Conformance Test Kit

These `.feature` files are the official OWS Conformance Test Kit, vendored from
the Open Workflow Specification repository so the data-driven conformance suite
runs in offline CI (previously it was skipped without an external checkout).

- Source: https://github.com/open-workflow-specification/specification
- Path: `ctk/features`
- Revision: `2dd2c84170d5f3e05d58e913e9ca298dcf8d543a` (2026-08-13)
- License: Apache-2.0

The runner (`ows-runtime-cli/src/conformance.rs`) executes them;
`crates/ows-runtime-cli/tests/conformance.rs` asserts the deterministic
(non-network) scenarios pass. Set `CTK_DIR` or `OWS_SPEC_REPO` to test against a
different revision.
