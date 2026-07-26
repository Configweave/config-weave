//! The testlab's only backend: vmlab. `provision` synthesizes a throwaway
//! one-machine lab in a tempdir (vmlab resolves the lab from the working
//! directory, like git) and brings it up. What that machine is comes from
//! the test's declaration:
//!
//! * `image = "debian:12"` — a vmlab **container**: the OCI image booted in
//!   a micro-VM. Linux only, ready in seconds, and — unlike an unprivileged
//!   container runtime — it holds a full capability set and its own kernel,
//!   so firewall and init-adjacent resources are testable.
//! * `template = "x86_64/ubuntu-24.04"` — a vmlab **VM**. Linux or windows,
//!   a real init system, and the only kind that can reboot.
//!
//! `open_lab` builds a multi-VM lab for scripted scenarios: the machines
//! are declared up front by the author and brought up by name.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use super::backend::{AttachInfo, ExecOutput, GuestOs};
use super::output::stderr_tail;
use crate::diag::Diag;
use crate::model::TestTarget;

/// Guest-exec timeout forwarded to vmlab. Package convergence can
/// legitimately take a while inside an instance (package managers,
/// downloads).
const EXEC_TIMEOUT_SECS: &str = "3600";

/// How long to wait for the guest agent to answer after `up`. `vmlab up`
/// only blocks on readiness for machines something depends on, and our
/// throwaway lab is a single machine with no dependents — so the agent may
/// still be coming up, especially on Windows, which boots well past
/// `osinfo`'s own 30s agent wait. Generous enough for a cold Windows boot.
const READY_DEADLINE: Duration = Duration::from_secs(300);

/// Default readiness wait after a reboot. DC promotion finalizes on the
/// next boot and a Windows guest can take several minutes to answer again.
const REBOOT_DEADLINE: Duration = Duration::from_secs(900);

/// Pause between readiness attempts while the agent is still coming up.
const READY_POLL: Duration = Duration::from_secs(3);

/// The fixed machine name inside a single-instance synthesized lab.
const MACHINE: &str = "box";

/// Where a container instance's payload directory is mounted inside the
/// guest. Everything the runner copies in lives under this path (see
/// `runner::GuestPaths`), so a container's `copy_in` is a host-side write
/// into the bind-mounted directory rather than a guest file transfer.
const CONTAINER_MOUNT: &str = "/weave";

/// The host-side directory bind-mounted at [`CONTAINER_MOUNT`], relative to
/// the synthesized lab root.
const PAYLOAD_DIR: &str = "payload";

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

    /// Provision a running instance of `target`, ready for `exec`. With
    /// `keep`, automatic teardown is disabled for post-mortem debugging.
    pub fn provision(&self, target: &TestTarget, keep: bool) -> Result<VmlabInstance, Diag> {
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
        if matches!(target, TestTarget::Container(_)) {
            std::fs::create_dir_all(dir.path().join(PAYLOAD_DIR))
                .map_err(|e| Diag::bare(format!("cannot create the container payload dir: {e}")))?;
        }
        std::fs::write(dir.path().join("vmlab.wcl"), lab_wcl(&lab_name, target))
            .map_err(|e| Diag::bare(format!("cannot write the lab file: {e}")))?;

        let mut instance = VmlabInstance {
            cmd: self.cmd.clone(),
            dir: dir.keep(),
            lab: lab_name,
            machine: MACHINE.to_string(),
            target: Some(target.clone()),
            os: GuestOs::Linux, // refined below for VMs
            keep,
            owns_lab: true,
            gone: false,
        };

        if !self.quiet {
            eprintln!("bringing up lab {} ({target})…", instance.lab);
        }
        let up = instance.run(&["up"])?;
        if !up.status.success() {
            let msg = format!("cannot bring up a {target}: {}", stderr_tail(&up));
            let _ = instance.teardown();
            return Err(Diag::bare(msg));
        }

        // `up` does not guarantee the agent is answering yet.
        if let Err(d) = instance.wait_ready_inner(READY_DEADLINE) {
            let _ = instance.teardown();
            return Err(Diag::bare(format!(
                "{target} never became ready: {}",
                d.message
            )));
        }
        // A container is always Linux; a VM's OS decides the runner's whole
        // path/shell scheme, so ask the guest agent.
        if instance.is_vm() {
            instance.os = guest_os(&instance.osinfo()?);
        }

        Ok(instance)
    }

    /// Open a declared lab for a scripted scenario from `lab_dir` (a
    /// directory holding a `vmlab.wcl`). Machines are brought up by name on
    /// demand via [`VmlabLab::machine`]. With `keep`, teardown is disabled
    /// for post-mortem debugging.
    pub fn open_lab(&self, lab_dir: &Path, keep: bool) -> Result<VmlabLab, Diag> {
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

        Ok(VmlabLab {
            cmd: self.cmd.clone(),
            quiet: self.quiet,
            dir: dir.keep(),
            lab: lab_name,
            keep,
            gone: false,
        })
    }
}

/// The lab definition for one disposable instance. A container mounts the
/// payload directory read-write at [`CONTAINER_MOUNT`] and runs as root
/// with no entrypoint — the instance exists to be exec'd into, not to run
/// the image's own process. A VM is a plain clone of the template.
pub fn lab_wcl(lab_name: &str, target: &TestTarget) -> String {
    let machine = match target {
        // `mode = :idle` keeps the micro-VM up without starting the image's
        // entrypoint; `user = "0:0"` forces root, since the testlab
        // provisions /weave and converges system state (a no-op for the
        // usual root images, but images like mssql default to non-root).
        TestTarget::Container(image) => format!(
            "  container \"{MACHINE}\" {{\n    image = \"{image}\"\n    \
             mode  = :idle\n    user  = \"0:0\"\n    nic {{ nat = true }}\n    \
             volume {{ host = \"./{PAYLOAD_DIR}\" target = \"{CONTAINER_MOUNT}\" }}\n  }}\n"
        ),
        TestTarget::Vm(template) => format!(
            "  vm \"{MACHINE}\" {{\n    template = \"{template}\"\n    \
             nic {{ nat = true }}\n  }}\n"
        ),
    };
    format!("import <vmlab.wcl>\n\nlab \"{lab_name}\" {{\n{machine}}}\n")
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

/// Map vmlab's `osinfo` JSON to our guest OS classification. The vmlab
/// agent reports `windows`; `mswindows` is what the QEMU guest agent used
/// before vmlab replaced it, and is still accepted so an older template
/// keeps working. Getting this wrong is silent and nasty — the runner would
/// copy the linux binary into a windows guest — so it is unit-tested.
fn guest_os(parsed: &serde_json::Value) -> GuestOs {
    match parsed["id"].as_str() {
        Some("windows") | Some("mswindows") => GuestOs::Windows,
        _ => GuestOs::Linux,
    }
}

/// A declared lab for scenarios: all machines are defined up front in the
/// copied lab file, so `machine` can bring any one up by name on demand
/// (the lab daemon already knows them); the lab owns teardown of them all.
pub struct VmlabLab {
    cmd: String,
    quiet: bool,
    dir: PathBuf,
    lab: String,
    keep: bool,
    gone: bool,
}

impl VmlabLab {
    fn run(&self, args: &[&str]) -> Result<Output, Diag> {
        super::backend::run_cmd(&self.cmd, args, Some(&self.dir))
    }

    /// Bring up the declared machine `name` (idempotent if already up) and
    /// return a handle on it. Scenario labs declare VMs — reboots and
    /// multi-machine topologies are the whole point — so the handle is a VM
    /// handle and its OS comes from the guest agent.
    pub fn machine(&self, name: &str) -> Result<VmlabInstance, Diag> {
        if !self.quiet {
            eprintln!("bringing up {name} in lab {}…", self.lab);
        }
        // The machine is declared in the lab file, so `up <name>` ensures
        // the daemon (full config) and starts just this one — no reload.
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
            machine: name.to_string(),
            target: None, // declared by the author's lab file, not by us
            os: GuestOs::Linux,
            keep: self.keep,
            owns_lab: false, // the lab tears every machine down at once
            gone: false,
        };
        instance.wait_ready_inner(READY_DEADLINE).map_err(|d| {
            Diag::bare(format!(
                "machine '{name}' guest agent never answered: {}",
                d.message
            ))
        })?;
        instance.os = guest_os(&instance.osinfo()?);
        Ok(instance)
    }

    /// Human-readable handle for `--keep` messages.
    pub fn handle(&self) -> String {
        format!("lab {} at {}", self.lab, self.dir.display())
    }

    /// Tear the whole lab down; no-op when kept or already gone.
    pub fn teardown(&mut self) -> Result<(), Diag> {
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
    dir: PathBuf,
    lab: String,
    /// The machine name inside the lab this handle targets.
    machine: String,
    /// What we provisioned it from; `None` for a scenario machine, which
    /// the author's own lab file declares (always a VM).
    target: Option<TestTarget>,
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

    /// Whether this handle addresses a VM (as opposed to a container).
    /// Scenario machines have no target of ours and are always VMs.
    fn is_vm(&self) -> bool {
        !matches!(self.target, Some(TestTarget::Container(_)))
    }

    /// The host-side directory bind-mounted into a container instance.
    fn payload_root(&self) -> PathBuf {
        self.dir.join(PAYLOAD_DIR)
    }

    /// `vmlab osinfo <machine>` as parsed JSON. VM-only: vmlab's `osinfo`
    /// addresses VMs, and a container is Linux by construction.
    fn osinfo(&self) -> Result<serde_json::Value, Diag> {
        let out = self.run(&["osinfo", &self.machine])?;
        if !out.status.success() {
            return Err(Diag::bare(format!(
                "cannot identify the guest OS of {}: {}",
                self.handle(),
                stderr_tail(&out)
            )));
        }
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
            .map_err(|e| Diag::bare(format!("osinfo returned unparseable output: {e}")))
    }

    /// Poll until the guest agent answers, or `deadline` elapses. VMs are
    /// probed with `osinfo` (which also feeds OS detection); containers
    /// with a trivial exec, since `osinfo` is a VM verb.
    fn wait_ready_inner(&self, deadline: Duration) -> Result<(), Diag> {
        let start = Instant::now();
        loop {
            let last = if self.is_vm() {
                match self.osinfo() {
                    Ok(_) => return Ok(()),
                    Err(d) => d.message,
                }
            } else {
                match self.exec(&["/bin/sh", "-c", "exit 0"]) {
                    Ok(o) if o.exit_code == 0 => return Ok(()),
                    Ok(o) => format!("probe exec exited {}", o.exit_code),
                    Err(d) => d.message,
                }
            };
            if start.elapsed() >= deadline {
                return Err(Diag::bare(format!(
                    "guest agent still unavailable after {}s: {last}",
                    deadline.as_secs()
                )));
            }
            std::thread::sleep(READY_POLL);
        }
    }

    /// The instance's guest operating system.
    pub fn os(&self) -> GuestOs {
        self.os
    }

    /// Copy a host file or directory tree to `dest` inside the instance,
    /// creating parent directories.
    ///
    /// For a VM this is a guest file transfer over the agent channel. For a
    /// container it is a plain host-side copy into the bind-mounted payload
    /// directory — everything the runner copies lives under
    /// [`CONTAINER_MOUNT`], so the guest sees it immediately, with modes
    /// (including the binary's executable bit) preserved by the mount.
    pub fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag> {
        if !self.is_vm() {
            let rel = dest.strip_prefix(CONTAINER_MOUNT).and_then(|r| {
                let r = r.trim_start_matches('/');
                (!r.is_empty()).then_some(r)
            });
            let Some(rel) = rel else {
                return Err(Diag::bare(format!(
                    "cannot copy into '{dest}': a container instance only exposes \
                     {CONTAINER_MOUNT}/… to the host"
                )));
            };
            let to = self.payload_root().join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Diag::bare(format!("cannot create {}: {e}", parent.display())))?;
            }
            return if src.is_dir() {
                std::fs::create_dir_all(&to)
                    .map_err(|e| Diag::bare(format!("cannot create {}: {e}", to.display())))?;
                copy_dir_into(src, &to)
            } else {
                std::fs::copy(src, &to).map(|_| ()).map_err(|e| {
                    Diag::bare(format!(
                        "cannot copy {} into the container payload: {e}",
                        src.display()
                    ))
                })
            };
        }

        // vmlab verbs run with the lab tempdir as cwd (it resolves the lab
        // from the working directory), so a relative host src would resolve
        // against the lab dir, not ours — absolutize it first.
        let src_abs = std::fs::canonicalize(src)
            .map_err(|e| Diag::bare(format!("cannot resolve {}: {e}", src.display())))?;
        let src_str = src_abs.display().to_string();
        let target = format!("{}:{dest}", self.machine);
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

    /// Run argv inside the instance. The working directory is
    /// unspecified — the runner always passes absolute paths.
    pub fn exec(&self, argv: &[&str]) -> Result<ExecOutput, Diag> {
        let mut args = if self.is_vm() {
            vec![
                "exec",
                "--timeout",
                EXEC_TIMEOUT_SECS,
                self.machine.as_str(),
                "--",
            ]
        } else {
            vec![
                "container",
                "exec",
                self.machine.as_str(),
                "--timeout",
                EXEC_TIMEOUT_SECS,
                "--",
            ]
        };
        args.extend_from_slice(argv);
        let out = self.run(&args)?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Reboot the instance and wait until it is ready for `exec` again.
    pub fn reboot(&self) -> Result<(), Diag> {
        let verbs: [&str; 3] = if self.is_vm() {
            ["vm", "restart", &self.machine]
        } else {
            ["container", "restart", &self.machine]
        };
        let out = self.run(&verbs)?;
        if !out.status.success() {
            return Err(Diag::bare(format!(
                "cannot restart {}: {}",
                self.handle(),
                stderr_tail(&out)
            )));
        }
        self.wait_ready_inner(REBOOT_DEADLINE)
    }

    /// Block until the instance is ready for `exec`, up to `secs`.
    pub fn wait_ready(&self, secs: u64) -> Result<(), Diag> {
        self.wait_ready_inner(Duration::from_secs(secs))
    }

    /// Human-readable handle for `--keep` messages.
    pub fn handle(&self) -> String {
        let kind = if self.is_vm() { "vm" } else { "container" };
        let from = match &self.target {
            Some(t) => format!(" from {}", t.reference()),
            None => String::new(),
        };
        format!(
            "{kind} \"{}\" in lab {} at {}{from}",
            self.machine,
            self.lab,
            self.dir.display()
        )
    }

    /// Raw attach/cleanup coordinates for external tooling (the
    /// `instance_ready` event of `--events-ndjson`).
    pub fn attach_info(&self) -> AttachInfo {
        AttachInfo::Vmlab {
            lab_dir: self.dir.display().to_string(),
            lab: self.lab.clone(),
            machine: self.machine.clone(),
            machine_kind: if self.is_vm() { "vm" } else { "container" },
            source: self
                .target
                .as_ref()
                .map(|t| t.reference().to_string())
                .unwrap_or_default(),
        }
    }

    /// Tear down the instance; no-op when kept or already gone.
    pub fn teardown(&mut self) -> Result<(), Diag> {
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
    fn vm_lab_wcl_shape() {
        let wcl = lab_wcl(
            "cw-test-Ab12",
            &TestTarget::Vm("x86_64/linux-modern".into()),
        );
        assert!(wcl.starts_with("import <vmlab.wcl>\n"), "{wcl}");
        assert!(wcl.contains("lab \"cw-test-Ab12\""), "{wcl}");
        assert!(wcl.contains("vm \"box\""), "{wcl}");
        assert!(wcl.contains("template = \"x86_64/linux-modern\""), "{wcl}");
        assert!(wcl.contains("nic { nat = true }"), "{wcl}");
        assert!(
            !wcl.contains("volume"),
            "a VM needs no payload mount: {wcl}"
        );
    }

    #[test]
    fn container_lab_wcl_mounts_the_payload_and_idles_the_image() {
        let wcl = lab_wcl("cw-test-Ab12", &TestTarget::Container("debian:12".into()));
        assert!(wcl.contains("container \"box\""), "{wcl}");
        assert!(wcl.contains("image = \"debian:12\""), "{wcl}");
        // Without :idle the image's own entrypoint would run instead of
        // leaving the instance available to exec into.
        assert!(wcl.contains("mode  = :idle"), "{wcl}");
        assert!(wcl.contains("user  = \"0:0\""), "{wcl}");
        assert!(
            wcl.contains("volume { host = \"./payload\" target = \"/weave\" }"),
            "{wcl}"
        );
    }

    #[test]
    fn guest_os_recognizes_both_windows_agent_ids() {
        let os = |id: &str| guest_os(&serde_json::json!({ "id": id }));
        // What the vmlab agent reports…
        assert_eq!(os("windows"), GuestOs::Windows);
        // …and what the QEMU guest agent reported before it.
        assert_eq!(os("mswindows"), GuestOs::Windows);
        assert_eq!(os("ubuntu"), GuestOs::Linux);
        assert_eq!(os("alpine"), GuestOs::Linux);
        assert_eq!(guest_os(&serde_json::json!({})), GuestOs::Linux);
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
