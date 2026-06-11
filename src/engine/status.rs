//! Step statuses and run reports (PRD §4/§9/§11).

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    Apply,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Check => "check",
            Mode::Apply => "apply",
        }
    }
}

/// The six PRD statuses plus `NotRun` for steps left undispatched when a
/// run halts early (Error or RebootRequired upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    AlreadyConfigured,
    Configured,
    NotConfigured,
    RebootRequired,
    Skipped,
    Error,
    NotRun,
}

impl StepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StepStatus::AlreadyConfigured => "already configured",
            StepStatus::Configured => "configured",
            StepStatus::NotConfigured => "not configured",
            StepStatus::RebootRequired => "reboot required",
            StepStatus::Skipped => "skipped",
            StepStatus::Error => "error",
            StepStatus::NotRun => "not run",
        }
    }

    /// Stable machine-readable form for `--json`.
    pub fn id(&self) -> &'static str {
        match self {
            StepStatus::AlreadyConfigured => "already_configured",
            StepStatus::Configured => "configured",
            StepStatus::NotConfigured => "not_configured",
            StepStatus::RebootRequired => "reboot_required",
            StepStatus::Skipped => "skipped",
            StepStatus::Error => "error",
            StepStatus::NotRun => "not_run",
        }
    }

    /// Inverse of [`StepStatus::id`]; the testlab runner parses statuses
    /// out of in-container `--json` reports.
    pub fn from_id(s: &str) -> Option<StepStatus> {
        match s {
            "already_configured" => Some(StepStatus::AlreadyConfigured),
            "configured" => Some(StepStatus::Configured),
            "not_configured" => Some(StepStatus::NotConfigured),
            "reboot_required" => Some(StepStatus::RebootRequired),
            "skipped" => Some(StepStatus::Skipped),
            "error" => Some(StepStatus::Error),
            "not_run" => Some(StepStatus::NotRun),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StepReport {
    pub name: String,
    /// Enclosing container names, outermost first.
    pub container_path: Vec<String>,
    pub resource: String,
    pub status: StepStatus,
    pub message: Option<String>,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct GatherReport {
    pub name: String,
    pub gatherer: String,
}

#[derive(Debug)]
pub struct RunReport {
    pub playbook: String,
    pub version: String,
    pub play: String,
    pub mode: Mode,
    pub gathered: Vec<GatherReport>,
    /// Declaration order, always complete (PRD: deterministic reporting).
    pub steps: Vec<StepReport>,
    pub duration: Duration,
}

impl RunReport {
    /// Exit code per PRD §9.
    pub fn exit_code(&self) -> u8 {
        if self.steps.iter().any(|s| s.status == StepStatus::Error) {
            1
        } else if self.mode == Mode::Apply
            && self
                .steps
                .iter()
                .any(|s| s.status == StepStatus::RebootRequired)
        {
            3
        } else {
            0
        }
    }
}
