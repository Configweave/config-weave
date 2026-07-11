//! Shared server state, handed to custom routes via an axum `Extension`
//! layer (forge's custom routes only carry `ForgeState`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_server::EventBus;

use crate::runs::RunManager;
use crate::sysruns::SysRunManager;
use crate::systems::ServiceDef;

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
    /// `{root}/services.wcl` — the service inventory, mirrored in memory
    /// and regenerated on every mutation.
    pub services_path: PathBuf,
    pub services: Mutex<Vec<ServiceDef>>,
    /// Deployable static binaries keyed `"{os}-{arch}"` (dist/ naming:
    /// `linux-x86_64`, `windows-x86_64`), for direct-system runs.
    pub deploy_binaries: HashMap<String, PathBuf>,
    pub sysruns: SysRunManager,
    /// The package repository (`--packages-dir`): a folder of package
    /// dirs, each with a package.wcl. None = feature hidden.
    pub packages_dir: Option<PathBuf>,
    pub pkg_wrapper: crate::packages::WrapperCache,
    /// `{root}/repos.wcl` — remote package repositories, mirrored in
    /// memory and regenerated on every mutation.
    pub repos_path: PathBuf,
    pub repos: Mutex<Vec<crate::repos::RepoDef>>,
    /// `{root}/.repo-cache` — one clone per remote repository.
    pub repo_cache: PathBuf,
    /// Serializes git operations on the cache: concurrent syncs of one
    /// repo would race `reset --hard`.
    pub repo_git_lock: tokio::sync::Mutex<()>,
    /// Query backends for the per-service Monitoring/Logs tabs; None =
    /// the proxy endpoints answer 503 and the tabs show "not configured".
    pub prometheus_url: Option<url::Url>,
    pub loki_url: Option<url::Url>,
    /// Shared client for the Prometheus/Loki proxy queries.
    pub http: reqwest::Client,
}

pub type SharedState = Arc<ServerState>;
