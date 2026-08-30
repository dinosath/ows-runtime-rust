//! Semantic validation of parsed OWS workflow definitions.
//!
//! This pass runs after deserialization (which only guarantees the shape is
//! decodable) and before runtime compilation. It checks OWS semantic rules that
//! cannot be expressed in the schema, producing structured issues.

use serverless_workflow_core::models::map::Map;
use serverless_workflow_core::models::task::TaskDefinition;
use serverless_workflow_core::models::workflow::WorkflowDefinition;

use crate::models::task::ForTaskDefinition;

/// A single validation issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// A stable machine readable code for the issue.
    pub code: String,
    /// A human readable message.
    pub message: String,
    /// A JSON pointer to the offending component, if known.
    pub instance: Option<String>,
}

impl ValidationIssue {
    /// Creates a new validation issue.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            instance: None,
        }
    }

    /// Attaches an instance pointer.
    pub fn at(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }
}

/// The result of a validation pass.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// Issues found; empty means the definition is valid.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Whether the report has no issues.
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Validates a parsed workflow definition for OWS semantic correctness.
pub fn validate(def: &WorkflowDefinition) -> ValidationReport {
    let mut report = ValidationReport::default();

    // Document checks.
    if def.document.name.is_empty() {
        report.issues.push(
            ValidationIssue::new("document.name.required", "workflow name must not be empty")
                .at("/document/name"),
        );
    }
    if def.document.version.is_empty() {
        report.issues.push(
            ValidationIssue::new(
                "document.version.required",
                "workflow version must not be empty",
            )
            .at("/document/version"),
        );
    }
    if def.document.dsl.is_empty() {
        report.issues.push(
            ValidationIssue::new("document.dsl.required", "DSL version must not be empty")
                .at("/document/dsl"),
        );
    } else if !def.document.dsl.starts_with("1.") {
        report.issues.push(
            ValidationIssue::new(
                "document.dsl.unsupported",
                format!("unsupported DSL version `{}`", def.document.dsl),
            )
            .at("/document/dsl"),
        );
    }

    // The workflow must define at least one task.
    if def.do_.entries.is_empty() {
        report.issues.push(
            ValidationIssue::new("do.empty", "workflow must define at least one task").at("/do"),
        );
    }

    // Validate each top level task and its flow.
    validate_task_scope(&mut report, &def.do_, "/do");

    report
}

/// Recursively validates a task scope, checking uniqueness and flow targets.
fn validate_task_scope(
    report: &mut ValidationReport,
    tasks: &Map<String, TaskDefinition>,
    pointer: &str,
) {
    let names: Vec<&String> = tasks
        .entries
        .iter()
        .filter_map(|e| e.keys().next())
        .collect();
    // Flow targets are all names in this scope.
    let valid_targets: std::collections::HashSet<&String> = names.iter().copied().collect();

    for entry in &tasks.entries {
        let Some((name, task)) = entry.iter().next() else {
            continue;
        };
        let task_pointer = format!("{pointer}/{name}");

        // Validate `then` directive targets within scope.
        if let Some(then) = common_then(task) {
            match then.as_str() {
                "continue" | "exit" | "end" => {}
                other => {
                    if !valid_targets.contains(&other.to_string()) {
                        report.issues.push(
                            ValidationIssue::new(
                                "flow.unknown-target",
                                format!(
                                    "flow directive `{other}` does not match any task in scope"
                                ),
                            )
                            .at(format!("{task_pointer}/then")),
                        );
                    }
                }
            }
        }

        // Recurse into composite tasks.
        match task {
            TaskDefinition::Do(d) => {
                validate_task_scope(report, &d.do_, &format!("{task_pointer}/do"))
            }
            TaskDefinition::For(f) => validate_for_task(report, f, &task_pointer),
            TaskDefinition::Fork(fk) => {
                validate_task_scope(
                    report,
                    &fk.fork.branches,
                    &format!("{task_pointer}/fork/branches"),
                );
            }
            TaskDefinition::Try(t) => {
                validate_task_scope(report, &t.try_, &format!("{task_pointer}/try"));
                if let Some(do_tasks) = &t.catch.do_ {
                    validate_task_scope(report, do_tasks, &format!("{task_pointer}/catch/do"));
                }
            }
            TaskDefinition::Switch(s) => {
                validate_switch(report, s, &task_pointer);
            }
            _ => {}
        }
    }
}

fn validate_for_task(report: &mut ValidationReport, f: &ForTaskDefinition, pointer: &str) {
    if f.for_.in_.is_empty() {
        report.issues.push(
            ValidationIssue::new("for.in.required", "for loop must define an `in` collection")
                .at(format!("{pointer}/for/in")),
        );
    }
    validate_task_scope(report, &f.do_, &format!("{pointer}/do"));
}

fn validate_switch(
    report: &mut ValidationReport,
    s: &serverless_workflow_core::models::task::SwitchTaskDefinition,
    pointer: &str,
) {
    if s.switch.entries.is_empty() {
        report.issues.push(
            ValidationIssue::new("switch.empty", "switch must define at least one case")
                .at(pointer),
        );
        return;
    }
    let mut default_seen = false;
    for entry in &s.switch.entries {
        let Some((name, case)) = entry.iter().next() else {
            continue;
        };
        if case.when.is_none() {
            if default_seen {
                report.issues.push(
                    ValidationIssue::new(
                        "switch.multiple-default",
                        "switch may only have one default case",
                    )
                    .at(format!("{pointer}/{name}")),
                );
            }
            default_seen = true;
        }
        if case.then.is_none() {
            report.issues.push(
                ValidationIssue::new(
                    "switch.then.required",
                    "switch case must define a `then` directive",
                )
                .at(format!("{pointer}/{name}")),
            );
        }
    }
}

/// Returns the `then` flow directive of a task if present.
fn common_then(task: &TaskDefinition) -> Option<&String> {
    match task {
        TaskDefinition::Call(t) => t.common.then.as_ref(),
        TaskDefinition::Do(t) => t.common.then.as_ref(),
        TaskDefinition::Emit(t) => t.common.then.as_ref(),
        TaskDefinition::For(t) => t.common.then.as_ref(),
        TaskDefinition::Fork(t) => t.common.then.as_ref(),
        TaskDefinition::Listen(t) => t.common.then.as_ref(),
        TaskDefinition::Raise(t) => t.common.then.as_ref(),
        TaskDefinition::Run(t) => t.common.then.as_ref(),
        TaskDefinition::Set(t) => t.common.then.as_ref(),
        TaskDefinition::Switch(t) => t.common.then.as_ref(),
        TaskDefinition::Try(t) => t.common.then.as_ref(),
        TaskDefinition::Wait(t) => t.common.then.as_ref(),
    }
}
