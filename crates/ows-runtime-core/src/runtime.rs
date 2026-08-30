//! Runtime identity and scheduler abstractions.

use std::sync::Arc;

use serde_json::{Map, Value};

/// Information about the runtime, exposed as the `$runtime` expression argument.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeInfo {
    /// A human friendly name for the runtime.
    pub name: String,
    /// The version of the runtime.
    pub version: String,
    /// Implementation specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

impl Default for RuntimeInfo {
    fn default() -> Self {
        Self {
            name: "ows-runtime".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            metadata: None,
        }
    }
}

impl RuntimeInfo {
    /// Returns the runtime info as a JSON value for expression arguments.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// A trigger for a scheduled workflow execution.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ScheduleTrigger {
    /// A fixed interval.
    Every(std::time::Duration),
    /// A delay after completion.
    After(std::time::Duration),
    /// A cron schedule.
    Cron(String),
    /// One or more events.
    OnEvents(Vec<EventMessageLike>),
}

/// A placeholder for event-based triggers (see the events crate for the real type).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventMessageLike {
    /// The event type.
    pub type_: String,
    /// The event source, if any.
    pub source: Option<String>,
}

/// A scheduled execution task.
#[derive(Debug, Clone)]
pub struct ScheduledExecution {
    /// The trigger configuration.
    pub trigger: ScheduleTrigger,
    /// The workflow to execute (by identity).
    pub workflow_namespace: String,
    pub workflow_name: String,
    pub workflow_version: String,
}

/// A scheduler abstraction.
///
/// The core runtime depends on this trait, not on a specific scheduler. A
/// deterministic test scheduler and a cron-backed scheduler are provided.
#[async_trait::async_trait]
pub trait Scheduler: Send + Sync {
    /// Registers a scheduled execution and returns a handle that can be used to
    /// cancel it.
    async fn schedule(
        &self,
        execution: ScheduledExecution,
    ) -> Result<ScheduledHandle, crate::error::WorkflowError>;
}

/// A handle to a scheduled execution.
#[derive(Clone)]
pub struct ScheduledHandle {
    /// The schedule id.
    pub id: String,
    /// Cancels the scheduled execution.
    pub cancel: Arc<dyn Fn() + Send + Sync>,
}

impl ScheduledHandle {
    /// Creates a new schedule handle.
    pub fn new(id: String, cancel: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            id,
            cancel: Arc::new(cancel),
        }
    }
}
