# Specification Coverage

This matrix tracks OWS 1.0.3 feature coverage. Status legend:

- ✅ Implemented
- 🚧 Partial / experimental
- ❌ Not implemented
- — Not applicable

"CTK" refers to the OWS Conformance Test Kit; deterministic scenarios are the
non-network ones.

| OWS Feature | Spec Section | Implemented | Unit Tests | Integration Tests | CTK | Notes |
|---|:---:|:---:|:---:|:---:|:---:|---|
| Workflow (document, input, output, timeout, evaluate) | Workflow | ✅ | ✅ | ✅ | ✅ | |
| Workflow `do` scope | Workflow | ✅ | ✅ | ✅ | ✅ | |
| Flow directives (`continue`/`exit`/`end`/name) | Flow Directive | ✅ | ✅ | ✅ | ✅ | |
| `Set` task | Set | ✅ | ✅ | ✅ | ✅ | |
| `Do` task | Do | ✅ | ✅ | ✅ | ✅ | |
| `For` task (each/in/at/while) | For | ✅ | ✅ | ✅ | ✅ | |
| `Fork` task (parallel, compete) | Fork | ✅ | ✅ | ✅ | ✅ | `compete` default injected by DSL normalization |
| `Switch` task (when/then/default) | Switch | ✅ | ✅ | ✅ | ✅ | |
| `Try` task (catch, retry, do) | Try | ✅ | ✅ | ✅ | ✅ | |
| `Raise` task | Raise | ✅ | ✅ | ✅ | ✅ | |
| `Emit` task | Emit | ✅ | ✅ | ✅ | ✅ | |
| `Listen` task | Listen | ✅ | ✅ | ✅ | ✅ | `one`/`any`/`all`, `foreach`, correlation `from`/`expect` + cross-event grouping |
| `Wait` task | Wait | ✅ | ✅ | ✅ | ✅ | |
| `Call` task — HTTP | Call/HTTP | ✅ | ✅ | ✅ | 🚧 | network scenarios skip by default |
| `Call` task — OpenAPI | Call/OpenAPI | ✅ | ✅ | ✅ | 🚧 | network |
| `Call` task — gRPC | Call/gRPC | ✅ | ✅ | ✅ | — | JSON/gateway + native protobuf/HTTP-2 (`grpc-native`) |
| `Call` task — AsyncAPI | Call/AsyncAPI | ✅ | ✅ | ✅ | — | bundled adapter (HTTP publish) |
| `Call` task — A2A | Call/A2A | ✅ | ✅ | ✅ | — | bundled JSON-RPC adapter |
| `Call` task — MCP | Call/MCP | ✅ | ✅ | ✅ | — | bundled JSON-RPC adapter |
| `Run` task — workflow | Run/Workflow | ✅ | ✅ | ✅ | — | |
| `Run` task — shell/script/container | Run | 🚧 | ✅ | ✅ | — | deny-by-default; `ProcessRunner` policy |
| Runtime expressions (jq) | Runtime Expressions | ✅ | ✅ | ✅ | ✅ | sandboxed subset |
| Input validation/transform | Data Flow | ✅ | ✅ | ✅ | ✅ | |
| Output transform/validation | Data Flow | ✅ | ✅ | ✅ | ✅ | |
| Export/context | Data Flow | ✅ | ✅ | ✅ | ✅ | |
| Workflow input/output schema | Schema | ✅ | ✅ | 🚧 | — | JSON Schema via `validation` feature |
| Retry policy | Retry | ✅ | ✅ | ✅ | 🚧 | |
| Timeout | Timeout | ✅ | ✅ | 🚧 | — | |
| Cancellation | Cancellation | ✅ | ✅ | 🚧 | — | |
| Events (CloudEvents, lifecycle) | Events | ✅ | ✅ | ✅ | — | in-memory broker |
| Event-driven scheduling | Schedule | ✅ | ✅ | ✅ | — | `Runtime::start_schedules` fires `on` triggers |
| Cron / every / after scheduling | Schedule | ✅ | ✅ | ✅ | — | fired end-to-end by `Runtime::start_schedules` |
| Authentication (basic/bearer) | Authentication | ✅ | ✅ | ✅ | 🚧 | |
| Secrets | Secrets | ✅ | ✅ | ✅ | — | `SecretResolver` (map/env); undeclared refs rejected at compile |
| Extensions | Extensions | ✅ | ✅ | ✅ | — | `before`/`after`/`when`; `then: exit` short-circuits |
| Catalogs | Catalogs | ✅ | ✅ | ✅ | — | `CatalogResolver` (static/file/HTTP), nested catalogs |
| Runtime policy (deny-by-default) | Security | ✅ | ✅ | ✅ | — | |
| Persistence (`ExecutionStore`) | Persistence | ✅ | ✅ | ✅ | — | in-memory + SQLite/Postgres/Redis; per-task checkpoints + `Runtime::resume` |
| Observability (tracing, lifecycle events) | Observability | ✅ | ✅ | ✅ | — | OTLP/HTTP JSON logs, spans and metrics in `ows-runtime-observability-otel` |

## Documented limitations

- The `grpc` function dispatches on the call arguments: with a `proto`
  descriptor it uses the native protobuf/HTTP-2 transport (the `grpc-native`
  feature; the `.proto` is compiled at runtime with `protox`, messages are
  dynamic `prost-reflect` messages, and the call is a unary `tonic` request over
  plaintext HTTP/2 — TLS is not bundled). Without `proto` it falls back to the
  JSON transcoding/gateway adapter. AsyncAPI supports both an HTTP channel
  binding and a broker-based transport (`transport.broker`) over the runtime's
  event publisher/consumer; A2A/MCP use HTTP/JSON transports.
- `run` container/script/shell require a `ProcessRunner` adapter and are
  deny-by-default.
- `listen` filters support `correlate` `from`/`expect` (with `expect` evaluated
  against the workflow context) and cross-event correlation *grouping*: for
  `all`, a first-seen key value becomes the expected value shared by subsequent
  filters unless a filter declares an explicit `expect`.
- Scheduling triggers (`every`/`after`/`cron`/`on`) fire end-to-end via
  `Runtime::start_schedules`, which also registers each schedule with the
  configured `Scheduler` trait. A durable/distributed scheduler can still be
  layered on the trait for multi-node coordination.
- `ows-runtime-stores` provides durable `ExecutionStore` backends (SQLite by
  default; PostgreSQL and Redis behind features). The engine persists an
  execution's start record, per-top-level-task checkpoints (next index + scope
  input + context), terminal phase and lifecycle events. `Runtime::resume`
  reloads a non-terminal record and continues from the checkpoint, reusing the
  execution id; checkpoints are at top-level task boundaries. Store failures are
  logged, not fatal. Integration tests cover in-memory and SQLite resume.
- `ows-runtime-observability-otel` exports lifecycle events to an
  OpenTelemetry collector over OTLP/HTTP JSON as **logs, spans and metrics**,
  verified end to end against a loopback OTLP/HTTP collector. OTLP/gRPC export
  would require the OpenTelemetry OTLP/gRPC SDK or a protobuf transport and is
  not bundled.
- The `evaluate.language` is honored as `jq`; the `js` expression language is
  parsed but not executed (the bundled engine is a sandboxed jq subset).
