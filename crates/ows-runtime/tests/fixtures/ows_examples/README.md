# Vendored official OWS examples

These `*.yaml` workflow definitions are the official examples from the Open
Workflow Specification repository, vendored so the data-driven examples suite
(`crates/ows-runtime/tests/examples.rs`) and the case suite
(`crates/ows-runtime/tests/fixtures/cases/`) run in offline CI.

- Source: https://github.com/open-workflow-specification/specification
- Path: `examples`
- Revision: `2dd2c84170d5f3e05d58e913e9ca298dcf8d543a` (2026-08-13)
- License: Apache-2.0

Every example is validated and compiled; deterministic examples are executed.
Set `OWS_SPEC_REPO` to run against a different revision instead.
