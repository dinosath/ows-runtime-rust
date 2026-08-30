//! The concrete sandboxed expression engine (`jq` subset) implementing the
//! [`ows_runtime_core::ExpressionEngine`] trait.

use ows_runtime_core::{CompiledExpression, ExpressionValue};
use serde_json::{Map, Value};

use crate::ast::Expr;
use crate::error::ExpressionError;
use crate::eval::{eval, Env};
use crate::parser::parse;

/// A compiled piece of an interpolation template.
#[derive(Debug, Clone)]
enum Piece {
    /// A literal string segment.
    Lit(String),
    /// A `${ ... }` expression block.
    Expr(Expr),
}

/// Prepared compilation of a source string.
#[derive(Debug, Clone)]
enum Prepared {
    /// A single expression (bare or `${ ... }` wrapped).
    Expr(Expr),
    /// A plain literal string with no expression blocks.
    Plain(String),
    /// A string with one or more `${ ... }` blocks.
    Interp(Vec<Piece>),
}

/// The sandboxed `jq` expression engine.
#[derive(Debug, Clone, Default)]
pub struct JqEngine;

impl JqEngine {
    /// Creates a new `jq` engine.
    pub fn new() -> Self {
        Self
    }
}

fn prepare(source: &str) -> Result<Prepared, ExpressionError> {
    let trimmed = source.trim();
    if let Some(inner) = single_block(trimmed) {
        let expr = parse(&inner)?;
        return Ok(Prepared::Expr(expr));
    }
    if trimmed.contains("${") {
        let pieces = parse_blocks(trimmed)?;
        return Ok(Prepared::Interp(pieces));
    }
    Ok(Prepared::Plain(source.to_string()))
}

/// If the source is a single balanced `${ ... }` block, returns the inner text.
///
/// Brace depth accounts for both nested `${ ... }` expressions and object
/// literal braces `{ ... }` inside the expression.
fn single_block(source: &str) -> Option<String> {
    let chars: Vec<char> = source.chars().collect();
    let start = first_non_ws(&chars)?;
    if chars.get(start) != Some(&'$') || chars.get(start + 1) != Some(&'{') {
        return None;
    }
    let open = start + 1;
    let close = find_block_close(&chars, open)?;
    if !chars[close + 1..].iter().all(|c| c.is_whitespace()) {
        return None;
    }
    let inner: String = chars[open + 1..close].iter().collect();
    Some(inner.trim().to_string())
}

fn first_non_ws(chars: &[char]) -> Option<usize> {
    chars.iter().position(|c| !c.is_whitespace())
}

/// Finds the index of the `}` that closes the block opened at `open` (which is
/// the index of the `{`). Returns `None` if unbalanced.
fn find_block_close(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            '"' => {
                // Skip string literals so braces inside strings don't count.
                let mut j = i;
                while j < chars.len() {
                    match chars[j] {
                        '\\' => j += 2,
                        '"' => break,
                        _ => j += 1,
                    }
                }
                if j >= chars.len() {
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a string into literal and `${ ... }` expression pieces.
fn parse_blocks(source: &str) -> Result<Vec<Piece>, ExpressionError> {
    let chars: Vec<char> = source.chars().collect();
    let mut pieces = Vec::new();
    let mut i = 0;
    let mut literal = String::new();
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'{') {
            if !literal.is_empty() {
                pieces.push(Piece::Lit(std::mem::take(&mut literal)));
            }
            let open = i + 1;
            let close = find_block_close(&chars, open)
                .ok_or_else(|| ExpressionError::parse(i, "unterminated `${` block"))?;
            let inner: String = chars[open + 1..close].iter().collect();
            let expr = parse(inner.trim())?;
            pieces.push(Piece::Expr(expr));
            i = close + 1;
        } else {
            literal.push(chars[i]);
            i += 1;
        }
    }
    if !literal.is_empty() {
        pieces.push(Piece::Lit(literal));
    }
    if pieces.is_empty() {
        pieces.push(Piece::Lit(String::new()));
    }
    Ok(pieces)
}

fn evaluate_value(
    compiled: &CompiledExpression,
    input: &Value,
    vars: &Map<String, Value>,
) -> Result<Value, ExpressionError> {
    let prepared = compiled
        .data
        .downcast_ref::<Prepared>()
        .ok_or_else(|| ExpressionError::eval("compiled expression has invalid data"))?;
    let env = Env::new(vars.clone());
    match prepared {
        Prepared::Expr(expr) => {
            let stream = eval(expr, input, &env)?;
            Ok(stream.into_iter().last().unwrap_or(Value::Null))
        }
        Prepared::Plain(s) => {
            // In evaluate context a plain string is treated as a bare expression.
            let expr = parse(s)?;
            let stream = eval(&expr, input, &env)?;
            Ok(stream.into_iter().last().unwrap_or(Value::Null))
        }
        Prepared::Interp(pieces) => interpolate(pieces, input, &env),
    }
}

fn evaluate_interp(
    compiled: &CompiledExpression,
    input: &Value,
    vars: &Map<String, Value>,
) -> Result<Value, ExpressionError> {
    let prepared = compiled
        .data
        .downcast_ref::<Prepared>()
        .ok_or_else(|| ExpressionError::eval("compiled expression has invalid data"))?;
    let env = Env::new(vars.clone());
    match prepared {
        Prepared::Expr(expr) => {
            let stream = eval(expr, input, &env)?;
            Ok(stream.into_iter().last().unwrap_or(Value::Null))
        }
        Prepared::Plain(s) => Ok(Value::String(s.clone())),
        Prepared::Interp(pieces) => interpolate(pieces, input, &env),
    }
}

fn interpolate(pieces: &[Piece], input: &Value, env: &Env) -> Result<Value, ExpressionError> {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Lit(s) => out.push_str(s),
            Piece::Expr(e) => {
                let stream = eval(e, input, env)?;
                let v = stream.into_iter().last().unwrap_or(Value::Null);
                out.push_str(&crate::eval::to_string(&v));
            }
        }
    }
    Ok(Value::String(out))
}

impl ows_runtime_core::ExpressionEngine for JqEngine {
    fn compile(
        &self,
        source: &str,
    ) -> Result<CompiledExpression, ows_runtime_core::ExpressionError> {
        let prepared = prepare(source).map_err(ows_runtime_core::ExpressionError::from)?;
        Ok(CompiledExpression::new(source, prepared))
    }

    fn evaluate(
        &self,
        compiled: &CompiledExpression,
        input: &ExpressionValue,
        variables: &Map<String, Value>,
    ) -> Result<ExpressionValue, ows_runtime_core::ExpressionError> {
        evaluate_value(compiled, input, variables).map_err(ows_runtime_core::ExpressionError::from)
    }

    fn is_expression(&self, value: &str) -> bool {
        value.trim_start().starts_with("${")
    }

    fn evaluate_interpolation(
        &self,
        source: &str,
        input: &ExpressionValue,
        variables: &Map<String, Value>,
    ) -> Result<ExpressionValue, ows_runtime_core::ExpressionError> {
        let compiled = self.compile(source)?;
        evaluate_interp(&compiled, input, variables)
            .map_err(ows_runtime_core::ExpressionError::from)
    }
}
