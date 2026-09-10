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

The `ows-runtime-observability-otel` crate bridges lifecycle events to
OpenTelemetry by implementing `EventPublisher`:

- `OpenTelemetryLogsPublisher` — OTLP/HTTP JSON log records.
- `OpenTelemetryTracePublisher` — OTLP/HTTP JSON spans.
- `OpenTelemetryMetricsExporter` / `RuntimeMetrics` — counters and duration
  histograms exported as OTLP/HTTP JSON metrics.
- Behind the `otlp-grpc` feature: `OtlpGrpcLogsPublisher`,
  `OtlpGrpcTracePublisher` and `OtlpGrpcMetricsExporter`, which use the
  generated `opentelemetry-proto` messages over `tonic` (default endpoint
  `http://localhost:4317`).

All exporters are verified end to end against a loopback collector
(OTLP/HTTP) and a real `tonic` collector (OTLP/gRPC). Because everything flows
through `EventPublisher`, tracing SDKs or broker adapters can be layered on the
same interface.
