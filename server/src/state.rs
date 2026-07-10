//! Shared server state, handed to custom routes via an axum `Extension`
//! layer (forge's custom routes only carry `ForgeState`).

use std::path::PathBuf;
use std::sync::Arc;

use forge_server::EventBus;

use crate::runs::RunManager;

pub struct ServerState {
    /// The runbooks root: every immediate child directory containing a
    /// `playbook.wcl` is a runbook. Canonicalized at startup — the
    /// traversal guard compares canonical prefixes against it.
    pub root: PathBuf,
    /// The config-weave CLI the server shells out to.
    pub config_weave: String,
    /// Optional static test binaries forwarded to `config-weave test`.
    pub test_binary: Option<PathBuf>,
    pub test_binary_windows: Option<PathBuf>,
    pub runs: RunManager,
    pub events: EventBus,
}

pub type SharedState = Arc<ServerState>;
