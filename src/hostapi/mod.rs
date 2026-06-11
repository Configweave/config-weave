//! The wisp host API Config Weave registers (PRD §7).
//!
//! Platform availability rule: every module is registered on every
//! platform so compilation, validation and `.wispi` emission are identical
//! everywhere. Foreign-platform functions return runtime errors.

pub mod fs;
pub mod log;
pub mod path;
pub mod types;

pub use types::{ApplyResult, CheckResult};

use wisp::Context;

/// Build the full host context all scripts compile and run against.
pub fn context() -> Context {
    Context::new()
        .module(wisp_std::value())
        .module(log::module())
        .module(fs::module())
        .module(path::module())
        .register_type::<CheckResult>()
        .register_type::<ApplyResult>()
}
