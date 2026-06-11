//! The wisp host API Config Weave registers (PRD §7).
//!
//! Platform availability rule: every module is registered on every
//! platform so compilation, validation and `.wispi` emission are identical
//! everywhere. Foreign-platform functions return runtime errors.

pub mod archive;
pub mod data;
pub mod env;
pub mod fs;
pub mod hash;
pub mod http;
pub mod log;
pub mod path;
pub mod shell;
pub mod sys;
pub mod types;

pub use types::{ApplyResult, CheckResult};

use wisp::Context;

/// Build the full host context all scripts compile and run against.
/// Identical on every platform (PRD §7): foreign-platform functions
/// return runtime errors instead of being absent.
pub fn context() -> Context {
    Context::new()
        .module(wisp_std::value())
        // Re-exported wisp-std data formats (PRD `data` overlap note).
        .module(wisp_std::json())
        .module(wisp_std::toml())
        .module(log::module())
        .module(fs::module())
        .module(path::module())
        .module(shell::module())
        .module(http::module())
        .module(hash::module())
        .module(archive::module())
        .module(env::module())
        .module(sys::module())
        .module(data::module())
        .register_type::<CheckResult>()
        .register_type::<ApplyResult>()
}

/// Install the print hook for the current thread: raw `print`/`println`
/// from scripts route into `log::info` so stdout stays clean (PRD §7).
pub fn redirect_print_to_log() {
    wisp::vm::set_print_hook(Some(Box::new(|text: &str, _newline: bool| {
        for line in text.lines() {
            log::emit(log::Level::Info, line);
        }
    })));
}
