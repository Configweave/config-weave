//! File logging (PRD §11): tracing-subscriber + tracing-appender emitting
//! NDJSON, enabled by --log-file/--log-level. Independent of the terminal
//! mode. The returned guard must live for the whole process or buffered
//! log lines are silently lost on exit.

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use tracing::Level;

use crate::diag::Diag;
use crate::hostapi::log as scriptlog;

pub struct LogGuard {
    _appender: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Terminal verbosity from `-v` (file logging is independent): debug
/// script log lines reach the terminal only at verbosity >= 1.
static VERBOSITY: AtomicU8 = AtomicU8::new(0);

pub fn set_verbosity(v: u8) {
    VERBOSITY.store(v, Ordering::Relaxed);
}

fn terminal_shows(level: scriptlog::Level) -> bool {
    level > scriptlog::Level::Debug || VERBOSITY.load(Ordering::Relaxed) >= 1
}

/// Install the global NDJSON subscriber when a log file is requested.
pub fn init(log_file: Option<&Path>, log_level: &str) -> Result<LogGuard, Diag> {
    let Some(path) = log_file else {
        return Ok(LogGuard { _appender: None });
    };
    let level: Level = log_level
        .parse()
        .map_err(|_| Diag::bare(format!("invalid --log-level '{log_level}'")))?;

    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let file_name = path
        .file_name()
        .ok_or_else(|| Diag::bare(format!("--log-file '{}' has no file name", path.display())))?;
    let appender =
        tracing_appender::rolling::never(dir.unwrap_or_else(|| Path::new(".")), file_name);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::fmt()
        .json()
        .with_max_level(level)
        .with_writer(writer)
        .with_current_span(false)
        .init();

    Ok(LogGuard {
        _appender: Some(guard),
    })
}

/// Clear the rich-mode live progress line before writing a log line, so
/// interleaved script output stays readable.
fn clear_live_line() {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        eprint!("\r\x1b[2K");
    }
}

/// Install the per-thread script log sink for one step: terminal line on
/// stderr plus a tracing event with step context fields (PRD §11).
pub fn install_step_sink(step: &str, resource: &str) {
    let step = step.to_string();
    let resource = resource.to_string();
    scriptlog::set_sink(Box::new(move |level, msg| {
        if terminal_shows(level) {
            clear_live_line();
            eprintln!("    [{step}] {}: {msg}", level.as_str());
        }
        match level {
            scriptlog::Level::Debug => {
                tracing::debug!(target: "script", step = %step, resource = %resource, "{msg}");
            }
            scriptlog::Level::Info => {
                tracing::info!(target: "script", step = %step, resource = %resource, "{msg}");
            }
            scriptlog::Level::Warn => {
                tracing::warn!(target: "script", step = %step, resource = %resource, "{msg}");
            }
            scriptlog::Level::Error => {
                tracing::error!(target: "script", step = %step, resource = %resource, "{msg}");
            }
        }
    }));
}

/// Sink for gatherer threads.
pub fn install_gatherer_sink(gatherer: &str) {
    let gatherer = gatherer.to_string();
    scriptlog::set_sink(Box::new(move |level, msg| {
        if terminal_shows(level) {
            clear_live_line();
            eprintln!("    [gather {gatherer}] {}: {msg}", level.as_str());
        }
        tracing::info!(target: "script", gatherer = %gatherer, "{msg}");
    }));
}
