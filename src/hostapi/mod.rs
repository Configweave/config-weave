//! The wscript host API Config Weave registers (PRD §7).
//!
//! Platform availability rule: every module is registered on every
//! platform so compilation, validation and `.wscripti` emission are identical
//! everywhere. Foreign-platform functions return runtime errors.

pub mod archive;
pub mod com;
pub mod data;
pub mod env;
pub mod fs;
pub mod hash;
pub mod http;
pub mod log;
pub mod path;
pub mod registry;
pub mod service;
pub mod shell;
pub mod sys;
pub mod template;
pub mod testlab;
pub mod types;
#[cfg(windows)]
pub mod windows_impl;

pub use types::{ApplyResult, CheckResult};

use wscript::Context;

/// Build the full host context all scripts compile and run against.
/// Identical on every platform (PRD §7): foreign-platform functions
/// return runtime errors instead of being absent.
pub fn context() -> Context {
    Context::new()
        .module(wscript_std::value())
        // Re-exported wscript-std data formats (PRD `data` overlap note).
        .module(wscript_std::json())
        .module(wscript_std::toml())
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
        .module(template::module())
        .module(registry::module())
        .module(service::module())
        .module(com::module())
        .register_type::<CheckResult>()
        .register_type::<ApplyResult>()
}

/// The context scenario driver scripts compile and run against: the full
/// host API plus the `testlab` module (the `Lab`/`Machine` driver API).
/// Used both by stage-5 validation and by the scenario runner.
pub fn scenario_context() -> Context {
    context().module(testlab::module())
}

/// Install the print hook for the current thread: raw `print`/`println`
/// from scripts route into `log::info` so stdout stays clean (PRD §7).
pub fn redirect_print_to_log() {
    wscript::vm::set_print_hook(Some(Box::new(|text: &str, _newline: bool| {
        for line in text.lines() {
            log::emit(log::Level::Info, line);
        }
    })));
}

/// Per-thread setup for any thread that runs scripts: print redirection
/// plus COM (STA) initialisation on Windows (PRD §7). Hold the guard for
/// the thread's lifetime.
pub struct WorkerGuard {
    #[cfg(windows)]
    _com: crate::comdispatch::ComInit,
}

pub fn worker_init() -> WorkerGuard {
    redirect_print_to_log();
    WorkerGuard {
        #[cfg(windows)]
        _com: crate::comdispatch::init_sta(),
    }
}
