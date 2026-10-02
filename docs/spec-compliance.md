# OWS runtime compliance audit

Audit baseline: the repository at the current revision, using the pinned
`serverless_workflow_core` SDK revision and the OWS 1.0.x task model already
used by the crate. The official SDK is the schema source for the Rust model;
the runtime adds semantic validation and execution behavior around it.

Status meanings: **Supported** is implemented and covered by automated tests;
**Partial** means a defined subset or an opt-in adapter is implemented;
**Missing** means it is not implemented in this repository.

| Spec section | Supported | Partial | Missing | Tests | Notes |
|---|---|---|---|---|---|
| Workflow document and metadata | document, namespace, name, version, title, summary, tags | DSL version acceptance is limited to `1.x` | schema-generated validation | `crates/ows-runtime-dsl/src/lib.rs` tests; `crates/ows-runtime-dsl/tests/serialization.rs` | `Workflow::from_yaml` / `from_json` are the library entry points. |
| JSON/YAML model | serde JSON/YAML deserialization and serialization; normalized fork defaults; ordered task maps | the upstream SDK has no `schemars` derives | independent generated JSON Schema artifact | DSL serialization tests | YAML and JSON are normalized through the same `serde_json::Value` path. |
| Workflow input/output and data flow | schema validation, `from`/`as` transforms, export/context propagation, null/object/array values | schema validation is feature-gated and uses `jsonschema` | — | `basic.rs`, `tasks.rs`, `data_driven.rs` | Runtime validation occurs before and after mapping stages. |
| Flow directives | `continue`, `exit`, `end`, named task targets | graph diagnostics model guarded and switch fall-through conservatively | prohibited-cycle policy is not applicable to ordinary OWS named transitions | `basic.rs`, `errors.rs`, DSL validation tests | Cycles are allowed because named transitions can intentionally repeat work. |
| Set task | map assignment and expression assignment | — | — | `tasks.rs`, `task_coverage.rs` | |
| Do task | ordered nested scope execution | — | — | `tasks.rs`, `task_coverage.rs` | |
| For task | `each`, `in`, `at`, `while`, inline collections, nested scope | loop body output semantics follow the current SDK/runtime model | — | `tasks.rs`, `task_coverage.rs`, `eval*.rs` | |
| Fork task | parallel branches and `compete` | branch cancellation is runtime cooperative cancellation | — | `tasks.rs`, `concurrency.rs`, `task_coverage.rs` | |
| Switch task | ordered conditions, default case, transitions | no-match behavior is modeled as fall-through | — | `tasks.rs`, `more_tasks.rs`, `task_coverage.rs` | |
| Try/catch task | error filters, `when`, `exceptWhen`, retries, handler scope | adapter-specific external errors depend on the invoker | — | `tasks.rs`, `errors.rs`, `task_coverage.rs` | |
| Raise task | inline and reusable errors with typed workflow faults | — | — | `basic.rs`, `errors.rs`, `task_coverage.rs` | |
| Emit task | CloudEvent construction and publication | broker adapters are separate crates/traits | — | `more_tasks.rs`, `ows-runtime-events` tests | |
| Listen task | one/any/all, filters, correlation, until, foreach | external brokers require an `EventConsumer` implementation | — | `listen_*.rs`, event unit tests | |
| Wait task | duration parsing and async waiting | cancellation/clock behavior is adapter-driven | — | `more_tasks.rs`, clock tests | |
| Call task | generic invoker plus HTTP, OpenAPI, gRPC, AsyncAPI, A2A, MCP adapters | network and protocol support is opt-in/policy controlled; some authentication schemes are not applied | — | `http*.rs`, `protocol_adapters.rs`, service tests | The runtime itself remains generic; adapters implement calls. |
| Run task | workflow subflow, shell/script/container process interfaces | process execution is deny-by-default; `run.return` is not represented by the pinned SDK model | full process return selection | `more_tasks.rs`, `process_run.rs` | Shell/container execution is deliberately an adapter concern. |
| Runtime expressions | sandboxed jq subset, variables, interpolation, conditions, selectors, filters, cached compilation | JavaScript language is recognized by the model but rejected as unsupported | full jq language and JS evaluator | expression unit/property tests; runtime expression regression | Invalid expression sources are now rejected before execution with JSON-pointer location. |
| Validation | required fields, DSL version, identifiers, duplicate names, targets, switch shape, nested scopes, reachability | structural schema validation remains mostly delegated to serde/upstream SDK | complete official JSON Schema diagnostics and every semantic rule | DSL validator tests; CLI validate tests | `Runtime::compile` is the mandatory validation boundary for the ergonomic API. |
| Lifecycle | running, completed, faulted, cancelled, waiting/pending phase types | pending/waiting/suspended are represented in core but the in-memory run path only emits running and terminal phases | server lifecycle management | core phase tests; runtime behavior tests | Hosting, persistence, scheduling, and monitoring are intentionally outside the core runtime API. |
| Sub-workflows | `run.workflow` lookup and execution through registered compiled workflows | referenced workflows must be registered in the runtime | dynamic deployment/version resolution | `more_tasks.rs` | No registry service or deployment behavior is implied. |
| Components (`use`) | errors, retries, timeouts, functions, extensions, secrets, catalogs | catalog HTTP resolution and authentication coverage are policy/adapter limited | — | component, catalog, extension, secret tests | These are definition-time/runtime dependencies, not a persistence platform. |
| Conformance | deterministic CTK/self-hosted scenarios and official examples vendored in fixtures | network CTK cases are opt-in; not every upstream example is represented as a dedicated test | exhaustive latest official YAML/JSON fixture mirror | `crates/ows-runtime-cli/tests/conformance.rs`, `examples.rs`, `tests/conformance` | The vendored CTK revision and known discrepancy are documented in `docs/conformance.md`. |
| Property testing | expression property tests and malformed-input no-panic coverage | workflow graph generation is not yet present | full workflow graph/property matrix | `ows-runtime-expressions/tests/property.rs` | |
| Fuzzing | — | — | cargo-fuzz targets for parsers, validator, and engine | — | No `fuzz/` workspace target currently exists. |
| Benchmarks | criterion benchmarks for parsing, compile, expressions, simple execution | benchmark matrix is small | allocation/deep-nesting/parallel benchmark suite | `crates/ows-runtime/benches/runtime.rs` | |
| `schemars` support | — | — | derived schema generation | — | The current official SDK uses serde derives, not schemars. Adding it would be a separate model/schema-generation change. |

## Runtime architecture

```text
YAML / JSON
    |
    v
Workflow::from_yaml / from_json
    |
    v
semantic validation + expression compilation/cache
    |
    v
compiled executable IR
    |
    v
Runtime::run -> ordered scopes -> state/task execution
    |
    v
data mapping, context/export, workflow output
```

The library-facing path is:

```rust,ignore
let workflow = ows_runtime::Workflow::from_yaml(yaml)?;
let runtime = ows_runtime::Runtime::new();
let result = runtime.run(&workflow, input).await?;
```

## Explicit non-goals

The runtime crate does not require a database, workflow server, scheduler,
deployment/version service, queue, user-management system, SaaS control plane,
or persistent callback store. Existing adapter crates for storage, scheduling,
observability, and CLI operation are optional integrations around the library;
they are not part of OWS definition parsing or state semantics.

## Remaining compliance work

The audit intentionally records the gaps instead of treating platform features
as specification coverage. The highest-value follow-ups are a schema-generation
strategy compatible with the upstream SDK, a pinned latest-CTK fixture import,
workflow-graph property tests, and cargo-fuzz targets.
