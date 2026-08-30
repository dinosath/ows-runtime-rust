//! Error conversions for the OWS runtime crate.

use ows_runtime_core::{ErrorKind, ProblemDetails, StandardErrorType, WorkflowError};

/// Converts a DSL definition error into a typed [`WorkflowError`], preserving
/// the parse / schema / semantic distinction.
pub fn from_definition_error(e: &ows_runtime_dsl::DefinitionError) -> WorkflowError {
    let (kind, detail) = match e {
        ows_runtime_dsl::DefinitionError::Parse(m) => (ErrorKind::Parse, m.clone()),
        ows_runtime_dsl::DefinitionError::Schema(m) => (ErrorKind::Schema, m.clone()),
        ows_runtime_dsl::DefinitionError::Semantic(m) => (ErrorKind::Semantic, m.clone()),
        ows_runtime_dsl::DefinitionError::Io(m) => (ErrorKind::Runtime, m.to_string()),
    };
    WorkflowError::new(
        kind,
        ProblemDetails::standard(StandardErrorType::Runtime).with_detail(detail),
    )
}

/// Creates a typed runtime error with the given detail.
pub fn runtime_error(detail: impl Into<String>) -> WorkflowError {
    WorkflowError::new(
        ErrorKind::Runtime,
        ProblemDetails::standard(StandardErrorType::Runtime).with_detail(detail),
    )
}

/// Creates a semantic error with the given detail.
pub fn semantic_error(detail: impl Into<String>) -> WorkflowError {
    WorkflowError::new(
        ErrorKind::Semantic,
        ProblemDetails::standard(StandardErrorType::Runtime).with_detail(detail),
    )
}

/// Creates an expression error with the given detail.
pub fn expression_error(detail: impl Into<String>) -> WorkflowError {
    WorkflowError::new(
        ErrorKind::Expression,
        ProblemDetails::standard(StandardErrorType::Expression).with_detail(detail),
    )
}

/// Creates a validation error with the given detail.
pub fn validation_error(detail: impl Into<String>) -> WorkflowError {
    WorkflowError::new(
        ErrorKind::Schema,
        ProblemDetails::standard(StandardErrorType::Validation).with_detail(detail),
    )
}

/// Creates a communication error with the given detail and optional status.
pub fn communication_error(status: u16, detail: impl Into<String>) -> WorkflowError {
    let mut problem = ProblemDetails::standard(StandardErrorType::Communication);
    problem.status = status;
    problem.detail = Some(detail.into());
    WorkflowError::new(ErrorKind::Communication, problem)
}

/// Creates a timeout error.
pub fn timeout_error() -> WorkflowError {
    WorkflowError::timeout(None)
}

/// Creates a policy error with the given detail.
pub fn policy_error(detail: impl Into<String>) -> WorkflowError {
    WorkflowError::new(
        ErrorKind::Policy,
        ProblemDetails::standard(StandardErrorType::Runtime).with_detail(detail),
    )
}
