//! Docker implementation of the test backend: shells out to the docker
//! CLI (podman works identically). Images must contain `sleep` and `sh`
//! — true of every libc-based distro image; distroless/scratch images
//! are unsupported in v1.

use std::path::Path;
use std::process::{Command, Output};

use super::backend::{ExecOutput, TestBackend, TestInstance};
use crate::diag::Diag;

#[derive(Debug)]
pub struct DockerBackend {
    cmd: String,
    /// Suppress stderr progress lines (JSON output mode).
    quiet: bool,
}

impl DockerBackend {
    /// Find a working container CLI: `$CONFIG_WEAVE_CONTAINER_CMD`, then
    /// `docker`, then `podman` — probed with `<cmd> version` so a CLI
    /// without a running daemon also fails here, with one clear message.
    pub fn discover(quiet: bool) -> Result<DockerBackend, Diag> {
        let candidates: Vec<String> = match std::env::var("CONFIG_WEAVE_CONTAINER_CMD") {
            Ok(c) if !c.is_empty() => vec![c],
            _ => vec!["docker".into(), "podman".into()],
        };
        for cmd in &candidates {
            let works = Command::new(cmd)
                .arg("version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if works {
                return Ok(DockerBackend {
                    cmd: cmd.clone(),
                    quiet,
                });
            }
        }
        Err(Diag::bare(format!(
            "config-weave test needs a working container CLI (tried: {}); install \
             docker or podman, or point CONFIG_WEAVE_CONTAINER_CMD at one",
            candidates.join(", ")
        )))
    }

    fn run(&self, args: &[&str]) -> Result<Output, Diag> {
        Command::new(&self.cmd).args(args).output().map_err(|e| {
            Diag::bare(format!(
                "failed to run `{} {}`: {e}",
                self.cmd,
                args.join(" ")
            ))
        })
    }

    /// Make the image available locally, pulling when missing.
    fn ensure_image(&self, image: &str) -> Result<(), Diag> {
        if self.run(&["image", "inspect", image])?.status.success() {
            return Ok(());
        }
        if !self.quiet {
            eprintln!("pulling {image}…");
        }
        let pull = self.run(&["pull", image])?;
        if pull.status.success() {
            Ok(())
        } else {
            Err(Diag::bare(format!(
                "cannot pull image '{image}': {}",
                stderr_tail(&pull)
            )))
        }
    }
}

impl TestBackend for DockerBackend {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn provision(&self, image: &str, keep: bool) -> Result<Box<dyn TestInstance>, Diag> {
        self.ensure_image(image)?;
        // `--entrypoint sleep` neutralizes whatever the image would run;
        // a literal second count instead of `infinity` keeps busybox
        // images working. `--rm` only when the container will be torn
        // down, so `--keep` leaves something inspectable behind.
        let mut args = vec!["run", "-d"];
        if !keep {
            args.push("--rm");
        }
        args.extend(["--entrypoint", "sleep", image, "2147483647"]);
        let out = self.run(&args)?;
        if !out.status.success() {
            return Err(Diag::bare(format!(
                "cannot start a container from '{image}': {}",
                stderr_tail(&out)
            )));
        }
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if id.is_empty() {
            return Err(Diag::bare(format!(
                "`{} run` for image '{image}' printed no container id",
                self.cmd
            )));
        }
        let instance = DockerInstance {
            cmd: self.cmd.clone(),
            id,
            image: image.to_string(),
            keep,
            gone: false,
        };
        // The exec working directory must exist before the first exec.
        let mkdir = instance.run(&["exec", &instance.id, "mkdir", "-p", "/weave"])?;
        if !mkdir.status.success() {
            return Err(Diag::bare(format!(
                "cannot create /weave inside the container (does '{image}' have a \
                 shell userland?): {}",
                stderr_tail(&mkdir)
            )));
        }
        Ok(Box::new(instance))
    }
}

pub struct DockerInstance {
    cmd: String,
    id: String,
    image: String,
    keep: bool,
    gone: bool,
}

impl DockerInstance {
    fn run(&self, args: &[&str]) -> Result<Output, Diag> {
        Command::new(&self.cmd).args(args).output().map_err(|e| {
            Diag::bare(format!(
                "failed to run `{} {}`: {e}",
                self.cmd,
                args.join(" ")
            ))
        })
    }
}

impl TestInstance for DockerInstance {
    fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag> {
        if let Some((parent, _)) = dest.rsplit_once('/')
            && !parent.is_empty()
        {
            let out = self.run(&["exec", &self.id, "mkdir", "-p", parent])?;
            if !out.status.success() {
                return Err(Diag::bare(format!(
                    "cannot create {parent} inside {}: {}",
                    self.handle(),
                    stderr_tail(&out)
                )));
            }
        }
        let src_str = src.display().to_string();
        let target = format!("{}:{dest}", self.id);
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
        let mut args = vec!["exec", "-w", "/weave", self.id.as_str()];
        args.extend_from_slice(argv);
        let out = self.run(&args)?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn handle(&self) -> String {
        let short = &self.id[..self.id.len().min(12)];
        format!("container {short} (image {})", self.image)
    }

    fn teardown(&mut self) -> Result<(), Diag> {
        if self.keep || self.gone {
            return Ok(());
        }
        let out = self.run(&["rm", "-f", &self.id])?;
        self.gone = true;
        if out.status.success() {
            Ok(())
        } else {
            Err(Diag::bare(format!(
                "cannot remove {}: {}",
                self.handle(),
                stderr_tail(&out)
            )))
        }
    }
}

/// Best-effort cleanup on panic or early `?`: a kept instance survives,
/// everything else is force-removed.
impl Drop for DockerInstance {
    fn drop(&mut self) {
        if !self.keep && !self.gone {
            let _ = Command::new(&self.cmd)
                .args(["rm", "-f", &self.id])
                .output();
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
        unsafe { std::env::set_var("CONFIG_WEAVE_CONTAINER_CMD", "/nonexistent/ctl") };
        let err = DockerBackend::discover(true).unwrap_err();
        unsafe { std::env::remove_var("CONFIG_WEAVE_CONTAINER_CMD") };
        assert!(err.message.contains("/nonexistent/ctl"), "{}", err.message);
        assert!(err.message.contains("docker or podman"), "{}", err.message);
    }
}
