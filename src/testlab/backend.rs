//! The backend seam: anything that can run a disposable instance of an
//! image, with just enough surface for the test runner — copy files in,
//! exec argv, tear down. Docker (linux containers) and vmlab (linux or
//! windows VMs) implement it.

use std::path::Path;
use std::process::{Command, Output};

use crate::diag::Diag;

/// Find a working CLI for a backend: the `$env_var` override if set and
/// non-empty, otherwise each of `candidates`, probed with `probe_arg`
/// (e.g. `version` / `--version`) so a CLI present but non-functional —
/// say, a container tool with no running daemon — also fails here. The
/// first that exits zero wins; otherwise `not_found` (which should name
/// the tried candidates and the override env var) becomes the error.
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

/// `Sync` so a single backend can be shared by reference across the
/// parallel group-runner threads; both implementations are plain structs
/// (`cmd`, `quiet`) that already qualify.
pub trait TestBackend: Sync {
    /// Backend id as written in `backend = "…"` test fields.
    fn name(&self) -> &'static str;

    /// Provision a running instance of `image`, ready for `exec`. With
    /// `keep`, automatic teardown is disabled for post-mortem debugging.
    fn provision(&self, image: &str, keep: bool) -> Result<Box<dyn TestInstance>, Diag>;

    /// Open a declared lab for a scripted scenario from `lab_dir` (a
    /// directory holding the backend's lab definition). VMs are brought up
    /// by name on demand via `TestLab::machine`. With `keep`, teardown is
    /// disabled for post-mortem debugging. Only the vmlab backend supports
    /// this.
    fn open_lab(&self, lab_dir: &Path, keep: bool) -> Result<Box<dyn TestLab>, Diag>;
}

/// A declared multi-machine lab driven by a scenario script. Its VMs are
/// defined up front in the lab file; `machine` brings one up by name and
/// returns a handle; teardown removes the whole lab.
pub trait TestLab {
    /// Bring up the declared machine `name` (idempotent if already up) and
    /// return a handle on it.
    fn machine(&self, name: &str) -> Result<Box<dyn TestInstance>, Diag>;

    /// Human-readable handle for `--keep` messages.
    fn handle(&self) -> String;

    /// Tear the whole lab down; no-op when kept or already gone.
    fn teardown(&mut self) -> Result<(), Diag>;
}

pub trait TestInstance {
    /// The instance's guest operating system.
    fn os(&self) -> GuestOs;

    /// Copy a host file or directory tree to `dest` inside the instance,
    /// creating parent directories.
    fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag>;

    /// Run argv inside the instance. The working directory is
    /// unspecified — the runner always passes absolute paths.
    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, Diag>;

    /// Reboot the instance and wait until it is ready for `exec` again.
    /// Only the vmlab backend supports this; docker returns an error.
    fn reboot(&self) -> Result<(), Diag>;

    /// Block until the instance is ready for `exec`, up to `secs`. vmlab
    /// polls the guest agent; docker is always ready (a no-op).
    fn wait_ready(&self, secs: u64) -> Result<(), Diag>;

    /// Human-readable handle for `--keep` messages.
    fn handle(&self) -> String;

    /// Tear down the instance; no-op when kept or already gone.
    fn teardown(&mut self) -> Result<(), Diag>;
}
