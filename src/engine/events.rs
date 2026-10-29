//! Engine → reporter events: the live-progress feed for the rich
//! terminal mode and the NDJSON log.

use std::sync::Arc;
use std::time::Duration;

use super::status::{StepReport, StepStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Applying,
    Rechecking,
    /// A wait step polling its probe.
    Waiting,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Checking => "checking",
            Phase::Applying => "applying",
            Phase::Rechecking => "re-checking",
            Phase::Waiting => "waiting",
        }
    }
}

/// One entry per step in the target play, flattened through containers in
/// declaration order — the same order the scheduler assigns `idx`.
#[derive(Debug, Clone)]
pub struct PlannedStep {
    pub name: String,
    /// `package.resource`.
    pub resource: String,
    /// Enclosing container names, outermost first.
    pub container_path: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// Emitted once after validation/DAG build, before gathering, so a
    /// machine consumer can pre-render the step table.
    RunStarted {
        play: String,
        mode: &'static str,
        steps: Vec<PlannedStep>,
    },
    /// The gather phase begins with this many unique executions.
    GatherStarted {
        unique: usize,
    },
    GatherFinished,
    StepStarted {
        idx: usize,
        name: String,
    },
    StepPhase {
        idx: usize,
        name: String,
        phase: Phase,
    },
    /// A wait step's probe reported not ready (or failed) and the step
    /// will try again after its interval.
    StepWaiting {
        idx: usize,
        name: String,
        attempt: u32,
        elapsed: Duration,
        timeout: Duration,
        /// The probe's error, when it failed rather than reporting not
        /// ready. Already scrubbed of secrets.
        error: Option<String>,
    },
    StepFinished {
        idx: usize,
        report: StepReport,
    },
    /// A step completed without running (skipped / blocked / halted).
    StepResolved {
        idx: usize,
        name: String,
        status: StepStatus,
    },
}

pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

pub fn null_sink() -> EventSink {
    Arc::new(|_| {})
}
