//! Terminal output (PRD §11): three mutually exclusive modes.
//!
//! - **Rich** (default on a TTY): colour, Unicode icons, a live progress
//!   line on stderr with phase detail, per-step timing.
//! - **Plain** (--no-color or not a TTY): ASCII, line-oriented, no cursor
//!   movement.
//! - **JSON** (--json): one complete JSON object on stdout at completion;
//!   nothing else touches stdout.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::sync::Mutex;

use crate::engine::events::{Event, EventSink, Phase};
use crate::engine::status::{RunReport, StepStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Rich,
    Plain,
    Json,
}

pub fn select_mode(json: bool, no_color: bool) -> OutputMode {
    if json {
        OutputMode::Json
    } else if no_color || !std::io::stdout().is_terminal() || !std::io::stderr().is_terminal() {
        OutputMode::Plain
    } else {
        OutputMode::Rich
    }
}

// ----------------------------------------------------------------- icons

fn icon(status: StepStatus) -> (&'static str, &'static str) {
    // (icon, ANSI colour)
    match status {
        StepStatus::AlreadyConfigured => ("✓", "\x1b[32m"),
        StepStatus::Configured => ("●", "\x1b[1;32m"),
        StepStatus::NotConfigured => ("○", "\x1b[33m"),
        StepStatus::RebootRequired => ("↻", "\x1b[35m"),
        StepStatus::Skipped => ("–", "\x1b[2m"),
        StepStatus::Error => ("✗", "\x1b[31m"),
        StepStatus::NotRun => ("·", "\x1b[2m"),
    }
}

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

// ------------------------------------------------------------- progress

/// Live progress sink for the rich mode. Completed steps print as
/// permanent lines; a single live status line at the bottom shows what is
/// in flight with its phase.
struct RichState {
    in_flight: BTreeMap<usize, (String, Phase)>,
    gathering: Option<usize>,
    live_line_shown: bool,
}

pub fn progress_sink(mode: OutputMode) -> EventSink {
    match mode {
        OutputMode::Rich => {
            let state = Mutex::new(RichState {
                in_flight: BTreeMap::new(),
                gathering: None,
                live_line_shown: false,
            });
            std::sync::Arc::new(move |event| {
                let mut s = state.lock().unwrap();
                rich_event(&mut s, event);
            })
        }
        // Plain and JSON modes have no live progress; the final report
        // (or JSON object) carries everything.
        _ => crate::engine::events::null_sink(),
    }
}

fn rich_event(s: &mut RichState, event: Event) {
    let mut err = std::io::stderr().lock();
    if s.live_line_shown {
        let _ = write!(err, "\r\x1b[2K");
        s.live_line_shown = false;
    }
    match event {
        Event::GatherStarted { unique } => {
            s.gathering = Some(unique);
        }
        Event::GatherFinished => {
            if let Some(n) = s.gathering.take() {
                let _ = writeln!(err, "{DIM}gathered {n} fact set(s){RESET}");
            }
        }
        Event::StepStarted { idx, name } => {
            s.in_flight.insert(idx, (name, Phase::Checking));
        }
        Event::StepPhase { idx, name, phase } => {
            s.in_flight.insert(idx, (name, phase));
        }
        Event::StepFinished { idx, report } => {
            s.in_flight.remove(&idx);
            let (ic, colour) = icon(report.status);
            let mut line = format!(
                "{colour}{ic}{RESET} {} {DIM}({}){RESET} {}",
                report.name,
                report.resource,
                report.status.as_str()
            );
            if report.duration.as_millis() >= 50 {
                line.push_str(&format!(
                    " {DIM}{:.1}s{RESET}",
                    report.duration.as_secs_f64()
                ));
            }
            if let Some(msg) = &report.message {
                line.push_str(&format!(" {DIM}— {msg}{RESET}"));
            }
            let _ = writeln!(err, "{line}");
        }
        Event::StepResolved { idx, name, status } => {
            s.in_flight.remove(&idx);
            let (ic, colour) = icon(status);
            let _ = writeln!(err, "{colour}{ic}{RESET} {name} {}", status.as_str());
        }
    }
    if let Some(n) = s.gathering {
        let _ = write!(err, "{DIM}⟳ gathering ({n} unique)…{RESET}");
        s.live_line_shown = true;
    } else if !s.in_flight.is_empty() {
        let items: Vec<String> = s
            .in_flight
            .values()
            .map(|(name, phase)| format!("{name} ({})", phase.as_str()))
            .collect();
        let _ = write!(err, "{DIM}⟳ {}{RESET}", items.join(", "));
        s.live_line_shown = true;
    }
    let _ = err.flush();
}

// --------------------------------------------------------------- report

/// Plain mode: one line per step in declaration order, then a summary.
pub fn plain(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} '{}' v{} — play '{}'\n",
        report.mode.as_str(),
        report.playbook,
        report.version,
        report.play
    ));
    if !report.gathered.is_empty() {
        out.push_str("gathered:\n");
        for g in &report.gathered {
            out.push_str(&format!("  {} <- {}\n", g.name, g.gatherer));
        }
    }
    out.push_str("steps:\n");
    for s in &report.steps {
        let path = if s.container_path.is_empty() {
            s.name.clone()
        } else {
            format!("{}/{}", s.container_path.join("/"), s.name)
        };
        let mut line = format!("  [{:>19}] {} ({})", s.status.as_str(), path, s.resource);
        if s.duration.as_millis() > 0 {
            line.push_str(&format!(" {:.1}s", s.duration.as_secs_f64()));
        }
        if let Some(msg) = &s.message {
            line.push_str(&format!(" — {msg}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&summary_line(report));
    out
}

/// Rich mode final report: the live lines already told the story, so
/// print the coloured summary.
pub fn rich(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n{} '{}' v{} — play '{}'\n",
        report.mode.as_str(),
        report.playbook,
        report.version,
        report.play
    ));
    for s in &report.steps {
        let (ic, colour) = icon(s.status);
        let path = if s.container_path.is_empty() {
            s.name.clone()
        } else {
            format!("{}/{}", s.container_path.join("/"), s.name)
        };
        let mut line = format!(
            "  {colour}{ic}{RESET} {:<30} {:<20}",
            path,
            s.status.as_str()
        );
        if s.duration.as_millis() > 0 {
            line.push_str(&format!(" {DIM}{:.1}s{RESET}", s.duration.as_secs_f64()));
        }
        if let Some(msg) = &s.message {
            line.push_str(&format!(" {DIM}— {msg}{RESET}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&summary_line(report));
    out
}

fn summary_line(report: &RunReport) -> String {
    let count = |status: StepStatus| report.steps.iter().filter(|s| s.status == status).count();
    format!(
        "summary: {} already configured, {} configured, {} not configured, {} reboot required, \
         {} skipped, {} error, {} not run ({:.1}s)\n",
        count(StepStatus::AlreadyConfigured),
        count(StepStatus::Configured),
        count(StepStatus::NotConfigured),
        count(StepStatus::RebootRequired),
        count(StepStatus::Skipped),
        count(StepStatus::Error),
        count(StepStatus::NotRun),
        report.duration.as_secs_f64(),
    )
}

// ----------------------------------------------------------------- json

// The `--json` object is schema-stable (PRD §11) and consumed by test
// harnesses — including config-weave itself: the testlab runner parses
// reports produced inside containers with these same types, so the
// schema stays single-sourced.

#[derive(serde::Serialize, serde::Deserialize)]
pub struct JsonRunStep {
    pub name: String,
    pub container_path: Vec<String>,
    pub resource: String,
    /// `StepStatus::id()` form, e.g. "already_configured".
    pub status: String,
    pub message: Option<String>,
    pub duration_secs: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct JsonRunGather {
    pub name: String,
    pub gatherer: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct JsonRunReport {
    pub playbook: String,
    pub version: String,
    pub play: String,
    pub mode: String,
    pub exit_code: u8,
    pub duration_secs: f64,
    pub gathered: Vec<JsonRunGather>,
    pub steps: Vec<JsonRunStep>,
}

impl JsonRunReport {
    pub fn from_report(report: &RunReport) -> JsonRunReport {
        JsonRunReport {
            playbook: report.playbook.clone(),
            version: report.version.clone(),
            play: report.play.clone(),
            mode: report.mode.as_str().to_string(),
            exit_code: report.exit_code(),
            duration_secs: report.duration.as_secs_f64(),
            gathered: report
                .gathered
                .iter()
                .map(|g| JsonRunGather {
                    name: g.name.clone(),
                    gatherer: g.gatherer.clone(),
                })
                .collect(),
            steps: report
                .steps
                .iter()
                .map(|s| JsonRunStep {
                    name: s.name.clone(),
                    container_path: s.container_path.clone(),
                    resource: s.resource.clone(),
                    status: s.status.id().to_string(),
                    message: s.message.clone(),
                    duration_secs: s.duration.as_secs_f64(),
                })
                .collect(),
        }
    }
}

/// JSON mode: the single, schema-stable object (PRD §11).
pub fn json(report: &RunReport) -> String {
    serde_json::to_string_pretty(&JsonRunReport::from_report(report))
        .expect("report serialization cannot fail")
}
