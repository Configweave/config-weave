//! Terminal report rendering. M2 ships the plain (ASCII, line-oriented)
//! mode; M6 adds the rich TTY and JSON modes.

use crate::engine::status::{RunReport, StepStatus};

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
        let mut line = format!(
            "  [{:>19}] {} ({})",
            s.status.as_str(),
            path,
            s.resource
        );
        if s.duration.as_millis() > 0 {
            line.push_str(&format!(" {:.1}s", s.duration.as_secs_f64()));
        }
        if let Some(msg) = &s.message {
            line.push_str(&format!(" — {msg}"));
        }
        out.push_str(&line);
        out.push('\n');
    }

    let count = |status: StepStatus| report.steps.iter().filter(|s| s.status == status).count();
    out.push_str(&format!(
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
    ));
    out
}
