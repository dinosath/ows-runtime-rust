# Conformance Testing

This document describes how the runtime runs the official OWS Conformance Test
Kit (CTK).

## Overview

The CTK is a suite of Gherkin feature files in the OWS specification repository
(`ctk/features`). Each scenario defines a workflow, an optional input, executes
it, and asserts on the outcome (completion, output, faults, task ordering).

The runtime does **not** vendor the CTK. Instead, it pins the specification
repository (and thus the CTK) to a specific version and executes against that
path.

## Running

```sh
# Deterministic scenarios (no network).
cargo run -p ows-runtime-cli -- conformance --ctk <path-to-ctk/features>

# Include network-dependent scenarios (requires network access).
cargo run -p ows-runtime-cli -- conformance --ctk <path-to-ctk/features> --include-network

# Write a machine-readable JSON report.
cargo run -p ows-runtime-cli -- conformance --ctk <path> --output report.json
```

Set `OWS_SPEC_REPO` to the specification repository checkout if you do not pass
`--ctk`:

```sh
export OWS_SPEC_REPO=/path/to/specification
cargo run -p ows-runtime-cli -- conformance
```

There is also a library API:

```rust,ignore
let report = ows_runtime_cli::conformance::run_ctk_with(
    std::path::Path::new("/path/to/ctk/features"),
    /* include_network */ false,
).await;
```

## What passes

The deterministic CTK scenarios cover `set`, `do`, `for`, `fork`, `switch`,
`try`, `raise`, `emit`, flow directives, and data flow. These pass 12/12 with no
failures.

Network-dependent scenarios (`call` to petstore/httpbin, and HTTP-based
`data-flow`/`try` scenarios) are skipped by default to keep CI deterministic.
When `--include-network` is passed they are executed against the live endpoints,
which is inherently non-deterministic (the live services' data changes over
time).

## In-repo self-hosted scenarios

The official CTK lives in the specification repository and is not vendored, so
the `deterministic_ctk_scenarios_pass` test is skipped when no checkout is
present. To keep conformance coverage meaningful in offline CI,
`crates/ows-runtime-cli/tests/features/` contains a small deterministic Gherkin
suite (`set`, `for`, `fork`, `switch`). The
`self_hosted_deterministic_scenarios_pass` test runs it through the same
Gherkin runner, guarding the runner and the runtime's deterministic task
semantics without any external dependency.

## Report format

The report is serialized to JSON:

```json
{
  "scenarios": [
    { "feature": "...", "name": "...", "status": "Pass|Fail|Skip", "message": null }
  ],
  "passes": 12,
  "failures": 0,
  "skips": 9,
  "ctk_version": null
}
```

## Known CTK / spec discrepancies

- The CTK `try.feature` filter uses the outdated error type URL
  `https://open-workflow-specification.org/dsl/errors/types/communication`. The
  runtime emits the current standard type
  `https://open-workflow-specification.org/spec/1.0.0/errors/communication`, so
  those `try` scenarios do not match the filter. The specification is treated as
  authoritative.

## CI

Conformance runs are part of CI (see `.github/workflows/ci.yml`). They are
pinned to the specification repository revision documented in the workflow.
