//! Compilation of OWS definitions into executable IR.
//!
//! The compiler walks the validated [`serverless_workflow_core`] definition and
//! produces a [`CompiledWorkflow`]. Reference resolution (errors, retries,
//! timeouts) happens here against the workflow's reusable components.

use std::collections::HashMap;
use std::time::Duration;

use ows_runtime_core::WorkflowError;
use serverless_workflow_core::models::task::TaskDefinition;
use serverless_workflow_core::models::workflow::WorkflowDefinition;

use crate::dsl_models;
use crate::ir::*;

/// Compiles a validated workflow definition into executable IR.
pub fn compile(def: &WorkflowDefinition) -> Result<CompiledWorkflow, WorkflowError> {
    let components = def.use_.clone();
    let task_index = HashMap::new();

    let eval = match &def.evaluate {
        Some(e) => EvalConfig {
            language: e.language.clone(),
            mode: match e.mode.as_deref() {
                Some("loose") => EvalMode::Loose,
                _ => EvalMode::Strict,
            },
        },
        None => EvalConfig::default(),
    };

    let timeout = resolve_workflow_timeout(&def.timeout, components.as_ref())?;

    let tasks = compile_scope(&def.do_, "/do", components.as_ref())?;

    let schedule = def.schedule.as_ref().map(|s| CompiledSchedule {
        trigger: build_schedule_trigger(s),
    });

    Ok(CompiledWorkflow {
        id: crate::workflow_id(def),
        definition: def.clone(),
        tasks,
        task_index,
        input: compile_data(def.input.as_ref()),
        output: compile_output(def.output.as_ref()),
        timeout,
        evaluate: eval,
        secrets: components
            .as_ref()
            .and_then(|c| c.secrets.clone())
            .unwrap_or_default(),
        components,
        schedule,
    })
}

/// Compiles an ordered scope of tasks.
pub fn compile_scope(
    map: &dsl_models::Map<String, TaskDefinition>,
    base_reference: &str,
    components: Option<&dsl_models::ComponentDefinitionCollection>,
) -> Result<Vec<CompiledTask>, WorkflowError> {
    let entries = scope_entries(map);
    let mut out = Vec::new();
    for (index, (name, task)) in entries.into_iter().enumerate() {
        let reference = format!("{base_reference}/{index}/{name}");
        let compiled = compile_task(&name, &reference, &task, components)?;
        out.push(compiled);
    }
    Ok(out)
}

fn compile_task(
    name: &str,
    reference: &str,
    task: &TaskDefinition,
    components: Option<&dsl_models::ComponentDefinitionCollection>,
) -> Result<CompiledTask, WorkflowError> {
    let fields = common_fields(task);
    let kind = compile_kind(task, reference, components)?;

    let descriptor = task_to_value(task);

    Ok(CompiledTask {
        name: name.to_string(),
        reference: reference.to_string(),
        if_: fields.if_.clone(),
        input: compile_data(fields.input.as_ref()),
        output: compile_output(fields.output.as_ref()),
        export: compile_export(fields.export.as_ref()),
        timeout: resolve_timeout(fields.timeout.as_ref(), components)?,
        then: fields.then.clone(),
        metadata: fields.metadata.clone(),
        descriptor,
        kind,
    })
}

fn compile_kind(
    task: &TaskDefinition,
    reference: &str,
    components: Option<&dsl_models::ComponentDefinitionCollection>,
) -> Result<CompiledTaskKind, WorkflowError> {
    match task {
        TaskDefinition::Set(t) => Ok(CompiledTaskKind::Set(SetDef {
            values: match &t.set {
                dsl_models::SetValue::Map(m) => {
                    SetValues::Map(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                }
                dsl_models::SetValue::Expression(e) => SetValues::Expression(e.clone()),
            },
        })),
        TaskDefinition::Do(t) => Ok(CompiledTaskKind::Do(compile_scope(
            &t.do_,
            &format!("{reference}/do"),
            components,
        )?)),
        TaskDefinition::For(t) => Ok(CompiledTaskKind::For(ForDef {
            each: t.for_.each.clone(),
            in_: t.for_.in_.clone(),
            at: t.for_.at.clone(),
            while_: t.while_.clone(),
            body: compile_scope(&t.do_, &format!("{reference}/do"), components)?,
        })),
        TaskDefinition::Fork(t) => Ok(CompiledTaskKind::Fork(ForkDef {
            branches: compile_scope(
                &t.fork.branches,
                &format!("{reference}/fork/branches"),
                components,
            )?,
            compete: t.fork.compete,
        })),
        TaskDefinition::Switch(t) => {
            let mut cases = Vec::new();
            for e in &t.switch.entries {
                if let Some((case_name, case)) = e.iter().next() {
                    cases.push(SwitchCase {
                        name: case_name.clone(),
                        when: case.when.clone(),
                        then: case.then.clone(),
                    });
                }
            }
            Ok(CompiledTaskKind::Switch(cases))
        }
        TaskDefinition::Try(t) => {
            let catch = t.catch.clone();
            Ok(CompiledTaskKind::Try(TryDef {
                body: compile_scope(&t.try_, &format!("{reference}/try"), components)?,
                catch: CatchDef {
                    filter: catch.errors.as_ref().and_then(|e| e.with.clone()),
                    as_: catch.as_.clone(),
                    when: catch.when.clone(),
                    except_when: catch.except_when.clone(),
                    retry: resolve_retry(catch.retry.as_ref(), components)?,
                    do_: match &catch.do_ {
                        Some(d) => Some(compile_scope(
                            d,
                            &format!("{reference}/catch/do"),
                            components,
                        )?),
                        None => None,
                    },
                    then: None,
                },
            }))
        }
        TaskDefinition::Emit(t) => Ok(CompiledTaskKind::Emit(EmitDef {
            event: t.emit.event.with.clone(),
        })),
        TaskDefinition::Listen(t) => Ok(CompiledTaskKind::Listen(ListenDef {
            to: compile_listen_to(&t.listen.to),
            read: t.listen.read.clone(),
        })),
        TaskDefinition::Raise(t) => Ok(CompiledTaskKind::Raise(RaiseDef {
            error: match &t.raise.error {
                dsl_models::OneOfErrorDefinitionOrReference::Error(e) => {
                    ErrorRef::Inline(e.clone())
                }
                dsl_models::OneOfErrorDefinitionOrReference::Reference(r) => {
                    ErrorRef::Reference(r.clone())
                }
            },
        })),
        TaskDefinition::Call(t) => Ok(CompiledTaskKind::Call(CallDef {
            call: t.call.clone(),
            with: t.with.clone(),
        })),
        TaskDefinition::Run(t) => compile_run(&t.run),
        TaskDefinition::Wait(t) => {
            let dur = resolve_duration(&t.wait)
                .map_err(|msg| runtime_err("semantic", format!("wait: {msg}")))?;
            Ok(CompiledTaskKind::Wait(WaitDef {
                duration: dur.unwrap_or_default(),
            }))
        }
    }
}

fn compile_listen_to(s: &dsl_models::EventConsumptionStrategyDefinition) -> ListenTo {
    let filter = |f: &dsl_models::EventFilterDefinition| EventFilterDef {
        with: f.with.clone().unwrap_or_default(),
        correlate: f
            .correlate
            .as_ref()
            .map(|c| {
                c.values()
                    .map(|corr| CorrelationDef {
                        from: corr.from.clone(),
                        expect: corr.expect.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    if let Some(one) = &s.one {
        ListenTo::One(filter(one))
    } else if let Some(all) = &s.all {
        ListenTo::All(all.iter().map(filter).collect())
    } else if let Some(any) = &s.any {
        ListenTo::Any(any.iter().map(filter).collect())
    } else {
        // Default: any (all events).
        ListenTo::Any(Vec::new())
    }
}

fn compile_run(run: &dsl_models::ProcessTypeDefinition) -> Result<CompiledTaskKind, WorkflowError> {
    let await_ = run.await_.unwrap_or(true);
    let return_ = RunReturn::Stdout;
    let process = if let Some(c) = &run.container {
        RunProcess::Container(RunContainer {
            image: c.image.clone(),
            command: c.command.clone(),
            arguments: c.arguments.clone().unwrap_or_default(),
            environment: c.environment.clone().unwrap_or_default(),
            stdin: c.stdin.clone(),
            name: c.name.clone(),
        })
    } else if let Some(s) = &run.script {
        RunProcess::Script(RunScript {
            language: s.language.clone(),
            code: s.code.clone().unwrap_or_default(),
            arguments: s.arguments.clone().unwrap_or_default(),
            environment: s.environment.clone().unwrap_or_default(),
            stdin: s.stdin.clone(),
        })
    } else if let Some(s) = &run.shell {
        RunProcess::Shell(RunShell {
            command: s.command.clone(),
            arguments: s.arguments.clone().unwrap_or_default(),
            environment: s.environment.clone().unwrap_or_default(),
            stdin: None,
        })
    } else if let Some(w) = &run.workflow {
        RunProcess::Workflow(RunWorkflow {
            namespace: w.namespace.clone(),
            name: w.name.clone(),
            version: w.version.clone(),
            input: w.input.clone(),
        })
    } else {
        return Err(runtime_err(
            "semantic",
            "run task must define a process".to_string(),
        ));
    };
    Ok(CompiledTaskKind::Run(RunDef {
        process,
        await_,
        return_,
    }))
}

fn compile_data(input: Option<&dsl_models::InputDataModelDefinition>) -> DataDef {
    input
        .map(|i| DataDef {
            schema: i.schema.as_ref().map(compile_schema),
            transform: i.from.as_ref().and_then(value_to_string),
        })
        .unwrap_or_default()
}

fn compile_output(output: Option<&dsl_models::OutputDataModelDefinition>) -> DataDef {
    output
        .map(|o| DataDef {
            schema: o.schema.as_ref().map(compile_schema),
            transform: o.as_.as_ref().and_then(value_to_string),
        })
        .unwrap_or_default()
}

fn compile_export(output: Option<&dsl_models::OutputDataModelDefinition>) -> DataDef {
    compile_output(output)
}

fn compile_schema(s: &dsl_models::SchemaDefinition) -> SchemaDef {
    SchemaDef {
        format: s.format.clone(),
        document: s.document.clone(),
        resource: s.resource.as_ref().map(|r| match &r.endpoint {
            dsl_models::OneOfEndpointDefinitionOrUri::Uri(u) => u.clone(),
            dsl_models::OneOfEndpointDefinitionOrUri::Endpoint(e) => e.uri.clone(),
        }),
    }
}

fn value_to_string(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

fn common_fields(task: &TaskDefinition) -> &dsl_models::TaskDefinitionFields {
    match task {
        TaskDefinition::Call(t) => &t.common,
        TaskDefinition::Do(t) => &t.common,
        TaskDefinition::Emit(t) => &t.common,
        TaskDefinition::For(t) => &t.common,
        TaskDefinition::Fork(t) => &t.common,
        TaskDefinition::Listen(t) => &t.common,
        TaskDefinition::Raise(t) => &t.common,
        TaskDefinition::Run(t) => &t.common,
        TaskDefinition::Set(t) => &t.common,
        TaskDefinition::Switch(t) => &t.common,
        TaskDefinition::Try(t) => &t.common,
        TaskDefinition::Wait(t) => &t.common,
    }
}

fn resolve_timeout(
    timeout: Option<&dsl_models::OneOfTimeoutDefinitionOrReference>,
    components: Option<&dsl_models::ComponentDefinitionCollection>,
) -> Result<Option<Duration>, WorkflowError> {
    match timeout {
        Some(dsl_models::OneOfTimeoutDefinitionOrReference::Timeout(t)) => {
            resolve_duration(&t.after).map_err(|m| runtime_err("semantic", m))
        }
        Some(dsl_models::OneOfTimeoutDefinitionOrReference::Reference(name)) => {
            let comp = components
                .and_then(|c| c.timeouts.as_ref())
                .and_then(|m| m.get(name))
                .ok_or_else(|| runtime_err("semantic", format!("undefined timeout `{name}`")))?;
            resolve_duration(&comp.after).map_err(|m| runtime_err("semantic", m))
        }
        None => Ok(None),
    }
}

fn resolve_workflow_timeout(
    timeout: &Option<dsl_models::OneOfTimeoutDefinitionOrReference>,
    components: Option<&dsl_models::ComponentDefinitionCollection>,
) -> Result<Option<Duration>, WorkflowError> {
    resolve_timeout(timeout.as_ref(), components)
}

fn resolve_retry(
    retry: Option<&dsl_models::OneOfRetryPolicyDefinitionOrReference>,
    components: Option<&dsl_models::ComponentDefinitionCollection>,
) -> Result<Option<RetryPolicyDef>, WorkflowError> {
    let Some(retry) = retry else {
        return Ok(None);
    };
    let policy = match retry {
        dsl_models::OneOfRetryPolicyDefinitionOrReference::Retry(p) => p.clone(),
        dsl_models::OneOfRetryPolicyDefinitionOrReference::Reference(name) => components
            .and_then(|c| c.retries.as_ref())
            .and_then(|m| m.get(name))
            .cloned()
            .ok_or_else(|| runtime_err("semantic", format!("undefined retry policy `{name}`")))?,
    };

    let backoff = match &policy.backoff {
        Some(b) if b.exponential.is_some() => Backoff::Exponential,
        Some(b) if b.linear.is_some() => Backoff::Linear,
        _ => Backoff::Constant,
    };

    let delay = policy
        .delay
        .as_ref()
        .map(|d| Duration::from_millis(d.total_milliseconds()));
    let jitter = policy.jitter.as_ref().map(|j| {
        (
            Duration::from_millis(j.from.total_milliseconds()),
            Duration::from_millis(j.to.total_milliseconds()),
        )
    });

    Ok(Some(RetryPolicyDef {
        when: policy.when.clone(),
        except_when: policy.except_when.clone(),
        delay,
        backoff,
        jitter,
        attempt_count: policy
            .limit
            .as_ref()
            .and_then(|l| l.attempt.as_ref())
            .and_then(|a| a.count.map(|c| c as usize)),
        attempt_duration: policy
            .limit
            .as_ref()
            .and_then(|l| l.attempt.as_ref())
            .and_then(|a| a.duration.as_ref())
            .map(|d| Duration::from_millis(d.total_milliseconds())),
        retry_duration: policy
            .limit
            .as_ref()
            .and_then(|l| l.duration.as_ref())
            .map(|d| Duration::from_millis(d.total_milliseconds())),
    }))
}

fn build_schedule_trigger(
    s: &dsl_models::WorkflowScheduleDefinition,
) -> ows_runtime_core::ScheduleTrigger {
    use ows_runtime_core::ScheduleTrigger;
    if let Some(every) = &s.every {
        ScheduleTrigger::Every(Duration::from_millis(every.total_milliseconds()))
    } else if let Some(after) = &s.after {
        ScheduleTrigger::After(Duration::from_millis(after.total_milliseconds()))
    } else if let Some(cron) = &s.cron {
        ScheduleTrigger::Cron(cron.clone())
    } else {
        // Event-driven schedule.
        let events =
            s.on.as_ref()
                .map(|on| {
                    let mut out = Vec::new();
                    if let Some(one) = &on.one {
                        if let Some(with) = &one.with {
                            out.push(ows_runtime_core::EventMessageLike {
                                type_: with
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("*")
                                    .to_string(),
                                source: with
                                    .get("source")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                            });
                        }
                    }
                    if let Some(any) = &on.any {
                        for f in any {
                            if let Some(with) = &f.with {
                                out.push(ows_runtime_core::EventMessageLike {
                                    type_: with
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("*")
                                        .to_string(),
                                    source: with
                                        .get("source")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                });
                            }
                        }
                    }
                    out
                })
                .unwrap_or_default();
        ScheduleTrigger::OnEvents(events)
    }
}

fn task_to_value(task: &TaskDefinition) -> serde_json::Value {
    serde_json::to_value(task).unwrap_or(serde_json::Value::Null)
}

fn runtime_err(category: &str, msg: String) -> WorkflowError {
    let kind = match category {
        "semantic" => ows_runtime_core::ErrorKind::Semantic,
        _ => ows_runtime_core::ErrorKind::Runtime,
    };
    WorkflowError::new(
        kind,
        ows_runtime_core::ProblemDetails::standard(ows_runtime_core::StandardErrorType::Runtime)
            .with_detail(msg),
    )
}
