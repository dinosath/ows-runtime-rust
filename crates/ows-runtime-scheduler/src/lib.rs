//! Scheduling for the OWS runtime.
//!
//! The core runtime depends on the [`ows_runtime_core::Scheduler`] trait rather
//! than a specific scheduler. This crate provides a `no-op` scheduler, a
//! recording scheduler for tests, and cron expression support via the `cron`
//! crate. The engine keeps scheduling behind the trait so future schedulers
//! (e.g. durable, distributed) can be added without changing workflow
//! semantics.
#![allow(clippy::result_large_err)]

use ows_runtime_core::{
    ScheduleTrigger, ScheduledExecution, ScheduledHandle, Scheduler, WorkflowError,
};
use std::str::FromStr;
use std::sync::Arc;

/// A scheduler that does nothing. Useful when no scheduling is required.
#[derive(Debug, Clone, Default)]
pub struct NoopScheduler;

#[async_trait::async_trait]
impl Scheduler for NoopScheduler {
    async fn schedule(
        &self,
        execution: ScheduledExecution,
    ) -> Result<ScheduledHandle, WorkflowError> {
        let id = format!("{:?}-{}", execution.trigger, std::process::id());
        Ok(ScheduledHandle::new(id, || {}))
    }
}

/// A scheduler that records every scheduled execution without firing them.
///
/// Intended for tests that only need to assert that a schedule was registered.
#[derive(Debug, Clone, Default)]
pub struct RecordingScheduler {
    scheduled: Arc<std::sync::Mutex<Vec<ScheduledExecution>>>,
}

impl RecordingScheduler {
    /// Creates a new recording scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of all recorded scheduled executions.
    pub fn recorded(&self) -> Vec<ScheduledExecution> {
        self.scheduled.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Scheduler for RecordingScheduler {
    async fn schedule(
        &self,
        execution: ScheduledExecution,
    ) -> Result<ScheduledHandle, WorkflowError> {
        let id = format!("sched-{}", self.scheduled.lock().unwrap().len());
        self.scheduled.lock().unwrap().push(execution);
        let done = Arc::clone(&self.scheduled);
        let cancel = move || {
            // No-op cancellation for the recording scheduler.
            let _ = &done;
        };
        Ok(ScheduledHandle::new(id, cancel))
    }
}

/// A parsed cron expression.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    schedule: cron::Schedule,
    /// The original cron string.
    pub expression: String,
}

impl CronSchedule {
    /// Parses a cron expression.
    pub fn parse(expression: &str) -> Result<Self, WorkflowError> {
        let schedule = cron::Schedule::from_str(expression).map_err(|e| {
            let problem = ows_runtime_core::ProblemDetails::standard(
                ows_runtime_core::StandardErrorType::Validation,
            )
            .with_detail(format!("invalid cron expression `{expression}`: {e}"));
            WorkflowError::new(ows_runtime_core::ErrorKind::Schema, problem)
        })?;
        Ok(Self {
            schedule,
            expression: expression.to_string(),
        })
    }

    /// Returns the next run time after the given time (as epoch seconds).
    pub fn next_after_epoch(&self, epoch_seconds: i64) -> Option<i64> {
        use chrono::TimeZone;
        let dt = chrono::Utc.timestamp_opt(epoch_seconds, 0).single()?;
        self.schedule.after(&dt).next().map(|t| t.timestamp())
    }

    /// Returns the next N run times after the given epoch.
    pub fn next_after_epoch_n(&self, epoch_seconds: i64, n: usize) -> Vec<i64> {
        use chrono::TimeZone;
        let Some(dt) = chrono::Utc.timestamp_opt(epoch_seconds, 0).single() else {
            return Vec::new();
        };
        self.schedule
            .after(&dt)
            .take(n)
            .map(|t| t.timestamp())
            .collect()
    }
}

/// Returns a human description of a schedule trigger.
pub fn describe_trigger(trigger: &ScheduleTrigger) -> String {
    match trigger {
        ScheduleTrigger::Every(d) => format!("every {}ms", d.as_millis()),
        ScheduleTrigger::After(d) => format!("after {}ms", d.as_millis()),
        ScheduleTrigger::Cron(c) => format!("cron `{c}`"),
        ScheduleTrigger::OnEvents(evts) => {
            let types: Vec<&str> = evts.iter().map(|e| e.type_.as_str()).collect();
            format!("on events [{}]", types.join(", "))
        }
    }
}
