//! Status phases for workflows and tasks, as defined by the OWS DSL.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The status phases a workflow or task can occupy, per the OWS DSL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Initiated and pending execution.
    Pending,
    /// Currently in progress.
    Running,
    /// Temporarily paused, awaiting events or a time interval.
    Waiting,
    /// Manually paused by a user.
    Suspended,
    /// Terminated before completion.
    Cancelled,
    /// Encountered an error.
    Faulted,
    /// Ran to completion.
    Completed,
}

impl Phase {
    /// Whether the phase is terminal.
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Cancelled | Phase::Faulted | Phase::Completed)
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Phase::Pending => "pending",
            Phase::Running => "running",
            Phase::Waiting => "waiting",
            Phase::Suspended => "suspended",
            Phase::Cancelled => "cancelled",
            Phase::Faulted => "faulted",
            Phase::Completed => "completed",
        };
        f.write_str(s)
    }
}
