//! # ows-runtime-expressions
//!
//! A fully sandboxed runtime expression engine implementing the `jq` subset
//! required by the Open Workflow Specification. Expressions are parsed and
//! interpreted; they are never evaluated through unsafe evaluation, `eval`,
//! shell execution or arbitrary code execution.
//!
//! The engine implements [`ows_runtime_core::ExpressionEngine`] and supports:
//! literals, variables, field access, array/object construction, pipes, binary
//! operators, comparisons, string interpolation, `if/then/else`, and a set of
//! `jq` functions (`map`, `select`, `join`, `length`, ...).
#![allow(clippy::result_large_err)]

pub mod ast;
pub mod engine;
pub mod error;
pub mod eval;
pub mod lexer;
pub mod parser;

pub use engine::JqEngine;
pub use error::ExpressionError;
pub use eval::{eval, eval_single, Env};
pub use parser::parse;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval_str(
        expr: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ExpressionError> {
        let parsed = parse(expr)?;
        eval_single(&parsed, &input, &Env::new(Default::default()))
    }

    #[test]
    fn identity() {
        assert_eq!(eval_str(".", json!({"a": 1})).unwrap(), json!({"a": 1}));
    }

    #[test]
    fn field_access() {
        let input = json!({"user": {"name": "alice"}, "tags": ["x", "y"]});
        assert_eq!(
            eval_str(".user.name", input.clone()).unwrap(),
            json!("alice")
        );
        assert_eq!(eval_str(".tags[1]", input.clone()).unwrap(), json!("y"));
        assert_eq!(eval_str(".missing", input).unwrap(), json!(null));
    }

    #[test]
    fn array_and_object_construction() {
        let input = json!({"colors": ["red"]});
        assert_eq!(
            eval_str(".colors + [\"green\"]", input).unwrap(),
            json!(["red", "green"])
        );
        assert_eq!(
            eval_str("{ ids: [ 1, 2 ] }", json!(null)).unwrap(),
            json!({"ids": [1, 2]})
        );
    }

    #[test]
    fn comparisons_and_boolean() {
        let input = json!({"color": "red"});
        assert_eq!(
            eval_str(".color == \"red\"", input.clone()).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_str(".color == \"blue\"", input.clone()).unwrap(),
            json!(false)
        );
        assert_eq!(
            eval_str(".color != \"blue\"", input.clone()).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_str(".color == \"red\" and .color != \"blue\"", input).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn pipe_and_functions() {
        let input = json!([{"name": "a"}, {"name": "b"}]);
        assert_eq!(
            eval_str(". | map(.name) | join(\",\")", input).unwrap(),
            json!("a,b")
        );
        assert_eq!(
            eval_str(
                ".[] | select(.name == \"b\")",
                json!([
                    {"name": "a"}, {"name": "b"}
                ])
            )
            .unwrap(),
            json!({"name": "b"})
        );
    }

    #[test]
    fn string_interpolation() {
        let input = json!({"user": {"firstName": "John", "lastName": "Doe"}});
        let expr = r#""Hello \(.user.firstName) \(.user.lastName)!""#;
        assert_eq!(eval_str(expr, input).unwrap(), json!("Hello John Doe!"));
    }

    #[test]
    fn truthiness_follows_jq() {
        // In jq, 0 and "" are truthy.
        assert_eq!(eval_str("0", json!(null)).unwrap(), json!(0));
        assert_eq!(eval_str(". == true", json!(0)).unwrap(), json!(false));
    }

    #[test]
    fn engine_trait_interpolation() {
        use ows_runtime_core::ExpressionEngine;
        let engine = JqEngine::new();
        // Single block returns raw value.
        let input = json!({"a": 1});
        let vars = Default::default();
        let v = engine
            .evaluate_interpolation("${ .a }", &input, &vars)
            .unwrap();
        assert_eq!(v, json!(1));
        // Bare string is literal.
        let v = engine
            .evaluate_interpolation("hello", &input, &vars)
            .unwrap();
        assert_eq!(v, json!("hello"));
        // Embedded block in a string.
        let v = engine
            .evaluate_interpolation("count=${ .a }", &input, &vars)
            .unwrap();
        assert_eq!(v, json!("count=1"));
    }

    #[test]
    fn variables() {
        let input = json!(null);
        let mut vars = serde_json::Map::new();
        vars.insert("pet".to_string(), json!({"id": 7}));
        let parsed = parse("$pet.id").unwrap();
        let v = eval_single(&parsed, &input, &Env::new(vars)).unwrap();
        assert_eq!(v, json!(7));
    }

    #[test]
    fn select_with_variable() {
        let input = json!({"pets": [{"id": 1}, {"id": 2}]});
        let mut vars = serde_json::Map::new();
        vars.insert("x".to_string(), json!(2));
        let parsed = parse(".pets | map(select(.id == $x))").unwrap();
        let v = eval_single(&parsed, &input, &Env::new(vars)).unwrap();
        assert_eq!(v, json!([{"id": 2}]));
    }

    #[test]
    fn context_plus_current() {
        let input = json!({"foo": "bar"});
        let mut vars = serde_json::Map::new();
        vars.insert("context".to_string(), json!({"base": 1}));
        let parsed = parse("$context + .").unwrap();
        let v = eval_single(&parsed, &input, &Env::new(vars)).unwrap();
        assert_eq!(v, json!({"base": 1, "foo": "bar"}));
    }

    #[test]
    fn if_then_else() {
        let input = json!({"n": 5});
        assert_eq!(
            eval_str("if .n > 3 then \"big\" else \"small\" end", input).unwrap(),
            json!("big")
        );
    }

    #[test]
    fn nested_object_with_variables() {
        let input = json!({"processed": {"colors": []}});
        let mut vars = serde_json::Map::new();
        vars.insert("color".to_string(), json!("red"));
        vars.insert("index".to_string(), json!(0));
        let parsed =
            parse("{ colors: (.processed.colors + [ $color ]), indexes: [ $index ] }").unwrap();
        let v = eval_single(&parsed, &input, &Env::new(vars)).unwrap();
        assert_eq!(v, json!({"colors": ["red"], "indexes": [0]}));
    }

    #[test]
    fn missing_field_is_null() {
        let input = json!({"a": 1});
        assert_eq!(eval_str(".z.z", input).unwrap(), json!(null));
    }
}
