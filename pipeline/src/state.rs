//! Shared daemon state, handed to custom routes via an axum `Extension`
//! layer (forge's own routes only carry `ForgeState`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_server::EventBus;

use crate::pipelines::PipelineDef;
use crate::runs::PipelineRunManager;

pub struct PipelineState {
    /// `{root}/pipelines.wcl` — the pipeline inventory, mirrored in memory
    /// and regenerated on every mutation.
    pub pipelines_path: PathBuf,
    pub pipelines: Mutex<Vec<PipelineDef>>,
    /// Root under which a play step's `playbook` name resolves to a
    /// playbook dir (`{playbooks_dir}/{playbook}`).
    pub playbooks_dir: PathBuf,
    /// The config-weave CLI the daemon shells out to for play steps.
    pub config_weave: String,
    pub runs: PipelineRunManager,
    pub events: EventBus,
}

pub type SharedState = Arc<PipelineState>;
