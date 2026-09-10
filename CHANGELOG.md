# Changelog

## [Unreleased]

### Added

- OWS runtime workspace with library-first architecture.
- `ows-runtime-core`: typed errors (ProblemDetails, WorkflowError), injectable
  clocks/RNG/UUIDs, `RuntimePolicy`, `ExecutionStore`, service/event/scheduler
  traits.
- `ows-runtime-expressions`: sandboxed jq-subset interpreter with strict/loose
  expression handling, interpolation, and property-based tests.
- `ows-runtime-dsl`: parsing (YAML/JSON), validation, and default normalization
  on top of the official `serverless_workflow_core` SDK.
- `ows-runtime`: scope-based execution engine, all twelve OWS task types,
  data-flow pipeline, flow directives, retries, timeouts, cancellation, and
  HTTP/OpenAPI function invokers.
- `ows-runtime-events`: CloudEvents, event matching, and an in-memory broker.
- `ows-runtime-scheduler`: scheduling abstractions and cron parsing.
- `ows-runtime-observability`: tracing and OWS lifecycle event emission.
- `ows-runtime-testing`: deterministic test helpers and fakes.
- `ows-runtime-cli`: `validate`, `compile`, `run`, `conformance` subcommands with
  a Gherkin CTK runner.
- `use.extensions` support: extension `before`/`after` tasks are compiled and
  executed around tasks of the extended type, guarded by `when`, with
  `then: exit` short-circuiting the extended task.
- `use.catalogs` support: a `CatalogResolver` trait plus static, filesystem and
  HTTP resolvers; imported components (including nested catalogs) are merged
  before compilation. `Runtime::resolve_definition` /
  `register_definition_resolved`.
- `use.secrets` support: a `SecretResolver` trait plus in-memory and
  environment resolvers; declared secrets are exposed as `$secrets` and
  references to undeclared secrets are rejected at compile time.
- End-to-end scheduling: `Runtime::start_schedules` fires `every`/`after`/
  `cron`/`on` triggers, returning a cancellable `ScheduleSet`.
- Listen correlation grouping: `expect` is evaluated against the workflow
  context and first-seen correlation values are shared across `all` filters.
- Durable checkpoint/resume: the engine persists a per-top-level-task
  checkpoint and `Runtime::resume` continues a non-terminal execution.
- OpenTelemetry OTLP/HTTP JSON spans and metrics (in addition to logs) in
  `ows-runtime-observability-otel`.
- Broker-based AsyncAPI transport (`transport.broker`) over the runtime's event
  publisher/consumer.
- Native protobuf/HTTP-2 gRPC transport behind the `grpc-native` feature:
  runtime `.proto` compilation (`protox`), dynamic messages (`prost-reflect`)
  and unary calls over `tonic`. The `grpc` function dispatches to it when the
  call supplies a `proto` descriptor, else to the JSON/gateway adapter.
- Inline `for.in` collections are normalized so the official examples parse.
- Data-driven tests: the official OWS CTK features and examples are vendored
  and executed in offline CI, plus a case-file suite under
  `crates/ows-runtime/tests/fixtures/cases`.

### Conformance

- 12/12 deterministic OWS CTK scenarios pass against the vendored CTK, with no
  external checkout required.
- Network-dependent scenarios are skipped by default and can be enabled with
  `--include-network`.
