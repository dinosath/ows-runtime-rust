//! Stream-based evaluator for the sandboxed expression language.
//!
//! Evaluation follows jq's model: an expression maps a single input value to a
//! stream of zero or more output values. Pipes compose these streams. This
//! keeps the engine faithful to jq semantics while remaining fully sandboxed
//! (no arbitrary code execution).

use serde_json::{Map, Value};

use super::ast::{BinOp, Expr, InterpPart, ObjKey, UnaryOp};
use super::error::ExpressionError;

/// The evaluation environment (variables, e.g. `$input`, `$context`, `$pet`).
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// Named variables (`$name`).
    pub variables: Map<String, Value>,
}

impl Env {
    /// Creates a new environment.
    pub fn new(variables: Map<String, Value>) -> Self {
        Self { variables }
    }
}

/// Evaluates an expression against an input value, producing an output stream.
pub fn eval(expr: &Expr, input: &Value, env: &Env) -> Result<Vec<Value>, ExpressionError> {
    match expr {
        Expr::Identity => Ok(vec![input.clone()]),
        Expr::Literal(v) => Ok(vec![v.clone()]),
        Expr::Variable(name) => match env.variables.get(name) {
            Some(v) => Ok(vec![v.clone()]),
            None => Err(ExpressionError::type_error(format!(
                "undefined variable `${name}`"
            ))),
        },
        Expr::Str(parts) => eval_string(parts, input, env),
        Expr::Field(base, name) => {
            let stream = eval(base, input, env)?;
            let mut out = Vec::new();
            for v in stream {
                out.push(field_access(&v, name));
            }
            Ok(out)
        }
        Expr::Index(base, idx) => {
            let base_stream = eval(base, input, env)?;
            let idx_stream = eval(idx, input, env)?;
            let mut out = Vec::new();
            for v in &base_stream {
                for i in &idx_stream {
                    out.push(index_access(v, i)?);
                }
            }
            Ok(out)
        }
        Expr::Iterate(base) => {
            let stream = eval(base, input, env)?;
            let mut out = Vec::new();
            for v in stream {
                match v {
                    Value::Array(items) => out.extend(items),
                    Value::Object(map) => out.extend(map.into_values()),
                    other => out.push(other),
                }
            }
            Ok(out)
        }
        Expr::Binary(op, l, r) => eval_binary(*op, l, r, input, env),
        Expr::Unary(op, inner) => {
            let stream = eval(inner, input, env)?;
            let mut out = Vec::new();
            for v in stream {
                let r = match op {
                    UnaryOp::Not => Value::Bool(!truthy(&v)),
                    UnaryOp::Negate => match v.as_f64() {
                        Some(n) => json_num(-n),
                        None => {
                            return Err(ExpressionError::type_error(format!(
                                "cannot negate non-number `{v}`"
                            )))
                        }
                    },
                };
                out.push(r);
            }
            Ok(out)
        }
        Expr::Pipe(l, r) => {
            let left = eval(l, input, env)?;
            let mut out = Vec::new();
            for v in left {
                out.extend(eval(r, &v, env)?);
            }
            Ok(out)
        }
        Expr::Array(elems) => {
            let mut out = Vec::new();
            for e in elems {
                out.extend(eval(e, input, env)?);
            }
            Ok(vec![Value::Array(out)])
        }
        Expr::Object(entries) => eval_object(entries, input, env),
        Expr::If(c, t, f) => {
            let cond = eval(c, input, env)?;
            let cond = cond.first().cloned().unwrap_or(Value::Null);
            if truthy(&cond) {
                eval(t, input, env)
            } else {
                eval(f, input, env)
            }
        }
        Expr::Call(name, args) => call_function(name, args, input, env),
        Expr::Stream(parts) => {
            let mut out = Vec::new();
            for p in parts {
                out.extend(eval(p, input, env)?);
            }
            Ok(out)
        }
    }
}

fn eval_string(
    parts: &[InterpPart],
    input: &Value,
    env: &Env,
) -> Result<Vec<Value>, ExpressionError> {
    let mut out = String::new();
    for part in parts {
        match part {
            InterpPart::Literal(s) => out.push_str(s),
            InterpPart::Expr(e) => {
                let stream = eval(e, input, env)?;
                let v = stream.last().cloned().unwrap_or(Value::Null);
                out.push_str(&to_string(&v));
            }
        }
    }
    Ok(vec![Value::String(out)])
}

fn eval_binary(
    op: BinOp,
    l: &Expr,
    r: &Expr,
    input: &Value,
    env: &Env,
) -> Result<Vec<Value>, ExpressionError> {
    let lstream = eval(l, input, env)?;

    // `and`, `or` and `//` short-circuit: the right side is only evaluated when
    // necessary (jq semantics, and avoids e.g. division by zero in dead branches).
    match op {
        BinOp::And => {
            let lv = lstream.last().cloned().unwrap_or(Value::Null);
            if !truthy(&lv) {
                return Ok(vec![Value::Bool(false)]);
            }
            let rv = eval(r, input, env)?.last().cloned().unwrap_or(Value::Null);
            return Ok(vec![Value::Bool(truthy(&rv))]);
        }
        BinOp::Or => {
            let lv = lstream.last().cloned().unwrap_or(Value::Null);
            if truthy(&lv) {
                return Ok(vec![Value::Bool(true)]);
            }
            let rv = eval(r, input, env)?.last().cloned().unwrap_or(Value::Null);
            return Ok(vec![Value::Bool(truthy(&rv))]);
        }
        BinOp::Alternative => {
            let lv = lstream.last().cloned().unwrap_or(Value::Null);
            if !truthy(&lv) {
                return eval(r, input, env);
            } else {
                return Ok(lstream);
            }
        }
        _ => {}
    }

    let rstream = eval(r, input, env)?;
    let mut out = Vec::new();
    for lv in &lstream {
        for rv in &rstream {
            out.push(apply_binop(op, lv, rv)?);
        }
    }
    Ok(out)
}

fn apply_binop(op: BinOp, l: &Value, r: &Value) -> Result<Value, ExpressionError> {
    match op {
        BinOp::Add => add(l, r),
        BinOp::Sub => sub(l, r),
        BinOp::Mul => mul(l, r),
        BinOp::Div => div(l, r),
        BinOp::Mod => mod_op(l, r),
        BinOp::Eq => Ok(Value::Bool(equal(l, r))),
        BinOp::Ne => Ok(Value::Bool(!equal(l, r))),
        BinOp::Lt => Ok(Value::Bool(cmp(l, r)? < std::cmp::Ordering::Equal)),
        BinOp::Le => Ok(Value::Bool(cmp(l, r)? != std::cmp::Ordering::Greater)),
        BinOp::Gt => Ok(Value::Bool(cmp(l, r)? > std::cmp::Ordering::Equal)),
        BinOp::Ge => Ok(Value::Bool(cmp(l, r)? != std::cmp::Ordering::Less)),
        BinOp::And | BinOp::Or | BinOp::Alternative => unreachable!("handled above"),
    }
}

fn add(l: &Value, r: &Value) -> Result<Value, ExpressionError> {
    // `null` is the identity for addition in jq.
    if matches!(l, Value::Null) {
        return Ok(r.clone());
    }
    if matches!(r, Value::Null) {
        return Ok(l.clone());
    }
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(ai), Some(bi)) = (a.as_i64(), b.as_i64()) {
                Ok(json_int(ai + bi))
            } else {
                let sum = a.as_f64().unwrap() + b.as_f64().unwrap();
                Ok(json_num(sum))
            }
        }
        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
        (Value::Array(a), Value::Array(b)) => {
            let mut out = a.clone();
            out.extend(b.clone());
            Ok(Value::Array(out))
        }
        (Value::String(a), b) => Ok(Value::String(format!("{a}{}", to_string(b)))),
        (a, Value::String(b)) => Ok(Value::String(format!("{}{b}", to_string(a)))),
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            out.extend(b.clone());
            Ok(Value::Object(out))
        }
        _ => Err(ExpressionError::type_error(format!(
            "cannot add `{l}` and `{r}`"
        ))),
    }
}

fn sub(l: &Value, r: &Value) -> Result<Value, ExpressionError> {
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(ai), Some(bi)) = (a.as_i64(), b.as_i64()) {
                Ok(json_int(ai - bi))
            } else {
                Ok(json_num(a.as_f64().unwrap() - b.as_f64().unwrap()))
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            let out = a.iter().filter(|x| !b.contains(x)).cloned().collect();
            Ok(Value::Array(out))
        }
        _ => Err(ExpressionError::type_error(format!(
            "cannot subtract `{r}` from `{l}`"
        ))),
    }
}

fn mul(l: &Value, r: &Value) -> Result<Value, ExpressionError> {
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(ai), Some(bi)) = (a.as_i64(), b.as_i64()) {
                Ok(json_int(ai * bi))
            } else {
                Ok(json_num(a.as_f64().unwrap() * b.as_f64().unwrap()))
            }
        }
        (Value::String(s), Value::Number(n)) => {
            let n = n.as_f64().unwrap().max(0.0) as usize;
            Ok(Value::String(s.repeat(n)))
        }
        (Value::Number(n), Value::String(s)) => {
            let n = n.as_f64().unwrap().max(0.0) as usize;
            Ok(Value::String(s.repeat(n)))
        }
        _ => Err(ExpressionError::type_error(format!(
            "cannot multiply `{l}` and `{r}`"
        ))),
    }
}

fn div(l: &Value, r: &Value) -> Result<Value, ExpressionError> {
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => {
            let b = b.as_f64().unwrap();
            if b == 0.0 {
                Err(ExpressionError::eval("division by zero"))
            } else {
                Ok(json_num(a.as_f64().unwrap() / b))
            }
        }
        (Value::String(a), Value::String(b)) => {
            let parts: Vec<Value> = a.split(b).map(|s| Value::String(s.to_string())).collect();
            Ok(Value::Array(parts))
        }
        _ => Err(ExpressionError::type_error(format!(
            "cannot divide `{l}` by `{r}`"
        ))),
    }
}

fn mod_op(l: &Value, r: &Value) -> Result<Value, ExpressionError> {
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(ai), Some(bi)) = (a.as_i64(), b.as_i64()) {
                if bi == 0 {
                    Err(ExpressionError::eval("modulo by zero"))
                } else {
                    Ok(json_int(ai % bi))
                }
            } else {
                let b = b.as_f64().unwrap();
                if b == 0.0 {
                    Err(ExpressionError::eval("modulo by zero"))
                } else {
                    Ok(json_num(a.as_f64().unwrap() % b))
                }
            }
        }
        _ => Err(ExpressionError::type_error(format!(
            "cannot take `{l}` modulo `{r}`"
        ))),
    }
}

fn equal(l: &Value, r: &Value) -> bool {
    // Numbers compared numerically.
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => a.as_f64().unwrap() == b.as_f64().unwrap(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).map(|bv| equal(v, bv)).unwrap_or(false))
        }
        _ => l == r,
    }
}

fn cmp(l: &Value, r: &Value) -> Result<std::cmp::Ordering, ExpressionError> {
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => Ok(a
            .as_f64()
            .unwrap()
            .partial_cmp(&b.as_f64().unwrap())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        (Value::Array(a), Value::Array(b)) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let ord = cmp(x, y)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(a.len().cmp(&b.len()))
        }
        _ => Err(ExpressionError::type_error(format!(
            "cannot compare `{l}` and `{r}`"
        ))),
    }
}

/// jq truthiness: only `false` and `null` are falsy.
fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Bool(false) | Value::Null)
}

fn field_access(v: &Value, name: &str) -> Value {
    match v {
        Value::Object(map) => map.get(name).cloned().unwrap_or(Value::Null),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|e| match e {
                    Value::Object(m) => m.get(name).cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

fn index_access(v: &Value, idx: &Value) -> Result<Value, ExpressionError> {
    match v {
        Value::Array(items) => match idx {
            Value::Number(n) => {
                let i = n.as_f64().unwrap() as i64;
                let len = items.len() as i64;
                let real = if i < 0 { len + i } else { i };
                if real < 0 || real >= len {
                    Ok(Value::Null)
                } else {
                    Ok(items[real as usize].clone())
                }
            }
            Value::String(_) => Ok(Value::Null),
            other => Err(ExpressionError::type_error(format!(
                "array index must be a number, got `{other}`"
            ))),
        },
        Value::Object(map) => match idx {
            Value::String(s) => Ok(map.get(s).cloned().unwrap_or(Value::Null)),
            Value::Number(_) => Ok(Value::Null),
            other => Err(ExpressionError::type_error(format!(
                "object index must be a string, got `{other}`"
            ))),
        },
        Value::String(s) => match idx {
            Value::Number(n) => {
                let i = n.as_f64().unwrap() as i64;
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let real = if i < 0 { len + i } else { i };
                if real < 0 || real >= len {
                    Ok(Value::Null)
                } else {
                    Ok(Value::String(chars[real as usize].to_string()))
                }
            }
            _ => Err(ExpressionError::type_error("string index must be a number")),
        },
        _ => Ok(Value::Null),
    }
}

fn eval_object(
    entries: &[(ObjKey, Expr)],
    input: &Value,
    env: &Env,
) -> Result<Vec<Value>, ExpressionError> {
    // Evaluate each entry value to a stream, then take the cartesian product.
    let mut evaluated: Vec<(String, Vec<Value>)> = Vec::new();
    for (key, value_expr) in entries {
        let key_str = match key {
            ObjKey::Ident(s) | ObjKey::Str(s) => s.clone(),
            ObjKey::Computed(e) => {
                let stream = eval(e, input, env)?;
                let v = stream.last().cloned().unwrap_or(Value::Null);
                to_string(&v)
            }
        };
        let stream = eval(value_expr, input, env)?;
        evaluated.push((key_str, stream));
    }

    // Build objects from the cartesian product.
    let mut results = vec![Map::new()];
    for (key, values) in &evaluated {
        let mut next = Vec::new();
        for obj in &results {
            if values.is_empty() {
                let mut obj = obj.clone();
                obj.insert(key.clone(), Value::Null);
                next.push(obj);
            } else {
                for v in values {
                    let mut obj = obj.clone();
                    obj.insert(key.clone(), v.clone());
                    next.push(obj);
                }
            }
        }
        results = next;
    }
    Ok(results.into_iter().map(Value::Object).collect())
}

fn call_function(
    name: &str,
    args: &[Expr],
    input: &Value,
    env: &Env,
) -> Result<Vec<Value>, ExpressionError> {
    match name {
        "length" => {
            let n = match input {
                Value::Object(m) => m.len(),
                Value::Array(a) => a.len(),
                Value::String(s) => s.chars().count(),
                Value::Number(n) => n.as_f64().unwrap().abs() as usize,
                Value::Null => 0,
                _ => return Err(ExpressionError::type_error("length of unsupported type")),
            };
            Ok(vec![json_num(n as f64)])
        }
        "keys" => match input {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                Ok(vec![Value::Array(
                    keys.into_iter().map(|k| Value::String(k.clone())).collect(),
                )])
            }
            Value::Array(a) => {
                let keys: Vec<Value> = (0..a.len()).map(|i| json_num(i as f64)).collect();
                Ok(vec![Value::Array(keys)])
            }
            _ => Err(ExpressionError::type_error("keys of non-object/array")),
        },
        "map" => {
            let f = args
                .first()
                .ok_or_else(|| ExpressionError::eval("map requires a filter"))?;
            match input {
                Value::Array(items) => {
                    let mut out = Vec::new();
                    for item in items {
                        out.extend(eval(f, item, env)?);
                    }
                    Ok(vec![Value::Array(out)])
                }
                other => Err(ExpressionError::type_error(format!(
                    "map expects an array, got `{other}`"
                ))),
            }
        }
        "select" => {
            let f = args
                .first()
                .ok_or_else(|| ExpressionError::eval("select requires a filter"))?;
            if truthy(&eval(f, input, env)?.first().cloned().unwrap_or(Value::Null)) {
                Ok(vec![input.clone()])
            } else {
                Ok(vec![])
            }
        }
        "join" => {
            let sep_arg = args.first().map(|e| eval(e, input, env)).transpose()?;
            let sep = sep_arg
                .and_then(|s| s.first().cloned())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            match input {
                Value::Array(items) => {
                    let joined = items.iter().map(to_string).collect::<Vec<_>>().join(&sep);
                    Ok(vec![Value::String(joined)])
                }
                other => Err(ExpressionError::type_error(format!(
                    "join expects an array, got `{other}`"
                ))),
            }
        }
        "tostring" => Ok(vec![Value::String(to_string(input))]),
        "tonumber" => {
            let n = match input {
                Value::Number(_) => input.as_f64().unwrap(),
                Value::String(s) => s.parse::<f64>().map_err(|_| {
                    ExpressionError::eval(format!("cannot parse `{s}` as a number"))
                })?,
                Value::Bool(b) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
                other => {
                    return Err(ExpressionError::type_error(format!(
                        "cannot convert `{other}` to number"
                    )))
                }
            };
            Ok(vec![json_num(n)])
        }
        "startswith" | "endswith" => {
            let arg = args.first().map(|e| eval(e, input, env)).transpose()?;
            let needle = arg
                .and_then(|s| s.first().cloned())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .ok_or_else(|| ExpressionError::eval("startswith/endswith requires a string"))?;
            let s = input.as_str().ok_or_else(|| {
                ExpressionError::type_error("startswith/endswith expects a string input")
            })?;
            let res = if name == "startswith" {
                s.starts_with(&needle)
            } else {
                s.ends_with(&needle)
            };
            Ok(vec![Value::Bool(res)])
        }
        "contains" => {
            let arg = args.first().map(|e| eval(e, input, env)).transpose()?;
            let needle = arg.and_then(|s| s.first().cloned()).unwrap_or(Value::Null);
            Ok(vec![Value::Bool(contains_value(input, &needle))])
        }
        "split" => {
            let arg = args.first().map(|e| eval(e, input, env)).transpose()?;
            let sep = arg
                .and_then(|s| s.first().cloned())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            match input {
                Value::String(s) => Ok(vec![Value::Array(
                    s.split(&sep)
                        .map(|p| Value::String(p.to_string()))
                        .collect(),
                )]),
                other => Err(ExpressionError::type_error(format!(
                    "split expects a string, got `{other}`"
                ))),
            }
        }
        "empty" => Ok(vec![]),
        "range" => {
            let mut vals = Vec::new();
            let nums: Result<Vec<f64>, _> = args
                .iter()
                .map(|a| eval(a, input, env))
                .collect::<Result<Vec<_>, _>>()
                .and_then(|streams| {
                    streams
                        .iter()
                        .map(|s| {
                            s.first()
                                .and_then(|v| v.as_f64())
                                .ok_or_else(|| ExpressionError::eval("range requires numbers"))
                        })
                        .collect()
                });
            let nums = nums?;
            if nums.len() == 1 {
                for i in 0..nums[0] as i64 {
                    vals.push(json_num(i as f64));
                }
            } else if nums.len() >= 2 {
                let (a, b) = (nums[0] as i64, nums[1] as i64);
                let mut i = a;
                while i < b {
                    vals.push(json_num(i as f64));
                    i += 1;
                }
            }
            Ok(vals)
        }
        "first" => Ok(match input {
            Value::Array(a) => vec![a.first().cloned().unwrap_or(Value::Null)],
            other => vec![other.clone()],
        }),
        "last" => Ok(match input {
            Value::Array(a) => vec![a.last().cloned().unwrap_or(Value::Null)],
            other => vec![other.clone()],
        }),
        "flatten" => {
            let mut out = Vec::new();
            flatten_into(input, &mut out);
            Ok(vec![Value::Array(out)])
        }
        "add" => match input {
            Value::Array(items) => {
                let mut acc = Value::Null;
                for item in items {
                    acc = add(&acc, item)?;
                }
                Ok(vec![acc])
            }
            other => Ok(vec![other.clone()]),
        },
        "type" => {
            let t = match input {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            Ok(vec![Value::String(t.into())])
        }
        "not" => Ok(vec![Value::Bool(!truthy(input))]),
        "has" => {
            let arg = args.first().map(|e| eval(e, input, env)).transpose()?;
            let key = arg.and_then(|s| s.first().cloned()).unwrap_or(Value::Null);
            let res = match input {
                Value::Object(m) => m.contains_key(key.as_str().unwrap_or("")),
                Value::Array(a) => key
                    .as_f64()
                    .map(|i| (i as i64) >= 0 && (i as usize) < a.len())
                    .unwrap_or(false),
                _ => false,
            };
            Ok(vec![Value::Bool(res)])
        }
        "ascii_downcase" | "ascii_upcase" => {
            let s = input
                .as_str()
                .ok_or_else(|| ExpressionError::type_error("expects a string"))?;
            let res = if name == "ascii_downcase" {
                s.to_ascii_lowercase()
            } else {
                s.to_ascii_uppercase()
            };
            Ok(vec![Value::String(res)])
        }
        "ltrimstr" | "rtrimstr" => {
            let arg = args.first().map(|e| eval(e, input, env)).transpose()?;
            let trim = arg
                .and_then(|s| s.first().cloned())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let s = input.as_str().unwrap_or("");
            let res = if name == "ltrimstr" {
                s.strip_prefix(&trim).unwrap_or(s)
            } else {
                s.strip_suffix(&trim).unwrap_or(s)
            };
            Ok(vec![Value::String(res.to_string())])
        }
        "error" => {
            let msg = if args.is_empty() {
                to_string(input)
            } else {
                to_string(
                    &eval(&args[0], input, env)?
                        .first()
                        .cloned()
                        .unwrap_or(Value::Null),
                )
            };
            Err(ExpressionError::eval(msg))
        }
        "fabs" => match input {
            Value::Number(_) => Ok(vec![json_num(input.as_f64().unwrap().abs())]),
            other => Err(ExpressionError::type_error(format!("fabs of `{other}`"))),
        },
        "floor" => match input {
            Value::Number(_) => Ok(vec![json_num(input.as_f64().unwrap().floor())]),
            other => Err(ExpressionError::type_error(format!("floor of `{other}`"))),
        },
        "values" | "arrays" | "objects" | "strings" | "numbers" | "booleans" | "nulls" => {
            let keep = match name {
                "values" => !matches!(input, Value::Null),
                "arrays" => matches!(input, Value::Array(_)),
                "objects" => matches!(input, Value::Object(_)),
                "strings" => matches!(input, Value::String(_)),
                "numbers" => matches!(input, Value::Number(_)),
                "booleans" => matches!(input, Value::Bool(_)),
                "nulls" => matches!(input, Value::Null),
                _ => unreachable!(),
            };
            if keep {
                Ok(vec![input.clone()])
            } else {
                Ok(vec![])
            }
        }
        _ => Err(ExpressionError::UnsupportedFunction { name: name.into() }),
    }
}

fn contains_value(haystack: &Value, needle: &Value) -> bool {
    match (haystack, needle) {
        (Value::String(h), Value::String(n)) => h.contains(n),
        (Value::Array(h), Value::Array(n)) => {
            n.iter().all(|n| h.iter().any(|h| contains_value(h, n)))
        }
        (Value::Object(h), Value::Object(n)) => n
            .iter()
            .all(|(k, v)| h.get(k).map(|hv| contains_value(hv, v)).unwrap_or(false)),
        (h, n) => equal(h, n),
    }
}

fn flatten_into(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(items) => {
            for item in items {
                flatten_into(item, out);
            }
        }
        other => out.push(other.clone()),
    }
}

/// Converts a JSON value to its jq string representation.
pub fn to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::Null => "null".into(),
        Value::Number(n) => n.to_string(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn json_int(n: i64) -> Value {
    Value::Number(serde_json::Number::from(n))
}

fn json_num(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        Value::Number(serde_json::Number::from(n as i64))
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// Convenience helper for tests and engine integration.
pub fn eval_single(expr: &Expr, input: &Value, env: &Env) -> Result<Value, ExpressionError> {
    let stream = eval(expr, input, env)?;
    Ok(stream.into_iter().last().unwrap_or(Value::Null))
}
