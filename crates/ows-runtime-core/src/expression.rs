//! Runtime expression abstractions.
//!
//! The concrete evaluation engine (a sandboxed `jq` interpreter) lives in the
//! `ows-runtime-expressions` crate. This module only defines the interfaces the
//! execution engine depends on, keeping the core free of a specific expression
//! language.

use std::collections::HashMap;
use std::fmt;

use serde_json::{Map, Value};

/// A value produced or consumed by an expression.
pub type ExpressionValue = Value;

/// Arguments available during expression evaluation, per the OWS DSL.
///
/// Not every argument is available for every runtime expression. See the
/// "Runtime expression arguments" table in the specification.
#[derive(Debug, Clone)]
pub struct ExpressionContext {
    /// The workflow's context data (`$context`).
    pub context: ExpressionValue,
    /// The task's transformed input (`$input`).
    pub input: ExpressionValue,
    /// The task's transformed output (`$output`).
    pub output: ExpressionValue,
    /// A key/value map of workflow secrets (`$secrets`).
    pub secrets: HashMap<String, String>,
    /// The resolved authorization (`$authorization`), if any.
    pub authorization: Option<Value>,
    /// The current task descriptor (`$task`), if any.
    pub task: Option<Value>,
    /// The current workflow descriptor (`$workflow`), if any.
    pub workflow: Option<Value>,
    /// The runtime descriptor (`$runtime`), if any.
    pub runtime: Option<Value>,
    /// Additional variables (e.g. loop variables `$each`, `$index`) exposed to
    /// expressions alongside the standard arguments.
    pub extra_vars: Map<String, Value>,
}

impl Default for ExpressionContext {
    fn default() -> Self {
        Self {
            context: Value::Null,
            input: Value::Null,
            output: Value::Null,
            secrets: HashMap::new(),
            authorization: None,
            task: None,
            workflow: None,
            runtime: None,
            extra_vars: Map::new(),
        }
    }
}

impl ExpressionContext {
    /// Builds the map of `$`-prefixed arguments exposed to the expression.
    pub fn as_variable_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("context".to_string(), self.context.clone());
        map.insert("input".to_string(), self.input.clone());
        map.insert("output".to_string(), self.output.clone());
        let secrets = self
            .secrets
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect::<Map<_, _>>();
        map.insert("secrets".to_string(), Value::Object(secrets));
        if let Some(auth) = &self.authorization {
            map.insert("authorization".to_string(), auth.clone());
        }
        if let Some(task) = &self.task {
            map.insert("task".to_string(), task.clone());
        }
        if let Some(workflow) = &self.workflow {
            map.insert("workflow".to_string(), workflow.clone());
        }
        if let Some(runtime) = &self.runtime {
            map.insert("runtime".to_string(), runtime.clone());
        }
        for (k, v) in &self.extra_vars {
            map.insert(k.clone(), v.clone());
        }
        map
    }

    /// Builds a variable map with the given input/output overrides.
    ///
    /// This is a convenience for the data-flow stages where the input/output
    /// arguments change between steps.
    pub fn as_variable_map_updated(&self, input: &Value, output: &Value) -> Map<String, Value> {
        let mut updated = self.clone();
        updated.input = input.clone();
        updated.output = output.clone();
        updated.as_variable_map()
    }
}

/// An error raised during expression evaluation.
#[derive(Debug, Clone)]
pub struct ExpressionError {
    /// A human readable message.
    pub message: String,
    /// The expression that failed, if known.
    pub expression: Option<String>,
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(expr) = &self.expression {
            write!(f, "expression error in `{expr}`: {}", self.message)
        } else {
            write!(f, "expression error: {}", self.message)
        }
    }
}

impl std::error::Error for ExpressionError {}

/// A runtime expression engine.
///
/// Implementations must be sandboxed: expressions must never be evaluated using
/// unsafe evaluation, shell execution or arbitrary code execution.
#[async_trait::async_trait]
pub trait ExpressionEngine: Send + Sync {
    /// Parses and compiles the given expression, returning an opaque handle.
    fn compile(&self, source: &str) -> Result<CompiledExpression, ExpressionError>;

    /// Evaluates a compiled expression against an input value and argument map.
    fn evaluate(
        &self,
        compiled: &CompiledExpression,
        input: &ExpressionValue,
        variables: &Map<String, Value>,
    ) -> Result<ExpressionValue, ExpressionError>;

    /// Whether the given string contains an inline expression.
    fn is_expression(&self, value: &str) -> bool;

    /// Extracts the inner expression(s) and evaluates interpolation on a string.
    ///
    /// In strict mode, `${ ... }` blocks are evaluated and substituted. In
    /// loose mode, a bare string that parses as a single expression is evaluated
    /// and otherwise returned verbatim.
    fn evaluate_interpolation(
        &self,
        source: &str,
        input: &ExpressionValue,
        variables: &Map<String, Value>,
    ) -> Result<ExpressionValue, ExpressionError>;
}

/// A compiled expression handle returned by [`ExpressionEngine::compile`].
#[derive(Clone)]
pub struct CompiledExpression {
    /// The original source text.
    pub source: String,
    /// Opaque, engine-specific compiled data.
    pub data: Arc<dyn std::any::Any + Send + Sync>,
}

impl CompiledExpression {
    /// Creates a new compiled expression.
    pub fn new<D>(source: impl Into<String>, data: D) -> Self
    where
        D: Send + Sync + 'static,
    {
        Self {
            source: source.into(),
            data: Arc::new(data),
        }
    }
}

use std::sync::Arc;
