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
| `Listen` task | Listen | ✅ | ✅ | 🚧 | 🚧 | basic `one`/`any`/`all`; `foreach` pending |
| `Wait` task | Wait | ✅ | ✅ | ✅ | ✅ | |
| `Call` task — HTTP | Call/HTTP | ✅ | ✅ | ✅ | 🚧 | network scenarios skip by default |
| `Call` task — OpenAPI | Call/OpenAPI | ✅ | ✅ | ✅ | 🚧 | network |
| `Call` task — gRPC | Call/gRPC | ❌ | — | — | — | adapter pending |
| `Call` task — AsyncAPI | Call/AsyncAPI | ❌ | — | — | — | adapter pending |
| `Call` task — A2A | Call/A2A | ❌ | — | — | — | adapter pending |
| `Call` task — MCP | Call/MCP | ❌ | — | — | — | adapter pending |
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
| Persistence (`ExecutionStore`) | Persistence | ✅ | ✅ | — | — | in-memory impl |
| Observability (tracing, lifecycle events) | Observability | ✅ | ✅ | ✅ | — | |

## Documented limitations

- gRPC, AsyncAPI, A2A, MCP `call` functions are not bundled.
- `run` container/script/shell require a `ProcessRunner` adapter and are
  deny-by-default.
- `listen` `foreach` iteration is not yet implemented.
- Scheduling triggers are parsed; the runtime registers them via the
  `Scheduler` trait but does not yet fire periodic executions end-to-end.
- OpenTelemetry integration is not yet bundled (lifecycle events use the core
  `EventPublisher` trait, so an OTel adapter can be added separately).
