# Testing

Testing is a first-class requirement of the runtime. Tests are deterministic:
they use injectable clocks, RNG and UUIDs, an in-memory event broker, and fake
service invokers, so they never depend on real network, sleeping, or wall-clock
timing.

## Running tests

```sh
cargo test --workspace --all-features
cargo test --workspace --no-default-features
cargo test --workspace                     # default features
```

## Test layout

- **Unit tests** live next to each component (e.g. `ows-runtime-expressions`
  has lexer/parser/evaluator tests).
- **Integration tests** exercise complete workflow execution
  (`crates/ows-runtime/tests/`).
- **Property tests** use `proptest` for expression evaluation
  (`ows-runtime-expressions/tests/property.rs`).
- **Conformance tests** run the OWS CTK (`docs/conformance.md`).
- **Regression tests** are added for every discovered bug.

## Deterministic helpers

The `ows-runtime-testing` crate provides:

- `test_runtime()` — a runtime with a deterministic clock and seeded RNG.
- `FakeServiceInvoker` — scripted `ServiceInvoker` responses for `call` tests.
- `FakeHttpFunction` — drives the `http` function through a fake service.
- `RecordingEventPublisher` — captures emitted events.
- `TestClock` — explicit virtual time control.

## Error-path testing

At least half of execution tests exercise failure behavior: task faults, timeout,
cancellation, retry exhaustion, invalid expressions, missing variables, and
policy violations. Errors remain typed and structured (`WorkflowError` /
`ProblemDetails`).

## Determinism

Business logic never calls `Instant::now()`, `Uuid::new_v4()` or `sleep()`
directly. All timing and randomness go through the injected `Clock`,
`RandomGenerator` and `UuidGenerator`, so tests are reproducible.
