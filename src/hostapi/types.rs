//! Host-registered types shared across the script contract: the result
//! enums every resource script's `check`/`apply` return. Errors are *not*
//! enum variants — scripts use `Result`/`?` and an `Err` maps to the step's
//! Error status (PRD §6).

use wscript::Script;

#[derive(Script, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckResult {
    AlreadyConfigured,
    NotConfigured,
    RebootRequired,
}

#[derive(Script, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyResult {
    Success,
    RebootRequired,
}
