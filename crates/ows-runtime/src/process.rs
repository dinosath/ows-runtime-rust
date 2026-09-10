//! A real [`ProcessRunner`] backed by `tokio::process`.
//!
//! The runtime is deny-by-default for process execution: the default runner is
//! a no-op that rejects every process. [`TokioProcessRunner`] is an opt-in
//! runner that actually executes shell commands, scripts and containers, with
//! explicit switches for each capability.

use std::collections::HashMap;
use std::process::Stdio;

use ows_runtime_core::{ProcessResult, ProcessRunner, WorkflowError};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Executes processes with `tokio::process`.
///
/// By default scripts and containers are both disabled; enable them explicitly
/// with [`TokioProcessRunner::allow_scripts`] and
/// [`TokioProcessRunner::allow_containers`].
#[derive(Debug, Clone)]
pub struct TokioProcessRunner {
    allow_scripts: bool,
    allow_containers: bool,
    docker: String,
}

impl Default for TokioProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl TokioProcessRunner {
    /// Creates a runner with process execution disabled.
    pub fn new() -> Self {
        Self {
            allow_scripts: false,
            allow_containers: false,
            docker: "docker".to_string(),
        }
    }

    /// Enables shell and script execution.
    pub fn allow_scripts(mut self) -> Self {
        self.allow_scripts = true;
        self
    }

    /// Enables container execution (via the Docker CLI).
    pub fn allow_containers(mut self) -> Self {
        self.allow_containers = true;
        self
    }

    /// Sets the container runtime executable (default `docker`).
    pub fn with_docker(mut self, docker: impl Into<String>) -> Self {
        self.docker = docker.into();
        self
    }

    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(env)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            crate::error::communication_error(500, format!("failed to spawn `{program}`: {e}"))
        })?;

        if let Some(input) = stdin {
            if let Some(mut handle) = child.stdin.take() {
                let _ = handle.write_all(input.as_bytes()).await;
                let _ = handle.shutdown().await;
            }
        }

        let output = child.wait_with_output().await.map_err(|e| {
            crate::error::communication_error(500, format!("failed to await `{program}`: {e}"))
        })?;

        Ok(ProcessResult {
            code: output.status.code(),
            stdout: Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
        })
    }
}

#[async_trait::async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run_shell(
        &self,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        if !self.allow_scripts {
            return Err(crate::error::policy_error(
                "shell execution is disabled; configure TokioProcessRunner::allow_scripts",
            ));
        }
        self.run(command, args, env, stdin).await
    }

    async fn run_script(
        &self,
        language: &str,
        code: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        if !self.allow_scripts {
            return Err(crate::error::policy_error(
                "script execution is disabled; configure TokioProcessRunner::allow_scripts",
            ));
        }
        let (program, mut program_args) = script_interpreter(language).ok_or_else(|| {
            crate::error::semantic_error(format!("unsupported script language `{language}`"))
        })?;
        program_args.push(code.to_string());
        program_args.extend(args.iter().cloned());
        self.run(program, &program_args, env, stdin).await
    }

    async fn run_container(
        &self,
        image: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        if !self.allow_containers {
            return Err(crate::error::policy_error(
                "container execution is disabled; configure TokioProcessRunner::allow_containers",
            ));
        }
        let mut docker_args = vec!["run".to_string(), "--rm".to_string(), "-i".to_string()];
        for (key, value) in env {
            docker_args.push("-e".to_string());
            docker_args.push(format!("{key}={value}"));
        }
        docker_args.push(image.to_string());
        docker_args.extend(args.iter().cloned());
        self.run(&self.docker, &docker_args, &HashMap::new(), stdin)
            .await
    }
}

/// Returns the interpreter program and the flag used to pass inline code.
fn script_interpreter(language: &str) -> Option<(&'static str, Vec<String>)> {
    let (program, flag) = match language {
        "python" | "python3" => ("python3", "-c"),
        "node" | "javascript" | "js" => ("node", "-e"),
        "bash" => ("bash", "-c"),
        "sh" | "shell" => ("sh", "-c"),
        "ruby" => ("ruby", "-e"),
        "php" => ("php", "-r"),
        "perl" => ("perl", "-e"),
        _ => return None,
    };
    Some((program, vec![flag.to_string()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreter_mapping() {
        assert_eq!(script_interpreter("python3").unwrap().0, "python3");
        assert_eq!(script_interpreter("bash").unwrap().0, "bash");
        assert!(script_interpreter("unknown").is_none());
    }

    #[tokio::test]
    async fn denies_by_default() {
        let runner = TokioProcessRunner::new();
        let err = runner
            .run_shell("echo", &[], &HashMap::new(), None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ows_runtime_core::ErrorKind::Policy);
        let err = runner
            .run_container("hello-world", &[], &HashMap::new(), None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ows_runtime_core::ErrorKind::Policy);
    }

    #[tokio::test]
    async fn runs_a_shell_command_when_allowed() {
        let runner = TokioProcessRunner::new().allow_scripts();
        let result = runner
            .run_shell("echo", &["hello".to_string()], &HashMap::new(), None)
            .await
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert!(result.stdout.unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn runs_a_script_and_feeds_stdin() {
        let runner = TokioProcessRunner::new().allow_scripts();
        let result = runner
            .run_script(
                "bash",
                "cat",
                &[],
                &HashMap::new(),
                Some("from-stdin".to_string()),
            )
            .await
            .unwrap();
        assert!(result.stdout.unwrap().contains("from-stdin"));
    }

    #[tokio::test]
    async fn unsupported_language_errors() {
        let runner = TokioProcessRunner::new().allow_scripts();
        let err = runner
            .run_script("cobol", "DISPLAY 'x'.", &[], &HashMap::new(), None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ows_runtime_core::ErrorKind::Semantic);
    }
}
