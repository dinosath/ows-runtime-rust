//! Benchmarks for parsing, compilation, expression evaluation, and execution.
#![allow(clippy::result_large_err)]

use criterion::{criterion_group, criterion_main, Criterion};
use ows_runtime::Runtime;
use ows_runtime_expressions::{parse, Env};
use serde_json::json;

const WORKFLOW: &str = r#"
document:
  dsl: '1.0.3'
  namespace: bench
  name: bench
  version: '1.0.0'
do:
  - build:
      set:
        greeting: '${ "Hello, \(.name)!" }'
"#;

fn bench_parse(c: &mut Criterion) {
    c.bench_function("parse_yaml", |b| {
        b.iter(|| ows_runtime_dsl::from_yaml(WORKFLOW).unwrap())
    });
}

fn bench_compile(c: &mut Criterion) {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(WORKFLOW).unwrap();
    c.bench_function("compile", |b| b.iter(|| runtime.compile(&def).unwrap()));
}

fn bench_expression(c: &mut Criterion) {
    c.bench_function("expression_field_access", |b| {
        let expr = parse(".user.claims.subject").unwrap();
        let input = json!({ "user": { "claims": { "subject": "abc" } } });
        let env = Env::new(Default::default());
        b.iter(|| ows_runtime_expressions::eval_single(&expr, &input, &env))
    });
}

fn bench_execute(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("execute_simple_workflow", |b| {
        b.iter_batched(
            || {
                let runtime = Runtime::builder().build().unwrap();
                let def = ows_runtime_dsl::from_yaml(WORKFLOW).unwrap();
                let wf = runtime.register_definition(&def).unwrap();
                (runtime, wf)
            },
            |(runtime, wf)| rt.block_on(runtime.run(wf, json!({ "name": "World" }))),
            criterion::BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_parse,
    bench_compile,
    bench_expression,
    bench_execute
);
criterion_main!(benches);
