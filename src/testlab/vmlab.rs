//! vmlab implementation of the test backend: shells out to the vmlab
//! CLI. A single-instance `provision` synthesizes a throwaway one-VM lab
//! in a tempdir (vmlab resolves the lab from the working directory, like
//! git), brings it up, and asks the guest agent which OS is inside — so a
//! single `image` (a vmlab template ref) drives both Linux and Windows
//! tests. `open_lab` builds a multi-VM lab for scripted scenarios: VMs are
//! added incrementally to the lab file and brought up by name.

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use super::backend::{AttachInfo, ExecOutput, GuestOs, TestBackend, TestInstance, TestLab};
use super::output::stderr_tail;
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

/// Default readiness wait after a reboot. DC promotion finalizes on the
/// next boot and a Windows guest can take several minutes to answer again.
const REBOOT_DEADLINE: Duration = Duration::from_secs(900);

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
        let cmd = super::backend::discover_cli(
            "CONFIG_WEAVE_VMLAB_CMD",
            &["vmlab"],
            "--version",
            |tried| {
                format!(
                    "config-weave test needs a working vmlab CLI (tried: {}); install vmlab, \
                     or point CONFIG_WEAVE_VMLAB_CMD at one",
                    tried.join(", ")
                )
            },
        )?;
        Ok(VmlabBackend { cmd, quiet })
    }
}

/// The fixed VM name inside a single-instance synthesized lab.
const VM: &str = "box";

/// The lab definition for one disposable instance: a single VM cloned
/// from `image` with internet egress, everything else template defaults.
pub fn lab_wcl(lab_name: &str, image: &str) -> String {
    format!(
        "import <vmlab.wcl>\n\nlab \"{lab_name}\" {{\n  vm \"{VM}\" {{\n    \
         template = \"{image}\"\n    nic {{ nat = true }}\n  }}\n}}\n"
    )
}

/// Recursively copy the contents of `src` into the existing dir `dst`.
fn copy_dir_into(src: &Path, dst: &Path) -> Result<(), Diag> {
    let fail = |e: std::io::Error| Diag::bare(format!("cannot copy {}: {e}", src.display()));
    for entry in std::fs::read_dir(src).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            std::fs::create_dir_all(&to).map_err(fail)?;
            copy_dir_into(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).map_err(fail)?;
        }
    }
    Ok(())
}

/// Rewrite the first `lab "<name>"` label, appending `-<suffix>` so a
/// throwaway scenario run never collides with a same-named lab in vmlab's
/// registry. Returns the rewritten source and the new lab name.
fn rewrite_lab_name(wcl: &str, suffix: &str) -> Option<(String, String)> {
    let key = "lab \"";
    let i = wcl.find(key)?;
    let start = i + key.len();
    let end = start + wcl[start..].find('"')?;
    let new_name = format!("{}-{suffix}", &wcl[start..end]);
    let new_wcl = format!("{}{}{}", &wcl[..start], new_name, &wcl[end..]);
    Some((new_wcl, new_name))
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
            vm: VM.to_string(),
            image: image.to_string(),
            os: GuestOs::Linux, // refined below
            keep,
            owns_lab: true,
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
            let _ = instance.teardown();
            return Err(Diag::bare(msg));
        }

        // `up` does not guarantee the agent is ready, so poll osinfo (it
        // also decides the runner's whole path/shell scheme).
        let parsed = match instance.wait_osinfo(OSINFO_DEADLINE) {
            Ok(v) => v,
            Err(d) => {
                let _ = instance.teardown();
                return Err(Diag::bare(format!(
                    "cannot identify the guest OS of '{image}': {}",
                    d.message
                )));
            }
        };
        instance.os = guest_os(&parsed);

        Ok(Box::new(instance))
    }

    fn open_lab(&self, lab_dir: &Path, keep: bool) -> Result<Box<dyn TestLab>, Diag> {
        // Copy the author's lab dir into a throwaway tempdir and give the
        // lab a unique name, so a run never disturbs a same-named lab in
        // vmlab's registry and `vmlab destroy` cleans a copy.
        let dir = tempfile::Builder::new()
            .prefix("cw-scenario-")
            .tempdir()
            .map_err(|e| Diag::bare(format!("cannot create a lab tempdir: {e}")))?;
        copy_dir_into(lab_dir, dir.path())?;

        let wcl_path = dir.path().join("vmlab.wcl");
        let wcl = std::fs::read_to_string(&wcl_path).map_err(|e| {
            Diag::bare(format!(
                "lab dir {} has no readable vmlab.wcl: {e}",
                lab_dir.display()
            ))
        })?;
        let suffix = super::output::rand_suffix();
        let (new_wcl, lab_name) = rewrite_lab_name(&wcl, &suffix)
            .ok_or_else(|| Diag::bare("vmlab.wcl has no `lab \"…\"` block".to_string()))?;
        std::fs::write(&wcl_path, new_wcl)
            .map_err(|e| Diag::bare(format!("cannot rewrite the lab file: {e}")))?;

        Ok(Box::new(VmlabLab {
            cmd: self.cmd.clone(),
            quiet: self.quiet,
            dir: dir.keep(),
            lab: lab_name,
            keep,
            gone: false,
        }))
    }
}

/// Map vmlab's `osinfo` JSON to our guest OS classification.
fn guest_os(parsed: &serde_json::Value) -> GuestOs {
    if parsed["id"].as_str() == Some("mswindows") {
        GuestOs::Windows
    } else {
        GuestOs::Linux
    }
}

/// A declared lab for scenarios: all VMs are defined up front in the copied
/// lab file, so `machine` can bring any one up by name on demand (the lab
/// daemon already knows them); the lab owns teardown of them all.
pub struct VmlabLab {
    cmd: String,
    quiet: bool,
    dir: std::path::PathBuf,
    lab: String,
    keep: bool,
    gone: bool,
}

impl VmlabLab {
    fn run(&self, args: &[&str]) -> Result<Output, Diag> {
        super::backend::run_cmd(&self.cmd, args, Some(&self.dir))
    }
}

impl TestLab for VmlabLab {
    fn machine(&self, name: &str) -> Result<Box<dyn TestInstance>, Diag> {
        if !self.quiet {
            eprintln!("bringing up {name} in lab {}…", self.lab);
        }
        // The VM is declared in the lab file, so `up <name>` ensures the
        // daemon (full config) and starts just this one — no reload needed.
        let up = self.run(&["up", name])?;
        if !up.status.success() {
            return Err(Diag::bare(format!(
                "cannot bring up machine '{name}': {}",
                stderr_tail(&up)
            )));
        }
        let mut instance = VmlabInstance {
            cmd: self.cmd.clone(),
            dir: self.dir.clone(),
            lab: self.lab.clone(),
            vm: name.to_string(),
            image: String::new(),
            os: GuestOs::Linux,
            keep: self.keep,
            owns_lab: false, // the lab tears every machine down at once
            gone: false,
        };
        let parsed = instance.wait_osinfo(OSINFO_DEADLINE).map_err(|d| {
            Diag::bare(format!(
                "machine '{name}' guest agent never answered: {}",
                d.message
            ))
        })?;
        instance.os = guest_os(&parsed);
        Ok(Box::new(instance))
    }

    fn handle(&self) -> String {
        format!("lab {} at {}", self.lab, self.dir.display())
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

impl Drop for VmlabLab {
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

pub struct VmlabInstance {
    cmd: String,
    /// The synthesized lab root; vmlab verbs run with this as cwd.
    dir: std::path::PathBuf,
    lab: String,
    /// The VM name inside the lab this handle targets.
    vm: String,
    image: String,
    os: GuestOs,
    keep: bool,
    /// True when this handle owns the whole lab (single-instance path) and
    /// must destroy it on teardown; false when a `VmlabLab` owns teardown.
    owns_lab: bool,
    gone: bool,
}

impl VmlabInstance {
    fn run(&self, args: &[&str]) -> Result<Output, Diag> {
        super::backend::run_cmd(&self.cmd, args, Some(&self.dir))
    }

    /// Poll `vmlab osinfo <vm>` until the guest agent answers with
    /// parseable JSON, or `deadline` elapses.
    fn wait_osinfo(&self, deadline: Duration) -> Result<serde_json::Value, Diag> {
        let start = Instant::now();
        loop {
            let out = self.run(&["osinfo", &self.vm])?;
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
            if start.elapsed() >= deadline {
                return Err(Diag::bare(format!(
                    "guest agent still unavailable after {}s: {last}",
                    deadline.as_secs()
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
        let target = format!("{}:{dest}", self.vm);
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
        let mut args = vec![
            "exec",
            "--timeout",
            EXEC_TIMEOUT_SECS,
            self.vm.as_str(),
            "--",
        ];
        args.extend_from_slice(argv);
        let out = self.run(&args)?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn reboot(&self) -> Result<(), Diag> {
        let out = self.run(&["vm", "restart", &self.vm])?;
        if !out.status.success() {
            return Err(Diag::bare(format!(
                "cannot restart {}: {}",
                self.handle(),
                stderr_tail(&out)
            )));
        }
        self.wait_osinfo(REBOOT_DEADLINE).map(|_| ())
    }

    fn wait_ready(&self, secs: u64) -> Result<(), Diag> {
        self.wait_osinfo(Duration::from_secs(secs)).map(|_| ())
    }

    fn handle(&self) -> String {
        format!(
            "vm {} in lab {} at {} (template {})",
            self.vm,
            self.lab,
            self.dir.display(),
            self.image
        )
    }

    fn attach_info(&self) -> AttachInfo {
        AttachInfo::Vmlab {
            lab_dir: self.dir.display().to_string(),
            lab: self.lab.clone(),
            machine: self.vm.clone(),
            template: self.image.clone(),
        }
    }

    fn teardown(&mut self) -> Result<(), Diag> {
        if !self.owns_lab || self.keep || self.gone {
            self.gone = true;
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

/// Best-effort cleanup on panic or early `?`: a kept or lab-owned instance
/// survives, a single-instance lab is destroyed.
impl Drop for VmlabInstance {
    fn drop(&mut self) {
        if self.owns_lab && !self.keep && !self.gone {
            let _ = Command::new(&self.cmd)
                .arg("destroy")
                .current_dir(&self.dir)
                .output();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
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

    #[test]
    fn lab_name_rewrite_is_unique_and_preserves_the_rest() {
        let wcl = "import <vmlab.wcl>\n\nlab \"ad-lab\" {\n  vm \"dc01\" { template = \"x\" }\n}\n";
        let (out, name) = rewrite_lab_name(wcl, "Xy9").unwrap();
        assert_eq!(name, "ad-lab-Xy9");
        assert!(out.contains("lab \"ad-lab-Xy9\""), "{out}");
        assert!(out.contains("vm \"dc01\""), "{out}");
        assert!(rewrite_lab_name("no lab here", "Xy9").is_none());
    }
}
