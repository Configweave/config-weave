//! Engine → reporter events: the live-progress feed for the rich
//! terminal mode and the NDJSON log.

use std::sync::Arc;

use super::status::{StepReport, StepStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Applying,
    Rechecking,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Checking => "checking",
            Phase::Applying => "applying",
            Phase::Rechecking => "re-checking",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
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
