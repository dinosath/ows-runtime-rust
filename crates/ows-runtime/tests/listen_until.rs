//! Integration tests for the `listen` `until` stop condition.

use std::sync::Arc;

use ows_runtime::Runtime;
use ows_runtime_core::{ErrorKind, EventPublisher};
use ows_runtime_events::InMemoryBroker;
use serde_json::{json, Value};

async fn run_listen(yaml: &str, events: Vec<(&str, Value)>) -> Value {
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let runtime2 = runtime.clone();
    let handle = tokio::spawn(async move { runtime2.run(wf, Value::Null).await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    for (i, (type_, data)) in events.into_iter().enumerate() {
        broker
            .publish(
                &ows_runtime_core::EventMessage::new(
                    format!("id-{i}"),
                    "https://example.com/things",
                    type_.to_string(),
                )
                .with_data(data),
            )
            .await
            .unwrap();
    }
    handle.await.unwrap().unwrap()
}

#[tokio::test]
async fn until_expression_stops_after_enough_events() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-until-expression
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          any: []
          until: '. | length >= 3'
"#;
    let out = run_listen(
        yaml,
        vec![
            ("t", json!({"n": 1})),
            ("t", json!({"n": 2})),
            ("t", json!({"n": 3})),
            ("t", json!({"n": 4})),
        ],
    )
    .await;
    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 3, "until should stop after 3 events: {out}");
    assert_eq!(arr[2]["n"], json!(3));
}

#[tokio::test]
async fn until_strategy_consumes_the_stop_event() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-until-strategy
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          any: []
          until:
            one:
              with:
                type: com.example.stop
"#;
    let out = run_listen(
        yaml,
        vec![
            ("com.example.a", json!({"n": 1})),
            ("com.example.a", json!({"n": 2})),
            ("com.example.stop", json!({"n": 3})),
        ],
    )
    .await;
    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 3, "the stop event is consumed too: {out}");
    assert_eq!(arr[2]["n"], json!(3));
}

#[tokio::test]
async fn until_false_keeps_listening_until_cancelled() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-until-forever
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          any: []
          until: 'false'
"#;
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let handle = runtime.execute(wf, Value::Null).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    broker
        .publish(&ows_runtime_core::EventMessage::new("1", "src", "t").with_data(json!({ "n": 1 })))
        .await
        .unwrap();
    broker
        .publish(&ows_runtime_core::EventMessage::new("2", "src", "t").with_data(json!({ "n": 2 })))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle.cancel();
    let err = handle.wait().await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
}
