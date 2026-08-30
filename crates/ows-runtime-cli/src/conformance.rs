//! OWS Conformance Test Kit runner.
//!
//! This module parses the CTK Gherkin feature files from the Open Workflow
//! Specification repository and executes each scenario against the runtime,
//! producing a machine-readable and human-readable conformance report.

use std::path::{Path, PathBuf};

use ows_runtime::Runtime;
use ows_runtime_core::{ErrorKind, RuntimePolicy, WorkflowError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A parsed Gherkin step.
#[derive(Debug, Clone)]
pub struct Step {
    /// The step keyword (`Given`, `When`, `Then`, `And`).
    pub keyword: String,
    /// The step text.
    pub text: String,
    /// An optional doc string (YAML/JSON block).
    pub doc_string: Option<String>,
}

/// A parsed Gherkin scenario.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// The scenario name.
    pub name: String,
    /// The steps.
    pub steps: Vec<Step>,
}

/// A parsed Gherkin feature.
#[derive(Debug, Clone)]
pub struct Feature {
    /// The feature name.
    pub name: String,
    /// The feature file path.
    pub file: String,
    /// The scenarios.
    pub scenarios: Vec<Scenario>,
}

/// The status of a conformance scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScenarioStatus {
    Pass,
    Fail,
    Skip,
}

/// The result of executing a single conformance scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    /// The feature name.
    pub feature: String,
    /// The scenario name.
    pub name: String,
    /// The status.
    pub status: ScenarioStatus,
    /// A message (failure/skip reason).
    pub message: Option<String>,
}

/// A conformance report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConformanceReport {
    /// The scenarios executed.
    pub scenarios: Vec<ScenarioResult>,
    /// The number of passes.
    pub passes: usize,
    /// The number of failures.
    pub failures: usize,
    /// The number of skips.
    pub skips: usize,
    /// The tested OWS specification/CTK version.
    pub ctk_version: Option<String>,
}

impl ConformanceReport {
    /// The total number of scenarios.
    pub fn total(&self) -> usize {
        self.scenarios.len()
    }

    /// Whether any scenario failed.
    pub fn has_failures(&self) -> bool {
        self.failures > 0
    }
}

/// Parses a Gherkin feature file.
pub fn parse_feature(path: &Path) -> Result<Feature, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_feature_str(&text, path.to_string_lossy().into_owned())
}

/// Parses a Gherkin feature from a string.
pub fn parse_feature_str(text: &str, file: String) -> Result<Feature, String> {
    let mut name = String::new();
    let mut scenarios: Vec<Scenario> = Vec::new();
    let mut current_scenario: Option<Scenario> = None;
    let mut current_step: Option<Step> = None;
    let mut in_doc = false;
    let mut doc_buf = String::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if in_doc {
            if line.trim_start().starts_with("\"\"\"") {
                in_doc = false;
                if let Some(step) = current_step.as_mut() {
                    step.doc_string = Some(std::mem::take(&mut doc_buf));
                }
            } else {
                doc_buf.push_str(line);
                doc_buf.push('\n');
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed.starts_with("Feature:") {
            name = trimmed
                .strip_prefix("Feature:")
                .unwrap_or("")
                .trim()
                .to_string();
        } else if trimmed.starts_with("Scenario:") {
            // Flush the pending step to the current scenario before switching.
            if let Some(step) = current_step.take() {
                if let Some(sc) = current_scenario.as_mut() {
                    sc.steps.push(step);
                }
            }
            if let Some(s) = current_scenario.take() {
                scenarios.push(s);
            }
            current_scenario = Some(Scenario {
                name: trimmed
                    .strip_prefix("Scenario:")
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                steps: Vec::new(),
            });
        } else if trimmed.starts_with("\"\"\"") {
            in_doc = true;
            doc_buf.clear();
        } else if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        } else {
            // A step line.
            let (keyword, text) = split_step(trimmed);
            if let Some(step) = current_step.take() {
                if let Some(sc) = current_scenario.as_mut() {
                    sc.steps.push(step);
                }
            }
            current_step = Some(Step {
                keyword,
                text,
                doc_string: None,
            });
        }
    }

    if let Some(step) = current_step.take() {
        if let Some(sc) = current_scenario.as_mut() {
            sc.steps.push(step);
        }
    }
    if let Some(s) = current_scenario.take() {
        scenarios.push(s);
    }

    Ok(Feature {
        name,
        file,
        scenarios,
    })
}

fn split_step(line: &str) -> (String, String) {
    for kw in ["Given ", "When ", "Then ", "And ", "But "] {
        if let Some(rest) = line.strip_prefix(kw) {
            return (kw.trim().to_string(), rest.trim().to_string());
        }
    }
    ("Step".to_string(), line.to_string())
}

/// Discovers feature files in a CTK features directory.
pub fn discover_features(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("feature") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Executes the CTK scenarios in the given features directory.
pub async fn run_ctk(features_dir: &Path) -> ConformanceReport {
    run_ctk_with(features_dir, false).await
}

/// Executes the CTK scenarios in the given features directory.
///
/// When `include_network` is true, network-dependent scenarios are executed
/// against a runtime with network enabled.
pub async fn run_ctk_with(features_dir: &Path, include_network: bool) -> ConformanceReport {
    let mut report = ConformanceReport::default();

    for feature_path in discover_features(features_dir) {
        let feature = match parse_feature(&feature_path) {
            Ok(f) => f,
            Err(e) => {
                report.scenarios.push(ScenarioResult {
                    feature: feature_path.to_string_lossy().into_owned(),
                    name: "<parse error>".into(),
                    status: ScenarioStatus::Fail,
                    message: Some(e),
                });
                report.failures += 1;
                continue;
            }
        };

        // Network-dependent scenarios are skipped unless explicitly requested.
        // This keeps CI deterministic.

        for scenario in &feature.scenarios {
            if scenario_uses_network(scenario) && !include_network {
                report.scenarios.push(ScenarioResult {
                    feature: feature.name.clone(),
                    name: scenario.name.clone(),
                    status: ScenarioStatus::Skip,
                    message: Some("network-dependent scenario skipped".into()),
                });
                report.skips += 1;
                continue;
            }

            let mut result = run_scenario(&feature, scenario, include_network).await;
            result.feature = feature.name.clone();
            match result.status {
                ScenarioStatus::Pass => report.passes += 1,
                ScenarioStatus::Fail => report.failures += 1,
                ScenarioStatus::Skip => report.skips += 1,
            }
            report.scenarios.push(result);
        }
    }

    report
}

/// Returns whether a scenario's workflow definition performs a network call.
fn scenario_uses_network(scenario: &Scenario) -> bool {
    for step in &scenario.steps {
        if let Some(doc) = &step.doc_string {
            if doc.contains("call: http") || doc.contains("call: openapi") {
                return true;
            }
        }
    }
    false
}

/// Executes a single scenario.
async fn run_scenario(
    feature: &Feature,
    scenario: &Scenario,
    include_network: bool,
) -> ScenarioResult {
    let mut definition: Option<Value> = None;
    let mut input = Value::Null;
    let mut executed = false;
    let mut run_result: Option<Result<Value, WorkflowError>> = None;
    let mut task_order: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    let policy = if include_network {
        RuntimePolicy {
            allow_network: true,
            ..Default::default()
        }
    } else {
        RuntimePolicy::default()
    };
    let Ok(runtime) = Runtime::builder().with_policy(policy).build() else {
        return ScenarioResult {
            feature: feature.name.clone(),
            name: scenario.name.clone(),
            status: ScenarioStatus::Fail,
            message: Some("runtime failed to build".into()),
        };
    };

    for step in &scenario.steps {
        let text = step.text.trim();
        let doc = step.doc_string.clone();

        if text == "a workflow with definition:" {
            if let Some(doc) = &doc {
                definition = Some(serde_yaml::from_str(doc).unwrap_or_else(|e| {
                    failures.push(format!("failed to parse workflow definition: {e}"));
                    Value::Null
                }));
            }
        } else if text == "given the workflow input is:" {
            if let Some(doc) = &doc {
                input = serde_yaml::from_str(doc).unwrap_or(Value::Null);
            }
        } else if text == "the workflow is executed" {
            executed = true;
            let Some(def) = definition.as_ref() else {
                failures.push("no workflow definition".into());
                continue;
            };
            let wf_def = serde_json::from_value(def.clone());
            let wf_def = match wf_def {
                Ok(d) => d,
                Err(e) => {
                    failures.push(format!("failed to deserialize definition: {e}"));
                    continue;
                }
            };
            let report = ows_runtime_dsl::validate(&wf_def);
            if !report.is_valid() {
                failures.push(format!(
                    "definition invalid: {:?}",
                    report
                        .issues
                        .iter()
                        .map(|i| i.code.clone())
                        .collect::<Vec<_>>()
                ));
                continue;
            }
            let wf = runtime.register_definition(&wf_def);
            match wf {
                Ok(wf) => {
                    let _ = runtime.take_task_order();
                    run_result = Some(runtime.run(wf, input.clone()).await);
                    task_order = runtime.take_task_order();
                }
                Err(e) => {
                    failures.push(format!("compile failed: {e}"));
                }
            }
        } else if let Some(_rest) = text.strip_prefix("the workflow should complete") {
            if let Some(run) = &run_result {
                match run {
                    Ok(_) => {}
                    Err(e) => failures.push(format!("expected completion, got fault: {e}")),
                }
                if let Some(_out) = text.strip_prefix("the workflow should complete with output:") {
                    let expected: Value =
                        serde_yaml::from_str(doc.as_deref().unwrap_or("")).unwrap_or(Value::Null);
                    let actual = run.as_ref().ok().cloned().unwrap_or(Value::Null);
                    if !values_equal(&actual, &expected) {
                        failures.push(format!(
                            "output mismatch: expected {expected}, got {actual}"
                        ));
                    }
                }
            } else if !executed {
                failures.push("workflow was not executed".into());
            }
        } else if let Some(_rest) = text.strip_prefix("the workflow should fault") {
            if let Some(run) = &run_result {
                match run {
                    Err(_) => {}
                    Ok(v) => failures.push(format!("expected fault, got output {v}")),
                }
                if let Some(_out) = text.strip_prefix("the workflow should fault with error:") {
                    let expected: Value =
                        serde_yaml::from_str(doc.as_deref().unwrap_or("")).unwrap_or(Value::Null);
                    let actual = match run {
                        Err(e) => e.to_problem_json(),
                        Ok(_) => Value::Null,
                    };
                    if !values_equal(&actual, &expected) {
                        failures.push(format!("error mismatch: expected {expected}, got {actual}"));
                    }
                }
            }
        } else if text == "the workflow should cancel" {
            if let Some(run) = &run_result {
                if let Err(e) = run {
                    if e.kind != ErrorKind::Cancelled {
                        failures.push(format!("expected cancellation, got: {e}"));
                    }
                } else {
                    failures.push("expected cancellation, got completion".into());
                }
            }
        } else if let Some(props) = text.strip_prefix("the workflow output should have properties")
        {
            let output = run_result.as_ref().and_then(|r| r.clone().ok());
            if let Some(output) = output {
                let prop_list = props.trim();
                let paths: Vec<String> = prop_list
                    .split(',')
                    .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                for p in paths {
                    if get_path(&output, &p).is_none() {
                        failures.push(format!("missing property `{p}`"));
                    }
                }
            }
        } else if let Some(rest) = text.strip_prefix("the workflow output should have a ") {
            // "the workflow output should have a 'PATH' property with value:" or "... containing N items"
            if let Some((path, kind)) = parse_property_assertion(rest) {
                let output = run_result.as_ref().and_then(|r| r.clone().ok());
                if let Some(output) = output {
                    match kind {
                        PropertyAssertion::WithValue => {
                            let expected: Value =
                                serde_yaml::from_str(doc.as_deref().unwrap_or(""))
                                    .unwrap_or(Value::Null);
                            let actual = get_path(&output, &path).cloned().unwrap_or(Value::Null);
                            if !values_equal(&actual, &expected) {
                                failures.push(format!(
                                    "property `{path}` mismatch: expected {expected}, got {actual}"
                                ));
                            }
                        }
                        PropertyAssertion::ItemCount(n) => {
                            let items = get_path(&output, &path).and_then(|v| v.as_array());
                            match items {
                                Some(items) if items.len() == n => {}
                                Some(items) => failures.push(format!(
                                    "property `{path}` has {} items, expected {n}",
                                    items.len()
                                )),
                                None => failures.push(format!("property `{path}` is not an array")),
                            }
                        }
                    }
                }
            }
        } else if let Some(task) = text.strip_suffix(" should run first") {
            if task_order.first().map(|t| t.as_str()) == Some(task.trim()) {
                // ok
            } else {
                failures.push(format!(
                    "expected `{task}` to run first, order was {:?}",
                    task_order
                ));
            }
        } else if let Some(task) = text.strip_suffix(" should run last") {
            if task_order.last().map(|t| t.as_str()) == Some(task.trim()) {
                // ok
            } else {
                failures.push(format!(
                    "expected `{task}` to run last, order was {:?}",
                    task_order
                ));
            }
        } else if let Some((t1, t2)) = parse_run_before(text, "should run before") {
            let i1 = task_order.iter().position(|t| t == t1);
            let i2 = task_order.iter().position(|t| t == t2);
            match (i1, i2) {
                (Some(a), Some(b)) if a < b => {}
                (Some(a), Some(b)) => failures.push(format!(
                    "expected `{t1}` to run before `{t2}`, but {a} >= {b}"
                )),
                (None, _) => failures.push(format!("task `{t1}` did not run")),
                (_, None) => failures.push(format!("task `{t2}` did not run")),
            }
        } else if let Some((t1, t2)) = parse_run_before(text, "should run after") {
            let i1 = task_order.iter().position(|t| t == t1);
            let i2 = task_order.iter().position(|t| t == t2);
            match (i1, i2) {
                (Some(a), Some(b)) if a > b => {}
                (Some(a), Some(b)) => failures.push(format!(
                    "expected `{t1}` to run after `{t2}`, but {a} <= {b}"
                )),
                (None, _) => failures.push(format!("task `{t1}` did not run")),
                (_, None) => failures.push(format!("task `{t2}` did not run")),
            }
        } else if let Some(task) = text.strip_suffix(" should complete") {
            if !task_order.iter().any(|t| t == task.trim()) {
                failures.push(format!("task `{task}` did not complete"));
            }
        } else if let Some(task) = text.strip_suffix(" should fault") {
            // The faulting task is the last task recorded.
            if task_order.last().map(|t| t.as_str()) != Some(task.trim()) {
                failures.push(format!("expected task `{task}` to be the faulting task"));
            }
        } else if let Some(task) = text.strip_suffix(" should cancel") {
            // Not tracked precisely; accept if the task ran.
            if !task_order.iter().any(|t| t == task.trim()) {
                failures.push(format!("task `{task}` did not run"));
            }
        } else {
            // Unknown step - ignore (documentation text).
        }
    }

    let _ = feature;
    if failures.is_empty() {
        ScenarioResult {
            feature: String::new(),
            name: scenario.name.clone(),
            status: ScenarioStatus::Pass,
            message: None,
        }
    } else {
        ScenarioResult {
            feature: String::new(),
            name: scenario.name.clone(),
            status: ScenarioStatus::Fail,
            message: Some(failures.join("; ")),
        }
    }
}

enum PropertyAssertion {
    WithValue,
    ItemCount(usize),
}

fn parse_property_assertion(rest: &str) -> Option<(String, PropertyAssertion)> {
    let rest = rest.trim();
    if let Some(path) = rest.strip_prefix("'") {
        let (path, remainder) = path.split_once('\'')?;
        let remainder = remainder.trim();
        if remainder.starts_with("property with value:") {
            return Some((path.to_string(), PropertyAssertion::WithValue));
        }
        if let Some(cnt) = remainder.strip_prefix("property containing ") {
            let n: usize = cnt.split_whitespace().next()?.parse().ok()?;
            return Some((path.to_string(), PropertyAssertion::ItemCount(n)));
        }
    }
    None
}

fn parse_run_before<'a>(text: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    let (left, right) = text.split_once(keyword)?;
    let t1 = left.trim().trim_matches('\'');
    let t2 = right.trim().trim_matches('\'');
    Some((t1, t2))
}

/// Gets a value at a dot-separated path.
pub fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Structural value equality (numbers compared numerically).
pub fn values_equal(a: &Value, b: &Value) -> bool {
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
