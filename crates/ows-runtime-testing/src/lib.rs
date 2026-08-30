#![allow(clippy::result_large_err)]
//! # ows-runtime-testing
//!
//! Test helpers, fakes and deterministic runtime construction for the OWS
//! runtime. Use these to write fast, deterministic integration and unit tests
//! without real network, clocks or random sources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ows_runtime::Runtime;
use ows_runtime_core::{
    Clock, DeterministicClock, EventMessage, EventPublisher, SeededRandomGenerator, ServiceInvoker,
    ServiceRequest, ServiceResponse, WorkflowError,
};

/// Builds a `Runtime` with deterministic clocks, RNG and no network.
///
/// The runtime uses a [`DeterministicClock`] and a seeded random generator so
/// timing and jitter are fully reproducible in tests.
pub fn test_runtime() -> Runtime {
    Runtime::builder()
        .with_clock(Arc::new(DeterministicClock::new()))
        .with_random(Arc::new(SeededRandomGenerator::new(42)))
        .build()
        .expect("test runtime should build")
}

/// Builds a `Runtime` with a fake service invoker that returns the configured
/// responses, so `call` tasks can be tested without network access.
pub fn runtime_with_service(service: FakeServiceInvoker) -> Runtime {
    Runtime::builder()
        .with_clock(Arc::new(DeterministicClock::new()))
        .with_random(Arc::new(SeededRandomGenerator::new(42)))
        .with_service(Arc::new(service))
        .build()
        .expect("test runtime should build")
}

/// A deterministic clock wrapper exposing virtual time control for tests.
pub struct TestClock {
    clock: DeterministicClock,
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TestClock {
    /// Creates a new test clock.
    pub fn new() -> Self {
        Self {
            clock: DeterministicClock::new(),
        }
    }
    /// Advances the virtual clock by the given number of milliseconds.
    pub fn advance_ms(&self, ms: u64) {
        self.clock.advance(std::time::Duration::from_millis(ms));
    }

    /// Returns the current virtual epoch seconds.
    pub fn now(&self) -> i64 {
        self.clock.epoch_seconds()
    }

    /// Wraps the clock for injection into a runtime.
    pub fn arc(self) -> Arc<dyn Clock> {
        Arc::new(self.clock)
    }
}

/// A fake [`ServiceInvoker`] that returns scripted responses based on the
/// request URI and method.
#[derive(Clone, Default)]
pub struct FakeServiceInvoker {
    responses: Arc<Mutex<HashMap<String, ServiceResponse>>>,
    calls: Arc<Mutex<Vec<ServiceRequest>>>,
}

impl FakeServiceInvoker {
    /// Creates a new empty fake service invoker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a response for a `METHOD uri` key.
    pub fn stub(&self, method: &str, uri: &str, response: ServiceResponse) {
        self.responses
            .lock()
            .unwrap()
            .insert(format!("{method} {uri}"), response);
    }

    /// Returns the list of calls made, for assertion.
    pub fn calls(&self) -> Vec<ServiceRequest> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ServiceInvoker for FakeServiceInvoker {
    async fn invoke(&self, request: &ServiceRequest) -> Result<ServiceResponse, WorkflowError> {
        self.calls.lock().unwrap().push(request.clone());
        let key = format!(
            "{} {}",
            request.operation,
            request.uri.as_deref().unwrap_or("")
        );
        self.responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                WorkflowError::new(
                    ows_runtime_core::ErrorKind::Communication,
                    ows_runtime_core::ProblemDetails::standard(
                        ows_runtime_core::StandardErrorType::Communication,
                    )
                    .with_detail(format!("no stub for `{key}`")),
                )
            })
    }
}

/// A [`FunctionInvoker`] that drives the `http` function through a
/// [`FakeServiceInvoker`], so `call: http` tasks can be tested without network.
#[derive(Clone)]
pub struct FakeHttpFunction {
    service: FakeServiceInvoker,
}

impl FakeHttpFunction {
    /// Creates a new fake HTTP function over the given service.
    pub fn new(service: FakeServiceInvoker) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl ows_runtime::service::FunctionInvoker for FakeHttpFunction {
    async fn invoke(
        &self,
        req: ows_runtime::service::FunctionRequest<'_>,
    ) -> Result<serde_json::Value, WorkflowError> {
        let args = &req.args;
        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();
        let mut uri = args
            .get("endpoint")
            .and_then(|e| match e {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(m) => {
                    m.get("uri").and_then(|u| u.as_str()).map(|s| s.to_string())
                }
                _ => None,
            })
            .unwrap_or_default();

        // Expand `{var}` URI templates against `$workflow.input`.
        let workflow_input = req
            .context
            .workflow
            .as_ref()
            .and_then(|w| w.get("input"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let chars: Vec<char> = uri.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '{' {
                if let Some(close) = chars[i + 1..].iter().position(|c| *c == '}') {
                    let name: String = chars[i + 1..i + 1 + close].iter().collect();
                    if let Some(v) = workflow_input.get(&name) {
                        let replacement = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        uri = uri.replace(&format!("{{{name}}}"), &replacement);
                    }
                    i += close + 2;
                    continue;
                }
            }
            i += 1;
        }

        let request = ServiceRequest {
            service: "http".to_string(),
            operation: method,
            uri: Some(uri),
            headers: HashMap::new(),
            body: args.get("body").cloned(),
            query: HashMap::new(),
            timeout: None,
        };
        let response = self.service.invoke(&request).await?;
        Ok(response.body.unwrap_or(serde_json::Value::Null))
    }
}

/// Records every published event so tests can assert on them.
#[derive(Clone, Default)]
pub struct RecordingEventPublisher {
    events: Arc<Mutex<Vec<EventMessage>>>,
}

impl RecordingEventPublisher {
    /// Creates a new recording publisher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the recorded events.
    pub fn events(&self) -> Vec<EventMessage> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl EventPublisher for RecordingEventPublisher {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}
