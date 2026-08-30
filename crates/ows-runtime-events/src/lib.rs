//! CloudEvents, event matching and an in-memory event broker for the OWS runtime.
//!
//! This crate implements the CloudEvents shape used by OWS lifecycle and
//! workflow events, plus event matching/correlation primitives and an in-memory
//! broker that implements the core [`EventPublisher`] / [`EventConsumer`] traits
//! so workflows can be tested without Kafka, NATS or other transports.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use ows_runtime_core::{
    EventConsumer, EventMessage, EventPublisher, EventSubscription, WorkflowError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, Mutex};

/// The CloudEvents specification version emitted by the runtime.
pub const CLOUDEVENTS_SPEC_VERSION: &str = "1.0";

/// A CloudEvent as defined by the CloudEvents 1.0 specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudEvent {
    /// Identifies the event.
    pub id: String,
    /// The context in which an event happened.
    pub source: String,
    /// The type of event.
    #[serde(rename = "type")]
    pub type_: String,
    /// The version of the CloudEvents specification.
    pub specversion: String,
    /// The timestamp of the occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// The subject of the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The content type of `data`.
    #[serde(rename = "datacontenttype", skip_serializing_if = "Option::is_none")]
    pub data_content_type: Option<String>,
    /// The schema that `data` adheres to.
    #[serde(rename = "dataschema", skip_serializing_if = "Option::is_none")]
    pub data_schema: Option<String>,
    /// The event payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Extension context attributes.
    #[serde(flatten)]
    pub extensions: Map<String, Value>,
}

impl CloudEvent {
    /// Creates a new CloudEvent with the given id, source and type.
    pub fn new(id: impl Into<String>, source: impl Into<String>, type_: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            type_: type_.into(),
            specversion: CLOUDEVENTS_SPEC_VERSION.to_string(),
            time: None,
            subject: None,
            data_content_type: None,
            data_schema: None,
            data: None,
            extensions: Map::new(),
        }
    }

    /// Sets the event data.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Sets the event time.
    pub fn with_time(mut self, time: impl Into<String>) -> Self {
        self.time = Some(time.into());
        self
    }

    /// Converts the CloudEvent into the core [`EventMessage`] type.
    pub fn to_message(&self) -> EventMessage {
        let mut msg = EventMessage::new(&self.id, &self.source, &self.type_);
        msg.time = self.time.clone();
        msg.subject = self.subject.clone();
        msg.data_content_type = self.data_content_type.clone();
        msg.data_schema = self.data_schema.clone();
        msg.data = self.data.clone();
        msg.extensions = self
            .extensions
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        msg
    }

    /// Builds a CloudEvent from a core [`EventMessage`].
    pub fn from_message(msg: &EventMessage) -> Self {
        Self {
            id: msg.id.clone(),
            source: msg.source.clone(),
            type_: msg.type_.clone(),
            specversion: CLOUDEVENTS_SPEC_VERSION.to_string(),
            time: msg.time.clone(),
            subject: msg.subject.clone(),
            data_content_type: msg.data_content_type.clone(),
            data_schema: msg.data_schema.clone(),
            data: msg.data.clone(),
            extensions: msg
                .extensions
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

/// An in-memory event subscription receiving events on an unbounded channel.
pub struct InMemorySubscription {
    rx: Mutex<mpsc::UnboundedReceiver<EventMessage>>,
}

#[async_trait::async_trait]
impl EventSubscription for InMemorySubscription {
    async fn recv(&self) -> Option<EventMessage> {
        self.rx.lock().await.recv().await
    }
}

/// An in-memory event broker implementing both [`EventPublisher`] and
/// [`EventConsumer`]. It broadcasts every published event to all active
/// subscribers that match their filter.
#[derive(Clone, Default)]
pub struct InMemoryBroker {
    subscribers: Arc<Mutex<Vec<InMemorySubscriber>>>,
}

struct InMemorySubscriber {
    filter: Box<dyn for<'a> Fn(&'a EventMessage) -> bool + Send + Sync>,
    tx: mpsc::UnboundedSender<EventMessage>,
}

impl InMemoryBroker {
    /// Creates a new empty broker.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl EventPublisher for InMemoryBroker {
    async fn publish(&self, event: &EventMessage) -> Result<(), WorkflowError> {
        let subscribers = self.subscribers.lock().await;
        for sub in subscribers.iter() {
            if (sub.filter)(event) {
                let _ = sub.tx.send(event.clone());
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EventConsumer for InMemoryBroker {
    async fn subscribe(
        &self,
        filter: Box<dyn for<'a> Fn(&'a EventMessage) -> bool + Send + Sync>,
    ) -> Result<Arc<dyn EventSubscription>, WorkflowError> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .await
            .push(InMemorySubscriber { filter, tx });
        Ok(Arc::new(InMemorySubscription { rx: Mutex::new(rx) }))
    }
}

/// A field constraint used by an event filter.
pub enum FieldConstraint {
    /// The value must be exactly equal.
    Equals(Value),
    /// The value must match a regular expression.
    MatchesRegex(regex::Regex),
    /// The value must satisfy a runtime expression predicate.
    Expression(Box<dyn Fn(&CloudEvent) -> bool + Send + Sync>),
}

/// Matches CloudEvents against a set of field constraints.
#[derive(Default)]
pub struct EventMatcher {
    /// The event type to match, if any.
    pub type_: Option<String>,
    /// The event source to match, if any.
    pub source: Option<String>,
    /// The event subject to match, if any.
    pub subject: Option<String>,
    /// Additional attribute constraints.
    pub attributes: Vec<(String, FieldConstraint)>,
}

impl EventMatcher {
    /// Creates an empty matcher (matches all events).
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether the given event matches all constraints.
    pub fn matches(&self, event: &CloudEvent) -> bool {
        if let Some(t) = &self.type_ {
            if t != &event.type_ {
                return false;
            }
        }
        if let Some(s) = &self.source {
            if s != &event.source {
                return false;
            }
        }
        if let Some(s) = &self.subject {
            if s != event.subject.as_deref().unwrap_or("") {
                return false;
            }
        }
        for (key, constraint) in &self.attributes {
            let value = match key.as_str() {
                "type" => Value::String(event.type_.clone()),
                "source" => Value::String(event.source.clone()),
                "subject" => event
                    .subject
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
                "data" => event.data.clone().unwrap_or(Value::Null),
                "id" => Value::String(event.id.clone()),
                other => event
                    .extensions
                    .get(other)
                    .cloned()
                    .or_else(|| event.data.as_ref().and_then(|d| d.get(other).cloned()))
                    .unwrap_or(Value::Null),
            };
            let ok = match constraint {
                FieldConstraint::Equals(expected) => values_equal(&value, expected),
                FieldConstraint::MatchesRegex(re) => match &value {
                    Value::String(s) => re.is_match(s),
                    _ => false,
                },
                FieldConstraint::Expression(f) => f(event),
            };
            if !ok {
                return false;
            }
        }
        true
    }
}

/// Compares two values for equality, treating numbers numerically.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64().unwrap() == y.as_f64().unwrap(),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).map(|bv| values_equal(v, bv)).unwrap_or(false))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(xv, yv)| values_equal(xv, yv))
        }
        _ => a == b,
    }
}
