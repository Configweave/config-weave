//! Result types for `config-weave test`. Formatting lives with the run
//! formatters in `crate::report`.

use std::time::Duration;

use crate::engine::status::StepStatus;
use crate::model::Expect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    /// Expectations, gather assertions or verify said no.
    Failed,
    /// Environmental: provisioning, setup, protocol or parse trouble.
    Error,
}

impl TestOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            TestOutcome::Passed => "passed",
            TestOutcome::Failed => "failed",
            TestOutcome::Error => "error",
        }
    }
}

/// One step's statuses across the three engine runs, plus every
/// expectation mismatch.
#[derive(Debug)]
pub struct TestStepResult {
    pub name: String,
    pub expect: Expect,
    pub check: Option<StepStatus>,
    pub apply: Option<StepStatus>,
    pub second_apply: Option<StepStatus>,
    pub failures: Vec<String>,
}

#[derive(Debug)]
pub struct TestGatherResult {
    pub name: String,
    pub failures: Vec<String>,
}

#[derive(Debug)]
pub struct VerifyResult {
    pub passed: bool,
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct TestReport {
    pub package: String,
    pub name: String,
    pub backend: String,
    pub image: String,
    pub outcome: TestOutcome,
    pub steps: Vec<TestStepResult>,
    pub gathers: Vec<TestGatherResult>,
    pub verify: Option<VerifyResult>,
    /// What broke, when `outcome` is `Error`.
    pub error: Option<String>,
    /// Instance handle when kept for debugging.
    pub kept: Option<String>,
    pub duration: Duration,
}

impl TestReport {
    /// Every failure string, prefixed for display.
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for g in &self.gathers {
            out.extend(g.failures.iter().cloned());
        }
        for s in &self.steps {
            out.extend(s.failures.iter().cloned());
        }
        if let Some(v) = &self.verify
            && !v.passed
        {
            out.push(match &v.message {
                Some(m) => format!("verify: {m}"),
                None => "verify: failed".into(),
            });
        }
        if let Some(e) = &self.error {
            out.push(e.clone());
        }
        out
    }
}

#[derive(Debug)]
pub struct TestRunReport {
    pub playbook: String,
    pub tests: Vec<TestReport>,
    pub duration: Duration,
}

impl TestRunReport {
    /// 0 = every test passed, 1 = any failed or errored. (Validation and
    /// environment problems exit 2 before a report exists.)
    pub fn exit_code(&self) -> u8 {
        if self.tests.iter().any(|t| t.outcome != TestOutcome::Passed) {
            1
        } else {
            0
        }
    }
}
