# ows-runtime-rust

A production-grade, library-first Rust runtime for the
[Open Workflow Specification](https://open-workflow-specification.org/) (OWS).

`ows-runtime-rust` loads OWS workflow definitions (YAML/JSON), validates them,
compiles them into an executable internal representation, and executes them
asynchronously with support for runtime expressions, tasks, events, scheduling,
retries, timeouts, cancellation and observability. The execution engine is
independent of HTTP frameworks, databases, message brokers and cloud providers;
those are adapters behind stable core traits.

## Status

- **Specification compliant**: core DSL model mirrors OWS **1.0.3**.
- **Implemented**: all twelve OWS task types (`set`, `do`, `for`, `fork`,
  `switch`, `try`, `emit`, `listen`, `raise`, `run`, `call`, `wait`), the jq
  runtime expression engine, data-flow pipeline, retries, timeouts, flow
  directives, lifecycle events, and an in-memory event broker. The `listen`
  task supports `one`/`any`/`all` consumption with `foreach` iteration and
  correlation grouping, and the `call` task bundles HTTP, OpenAPI, gRPC,
  AsyncAPI, A2A and MCP function adapters. The full `use` component collection
  is honoured: reusable functions/errors/retries/timeouts, `extensions`,
  `catalogs` and `secrets`.
- **Scheduling**: `schedule.every`/`after`/`cron`/`on` triggers are executed
  end-to-end via `Runtime::start_schedules`.
- **Conformance**: the vendored deterministic OWS CTK scenarios (12/12) pass.
  Network scenarios are skipped by default and can be enabled with
  `--include-network`.
- **Durability & observability**: durable `ExecutionStore` adapters (SQLite,
  PostgreSQL, Redis) live in `ows-runtime-stores`, and the engine persists each
  execution's start record, per-task checkpoints, terminal phase and lifecycle
  events through the configured store, so interrupted executions can be resumed
  with `Runtime::resume`. `ows-runtime-observability-otel` exports lifecycle
  logs, spans and metrics over OTLP/HTTP JSON, and OTLP/gRPC behind the
  `otlp-grpc` feature.
- **Experimental / opt-in transports**: the `grpc` adapter targets gRPC services
  exposed through a JSON/gateway endpoint, and (behind the `grpc-native`
  feature) natively over protobuf/HTTP-2 by compiling the workflow's `.proto` at
  runtime. The AsyncAPI adapter publishes over an HTTP channel binding or a
  broker-based transport. Container/script/shell `run` processes are
  deny-by-default behind the `ProcessRunner` policy.

See [`docs/specification-coverage.md`](docs/specification-coverage.md) for the
full feature matrix.

## Architecture

This is a Cargo workspace. The runtime is library-first; a thin CLI and a
conformance runner sit on top of the library.

```text
ows-runtime/
├── crates/
│   ├── ows-runtime-core/          # core traits, errors, policy, clocks, stores
│   ├── ows-runtime-expressions/   # sandboxed jq-subset expression engine
│   ├── ows-runtime-dsl/           # parsing + validation on the official SDK
│   ├── ows-runtime-events/        # CloudEvents, matching, in-memory broker
│   ├── ows-runtime-scheduler/     # scheduling abstractions + cron
│   ├── ows-runtime-observability/ # tracing + lifecycle event emission
│   ├── ows-runtime-observability-otel/ # OTLP/HTTP + OTLP/gRPC exporters
│   ├── ows-runtime-stores/        # durable ExecutionStore adapters (SQLite/Postgres/Redis)
│   ├── ows-runtime/               # compilation + execution engine
│   ├── ows-runtime-testing/       # deterministic test helpers and fakes
│   └── ows-runtime-cli/           # validate / compile / run / conformance
├── tests/
│   ├── conformance/               # CTK runner and fixtures
│   ├── integration/               # workflow execution tests
│   └── regression/                # regression tests
├── benches/                       # criterion benchmarks
└── docs/
```

The definition model uses the official
[`serverless_workflow_core`](https://github.com/open-workflow-specification/sdk-rust)
SDK as the primary representation of OWS workflow definitions. The runtime
introduces its own optimized, executable IR while preserving the exact semantics
of the original definition.

See [`docs/architecture.md`](docs/architecture.md) and
[`docs/execution-model.md`](docs/execution-model.md) for details.

## Supported OWS version

`1.0.3`

## Installation

```sh
cargo add --git https://github.com/open-workflow-specification/sdk-rust  # pinned by the workspace
# then use the workspace crates
```

Or clone this repository and build the workspace:

```sh
git clone https://github.com/open-workflow-specification/sdk-rust  # not required; SDK is a git dep
cargo build --workspace
cargo test --workspace
```

## Basic usage

```rust,ignore
use ows_runtime::Runtime;

let runtime = Runtime::builder()
    .register_task(...)     // custom functions, services
    .build()?;

let definition = ows_runtime_dsl::from_yaml(yaml)?;
let workflow = runtime.register_definition(&definition)?;

let output = runtime
    .run(workflow, input)
    .await?;
```

## Example workflow

```yaml
document:
  dsl: '1.0.3'
  namespace: examples
  name: greet
  version: '0.1.0'
do:
  - buildGreeting:
      set:
        greeting: '${ "Hello, \(.name)!" }'
```

```rust,ignore
let def = ows_runtime_dsl::from_yaml(yaml)?;
let report = ows_runtime_dsl::validate(&def);
assert!(report.is_valid());

let runtime = Runtime::builder().build()?;
let wf = runtime.register_definition(&def)?;
let out = runtime.run(wf, json!({ "name": "World" })).await?;
assert_eq!(out, json!({ "greeting": "Hello, World!" }));
```

## CLI

```sh
# A thin wrapper around the library.
cargo run -p ows-runtime-cli -- validate workflow.yaml
cargo run -p ows-runtime-cli -- compile workflow.yaml
cargo run -p ows-runtime-cli -- run workflow.yaml --input input.yaml
cargo run -p ows-runtime-cli -- conformance --ctk <path-to-ctk-features>
```

## Testing

```sh
cargo test --workspace --all-features
cargo test --workspace --no-default-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test --test conformance   # runs the CTK runner
```

Line coverage of the runtime library crates is measured with `cargo-llvm-cov` and
enforced at **>= 90%** in CI (`./scripts/coverage.sh`).

The runtime is deterministic in tests: clocks, RNG and UUIDs are injectable, and
the event subsystem uses an in-memory broker. See
[`docs/testing.md`](docs/testing.md).

## Conformance status

The runtime passes the deterministic OWS Conformance Test Kit scenarios and
executes the network-dependent scenarios when enabled. See
[`docs/conformance.md`](docs/conformance.md) for the report format and how to
pin the CTK version.

## Roadmap

Roadmap items implemented:

- gRPC, AsyncAPI, A2A and MCP `call` function adapters (bundled). gRPC supports
  both a JSON/gateway endpoint and, behind the `grpc-native` feature, a native
  protobuf/HTTP-2 transport that compiles the workflow's `.proto` at runtime
  with `protox` and calls it with dynamic messages (`prost-reflect`) over
  `tonic`. AsyncAPI supports an HTTP channel binding **or** a broker-based
  transport using the runtime's event publisher/consumer.
- `use.extensions`: extension tasks run `before`/`after` every task of the
  targeted type, guarded by `when`, with `then: exit` short-circuiting the
  extended task.
- `use.catalogs`: reusable components are imported from catalog endpoints via a
  `CatalogResolver` (static, filesystem and HTTP resolvers bundled), including
  nested catalogs.
- `use.secrets`: declared secrets are resolved through a `SecretResolver`
  (in-memory or environment-backed) and exposed as `$secrets`; references to
  undeclared secrets are rejected at compile time.
- End-to-end scheduling: `schedule.every`/`after`/`cron`/`on` triggers fire
  executions via `Runtime::start_schedules` on the injectable clock and event
  consumer, with a cancellable `ScheduleSet`.
- Process execution: an opt-in `TokioProcessRunner` runs `run` shell/script
  tasks (and containers via Docker). The default remains deny-by-default.
- Durable execution via `ExecutionStore` adapters in `ows-runtime-stores`
  (SQLite by default; PostgreSQL and Redis behind the `postgres`/`redis`
  features), including per-task **checkpointing and `Runtime::resume`**.
- OpenTelemetry integration crate (`ows-runtime-observability-otel`) that
  exports lifecycle events as OTLP/HTTP JSON **logs, spans and metrics**, plus
  OTLP/gRPC exporters behind the `otlp-grpc` feature.
- `listen` `one`/`any`/`all` with `foreach`, the `until` stop condition,
  correlation `from`/`expect` evaluated against the workflow context, and
  cross-event correlation *grouping* (first-seen key values shared across
  filters).
- Data-driven testing: the official OWS CTK features and examples are vendored
  and run in offline CI, alongside a case-file suite for extension, catalog,
  secret, scheduling, listen, flow and data-flow behavior.

Remaining / future work:

- Live CTK network scenarios (the network-dependent CTK scenarios are skipped by
  default and can be enabled with `--include-network`).


## License

MIT. The official OWS SDK (`serverless_workflow_core`) is Apache-2.0 and is used
as a git dependency.
