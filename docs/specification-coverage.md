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
| `Listen` task | Listen | ✅ | ✅ | ✅ | 🚧 | `one`/`any`/`all` plus `foreach` iteration |
| `Wait` task | Wait | ✅ | ✅ | ✅ | ✅ | |
| `Call` task — HTTP | Call/HTTP | ✅ | ✅ | ✅ | 🚧 | network scenarios skip by default |
| `Call` task — OpenAPI | Call/OpenAPI | ✅ | ✅ | ✅ | 🚧 | network |
| `Call` task — gRPC | Call/gRPC | ✅ | ✅ | ✅ | — | bundled JSON/gateway adapter |
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
| Event-driven scheduling | Schedule | 🚧 | ✅ | 🚧 | — | |
| Cron / every / after scheduling | Schedule | 🚧 | ✅ | 🚧 | — | |
| Authentication (basic/bearer) | Authentication | ✅ | ✅ | ✅ | 🚧 | |
| Secrets | Secrets | 🚧 | ✅ | 🚧 | — | declared but not enforced |
| Extensions | Extensions | ❌ | — | — | — | |
| Catalogs | Catalogs | ❌ | — | — | — | |
| Runtime policy (deny-by-default) | Security | ✅ | ✅ | ✅ | — | |
| Persistence (`ExecutionStore`) | Persistence | ✅ | ✅ | — | — | in-memory + SQLite/Postgres/Redis adapters in `ows-runtime-stores` |
| Observability (tracing, lifecycle events) | Observability | ✅ | ✅ | ✅ | — | OTLP/HTTP JSON exporter in `ows-runtime-observability-otel` |

## Documented limitations

- The gRPC adapter targets gRPC services exposed through a JSON transcoding /
  HTTP gateway (the runtime does not embed protobuf code generation). AsyncAPI
  and A2A/MCP adapters use HTTP/JSON transports (the AsyncAPI adapter publishes
  over an HTTP channel binding). Full protobuf/HTTP-2 gRPC and broker-based
  AsyncAPI require transport adapters and are opt-in.
- `run` container/script/shell require a `ProcessRunner` adapter and are
  deny-by-default.
- `listen` `all` consumes one event per filter (correlation groups beyond a
  single `from`/`expect` key are not fully modelled).
- Scheduling triggers are parsed; the runtime registers them via the
  `Scheduler` trait but does not yet fire periodic executions end-to-end.
- `ows-runtime-stores` provides durable `ExecutionStore` backends (SQLite by
  default; PostgreSQL and Redis behind features). The in-memory store remains
  the engine default; wiring a durable store into long-running resume is
  layered on the same trait.
- `ows-runtime-observability-otel` exports lifecycle events to an
  OpenTelemetry collector over OTLP/HTTP JSON. A full tracing SDK (spans,
  metrics, OTLP/gRPC) can be layered on the `EventPublisher` trait.
