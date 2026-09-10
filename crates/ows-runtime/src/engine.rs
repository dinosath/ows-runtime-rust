//! The execution engine: a scope-based state machine for OWS workflows.
//!
//! Execution is driven by an explicit event loop over task scopes. Each task
//! transitions through the OWS data-flow pipeline (validate → transform input →
//! execute → transform output → validate → export → validate context) and emits
//! structured tracing events and lifecycle cloud events. The engine supports
//! cancellation, timeouts and retries.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ows_runtime_core::{
    ErrorKind, ExecutionRecord, ExpressionContext, Phase, ProblemDetails, StandardErrorType,
    StoredEvent, WorkflowError,
};
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use crate::ir::*;
use crate::runtime::{Cancellation, RuntimeInner};
use crate::tasks;

/// A flow directive produced by a task.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FlowDirective {
    /// Continue with the next task in scope.
    Continue,
    /// End the current scope.
    Exit,
    /// End the entire workflow.
    End,
    /// Jump to a named task in the current scope.
    Goto(String),
}

/// Parses a flow directive string.
pub(crate) fn parse_directive(s: &str) -> FlowDirective {
    match s {
        "continue" => FlowDirective::Continue,
        "exit" => FlowDirective::Exit,
        "end" => FlowDirective::End,
        other => FlowDirective::Goto(other.to_string()),
    }
}

/// The result of executing a single task.
pub(crate) struct TaskExec {
    /// The task's transformed output.
    pub output: Value,
    /// An optional directive decided by the task itself (e.g. switch).
    pub directive: Option<FlowDirective>,
}

/// The per-execution context. Owned so it can be cloned for structured
/// concurrency (fork branches) while sharing mutable state via `Arc`.
#[derive(Clone)]
pub(crate) struct ExecContext {
    pub inner: Arc<RuntimeInner>,
    pub execution_id: String,
    /// The workflow context data (`$context`), shared across branches.
    pub context: Arc<Mutex<Value>>,
    /// The resolved secrets.
    pub secrets: HashMap<String, String>,
    /// The workflow descriptor.
    pub workflow_descriptor: Value,
    /// The runtime descriptor.
    pub runtime_descriptor: Value,
    /// Set to true when an `end` directive is encountered.
    pub ended: Arc<AtomicBool>,
    /// The workflow timeout.
    pub timeout: Option<Duration>,
    /// The cancellation token.
    pub cancel: Arc<Cancellation>,
    /// Loop variables currently in scope (e.g. `$each`, `$index`).
    pub loop_vars: Map<String, Value>,
    /// Whether workflow extensions apply. Disabled while running extension
    /// tasks themselves so extensions never recurse infinitely.
    pub apply_extensions: bool,
    /// Set when the current scope exits via an `exit` flow directive. Used by
    /// extensions to short-circuit the extended task.
    pub scope_exited: bool,
}

impl ExecContext {
    /// Returns the current workflow context value.
    pub async fn context_value(&self) -> Value {
        self.context.lock().await.clone()
    }

    /// Sets the workflow context value.
    pub async fn set_context(&self, value: Value) {
        *self.context.lock().await = value;
    }

    /// Builds an expression variable map for the given task state.
    pub(crate) fn variables(
        &self,
        input: &Value,
        output: &Value,
        task: Option<&Value>,
    ) -> Map<String, Value> {
        let ctx = ExpressionContext {
            context: self.context_value_sync(),
            input: input.clone(),
            output: output.clone(),
            secrets: self.secrets.clone(),
            authorization: None,
            task: task.cloned(),
            workflow: Some(self.workflow_descriptor.clone()),
            runtime: Some(self.runtime_descriptor.clone()),
            extra_vars: self.loop_vars.clone(),
        };
        let mut map = ctx.as_variable_map();
        for (k, v) in &self.loop_vars {
            map.insert(k.clone(), v.clone());
        }
        map
    }

    fn context_value_sync(&self) -> Value {
        match self.context.try_lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Value::Null,
        }
    }
}

/// Options controlling an execution.
#[derive(Default)]
pub(crate) struct ExecutionOptions {
    /// A stable execution id to reuse when resuming.
    pub execution_id: Option<String>,
    /// Resume state, if resuming a partially executed workflow.
    pub resume: Option<ResumeState>,
}

/// Resume state loaded from a durable checkpoint.
#[derive(Debug, Clone)]
pub(crate) struct ResumeState {
    /// The top-level task index to resume from.
    pub next_index: usize,
    /// The input to feed the resumed scope.
    pub input: Value,
    /// The workflow context at the checkpoint.
    pub context: Value,
}

/// Top-level scope checkpoint configuration.
pub(crate) struct ScopeCheckpoint {
    /// The index of the first task to execute.
    pub start_index: usize,
    /// The execution start time.
    pub started_at: i64,
}

/// Executes a compiled workflow with the given raw input.
pub(crate) async fn execute(
    inner: Arc<RuntimeInner>,
    workflow: Arc<CompiledWorkflow>,
    raw_input: Value,
    cancel: Arc<Cancellation>,
) -> Result<Value, WorkflowError> {
    execute_with(
        inner,
        workflow,
        raw_input,
        cancel,
        ExecutionOptions::default(),
    )
    .await
}

/// Executes a compiled workflow, optionally reusing an execution id and
/// resuming from a durable checkpoint.
pub(crate) async fn execute_with(
    inner: Arc<RuntimeInner>,
    workflow: Arc<CompiledWorkflow>,
    raw_input: Value,
    cancel: Arc<Cancellation>,
    options: ExecutionOptions,
) -> Result<Value, WorkflowError> {
    let execution_id = options
        .execution_id
        .clone()
        .unwrap_or_else(|| inner.uuid.new_v4().to_string());
    let started_at = inner.clock.epoch_seconds();

    let workflow_descriptor = json!({
        "id": execution_id,
        "definition": serde_json::to_value(&workflow.definition).unwrap_or(Value::Null),
        "input": raw_input,
        "startedAt": started_at,
    });

    let secrets = resolve_workflow_secrets(&inner, &workflow);

    let mut ctx = ExecContext {
        inner: inner.clone(),
        execution_id: execution_id.clone(),
        context: Arc::new(Mutex::new(Value::Null)),
        secrets,
        workflow_descriptor,
        runtime_descriptor: inner.runtime_info.to_value(),
        ended: Arc::new(AtomicBool::new(false)),
        timeout: workflow.timeout,
        cancel,
        loop_vars: Map::new(),
        apply_extensions: true,
        scope_exited: false,
    };

    let wf_ref = ows_runtime_observability::WorkflowRef {
        namespace: workflow.id.namespace.clone(),
        name: workflow.id.name.clone(),
        version: workflow.id.version.clone(),
    };

    // 1. Determine the scope entry point. On a fresh run, emit the started
    //    lifecycle event, run the input pipeline and persist the start record.
    //    On resume, restore the checkpointed context and input.
    let (scope_input, start_index) = match &options.resume {
        Some(resume) => {
            tracing::info!(execution.id = %execution_id, "workflow.resumed");
            ctx.set_context(resume.context.clone()).await;
            (resume.input.clone(), resume.next_index)
        }
        None => {
            tracing::info!(execution.id = %execution_id, workflow.name = %workflow.id.name, workflow.namespace = %workflow.id.namespace, workflow.version = %workflow.id.version, "workflow.started");
            inner
                .lifecycle()
                .workflow_started(&wf_ref, &execution_id, &iso_time(started_at))
                .await
                .ok();
            let transformed = apply_workflow_input(&inner, &workflow, &ctx, raw_input).await?;
            ctx.set_context(transformed.clone()).await;
            persist_started(&inner, &workflow, &ctx, &execution_id, started_at).await;
            (transformed, 0)
        }
    };
    // 2. Execute the top-level scope, then transform + validate output. Faults
    //    are captured so the durable store records the terminal phase.
    let checkpoint = ScopeCheckpoint {
        start_index,
        started_at,
    };
    let outcome = async {
        let scope_output = exec_scope(
            &inner,
            &workflow,
            &workflow.tasks,
            &mut ctx,
            &scope_input,
            Some(&checkpoint),
        )
        .await?;
        apply_workflow_output(&inner, &workflow, &ctx, scope_output).await
    }
    .await;

    match &outcome {
        Ok(_) => {
            tracing::info!(execution.id = %execution_id, "workflow.completed");
            inner
                .lifecycle()
                .workflow_completed(
                    &wf_ref,
                    &execution_id,
                    &iso_time(inner.clock.epoch_seconds()),
                )
                .await
                .ok();
            persist_terminal(
                &inner,
                &workflow,
                &ctx,
                &execution_id,
                Phase::Completed,
                None,
                started_at,
            )
            .await;
        }
        Err(err) => {
            let phase = if err.kind == ErrorKind::Cancelled {
                Phase::Cancelled
            } else {
                Phase::Faulted
            };
            persist_terminal(
                &inner,
                &workflow,
                &ctx,
                &execution_id,
                phase,
                Some(err),
                started_at,
            )
            .await;
        }
    }

    outcome
}

/// Builds an [`ExecutionRecord`] snapshot of the current execution state.
fn execution_record(
    workflow: &Arc<CompiledWorkflow>,
    ctx: &ExecContext,
    execution_id: &str,
    phase: Phase,
    error: Option<Value>,
    started_at: i64,
) -> ExecutionRecord {
    ExecutionRecord {
        execution_id: execution_id.to_string(),
        workflow: workflow.id.key(),
        namespace: workflow.id.namespace.clone(),
        name: workflow.id.name.clone(),
        version: workflow.id.version.clone(),
        phase,
        context: ctx.context_value_sync(),
        pointer: Value::Null,
        error,
        started_at,
    }
}

/// Persists the execution's start record and a `workflow.started` event.
async fn persist_started(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    ctx: &ExecContext,
    execution_id: &str,
    started_at: i64,
) {
    let mut record = execution_record(
        workflow,
        ctx,
        execution_id,
        Phase::Running,
        None,
        started_at,
    );
    record.pointer = json!({ "next": 0, "input": ctx.context_value_sync() });
    if let Err(e) = inner.store.create(record).await {
        tracing::warn!(execution.id = %execution_id, error = %e, "failed to persist execution start");
    }
    let event = StoredEvent {
        execution_id: execution_id.to_string(),
        sequence: 1,
        event_type: "workflow.started".into(),
        payload: json!({ "startedAt": started_at }),
        timestamp: started_at,
    };
    if let Err(e) = inner.store.append_event(event).await {
        tracing::warn!(execution.id = %execution_id, error = %e, "failed to persist workflow.started event");
    }
}

/// Persists a top-level progress checkpoint (best-effort).
///
/// The record's `pointer` stores the index of the next top-level task and the
/// input to feed it, so [`crate::Runtime::resume`] can continue the workflow
/// from where it stopped.
async fn persist_checkpoint(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    ctx: &ExecContext,
    started_at: i64,
    next_index: usize,
    input: &Value,
) {
    let record = ExecutionRecord {
        execution_id: ctx.execution_id.clone(),
        workflow: workflow.id.key(),
        namespace: workflow.id.namespace.clone(),
        name: workflow.id.name.clone(),
        version: workflow.id.version.clone(),
        phase: Phase::Running,
        context: ctx.context_value_sync(),
        pointer: json!({ "next": next_index, "input": input }),
        error: None,
        started_at,
    };
    if let Err(e) = inner.store.update(&record).await {
        tracing::warn!(execution.id = %ctx.execution_id, error = %e, "failed to persist execution checkpoint");
    }
}

/// Persists the terminal phase of an execution and its terminal event.
async fn persist_terminal(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    ctx: &ExecContext,
    execution_id: &str,
    phase: Phase,
    error: Option<&WorkflowError>,
    started_at: i64,
) {
    let error_value = error.map(|e| e.to_problem_json());
    let record = execution_record(workflow, ctx, execution_id, phase, error_value, started_at);
    if let Err(e) = inner.store.update(&record).await {
        tracing::warn!(execution.id = %execution_id, error = %e, "failed to persist execution terminal phase");
    }

    let event_type = match phase {
        Phase::Completed => "workflow.completed",
        Phase::Cancelled => "workflow.cancelled",
        _ => "workflow.faulted",
    };
    let timestamp = inner.clock.epoch_seconds();
    let event = StoredEvent {
        execution_id: execution_id.to_string(),
        sequence: 2,
        event_type: event_type.into(),
        payload: error
            .map(|e| e.to_problem_json())
            .unwrap_or_else(|| json!({ "completedAt": timestamp })),
        timestamp,
    };
    if let Err(e) = inner.store.append_event(event).await {
        tracing::warn!(execution.id = %execution_id, error = %e, "failed to persist terminal event");
    }
}

/// Applies workflow `input.from` and validation.
async fn apply_workflow_input(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    ctx: &ExecContext,
    raw_input: Value,
) -> Result<Value, WorkflowError> {
    // Validate raw input against the schema.
    validate_schema(inner, &workflow.input.schema, &raw_input, "/input").await?;

    // Transform.
    match &workflow.input.transform {
        Some(src) => {
            let vars = ctx.variables(&raw_input, &Value::Null, None);
            let value = eval_expr(inner, src, &raw_input, &vars, None)?;
            Ok(value)
        }
        None => Ok(raw_input),
    }
}

/// Applies workflow `output.as` and validation.
async fn apply_workflow_output(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    ctx: &ExecContext,
    raw_output: Value,
) -> Result<Value, WorkflowError> {
    let transformed = match &workflow.output.transform {
        Some(src) => {
            let vars = ctx.variables(&Value::Null, &raw_output, None);
            eval_expr(inner, src, &raw_output, &vars, None)?
        }
        None => raw_output,
    };
    validate_schema(inner, &workflow.output.schema, &transformed, "/output").await?;
    Ok(transformed)
}

/// A boxed future used to break async recursion between engine functions.
pub(crate) type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Executes a scope of tasks, returning the scope's output value.
pub(crate) fn exec_scope<'a>(
    inner: &'a Arc<RuntimeInner>,
    workflow: &'a Arc<CompiledWorkflow>,
    tasks: &'a [CompiledTask],
    ctx: &'a mut ExecContext,
    initial_input: &'a Value,
    checkpoint: Option<&'a ScopeCheckpoint>,
) -> BoxFut<'a, Result<Value, WorkflowError>> {
    Box::pin(async move {
        let mut index: usize = checkpoint.map(|c| c.start_index).unwrap_or(0);
        let mut input = initial_input.clone();

        loop {
            if ctx.cancel.is_cancelled() {
                return Err(WorkflowError::cancelled().with_execution(ctx.execution_id.clone()));
            }
            if ctx.ended.load(Ordering::SeqCst) {
                return Ok(input);
            }
            if index >= tasks.len() {
                return Ok(input);
            }

            let task = &tasks[index];
            let task_timeout = task.timeout.or(ctx.timeout);

            let result = if let Some(dur) = task_timeout {
                match tokio::time::timeout(dur, exec_task(inner, workflow, task, &mut input, ctx))
                    .await
                {
                    Ok(res) => res,
                    Err(_elapsed) => {
                        let err = WorkflowError::timeout(Some(task.reference.clone()))
                            .with_execution(ctx.execution_id.clone());
                        return Err(err);
                    }
                }
            } else {
                exec_task(inner, workflow, task, &mut input, ctx).await
            };

            let exec = result?;

            // Determine the final directive.
            let directive = exec
                .directive
                .or_else(|| task.then.as_deref().map(parse_directive))
                .unwrap_or(FlowDirective::Continue);

            input = exec.output;

            match directive {
                FlowDirective::Continue => {
                    index += 1;
                }
                FlowDirective::Goto(name) => match tasks.iter().position(|t| t.name == name) {
                    Some(pos) => {
                        index = pos;
                    }
                    None => {
                        return Err(WorkflowError::new(
                            ErrorKind::Semantic,
                            ProblemDetails::standard(StandardErrorType::Runtime).with_detail(
                                format!("flow directive targets unknown task `{name}` in scope"),
                            ),
                        )
                        .with_execution(ctx.execution_id.clone()));
                    }
                },
                FlowDirective::Exit => {
                    ctx.scope_exited = true;
                    return Ok(input);
                }
                FlowDirective::End => {
                    ctx.ended.store(true, Ordering::SeqCst);
                    return Ok(input);
                }
            }

            // Persist progress after each task in the top-level scope so a
            // partially executed workflow can be resumed.
            if let Some(cp) = checkpoint {
                persist_checkpoint(inner, workflow, ctx, cp.started_at, index, &input).await;
            }
        }
    })
}

/// Executes a single task through the full OWS data-flow pipeline.
pub(crate) fn exec_task<'a>(
    inner: &'a Arc<RuntimeInner>,
    workflow: &'a Arc<CompiledWorkflow>,
    task: &'a CompiledTask,
    raw_input: &'a mut Value,
    ctx: &'a mut ExecContext,
) -> BoxFut<'a, Result<TaskExec, WorkflowError>> {
    Box::pin(async move {
        let task_descriptor = task.descriptor.clone();

        inner.record_task(&task.name);

        tracing::debug!(task.name = %task.name, "task.started");

        // 1. Evaluate `if` guard.
        if let Some(if_expr) = &task.if_ {
            let vars = ctx.variables(raw_input, &Value::Null, Some(&task_descriptor));
            let cond = eval_expr(inner, if_expr, raw_input, &vars, Some(task))?;
            if !is_truthy(&cond) {
                tracing::debug!(task.name = %task.name, "task.skipped");
                return Ok(TaskExec {
                    output: raw_input.clone(),
                    directive: None,
                });
            }
        }

        // 2. Validate task input.
        validate_schema(inner, &task.input.schema, raw_input, &task.reference).await?;

        // 3. Transform task input.
        let mut task_input = match &task.input.transform {
            Some(src) => {
                let vars = ctx.variables(raw_input, &Value::Null, Some(&task_descriptor));
                eval_expr(inner, src, raw_input, &vars, Some(task))?
            }
            None => raw_input.clone(),
        };

        // 3b. Apply matching extensions' `before` tasks. An extension task with
        //     `then: exit` short-circuits the extended task body.
        let extensions = applicable_extensions(inner, workflow, task, raw_input, ctx)?;
        let mut skip_body = false;
        for ext in &extensions {
            let outcome =
                run_extension_scope(inner, workflow, &ext.before, ctx, &task_input).await?;
            task_input = outcome.value;
            if outcome.exited {
                skip_body = true;
                break;
            }
        }

        // Build the expression context used by the task definition.
        let mut expr_ctx = ExpressionContext {
            context: ctx.context_value().await,
            input: task_input.clone(),
            output: Value::Null,
            secrets: ctx.secrets.clone(),
            authorization: None,
            task: Some(task_descriptor.clone()),
            workflow: Some(ctx.workflow_descriptor.clone()),
            runtime: Some(ctx.runtime_descriptor.clone()),
            extra_vars: ctx.loop_vars.clone(),
        };

        // 4. Execute the task body (or use the extension output when exited).
        let (raw_output, task_directive) = if skip_body {
            (task_input.clone(), None)
        } else {
            let dispatched =
                tasks::dispatch(inner, workflow, task, &task_input, &mut expr_ctx, ctx).await?;
            (dispatched.output, dispatched.directive)
        };

        tracing::debug!(task.name = %task.name, "task.completed");

        // 5. Transform task output.
        let vars_out = expr_ctx.as_variable_map_updated(&task_input, &raw_output);
        let mut transformed_output = match &task.output.transform {
            Some(src) => eval_expr(inner, src, &raw_output, &vars_out, Some(task))?,
            None => raw_output.clone(),
        };

        // 5b. Apply matching extensions' `after` tasks.
        for ext in &extensions {
            let outcome =
                run_extension_scope(inner, workflow, &ext.after, ctx, &transformed_output).await?;
            transformed_output = outcome.value;
        }

        // 6. Validate task output.
        validate_schema(
            inner,
            &task.output.schema,
            &transformed_output,
            &task.reference,
        )
        .await?;

        // 7. Update workflow context via export.as.
        let export_vars = expr_ctx.as_variable_map_updated(&task_input, &transformed_output);
        let new_context = match &task.export.transform {
            Some(src) => eval_expr(inner, src, &transformed_output, &export_vars, Some(task))?,
            None => ctx.context_value().await,
        };

        // 8. Validate exported context.
        validate_schema(inner, &task.export.schema, &new_context, &task.reference).await?;
        ctx.set_context(new_context).await;

        Ok(TaskExec {
            output: transformed_output,
            directive: task_directive,
        })
    })
}

/// Resolves the workflow's declared secrets through the configured resolver.
///
/// Only declared secret names are resolved. A declared secret that the resolver
/// cannot provide is omitted (and thus appears as `null` to expressions).
pub(crate) fn resolve_workflow_secrets(
    inner: &Arc<RuntimeInner>,
    workflow: &CompiledWorkflow,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for name in &workflow.secrets {
        if let Some(value) = inner.secrets.resolve(name) {
            out.insert(name.clone(), value);
        }
    }
    out
}

/// Selects the extensions that apply to a task, evaluating their `when` guards.
fn applicable_extensions(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    task: &CompiledTask,
    input: &Value,
    ctx: &ExecContext,
) -> Result<Vec<CompiledExtension>, WorkflowError> {
    if !ctx.apply_extensions || workflow.extensions.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for ext in &workflow.extensions {
        if !extension_targets(ext, task) {
            continue;
        }
        if let Some(when) = &ext.when {
            let vars = ctx.variables(input, &Value::Null, Some(&task.descriptor));
            let cond = eval_expr(inner, when, input, &vars, Some(task))?;
            if !is_truthy(&cond) {
                continue;
            }
        }
        out.push(ext.clone());
    }
    Ok(out)
}

/// Returns whether an extension targets the given task.
fn extension_targets(ext: &CompiledExtension, task: &CompiledTask) -> bool {
    if ext.extend == task.type_name() {
        return true;
    }
    // `call` tasks may additionally be extended by their function name.
    if let CompiledTaskKind::Call(call) = &task.kind {
        return ext.extend == call.call;
    }
    false
}

/// The result of running an extension `before`/`after` scope.
struct ExtensionOutcome {
    /// The scope's resulting value.
    value: Value,
    /// Whether the scope exited via an `exit` flow directive.
    exited: bool,
}

/// Runs an extension's `before`/`after` scope with extensions temporarily
/// disabled so an extension never extends itself.
async fn run_extension_scope(
    inner: &Arc<RuntimeInner>,
    workflow: &Arc<CompiledWorkflow>,
    scope: &[CompiledTask],
    ctx: &mut ExecContext,
    input: &Value,
) -> Result<ExtensionOutcome, WorkflowError> {
    if scope.is_empty() {
        return Ok(ExtensionOutcome {
            value: input.clone(),
            exited: false,
        });
    }
    let previous = ctx.apply_extensions;
    ctx.apply_extensions = false;
    ctx.scope_exited = false;
    let result = exec_scope(inner, workflow, scope, ctx, input, None).await;
    let exited = ctx.scope_exited;
    ctx.scope_exited = false;
    ctx.apply_extensions = previous;
    Ok(ExtensionOutcome {
        value: result?,
        exited,
    })
}

/// Evaluates an expression, converting errors to typed [`WorkflowError`]s.
pub(crate) fn eval_expr(
    inner: &Arc<RuntimeInner>,
    source: &str,
    input: &Value,
    vars: &Map<String, Value>,
    task: Option<&CompiledTask>,
) -> Result<Value, WorkflowError> {
    let compiled = inner
        .expression
        .compile(source)
        .map_err(|e| to_expression_error(e, task))?;
    inner
        .expression
        .evaluate(&compiled, input, vars)
        .map_err(|e| to_expression_error(e, task))
}

/// Evaluates a value that may contain `${...}` interpolation.
pub(crate) fn eval_interpolation(
    inner: &Arc<RuntimeInner>,
    value: &Value,
    ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    let vars = ctx.as_variable_map();
    match value {
        Value::String(s) => inner
            .expression
            .evaluate_interpolation(s, &ctx.input, &vars)
            .map_err(|e| to_expression_error(e, None)),
        other => Ok(other.clone()),
    }
}

fn to_expression_error(
    e: ows_runtime_core::ExpressionError,
    task: Option<&CompiledTask>,
) -> WorkflowError {
    let mut err = WorkflowError::new(
        ErrorKind::Expression,
        ProblemDetails::standard(StandardErrorType::Expression).with_detail(e.to_string()),
    );
    if let Some(t) = task {
        err.task = Some(t.reference.clone());
    }
    err
}

/// Validates a value against an optional schema.
pub(crate) async fn validate_schema(
    inner: &Arc<RuntimeInner>,
    schema: &Option<SchemaDef>,
    value: &Value,
    instance: &str,
) -> Result<(), WorkflowError> {
    let Some(schema) = schema else {
        return Ok(());
    };
    let Some(document) = &schema.document else {
        return Ok(());
    };
    match inner.validate(document, value) {
        Ok(()) => Ok(()),
        Err(errors) => {
            let detail = errors.join("; ");
            Err(WorkflowError::new(
                ErrorKind::Schema,
                ProblemDetails::standard(StandardErrorType::Validation)
                    .with_detail(detail)
                    .with_instance(instance),
            ))
        }
    }
}

/// jq-style truthiness: only `false` and `null` are falsy.
pub(crate) fn is_truthy(v: &Value) -> bool {
    !matches!(v, Value::Bool(false) | Value::Null)
}

/// Formats an epoch seconds value as an ISO 8601 UTC timestamp.
pub(crate) fn iso_time(epoch_seconds: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(epoch_seconds, 0)
        .single()
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_directives() {
        assert!(matches!(
            parse_directive("continue"),
            FlowDirective::Continue
        ));
        assert!(matches!(parse_directive("exit"), FlowDirective::Exit));
        assert!(matches!(parse_directive("end"), FlowDirective::End));
        assert!(matches!(parse_directive("someTask"), FlowDirective::Goto(s) if s == "someTask"));
    }

    #[test]
    fn truthiness_semantics() {
        assert!(is_truthy(&json!(0)));
        assert!(is_truthy(&json!("")));
        assert!(is_truthy(&json!([])));
        assert!(!is_truthy(&json!(false)));
        assert!(!is_truthy(&json!(null)));
        assert!(is_truthy(&json!(true)));
    }

    #[test]
    fn iso_time_formats() {
        assert_eq!(iso_time(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_time(1700000000), "2023-11-14T22:13:20Z");
    }
}
