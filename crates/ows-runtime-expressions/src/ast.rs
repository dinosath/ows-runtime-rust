//! Abstract syntax tree for the expression language.

use serde_json::Value;

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Alternative,
}

impl BinOp {
    /// A short display form for error messages.
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::Alternative => "//",
        }
    }
}

/// An object construction key.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjKey {
    /// A literal identifier key, e.g. `{ foo: ... }`.
    Ident(String),
    /// A string key, e.g. `{ "foo bar": ... }`.
    Str(String),
    /// A computed key, e.g. `{ ($k): ... }`.
    Computed(Box<Expr>),
}

/// A single part of a string with interpolation.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    /// A literal string segment.
    Literal(String),
    /// An interpolated expression `\( ... )`.
    Expr(Box<Expr>),
}

/// An expression AST node.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `.` identity.
    Identity,
    /// `.foo` field selection applied to the current input.
    Field(Box<Expr>, String),
    /// `.[expr]` / `.foo[expr]` index/access applied to the current input.
    Index(Box<Expr>, Box<Expr>),
    /// `.[]` / `.foo[]` iteration applied to the current input.
    Iterate(Box<Expr>),
    /// `$name` variable reference.
    Variable(String),
    /// A literal JSON value.
    Literal(Value),
    /// A string literal, possibly with interpolation.
    Str(Vec<InterpPart>),
    /// A binary operation.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `not expr` / `-expr`.
    Unary(UnaryOp, Box<Expr>),
    /// `a | b`.
    Pipe(Box<Expr>, Box<Expr>),
    /// `[ a, b, c ]` array constructor.
    Array(Vec<Expr>),
    /// `{ k: v, ... }` object constructor.
    Object(Vec<(ObjKey, Expr)>),
    /// `if cond then t else f end`.
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `name(args...)` function call.
    Call(String, Vec<Expr>),
    /// `(a, b)` stream (comma expression).
    Stream(Vec<Expr>),
}

/// A unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Negate,
}
