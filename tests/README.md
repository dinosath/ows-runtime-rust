# Workspace-level tests

This directory holds workspace-level test resources. Because the workspace is
virtual (no root crate), most executable tests live in the individual crates
(see `crates/*/tests/` and `crates/*/src/`). This directory keeps conformance,
integration, regression and fixture assets organized.

- `conformance/` — CTK runner notes (see `docs/conformance.md`).
- `integration/` — workflow execution fixtures/examples.
- `fixtures/` — sample OWS workflow definitions.
- `regression/` — regression scenarios.
