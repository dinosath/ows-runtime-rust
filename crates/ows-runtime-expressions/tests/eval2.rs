//! Additional evaluation tests covering edge cases and uncovered branches.

use ows_runtime_expressions::{eval_single, parse, Env};
use serde_json::{json, Value};

fn evalv(expr: &str, input: Value) -> Value {
    let e = parse(expr).unwrap_or_else(|e| panic!("parse `{expr}`: {e}"));
    eval_single(&e, &input, &Env::new(Default::default()))
        .unwrap_or_else(|e| panic!("eval `{expr}`: {e}"))
}

#[test]
fn mixed_type_addition() {
    assert_eq!(evalv("1 + \"a\"", json!(null)), json!("1a"));
    assert_eq!(evalv("[1] + \"a\"", json!(null)), json!("[1]a"));
}

#[test]
fn array_iteration_and_field() {
    assert_eq!(evalv("[.name]", json!({"a":1})), json!([json!(null)]));
    assert_eq!(evalv(".items[0]", json!({"items":[5,6]})), json!(5));
    assert_eq!(
        evalv(".name", json!([{"name":"a"},{"name":"b"}])),
        json!(["a", "b"])
    );
}

#[test]
fn object_computed_keys_and_values() {
    let e = parse("{($k): $v}").unwrap();
    let mut vars = serde_json::Map::new();
    vars.insert("k".to_string(), json!("key"));
    vars.insert("v".to_string(), json!(7));
    let out = eval_single(&e, &json!(null), &Env::new(vars)).unwrap();
    assert_eq!(out, json!({"key":7}));
}

#[test]
fn tostring_of_complex_values() {
    assert_eq!(evalv("tostring", json!({"a":1})), json!("{\"a\":1}"));
    assert_eq!(evalv("tostring", json!([1, 2])), json!("[1,2]"));
    assert_eq!(evalv("tostring", json!(null)), json!("null"));
}

#[test]
fn empty_function_produces_no_output() {
    // empty in a stream; eval_single takes last which is null when empty.
    let e = parse("empty").unwrap();
    assert_eq!(
        eval_single(&e, &json!(null), &Env::new(Default::default())).unwrap(),
        Value::Null
    );
}

#[test]
fn range_single_arg() {
    assert_eq!(evalv("[range(3)]", json!(null)), json!([0, 1, 2]));
}

#[test]
fn first_last_on_scalar() {
    assert_eq!(evalv("first", json!(42)), json!(42));
    assert_eq!(evalv("last", json!(42)), json!(42));
}

#[test]
fn object_index_with_string() {
    assert_eq!(evalv(".[\"a\"]", json!({"a": 9})), json!(9));
}

#[test]
fn error_function_fails() {
    let e = parse("error(\"custom boom\")").unwrap();
    assert!(eval_single(&e, &json!(null), &Env::new(Default::default())).is_err());
}

#[test]
fn if_without_else() {
    assert_eq!(evalv("if true then 1 end", json!(null)), json!(1));
    assert_eq!(evalv("if false then 1 end", json!(null)), json!(null));
}

#[test]
fn string_index() {
    assert_eq!(evalv(".[1]", json!("abc")), json!("b"));
    assert_eq!(evalv(".[-1]", json!("abc")), json!("c"));
}

#[test]
fn add_null_identity() {
    assert_eq!(evalv("null + \"x\"", json!(null)), json!("x"));
    assert_eq!(evalv("\"x\" + null", json!(null)), json!("x"));
    assert_eq!(evalv("null + 5", json!(null)), json!(5));
    assert_eq!(evalv("null + [1]", json!(null)), json!([1]));
    assert_eq!(evalv("null + {a:1}", json!(null)), json!({"a":1}));
}

#[test]
fn div_by_string_split() {
    assert_eq!(
        evalv("\"a/b/c\" / \"/\"", json!(null)),
        json!(["a", "b", "c"])
    );
}

#[test]
fn nested_object_variable_projection() {
    let out = evalv(
        "{ total: (.items | map(.n) | add) }",
        json!({"items":[{"n":1},{"n":2}]}),
    );
    assert_eq!(out, json!({"total": 3}));
}
