//! The `log` module. Script log calls route through a thread-local sink so
//! the engine can attach step context (each worker thread runs one step at
//! a time). The default sink writes to stderr; the run engine installs a
//! sink that forwards into the reporting/logging pipeline.

use std::cell::RefCell;

use wisp::Module;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

type Sink = Box<dyn Fn(Level, &str)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Install a log sink for the current thread (the engine calls this per
/// worker with the in-flight step's context baked in).
pub fn set_sink(sink: Sink) {
    SINK.with(|s| *s.borrow_mut() = Some(sink));
}

pub fn clear_sink() {
    SINK.with(|s| *s.borrow_mut() = None);
}

/// Emit a script log line through the current thread's sink.
pub fn emit(level: Level, msg: &str) {
    SINK.with(|s| match &*s.borrow() {
        Some(sink) => sink(level, msg),
        None => eprintln!("[{}] {msg}", level.as_str()),
    });
}

pub fn module() -> Module {
    let mut m = Module::new("log");
    m.doc("Structured logging with step context attached");
    m.doc_next("Log at debug level");
    m.fn_("debug", |msg: &str| emit(Level::Debug, msg));
    m.doc_next("Log at info level");
    m.fn_("info", |msg: &str| emit(Level::Info, msg));
    m.doc_next("Log at warn level");
    m.fn_("warn", |msg: &str| emit(Level::Warn, msg));
    m.doc_next("Log at error level");
    m.fn_("error", |msg: &str| emit(Level::Error, msg));
    m
}
