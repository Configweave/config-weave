//! Shared vocabulary for the testlab's instances: what OS is inside, what
//! one exec produced, and how an external tool reattaches to a live
//! instance. The instances themselves are vmlab machines — see `vmlab`.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

/// As [`run_cmd`], but give up after `deadline`: the child is killed and
/// `Ok(None)` comes back, so the caller can say what timed out — only it
/// knows the machine and what was being asked of it.
///
/// The pipes are drained on threads while the child runs, or a chatty
/// child would fill one and never exit. After a kill they get a short
/// grace to finish rather than an unbounded join: a grandchild that
/// inherited them (a shell's `sleep`, say) can hold them open long after
/// the child itself is gone.
pub fn run_cmd_deadline(
    cmd: &str,
    args: &[&str],
    cwd: Option<&Path>,
    deadline: Duration,
) -> Result<Option<Output>, Diag> {
    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|e| Diag::bare(format!("failed to run `{} {}`: {e}", cmd, args.join(" "))))?;

    fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        rx
    }
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            Ok(None) => std::thread::sleep(DEADLINE_POLL),
            Err(e) => {
                let _ = child.kill();
                return Err(Diag::bare(format!(
                    "failed waiting on `{} {}`: {e}",
                    cmd,
                    args.join(" ")
                )));
            }
        }
    };
    // Exited on its own, so the pipes close with it — unless a grandchild
    // kept them, in which case take what has arrived.
    let collect = |rx: mpsc::Receiver<Vec<u8>>| rx.recv_timeout(PIPE_GRACE).unwrap_or_default();
    Ok(Some(Output {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    }))
}

/// How often [`run_cmd_deadline`] checks whether its child has exited.
const DEADLINE_POLL: Duration = Duration::from_millis(100);

/// How long [`run_cmd_deadline`] waits for a pipe to close once the child
/// has exited.
const PIPE_GRACE: Duration = Duration::from_secs(5);

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
#[derive(Clone)]
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_command_inside_its_deadline_reports_as_run_cmd_would() {
        let out = run_cmd_deadline(
            "sh",
            &["-c", "echo out; echo err >&2; exit 3"],
            None,
            Duration::from_secs(30),
        )
        .unwrap()
        .expect("finished in time");
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "err\n");
    }

    #[test]
    fn a_command_past_its_deadline_is_killed() {
        let start = Instant::now();
        // No `exec`: the sleep is a grandchild holding the pipes, which
        // must not keep the caller waiting once the shell is killed.
        let out = run_cmd_deadline(
            "sh",
            &["-c", "sleep 60; echo late"],
            None,
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(out.is_none(), "timed out");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "returned at the deadline, not when the child would have exited: {:?}",
            start.elapsed()
        );
    }
}
