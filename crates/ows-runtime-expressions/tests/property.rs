//! Property-based tests for the expression engine using `proptest`.

use ows_runtime_expressions::{eval_single, parse, Env};
use proptest::prelude::*;
use serde_json::{json, Value};

fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(|n| json!(n)),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| json!(f)),
        any::<bool>().prop_map(Value::Bool),
        "[a-z]{0,8}".prop_map(Value::String),
        Just(Value::Null),
    ];
    leaf.prop_recursive(4, 32, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::hash_map("[a-z]{1,4}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Identity must always return the input unchanged.
    #[test]
    fn identity_is_identity(input in arb_value()) {
        let e = parse(".").unwrap();
        let out = eval_single(&e, &input, &Env::new(Default::default())).unwrap();
        assert_eq!(out, input);
    }

    /// `. == input` must be true for any input.
    #[test]
    fn equality_with_input(input in arb_value()) {
        let expr = format!(". == {}", serde_json::to_string(&input).unwrap());
        let e = parse(&expr).unwrap();
        let out = eval_single(&e, &input, &Env::new(Default::default())).unwrap();
        assert_eq!(out, Value::Bool(true), "input {input} expr {expr}");
    }

    /// Adding zero leaves numbers unchanged.
    #[test]
    fn add_zero_is_identity(n in any::<i64>()) {
        let input = json!(n);
        let e = parse(". + 0").unwrap();
        let out = eval_single(&e, &input, &Env::new(Default::default())).unwrap();
        assert_eq!(out, json!(n));
    }

    /// Object construction preserves the input field values.
    #[test]
    fn object_roundtrip(k in "[a-z]{1,4}", v in any::<i64>()) {
        let input = json!({ k.clone(): v });
        let expr = format!("{{ {}: .{} }}", k, k);
        let e = parse(&expr).unwrap();
        let out = eval_single(&e, &input, &Env::new(Default::default())).unwrap();
        assert_eq!(out, json!({ k.clone(): v }));
    }
}

/// Proptest strategy that always yields valid strings is used by the tests above;
/// this extra test guards against panics on malformed input.
#[test]
fn never_panics_on_garbage() {
    for src in [
        ".",
        "..",
        ".foo.bar",
        "[",
        "{",
        ")",
        ".a +",
        "if then",
        "map(",
        "\"unterminated",
        ".a.b.c[0].d",
        "[1,2,3] | .[1]",
        "{ a: .b, c: [ .d, $x ] }",
    ] {
        let _ = parse(src);
    }
}
