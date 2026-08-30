# Extending the Runtime

The runtime is designed so new capabilities are added as adapters behind stable
core traits, without changing workflow semantics.

## Custom functions (`call`)

Implement the [`FunctionInvoker`](crate::service::FunctionInvoker) trait and
register it:

```rust,ignore
use ows_runtime::service::{FunctionInvoker, FunctionRequest};

struct MyFunction;

#[async_trait::async_trait]
impl FunctionInvoker for MyFunction {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<serde_json::Value, WorkflowError> {
        // req.args is the interpolated `with` map; req.context has the expression context.
        Ok(serde_json::json!({ "result": "hello" }))
    }
}

let runtime = Runtime::builder()
    .register_function("myFunc", Arc::new(MyFunction))
    .build()?;
```

Workflows then use `call: myFunc`.

## External service invocation

Implement [`ServiceInvoker`](crate::service::ServiceInvoker) for a transport
(gRPC, Kafka, ...) and provide it via `RuntimeBuilder::with_service`.

## Events

Implement [`EventPublisher`] and [`EventConsumer`](crate::service::EventConsumer)
for a real broker (Kafka, NATS, HTTP binding) and provide them via
`with_event_publisher` / `with_event_consumer`. The in-memory broker in
`ows-runtime-events` is used for tests.

## Scheduling

Implement [`Scheduler`](crate::runtime::Scheduler) for a durable or distributed
scheduler and provide it via `with_scheduler`.

## Process execution

Implement [`ProcessRunner`](crate::service::ProcessRunner) for container/script/
shell execution and enable `allow_scripts`/`allow_containers` in the policy.

## Persistence

Implement [`ExecutionStore`](crate::store::ExecutionStore) for PostgreSQL,
Redis, SQLite, or a distributed store and provide it via `with_store`. The
`InMemoryExecutionStore` is the default.

## Expression languages

Implement [`ExpressionEngine`](crate::expression::ExpressionEngine) to add
another runtime expression language beyond `jq`, and configure workflows with
`evaluate.language`.

## Observability

Lifecycle events flow through the [`EventPublisher`] trait; an OpenTelemetry
adapter can be built on top without coupling to the engine. `tracing` spans and
events are emitted with structured fields on every workflow and task transition.

## New task types

OWS task types are a closed set in the specification. To add custom behavior
without extending the DSL, prefer a custom `FunctionInvoker` (for `call`) or a
`ProcessRunner` (for `run`).
