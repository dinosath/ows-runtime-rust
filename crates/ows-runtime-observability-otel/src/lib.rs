//! OpenTelemetry (OTLP/HTTP JSON) integration for OWS lifecycle events.
//!
//! The runtime decouples observability behind the core [`EventPublisher`] trait.
//! This crate bridges those lifecycle events (OWL CloudEvents) to OpenTelemetry
//! by exporting them as OTLP log records over HTTP/JSON to an OpenTelemetry
//! collector. It implements a compact OTLP/HTTP JSON exporter directly (no
//! gRPC/collector dependency), so it is deterministic and easy to run anywhere
//! a collector's `/v1/logs` HTTP endpoint is reachable.
//!
//! The full `opentelemetry` SDK can be layered on top of the same
//! [`EventPublisher`] interface when richer tracing is required; this crate
//! keeps the runtime free of a specific SDK.

use async_trait::async_trait;
use ows_runtime_core::{ErrorKind, EventMessage, EventPublisher, StandardErrorType, WorkflowError};
use serde_json::{json, Value};

/// The default OTLP/HTTP logs endpoint for a local collector.
pub const DEFAULT_OTLP_LOGS_ENDPOINT: &str = "http://localhost:4318/v1/logs";

/// An [`EventPublisher`] that exports OWS lifecycle events as OTLP log records.
#[derive(Clone)]
pub struct OpenTelemetryLogsPublisher {
    endpoint: String,
    service_name: String,
    client: reqwest::Client,
}

impl OpenTelemetryLogsPublisher {
    /// Creates a publisher targeting the given OTLP/HTTP logs endpoint.
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            client,
        }
    }

    /// Creates a publisher targeting the default local collector endpoint.
    pub fn new_default(service_name: impl Into<String>) -> Self {
        Self::new(DEFAULT_OTLP_LOGS_ENDPOINT, service_name)
    }

    /// The exporter's HTTP endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl EventPublisher for OpenTelemetryLogsPublisher {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        let payload = encode_log_request(event, &self.service_name);
        let response = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| comm_error(format!("otlp export to `{}` failed: {e}", self.endpoint)))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(comm_error(format!(
                "otlp export to `{}` returned status {status}",
                self.endpoint
            )));
        }
        Ok(())
    }
}

fn comm_error(detail: String) -> WorkflowError {
    let mut e = WorkflowError::standard(ErrorKind::Communication, StandardErrorType::Communication);
    e.problem.detail = Some(detail);
    e
}

/// Converts a CloudEvent-style [`EventMessage`] into an OTLP `ExportLogsServiceRequest`
/// proto-JSON payload (an object whose `resourceLogs` holds a single log record).
pub fn encode_log_request(event: &EventMessage, service_name: &str) -> Value {
    let time = event
        .time
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .and_then(|dt| dt.timestamp_nanos_opt())
        .unwrap_or(0);

    let mut attributes = vec![
        json!({ "key": "event.id", "value": { "stringValue": event.id } }),
        json!({ "key": "event.source", "value": { "stringValue": event.source } }),
        json!({ "key": "event.type", "value": { "stringValue": event.type_ } }),
        json!({ "key": "service.name", "value": { "stringValue": service_name } }),
    ];
    if let Some(subject) = &event.subject {
        attributes.push(json!({ "key": "event.subject", "value": { "stringValue": subject } }));
    }
    if let Some(data) = &event.data {
        attributes.push(json!({
            "key": "event.data",
            "value": { "stringValue": serde_json::to_string(data).unwrap_or_default() },
        }));
    }

    json!({
        "resourceLogs": [{
            "resource": { "attributes": [{
                "key": "service.name",
                "value": { "stringValue": service_name },
            }] },
            "scopeLogs": [{
                "scope": { "name": "ows-runtime" },
                "logRecords": [{
                    "timeUnixNano": time.to_string(),
                    "severityText": "INFO",
                    "body": { "stringValue": event.type_ },
                    "attributes": attributes,
                }],
            }],
        }],
    })
}

/// The default OTLP/HTTP traces endpoint for a local collector.
pub const DEFAULT_OTLP_TRACES_ENDPOINT: &str = "http://localhost:4318/v1/traces";

/// The default OTLP/HTTP metrics endpoint for a local collector.
pub const DEFAULT_OTLP_METRICS_ENDPOINT: &str = "http://localhost:4318/v1/metrics";

/// Derives a deterministic hex id of the given byte length from a seed.
///
/// OTLP requires non-zero trace/span ids. Deterministic derivation keeps tests
/// reproducible without pulling in a cryptographic hash.
fn hash_hex(seed: &str, bytes: usize) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut out = String::with_capacity(bytes * 2);
    let mut counter: u64 = 0;
    while out.len() < bytes * 2 {
        let mut hasher = DefaultHasher::new();
        counter.hash(&mut hasher);
        seed.hash(&mut hasher);
        out.push_str(&format!("{:016x}", hasher.finish()));
        counter += 1;
    }
    out.truncate(bytes * 2);
    if out.chars().all(|c| c == '0') {
        out.replace_range(0..1, "1");
    }
    out
}

fn event_time_nanos(event: &EventMessage) -> u64 {
    event
        .time
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .and_then(|dt| dt.timestamp_nanos_opt())
        .map(|n| n.max(0) as u64)
        .unwrap_or(0)
}

/// Converts a lifecycle [`EventMessage`] into an OTLP `ExportTraceServiceRequest`
/// proto-JSON payload with a single span.
pub fn encode_trace_request(event: &EventMessage, service_name: &str) -> Value {
    let time = event_time_nanos(event);
    let parent_seed = format!(
        "{}/{}",
        event.source,
        event.subject.clone().unwrap_or_default()
    );
    let trace_id = hash_hex(&parent_seed, 16);
    let span_id = hash_hex(&event.id, 8);

    let mut attributes = vec![
        json!({ "key": "event.id", "value": { "stringValue": event.id } }),
        json!({ "key": "event.source", "value": { "stringValue": event.source } }),
        json!({ "key": "event.type", "value": { "stringValue": event.type_ } }),
    ];
    if let Some(subject) = &event.subject {
        attributes.push(json!({ "key": "event.subject", "value": { "stringValue": subject } }));
    }
    if let Some(data) = &event.data {
        attributes.push(json!({
            "key": "event.data",
            "value": { "stringValue": serde_json::to_string(data).unwrap_or_default() },
        }));
    }

    // OTLP status: 1 = OK, 2 = ERROR.
    let errored = event.type_.contains("faulted")
        || event.type_.contains("cancelled")
        || event.type_.contains("failed");
    let status_code = if errored { 2 } else { 1 };

    json!({
        "resourceSpans": [{
            "resource": { "attributes": [{
                "key": "service.name",
                "value": { "stringValue": service_name },
            }] },
            "scopeSpans": [{
                "scope": { "name": "ows-runtime" },
                "spans": [{
                    "traceId": trace_id,
                    "spanId": span_id,
                    "name": event.type_,
                    "kind": 1,
                    "startTimeUnixNano": time.to_string(),
                    "endTimeUnixNano": time.to_string(),
                    "attributes": attributes,
                    "status": { "code": status_code },
                }],
            }],
        }],
    })
}

/// An [`EventPublisher`] that exports OWS lifecycle events as OTLP/HTTP spans.
#[derive(Clone)]
pub struct OpenTelemetryTracePublisher {
    endpoint: String,
    service_name: String,
    client: reqwest::Client,
}

impl OpenTelemetryTracePublisher {
    /// Creates a publisher targeting the given OTLP/HTTP traces endpoint.
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            client: reqwest::Client::builder().build().unwrap_or_default(),
        }
    }

    /// Creates a publisher targeting the default local collector endpoint.
    pub fn new_default(service_name: impl Into<String>) -> Self {
        Self::new(DEFAULT_OTLP_TRACES_ENDPOINT, service_name)
    }

    /// The exporter's HTTP endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl EventPublisher for OpenTelemetryTracePublisher {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        let payload = encode_trace_request(event, &self.service_name);
        let response = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                comm_error(format!(
                    "otlp trace export to `{}` failed: {e}",
                    self.endpoint
                ))
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(comm_error(format!(
                "otlp trace export to `{}` returned status {status}",
                self.endpoint
            )));
        }
        Ok(())
    }
}

/// In-memory metrics collected from lifecycle events: counters and a duration
/// histogram, encodable as an OTLP/HTTP `ExportMetricsServiceRequest`.
#[derive(Debug, Default, Clone)]
pub struct RuntimeMetrics {
    state: std::sync::Arc<std::sync::Mutex<MetricsState>>,
}

#[derive(Debug, Default)]
struct MetricsState {
    counters: std::collections::BTreeMap<String, i64>,
    /// Observed duration samples (milliseconds) per metric name.
    durations: std::collections::BTreeMap<String, Vec<f64>>,
}

/// The histogram bucket bounds (milliseconds) used for duration metrics.
pub const DURATION_BUCKET_BOUNDS_MS: [f64; 8] =
    [1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0];

impl RuntimeMetrics {
    /// Creates empty metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increments a monotonic counter.
    pub fn inc_counter(&self, name: &str, delta: i64) {
        let mut state = self.state.lock().unwrap();
        *state.counters.entry(name.to_string()).or_insert(0) += delta;
    }

    /// Records a duration observation (milliseconds).
    pub fn observe_duration(&self, name: &str, millis: f64) {
        let mut state = self.state.lock().unwrap();
        state
            .durations
            .entry(name.to_string())
            .or_default()
            .push(millis);
    }

    /// Records a lifecycle event into the metrics (counters by event type).
    pub fn record_event(&self, event: &EventMessage) {
        self.inc_counter(&format!("ows.events.{}", event.type_), 1);
        if event.type_.contains("workflow.completed") {
            self.inc_counter("ows.workflow.completed", 1);
        } else if event.type_.contains("workflow.faulted") {
            self.inc_counter("ows.workflow.faulted", 1);
        }
    }

    /// Returns a snapshot of the counters.
    pub fn counters(&self) -> std::collections::BTreeMap<String, i64> {
        self.state.lock().unwrap().counters.clone()
    }

    /// Encodes the metrics as an OTLP/HTTP JSON request.
    pub fn encode_metrics_request(&self, service_name: &str, time_unix_nano: u64) -> Value {
        let state = self.state.lock().unwrap();
        let mut metrics = Vec::new();

        for (name, value) in &state.counters {
            metrics.push(json!({
                "name": name,
                "unit": "1",
                "sum": {
                    "aggregationTemporality": 2,
                    "isMonotonic": true,
                    "dataPoints": [{
                        "asInt": value.to_string(),
                        "timeUnixNano": time_unix_nano.to_string(),
                    }],
                },
            }));
        }

        for (name, samples) in &state.durations {
            if samples.is_empty() {
                continue;
            }
            let count = samples.len();
            let sum: f64 = samples.iter().sum();
            let mut bucket_counts = vec![0i64; DURATION_BUCKET_BOUNDS_MS.len() + 1];
            for sample in samples {
                let mut placed = false;
                for (i, bound) in DURATION_BUCKET_BOUNDS_MS.iter().enumerate() {
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
            metrics.push(json!({
                "name": name,
                "unit": "ms",
                "histogram": {
                    "aggregationTemporality": 2,
                    "dataPoints": [{
                        "count": count.to_string(),
                        "sum": sum,
                        "bucketCounts": bucket_counts.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                        "explicitBounds": DURATION_BUCKET_BOUNDS_MS,
                        "timeUnixNano": time_unix_nano.to_string(),
                    }],
                },
            }));
        }

        json!({
            "resourceMetrics": [{
                "resource": { "attributes": [{
                    "key": "service.name",
                    "value": { "stringValue": service_name },
                }] },
                "scopeMetrics": [{
                    "scope": { "name": "ows-runtime" },
                    "metrics": metrics,
                }],
            }],
        })
    }
}

/// An [`EventPublisher`] that both counts lifecycle events and can export the
/// accumulated metrics over OTLP/HTTP.
#[derive(Clone)]
pub struct OpenTelemetryMetricsExporter {
    endpoint: String,
    service_name: String,
    metrics: RuntimeMetrics,
    client: reqwest::Client,
}

impl OpenTelemetryMetricsExporter {
    /// Creates an exporter targeting the given OTLP/HTTP metrics endpoint.
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            metrics: RuntimeMetrics::new(),
            client: reqwest::Client::builder().build().unwrap_or_default(),
        }
    }

    /// Creates an exporter targeting the default local collector endpoint.
    pub fn new_default(service_name: impl Into<String>) -> Self {
        Self::new(DEFAULT_OTLP_METRICS_ENDPOINT, service_name)
    }

    /// The exporter's HTTP endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The accumulated metrics.
    pub fn metrics(&self) -> &RuntimeMetrics {
        &self.metrics
    }

    /// Flushes the accumulated metrics to the collector.
    pub async fn export(&self, time_unix_nano: u64) -> Result<(), WorkflowError> {
        let payload = self
            .metrics
            .encode_metrics_request(&self.service_name, time_unix_nano);
        let response = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                comm_error(format!(
                    "otlp metric export to `{}` failed: {e}",
                    self.endpoint
                ))
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(comm_error(format!(
                "otlp metric export to `{}` returned status {status}",
                self.endpoint
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl EventPublisher for OpenTelemetryMetricsExporter {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        self.metrics.record_event(event);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn event() -> EventMessage {
        let mut e = EventMessage::new(
            "exec.1.started",
            "/ns/wf/1.0.0",
            "io.serverlessworkflow.workflow.started.v1",
        );
        e.time = Some("2024-01-01T00:00:00Z".into());
        e.subject = Some("exec-1".into());
        e.data = Some(json!({ "name": "wf-1.0.0.ns" }));
        e
    }

    #[test]
    fn encodes_otlp_json_shape() {
        let payload = encode_log_request(&event(), "my-service");
        let log_records = payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
            .as_array()
            .unwrap();
        assert_eq!(log_records.len(), 1);
        let record = &log_records[0];
        assert_eq!(record["body"]["stringValue"], event().type_);
        assert_eq!(record["timeUnixNano"], "1704067200000000000");
        let attrs = record["attributes"].as_array().unwrap();
        assert!(attrs.iter().any(|a| a["key"] == "event.type"));
    }

    #[tokio::test]
    async fn default_endpoint_and_no_network() {
        let p = OpenTelemetryLogsPublisher::new_default("svc");
        assert_eq!(p.endpoint(), DEFAULT_OTLP_LOGS_ENDPOINT);
        // No live collector is required for construction/encoding.
        let _ = encode_log_request(&event(), "svc");
    }

    /// Serves a single OTLP/HTTP request, capturing the JSON body.
    async fn serve_collector(captured: Arc<Mutex<Vec<u8>>>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                if sock.read(&mut byte).await.unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&head);
            let len = text
                .lines()
                .find_map(|l| {
                    let lower = l.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            let _ = sock.read_exact(&mut body).await;
            *captured.lock().unwrap() = body;
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        addr
    }

    #[tokio::test]
    async fn publishes_otlp_json_to_collector() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let addr = serve_collector(captured.clone()).await;
        let publisher = OpenTelemetryLogsPublisher::new(format!("http://{addr}/v1/logs"), "svc");
        publisher.publish(&event()).await.unwrap();

        let body: Value = serde_json::from_slice(&captured.lock().unwrap()).unwrap();
        let record = &body["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
        assert_eq!(record["body"]["stringValue"], event().type_);
        assert_eq!(record["timeUnixNano"], "1704067200000000000");
    }

    #[test]
    fn encodes_otlp_trace_shape() {
        let payload = encode_trace_request(&event(), "my-service");
        let span = &payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["name"], event().type_);
        assert_eq!(span["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(span["spanId"].as_str().unwrap().len(), 16);
        assert_eq!(span["status"]["code"], 1);
        assert_eq!(span["startTimeUnixNano"], "1704067200000000000");
    }

    #[test]
    fn trace_status_is_error_for_faults() {
        let mut e = event();
        e.type_ = "io.serverlessworkflow.workflow.faulted.v1".into();
        let payload = encode_trace_request(&e, "svc");
        let span = &payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["status"]["code"], 2);
    }

    #[tokio::test]
    async fn publishes_otlp_trace_to_collector() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let addr = serve_collector(captured.clone()).await;
        let publisher = OpenTelemetryTracePublisher::new(format!("http://{addr}/v1/traces"), "svc");
        publisher.publish(&event()).await.unwrap();

        let body: Value = serde_json::from_slice(&captured.lock().unwrap()).unwrap();
        let span = &body["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["name"], event().type_);
        assert_eq!(span["traceId"].as_str().unwrap().len(), 32);
    }

    #[test]
    fn encodes_counters_and_histograms() {
        let metrics = RuntimeMetrics::new();
        metrics.inc_counter("ows.workflow.completed", 3);
        metrics.observe_duration("ows.task.duration_ms", 4.0);
        metrics.observe_duration("ows.task.duration_ms", 250.0);

        let payload = metrics.encode_metrics_request("svc", 42);
        let encoded = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap();
        let counter = encoded
            .iter()
            .find(|m| m["name"] == "ows.workflow.completed")
            .unwrap();
        assert_eq!(counter["sum"]["dataPoints"][0]["asInt"], "3");
        let histogram = encoded
            .iter()
            .find(|m| m["name"] == "ows.task.duration_ms")
            .unwrap();
        assert_eq!(histogram["histogram"]["dataPoints"][0]["count"], "2");
        assert_eq!(histogram["histogram"]["dataPoints"][0]["sum"], 254.0);
    }

    #[tokio::test]
    async fn metrics_exporter_publishes_and_flushes() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let addr = serve_collector(captured.clone()).await;
        let exporter =
            OpenTelemetryMetricsExporter::new(format!("http://{addr}/v1/metrics"), "svc");

        // Publishing an event records it without any network call.
        EventPublisher::publish(&exporter, &event()).await.unwrap();
        assert!(exporter
            .metrics()
            .counters()
            .contains_key(&format!("ows.events.{}", event().type_)));

        exporter.export(123).await.unwrap();
        let body: Value = serde_json::from_slice(&captured.lock().unwrap()).unwrap();
        let metrics = body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap();
        assert!(!metrics.is_empty());
    }

    #[test]
    fn records_workflow_terminal_counters() {
        let metrics = RuntimeMetrics::new();
        let mut completed = event();
        completed.type_ = "io.serverlessworkflow.workflow.completed.v1".into();
        let mut faulted = event();
        faulted.id = "exec.2.faulted".into();
        faulted.type_ = "io.serverlessworkflow.workflow.faulted.v1".into();
        metrics.record_event(&completed);
        metrics.record_event(&faulted);
        let counters = metrics.counters();
        assert_eq!(counters["ows.workflow.completed"], 1);
        assert_eq!(counters["ows.workflow.faulted"], 1);
    }
}
