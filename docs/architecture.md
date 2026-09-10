# Architecture

## Overview

`ows-runtime-rust` is a library-first Cargo workspace. The execution engine is
kept independent of HTTP frameworks, databases, message brokers and cloud
providers; those are adapters behind stable core traits defined in
`ows-runtime-core`.

## Crate layout

| Crate | Responsibility |
|-------|----------------|
| `ows-runtime-core` | Core traits (`Clock`, `UuidGenerator`, `RandomGenerator`, `ExecutionStore`, `ServiceInvoker`, `EventPublisher`, `EventConsumer`, `Scheduler`, `ExpressionEngine`), typed errors (`ProblemDetails`, `WorkflowError`), `RuntimePolicy`, status `Phase`. |
| `ows-runtime-dsl` | Parsing (YAML/JSON), schema/semantic validation, and default normalization on top of the official `serverless_workflow_core` SDK. |
| `ows-runtime-expressions` | A fully sandboxed `jq`-subset interpreter. No `eval`, no shell, no arbitrary code. |
| `ows-runtime-events` | CloudEvents shape, event matching/correlation, and an in-memory broker implementing `EventPublisher`/`EventConsumer`. |
| `ows-runtime-scheduler` | Scheduling abstractions plus a cron-backed schedule parser. |
| `ows-runtime-observability` | `tracing` setup and OWS lifecycle cloud-event emission. |
| `ows-runtime` | Compilation pipeline and the scope-based execution engine, all task types, HTTP/OpenAPI function invokers. |
| `ows-runtime-testing` | Deterministic test helpers and fakes. |
| `ows-runtime-cli` | Thin CLI: `validate`, `compile`, `run`, `conformance`. |

## Compilation pipeline

```
YAML / JSON
   │  Deserialize (+ default normalization)
   ▼
OWS Definition (serverless_workflow_core)
   │  Catalog resolution (`use.catalogs`, optional)
   │  Schema / semantic validation
   ▼
Validation report
   │  Runtime compilation
   ▼
Executable IR (CompiledWorkflow / CompiledTask)
   │  Execution
   ▼
Execution
```

Error categories are never collapsed:

- **Parse errors** — the input cannot be decoded.
- **Schema errors** — the definition violates the OWS schema.
- **Semantic errors** — structurally valid but not executable per OWS semantics.
- **Runtime errors** — the workflow is valid but execution failed.

## Definition model

The workflow definition model uses the official
[`serverless_workflow_core`](https://github.com/open-workflow-specification/sdk-rust)
SDK as the primary representation, so the runtime does not duplicate the OWS DSL
model. The compiler walks the SDK types and produces an optimized, executable IR
(`CompiledWorkflow`, `CompiledTask`, `CompiledTaskKind`) that is designed for
execution rather than serialization.

## Execution engine

The engine is a scope-based state machine. Each task flows through the OWS
data-flow pipeline (validate input → transform input → execute → transform
output → validate output → export → validate context). Execution state is
explicit: every execution and every task has a stable identifier, and mutable
state is shared via `Arc` so structured concurrency (fork branches) is safe.

See [`execution-model.md`](execution-model.md).

## Feature flags

The `ows-runtime` crate exposes:

- `default = ["http", "validation"]`
- `http` — the built-in HTTP and OpenAPI function invokers.
- `validation` — JSON Schema input/output validation.

`cargo test --workspace --no-default-features` and
`cargo test --workspace --all-features` both work.

## Determinism

Clocks, RNG, UUIDs, sleeping and external services are injectable. Business
logic never calls `Instant::now()`, `Uuid::new_v4()` or `sleep()` directly;
everything goes through injected dependencies so workflow timing is
deterministic in tests.
