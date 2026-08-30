//! # ows-runtime-core
//!
//! Core types, traits and abstractions for the OWS runtime. This crate has no
//! knowledge of any particular expression language, HTTP framework, message
//! broker, database or scheduler. It defines the interfaces that the execution
//! engine (`ows-runtime`) depends on, keeping the engine portable and
//! deterministic.
#![allow(clippy::result_large_err)]

pub mod clock;
pub mod error;
pub mod expression;
pub mod phase;
pub mod policy;
pub mod runtime;
pub mod service;
pub mod store;
pub mod uuid;

pub use clock::{Clock, DeterministicClock, SystemClock};
pub use error::{ErrorKind, ProblemDetails, StandardErrorType, WorkflowError, ERROR_TYPE_PREFIX};
pub use expression::{
    CompiledExpression, ExpressionContext, ExpressionEngine, ExpressionError, ExpressionValue,
};
pub use phase::Phase;
pub use policy::{RetryBounds, RuntimePolicy};
pub use runtime::{
    EventMessageLike, RuntimeInfo, ScheduleTrigger, ScheduledExecution, ScheduledHandle, Scheduler,
};
pub use service::{
    EventConsumer, EventMessage, EventPublisher, EventSubscription, NoopEventConsumer,
    NullEventPublisher, ProcessResult, ProcessRunner, ServiceInvoker, ServiceRequest,
    ServiceResponse, TaskOutcome, TaskResult,
};
pub use store::{ExecutionRecord, ExecutionStore, InMemoryExecutionStore, StoredEvent};
pub use uuid::{
    DefaultRandomGenerator, DefaultUuidGenerator, RandomGenerator, SeededRandomGenerator,
    UuidGenerator,
};

#[cfg(test)]
mod integration_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn phase_terminals() {
        assert!(Phase::Completed.is_terminal());
        assert!(Phase::Faulted.is_terminal());
        assert!(Phase::Cancelled.is_terminal());
        assert!(!Phase::Running.is_terminal());
        assert_eq!(Phase::Running.to_string(), "running");
    }

    #[test]
    fn runtime_info_defaults() {
        let info = RuntimeInfo::default();
        assert_eq!(info.name, "ows-runtime");
        assert!(!info.version.is_empty());
        assert!(info.to_value().is_object());
    }

    #[test]
    fn scheduled_handle_cancel() {
        let mut called = false;
        let handle = ScheduledHandle::new("s1".to_string(), || {});
        (handle.cancel)();
        assert_eq!(handle.id, "s1");
        let _ = &mut called;
    }

    #[test]
    fn runtime_policy_host_and_scheme() {
        let p = RuntimePolicy::default();
        assert!(!p.host_allowed("example.com"));
        assert!(!p.scheme_allowed("https"));
        let p = RuntimePolicy {
            allow_network: true,
            allowed_hosts: vec!["example.com".into()],
            allowed_schemes: vec!["https".into()],
            ..Default::default()
        };
        assert!(p.host_allowed("example.com"));
        assert!(p.host_allowed("api.example.com"));
        assert!(!p.host_allowed("other.com"));
        assert!(p.scheme_allowed("https"));
        assert!(!p.scheme_allowed("http"));
    }

    #[test]
    fn expression_context_variables() {
        let ctx = ExpressionContext {
            context: json!({"a": 1}),
            input: json!({"b": 2}),
            output: json!({"c": 3}),
            secrets: [("token".to_string(), "secret".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let vars = ctx.as_variable_map();
        assert_eq!(vars["context"], json!({"a": 1}));
        assert_eq!(vars["input"], json!({"b": 2}));
        assert_eq!(vars["output"], json!({"c": 3}));
        assert_eq!(vars["secrets"]["token"], "secret");
        let upd = ctx.as_variable_map_updated(&json!({"x":1}), &json!({"y":2}));
        assert_eq!(upd["input"], json!({"x":1}));
        assert_eq!(upd["output"], json!({"y":2}));
    }

    #[tokio::test]
    async fn noop_event_publisher_and_consumer() {
        let p = NullEventPublisher;
        let msg = EventMessage::new("1", "s", "t");
        let res = crate::service::EventPublisher::publish(&p, &msg).await;
        assert!(res.is_ok());
    }

    #[test]
    fn task_outcome_and_result() {
        let r = TaskResult::new(json!({"x":1}));
        assert_eq!(r.output["x"], 1);
        assert_eq!(r.attempts, 1);
    }
}
