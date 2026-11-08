//! The wscript host API Config Weave registers (PRD §7).
//!
//! Platform availability rule: every module is registered on every
//! platform so compilation, validation and `.wscripti` emission are identical
//! everywhere. Foreign-platform functions return runtime errors.

pub mod acl;
pub mod archive;
pub mod com;
pub mod data;
pub mod encoding;
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
pub mod version;
#[cfg(windows)]
pub mod windows_impl;

pub use types::{ApplyResult, CheckResult};

use wscript::Context;

/// Parse a `timeout` option given in seconds, as an int or a float.
/// Returns `None` for a negative, NaN, infinite or too large value, which
/// the caller reports as an error. `Duration::from_secs_f64` would panic on
/// those, and a negative int would wrap through `as u64`.
pub(crate) fn timeout_secs(v: &wscript_std::DynValue) -> Option<std::time::Duration> {
    match v {
        wscript_std::DynValue::Int(secs) => {
            u64::try_from(*secs).ok().map(std::time::Duration::from_secs)
        }
        wscript_std::DynValue::Float(secs) => std::time::Duration::try_from_secs_f64(*secs).ok(),
        _ => None,
    }
}

/// Build the full host context all scripts compile and run against.
/// Identical on every platform (PRD §7): foreign-platform functions
/// return runtime errors instead of being absent.
pub fn context() -> Context {
    Context::new()
        .module(wscript_std::value())
        // Re-exported wscript-std data formats (PRD `data` overlap note).
        // `data` covers INI; these three are re-exported rather than
        // reimplemented.
        .module(wscript_std::json())
        .module(wscript_std::toml())
        .module(wscript_std::xml())
        // Pattern matching over command output and config files. Scripts
        // otherwise reach for `shell::run("grep …")`, which is a
        // subprocess and platform-dependent.
        .module(wscript_std::regex())
        // Clock access: resources that converge on an age (a package cache
        // refreshed within the last N minutes) need "now" to compare a
        // stamp file's mtime against, and shelling out to `date +%s` for it
        // is both a subprocess and Unix-only.
        .module(wscript_std::time())
        .module(log::module())
        .module(fs::module())
        .module(path::module())
        .module(shell::module())
        .module(http::module())
        .module(hash::module())
        .module(encoding::module())
        .module(version::module())
        .module(archive::module())
        .module(env::module())
        .module(sys::module())
        .module(data::module())
        .module(template::module())
        .module(registry::module())
        .module(acl::module())
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

/// Build a VM whose `print`/`println` route into `log::info` so stdout
/// stays clean (PRD §7). Every VM config-weave runs scripts on is built
/// here — the sink is per-VM, so a VM built any other way prints to
/// stdout and breaks the output contract.
pub fn vm(ctx: &Context) -> wscript::Vm {
    wscript::Vm::with_config(
        ctx,
        wscript::VmConfig {
            out: Box::new(|text: &str, _newline: bool| {
                for line in text.lines() {
                    log::emit(log::Level::Info, line);
                }
            }),
            ..wscript::VmConfig::default()
        },
    )
}

/// Per-thread setup for any thread that runs scripts: COM (STA)
/// initialisation on Windows (PRD §7). Hold the guard for the thread's
/// lifetime.
pub struct WorkerGuard {
    #[cfg(windows)]
    _com: crate::comdispatch::ComInit,
}

pub fn worker_init() -> WorkerGuard {
    WorkerGuard {
        #[cfg(windows)]
        _com: crate::comdispatch::init_sta(),
    }
}

#[cfg(test)]
mod tests {
    use super::timeout_secs;
    use std::time::Duration;
    use wscript_std::DynValue;

    #[test]
    fn timeout_secs_accepts_non_negative_numbers() {
        assert_eq!(timeout_secs(&DynValue::Int(0)), Some(Duration::ZERO));
        assert_eq!(timeout_secs(&DynValue::Int(2)), Some(Duration::from_secs(2)));
        assert_eq!(
            timeout_secs(&DynValue::Float(1.5)),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn timeout_secs_rejects_values_that_would_panic_or_wrap() {
        for v in [
            DynValue::Int(-1),
            DynValue::Float(-1.0),
            DynValue::Float(f64::NAN),
            DynValue::Float(f64::INFINITY),
            DynValue::Float(1e30),
            DynValue::String("5".into()),
        ] {
            assert_eq!(timeout_secs(&v), None, "{v:?}");
        }
    }
}
