//! # ows-runtime
//!
//! The execution engine for the Open Workflow Specification. This crate is
//! library-first: it loads OWS definitions (via `ows-runtime-dsl` on top of the
//! official SDK), validates, compiles to an executable IR, and executes
//! workflows asynchronously with support for expressions, tasks, events,
//! scheduling, retries, timeouts, cancellation and observability.
//!
//! The execution engine is independent of HTTP frameworks, databases, message
//! brokers and cloud providers; those are adapters behind the core traits.
#![allow(clippy::result_large_err)]

pub mod catalog;
pub mod compile;
pub mod engine;
pub mod error;
pub mod ir;
pub mod runtime;
pub mod schedule;
pub mod service;
pub mod tasks;

pub use ows_runtime_dsl::workflow_id;
pub use ows_runtime_dsl::WorkflowId;
pub use runtime::{ExecutionHandle, Runtime, RuntimeBuilder};
pub use schedule::ScheduleSet;

/// Re-exports of the official SDK models, used throughout the runtime.
pub mod dsl_models {
    pub use serverless_workflow_core::models::authentication::*;
    pub use serverless_workflow_core::models::catalog::*;
    pub use serverless_workflow_core::models::duration::*;
    pub use serverless_workflow_core::models::error::*;
    pub use serverless_workflow_core::models::event::*;
    pub use serverless_workflow_core::models::extension::*;
    pub use serverless_workflow_core::models::input::*;
    pub use serverless_workflow_core::models::map::*;
    pub use serverless_workflow_core::models::output::*;
    pub use serverless_workflow_core::models::resource::*;
    pub use serverless_workflow_core::models::retry::*;
    pub use serverless_workflow_core::models::schema::*;
    pub use serverless_workflow_core::models::task::*;
    pub use serverless_workflow_core::models::timeout::*;
    pub use serverless_workflow_core::models::workflow::*;
}
