//! Abstractions for external service / resource invocation.
//!
//! The core execution engine never hard-codes HTTP clients, brokers, databases
//! or cloud providers. All external interactions are behind these traits, with
//! concrete adapters provided as separate crates or behind feature flags.

use std::collections::HashMap;

use serde_json::Value;

use crate::error::WorkflowError;
use crate::expression::{ExpressionContext, ExpressionValue};

/// A generic outbound service request.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServiceRequest {
    /// The service to call (e.g. a scheme + host, or a logical name).
    pub service: String,
    /// The operation to perform (e.g. an HTTP method).
    pub operation: String,
    /// The destination URI, if applicable.
    pub uri: Option<String>,
    /// The request headers.
    pub headers: HashMap<String, String>,
    /// The request body, if any.
    pub body: Option<Value>,
    /// The query parameters, if any.
    pub query: HashMap<String, Value>,
    /// The timeout for the call, if any.
    pub timeout: Option<std::time::Duration>,
}

/// A generic outbound service response.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServiceResponse {
    /// The response status code.
    pub status: u16,
    /// The response headers.
    pub headers: HashMap<String, String>,
    /// The deserialized response body, if any.
    pub body: Option<Value>,
    /// The raw response body, if any.
    pub raw: Option<Vec<u8>>,
    /// The content type, if any.
    pub content_type: Option<String>,
}

/// Invokes external services.
///
/// This is a generic, protocol-agnostic service invoker. Concrete adapters
/// (HTTP, gRPC, AsyncAPI, ...) implement it. HTTP is provided as the first
/// concrete integration.
#[async_trait::async_trait]
pub trait ServiceInvoker: Send + Sync {
    /// Invokes the service and returns the response.
    async fn invoke(&self, request: &ServiceRequest) -> Result<ServiceResponse, WorkflowError>;
}

/// A request to invoke a workflow function (`call`).
#[derive(Debug, Clone)]
pub struct FunctionRequest<'a> {
    /// The name of the function being called.
    pub name: String,
    /// The function arguments (`with`).
    pub args: HashMap<String, Value>,
    /// The expression context.
    pub context: &'a ExpressionContext,
}

/// A request to publish an event.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventMessage {
    /// The CloudEvent id.
    pub id: String,
    /// The CloudEvent source.
    pub source: String,
    /// The CloudEvent type.
    pub type_: String,
    /// The CloudEvent time.
    pub time: Option<String>,
    /// The CloudEvent subject, if any.
    pub subject: Option<String>,
    /// The CloudEvent data content type.
    pub data_content_type: Option<String>,
    /// The CloudEvent data schema, if any.
    pub data_schema: Option<String>,
    /// The CloudEvent data.
    pub data: Option<Value>,
    /// Extension attributes.
    pub extensions: HashMap<String, Value>,
}

impl EventMessage {
    /// Creates a new event message.
    pub fn new(id: impl Into<String>, source: impl Into<String>, type_: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            type_: type_.into(),
            time: None,
            subject: None,
            data_content_type: None,
            data_schema: None,
            data: None,
            extensions: HashMap::new(),
        }
    }

    /// Sets the event data.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Sets the event time.
    pub fn with_time(mut self, time: impl Into<String>) -> Self {
        self.time = Some(time.into());
        self
    }
}

/// Publishes events to a broker / transport.
#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    /// Publishes an event.
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError>;
}

/// A handle to a subscription for incoming events.
#[async_trait::async_trait]
pub trait EventSubscription: Send + Sync {
    /// Receives the next event, or `None` if the subscription is closed.
    async fn recv(&self) -> Option<EventMessage>;
}

/// Consumes events from a broker / transport.
#[async_trait::async_trait]
pub trait EventConsumer: Send + Sync {
    /// Subscribes to events matching the given filter predicate.
    ///
    /// The predicate receives the raw event and returns whether it matches.
    async fn subscribe(
        &self,
        filter: Box<dyn for<'a> Fn(&'a EventMessage) -> bool + Send + Sync>,
    ) -> Result<Arc<dyn EventSubscription>, WorkflowError>;
}

use std::sync::Arc;

/// A no-op event publisher that discards every event.
#[derive(Debug, Clone, Default)]
pub struct NullEventPublisher;

#[async_trait::async_trait]
impl EventPublisher for NullEventPublisher {
    async fn publish(&self, _event: &EventMessage) -> Result<(), WorkflowError> {
        Ok(())
    }
}

/// A no-op event consumer that never delivers events.
#[derive(Debug, Clone, Default)]
pub struct NoopEventConsumer;

#[async_trait::async_trait]
impl EventConsumer for NoopEventConsumer {
    async fn subscribe(
        &self,
        _filter: Box<dyn for<'a> Fn(&'a EventMessage) -> bool + Send + Sync>,
    ) -> Result<Arc<dyn EventSubscription>, WorkflowError> {
        Ok(Arc::new(NoopSubscription))
    }
}

struct NoopSubscription;

#[async_trait::async_trait]
impl EventSubscription for NoopSubscription {
    async fn recv(&self) -> Option<EventMessage> {
        // Never returns.
        std::future::pending().await
    }
}

/// The result of running a process (`run` task).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessResult {
    /// The process exit code.
    pub code: Option<i32>,
    /// The captured STDOUT.
    pub stdout: Option<String>,
    /// The captured STDERR.
    pub stderr: Option<String>,
}

/// Executes a `run` process (container, script, shell).
///
/// This is where script execution is gated behind policy. Implementations must
/// respect cancellation and timeouts.
#[async_trait::async_trait]
pub trait ProcessRunner: Send + Sync {
    /// Runs a shell command.
    async fn run_shell(
        &self,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError>;

    /// Runs a script.
    async fn run_script(
        &self,
        language: &str,
        code: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError>;

    /// Runs a container.
    async fn run_container(
        &self,
        image: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError>;
}

/// The result of a task execution.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
    /// The task ran to completion.
    Completed(ExpressionValue),
    /// The task was cancelled.
    Cancelled,
}

/// A task result carrying both output and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskResult {
    /// The task output.
    pub output: ExpressionValue,
    /// The number of attempts taken.
    pub attempts: u32,
}

impl TaskResult {
    /// Creates a new task result.
    pub fn new(output: ExpressionValue) -> Self {
        Self {
            output,
            attempts: 1,
        }
    }
}
