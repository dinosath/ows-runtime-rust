//! Tracing setup and OWS lifecycle event observability.
//!
//! The runtime emits structured `tracing` spans/events and OWS lifecycle cloud
//! events (per the DSL reference) through a pluggable [`EventPublisher`]. This
//! crate is not coupled to OpenTelemetry; an OpenTelemetry adapter can be added
//! separately on top of the same interfaces.
#![allow(clippy::result_large_err)]

use ows_runtime_core::{EventPublisher, WorkflowError};
use ows_runtime_events::CloudEvent;
use serde_json::{json, Value};

/// Initializes a default tracing subscriber suitable for a CLI or server.
///
/// This is a convenience for binaries; library consumers are free to configure
/// their own `tracing` subscriber.
pub fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("ows_runtime=info".parse().unwrap());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

/// Identifies a workflow for lifecycle events.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowRef {
    /// The workflow namespace.
    pub namespace: String,
    /// The workflow name.
    pub name: String,
    /// The workflow version.
    pub version: String,
}

impl WorkflowRef {
    /// The qualified name used in lifecycle events.
    pub fn qualified_name(&self) -> String {
        format!("{}-{}.{}", self.name, self.version, self.namespace)
    }
}

/// The OWS lifecycle event types.
pub mod event_types {
    pub const WORKFLOW_STARTED: &str = "io.serverlessworkflow.workflow.started.v1";
    pub const WORKFLOW_SUSPENDED: &str = "io.serverlessworkflow.workflow.suspended.v1";
    pub const WORKFLOW_RESUMED: &str = "io.serverlessworkflow.workflow.resumed.v1";
    pub const WORKFLOW_CORRELATION_STARTED: &str =
        "io.serverlessworkflow.workflow.correlation-started.v1";
    pub const WORKFLOW_CORRELATION_COMPLETED: &str =
        "io.serverlessworkflow.workflow.correlation-completed.v1";
    pub const WORKFLOW_CANCELLED: &str = "io.serverlessworkflow.workflow.cancelled.v1";
    pub const WORKFLOW_FAULTED: &str = "io.serverlessworkflow.workflow.faulted.v1";
    pub const WORKFLOW_COMPLETED: &str = "io.serverlessworkflow.workflow.completed.v1";
    pub const WORKFLOW_STATUS_CHANGED: &str = "io.serverlessworkflow.workflow.status-changed.v1";

    pub const TASK_CREATED: &str = "io.serverlessworkflow.task.created.v1";
    pub const TASK_STARTED: &str = "io.serverlessworkflow.task.started.v1";
    pub const TASK_SUSPENDED: &str = "io.serverlessworkflow.task.suspended.v1";
    pub const TASK_RESUMED: &str = "io.serverlessworkflow.task.resumed.v1";
    pub const TASK_RETRIED: &str = "io.serverlessworkflow.task.retried.v1";
    pub const TASK_CANCELLED: &str = "io.serverlessworkflow.task.cancelled.v1";
    pub const TASK_FAULTED: &str = "io.serverlessworkflow.task.faulted.v1";
    pub const TASK_COMPLETED: &str = "io.serverlessworkflow.task.completed.v1";
    pub const TASK_STATUS_CHANGED: &str = "io.serverlessworkflow.task.status-changed.v1";
}

/// Emits OWS lifecycle cloud events through an [`EventPublisher`].
#[derive(Clone)]
pub struct LifecycleEmitter {
    publisher: std::sync::Arc<dyn EventPublisher>,
}

impl LifecycleEmitter {
    /// Creates a new emitter.
    pub fn new(publisher: std::sync::Arc<dyn EventPublisher>) -> Self {
        Self { publisher }
    }

    /// The source used for lifecycle events.
    fn source(&self, wf: &WorkflowRef) -> String {
        format!("/{}/{}/{}", wf.namespace, wf.name, wf.version)
    }

    async fn emit(
        &self,
        id: &str,
        source: &str,
        event_type: &str,
        time: &str,
        data: Value,
    ) -> Result<(), WorkflowError> {
        let event = CloudEvent::new(id, source, event_type)
            .with_time(time)
            .with_data(data);
        self.publisher.publish(&event.to_message()).await
    }

    /// Emits a `workflow.started` lifecycle event.
    pub async fn workflow_started(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        started_at: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({
            "name": wf.qualified_name(),
            "definition": { "name": wf.name, "namespace": wf.namespace, "version": wf.version },
            "startedAt": started_at,
        });
        let id = format!("{execution_id}.started");
        self.emit(
            &id,
            &self.source(wf),
            event_types::WORKFLOW_STARTED,
            started_at,
            data,
        )
        .await
    }

    /// Emits a `workflow.completed` lifecycle event.
    pub async fn workflow_completed(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        completed_at: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({ "name": wf.qualified_name(), "completedAt": completed_at });
        let id = format!("{execution_id}.completed");
        self.emit(
            &id,
            &self.source(wf),
            event_types::WORKFLOW_COMPLETED,
            completed_at,
            data,
        )
        .await
    }

    /// Emits a `workflow.faulted` lifecycle event.
    pub async fn workflow_faulted(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        faulted_at: &str,
        error: &Value,
    ) -> Result<(), WorkflowError> {
        let data = json!({ "name": wf.qualified_name(), "faultedAt": faulted_at, "error": error });
        let id = format!("{execution_id}.faulted");
        self.emit(
            &id,
            &self.source(wf),
            event_types::WORKFLOW_FAULTED,
            faulted_at,
            data,
        )
        .await
    }

    /// Emits a `workflow.cancelled` lifecycle event.
    pub async fn workflow_cancelled(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        cancelled_at: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({ "name": wf.qualified_name(), "cancelledAt": cancelled_at });
        let id = format!("{execution_id}.cancelled");
        self.emit(
            &id,
            &self.source(wf),
            event_types::WORKFLOW_CANCELLED,
            cancelled_at,
            data,
        )
        .await
    }

    /// Emits a `workflow.status-changed` lifecycle event.
    pub async fn workflow_status_changed(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        from: &str,
        to: &str,
        time: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({ "name": wf.qualified_name(), "from": from, "to": to });
        let id = format!("{execution_id}.status.{to}");
        self.emit(
            &id,
            &self.source(wf),
            event_types::WORKFLOW_STATUS_CHANGED,
            time,
            data,
        )
        .await
    }

    /// Emits a `task.started` lifecycle event.
    pub async fn task_started(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        task_id: &str,
        task_name: &str,
        started_at: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({
            "name": wf.qualified_name(),
            "executionId": execution_id,
            "task": { "id": task_id, "name": task_name },
            "startedAt": started_at,
        });
        let id = format!("{execution_id}.{task_id}.started");
        self.emit(
            &id,
            &self.source(wf),
            event_types::TASK_STARTED,
            started_at,
            data,
        )
        .await
    }

    /// Emits a `task.completed` lifecycle event.
    pub async fn task_completed(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        task_id: &str,
        task_name: &str,
        completed_at: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({
            "name": wf.qualified_name(),
            "executionId": execution_id,
            "task": { "id": task_id, "name": task_name },
            "completedAt": completed_at,
        });
        let id = format!("{execution_id}.{task_id}.completed");
        self.emit(
            &id,
            &self.source(wf),
            event_types::TASK_COMPLETED,
            completed_at,
            data,
        )
        .await
    }

    /// Emits a `task.faulted` lifecycle event.
    pub async fn task_faulted(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        task_id: &str,
        task_name: &str,
        faulted_at: &str,
        error: &Value,
    ) -> Result<(), WorkflowError> {
        let data = json!({
            "name": wf.qualified_name(),
            "executionId": execution_id,
            "task": { "id": task_id, "name": task_name },
            "faultedAt": faulted_at,
            "error": error,
        });
        let id = format!("{execution_id}.{task_id}.faulted");
        self.emit(
            &id,
            &self.source(wf),
            event_types::TASK_FAULTED,
            faulted_at,
            data,
        )
        .await
    }

    /// Emits a `task.retried` lifecycle event.
    pub async fn task_retried(
        &self,
        wf: &WorkflowRef,
        execution_id: &str,
        task_id: &str,
        task_name: &str,
        attempt: u32,
        retried_at: &str,
    ) -> Result<(), WorkflowError> {
        let data = json!({
            "name": wf.qualified_name(),
            "executionId": execution_id,
            "task": { "id": task_id, "name": task_name },
            "attempt": attempt,
            "retriedAt": retried_at,
        });
        let id = format!("{execution_id}.{task_id}.retried");
        self.emit(
            &id,
            &self.source(wf),
            event_types::TASK_RETRIED,
            retried_at,
            data,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ows_runtime_core::EventMessage;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<EventMessage>>>);
    #[async_trait::async_trait]
    impl ows_runtime_core::EventPublisher for Recorder {
        async fn publish(&self, e: &EventMessage) -> Result<(), ows_runtime_core::WorkflowError> {
            self.0.lock().unwrap().push(e.clone());
            Ok(())
        }
    }

    fn wf() -> WorkflowRef {
        WorkflowRef {
            namespace: "ns".into(),
            name: "wf".into(),
            version: "1.0.0".into(),
        }
    }

    #[tokio::test]
    async fn workflow_started_event() {
        let r = Recorder::default();
        let em = LifecycleEmitter::new(Arc::new(r.clone()));
        em.workflow_started(&wf(), "exec", "2024-01-01T00:00:00Z")
            .await
            .unwrap();
        let events = r.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].type_, event_types::WORKFLOW_STARTED);
        assert_eq!(events[0].source, "/ns/wf/1.0.0");
        let data = serde_json::to_value(&events[0].data).unwrap();
        assert_eq!(data["name"], "wf-1.0.0.ns");
    }

    #[tokio::test]
    async fn workflow_completed_event() {
        let r = Recorder::default();
        let em = LifecycleEmitter::new(Arc::new(r.clone()));
        em.workflow_completed(&wf(), "exec", "2024-01-01T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(
            r.0.lock().unwrap()[0].type_,
            event_types::WORKFLOW_COMPLETED
        );
    }

    #[tokio::test]
    async fn workflow_faulted_event() {
        let r = Recorder::default();
        let em = LifecycleEmitter::new(Arc::new(r.clone()));
        em.workflow_faulted(
            &wf(),
            "exec",
            "2024-01-01T00:00:00Z",
            &serde_json::json!({"type":"x"}),
        )
        .await
        .unwrap();
        let e = &r.0.lock().unwrap()[0];
        assert_eq!(e.type_, event_types::WORKFLOW_FAULTED);
        let data = serde_json::to_value(&e.data).unwrap();
        assert_eq!(data["error"]["type"], "x");
    }

    #[tokio::test]
    async fn workflow_cancelled_and_status_changed() {
        let r = Recorder::default();
        let em = LifecycleEmitter::new(Arc::new(r.clone()));
        em.workflow_cancelled(&wf(), "exec", "t").await.unwrap();
        em.workflow_status_changed(&wf(), "exec", "running", "completed", "t")
            .await
            .unwrap();
        let ev = r.0.lock().unwrap();
        assert_eq!(ev[0].type_, event_types::WORKFLOW_CANCELLED);
        assert_eq!(ev[1].type_, event_types::WORKFLOW_STATUS_CHANGED);
    }

    #[tokio::test]
    async fn task_lifecycle_events() {
        let r = Recorder::default();
        let em = LifecycleEmitter::new(Arc::new(r.clone()));
        em.task_started(&wf(), "exec", "t1", "task1", "t")
            .await
            .unwrap();
        em.task_completed(&wf(), "exec", "t1", "task1", "t")
            .await
            .unwrap();
        em.task_retried(&wf(), "exec", "t1", "task1", 2, "t")
            .await
            .unwrap();
        em.task_faulted(&wf(), "exec", "t1", "task1", "t", &serde_json::json!({}))
            .await
            .unwrap();
        let ev = r.0.lock().unwrap();
        assert_eq!(ev[0].type_, event_types::TASK_STARTED);
        assert_eq!(ev[1].type_, event_types::TASK_COMPLETED);
        assert_eq!(ev[2].type_, event_types::TASK_RETRIED);
        assert_eq!(ev[3].type_, event_types::TASK_FAULTED);
    }
}
