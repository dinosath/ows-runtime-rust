//! Parsing, serialization and validation of Open Workflow Specification
//! definitions.
//!
//! This crate uses the official [`serverless_workflow_core`] SDK types as the
//! primary representation of OWS workflow definitions. It provides parsing from
//! YAML and JSON, serialization, and a validation pass that distinguishes
//! parse, schema and semantic errors per the OWS compilation pipeline.
#![allow(clippy::result_large_err)]

use std::path::Path;

use serde_json::Value;
use serverless_workflow_core::models::workflow::WorkflowDefinition;
use thiserror::Error;

/// Errors that can occur while parsing or validating a workflow definition.
#[derive(Debug, Error)]
pub enum DefinitionError {
    /// The input could not be decoded (malformed YAML / JSON, bad UTF-8, ...).
    #[error("parse error: {0}")]
    Parse(String),
    /// The definition violates the OWS schema.
    #[error("schema error: {0}")]
    Schema(String),
    /// The structure is syntactically valid but cannot be executed per OWS semantics.
    #[error("semantic error: {0}")]
    Semantic(String),
    /// Reading the source failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl DefinitionError {
    /// Whether this is a parse error.
    pub fn is_parse(&self) -> bool {
        matches!(self, DefinitionError::Parse(_))
    }
    /// Whether this is a schema error.
    pub fn is_schema(&self) -> bool {
        matches!(self, DefinitionError::Schema(_))
    }
    /// Whether this is a semantic error.
    pub fn is_semantic(&self) -> bool {
        matches!(self, DefinitionError::Semantic(_))
    }
}

/// Parses a workflow definition from a YAML string.
pub fn from_yaml(yaml: &str) -> Result<WorkflowDefinition, DefinitionError> {
    let mut value: serde_json::Value =
        serde_yaml::from_str(yaml).map_err(|e| DefinitionError::Parse(e.to_string()))?;
    normalize_definition(&mut value);
    serde_json::from_value(value).map_err(|e| DefinitionError::Parse(e.to_string()))
}

/// Parses a workflow definition from a JSON string.
pub fn from_json(json: &str) -> Result<WorkflowDefinition, DefinitionError> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| DefinitionError::Parse(e.to_string()))?;
    normalize_definition(&mut value);
    serde_json::from_value(value).map_err(|e| DefinitionError::Parse(e.to_string()))
}

/// Applies OWS default normalization that the underlying SDK model does not
/// express, so parsed definitions remain spec-compliant.
///
/// Currently this injects the default `fork.compete: false` where omitted (the
/// spec defaults `compete` to `false`, but the SDK model requires the field).
pub fn normalize_definition(value: &mut serde_json::Value) {
    normalize_scope(value);
}

fn normalize_scope(value: &mut serde_json::Value) {
    let Value::Object(map) = value else {
        return;
    };
    // `for.in` accepts an inline collection (array/object) in addition to a
    // runtime expression. The SDK model types it as a string, so encode inline
    // collections as JSON expressions (which the jq engine evaluates to the
    // same value).
    if let Some(Value::Object(loop_def)) = map.get_mut("for") {
        if let Some(in_) = loop_def.get_mut("in") {
            if !in_.is_string() {
                let encoded = serde_json::to_string(in_).unwrap_or_default();
                *in_ = Value::String(encoded);
            }
        }
    }
    // Normalize `do` lists (and nested scope lists).
    if let Some(Value::Array(tasks)) = map.get_mut("do") {
        for task in tasks.iter_mut() {
            normalize_task(task);
        }
    }
    if let Some(Value::Array(tasks)) = map.get_mut("try") {
        for task in tasks.iter_mut() {
            normalize_task(task);
        }
    }
    if let Some(Value::Object(catch)) = map.get_mut("catch") {
        if let Some(Value::Array(tasks)) = catch.get_mut("do") {
            for task in tasks.iter_mut() {
                normalize_task(task);
            }
        }
    }
    if let Some(Value::Object(fork)) = map.get_mut("fork") {
        if !fork.contains_key("compete") {
            fork.insert("compete".to_string(), serde_json::Value::Bool(false));
        }
        if let Some(Value::Array(branches)) = fork.get_mut("branches") {
            for task in branches.iter_mut() {
                normalize_task(task);
            }
        }
    }
}

fn normalize_task(value: &mut serde_json::Value) {
    let Value::Object(map) = value else {
        return;
    };
    for (_name, task) in map.iter_mut() {
        normalize_scope(task);
    }
}

/// Parses a workflow definition from raw bytes, attempting YAML then JSON.
pub fn from_bytes(bytes: &[u8]) -> Result<WorkflowDefinition, DefinitionError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| DefinitionError::Parse(format!("invalid UTF-8: {e}")))?;
    from_yaml(text)
}

/// Parses a workflow definition from a file, based on its extension.
///
/// `.json` files are parsed as JSON; everything else is treated as YAML (which
/// is a superset of JSON).
pub fn from_file(path: impl AsRef<Path>) -> Result<WorkflowDefinition, DefinitionError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| DefinitionError::Parse(format!("invalid UTF-8: {e}")))?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => from_json(text),
        _ => from_yaml(text),
    }
}

/// Serializes a workflow definition to YAML.
pub fn to_yaml(def: &WorkflowDefinition) -> Result<String, DefinitionError> {
    serde_yaml::to_string(def).map_err(|e| DefinitionError::Parse(e.to_string()))
}

/// Serializes a workflow definition to JSON.
pub fn to_json(def: &WorkflowDefinition) -> Result<String, DefinitionError> {
    serde_json::to_string_pretty(def).map_err(|e| DefinitionError::Parse(e.to_string()))
}

/// The identity of a workflow definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WorkflowId {
    /// The workflow namespace.
    pub namespace: String,
    /// The workflow name.
    pub name: String,
    /// The workflow version.
    pub version: String,
}

impl WorkflowId {
    /// Builds the qualified name `{name}-{...}.{namespace}` style identity.
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.name, self.namespace)
    }

    /// Returns the workflow's unique identity used for scheduling and invocation.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.namespace, self.name, self.version)
    }
}

/// Gets the identity of a parsed workflow definition.
pub fn workflow_id(def: &WorkflowDefinition) -> WorkflowId {
    WorkflowId {
        namespace: def.document.namespace.clone(),
        name: def.document.name.clone(),
        version: def.document.version.clone(),
    }
}

/// Re-exports the official SDK models for convenience.
pub mod models {
    pub use serverless_workflow_core::models::*;
}

mod validate;

pub use validate::{validate, ValidationIssue, ValidationReport};

#[cfg(test)]
mod tests {
    use super::*;

    const WF: &str = r#"
document: { dsl: '1.0.3', namespace: ns, name: wf, version: '0.1.0' }
do:
  - a: { set: { x: 1 } }
"#;

    #[test]
    fn parse_yaml_and_json_and_identity() {
        let def = from_yaml(WF).unwrap();
        assert_eq!(workflow_id(&def).qualified_name(), "wf.ns");
        assert_eq!(workflow_id(&def).key(), "ns/wf/0.1.0");
        let json = to_json(&def).unwrap();
        let from_json = from_json(&json).unwrap();
        assert_eq!(from_json.document.name, "wf");
    }

    #[test]
    fn to_yaml_roundtrips() {
        let def = from_yaml(WF).unwrap();
        let yaml = to_yaml(&def).unwrap();
        let again = from_yaml(&yaml).unwrap();
        assert_eq!(again.do_.entries.len(), 1);
    }

    #[test]
    fn parse_errors_are_classified() {
        assert!(from_yaml("do: [").unwrap_err().is_parse());
        assert!(from_json("{").unwrap_err().is_parse());
    }

    #[test]
    fn normalize_injects_compete() {
        let mut v: serde_json::Value = serde_yaml::from_str(
            r#"
document: { dsl: '1.0.3', namespace: t, name: w, version: '0.1.0' }
do:
  - f:
      fork:
        branches:
          - a: { set: { x: 1 } }
"#,
        )
        .unwrap();
        normalize_definition(&mut v);
        let fork = &v["do"][0]["f"]["fork"];
        assert_eq!(fork["compete"], serde_json::Value::Bool(false));
    }

    #[test]
    fn from_bytes_parses() {
        let def = from_bytes(WF.as_bytes()).unwrap();
        assert_eq!(def.document.name, "wf");
    }
}
