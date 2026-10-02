//! Acceptance tests exercising the public CLI (`ows-runtime` binary).
//!
//! These drive the real binary through its command line, covering `validate`,
//! `compile`, `run` and the `run` policy/secret/catalog flags.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ows-runtime")
}

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ows-cli-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin()).args(args).output().expect("run cli")
}

fn run_with_stdin(args: &[&str], input: &str) -> std::process::Output {
    let mut child = Command::new(bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run cli");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

const SET_WORKFLOW: &str = r#"
document: { dsl: '1.0.3', namespace: cli, name: set, version: '1.0.0' }
do:
  - s:
      set:
        greeting: '${ "hi " + .name }'
"#;

#[test]
fn validate_accepts_a_valid_workflow() {
    let file = temp_file("valid.yaml", SET_WORKFLOW);
    let out = run(&["validate", file.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("valid"));
    let _ = std::fs::remove_file(file);
}

#[test]
fn validate_rejects_an_invalid_workflow() {
    let file = temp_file(
        "invalid.yaml",
        r#"
document: { dsl: '1.0.3', namespace: cli, name: '', version: '1.0.0' }
do:
  - a: { set: { x: 1 } }
"#,
    );
    let out = run(&["validate", file.to_str().unwrap()]);
    assert!(!out.status.success());
    let _ = std::fs::remove_file(file);
}

#[test]
fn compile_emits_a_json_summary() {
    let file = temp_file("compile.yaml", SET_WORKFLOW);
    let out = run(&["compile", file.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).expect("json summary");
    assert_eq!(summary["workflow"]["name"], "set");
    assert_eq!(summary["tasks"], 1);
    assert_eq!(summary["task_types"][0], "set");
    let _ = std::fs::remove_file(file);
}

#[test]
fn run_executes_and_prints_the_output() {
    let file = temp_file("run.yaml", SET_WORKFLOW);
    let input = temp_file("run-input.yaml", "name: there\n");
    let out = run(&[
        "run",
        file.to_str().unwrap(),
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: serde_json::Value = serde_yaml::from_str(&stdout).unwrap();
    assert_eq!(value["greeting"], "hi there");
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(input);
}

#[test]
fn run_can_save_output_to_a_file() {
    let file = temp_file("run-output.yaml", SET_WORKFLOW);
    let output_file = temp_file("saved-output.yaml", "");
    let input = temp_file("saved-output-input.yaml", "name: there\n");
    let out = run(&[
        "run",
        file.to_str().unwrap(),
        "--input",
        input.to_str().unwrap(),
        "--output",
        output_file.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("output written to"));
    let value: serde_json::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&output_file).unwrap()).unwrap();
    assert_eq!(value["greeting"], "hi there");
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output_file);
}

#[test]
fn run_prompts_for_declared_input() {
    let workflow = r#"
document: { dsl: '1.0.3', namespace: cli, name: prompt, version: '1.0.0' }
input:
  schema:
    format: json
    document:
      type: object
      required: [name]
do:
  - s:
      set:
        greeting: '${ "hi " + .name }'
"#;
    let file = temp_file("prompt.yaml", workflow);
    let out = run_with_stdin(&["run", file.to_str().unwrap()], "name: prompted\n");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("declares input"));
    let value: serde_json::Value =
        serde_yaml::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(value["greeting"], "hi prompted");
    let _ = std::fs::remove_file(file);
}

#[test]
fn run_accepts_novel_url_and_chapter_range_flags() {
    let workflow = r#"
document: { dsl: '1.0.3', namespace: cli, name: flags, version: '1.0.0' }
input: { from: . }
do:
  - echo:
      set:
        url: '${ .novel_url }'
        from: '${ .chapter_from }'
        to: '${ .chapter_to }'
"#;
    let file = temp_file("flags.yaml", workflow);
    let out = run(&[
        "run",
        file.to_str().unwrap(),
        "--url",
        "https://brightnovels.com/series/example",
        "--from",
        "1",
        "--to",
        "5",
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value =
        serde_yaml::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(value["url"], "https://brightnovels.com/series/example");
    assert_eq!(value["from"], 1);
    assert_eq!(value["to"], 5);
    let _ = std::fs::remove_file(file);
}

#[test]
fn run_exposes_secrets_from_flags() {
    let workflow = r#"
document: { dsl: '1.0.3', namespace: cli, name: secrets, version: '1.0.0' }
use:
  secrets: [token]
do:
  - s:
      set:
        token: '${ $secrets.token }'
"#;
    let file = temp_file("secrets.yaml", workflow);
    let out = run(&["run", file.to_str().unwrap(), "--secret", "token=abc123"]);
    assert!(out.status.success());
    let value: serde_json::Value =
        serde_yaml::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(value["token"], "abc123");
    let _ = std::fs::remove_file(file);
}

#[test]
fn run_resolves_catalogs_from_flags() {
    let catalog = r#"
errors:
  catalogError:
    type: https://example.com/errors/catalog
    title: Catalog Error
    status: 418
"#;
    let catalog_file = temp_file("catalog.yaml", catalog);
    let workflow = r#"
document: { dsl: '1.0.3', namespace: cli, name: catalog, version: '1.0.0' }
use:
  catalogs:
    errors:
      endpoint: https://catalog.example/errors
do:
  - r:
      raise:
        error: catalogError
"#;
    let file = temp_file("catalog-wf.yaml", workflow);
    let spec = format!("https://catalog.example/errors={}", catalog_file.display());
    let out = run(&["run", file.to_str().unwrap(), "--catalog", &spec]);
    // The catalog error is raised, so the workflow faults with its type.
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("https://example.com/errors/catalog"),
        "stderr: {stderr}"
    );
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(catalog_file);
}

#[test]
fn run_executes_shell_when_scripts_are_allowed() {
    let workflow = r#"
document: { dsl: '1.0.3', namespace: cli, name: shell, version: '1.0.0' }
do:
  - say:
      run:
        shell:
          command: echo
          arguments: [hello-cli]
"#;
    let file = temp_file("shell.yaml", workflow);

    // Deny-by-default: the run faults.
    let denied = run(&["run", file.to_str().unwrap()]);
    assert!(!denied.status.success());

    // With --allow-scripts it executes.
    let allowed = run(&["run", file.to_str().unwrap(), "--allow-scripts"]);
    assert!(
        allowed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let value: serde_json::Value =
        serde_yaml::from_str(&String::from_utf8_lossy(&allowed.stdout)).unwrap();
    assert_eq!(value, serde_json::json!("hello-cli\n"));
    let _ = std::fs::remove_file(file);
}

#[test]
fn run_faults_with_a_nonzero_exit_code() {
    let workflow = r#"
document: { dsl: '1.0.3', namespace: cli, name: fault, version: '1.0.0' }
do:
  - boom:
      raise:
        error:
          type: https://example.com/errors/boom
          status: 500
          title: Boom
"#;
    let file = temp_file("fault.yaml", workflow);
    let out = run(&["run", file.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("faulted"));
    let _ = std::fs::remove_file(file);
}

#[test]
fn conformance_runs_the_vendored_ctk_and_writes_a_report() {
    let features = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("ctk")
        .join("features");
    let report = std::env::temp_dir().join(format!("ows-cli-report-{}.json", std::process::id()));

    let out = run(&[
        "conformance",
        "--ctk",
        features.to_str().unwrap(),
        "--output",
        report.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Total:"));

    let text = std::fs::read_to_string(&report).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["failures"], 0);
    assert!(value["passes"].as_u64().unwrap() > 0);
    let _ = std::fs::remove_file(report);
}
