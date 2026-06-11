//! The execution engine: validation orchestration, gatherer phase, DAG
//! scheduling and the step lifecycle.

pub mod dag;
pub mod scripts;

use crate::diag::Diag;
use crate::model::Playbook;

/// The full validation pipeline (PRD §8) beyond what `model::load`
/// already performed: DAG construction per play and compilation of every
/// wisp script against the host context.
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
