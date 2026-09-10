//! Broker-based AsyncAPI transport tests (no HTTP server required).
#![cfg(feature = "http")]

use std::sync::Arc;
use std::time::Duration;

use ows_runtime::Runtime;
use ows_runtime_core::{EventConsumer, EventPublisher};
use ows_runtime_events::InMemoryBroker;
use serde_json::{json, Value};

#[tokio::test]
async fn asyncapi_broker_publish_sends_event() {
    let broker = InMemoryBroker::new();
    let subscription = broker
        .subscribe(Box::new(|e| e.type_ == "com.example.notification"))
        .await
        .unwrap();
    let runtime = Runtime::builder()
        .with_event_publisher(Arc::new(broker.clone()))
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();

    let yaml = r#"
document: { dsl: '1.0.3', namespace: default, name: asyncapi-broker-pub, version: '1.0.0' }
do:
  - send:
      call: asyncapi
      with:
        document:
          channels:
            /notifications:
              publish:
                operationId: sendNotification
        channel: /notifications
        operationId: sendNotification
        transport:
          broker:
            type: kafka
        type: com.example.notification
        message: { hello: world }
"#;
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out["published"], true);
    assert_eq!(out["type"], "com.example.notification");

    let event = subscription
        .recv()
        .await
        .expect("event should be published");
    assert_eq!(event.type_, "com.example.notification");
    assert_eq!(event.data, Some(json!({ "hello": "world" })));
}

#[tokio::test]
async fn asyncapi_broker_subscribe_consumes_event() {
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();

    let yaml = r#"
document: { dsl: '1.0.3', namespace: default, name: asyncapi-broker-sub, version: '1.0.0' }
do:
  - receive:
      call: asyncapi
      with:
        document:
          channels:
            /notifications:
              subscribe:
                operationId: onNotification
        channel: /notifications
        operationId: onNotification
        transport:
          broker: {}
        count: 1
"#;
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let runtime2 = runtime.clone();
    let handle = tokio::spawn(async move { runtime2.run(wf, Value::Null).await });

    tokio::time::sleep(Duration::from_millis(100)).await;
    broker
        .publish(
            &ows_runtime_core::EventMessage::new(
                "n1",
                "/notifications",
                "com.example.notification",
            )
            .with_data(json!({ "id": 42 })),
        )
        .await
        .unwrap();

    let out = handle.await.unwrap().unwrap();
    assert_eq!(out, json!({ "id": 42 }));
}

#[tokio::test]
async fn asyncapi_broker_subscribe_applies_filter() {
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_consumer(Arc::new(broker.clone()))
        .build()
        .unwrap();

    let yaml = r#"
document: { dsl: '1.0.3', namespace: default, name: asyncapi-broker-filter, version: '1.0.0' }
do:
  - receive:
      call: asyncapi
      with:
        document:
          channels:
            /notifications:
              subscribe:
                operationId: onNotification
        channel: /notifications
        operationId: onNotification
        transport:
          broker: {}
        subscription:
          filter: '.kind == "alert"'
          consume:
            count: 1
"#;
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let runtime2 = runtime.clone();
    let handle = tokio::spawn(async move { runtime2.run(wf, Value::Null).await });

    tokio::time::sleep(Duration::from_millis(100)).await;
    // A non-matching event must be ignored.
    broker
        .publish(
            &ows_runtime_core::EventMessage::new("n1", "/notifications", "t")
                .with_data(json!({ "kind": "info" })),
        )
        .await
        .unwrap();
    // The matching event is consumed.
    broker
        .publish(
            &ows_runtime_core::EventMessage::new("n2", "/notifications", "t")
                .with_data(json!({ "kind": "alert", "id": 7 })),
        )
        .await
        .unwrap();

    let out = handle.await.unwrap().unwrap();
    assert_eq!(out["id"], 7);
}
