//! More evaluation tests targeting error paths and remaining branches.

use ows_runtime_expressions::{eval_single, parse, Env};
use serde_json::{json, Value};

fn evalv(expr: &str, input: Value) -> Value {
    let e = parse(expr).unwrap();
    eval_single(&e, &input, &Env::new(Default::default())).unwrap()
}

fn evalf(expr: &str, input: Value) -> Result<Value, ows_runtime_expressions::ExpressionError> {
    let e = parse(expr).unwrap();
    eval_single(&e, &input, &Env::new(Default::default()))
}

#[test]
fn join_empty_and_select_no_match() {
    assert_eq!(evalv("join(\",\")", json!([])), json!(""));
    assert_eq!(evalv("select(. > 10)", json!(5)), json!(null));
}

#[test]
fn error_function_without_args() {
    assert!(evalf("error", json!("oops")).is_err());
}

#[test]
fn contains_array_and_object() {
    assert_eq!(evalv("contains([2])", json!([1, 2, 3])), json!(true));
    assert_eq!(evalv("contains([9])", json!([1, 2])), json!(false));
    assert_eq!(evalv("contains({a:1})", json!({"a":1,"b":2})), json!(true));
}

#[test]
fn tonumber_parse_error() {
    assert!(evalf("tonumber", json!("abc")).is_err());
}

#[test]
fn range_negative() {
    assert_eq!(evalv("[range(-2)]", json!(null)), json!([]));
}

#[test]
fn to_string_primitives() {
    assert_eq!(evalv("tostring", json!(true)), json!("true"));
    assert_eq!(evalv("tostring", json!(false)), json!("false"));
}

#[test]
fn map_and_join_pipeline() {
    assert_eq!(
        evalv(
            ".pets | map(.name) | join(\", \")",
            json!({"pets":[{"name":"a"},{"name":"b"}]})
        ),
        json!("a, b")
    );
}

#[test]
fn nested_arithmetic_and_parentheses() {
    assert_eq!(evalv("(1 + 2) * (3 + 4)", json!(null)), json!(21));
    assert_eq!(evalv("2 * 3 + 4", json!(null)), json!(10));
}

#[test]
fn values_empty_stream() {
    assert_eq!(evalv("[.[] | empty] | length", json!([1, 2, 3])), json!(0));
}

#[test]
fn alternative_and_type_mix() {
    assert_eq!(
        evalv(".missing // \"fallback\"", json!({"a":1})),
        json!("fallback")
    );
}

#[test]
fn unary_negation_negative_number() {
    assert_eq!(evalv("-(-5)", json!(null)), json!(5));
}
