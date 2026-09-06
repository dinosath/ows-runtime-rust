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
  task supports `one`/`any`/`all` consumption with `foreach` iteration, and the
  `call` task bundles HTTP, OpenAPI, gRPC, AsyncAPI, A2A and MCP function
  adapters.
- **Conformance**: the deterministic OWS CTK scenarios (12/12) pass. Network
  scenarios are skipped by default and can be enabled with `--include-network`.
- **Durability & observability**: durable `ExecutionStore` adapters (SQLite,
  PostgreSQL, Redis) live in `ows-runtime-stores`, and the engine persists each
  execution's start record, terminal phase and lifecycle events through the
  configured store. An OpenTelemetry OTLP/HTTP JSON exporter for lifecycle
  events lives in `ows-runtime-observability-otel`.
- **Experimental / opt-in transports**: the gRPC adapter targets gRPC services
  exposed through a JSON/gateway endpoint, and the AsyncAPI adapter publishes
  over an HTTP channel binding (no protobuf codegen or broker adapters are
  bundled). Container/script/shell `run` processes are deny-by-default behind
  the `ProcessRunner` policy.

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
│   ├── ows-runtime-observability-otel/ # OTLP/HTTP JSON exporter
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

- gRPC, AsyncAPI, A2A and MCP `call` function adapters (bundled; gRPC via a
  JSON/gateway endpoint and AsyncAPI over an HTTP channel binding).
- Durable execution via `ExecutionStore` adapters in `ows-runtime-stores`
  (SQLite by default; PostgreSQL and Redis behind the `postgres`/`redis`
  features).
- OpenTelemetry integration crate (`ows-runtime-observability-otel`) that
  exports lifecycle events over OTLP/HTTP JSON.
- `listen` `foreach` iteration (and gathering multiple events for `all`).
- New scenario coverage: offline, deterministic tests for `listen` `foreach`/`all`
  and the gRPC/AsyncAPI/A2A/MCP call functions, plus an in-repo self-hosted
  Gherkin conformance suite (`set`/`for`/`fork`/`switch`) that runs through the
  same CTK runner in normal CI (see [`docs/conformance.md`](docs/conformance.md)).

Remaining / future work:

- Wiring a durable store into long-running execution *resume* (checkpoint and
  restart) on top of the existing `ExecutionStore` trait.
- Live CTK network scenarios and full correlation-group modelling for `listen`.
- Native protobuf/HTTP-2 gRPC and broker-based AsyncAPI transports.
- A full OpenTelemetry tracing/metrics SDK layer (spans, OTLP/gRPC).

## License

MIT. The official OWS SDK (`serverless_workflow_core`) is Apache-2.0 and is used
as a git dependency.
