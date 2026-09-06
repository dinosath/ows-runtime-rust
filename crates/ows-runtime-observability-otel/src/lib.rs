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
}
