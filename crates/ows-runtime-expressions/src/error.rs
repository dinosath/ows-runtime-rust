//! Error types for the expression engine.

use thiserror::Error;

/// An error produced while lexing, parsing or evaluating an expression.
#[derive(Debug, Clone, Error)]
pub enum ExpressionError {
    /// The expression could not be tokenized.
    #[error("lex error at byte {pos}: {message}")]
    Lex { pos: usize, message: String },
    /// The expression could not be parsed.
    #[error("parse error at byte {pos}: {message}")]
    Parse { pos: usize, message: String },
    /// Evaluation failed at runtime.
    #[error("eval error: {message}")]
    Eval { message: String },
    /// A type mismatch or unsupported operation.
    #[error("type error: {message}")]
    TypeError { message: String },
    /// The requested function is not supported by this sandboxed engine.
    #[error("unsupported function `{name}`")]
    UnsupportedFunction { name: String },
}

impl ExpressionError {
    /// Creates a parse error at a position.
    pub fn parse(pos: usize, message: impl Into<String>) -> Self {
        Self::Parse {
            pos,
            message: message.into(),
        }
    }

    /// Creates an evaluation error.
    pub fn eval(message: impl Into<String>) -> Self {
        Self::Eval {
            message: message.into(),
        }
    }

    /// Creates a type error.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self::TypeError {
            message: message.into(),
        }
    }
}

impl From<ExpressionError> for ows_runtime_core::ExpressionError {
    fn from(e: ExpressionError) -> Self {
        ows_runtime_core::ExpressionError {
            message: e.to_string(),
            expression: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors() {
        let p = ExpressionError::parse(3, "bad");
        assert!(matches!(p, ExpressionError::Parse { pos: 3, .. }));
        let e = ExpressionError::eval("boom");
        assert!(matches!(e, ExpressionError::Eval { .. }));
        let t = ExpressionError::type_error("type");
        assert!(matches!(t, ExpressionError::TypeError { .. }));
        let u = ExpressionError::UnsupportedFunction { name: "f".into() };
        assert!(matches!(u, ExpressionError::UnsupportedFunction { .. }));
    }

    #[test]
    fn display_and_conversion() {
        let e = ExpressionError::parse(0, "bad");
        assert!(e.to_string().contains("parse error"));
        let core: ows_runtime_core::ExpressionError = e.into();
        assert!(!core.message.is_empty());
    }
}
