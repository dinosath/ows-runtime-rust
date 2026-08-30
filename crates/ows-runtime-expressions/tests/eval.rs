//! Comprehensive evaluation tests covering all operators and functions of the
//! sandboxed expression engine.

use ows_runtime_expressions::{eval_single, parse, Env};
use serde_json::{json, Value};

fn evalv(expr: &str, input: Value) -> Value {
    let e = parse(expr).unwrap_or_else(|e| panic!("parse `{expr}`: {e}"));
    eval_single(&e, &input, &Env::new(Default::default()))
        .unwrap_or_else(|e| panic!("eval `{expr}`: {e}"))
}

fn eval_vars(expr: &str, input: Value, vars: serde_json::Map<String, Value>) -> Value {
    let e = parse(expr).unwrap();
    eval_single(&e, &input, &Env::new(vars)).unwrap()
}

// ---- literals and arithmetic ----

#[test]
fn literals() {
    assert_eq!(evalv("1", json!(null)), json!(1));
    assert_eq!(evalv("1.5", json!(null)), json!(1.5));
    assert_eq!(evalv("\"hi\"", json!(null)), json!("hi"));
    assert_eq!(evalv("true", json!(null)), json!(true));
    assert_eq!(evalv("null", json!(null)), json!(null));
    assert_eq!(evalv("[1,2]", json!(null)), json!([1, 2]));
}

#[test]
fn arithmetic_integer_preserved() {
    assert_eq!(evalv("1000000 + 1", json!(null)), json!(1000001));
    assert_eq!(evalv("10 - 3", json!(null)), json!(7));
    assert_eq!(evalv("6 * 7", json!(null)), json!(42));
    assert_eq!(evalv("7 / 2", json!(null)), json!(3.5));
    assert_eq!(evalv("10 % 3", json!(null)), json!(1));
    assert_eq!(evalv("-5", json!(null)), json!(-5));
}

#[test]
fn arithmetic_float() {
    assert_eq!(evalv("1.5 + 2.25", json!(null)), json!(3.75));
    assert_eq!(evalv("2.0 * 3.5", json!(null)), json!(7));
}

#[test]
fn string_ops() {
    assert_eq!(evalv("\"foo\" + \"bar\"", json!(null)), json!("foobar"));
    assert_eq!(evalv("\"ab\" * 3", json!(null)), json!("ababab"));
    assert_eq!(
        evalv("\"a,b,c\" / \",\"", json!(null)),
        json!(["a", "b", "c"])
    );
}

#[test]
fn array_and_object_ops() {
    assert_eq!(evalv("[1,2] + [3]", json!(null)), json!([1, 2, 3]));
    assert_eq!(evalv("[1,2,3] - [2]", json!(null)), json!([1, 3]));
    assert_eq!(evalv("{a:1} + {b:2}", json!(null)), json!({"a":1,"b":2}));
    assert_eq!(evalv("null + [1]", json!(null)), json!([1]));
    assert_eq!(evalv("null + \"x\"", json!(null)), json!("x"));
}

// ---- comparisons and boolean ----

#[test]
fn comparisons() {
    assert_eq!(evalv("1 < 2", json!(null)), json!(true));
    assert_eq!(evalv("2 <= 2", json!(null)), json!(true));
    assert_eq!(evalv("3 > 2", json!(null)), json!(true));
    assert_eq!(evalv("3 >= 4", json!(null)), json!(false));
    assert_eq!(evalv("\"a\" < \"b\"", json!(null)), json!(true));
    assert_eq!(evalv("[1,2] < [1,3]", json!(null)), json!(true));
    assert_eq!(evalv("1 == 1.0", json!(null)), json!(true));
    assert_eq!(evalv("{a:1} == {a:1}", json!(null)), json!(true));
    assert_eq!(evalv("1 != 2", json!(null)), json!(true));
}

#[test]
fn boolean_logic() {
    assert_eq!(evalv("true and true", json!(null)), json!(true));
    assert_eq!(evalv("true and false", json!(null)), json!(false));
    assert_eq!(evalv("false or true", json!(null)), json!(true));
    assert_eq!(evalv("false or false", json!(null)), json!(false));
    assert_eq!(evalv("not false", json!(null)), json!(true));
    assert_eq!(evalv("not true", json!(null)), json!(false));
    // jq truthiness: 0 and "" are truthy.
    assert_eq!(evalv("0 and true", json!(null)), json!(true));
    assert_eq!(evalv("null or \"x\"", json!(null)), json!(true));
}

#[test]
fn alternative_operator() {
    assert_eq!(evalv("false // 5", json!(null)), json!(5));
    assert_eq!(evalv("null // 5", json!(null)), json!(5));
    assert_eq!(evalv("3 // 5", json!(null)), json!(3));
}

// ---- functions ----

#[test]
fn length_function() {
    assert_eq!(evalv("length", json!("hello")), json!(5));
    assert_eq!(evalv("length", json!([1, 2, 3])), json!(3));
    assert_eq!(evalv("length", json!({"a":1,"b":2})), json!(2));
    assert_eq!(evalv("length", json!(null)), json!(0));
    assert_eq!(evalv("length", json!(-5)), json!(5));
}

#[test]
fn keys_function() {
    assert_eq!(evalv("keys", json!({"b":1,"a":2})), json!(["a", "b"]));
    assert_eq!(evalv("keys", json!(["x", "y"])), json!([0, 1]));
}

#[test]
fn map_and_select() {
    assert_eq!(evalv("map(. * 2)", json!([1, 2, 3])), json!([2, 4, 6]));
    assert_eq!(evalv("map(select(. > 1))", json!([1, 2, 3])), json!([2, 3]));
}

#[test]
fn join_function() {
    assert_eq!(evalv("join(\"-\")", json!(["a", "b", "c"])), json!("a-b-c"));
    assert_eq!(evalv("join(\"\")", json!([1, 2])), json!("12"));
}

#[test]
fn string_conversion() {
    assert_eq!(evalv("tostring", json!(42)), json!("42"));
    assert_eq!(evalv("tostring", json!("abc")), json!("abc"));
    assert_eq!(evalv("tonumber", json!("3.5")), json!(3.5));
    assert_eq!(evalv("tonumber", json!(7)), json!(7));
    assert_eq!(evalv("tonumber", json!(true)), json!(1));
}

#[test]
fn string_predicates() {
    assert_eq!(evalv("startswith(\"foo\")", json!("foobar")), json!(true));
    assert_eq!(evalv("endswith(\"bar\")", json!("foobar")), json!(true));
    assert_eq!(evalv("startswith(\"baz\")", json!("foobar")), json!(false));
    assert_eq!(evalv("contains(\"oba\")", json!("foobar")), json!(true));
    assert_eq!(evalv("contains(\"xyz\")", json!("foobar")), json!(false));
}

#[test]
fn split_and_trim() {
    assert_eq!(
        evalv("split(\",\")", json!("a,b,c")),
        json!(["a", "b", "c"])
    );
    assert_eq!(evalv("ltrimstr(\"foo\")", json!("foobar")), json!("bar"));
    assert_eq!(evalv("rtrimstr(\"bar\")", json!("foobar")), json!("foo"));
    assert_eq!(evalv("ascii_upcase", json!("abc")), json!("ABC"));
    assert_eq!(evalv("ascii_downcase", json!("ABC")), json!("abc"));
}

#[test]
fn array_functions() {
    assert_eq!(evalv("first", json!([1, 2, 3])), json!(1));
    assert_eq!(evalv("last", json!([1, 2, 3])), json!(3));
    assert_eq!(evalv("flatten", json!([1, [2, [3]]])), json!([1, 2, 3]));
    assert_eq!(evalv("add", json!([1, 2, 3])), json!(6));
    assert_eq!(evalv("add", json!(["a", "b"])), json!("ab"));
    assert_eq!(evalv("length", json!([])), json!(0));
}

#[test]
fn range_and_type() {
    assert_eq!(evalv("[range(0;3)]", json!(null)), json!([0, 1, 2]));
    assert_eq!(evalv("[range(2;5)]", json!(null)), json!([2, 3, 4]));
    assert_eq!(evalv("type", json!("x")), json!("string"));
    assert_eq!(evalv("type", json!(1)), json!("number"));
    assert_eq!(evalv("type", json!([])), json!("array"));
    assert_eq!(evalv("type", json!({})), json!("object"));
    assert_eq!(evalv("type", json!(null)), json!("null"));
    assert_eq!(evalv("type", json!(true)), json!("boolean"));
}

#[test]
fn has_function() {
    assert_eq!(evalv("has(\"a\")", json!({"a": 1})), json!(true));
    assert_eq!(evalv("has(\"b\")", json!({"a": 1})), json!(false));
    assert_eq!(evalv("has(0)", json!(["x", "y"])), json!(true));
    assert_eq!(evalv("has(5)", json!(["x"])), json!(false));
}

#[test]
fn math_functions() {
    assert_eq!(evalv("fabs", json!(-3.5)), json!(3.5));
    assert_eq!(evalv("floor", json!(3.7)), json!(3));
}

#[test]
fn type_filters() {
    assert_eq!(
        evalv("[.[] | numbers] | length", json!([1, "a", 2.5])),
        json!(2)
    );
    assert_eq!(
        evalv("[.[] | strings] | length", json!([1, "a", "b"])),
        json!(2)
    );
    assert_eq!(evalv("[.[] | nulls] | length", json!([null, 1])), json!(1));
    assert_eq!(evalv("[.[] | arrays] | length", json!([[1], 2])), json!(1));
    assert_eq!(
        evalv("[.[] | objects] | length", json!([{"a":1}, 2])),
        json!(1)
    );
    assert_eq!(
        evalv("[.[] | booleans] | length", json!([true, 2])),
        json!(1)
    );
    assert_eq!(evalv("[.[] | values] | length", json!([null, 2])), json!(1));
}

// ---- pipes, iteration, interpolation, objects ----

#[test]
fn pipe_and_iteration() {
    assert_eq!(evalv(".[] | . * 2", json!([1, 2, 3])), json!(6)); // last output
    assert_eq!(evalv("[.[]] | length", json!([1, 2])), json!(2));
}

#[test]
fn object_construction_cartesian() {
    assert_eq!(evalv("{a: 1, b: 2}", json!(null)), json!({"a": 1, "b": 2}));
    assert_eq!(
        evalv("{(.k): .v}", json!({"k": "x", "v": 9})),
        json!({"x": 9})
    );
    assert_eq!(evalv("{a: [1,2][0]}", json!(null)), json!({"a": 1}));
}

#[test]
fn string_interpolation_in_expression() {
    let v = evalv(
        r#""Hello \(.name), you are \(.age)""#,
        json!({"name":"Ann","age":30}),
    );
    assert_eq!(v, json!("Hello Ann, you are 30"));
}

#[test]
fn variables_in_expressions() {
    let mut vars = serde_json::Map::new();
    vars.insert("x".to_string(), json!(10));
    assert_eq!(eval_vars("$x + 5", json!(null), vars), json!(15));
}

#[test]
fn missing_field_is_null() {
    assert_eq!(evalv(".missing.deep", json!({"a": 1})), json!(null));
    assert_eq!(evalv(".[5]", json!([1, 2])), json!(null));
    assert_eq!(evalv(".[-1]", json!([1, 2, 3])), json!(3));
}

#[test]
fn conditional_expression() {
    assert_eq!(
        evalv(
            "if .n > 10 then \"big\" elif .n > 5 then \"mid\" else \"small\" end",
            json!({"n": 7})
        ),
        json!("mid")
    );
    assert_eq!(
        evalv(
            "if .n > 10 then \"big\" else \"small\" end",
            json!({"n": 3})
        ),
        json!("small")
    );
}

#[test]
fn unsupported_function_errors() {
    let e = parse("nonexistent_fn(1)").unwrap();
    let r = eval_single(&e, &json!(null), &Env::new(Default::default()));
    assert!(r.is_err());
}

#[test]
fn division_by_zero_errors() {
    let e = parse("1 / 0").unwrap();
    assert!(eval_single(&e, &json!(null), &Env::new(Default::default())).is_err());
}
