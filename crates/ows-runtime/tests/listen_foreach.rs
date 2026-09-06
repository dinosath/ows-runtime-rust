//! Integration tests for the `listen` task's `foreach` iterator and its
//! `all` consumption strategy.

use ows_runtime::Runtime;
use ows_runtime_core::EventPublisher;
use ows_runtime_events::InMemoryBroker;
use serde_json::{json, Value};
use std::sync::Arc;

/// Runs a workflow in a spawned task and publishes the given `(type, data)`
/// events after the listener has had time to subscribe.
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
    // Give the listener time to subscribe before publishing.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
async fn listen_one_runs_foreach_over_single_event() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-foreach
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          one:
            with:
              type: com.example.thing
      foreach:
        item: ev
        at: idx
        do:
          - record:
              set:
                recorded: '${ { value: $ev, index: $idx } }'
"#;
    let out = run_listen(yaml, vec![("com.example.thing", json!({"n": 1}))]).await;
    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0]["recorded"],
        json!({ "value": { "n": 1 }, "index": 0 })
    );
}

#[tokio::test]
async fn listen_all_runs_foreach_over_each_event() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-foreach-all
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          all:
            - with: { type: com.example.a }
            - with: { type: com.example.b }
        read: data
      foreach:
        item: payload
        do:
          - stamp:
              set:
                got: '${ $payload.number }'
"#;
    // Publish one event matching the first filter and one matching the second;
    // the `all` strategy stops once every filter has matched.
    let out = run_listen(
        yaml,
        vec![
            ("com.example.a", json!({"number": 1})),
            ("com.example.b", json!({"number": 2})),
        ],
    )
    .await;
    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["got"], json!(1));
    assert_eq!(arr[1]["got"], json!(2));
}
