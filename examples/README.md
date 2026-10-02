# Examples

Runnable example binaries live in the `ows-runtime-examples` package. Every
binary has its own `main`; workflow loading and runtime setup are shared by
`examples/src/lib.rs`.

- `run_workflow` — parses, compiles and executes a small OWS workflow.
  `cargo run -p ows-runtime-examples --bin run-workflow`

The workflow is available in both
[`run_workflow.yaml`](workflows/run_workflow.yaml) and
[`run_workflow.json`](workflows/run_workflow.json) form.

Standalone task workflow definitions are also available in
[`examples/workflows`](workflows/): `set.yaml`, `do.yaml`, `for.yaml`,
`fork.yaml`, `switch.yaml`, `try.yaml`, `emit.yaml`, `listen.yaml`,
`raise.yaml`, `call.yaml`, `run.yaml`, and `wait.yaml`. They are definition
fixtures, each with a matching `workflow-*` binary. For example:

```sh
cargo run -p ows-runtime-examples --bin workflow-set
cargo run -p ows-runtime-examples --bin workflow-listen
```

For a larger set of workflow definitions (the official OWS examples), see
`tests/fixtures` and the `OWS_SPEC_REPO`-driven test in
`crates/ows-runtime/tests/examples.rs`.
