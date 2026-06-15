//! vmlab implementation of the test backend: shells out to the vmlab
//! CLI. Each provision synthesizes a throwaway one-VM lab in a tempdir
//! (vmlab resolves the lab from the working directory, like git), brings
//! it up, and asks the guest agent which OS is inside — so a single
//! `image` (a vmlab template ref) drives both Linux and Windows tests.

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use super::backend::{ExecOutput, GuestOs, TestBackend, TestInstance};
use crate::diag::Diag;

/// Guest-exec timeout forwarded to `vmlab exec`. Package convergence can
/// legitimately take a while inside a VM (package managers, downloads).
const EXEC_TIMEOUT_SECS: &str = "3600";

/// How long to wait for the guest agent to answer `osinfo` after `up`.
/// `vmlab up` only blocks on readiness for VMs something depends on, and
/// our throwaway lab is a single VM with no dependents — so the agent may
/// still be coming up, especially on Windows, which boots well past
/// `osinfo`'s own 30s agent wait. Generous enough for a cold Windows boot.
const OSINFO_DEADLINE: Duration = Duration::from_secs(300);

/// Pause between `osinfo` attempts while the agent is still coming up.
const OSINFO_POLL: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub struct VmlabBackend {
    cmd: String,
    /// Suppress stderr progress lines (JSON output mode).
    quiet: bool,
}

impl VmlabBackend {
    /// Find a working vmlab CLI: `$CONFIG_WEAVE_VMLAB_CMD`, then `vmlab`
    /// — probed with `<cmd> --version`.
    pub fn discover(quiet: bool) -> Result<VmlabBackend, Diag> {
        let candidates: Vec<String> = match std::env::var("CONFIG_WEAVE_VMLAB_CMD") {
            Ok(c) if !c.is_empty() => vec![c],
            _ => vec!["vmlab".into()],
        };
        for cmd in &candidates {
            let works = Command::new(cmd)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if works {
                return Ok(VmlabBackend {
                    cmd: cmd.clone(),
                    quiet,
                });
            }
        }
        Err(Diag::bare(format!(
            "config-weave test needs a working vmlab CLI (tried: {}); install vmlab, \
             or point CONFIG_WEAVE_VMLAB_CMD at one",
            candidates.join(", ")
        )))
    }
}

/// The fixed VM name inside every synthesized lab.
const VM: &str = "box";

/// The lab definition for one disposable instance: a single VM cloned
/// from `image` with internet egress, everything else template defaults.
pub fn lab_wcl(lab_name: &str, image: &str) -> String {
    format!(
        "import <vmlab.wcl>\n\nlab \"{lab_name}\" {{\n  vm \"{VM}\" {{\n    \
         template = \"{image}\"\n    nic {{ nat = true }}\n  }}\n}}\n"
    )
}

impl TestBackend for VmlabBackend {
    fn name(&self) -> &'static str {
        "vmlab"
    }

    fn provision(&self, image: &str, keep: bool) -> Result<Box<dyn TestInstance>, Diag> {
        // The tempdir's unique suffix doubles as the lab name, keeping
        // concurrent runs out of each other's way in vmlab's registry.
        let dir = tempfile::Builder::new()
            .prefix("cw-test-")
            .tempdir()
            .map_err(|e| Diag::bare(format!("cannot create a lab tempdir: {e}")))?;
        let lab_name = dir
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "cw-test".into());
        std::fs::write(dir.path().join("vmlab.wcl"), lab_wcl(&lab_name, image))
            .map_err(|e| Diag::bare(format!("cannot write the lab file: {e}")))?;

        let mut instance = VmlabInstance {
            cmd: self.cmd.clone(),
            dir: dir.keep(),
            lab: lab_name,
            image: image.to_string(),
            os: GuestOs::Linux, // refined below
            keep,
            gone: false,
        };

        if !self.quiet {
            eprintln!("bringing up lab {} ({image})…", instance.lab);
        }
        let up = instance.run(&["up"])?;
        if !up.status.success() {
            let msg = format!(
                "cannot bring up a VM from template '{image}': {}",
                stderr_tail(&up)
            );
            // A failed `up` may have left a half-started lab behind.
            let _ = instance.teardown();
            return Err(Diag::bare(msg));
        }

        // `up` does not guarantee the agent is ready for a dependency-free
        // single-VM lab, so poll osinfo until it answers (it also decides
        // the runner's whole path/shell scheme).
        let parsed = match instance.wait_osinfo() {
            Ok(v) => v,
            Err(d) => {
                let _ = instance.teardown();
                return Err(Diag::bare(format!(
                    "cannot identify the guest OS of '{image}': {}",
                    d.message
                )));
            }
        };
        instance.os = if parsed["id"].as_str() == Some("mswindows") {
            GuestOs::Windows
        } else {
            GuestOs::Linux
        };

        Ok(Box::new(instance))
    }
}

pub struct VmlabInstance {
    cmd: String,
    /// The synthesized lab root; vmlab verbs run with this as cwd.
    dir: std::path::PathBuf,
    lab: String,
    image: String,
    os: GuestOs,
    keep: bool,
    gone: bool,
}

impl VmlabInstance {
    fn run(&self, args: &[&str]) -> Result<Output, Diag> {
        Command::new(&self.cmd)
            .args(args)
            .current_dir(&self.dir)
            .output()
            .map_err(|e| {
                Diag::bare(format!(
                    "failed to run `{} {}`: {e}",
                    self.cmd,
                    args.join(" ")
                ))
            })
    }

    /// Poll `vmlab osinfo` until the guest agent answers with parseable
    /// JSON, or `OSINFO_DEADLINE` elapses. Each attempt already blocks up
    /// to vmlab's own ~30s agent wait, so a short sleep between attempts is
    /// enough to cover a slow (Windows) boot without busy-looping.
    fn wait_osinfo(&self) -> Result<serde_json::Value, Diag> {
        let start = Instant::now();
        loop {
            let out = self.run(&["osinfo", VM])?;
            if out.status.success()
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(
                    String::from_utf8_lossy(&out.stdout).trim(),
                )
            {
                return Ok(v);
            }
            let last = if out.status.success() {
                "osinfo returned unparseable output".to_string()
            } else {
                stderr_tail(&out)
            };
            if start.elapsed() >= OSINFO_DEADLINE {
                return Err(Diag::bare(format!(
                    "guest agent still unavailable after {}s: {last}",
                    OSINFO_DEADLINE.as_secs()
                )));
            }
            std::thread::sleep(OSINFO_POLL);
        }
    }
}

impl TestInstance for VmlabInstance {
    fn os(&self) -> GuestOs {
        self.os
    }

    fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag> {
        // vmlab verbs run with the lab tempdir as cwd (it resolves the lab
        // from the working directory), so a relative host src would resolve
        // against the lab dir, not ours — absolutize it first.
        let src_abs = std::fs::canonicalize(src)
            .map_err(|e| Diag::bare(format!("cannot resolve {}: {e}", src.display())))?;
        let src_str = src_abs.display().to_string();
        let target = format!("{VM}:{dest}");
        let out = self.run(&["cp", &src_str, &target])?;
        if !out.status.success() {
            return Err(Diag::bare(format!(
                "cannot copy {src_str} into {}: {}",
                self.handle(),
                stderr_tail(&out)
            )));
        }
        Ok(())
    }

    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, Diag> {
        let mut args = vec!["exec", "--timeout", EXEC_TIMEOUT_SECS, VM, "--"];
        args.extend_from_slice(argv);
        let out = self.run(&args)?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn handle(&self) -> String {
        format!(
            "lab {} at {} (template {})",
            self.lab,
            self.dir.display(),
            self.image
        )
    }

    fn teardown(&mut self) -> Result<(), Diag> {
        if self.keep || self.gone {
            return Ok(());
        }
        let out = self.run(&["destroy"])?;
        self.gone = true;
        let dir_result = std::fs::remove_dir_all(&self.dir);
        if !out.status.success() {
            return Err(Diag::bare(format!(
                "cannot destroy {}: {}",
                self.handle(),
                stderr_tail(&out)
            )));
        }
        dir_result.map_err(|e| Diag::bare(format!("cannot remove {}: {e}", self.dir.display())))
    }
}

/// Best-effort cleanup on panic or early `?`: a kept instance survives,
/// everything else is destroyed.
impl Drop for VmlabInstance {
    fn drop(&mut self) {
        if !self.keep && !self.gone {
            let _ = Command::new(&self.cmd)
                .arg("destroy")
                .current_dir(&self.dir)
                .output();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// The interesting last lines of a CLI's stderr, for diagnostics.
fn stderr_tail(out: &Output) -> String {
    let s = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = s.trim().lines().rev().take(3).collect();
    let tail: Vec<&str> = lines.into_iter().rev().collect();
    if tail.is_empty() {
        format!("(no stderr, exit {})", out.status.code().unwrap_or(-1))
    } else {
        tail.join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_failure_names_candidates() {
        // SAFETY: the only test in the binary touching this variable.
        unsafe { std::env::set_var("CONFIG_WEAVE_VMLAB_CMD", "/nonexistent/vmlabctl") };
        let err = VmlabBackend::discover(true).unwrap_err();
        unsafe { std::env::remove_var("CONFIG_WEAVE_VMLAB_CMD") };
        assert!(
            err.message.contains("/nonexistent/vmlabctl"),
            "{}",
            err.message
        );
        assert!(err.message.contains("vmlab"), "{}", err.message);
    }

    #[test]
    fn lab_wcl_shape() {
        let wcl = lab_wcl("cw-test-Ab12", "x86_64/linux-modern");
        assert!(wcl.starts_with("import <vmlab.wcl>\n"), "{wcl}");
        assert!(wcl.contains("lab \"cw-test-Ab12\""), "{wcl}");
        assert!(wcl.contains("template = \"x86_64/linux-modern\""), "{wcl}");
        assert!(wcl.contains("nic { nat = true }"), "{wcl}");
    }
}
