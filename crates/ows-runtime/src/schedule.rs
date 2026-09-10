//! End-to-end workflow scheduling.
//!
//! A workflow may declare a `schedule` trigger (`every`, `after`, `cron`, or
//! `on` events). [`Runtime::start_schedules`] wires each registered workflow's
//! schedule to its execution: interval/cron triggers fire executions on the
//! injected [`Clock`](ows_runtime_core::Clock), and event triggers fire when a
//! matching event arrives on the configured `EventConsumer`.
//!
//! Scheduling is deterministic in tests: the clock is injectable, and the
//! returned [`ScheduleSet`] can be cancelled.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ows_runtime_core::{EventMessage, ScheduleTrigger, ScheduledExecution, WorkflowError};
use serde_json::{json, Value};

use crate::ir::CompiledWorkflow;
use crate::runtime::{Cancellation, Runtime};

/// A set of running schedules. Cancelling it (or dropping it) stops all of them.
pub struct ScheduleSet {
    pub(crate) cancel: Arc<Cancellation>,
    pub(crate) triggered: Arc<AtomicUsize>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl ScheduleSet {
    /// Requests cancellation of every running schedule.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The number of executions triggered so far.
    pub fn triggered(&self) -> usize {
        self.triggered.load(Ordering::SeqCst)
    }

    /// The number of schedules started.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Whether no schedules were started.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Awaits all schedule loops (they finish when cancelled).
    pub async fn join(mut self) {
        let handles = std::mem::take(&mut self.handles);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

impl Drop for ScheduleSet {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Runtime {
    /// Starts every registered workflow's schedule and returns a cancellable
    /// [`ScheduleSet`].
    ///
    /// Each schedule is also registered with the configured `Scheduler` (so a
    /// durable or distributed scheduler can observe/track it).
    pub async fn start_schedules(&self) -> Result<ScheduleSet, WorkflowError> {
        let cancel = Arc::new(Cancellation::new());
        let triggered = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        let workflows: Vec<Arc<CompiledWorkflow>> = {
            let guard = self.inner.workflows.lock().unwrap();
            guard.values().cloned().collect()
        };

        for workflow in workflows {
            let Some(schedule) = workflow.schedule.clone() else {
                continue;
            };

            // Let the configured scheduler observe the registration.
            let execution = ScheduledExecution {
                trigger: schedule.trigger.clone(),
                workflow_namespace: workflow.id.namespace.clone(),
                workflow_name: workflow.id.name.clone(),
                workflow_version: workflow.id.version.clone(),
            };
            if let Err(e) = self.inner.scheduler.schedule(execution).await {
                tracing::warn!(
                    workflow.name = %workflow.id.name,
                    error = %e,
                    "failed to register schedule with the configured scheduler"
                );
            }

            let runtime = self.clone();
            let cancel = cancel.clone();
            let triggered = triggered.clone();
            handles.push(tokio::spawn(async move {
                run_schedule(runtime, workflow, schedule.trigger, cancel, triggered).await;
            }));
        }

        Ok(ScheduleSet {
            cancel,
            triggered,
            handles,
        })
    }
}

/// Runs a single schedule trigger loop until cancelled.
async fn run_schedule(
    runtime: Runtime,
    workflow: Arc<CompiledWorkflow>,
    trigger: ScheduleTrigger,
    cancel: Arc<Cancellation>,
    triggered: Arc<AtomicUsize>,
) {
    match trigger {
        ScheduleTrigger::Every(interval) => loop {
            if !sleep_or_cancel(&runtime, interval, &cancel).await {
                return;
            }
            fire(&runtime, &workflow, Value::Null, &triggered).await;
        },
        ScheduleTrigger::After(delay) => loop {
            fire(&runtime, &workflow, Value::Null, &triggered).await;
            if !sleep_or_cancel(&runtime, delay, &cancel).await {
                return;
            }
        },
        ScheduleTrigger::Cron(expression) => {
            let schedule = match ows_runtime_scheduler::CronSchedule::parse(&expression) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(cron = %expression, error = %e, "invalid cron schedule");
                    return;
                }
            };
            loop {
                let now = runtime.inner.clock.epoch_seconds();
                let Some(next) = schedule.next_after_epoch(now) else {
                    return;
                };
                let delay = Duration::from_secs((next - now).max(0) as u64);
                if !sleep_or_cancel(&runtime, delay, &cancel).await {
                    return;
                }
                fire(&runtime, &workflow, Value::Null, &triggered).await;
            }
        }
        ScheduleTrigger::OnEvents(filters) => {
            let subscription = match runtime.inner.consumer.subscribe(Box::new(|_e| true)).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to subscribe for event-driven schedule");
                    return;
                }
            };
            loop {
                let recv = subscription.recv();
                tokio::pin!(recv);
                let event = tokio::select! {
                    ev = recv => ev,
                    _ = cancel.notified() => return,
                };
                let Some(event) = event else {
                    return;
                };
                if !event_matches(&event, &filters) {
                    continue;
                }
                let input = Value::Array(vec![event_envelope(&event)]);
                fire(&runtime, &workflow, input, &triggered).await;
            }
        }
    }
}

/// Executes the workflow once and counts the trigger.
async fn fire(
    runtime: &Runtime,
    workflow: &Arc<CompiledWorkflow>,
    input: Value,
    triggered: &Arc<AtomicUsize>,
) {
    triggered.fetch_add(1, Ordering::SeqCst);
    match runtime.run(workflow.clone(), input).await {
        Ok(_) => {}
        Err(e) if e.kind == ows_runtime_core::ErrorKind::Cancelled => {}
        Err(e) => {
            tracing::warn!(
                workflow.name = %workflow.id.name,
                error = %e,
                "scheduled execution failed"
            );
        }
    }
}

/// Sleeps for the duration using the runtime clock, returning `false` if
/// cancelled while waiting.
async fn sleep_or_cancel(runtime: &Runtime, duration: Duration, cancel: &Cancellation) -> bool {
    if cancel.is_cancelled() {
        return false;
    }
    let sleep = runtime.inner.clock.sleep(duration);
    tokio::pin!(sleep);
    tokio::select! {
        _ = &mut sleep => !cancel.is_cancelled(),
        _ = cancel.notified() => false,
    }
}

/// Returns whether an event matches any of the schedule's event filters.
fn event_matches(event: &EventMessage, filters: &[ows_runtime_core::EventMessageLike]) -> bool {
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|f| {
        let type_ok = f.type_ == "*" || f.type_ == event.type_;
        let source_ok = f
            .source
            .as_deref()
            .map(|s| s == event.source)
            .unwrap_or(true);
        type_ok && source_ok
    })
}

/// The CloudEvent envelope exposed to an event-driven execution's input.
fn event_envelope(event: &EventMessage) -> Value {
    let mut value = json!({
        "specversion": "1.0",
        "id": event.id,
        "source": event.source,
        "type": event.type_,
    });
    if let Some(data) = &event.data {
        value["data"] = data.clone();
    }
    if let Some(time) = &event.time {
        value["time"] = Value::String(time.clone());
    }
    if let Some(subject) = &event.subject {
        value["subject"] = Value::String(subject.clone());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_filter_matching() {
        let event = EventMessage::new("1", "src", "com.example.a");
        assert!(event_matches(&event, &[]));
        assert!(event_matches(
            &event,
            &[ows_runtime_core::EventMessageLike {
                type_: "com.example.a".into(),
                source: None,
            }]
        ));
        assert!(event_matches(
            &event,
            &[ows_runtime_core::EventMessageLike {
                type_: "*".into(),
                source: None,
            }]
        ));
        assert!(!event_matches(
            &event,
            &[ows_runtime_core::EventMessageLike {
                type_: "com.example.b".into(),
                source: None,
            }]
        ));
        assert!(!event_matches(
            &event,
            &[ows_runtime_core::EventMessageLike {
                type_: "com.example.a".into(),
                source: Some("other".into()),
            }]
        ));
    }

    #[test]
    fn event_envelope_shape() {
        let mut event = EventMessage::new("id-1", "src", "t");
        event.data = Some(json!({ "x": 1 }));
        let env = event_envelope(&event);
        assert_eq!(env["id"], "id-1");
        assert_eq!(env["type"], "t");
        assert_eq!(env["data"]["x"], 1);
    }
}
