//! Shared vocabulary for the testlab's instances: what OS is inside, what
//! one exec produced, and how an external tool reattaches to a live
//! instance. The instances themselves are vmlab machines — see `vmlab`.

use std::path::Path;
use std::process::{Command, Output};

use crate::diag::Diag;

/// Find a working CLI: the `$env_var` override if set and non-empty,
/// otherwise each of `candidates`, probed with `probe_arg` (e.g. `version`
/// / `--version`) so a CLI present but non-functional also fails here. The
/// first that exits zero wins; otherwise `not_found` (which should name the
/// tried candidates and the override env var) becomes the error.
pub fn discover_cli(
    env_var: &str,
    default_candidates: &[&str],
    probe_arg: &str,
    not_found: impl FnOnce(&[String]) -> String,
) -> Result<String, Diag> {
    let candidates: Vec<String> = match std::env::var(env_var) {
        Ok(c) if !c.is_empty() => vec![c],
        _ => default_candidates.iter().map(|s| s.to_string()).collect(),
    };
    for cmd in &candidates {
        let works = Command::new(cmd)
            .arg(probe_arg)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if works {
            return Ok(cmd.clone());
        }
    }
    Err(Diag::bare(not_found(&candidates)))
}

/// Spawn `cmd args` (optionally in `cwd`) and capture its output, mapping a
/// spawn failure to a `Diag`. A nonzero exit is success here — the caller
/// inspects `status`.
pub fn run_cmd(cmd: &str, args: &[&str], cwd: Option<&Path>) -> Result<Output, Diag> {
    let mut command = Command::new(cmd);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command
        .output()
        .map_err(|e| Diag::bare(format!("failed to run `{} {}`: {e}", cmd, args.join(" "))))
}

/// What the host can do to a live instance — and the only thing the
/// `guest` module needs from one.
///
/// Narrow on purpose. Everything interesting sits *above* this line: the
/// in-instance path scheme, which shell says what, how config-weave is
/// invoked in there, and how its output is read back. Putting the seam
/// below all of that means a scripted fake can stand in for a machine, so
/// those rules — including every Windows branch — are exercised by
/// `cargo test` on a host with no hypervisor.
///
/// This is deliberately *not* the backend trait removed in 2f60bf5. That
/// one sat at provisioning and leaked: `reboot` and `wait_ready` were
/// documented as unsupported on one of its two adapters. These three are
/// uniform — every instance vmlab hands back supports all of them the
/// same way — so there is no capability to interrogate. See
/// `docs/adr/0001-testlab-transport-seam.md`.
pub trait Transport {
    fn os(&self) -> GuestOs;
    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, Diag>;
    fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag>;
}

/// The operating system running inside an instance. The runner derives
/// the in-instance path scheme, setup shell, and which test binary to
/// copy in from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestOs {
    Linux,
    Windows,
}

/// Output of one exec inside an instance. A nonzero exit code is data
/// for the caller, never an `Err` — errors mean the transport failed.
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// What a troubleshooting session or external cleanup needs to reach a
/// live instance. Raw, untruncated identifiers on purpose — `handle()`
/// stays the human-readable (shortened) form.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachInfo {
    Vmlab {
        /// The synthesized lab root; vmlab verbs run with this as cwd.
        lab_dir: String,
        lab: String,
        machine: String,
        /// "vm" or "container" — which vmlab verb group reaches it.
        machine_kind: &'static str,
        /// The template (VMs) or OCI image (containers) it was made from.
        source: String,
    },
}
