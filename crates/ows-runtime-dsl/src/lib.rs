//! Parsing, serialization and validation of Open Workflow Specification
//! definitions.
//!
//! This crate uses the official [`serverless_workflow_core`] SDK types as the
//! primary representation of OWS workflow definitions. It provides parsing from
//! YAML and JSON, serialization, and a validation pass that distinguishes
//! parse, schema and semantic errors per the OWS compilation pipeline.
#![allow(clippy::result_large_err)]

use std::path::Path;

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
    serde_yaml::from_str(yaml).map_err(|e| DefinitionError::Parse(e.to_string()))
}

/// Parses a workflow definition from a JSON string.
pub fn from_json(json: &str) -> Result<WorkflowDefinition, DefinitionError> {
    serde_json::from_str(json).map_err(|e| DefinitionError::Parse(e.to_string()))
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
