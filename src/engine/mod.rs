//! The execution engine: validation orchestration, gatherer phase, DAG
//! scheduling and the step lifecycle.

pub mod dag;
pub mod events;
pub mod gather;
pub mod run;
pub mod scripts;
pub mod status;
pub mod vars;

use crate::diag::Diag;
use crate::model::Playbook;

use status::{Mode, RunReport};
use vars::VarStore;

/// Validate, gather, run: the full §9 sequence for one play.
pub fn execute(
    pb: &Playbook,
    play_name: &str,
    mode: Mode,
    continue_on_error: bool,
    jobs: Option<usize>,
    mut store: VarStore,
    events: events::EventSink,
) -> Result<RunReport, Vec<Diag>> {
    let Some(play) = pb.play(play_name) else {
        let names: Vec<&str> = pb.plays.iter().map(|p| p.name.as_str()).collect();
        return Err(vec![Diag::bare(format!(
            "no play named '{play_name}' (available: {})",
            names.join(", ")
        ))]);
    };

    let ctx = crate::hostapi::context();
    let scripts = scripts::compile_all(pb, &ctx)?;
    for p in &pb.plays {
        dag::build(p)?;
    }

    gather::run(pb, &scripts, &ctx, &mut store, &events)?;

    run::run_play(
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
    )
}

/// The full validation pipeline (PRD §8) beyond what `model::load`
/// already performed: DAG construction per play and compilation of every
/// wscript script against the host context.
pub fn validate(pb: &Playbook) -> Vec<Diag> {
    let mut diags = Vec::new();
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
