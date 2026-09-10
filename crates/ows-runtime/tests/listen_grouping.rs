//! Cross-event correlation grouping for `listen`.
//!
//! When an `all` listen declares correlation keys, events must agree on the
//! first-seen value of each key (a shared "group"), not just satisfy each filter
//! independently.

use std::sync::Arc;

use ows_runtime::Runtime;
use ows_runtime_core::EventPublisher;
use ows_runtime_events::InMemoryBroker;
use serde_json::{json, Value};

async fn run_listen(yaml: &str, input: Value, events: Vec<(&str, Value)>) -> Value {
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let runtime2 = runtime.clone();
    let handle = tokio::spawn(async move { runtime2.run(wf, input).await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    for (i, (type_, data)) in events.into_iter().enumerate() {
        broker
            .publish(
                &ows_runtime_core::EventMessage::new(
                    format!("id-{i}"),
                    "https://example.com/orders",
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
async fn all_requires_a_consistent_correlation_group() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-grouping
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          all:
            - with: { type: com.example.a }
              correlate:
                orderId:
                  from: '.orderId'
            - with: { type: com.example.b }
              correlate:
                orderId:
                  from: '.orderId'
        read: data
      foreach:
        item: ev
        do:
          - record:
              set:
                got: '${ $ev.orderId }'
"#;
    // The second event differs in `orderId`, so it must NOT satisfy the second
    // filter; only the third (same group as the first) completes the listen.
    let out = run_listen(
        yaml,
        Value::Null,
        vec![
            ("com.example.a", json!({ "orderId": 1 })),
            ("com.example.b", json!({ "orderId": 2 })),
            ("com.example.b", json!({ "orderId": 1 })),
        ],
    )
    .await;
    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 2, "expected two correlated events, got {out}");
    assert_eq!(arr[0]["got"], json!(1));
    assert_eq!(arr[1]["got"], json!(1));
}

#[tokio::test]
async fn expect_is_evaluated_against_the_context() {
    let yaml = r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: listen-expect-context
  version: '1.0.0'
do:
  - consume:
      listen:
        to:
          one:
            with: { type: com.example.order }
            correlate:
              orderId:
                from: '.orderId'
                expect: '.orderId'
"#;
    // The workflow input becomes the context, so `expect: .orderId` resolves to
    // 7 and the event with `orderId: 9` is ignored.
    let out = run_listen(
        yaml,
        json!({ "orderId": 7 }),
        vec![
            ("com.example.order", json!({ "orderId": 9 })),
            ("com.example.order", json!({ "orderId": 7 })),
        ],
    )
    .await;
    assert_eq!(out, json!([{ "orderId": 7 }]));
}
