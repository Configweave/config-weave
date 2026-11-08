//! The execution engine: validation orchestration, gatherer phase, DAG
//! scheduling and the step lifecycle.

pub mod dag;
pub mod events;
pub mod gather;
pub mod run;
pub mod scripts;
pub mod status;
pub mod vars;
pub mod wait;

use crate::diag::Diag;
use crate::model::Playbook;

use status::{Mode, RunReport};
use vars::VarStore;

/// Why `execute` failed. A gather failure is named apart, so the CLI can
/// say which phase stopped the run.
pub enum ExecError {
    /// A gatherer failed. No step ran.
    Gather(Vec<Diag>),
    /// Anything else: a missing play, a compile error, or a validation error.
    Diags(Vec<Diag>),
}

impl From<Vec<Diag>> for ExecError {
    fn from(diags: Vec<Diag>) -> Self {
        ExecError::Diags(diags)
    }
}

/// Validate, gather, run: the full §9 sequence for one play.
pub fn execute(
    pb: &Playbook,
    play_name: &str,
    mode: Mode,
    continue_on_error: bool,
    jobs: Option<usize>,
    mut store: VarStore,
    events: events::EventSink,
) -> Result<RunReport, ExecError> {
    let Some(play) = pb.play(play_name) else {
        let names: Vec<&str> = pb.plays.iter().map(|p| p.name.as_str()).collect();
        return Err(vec![Diag::bare(format!(
            "no play named '{play_name}' (available: {})",
            names.join(", ")
        ))]
        .into());
    };

    let ctx = crate::hostapi::context();
    let scripts = scripts::compile_all(pb, &ctx)?;
    for p in &pb.plays {
        dag::build(p)?;
    }

    events(events::Event::RunStarted {
        play: play.name.clone(),
        mode: mode.as_str(),
        steps: play
            .steps()
            .iter()
            .map(|s| events::PlannedStep {
                name: s.name.clone(),
                resource: s.target(),
                container_path: s.container_path.clone(),
            })
            .collect(),
    });

    gather::run(pb, &scripts, &ctx, &mut store, &events).map_err(ExecError::Gather)?;

    Ok(run::run_play(
        pb,
        play,
        &scripts,
        &ctx,
        &store,
        &run::RunOptions {
            mode,
            continue_on_error,
            jobs: jobs.unwrap_or_else(run::default_jobs),
            events,
        },
    )?)
}

/// The full validation pipeline (PRD §8) beyond what `model::load`
/// already performed: DAG construction per play and compilation of every
/// wscript script against the host context.
pub fn validate(pb: &Playbook) -> Vec<Diag> {
    let mut diags = Vec::new();
    diags.extend(validate_secrets(pb));
    for play in &pb.plays {
        if let Err(ds) = dag::build(play) {
            diags.extend(ds);
        }
    }
    let ctx = crate::hostapi::context();
    if let Err(ds) = scripts::compile_all(pb, &ctx) {
        diags.extend(ds);
    }
    diags
}

/// A `secret()` still holding a plaintext, or one this feature cannot
/// rewrite, fails validation — so a run can never silently use (or a
/// commit ship) an un-encrypted secret.
fn validate_secrets(pb: &Playbook) -> Vec<Diag> {
    let path = pb.root.join("playbook.wcl");
    match crate::secrets::scan::scan_source(&pb.source, &path.display().to_string()) {
        // A parse error here is already reported by `model::load`.
        Err(_) => Vec::new(),
        Ok(calls) => crate::secrets::validate_calls(&pb.source, &path, &calls),
    }
}
