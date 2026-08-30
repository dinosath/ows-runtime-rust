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
