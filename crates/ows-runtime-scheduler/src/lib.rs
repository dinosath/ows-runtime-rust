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

#[cfg(test)]
mod tests {
    use super::*;
    use ows_runtime_core::{ScheduleTrigger, ScheduledExecution};
    use std::time::Duration;

    #[tokio::test]
    async fn noop_scheduler_registers() {
        let s = NoopScheduler;
        let handle = s
            .schedule(ScheduledExecution {
                trigger: ScheduleTrigger::Every(Duration::from_secs(1)),
                workflow_namespace: "ns".into(),
                workflow_name: "wf".into(),
                workflow_version: "1".into(),
            })
            .await
            .unwrap();
        (handle.cancel)();
        assert!(!handle.id.is_empty());
    }

    #[tokio::test]
    async fn recording_scheduler_records() {
        let s = RecordingScheduler::new();
        s.schedule(ScheduledExecution {
            trigger: ScheduleTrigger::Cron("0 * * * *".into()),
            workflow_namespace: "ns".into(),
            workflow_name: "wf".into(),
            workflow_version: "1".into(),
        })
        .await
        .unwrap();
        assert_eq!(s.recorded().len(), 1);
        assert!(matches!(s.recorded()[0].trigger, ScheduleTrigger::Cron(_)));
    }

    #[test]
    fn cron_schedule_parses_and_next() {
        let c = CronSchedule::parse("0 0 * * * *").unwrap();
        assert_eq!(c.expression, "0 0 * * * *");
        let next = c.next_after_epoch(0).unwrap();
        assert!(next > 0);
        let n = c.next_after_epoch_n(0, 3);
        assert_eq!(n.len(), 3);
    }

    #[test]
    fn cron_invalid_fails() {
        assert!(CronSchedule::parse("not a cron").is_err());
    }

    #[test]
    fn describe_trigger_human() {
        assert!(
            describe_trigger(&ScheduleTrigger::Every(Duration::from_secs(1))).contains("every")
        );
        assert!(describe_trigger(&ScheduleTrigger::Cron("* * * * *".into())).contains("cron"));
        assert!(
            describe_trigger(&ScheduleTrigger::After(Duration::from_secs(1))).contains("after")
        );
        assert!(describe_trigger(&ScheduleTrigger::OnEvents(vec![
            ows_runtime_core::EventMessageLike {
                type_: "t".into(),
                source: None
            }
        ]))
        .contains("on events"));
    }
}
