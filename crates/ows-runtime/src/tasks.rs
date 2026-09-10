//! Built-in OWS task implementations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ows_runtime_core::{
    ErrorKind, EventMessage, ExpressionContext, ProblemDetails, StandardErrorType, WorkflowError,
};
use serde_json::{json, Map, Value};
use tokio::task::JoinSet;

use crate::engine::{
    eval_expr, eval_interpolation, exec_scope, exec_task, is_truthy, parse_directive, ExecContext,
    FlowDirective,
};
use crate::ir::*;
use crate::runtime::RuntimeInner;
use crate::service::FunctionRequest;

/// The result of dispatching a task.
pub(crate) struct Dispatched {
    /// The raw task output.
    pub output: Value,
    /// An optional directive decided by the task itself.
    pub directive: Option<FlowDirective>,
}

/// Dispatches a task to its concrete implementation and returns its raw output.
pub(crate) async fn dispatch(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    task: &CompiledTask,
    task_input: &Value,
    expr_ctx: &mut ExpressionContext,
    exec_ctx: &mut ExecContext,
) -> Result<Dispatched, WorkflowError> {
    let output = match &task.kind {
        CompiledTaskKind::Set(d) => task_set(inner, d, expr_ctx)?,
        CompiledTaskKind::Do(body) => {
            exec_scope(inner, workflow, body, exec_ctx, task_input, None).await?
        }
        CompiledTaskKind::For(d) => {
            task_for(inner, workflow, d, task_input, expr_ctx, exec_ctx).await?
        }
        CompiledTaskKind::Fork(d) => {
            task_fork(inner, workflow, d, task_input, expr_ctx, exec_ctx).await?
        }
        CompiledTaskKind::Switch(cases) => {
            let (out, directive) = task_switch(inner, cases, task_input, expr_ctx)?;
            return Ok(Dispatched {
                output: out,
                directive,
            });
        }
        CompiledTaskKind::Try(d) => {
            task_try(inner, workflow, d, task_input, expr_ctx, exec_ctx).await?
        }
        CompiledTaskKind::Emit(d) => task_emit(inner, workflow, d, expr_ctx).await?,
        CompiledTaskKind::Listen(d) => task_listen(inner, workflow, d, expr_ctx, exec_ctx).await?,
        CompiledTaskKind::Raise(d) => {
            return Err(task_raise(inner, workflow, task, d, expr_ctx, exec_ctx)?);
        }
        CompiledTaskKind::Call(d) => task_call(inner, workflow, d, expr_ctx).await?,
        CompiledTaskKind::Run(d) => task_run(inner, d, task_input, expr_ctx, exec_ctx).await?,
        CompiledTaskKind::Wait(d) => task_wait(inner, d, exec_ctx).await?,
    };
    Ok(Dispatched {
        output,
        directive: None,
    })
}

/// The `set` task.
fn task_set(
    inner: &Arc<RuntimeInner>,
    def: &SetDef,
    expr_ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    match &def.values {
        SetValues::Expression(src) => {
            eval_interpolation(inner, &Value::String(src.clone()), expr_ctx)
        }
        SetValues::Map(entries) => {
            let mut out = Map::new();
            for (k, v) in entries {
                let resolved = eval_interpolation(inner, v, expr_ctx)?;
                out.insert(k.clone(), resolved);
            }
            Ok(Value::Object(out))
        }
    }
}

/// The `for` task.
async fn task_for(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    def: &ForDef,
    task_input: &Value,
    expr_ctx: &ExpressionContext,
    exec_ctx: &mut ExecContext,
) -> Result<Value, WorkflowError> {
    let vars = expr_ctx.as_variable_map();
    let collection = eval_expr(inner, &def.in_, task_input, &vars, None)?;
    let items = match collection {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        other => {
            return Err(WorkflowError::new(
                ErrorKind::Expression,
                ProblemDetails::standard(StandardErrorType::Expression)
                    .with_detail(format!("for `in` must evaluate to an array, got `{other}`")),
            ))
        }
    };

    let max_iterations = inner.policy.max_loop_iterations;
    if items.len() > max_iterations {
        return Err(crate::error::policy_error(format!(
            "for loop exceeds the maximum of {max_iterations} iterations"
        )));
    }

    let mut body_input = task_input.clone();
    let mut output = Value::Null;

    // Per the spec, `each` defaults to `item` and `at` defaults to `index`.
    let each_name = def.each.as_str();
    let at_name = def.at.as_deref().unwrap_or("index");

    for (index, item) in items.into_iter().enumerate() {
        if exec_ctx.cancel.is_cancelled() {
            return Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()));
        }

        exec_ctx.loop_vars.insert(each_name.to_string(), item);
        exec_ctx.loop_vars.insert(at_name.to_string(), json!(index));

        if let Some(while_expr) = &def.while_ {
            let vars = exec_ctx.variables(&body_input, &Value::Null, None);
            let cond = eval_expr(inner, while_expr, &body_input, &vars, None)?;
            if !is_truthy(&cond) {
                break;
            }
        }

        let body_output =
            exec_scope(inner, workflow, &def.body, exec_ctx, &body_input, None).await?;
        output = body_output.clone();
        body_input = body_output;

        if exec_ctx.ended.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }

    exec_ctx.loop_vars.remove(each_name);
    exec_ctx.loop_vars.remove(at_name);

    Ok(output)
}

/// The `fork` task.
async fn task_fork(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    def: &ForkDef,
    task_input: &Value,
    expr_ctx: &ExpressionContext,
    exec_ctx: &ExecContext,
) -> Result<Value, WorkflowError> {
    let _ = expr_ctx;
    if def.branches.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }

    if def.compete {
        let mut set = JoinSet::new();
        for (i, branch) in def.branches.iter().enumerate() {
            set.spawn(run_branch(
                inner.clone(),
                workflow.clone(),
                branch.clone(),
                task_input.clone(),
                exec_ctx.clone(),
                i,
            ));
        }
        let winner = match set.join_next().await {
            Some(Ok((_i, Ok(value)))) => value,
            Some(Ok((_i, Err(err)))) => {
                set.shutdown().await;
                return Err(err);
            }
            Some(Err(err)) => {
                set.shutdown().await;
                return Err(crate::error::runtime_error(format!(
                    "fork branch failed: {err}"
                )));
            }
            None => Value::Null,
        };
        set.shutdown().await;
        Ok(winner)
    } else {
        let mut set = JoinSet::new();
        for (i, branch) in def.branches.iter().enumerate() {
            set.spawn(run_branch(
                inner.clone(),
                workflow.clone(),
                branch.clone(),
                task_input.clone(),
                exec_ctx.clone(),
                i,
            ));
        }

        let mut results: Vec<Option<Result<Value, WorkflowError>>> = vec![None; def.branches.len()];
        let mut first_error: Option<WorkflowError> = None;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((i, Ok(value))) => results[i] = Some(Ok(value)),
                Ok((i, Err(err))) => {
                    if first_error.is_none() {
                        first_error = Some(err.clone());
                    }
                    results[i] = Some(Err(err));
                }
                Err(err) => {
                    if first_error.is_none() {
                        first_error = Some(crate::error::runtime_error(format!(
                            "fork branch panicked: {err}"
                        )));
                    }
                }
            }
        }

        if let Some(err) = first_error {
            return Err(err);
        }

        let outputs = results
            .into_iter()
            .map(|r| r.unwrap().unwrap_or(Value::Null))
            .collect();
        Ok(Value::Array(outputs))
    }
}

/// Runs a single fork branch and returns `(branch_index, output)`.
async fn run_branch(
    inner: Arc<RuntimeInner>,
    workflow: Arc<CompiledWorkflow>,
    task: CompiledTask,
    input: Value,
    mut ctx: ExecContext,
    index: usize,
) -> (usize, Result<Value, WorkflowError>) {
    let mut input = input;
    let result = exec_task(&inner, &workflow, &task, &mut input, &mut ctx).await;
    (index, result.map(|r| r.output))
}

/// The `switch` task. Returns the matched case's directive, if any.
fn task_switch(
    inner: &Arc<RuntimeInner>,
    cases: &[SwitchCase],
    task_input: &Value,
    expr_ctx: &ExpressionContext,
) -> Result<(Value, Option<FlowDirective>), WorkflowError> {
    let vars = expr_ctx.as_variable_map();
    for case in cases {
        let matched = match &case.when {
            Some(cond) => is_truthy(&eval_expr(inner, cond, task_input, &vars, None)?),
            None => true, // default case
        };
        if matched {
            let directive = case
                .then
                .as_deref()
                .map(parse_directive)
                .unwrap_or(FlowDirective::Continue);
            return Ok((task_input.clone(), Some(directive)));
        }
    }
    Ok((task_input.clone(), None))
}

/// The `try` task.
async fn task_try(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    def: &TryDef,
    task_input: &Value,
    expr_ctx: &ExpressionContext,
    exec_ctx: &mut ExecContext,
) -> Result<Value, WorkflowError> {
    let catch = &def.catch;
    let mut attempt: usize = 0;

    loop {
        if exec_ctx.cancel.is_cancelled() {
            return Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()));
        }

        match exec_scope(inner, workflow, &def.body, exec_ctx, task_input, None).await {
            Ok(output) => return Ok(output),
            Err(err) => {
                set_error_var(exec_ctx, catch, &err);

                if !catch_matches(inner, catch, &err, expr_ctx, exec_ctx)? {
                    return Err(err);
                }

                let retryable = catch
                    .retry
                    .as_ref()
                    .map(|r| retryable(inner, r, &err, expr_ctx, exec_ctx))
                    .transpose()?
                    .unwrap_or(false);
                let attempts_left = catch
                    .retry
                    .as_ref()
                    .and_then(|r| r.attempt_count)
                    .map(|limit| attempt < limit)
                    .unwrap_or(false);

                if retryable && attempts_left {
                    attempt += 1;
                    let delay = compute_delay(inner, catch.retry.as_ref().unwrap(), attempt);
                    wait_with_cancel(exec_ctx, delay).await?;
                    continue;
                }

                if let Some(do_tasks) = &catch.do_ {
                    let handler_output =
                        exec_scope(inner, workflow, do_tasks, exec_ctx, task_input, None).await?;
                    return Ok(handler_output);
                }

                return Ok(task_input.clone());
            }
        }
    }
}

fn set_error_var(exec_ctx: &mut ExecContext, catch: &CatchDef, err: &WorkflowError) {
    let name = catch.as_.clone().unwrap_or_else(|| "error".to_string());
    exec_ctx.loop_vars.insert(name, err.to_problem_json());
}

fn catch_matches(
    inner: &Arc<RuntimeInner>,
    catch: &CatchDef,
    err: &WorkflowError,
    expr_ctx: &ExpressionContext,
    exec_ctx: &ExecContext,
) -> Result<bool, WorkflowError> {
    if let Some(filter) = &catch.filter {
        for (key, expected) in filter {
            let actual = match key.as_str() {
                "type" => Value::String(err.problem.type_.clone()),
                "status" => json!(err.problem.status),
                "title" => Value::String(err.problem.title.clone()),
                "detail" => Value::String(err.problem.detail.clone().unwrap_or_default()),
                "instance" => Value::String(err.problem.instance.clone().unwrap_or_default()),
                other => err
                    .problem
                    .extensions
                    .as_ref()
                    .and_then(|e| e.get(other))
                    .cloned()
                    .unwrap_or(Value::Null),
            };
            if !values_equal(&actual, expected) {
                return Ok(false);
            }
        }
    }

    let mut vars = expr_ctx.as_variable_map();
    for (k, v) in &exec_ctx.loop_vars {
        vars.insert(k.clone(), v.clone());
    }

    if let Some(when) = &catch.when {
        if !is_truthy(&eval_expr(inner, when, &Value::Null, &vars, None)?) {
            return Ok(false);
        }
    }
    if let Some(except) = &catch.except_when {
        if is_truthy(&eval_expr(inner, except, &Value::Null, &vars, None)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn retryable(
    inner: &Arc<RuntimeInner>,
    retry: &RetryPolicyDef,
    _err: &WorkflowError,
    expr_ctx: &ExpressionContext,
    exec_ctx: &ExecContext,
) -> Result<bool, WorkflowError> {
    let mut vars = expr_ctx.as_variable_map();
    for (k, v) in &exec_ctx.loop_vars {
        vars.insert(k.clone(), v.clone());
    }
    if let Some(when) = &retry.when {
        if !is_truthy(&eval_expr(inner, when, &Value::Null, &vars, None)?) {
            return Ok(false);
        }
    }
    if let Some(except) = &retry.except_when {
        if is_truthy(&eval_expr(inner, except, &Value::Null, &vars, None)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn compute_delay(inner: &Arc<RuntimeInner>, retry: &RetryPolicyDef, attempt: usize) -> Duration {
    let base = retry.delay.unwrap_or_default();
    let mut delay = match retry.backoff {
        Backoff::Constant => base,
        Backoff::Linear => base.saturating_mul(attempt as u32),
        Backoff::Exponential => {
            let factor = 2u32.saturating_pow((attempt as u32).saturating_sub(1));
            base.saturating_mul(factor)
        }
    };
    if let Some((from, to)) = retry.jitter {
        let f = inner.random.next_f64();
        let jitter_range = to.saturating_sub(from);
        let jitter = from.saturating_add(Duration::from_millis(
            (jitter_range.as_millis() as f64 * f) as u64,
        ));
        delay = delay.saturating_add(jitter);
    }
    delay
}

async fn wait_with_cancel(exec_ctx: &ExecContext, duration: Duration) -> Result<(), WorkflowError> {
    if exec_ctx.cancel.is_cancelled() {
        return Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()));
    }
    let sleep = exec_ctx.inner.clock.sleep(duration);
    tokio::pin!(sleep);
    tokio::select! {
        _ = &mut sleep => Ok(()),
        _ = exec_ctx.cancel.notified() => {
            Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()))
        }
    }
}

/// The `emit` task.
async fn task_emit(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    def: &EmitDef,
    expr_ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    let mut event = ows_runtime_events::CloudEvent::new(
        "",
        format!(
            "/{}/{}/{}",
            workflow.id.namespace, workflow.id.name, workflow.id.version
        ),
        "",
    );

    for (key, value) in &def.event {
        let resolved = deep_interpolate(inner, value, expr_ctx)?;
        match key.as_str() {
            "id" => event.id = resolved.as_str().unwrap_or_default().to_string(),
            "source" => event.source = resolved.as_str().unwrap_or_default().to_string(),
            "type" => event.type_ = resolved.as_str().unwrap_or_default().to_string(),
            "time" => event.time = resolved.as_str().map(|s| s.to_string()),
            "subject" => event.subject = resolved.as_str().map(|s| s.to_string()),
            "datacontenttype" => event.data_content_type = resolved.as_str().map(|s| s.to_string()),
            "dataschema" => event.data_schema = resolved.as_str().map(|s| s.to_string()),
            "data" => event.data = Some(resolved),
            other => {
                event.extensions.insert(other.to_string(), resolved);
            }
        }
    }

    if event.id.is_empty() {
        event.id = inner.uuid.new_v4().to_string();
    }
    if event.type_.is_empty() {
        return Err(crate::error::semantic_error(
            "emit task requires an event `type`",
        ));
    }
    if event.time.is_none() {
        event.time = Some(crate::engine::iso_time(inner.clock.epoch_seconds()));
    }

    inner.publisher.publish(&event.to_message()).await?;

    Ok(serde_json::to_value(&event).unwrap_or(Value::Null))
}

/// The `listen` task.
async fn task_listen(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    def: &ListenDef,
    expr_ctx: &ExpressionContext,
    exec_ctx: &mut ExecContext,
) -> Result<Value, WorkflowError> {
    let read = def.read.as_deref().unwrap_or("data");

    // Determine the filters and how many events the strategy requires.
    let (filters, need) = match &def.to {
        ListenTo::One(filter) => (
            vec![build_filter(inner, filter, expr_ctx)?],
            ListenNeed::One,
        ),
        ListenTo::Any(filters) => {
            let m = build_filters(inner, filters, expr_ctx)?;
            (m, ListenNeed::One)
        }
        ListenTo::All(filters) => {
            let m = build_filters(inner, filters, expr_ctx)?;
            let n = m.len();
            (m, ListenNeed::All(n))
        }
    };

    // Correlation `expect` expressions are evaluated against the workflow
    // context, so the variable map is fixed at listen start.
    let vars = expr_ctx.as_variable_map();

    // Subscribe to the consumer and filter/consume in the gather loop below.
    // An accept-all filter keeps a single code path for `one`, `any` and `all`
    // (and the in-memory broker broadcasts matching events regardless).
    let subscription = inner
        .consumer
        .subscribe(Box::new(|_e: &EventMessage| true))
        .await?;

    // Consume the events required by the strategy.
    let consumed = gather_listen_events(&filters, &vars, need, subscription, exec_ctx).await?;

    let read_values: Vec<Value> = consumed
        .iter()
        .map(|event| read_event(event, read))
        .collect();

    // Apply the optional `foreach` iterator over the consumed events.
    if let Some(fe) = &def.foreach {
        return foreach_listen(inner, workflow, fe, read_values, exec_ctx).await;
    }

    Ok(Value::Array(read_values))
}

/// How many events a listen strategy consumes before it may proceed.
enum ListenNeed {
    /// Consume exactly one matching event.
    One,
    /// Consume events until every one of the `usize` filters has matched.
    All(usize),
}

/// Consumes events from a subscription according to the strategy.
///
/// Correlation keys are grouped across events: with `all`, once a first value
/// is observed for a key it becomes the expected value for that key (unless the
/// filter declares an explicit `expect`), so all filters must agree on the
/// correlation key values. This is the "cross-event grouping" the spec
/// describes.
async fn gather_listen_events(
    filters: &[CompiledListenFilter],
    vars: &Map<String, Value>,
    need: ListenNeed,
    subscription: Arc<dyn ows_runtime_core::EventSubscription>,
    exec_ctx: &ExecContext,
) -> Result<Vec<EventMessage>, WorkflowError> {
    let mut consumed: Vec<EventMessage> = Vec::new();
    // Tracks which filter (by index) has already been satisfied for `all`.
    let mut satisfied: Vec<bool> = vec![false; filters.len()];
    // The first-seen correlation values for keys without an explicit `expect`.
    let mut group: Map<String, Value> = Map::new();

    loop {
        // One/Any: stop after the first matching event.
        if matches!(need, ListenNeed::One) && !consumed.is_empty() {
            break;
        }
        // All: stop once every filter has been satisfied (empty `all` resolves
        // immediately with no events).
        if let ListenNeed::All(n) = need {
            if n == 0 {
                break;
            }
            if satisfied.iter().all(|s| *s) {
                break;
            }
        }

        let recv = subscription.recv();
        tokio::pin!(recv);
        let event = tokio::select! {
            ev = recv => ev,
            _ = exec_ctx.cancel.notified() => {
                return Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()));
            }
        };

        let Some(event) = event else {
            break; // subscription closed
        };

        match need {
            ListenNeed::One => {
                // With no filters every event is accepted; otherwise the event
                // must satisfy at least one filter.
                let matched = filters.is_empty() || {
                    let mut trial = group.clone();
                    filters
                        .iter()
                        .any(|f| filter_matches(exec_ctx, f, &event, vars, &mut trial))
                };
                if matched {
                    consumed.push(event);
                }
            }
            ListenNeed::All(_) => {
                let mut advanced = false;
                for (i, f) in filters.iter().enumerate() {
                    if !satisfied[i] && filter_matches(exec_ctx, f, &event, vars, &mut group) {
                        satisfied[i] = true;
                        advanced = true;
                    }
                }
                if advanced {
                    consumed.push(event);
                }
            }
        }
    }

    Ok(consumed)
}

/// A compiled `listen` filter: `with` attribute constraints plus correlation.
struct CompiledListenFilter {
    /// The `with` attribute matcher.
    matcher: ows_runtime_events::EventMatcher,
    /// The correlation keys: `(from, expect)` expressions.
    correlate: Vec<(String, Option<String>)>,
}

/// Returns whether an event satisfies a filter's `with` constraints and
/// correlation keys. On success, any first-seen correlation values are committed
/// to `group`.
fn filter_matches(
    exec_ctx: &ExecContext,
    filter: &CompiledListenFilter,
    event: &EventMessage,
    vars: &Map<String, Value>,
    group: &mut Map<String, Value>,
) -> bool {
    let ce = ows_runtime_events::CloudEvent::from_message(event);
    if !filter.matcher.matches(&ce) {
        return false;
    }
    let mut trial = group.clone();
    for (from, expect) in &filter.correlate {
        let input = event.data.clone().unwrap_or(Value::Null);
        let got = match crate::engine::eval_expr(&exec_ctx.inner, from, &input, vars, None) {
            Ok(v) => v,
            Err(_) => return false,
        };
        match expect {
            Some(expect) => {
                let context = vars.get("context").cloned().unwrap_or(Value::Null);
                let want =
                    match crate::engine::eval_expr(&exec_ctx.inner, expect, &context, vars, None) {
                        Ok(v) => v,
                        Err(_) => return false,
                    };
                if !values_equal(&got, &want) {
                    return false;
                }
            }
            None => match trial.get(from) {
                Some(existing) => {
                    if !values_equal(&got, existing) {
                        return false;
                    }
                }
                None => {
                    trial.insert(from.clone(), got);
                }
            },
        }
    }
    *group = trial;
    true
}

/// Runs a listen `foreach` iterator body for each consumed event.
async fn foreach_listen(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    fe: &ListenForeachDef,
    items: Vec<Value>,
    exec_ctx: &mut ExecContext,
) -> Result<Value, WorkflowError> {
    let at_name = fe.at.clone().unwrap_or_else(|| "index".to_string());
    let mut outputs = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if exec_ctx.cancel.is_cancelled() {
            return Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()));
        }
        exec_ctx.loop_vars.insert(fe.item.clone(), item.clone());
        exec_ctx.loop_vars.insert(at_name.clone(), json!(i));

        let body_output = exec_scope(inner, workflow, &fe.body, exec_ctx, &item, None).await?;
        outputs.push(body_output);

        exec_ctx.loop_vars.remove(&fe.item);
        exec_ctx.loop_vars.remove(&at_name);

        if exec_ctx.ended.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }
    Ok(Value::Array(outputs))
}

/// The `raise` task.
fn task_raise(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    task: &CompiledTask,
    def: &RaiseDef,
    expr_ctx: &ExpressionContext,
    exec_ctx: &ExecContext,
) -> Result<WorkflowError, WorkflowError> {
    let _ = expr_ctx;
    let error = match &def.error {
        ErrorRef::Inline(e) => e.clone(),
        ErrorRef::Reference(name) => resolve_error_ref(inner, workflow, name)
            .ok_or_else(|| crate::error::semantic_error(format!("undefined error `{name}`")))?,
    };

    let mut problem = ProblemDetails::new(
        &error.type_,
        &error.title,
        error.status.as_u64().unwrap_or(500) as u16,
    );
    if let Some(detail) = &error.detail {
        problem.detail = Some(detail.clone());
    }
    problem.instance = Some(
        error
            .instance
            .clone()
            .unwrap_or_else(|| task.reference.clone()),
    );

    Err(WorkflowError::new(ErrorKind::Fault, problem).with_execution(exec_ctx.execution_id.clone()))
}

/// The `call` task.
async fn task_call(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    def: &CallDef,
    expr_ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    let args = interpolate_map(inner, &def.with, expr_ctx)?;

    let direct = inner.functions.read().unwrap().get(&def.call).cloned();
    if let Some(invoker) = direct {
        let req = FunctionRequest {
            name: def.call.clone(),
            args,
            context: expr_ctx,
        };
        return invoker.invoke(req).await;
    }

    if let Some(func) = resolve_function(workflow, &def.call) {
        let mut merged = func.with.clone().unwrap_or_default();
        for (k, v) in &args {
            merged.insert(k.clone(), v.clone());
        }
        let invoker = inner.functions.read().unwrap().get(&func.call).cloned();
        if let Some(invoker) = invoker {
            let req = FunctionRequest {
                name: func.call.clone(),
                args: merged,
                context: expr_ctx,
            };
            return invoker.invoke(req).await;
        }
        return Err(crate::error::semantic_error(format!(
            "reusable function `{}` references unknown function `{}`",
            def.call, func.call
        )));
    }

    Err(crate::error::semantic_error(format!(
        "unknown function `{}`",
        def.call
    )))
}

/// The `run` task.
async fn task_run(
    inner: &Arc<RuntimeInner>,
    def: &RunDef,
    task_input: &Value,
    expr_ctx: &ExpressionContext,
    exec_ctx: &mut ExecContext,
) -> Result<Value, WorkflowError> {
    if !def.await_ {
        return Ok(task_input.clone());
    }

    let result = match &def.process {
        RunProcess::Shell(shell) => {
            let command = interpolate_string(inner, &shell.command, expr_ctx)?;
            let args = interpolate_strings(inner, &shell.arguments, expr_ctx)?;
            let stdin = interpolate_opt_string(inner, shell.stdin.as_ref(), expr_ctx)?;
            let env = interpolate_env(inner, &shell.environment, expr_ctx)?;
            inner
                .process
                .run_shell(&command, &args, &env, stdin)
                .await?
        }
        RunProcess::Script(script) => {
            let code = interpolate_string(inner, &script.code, expr_ctx)?;
            let args = interpolate_strings(inner, &script.arguments, expr_ctx)?;
            let stdin = interpolate_opt_string(inner, script.stdin.as_ref(), expr_ctx)?;
            let env = interpolate_env(inner, &script.environment, expr_ctx)?;
            inner
                .process
                .run_script(&script.language, &code, &args, &env, stdin)
                .await?
        }
        RunProcess::Container(container) => {
            let args = interpolate_strings(inner, &container.arguments, expr_ctx)?;
            let stdin = interpolate_opt_string(inner, container.stdin.as_ref(), expr_ctx)?;
            let env = interpolate_env(inner, &container.environment, expr_ctx)?;
            inner
                .process
                .run_container(&container.image, &args, &env, stdin)
                .await?
        }
        RunProcess::Workflow(wf) => {
            let input = interpolate_value(inner, wf.input.as_ref(), expr_ctx)?;
            let key = format!("{}/{}/{}", wf.namespace, wf.name, wf.version);
            let sub = inner.workflow(&key).ok_or_else(|| {
                crate::error::runtime_error(format!("sub-workflow `{key}` is not registered"))
            })?;
            let sub_cancel = exec_ctx.cancel.clone();
            return crate::engine::execute(
                inner.clone(),
                sub,
                input.unwrap_or(Value::Null),
                sub_cancel,
            )
            .await;
        }
    };

    Ok(process_output(&result, def.return_))
}

fn process_output(result: &ows_runtime_core::ProcessResult, return_: RunReturn) -> Value {
    match return_ {
        RunReturn::Stdout => result
            .stdout
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
        RunReturn::Stderr => result
            .stderr
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
        RunReturn::Code => result.code.map(|c| json!(c)).unwrap_or(Value::Null),
        RunReturn::All => serde_json::to_value(result).unwrap_or(Value::Null),
        RunReturn::None => Value::Null,
    }
}

/// The `wait` task.
async fn task_wait(
    inner: &Arc<RuntimeInner>,
    def: &WaitDef,
    exec_ctx: &ExecContext,
) -> Result<Value, WorkflowError> {
    let _ = inner;
    if exec_ctx.cancel.is_cancelled() {
        return Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()));
    }
    let sleep = exec_ctx.inner.clock.sleep(def.duration);
    tokio::pin!(sleep);
    tokio::select! {
        _ = &mut sleep => Ok(Value::Null),
        _ = exec_ctx.cancel.notified() => {
            Err(WorkflowError::cancelled().with_execution(exec_ctx.execution_id.clone()))
        }
    }
}

// ---- helpers ----

fn resolve_error_ref(
    _inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    name: &str,
) -> Option<crate::dsl_models::ErrorDefinition> {
    workflow
        .components
        .as_ref()
        .and_then(|c| c.errors.as_ref())
        .and_then(|m| m.get(name))
        .cloned()
}

fn resolve_function(
    workflow: &Arc<CompiledWorkflow>,
    name: &str,
) -> Option<crate::dsl_models::CallTaskDefinition> {
    workflow
        .components
        .as_ref()
        .and_then(|c| c.functions.as_ref())
        .and_then(|m| m.get(name))
        .and_then(|task| match task {
            crate::dsl_models::TaskDefinition::Call(c) => Some(c.clone()),
            _ => None,
        })
}

fn interpolate_map(
    inner: &Arc<RuntimeInner>,
    map: &Option<HashMap<String, Value>>,
    expr_ctx: &ExpressionContext,
) -> Result<HashMap<String, Value>, WorkflowError> {
    let mut out = HashMap::new();
    if let Some(map) = map {
        for (k, v) in map {
            let resolved = eval_interpolation(inner, v, expr_ctx)?;
            out.insert(k.clone(), resolved);
        }
    }
    Ok(out)
}

fn interpolate_value(
    inner: &Arc<RuntimeInner>,
    value: Option<&Value>,
    expr_ctx: &ExpressionContext,
) -> Result<Option<Value>, WorkflowError> {
    match value {
        None => Ok(None),
        Some(v) => Ok(Some(eval_interpolation(inner, v, expr_ctx)?)),
    }
}

/// Recursively interpolates expressions within a JSON value.
fn deep_interpolate(
    inner: &Arc<RuntimeInner>,
    value: &Value,
    expr_ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), deep_interpolate(inner, v, expr_ctx)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(deep_interpolate(inner, item, expr_ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::String(_) => eval_interpolation(inner, value, expr_ctx),
        other => Ok(other.clone()),
    }
}

fn interpolate_string(
    inner: &Arc<RuntimeInner>,
    s: &str,
    expr_ctx: &ExpressionContext,
) -> Result<String, WorkflowError> {
    let v = eval_interpolation(inner, &Value::String(s.to_string()), expr_ctx)?;
    Ok(v.as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| v.to_string()))
}

fn interpolate_opt_string(
    inner: &Arc<RuntimeInner>,
    s: Option<&String>,
    expr_ctx: &ExpressionContext,
) -> Result<Option<String>, WorkflowError> {
    match s {
        Some(s) => Ok(Some(interpolate_string(inner, s, expr_ctx)?)),
        None => Ok(None),
    }
}

fn interpolate_strings(
    inner: &Arc<RuntimeInner>,
    list: &[String],
    expr_ctx: &ExpressionContext,
) -> Result<Vec<String>, WorkflowError> {
    let mut out = Vec::new();
    for s in list {
        out.push(interpolate_string(inner, s, expr_ctx)?);
    }
    Ok(out)
}

fn interpolate_env(
    inner: &Arc<RuntimeInner>,
    env: &HashMap<String, String>,
    expr_ctx: &ExpressionContext,
) -> Result<HashMap<String, String>, WorkflowError> {
    let mut out = HashMap::new();
    for (k, v) in env {
        out.insert(k.clone(), interpolate_string(inner, v, expr_ctx)?);
    }
    Ok(out)
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64().unwrap() == y.as_f64().unwrap(),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).map(|bv| values_equal(v, bv)).unwrap_or(false))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(xv, yv)| values_equal(xv, yv))
        }
        _ => a == b,
    }
}

/// Builds an event matcher from a filter's `with` constraints.
fn build_matcher(
    _inner: &Arc<RuntimeInner>,
    filter: &EventFilterDef,
    _expr_ctx: &ExpressionContext,
) -> Result<ows_runtime_events::EventMatcher, WorkflowError> {
    use ows_runtime_events::{EventMatcher, FieldConstraint};
    let mut matcher = EventMatcher::new();
    if let Some(t) = filter.with.get("type").and_then(|v| v.as_str()) {
        matcher.type_ = Some(t.to_string());
    }
    if let Some(s) = filter.with.get("source").and_then(|v| v.as_str()) {
        matcher.source = Some(s.to_string());
    }
    if let Some(s) = filter.with.get("subject").and_then(|v| v.as_str()) {
        matcher.subject = Some(s.to_string());
    }
    for (key, expected) in &filter.with {
        match key.as_str() {
            "type" | "source" | "subject" => {}
            _ => matcher
                .attributes
                .push((key.clone(), FieldConstraint::Equals(expected.clone()))),
        }
    }
    Ok(matcher)
}

/// Builds a compiled listen filter from an event filter definition.
fn build_filter(
    inner: &Arc<RuntimeInner>,
    filter: &EventFilterDef,
    expr_ctx: &ExpressionContext,
) -> Result<CompiledListenFilter, WorkflowError> {
    let matcher = build_matcher(inner, filter, expr_ctx)?;
    let correlate = filter
        .correlate
        .iter()
        .map(|c| (c.from.clone(), c.expect.clone()))
        .collect();
    Ok(CompiledListenFilter { matcher, correlate })
}

/// Builds compiled listen filters from a list of event filter definitions.
fn build_filters(
    inner: &Arc<RuntimeInner>,
    filters: &[EventFilterDef],
    expr_ctx: &ExpressionContext,
) -> Result<Vec<CompiledListenFilter>, WorkflowError> {
    filters
        .iter()
        .map(|f| build_filter(inner, f, expr_ctx))
        .collect()
}

fn read_event(event: &EventMessage, read: &str) -> Value {
    match read {
        "envelope" => serde_json::to_value(ows_runtime_events::CloudEvent::from_message(event))
            .unwrap_or(Value::Null),
        _ => event.data.clone().unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ows_runtime_core::ExpressionContext;
    use serde_json::json;

    fn inner() -> Arc<RuntimeInner> {
        crate::Runtime::builder().build().unwrap().inner
    }

    fn retry_policy(
        when: Option<&str>,
        except: Option<&str>,
        delay: Option<Duration>,
        backoff: Backoff,
    ) -> RetryPolicyDef {
        RetryPolicyDef {
            when: when.map(String::from),
            except_when: except.map(String::from),
            delay,
            backoff,
            jitter: None,
            attempt_count: None,
            attempt_duration: None,
            retry_duration: None,
        }
    }

    #[test]
    fn process_output_selection() {
        let r = ows_runtime_core::ProcessResult {
            code: Some(3),
            stdout: Some("out".into()),
            stderr: Some("err".into()),
        };
        assert_eq!(process_output(&r, RunReturn::Stdout), json!("out"));
        assert_eq!(process_output(&r, RunReturn::Stderr), json!("err"));
        assert_eq!(process_output(&r, RunReturn::Code), json!(3));
        assert_eq!(process_output(&r, RunReturn::None), Value::Null);
        let all = process_output(&r, RunReturn::All);
        assert_eq!(all["code"], json!(3));
    }

    #[test]
    fn read_event_modes() {
        let mut msg = ows_runtime_core::EventMessage::new("1", "src", "t");
        msg.data = Some(json!({"a":1}));
        assert_eq!(read_event(&msg, "data"), json!({"a":1}));
        assert_eq!(read_event(&msg, "raw"), json!({"a":1}));
        let env = read_event(&msg, "envelope");
        assert_eq!(env["type"], "t");
    }

    #[test]
    fn values_equal_semantics() {
        assert!(values_equal(&json!(1), &json!(1.0)));
        assert!(!values_equal(&json!([1]), &json!([2])));
        assert!(!values_equal(&json!({"a":1}), &json!({"b":1})));
    }

    #[test]
    fn compute_delay_backoff() {
        let inner = inner();
        let base = retry_policy(None, None, Some(Duration::from_secs(1)), Backoff::Constant);
        assert_eq!(compute_delay(&inner, &base, 1), Duration::from_secs(1));
        let lin = retry_policy(None, None, Some(Duration::from_secs(1)), Backoff::Linear);
        assert_eq!(compute_delay(&inner, &lin, 3), Duration::from_secs(3));
        let exp = retry_policy(
            None,
            None,
            Some(Duration::from_secs(1)),
            Backoff::Exponential,
        );
        assert_eq!(compute_delay(&inner, &exp, 3), Duration::from_secs(4));
    }

    #[test]
    fn deep_interpolate_nested() {
        let inner = inner();
        let ctx = ExpressionContext {
            input: json!({"name":"Bob"}),
            ..Default::default()
        };
        let v = json!({"greeting": "${ .name }", "list": ["${ .name }", 3]});
        let out = deep_interpolate(&inner, &v, &ctx).unwrap();
        assert_eq!(out, json!({"greeting":"Bob","list":["Bob",3]}));
    }

    #[test]
    fn build_matcher_type_and_data() {
        let filter = EventFilterDef {
            with: [
                ("type".to_string(), json!("t")),
                ("data".to_string(), json!({"a":1})),
            ]
            .into_iter()
            .collect(),
            correlate: vec![],
        };
        let m = build_matcher_filter(&filter);
        assert_eq!(m.type_.as_deref(), Some("t"));
        assert!(!m.attributes.is_empty());
    }

    fn build_matcher_filter(f: &EventFilterDef) -> ows_runtime_events::EventMatcher {
        // build_matcher needs inner + ctx; use a runtime.
        let rt = crate::Runtime::builder().build().unwrap();
        let ctx = ExpressionContext::default();
        build_matcher(&rt.inner, f, &ctx).unwrap()
    }

    #[test]
    fn retryable_when_and_except() {
        let inner = inner();
        let err = WorkflowError::new(
            ows_runtime_core::ErrorKind::Runtime,
            ows_runtime_core::ProblemDetails::standard(
                ows_runtime_core::StandardErrorType::Runtime,
            ),
        );
        let ctx = ExpressionContext::default();
        let ec = crate::engine::ExecContext {
            inner: inner.clone(),
            execution_id: "e".into(),
            context: Arc::new(tokio::sync::Mutex::new(Value::Null)),
            secrets: Default::default(),
            workflow_descriptor: Value::Null,
            runtime_descriptor: Value::Null,
            ended: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            timeout: None,
            cancel: Arc::new(crate::runtime::Cancellation::new()),
            loop_vars: Default::default(),
            apply_extensions: true,
            scope_exited: false,
        };
        let retry = retry_policy(Some("true"), None, None, Backoff::Constant);
        assert!(retryable(&inner, &retry, &err, &ctx, &ec).unwrap());
        let retry = retry_policy(None, Some("true"), None, Backoff::Constant);
        assert!(!retryable(&inner, &retry, &err, &ctx, &ec).unwrap());
    }

    #[test]
    fn catch_matches_filters() {
        let inner = inner();
        let err = WorkflowError::new(
            ows_runtime_core::ErrorKind::Runtime,
            ows_runtime_core::ProblemDetails::standard(
                ows_runtime_core::StandardErrorType::Communication,
            )
            .with_detail("d"),
        );
        let ctx = ExpressionContext::default();
        let ec = crate::engine::ExecContext {
            inner: inner.clone(),
            execution_id: "e".into(),
            context: Arc::new(tokio::sync::Mutex::new(Value::Null)),
            secrets: Default::default(),
            workflow_descriptor: Value::Null,
            runtime_descriptor: Value::Null,
            ended: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            timeout: None,
            cancel: Arc::new(crate::runtime::Cancellation::new()),
            loop_vars: Default::default(),
            apply_extensions: true,
            scope_exited: false,
        };
        let catch = CatchDef {
            filter: Some([("status".to_string(), json!(500))].into_iter().collect()),
            as_: None,
            when: None,
            except_when: None,
            retry: None,
            do_: None,
            then: None,
        };
        assert!(catch_matches(&inner, &catch, &err, &ctx, &ec).unwrap());
        let catch2 = CatchDef {
            filter: Some([("status".to_string(), json!(404))].into_iter().collect()),
            as_: None,
            when: None,
            except_when: None,
            retry: None,
            do_: None,
            then: None,
        };
        assert!(!catch_matches(&inner, &catch2, &err, &ctx, &ec).unwrap());
    }

    #[test]
    fn set_error_var_and_interpolate() {
        let mut ec = crate::engine::ExecContext {
            inner: inner(),
            execution_id: "e".into(),
            context: Arc::new(tokio::sync::Mutex::new(Value::Null)),
            secrets: Default::default(),
            workflow_descriptor: Value::Null,
            runtime_descriptor: Value::Null,
            ended: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            timeout: None,
            cancel: Arc::new(crate::runtime::Cancellation::new()),
            loop_vars: Default::default(),
            apply_extensions: true,
            scope_exited: false,
        };
        let catch = CatchDef {
            filter: None,
            as_: Some("err".into()),
            when: None,
            except_when: None,
            retry: None,
            do_: None,
            then: None,
        };
        let err = WorkflowError::new(
            ows_runtime_core::ErrorKind::Fault,
            ows_runtime_core::ProblemDetails::standard(
                ows_runtime_core::StandardErrorType::Runtime,
            ),
        );
        set_error_var(&mut ec, &catch, &err);
        assert!(ec.loop_vars.contains_key("err"));
    }

    #[test]
    fn interpolate_helpers() {
        let inner = inner();
        let ctx = ExpressionContext {
            input: json!({"x":"val"}),
            ..Default::default()
        };
        assert_eq!(interpolate_string(&inner, "${ .x }", &ctx).unwrap(), "val");
        let list = interpolate_strings(&inner, &["a".into(), "${ .x }".into()], &ctx).unwrap();
        assert_eq!(list, vec!["a", "val"]);
        let env = interpolate_env(
            &inner,
            &[("K".to_string(), "${ .x }".to_string())]
                .into_iter()
                .collect(),
            &ctx,
        )
        .unwrap();
        assert_eq!(env["K"], "val");
    }

    #[test]
    fn compute_delay_with_jitter() {
        let inner = inner();
        let p = RetryPolicyDef {
            delay: Some(Duration::from_secs(1)),
            backoff: Backoff::Constant,
            jitter: Some((Duration::ZERO, Duration::from_millis(10))),
            ..retry_policy(None, None, None, Backoff::Constant)
        };
        let d = compute_delay(&inner, &p, 1);
        assert!(
            d >= Duration::from_secs(1) && d <= Duration::from_secs(1) + Duration::from_millis(10)
        );
    }

    #[test]
    fn resolve_references() {
        let wf = Arc::new(crate::ir::CompiledWorkflow {
            id: crate::workflow_id(
                &ows_runtime_dsl::from_yaml(
                    "document: { dsl: '1.0.3', namespace: n, name: w, version: '1' }
do:
  - a: { set: { x: 1 } }
",
                )
                .unwrap(),
            ),
            definition: ows_runtime_dsl::from_yaml(
                "document: { dsl: '1.0.3', namespace: n, name: w, version: '1' }
do:
  - a: { set: { x: 1 } }
",
            )
            .unwrap(),
            tasks: vec![],
            task_index: Default::default(),
            input: Default::default(),
            output: Default::default(),
            timeout: None,
            evaluate: crate::ir::EvalConfig::default(),
            secrets: vec![],
            components: Some(
                serverless_workflow_core::models::workflow::ComponentDefinitionCollection {
                    errors: Some(
                        [(
                            "myErr".into(),
                            crate::dsl_models::ErrorDefinition::new(
                                "https://x/e",
                                "E",
                                json!(400),
                                None,
                                None,
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    functions: Some(
                        [(
                            "fn".into(),
                            crate::dsl_models::TaskDefinition::Call(
                                crate::dsl_models::CallTaskDefinition::new("http", None, None),
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    ..Default::default()
                },
            ),
            extensions: vec![],
            schedule: None,
        });
        let inner = inner();
        assert!(resolve_error_ref(&inner, &wf, "myErr").is_some());
        assert!(resolve_error_ref(&inner, &wf, "nope").is_none());
        assert!(resolve_function(&wf, "fn").is_some());
        assert!(resolve_function(&wf, "nope").is_none());
    }
}
