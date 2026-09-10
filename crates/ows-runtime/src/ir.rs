//! Compiled, executable workflow IR.
//!
//! This is the optimized internal representation produced by the compiler. It
//! is designed for execution (not serialization) while preserving the exact
//! semantics of the original OWS definition.

use std::collections::HashMap;
use std::time::Duration;

use crate::dsl_models;
use crate::dsl_models::{Map, OneOfDurationOrIso8601Expression, TaskDefinition};

/// The evaluation mode for runtime expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalMode {
    Strict,
    Loose,
}

/// The evaluation configuration of a workflow.
#[derive(Debug, Clone)]
pub struct EvalConfig {
    /// The expression language (must be `jq` for the bundled engine).
    pub language: String,
    /// The evaluation mode.
    pub mode: EvalMode,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            language: "jq".into(),
            mode: EvalMode::Strict,
        }
    }
}

/// A data input/output/export definition.
#[derive(Debug, Clone, Default)]
pub struct DataDef {
    /// The schema to validate against, if any.
    pub schema: Option<SchemaDef>,
    /// The transformation expression source, if any.
    pub transform: Option<String>,
}

/// A schema definition.
#[derive(Debug, Clone)]
pub struct SchemaDef {
    /// The schema format (e.g. `json`).
    pub format: String,
    /// The inline schema document, if any.
    pub document: Option<serde_json::Value>,
    /// The external resource endpoint, if any.
    pub resource: Option<String>,
}

/// A compiled workflow definition.
#[derive(Debug, Clone)]
pub struct CompiledWorkflow {
    /// The workflow identity.
    pub id: crate::WorkflowId,
    /// The original SDK definition.
    pub definition: dsl_models::WorkflowDefinition,
    /// The ordered top level tasks.
    pub tasks: Vec<CompiledTask>,
    /// A lookup from task name to index (top-level scope).
    pub task_index: HashMap<String, usize>,
    /// The workflow input definition.
    pub input: DataDef,
    /// The workflow output definition.
    pub output: DataDef,
    /// The workflow timeout.
    pub timeout: Option<Duration>,
    /// The evaluation configuration.
    pub evaluate: EvalConfig,
    /// The declared secrets.
    pub secrets: Vec<String>,
    /// The workflow's reusable components.
    pub components: Option<dsl_models::ComponentDefinitionCollection>,
    /// The workflow's extensions (`use.extensions`).
    pub extensions: Vec<CompiledExtension>,
    /// The schedule, if any.
    pub schedule: Option<CompiledSchedule>,
}

/// A compiled workflow extension (`use.extensions`).
///
/// An extension contributes tasks that run `before` and/or `after` every task
/// whose type matches `extend`. The optional `when` guard is evaluated against
/// the extended task's input to decide whether the extension applies.
#[derive(Debug, Clone)]
pub struct CompiledExtension {
    /// The OWS task type to extend (e.g. `set`, `call`, `http`).
    pub extend: String,
    /// The runtime expression guard, if any.
    pub when: Option<String>,
    /// Tasks to run before the extended task.
    pub before: Vec<CompiledTask>,
    /// Tasks to run after the extended task.
    pub after: Vec<CompiledTask>,
}

/// A compiled schedule.
#[derive(Debug, Clone)]
pub struct CompiledSchedule {
    /// The schedule trigger.
    pub trigger: ows_runtime_core::ScheduleTrigger,
}

/// A compiled task within a scope.
#[derive(Debug, Clone)]
pub struct CompiledTask {
    /// The task name (unique within its scope).
    pub name: String,
    /// The task's JSON pointer reference (e.g. `/do/2/myTask`).
    pub reference: String,
    /// The runtime expression guard, if any.
    pub if_: Option<String>,
    /// The input definition.
    pub input: DataDef,
    /// The output definition.
    pub output: DataDef,
    /// The export definition.
    pub export: DataDef,
    /// The task timeout.
    pub timeout: Option<Duration>,
    /// The flow directive (`then`).
    pub then: Option<String>,
    /// The task's metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// The task descriptor exposed as `$task`.
    pub descriptor: serde_json::Value,
    /// The concrete task definition.
    pub kind: CompiledTaskKind,
}

impl CompiledTask {
    /// Returns the task type name.
    pub fn type_name(&self) -> &'static str {
        self.kind.type_name()
    }
}

/// The concrete kinds of compiled tasks.
#[derive(Debug, Clone)]
pub enum CompiledTaskKind {
    /// `set`
    Set(SetDef),
    /// `do`
    Do(Vec<CompiledTask>),
    /// `for`
    For(ForDef),
    /// `fork`
    Fork(ForkDef),
    /// `switch`
    Switch(Vec<SwitchCase>),
    /// `try`
    Try(TryDef),
    /// `emit`
    Emit(EmitDef),
    /// `listen`
    Listen(ListenDef),
    /// `raise`
    Raise(RaiseDef),
    /// `call`
    Call(CallDef),
    /// `run`
    Run(RunDef),
    /// `wait`
    Wait(WaitDef),
}

impl CompiledTaskKind {
    /// Returns the OWS task type name.
    pub fn type_name(&self) -> &'static str {
        match self {
            CompiledTaskKind::Set(_) => "set",
            CompiledTaskKind::Do(_) => "do",
            CompiledTaskKind::For(_) => "for",
            CompiledTaskKind::Fork(_) => "fork",
            CompiledTaskKind::Switch(_) => "switch",
            CompiledTaskKind::Try(_) => "try",
            CompiledTaskKind::Emit(_) => "emit",
            CompiledTaskKind::Listen(_) => "listen",
            CompiledTaskKind::Raise(_) => "raise",
            CompiledTaskKind::Call(_) => "call",
            CompiledTaskKind::Run(_) => "run",
            CompiledTaskKind::Wait(_) => "wait",
        }
    }
}

/// `set` task definition.
#[derive(Debug, Clone)]
pub struct SetDef {
    /// Either a map of values or a direct expression.
    pub values: SetValues,
}

/// The values to set.
#[derive(Debug, Clone)]
pub enum SetValues {
    /// A map of key → expression/literal.
    Map(Vec<(String, serde_json::Value)>),
    /// A direct expression.
    Expression(String),
}

/// `for` task definition.
#[derive(Debug, Clone)]
pub struct ForDef {
    /// The iteration variable name.
    pub each: String,
    /// The collection expression source.
    pub in_: String,
    /// The index variable name.
    pub at: Option<String>,
    /// The while condition expression.
    pub while_: Option<String>,
    /// The body scope.
    pub body: Vec<CompiledTask>,
}

/// `fork` task definition.
#[derive(Debug, Clone)]
pub struct ForkDef {
    /// The branches, in order.
    pub branches: Vec<CompiledTask>,
    /// Whether branches compete.
    pub compete: bool,
}

/// A switch case.
#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// The case name.
    pub name: String,
    /// The condition expression, if any (default case).
    pub when: Option<String>,
    /// The flow directive when the case matches.
    pub then: Option<String>,
}

/// `try` task definition.
#[derive(Debug, Clone)]
pub struct TryDef {
    /// The try body scope.
    pub body: Vec<CompiledTask>,
    /// The catch configuration.
    pub catch: CatchDef,
}

/// The catch configuration.
#[derive(Debug, Clone)]
pub struct CatchDef {
    /// The error filter (`errors.with`), if any.
    pub filter: Option<HashMap<String, serde_json::Value>>,
    /// The variable name to store the error as.
    pub as_: Option<String>,
    /// The `when` condition expression.
    pub when: Option<String>,
    /// The `exceptWhen` condition expression.
    pub except_when: Option<String>,
    /// The retry policy, if any.
    pub retry: Option<RetryPolicyDef>,
    /// The catch handler scope, if any.
    pub do_: Option<Vec<CompiledTask>>,
    /// The flow directive for the catch path, if any.
    pub then: Option<String>,
}

/// A retry policy.
#[derive(Debug, Clone)]
pub struct RetryPolicyDef {
    /// The `when` condition.
    pub when: Option<String>,
    /// The `exceptWhen` condition.
    pub except_when: Option<String>,
    /// The delay between attempts.
    pub delay: Option<Duration>,
    /// The backoff strategy.
    pub backoff: Backoff,
    /// The jitter range.
    pub jitter: Option<(Duration, Duration)>,
    /// The attempt count limit.
    pub attempt_count: Option<usize>,
    /// The attempt duration limit.
    pub attempt_duration: Option<Duration>,
    /// The overall retry duration limit.
    pub retry_duration: Option<Duration>,
}

/// A retry backoff strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backoff {
    Constant,
    Linear,
    Exponential,
}

/// `emit` task definition.
#[derive(Debug, Clone)]
pub struct EmitDef {
    /// The event attributes map.
    pub event: HashMap<String, serde_json::Value>,
}

/// `listen` task definition.
#[derive(Debug, Clone)]
pub struct ListenDef {
    /// The consumption strategy.
    pub to: ListenTo,
    /// How events are read (`data`, `envelope`, `raw`).
    pub read: Option<String>,
    /// The optional `foreach` iterator applied to each consumed event.
    pub foreach: Option<ListenForeachDef>,
}

/// The `foreach` iterator on a `listen` task.
///
/// When present, each event consumed by the listen is processed by an
/// iteration scope (mirroring the `for` task's data-flow). The current item is
/// exposed as `$<item>` and its zero-based index as `$<at>`.
#[derive(Debug, Clone)]
pub struct ListenForeachDef {
    /// The variable name of the current consumed item.
    pub item: String,
    /// The optional variable name of the current item index.
    pub at: Option<String>,
    /// The iteration body scope.
    pub body: Vec<CompiledTask>,
}

/// The event consumption strategy for a listen task.
#[derive(Debug, Clone)]
pub enum ListenTo {
    /// Wait for all events.
    All(Vec<EventFilterDef>),
    /// Wait for any event.
    Any(Vec<EventFilterDef>),
    /// Wait for one event.
    One(EventFilterDef),
}

/// An event filter.
#[derive(Debug, Clone)]
pub struct EventFilterDef {
    /// The attribute constraints.
    pub with: HashMap<String, serde_json::Value>,
    /// The correlation keys.
    pub correlate: Vec<CorrelationDef>,
}

/// A correlation key.
#[derive(Debug, Clone)]
pub struct CorrelationDef {
    /// The extraction expression.
    pub from: String,
    /// The expected value expression.
    pub expect: Option<String>,
}

/// `raise` task definition.
#[derive(Debug, Clone)]
pub struct RaiseDef {
    /// The error to raise.
    pub error: ErrorRef,
}

/// The error to raise (inline or reference).
#[derive(Debug, Clone)]
pub enum ErrorRef {
    Inline(dsl_models::ErrorDefinition),
    Reference(String),
}

/// `call` task definition.
#[derive(Debug, Clone)]
pub struct CallDef {
    /// The function name to call.
    pub call: String,
    /// The call arguments.
    pub with: Option<HashMap<String, serde_json::Value>>,
}

/// `run` task definition.
#[derive(Debug, Clone)]
pub struct RunDef {
    /// The process to run.
    pub process: RunProcess,
    /// Whether to await the process.
    pub await_: bool,
    /// The output selection.
    pub return_: RunReturn,
}

/// The kind of process to run.
#[derive(Debug, Clone)]
pub enum RunProcess {
    Container(RunContainer),
    Script(RunScript),
    Shell(RunShell),
    Workflow(RunWorkflow),
}

/// A container process.
#[derive(Debug, Clone)]
pub struct RunContainer {
    /// The container image.
    pub image: String,
    /// The command to run.
    pub command: Option<String>,
    /// The arguments.
    pub arguments: Vec<String>,
    /// The environment.
    pub environment: HashMap<String, String>,
    /// The stdin expression.
    pub stdin: Option<String>,
    /// The container name expression.
    pub name: Option<String>,
}

/// A script process.
#[derive(Debug, Clone)]
pub struct RunScript {
    /// The language.
    pub language: String,
    /// The code.
    pub code: String,
    /// The arguments.
    pub arguments: Vec<String>,
    /// The environment.
    pub environment: HashMap<String, String>,
    /// The stdin expression.
    pub stdin: Option<String>,
}

/// A shell process.
#[derive(Debug, Clone)]
pub struct RunShell {
    /// The command.
    pub command: String,
    /// The arguments.
    pub arguments: Vec<String>,
    /// The environment.
    pub environment: HashMap<String, String>,
    /// The stdin expression.
    pub stdin: Option<String>,
}

/// A workflow process.
#[derive(Debug, Clone)]
pub struct RunWorkflow {
    /// The workflow namespace.
    pub namespace: String,
    /// The workflow name.
    pub name: String,
    /// The workflow version.
    pub version: String,
    /// The input.
    pub input: Option<serde_json::Value>,
}

/// The `return` selection for a run task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunReturn {
    Stdout,
    Stderr,
    Code,
    All,
    None,
}

/// `wait` task definition.
#[derive(Debug, Clone)]
pub struct WaitDef {
    /// The duration to wait.
    pub duration: Duration,
}

/// Resolves a duration-or-expression into an optional concrete [`Duration`].
pub fn resolve_duration(d: &OneOfDurationOrIso8601Expression) -> Result<Option<Duration>, String> {
    match d {
        OneOfDurationOrIso8601Expression::Duration(dur) => {
            Ok(Some(Duration::from_millis(dur.total_milliseconds())))
        }
        OneOfDurationOrIso8601Expression::Iso8601Expression(expr) => {
            parse_iso8601_duration(expr).map(Some)
        }
    }
}

/// Parses an ISO 8601 duration string (e.g. `PT30S`, `P1DT2H`).
pub fn parse_iso8601_duration(input: &str) -> Result<Duration, String> {
    let s = input.trim();
    if !s.starts_with('P') {
        return Err(format!("invalid ISO 8601 duration `{input}`"));
    }
    let body = &s[1..];
    // Split into date and time sections.
    let (date_part, time_part) = match body.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (body, None),
    };

    let mut total_nanos: u128 = 0;
    let mut in_number = false;
    let mut num = String::new();

    macro_rules! add {
        ($unit_nanos:expr) => {{
            let n: f64 = num
                .parse()
                .map_err(|_| format!("invalid ISO 8601 duration `{input}`"))?;
            total_nanos += (n * $unit_nanos as f64) as u128;
            num.clear();
            in_number = false;
        }};
    }

    for c in date_part.chars() {
        match c {
            '0'..='9' | '.' => {
                in_number = true;
                num.push(c);
            }
            'Y' => {
                if !in_number {
                    return Err(format!("invalid ISO 8601 duration `{input}`"));
                }
                add!(31_536_000_000_000_000u64);
            }
            'M' => {
                if !in_number {
                    return Err(format!("invalid ISO 8601 duration `{input}`"));
                }
                add!(2_592_000_000_000_000u64);
            }
            'W' => {
                if !in_number {
                    return Err(format!("invalid ISO 8601 duration `{input}`"));
                }
                add!(604_800_000_000_000u64);
            }
            'D' => {
                if !in_number {
                    return Err(format!("invalid ISO 8601 duration `{input}`"));
                }
                add!(86_400_000_000_000u64);
            }
            _ => return Err(format!("invalid ISO 8601 duration `{input}`")),
        }
    }

    if let Some(t) = time_part {
        let mut num = String::new();
        let mut in_number = false;
        for c in t.chars() {
            match c {
                '0'..='9' | '.' => {
                    in_number = true;
                    num.push(c);
                }
                'H' => {
                    if !in_number {
                        return Err(format!("invalid ISO 8601 duration `{input}`"));
                    }
                    let n: f64 = num
                        .parse()
                        .map_err(|_| format!("invalid ISO 8601 duration `{input}`"))?;
                    total_nanos += (n * 3600.0 * 1_000_000_000.0) as u128;
                    num.clear();
                    in_number = false;
                }
                'M' => {
                    if !in_number {
                        return Err(format!("invalid ISO 8601 duration `{input}`"));
                    }
                    let n: f64 = num
                        .parse()
                        .map_err(|_| format!("invalid ISO 8601 duration `{input}`"))?;
                    total_nanos += (n * 60.0 * 1_000_000_000.0) as u128;
                    num.clear();
                    in_number = false;
                }
                'S' => {
                    if !in_number {
                        return Err(format!("invalid ISO 8601 duration `{input}`"));
                    }
                    let n: f64 = num
                        .parse()
                        .map_err(|_| format!("invalid ISO 8601 duration `{input}`"))?;
                    total_nanos += (n * 1_000_000_000.0) as u128;
                    num.clear();
                    in_number = false;
                }
                _ => return Err(format!("invalid ISO 8601 duration `{input}`")),
            }
        }
    }

    Ok(Duration::from_nanos(total_nanos as u64))
}

/// Converts a scope `Map<String, TaskDefinition>` into an ordered vector of
/// `(name, TaskDefinition)` pairs, preserving declaration order.
pub fn scope_entries(map: &Map<String, TaskDefinition>) -> Vec<(String, TaskDefinition)> {
    map.entries
        .iter()
        .filter_map(|e| {
            e.iter()
                .next()
                .map(|(name, task)| (name.clone(), task.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso8601_durations() {
        assert_eq!(
            parse_iso8601_duration("PT30S").unwrap(),
            Duration::from_secs(30)
        );
        assert_eq!(
            parse_iso8601_duration("PT1M").unwrap(),
            Duration::from_secs(60)
        );
        assert_eq!(
            parse_iso8601_duration("PT1H30M").unwrap(),
            Duration::from_secs(90 * 60)
        );
        assert_eq!(
            parse_iso8601_duration("P1D").unwrap(),
            Duration::from_secs(24 * 3600)
        );
        assert_eq!(
            parse_iso8601_duration("P1DT2H").unwrap(),
            Duration::from_secs(26 * 3600)
        );
        assert_eq!(
            parse_iso8601_duration("PT0.5S").unwrap(),
            Duration::from_millis(500)
        );
        assert!(parse_iso8601_duration("not-a-duration").is_err());
        assert!(parse_iso8601_duration("PX").is_err());
    }

    #[test]
    fn resolve_duration_values() {
        let iso = OneOfDurationOrIso8601Expression::Iso8601Expression("PT10S".into());
        assert_eq!(
            resolve_duration(&iso).unwrap().unwrap(),
            Duration::from_secs(10)
        );
        let dur = OneOfDurationOrIso8601Expression::Duration(dsl_models::Duration::from_seconds(5));
        assert_eq!(
            resolve_duration(&dur).unwrap().unwrap(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn scope_entries_preserves_order() {
        use serverless_workflow_core::models::map::Map;
        let mut m = Map::new();
        m.add("b".to_string(), TaskDefinition::Set(Default::default()));
        m.add("a".to_string(), TaskDefinition::Set(Default::default()));
        let entries = scope_entries(&m);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "b");
        assert_eq!(entries[1].0, "a");
    }

    #[test]
    fn task_type_names() {
        let set = CompiledTaskKind::Set(SetDef {
            values: SetValues::Map(vec![]),
        });
        assert_eq!(set.type_name(), "set");
        assert_eq!(CompiledTaskKind::Do(vec![]).type_name(), "do");
        assert_eq!(
            CompiledTaskKind::Wait(WaitDef {
                duration: Duration::ZERO
            })
            .type_name(),
            "wait"
        );
        assert_eq!(
            CompiledTaskKind::Raise(RaiseDef {
                error: ErrorRef::Reference("e".into())
            })
            .type_name(),
            "raise"
        );
    }

    #[test]
    fn eval_mode_defaults() {
        assert_eq!(EvalConfig::default().language, "jq");
        assert!(matches!(EvalConfig::default().mode, EvalMode::Strict));
    }
}
