//! Error-path and edge-case evaluation tests.

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
fn undefined_variable_errors() {
    assert!(evalf("$nope", json!(null)).is_err());
}

#[test]
fn iterate_over_object() {
    assert_eq!(evalv(".[] | . * 2", json!({"a": 3})), json!(6));
}

#[test]
fn negate_non_number_errors() {
    assert!(evalf("- \"x\"", json!(null)).is_err());
}

#[test]
fn stream_expressions() {
    assert_eq!(evalv("(1, 2) | . * 10", json!(null)), json!(20));
}

#[test]
fn and_short_circuits() {
    // `false and (1/0)` short-circuits without evaluating the right side.
    assert_eq!(evalv("false and (1/0)", json!(null)), json!(false));
    assert_eq!(evalv("true and false", json!(null)), json!(false));
}

#[test]
fn binary_operator_type_errors() {
    assert!(evalf("5 - \"a\"", json!(null)).is_err());
    assert!(evalf("5 / \"a\"", json!(null)).is_err());
    assert!(evalf("5 % \"a\"", json!(null)).is_err());
    assert!(evalf("5 < \"a\"", json!(null)).is_err());
}

#[test]
fn add_mixed_branches() {
    assert_eq!(evalv("1 + \"a\"", json!(null)), json!("1a"));
    assert_eq!(evalv("\"a\" + 1", json!(null)), json!("a1"));
    assert_eq!(evalv("[1] + \"a\"", json!(null)), json!("[1]a"));
    assert_eq!(evalv("\"a\" + [1]", json!(null)), json!("a[1]"));
}

#[test]
fn comparison_ge_and_array_cmp() {
    assert_eq!(evalv("1 >= 1", json!(null)), json!(true));
    assert_eq!(evalv("[1,2] >= [1,2]", json!(null)), json!(true));
}

#[test]
fn index_access_errors() {
    // Array string index returns null; non-number index errors.
    assert_eq!(evalv(".[\"a\"]", json!([1, 2])), json!(null));
    assert!(evalf(".[true]", json!([1])).is_err());
    assert!(evalf(".[\"x\"]", json!("abc")).is_err());
    // Object numeric index returns null.
    assert_eq!(evalv(".[1]", json!({"a": 1})), json!(null));
    // Out-of-range string index.
    assert_eq!(evalv(".[9]", json!("abc")), json!(null));
}

#[test]
fn object_with_empty_value() {
    assert_eq!(
        evalv("{a: empty, b: 1}", json!(null)),
        json!({"a": null, "b": 1})
    );
}

#[test]
fn mul_string_by_number() {
    assert_eq!(evalv("\"ab\" * 2", json!(null)), json!("abab"));
    assert_eq!(evalv("2 * \"ab\"", json!(null)), json!("abab"));
}

#[test]
fn field_access_on_array() {
    assert_eq!(evalv(".name", json!([{"name":"a"}])), json!(["a"]));
}
