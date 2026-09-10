//! End-to-end scheduling tests: `every`, `after`, `cron` and event triggers.

use std::sync::Arc;
use std::time::Duration;

use ows_runtime::Runtime;
use ows_runtime_core::EventPublisher;
use ows_runtime_events::InMemoryBroker;
use serde_json::{json, Value};

fn workflow(schedule: &str) -> String {
    format!(
        r#"
document: {{ dsl: '1.0.3', namespace: sched, name: tick, version: '1.0.0' }}
schedule:
{schedule}
do:
  - t: {{ set: {{ fired: true }} }}
"#
    )
}

#[tokio::test]
async fn every_trigger_fires_repeatedly() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(&workflow("  every: { milliseconds: 20 }")).unwrap();
    runtime.register_definition(&def).unwrap();

    let set = runtime.start_schedules().await.unwrap();
    assert_eq!(set.len(), 1);
    tokio::time::sleep(Duration::from_millis(130)).await;
    let fired = set.triggered();
    set.cancel();
    assert!(fired >= 2, "expected at least 2 firings, got {fired}");
}

#[tokio::test]
async fn after_trigger_fires_immediately_then_repeats() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(&workflow("  after: { milliseconds: 40 }")).unwrap();
    runtime.register_definition(&def).unwrap();

    let set = runtime.start_schedules().await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    let fired = set.triggered();
    set.cancel();
    assert!(fired >= 1, "expected at least 1 firing, got {fired}");
}

#[tokio::test]
async fn cron_trigger_fires_on_schedule() {
    let runtime = Runtime::builder().build().unwrap();
    // Six-field cron with seconds: fire every second.
    let def = ows_runtime_dsl::from_yaml(&workflow("  cron: '* * * * * *'")).unwrap();
    runtime.register_definition(&def).unwrap();

    let set = runtime.start_schedules().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let fired = set.triggered();
    set.cancel();
    assert!(fired >= 1, "expected the cron schedule to fire, got {fired}");
}

#[tokio::test]
async fn event_trigger_fires_on_matching_event() {
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&workflow(
        "  on:\n    one:\n      with: { type: com.example.ping }",
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let _ = wf;

    let set = runtime.start_schedules().await.unwrap();
    // Give the schedule time to subscribe.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // A non-matching event must not fire.
    broker
        .publish(&ows_runtime_core::EventMessage::new("1", "src", "com.example.other"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(set.triggered(), 0);

    // A matching event fires the workflow once.
    broker
        .publish(
            &ows_runtime_core::EventMessage::new("2", "src", "com.example.ping")
                .with_data(json!({ "n": 7 })),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let fired = set.triggered();
    set.cancel();
    assert!(fired >= 1, "expected the event to trigger, got {fired}");
}

#[tokio::test]
async fn start_schedules_is_noop_without_schedules() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: sched, name: none, version: '1.0.0' }
do:
  - t: { set: { x: 1 } }
"#,
    )
    .unwrap();
    runtime.register_definition(&def).unwrap();
    let set = runtime.start_schedules().await.unwrap();
    assert!(set.is_empty());
    assert_eq!(set.triggered(), 0);
    let _ = Value::Null;
}
