//! OTLP/gRPC export of OWS lifecycle logs, spans and metrics.
//!
//! This complements the OTLP/HTTP JSON exporters in the crate root: when the
//! `otlp-grpc` feature is enabled, lifecycle events can be exported to an
//! OpenTelemetry collector's gRPC endpoint (default port 4317) using the
//! generated `opentelemetry-proto` messages and `tonic`.

#![cfg(feature = "otlp-grpc")]

use async_trait::async_trait;
use ows_runtime_core::{ErrorKind, EventMessage, EventPublisher, StandardErrorType, WorkflowError};

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_client::MetricsServiceClient;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    metric, AggregationTemporality, Histogram, HistogramDataPoint, Metric, NumberDataPoint,
    ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use tonic::transport::Channel;

use crate::{RuntimeMetrics, DEFAULT_OTLP_GRPC_ENDPOINT};

fn comm_error(detail: String) -> WorkflowError {
    let mut e = WorkflowError::standard(ErrorKind::Communication, StandardErrorType::Communication);
    e.problem.detail = Some(detail);
    e
}

/// Decodes a hex string into bytes (used for OTLP trace/span ids).
fn hex_bytes(seed: &str, bytes: usize) -> Vec<u8> {
    let hex = crate::hash_hex(seed, bytes);
    (0..bytes)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect()
}

fn string_value(value: impl Into<String>) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.into())),
    }
}

fn key_value(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(value),
        ..Default::default()
    }
}

fn scope() -> InstrumentationScope {
    InstrumentationScope {
        name: "ows-runtime".to_string(),
        ..Default::default()
    }
}

fn resource(service_name: &str) -> Resource {
    Resource {
        attributes: vec![key_value("service.name", string_value(service_name))],
        ..Default::default()
    }
}

/// Builds the OTLP `LogRecord` for a lifecycle event.
pub fn log_record_from_event(event: &EventMessage) -> LogRecord {
    let mut attributes = vec![
        key_value("event.id", string_value(event.id.clone())),
        key_value("event.source", string_value(event.source.clone())),
        key_value("event.type", string_value(event.type_.clone())),
    ];
    if let Some(subject) = &event.subject {
        attributes.push(key_value("event.subject", string_value(subject.clone())));
    }
    if let Some(data) = &event.data {
        attributes.push(key_value(
            "event.data",
            string_value(serde_json::to_string(data).unwrap_or_default()),
        ));
    }
    LogRecord {
        time_unix_nano: crate::event_time_nanos(event),
        observed_time_unix_nano: crate::event_time_nanos(event),
        severity_text: "INFO".to_string(),
        body: Some(string_value(event.type_.clone())),
        attributes,
        ..Default::default()
    }
}

/// Builds an OTLP logs export request.
pub fn logs_request(event: &EventMessage, service_name: &str) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource(service_name)),
            scope_logs: vec![ScopeLogs {
                scope: Some(scope()),
                log_records: vec![log_record_from_event(event)],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// Builds the OTLP `Span` for a lifecycle event.
pub fn span_from_event(event: &EventMessage) -> Span {
    let parent = format!(
        "{}/{}",
        event.source,
        event.subject.clone().unwrap_or_default()
    );
    let mut attributes = vec![
        key_value("event.id", string_value(event.id.clone())),
        key_value("event.source", string_value(event.source.clone())),
        key_value("event.type", string_value(event.type_.clone())),
    ];
    if let Some(data) = &event.data {
        attributes.push(key_value(
            "event.data",
            string_value(serde_json::to_string(data).unwrap_or_default()),
        ));
    }
    let time = crate::event_time_nanos(event);
    let errored = event.type_.contains("faulted")
        || event.type_.contains("cancelled")
        || event.type_.contains("failed");
    Span {
        trace_id: hex_bytes(&parent, 16),
        span_id: hex_bytes(&event.id, 8),
        name: event.type_.clone(),
        kind: 1,
        start_time_unix_nano: time,
        end_time_unix_nano: time,
        attributes,
        status: Some(opentelemetry_proto::tonic::trace::v1::Status {
            message: String::new(),
            code: if errored { 2 } else { 1 },
        }),
        ..Default::default()
    }
}

/// Builds an OTLP traces export request.
pub fn traces_request(event: &EventMessage, service_name: &str) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource(service_name)),
            scope_spans: vec![ScopeSpans {
                scope: Some(scope()),
                spans: vec![span_from_event(event)],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// Builds an OTLP metrics export request from accumulated counters/histograms.
pub fn metrics_request(
    metrics: &RuntimeMetrics,
    service_name: &str,
    time_unix_nano: u64,
) -> ExportMetricsServiceRequest {
    let mut out = Vec::new();

    for (name, value) in metrics.counters() {
        out.push(Metric {
            name,
            unit: "1".to_string(),
            data: Some(metric::Data::Sum(Sum {
                data_points: vec![NumberDataPoint {
                    time_unix_nano,
                    value: Some(
                        opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(
                            value,
                        ),
                    ),
                    ..Default::default()
                }],
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
                is_monotonic: true,
            })),
            ..Default::default()
        });
    }

    for (name, samples) in metrics.durations() {
        if samples.is_empty() {
            continue;
        }
        let count = samples.len() as u64;
        let sum: f64 = samples.iter().sum();
        let mut bucket_counts = vec![0u64; crate::DURATION_BUCKET_BOUNDS_MS.len() + 1];
        for sample in &samples {
            let mut placed = false;
            for (i, bound) in crate::DURATION_BUCKET_BOUNDS_MS.iter().enumerate() {
                if sample <= bound {
                    bucket_counts[i] += 1;
                    placed = true;
                    break;
                }
            }
            if !placed {
                let last = bucket_counts.len() - 1;
                bucket_counts[last] += 1;
            }
        }
        out.push(Metric {
            name,
            unit: "ms".to_string(),
            data: Some(metric::Data::Histogram(Histogram {
                data_points: vec![HistogramDataPoint {
                    time_unix_nano,
                    count,
                    sum: Some(sum),
                    bucket_counts,
                    explicit_bounds: crate::DURATION_BUCKET_BOUNDS_MS.to_vec(),
                    ..Default::default()
                }],
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
            })),
            ..Default::default()
        });
    }

    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource(service_name)),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(scope()),
                metrics: out,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// An [`EventPublisher`] that exports lifecycle events as OTLP/gRPC logs.
pub struct OtlpGrpcLogsPublisher {
    service_name: String,
    client: LogsServiceClient<Channel>,
}

impl OtlpGrpcLogsPublisher {
    /// Connects to an OTLP/gRPC collector endpoint.
    pub async fn connect(
        endpoint: impl Into<String>,
        service_name: impl Into<String>,
    ) -> Result<Self, WorkflowError> {
        let endpoint = endpoint.into();
        let client = LogsServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| comm_error(format!("failed to connect to `{endpoint}`: {e}")))?;
        Ok(Self {
            service_name: service_name.into(),
            client,
        })
    }

    /// Connects to the default local collector endpoint.
    pub async fn connect_default(service_name: impl Into<String>) -> Result<Self, WorkflowError> {
        Self::connect(DEFAULT_OTLP_GRPC_ENDPOINT, service_name).await
    }
}

#[async_trait]
impl EventPublisher for OtlpGrpcLogsPublisher {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        let request = logs_request(event, &self.service_name);
        self.client
            .clone()
            .export(request)
            .await
            .map_err(|e| comm_error(format!("OTLP/gRPC log export failed: {e}")))?;
        Ok(())
    }
}

/// An [`EventPublisher`] that exports lifecycle events as OTLP/gRPC spans.
pub struct OtlpGrpcTracePublisher {
    service_name: String,
    client: TraceServiceClient<Channel>,
}

impl OtlpGrpcTracePublisher {
    /// Connects to an OTLP/gRPC collector endpoint.
    pub async fn connect(
        endpoint: impl Into<String>,
        service_name: impl Into<String>,
    ) -> Result<Self, WorkflowError> {
        let endpoint = endpoint.into();
        let client = TraceServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| comm_error(format!("failed to connect to `{endpoint}`: {e}")))?;
        Ok(Self {
            service_name: service_name.into(),
            client,
        })
    }

    /// Connects to the default local collector endpoint.
    pub async fn connect_default(service_name: impl Into<String>) -> Result<Self, WorkflowError> {
        Self::connect(DEFAULT_OTLP_GRPC_ENDPOINT, service_name).await
    }
}

#[async_trait]
impl EventPublisher for OtlpGrpcTracePublisher {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        let request = traces_request(event, &self.service_name);
        self.client
            .clone()
            .export(request)
            .await
            .map_err(|e| comm_error(format!("OTLP/gRPC trace export failed: {e}")))?;
        Ok(())
    }
}

/// Records lifecycle events into metrics and can flush them over OTLP/gRPC.
pub struct OtlpGrpcMetricsExporter {
    service_name: String,
    metrics: RuntimeMetrics,
    client: MetricsServiceClient<Channel>,
}

impl OtlpGrpcMetricsExporter {
    /// Connects to an OTLP/gRPC collector endpoint.
    pub async fn connect(
        endpoint: impl Into<String>,
        service_name: impl Into<String>,
    ) -> Result<Self, WorkflowError> {
        let endpoint = endpoint.into();
        let client = MetricsServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| comm_error(format!("failed to connect to `{endpoint}`: {e}")))?;
        Ok(Self {
            service_name: service_name.into(),
            metrics: RuntimeMetrics::new(),
            client,
        })
    }

    /// Connects to the default local collector endpoint.
    pub async fn connect_default(service_name: impl Into<String>) -> Result<Self, WorkflowError> {
        Self::connect(DEFAULT_OTLP_GRPC_ENDPOINT, service_name).await
    }

    /// The accumulated metrics.
    pub fn metrics(&self) -> &RuntimeMetrics {
        &self.metrics
    }

    /// Flushes the accumulated metrics over OTLP/gRPC.
    pub async fn export(&self, time_unix_nano: u64) -> Result<(), WorkflowError> {
        let request = metrics_request(&self.metrics, &self.service_name, time_unix_nano);
        self.client
            .clone()
            .export(request)
            .await
            .map_err(|e| comm_error(format!("OTLP/gRPC metric export failed: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl EventPublisher for OtlpGrpcMetricsExporter {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        self.metrics.record_event(event);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> EventMessage {
        let mut e = EventMessage::new(
            "exec.1.started",
            "/ns/wf/1.0.0",
            "io.serverlessworkflow.workflow.started.v1",
        );
        e.time = Some("2024-01-01T00:00:00Z".into());
        e.subject = Some("exec-1".into());
        e.data = Some(serde_json::json!({ "name": "wf" }));
        e
    }

    #[test]
    fn builds_logs_and_traces_requests() {
        let logs = logs_request(&event(), "svc");
        let record = &logs.resource_logs[0].scope_logs[0].log_records[0];
        assert_eq!(record.time_unix_nano, 1_704_067_200_000_000_000);
        assert!(record.body.as_ref().unwrap().value.is_some());

        let traces = traces_request(&event(), "svc");
        let span = &traces.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(span.trace_id.len(), 16);
        assert_eq!(span.span_id.len(), 8);
        assert_eq!(span.name, event().type_);
        assert_eq!(span.status.as_ref().unwrap().code, 1);
    }

    #[tokio::test]
    async fn exports_logs_over_grpc_to_a_collector() {
        use std::sync::{Arc, Mutex};
        use tokio_stream::wrappers::TcpListenerStream;

        #[derive(Clone, Default)]
        struct TestCollector {
            captured: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
        }

        #[tonic::async_trait]
        impl opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::LogsService
            for TestCollector
        {
            async fn export(
                &self,
                request: tonic::Request<ExportLogsServiceRequest>,
            ) -> Result<
                tonic::Response<
                    opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceResponse,
                >,
                tonic::Status,
            > {
                self.captured.lock().unwrap().push(request.into_inner());
                Ok(tonic::Response::new(Default::default()))
            }
        }

        let captured = Arc::new(Mutex::new(Vec::new()));
        let collector = TestCollector {
            captured: captured.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(
                    opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::LogsServiceServer::new(collector),
                )
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let publisher = OtlpGrpcLogsPublisher::connect(format!("http://{addr}"), "svc")
            .await
            .unwrap();
        publisher.publish(&event()).await.unwrap();
        for _ in 0..50 {
            if !captured.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let record = &requests[0].resource_logs[0].scope_logs[0].log_records[0];
        assert!(record.body.as_ref().unwrap().value.is_some());
    }

    #[test]
    fn builds_metrics_request() {
        let metrics = RuntimeMetrics::new();
        metrics.inc_counter("ows.workflow.completed", 3);
        metrics.observe_duration("ows.task.duration_ms", 4.0);
        let request = metrics_request(&metrics, "svc", 42);
        let encoded = &request.resource_metrics[0].scope_metrics[0].metrics;
        assert!(encoded.iter().any(|m| m.name == "ows.workflow.completed"));
        assert!(encoded.iter().any(|m| m.name == "ows.task.duration_ms"));
    }
}
