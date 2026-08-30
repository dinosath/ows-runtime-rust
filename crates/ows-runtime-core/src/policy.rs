//! Runtime security and resource policies.
//!
//! Workflow definitions may be supplied by untrusted users. The runtime must be
//! deny-by-default for security sensitive capabilities. These policies are
//! enforced by the execution engine to bound CPU, memory, network, task counts,
//! loops and script execution.

use serde::{Deserialize, Serialize};

/// Bounds for the retry of a failing task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryBounds {
    /// Maximum number of retry attempts allowed for a single task.
    pub max_attempts: usize,
}

/// Runtime execution policy.
///
/// All limits are enforced deny-by-default for security-sensitive capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimePolicy {
    /// Maximum wall-clock time for a workflow execution, if any.
    pub max_execution_time: Option<std::time::Duration>,
    /// Maximum wall-clock time for a single task, if any.
    pub max_task_time: Option<std::time::Duration>,
    /// Maximum number of tasks that may run concurrently.
    pub max_parallel_tasks: usize,
    /// Maximum number of loop iterations in a single `for` construct.
    pub max_loop_iterations: usize,
    /// Maximum size, in bytes, of a workflow payload (input or output).
    pub max_payload_size: Option<usize>,
    /// Maximum number of retry attempts allowed.
    pub retry_bounds: RetryBounds,
    /// Whether scripts (`run.script` / `run.shell`) are allowed.
    pub allow_scripts: bool,
    /// Whether container processes (`run.container`) are allowed.
    pub allow_containers: bool,
    /// Whether external network calls are allowed at all.
    pub allow_network: bool,
    /// The network destinations allowed (host names). Empty means none.
    pub allowed_hosts: Vec<String>,
    /// The URL schemes allowed (e.g. `https`, `http`). Empty means none.
    pub allowed_schemes: Vec<String>,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            max_execution_time: None,
            max_task_time: None,
            max_parallel_tasks: 1024,
            max_loop_iterations: 1_000_000,
            max_payload_size: None,
            retry_bounds: RetryBounds { max_attempts: 10 },
            // Deny-by-default for security sensitive capabilities.
            allow_scripts: false,
            allow_containers: false,
            allow_network: false,
            allowed_hosts: Vec::new(),
            allowed_schemes: Vec::new(),
        }
    }
}

impl RuntimePolicy {
    /// Returns whether the given host is permitted for network access.
    pub fn host_allowed(&self, host: &str) -> bool {
        if !self.allow_network {
            return false;
        }
        self.allowed_hosts.is_empty()
            || self
                .allowed_hosts
                .iter()
                .any(|allowed| host == allowed || host.ends_with(&format!(".{allowed}")))
    }

    /// Returns whether the given URL scheme is permitted.
    pub fn scheme_allowed(&self, scheme: &str) -> bool {
        if !self.allow_network {
            return false;
        }
        self.allowed_schemes.is_empty() || self.allowed_schemes.iter().any(|s| s == scheme)
    }
}
