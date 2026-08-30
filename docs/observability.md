# Observability

The runtime emits structured `tracing` events and OWS lifecycle cloud events,
without coupling to any specific observability backend.

## Tracing

Every workflow execution and task transition emits structured `tracing` events
with fields such as `execution.id`, `workflow.name`, `workflow.namespace`,
`workflow.version`, `task.name`, `task.reference`.

Structured events are emitted for:

- workflow started / completed / faulted / cancelled
- task started / completed / faulted / retried / timed out
- event received
- external call started / completed

Binaries can install a subscriber with `ows_runtime_observability::init_tracing()`.

## Lifecycle cloud events

The OWS DSL defines lifecycle cloud events
(`io.serverlessworkflow.workflow.*.v1`, `io.serverlessworkflow.task.*.v1`). The
`ows-runtime-observability` crate's `LifecycleEmitter` builds and publishes these
through the core [`EventPublisher`](crate::service::EventPublisher) trait.

Because events flow through `EventPublisher`, an OpenTelemetry adapter, an HTTP
binding, or a broker adapter can be added without changing the engine. The
in-memory broker in `ows-runtime-events` is used for tests and local development.

## OpenTelemetry

An OpenTelemetry integration is not bundled yet, but the architecture supports
it: implement `EventPublisher` (or a tracing layer) to export to OpenTelemetry.
