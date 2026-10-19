//! Everything the host knows about talking to a guest.
//!
//! One machine, two very different callers. The test runner drives a
//! disposable instance through the three-run protocol; a scenario script
//! drives several through the `testlab` host module. Both need the same
//! facts — where the binary lives inside a guest, which shell runs a
//! script there, how config-weave is invoked and how its output reads
//! back — and each used to carry its own copy of them.
//!
//! The seam underneath is [`Transport`], deliberately narrow: guest OS,
//! one exec, one copy-in. Everything above it is ordinary logic a
//! scripted fake can drive, which is what makes the Windows branches
//! testable on a host that has never seen a hypervisor.

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::diag::Diag;
use crate::hostapi::shell::{powershell_argv, write_ps_script};
use crate::report::JsonRunReport;

use super::backend::{ExecOutput, GuestOs, Transport};
use super::synth::BinaryResolver;

/// One config-weave run inside a guest: the report it printed, and the
/// stderr it produced alongside.
///
/// The stderr comes back because the runner republishes it as a log event
/// — this module deliberately emits none itself, so event policy stays
/// with the caller.
pub struct RunOut {
    pub report: JsonRunReport,
    pub stderr: String,
}

/// What a gatherer said.
///
/// [`GatherOutcome::Refused`] is the gatherer answering "no" — data, not
/// a failure of this module. The two callers legitimately disagree about
/// whether that is fatal: a test records it and carries on, a scenario
/// raises it to the script. Keeping it distinct from `Err` is what stops
/// either of them mistaking a broken transport for a refusal.
pub enum GatherOutcome {
    Ok(serde_json::Value),
    Refused(String),
}

/// What a verify script said. A script that *broke* is an `Err` instead —
/// "the assertions did not hold" and "the script could not run" are not
/// the same answer.
pub enum VerifyOutcome {
    Passed,
    Failed(String),
}

/// What [`Workdir::powershell`] did: the script's own result, and whether
/// its copy in the guest was removed afterwards.
///
/// Two fields rather than one `Result` so a failed cleanup can never mask
/// the script's result. What to do about a leftover file is the caller's
/// call — this module emits nothing itself.
pub struct PsRun {
    pub out: Result<ExecOutput, Diag>,
    pub cleanup: Result<(), Diag>,
}

/// The interesting end of a command's output: stderr when there is any,
/// stdout otherwise. A config-weave run that printed no parseable report
/// usually explains itself on stderr, but a panic lands on stdout.
fn tail_of(out: &ExecOutput) -> String {
    let s = if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    super::output::output_tail(s, "(no output)")
}

/// The one directory inside a guest that everything the host copies lives
/// under.
///
/// Not cosmetic: a *container* instance exposes only this path to the
/// host, and `VmlabInstance::copy_in` refuses any destination outside it.
/// Because this module is the only place a guest path is built, that
/// refusal is now unreachable rather than merely checked.
pub const fn root_for(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Linux => "/weave",
        GuestOs::Windows => "C:/weave",
    }
}

/// Where the config-weave binary lives inside a guest of `os`. Shared: it
/// is copied once per instance and every test grouped onto that instance
/// invokes this one path.
fn bin_for(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Linux => "/weave/config-weave",
        GuestOs::Windows => "C:/weave/config-weave.exe",
    }
}

/// Guest paths are written with forward slashes throughout — Windows
/// accepts them everywhere these paths go. Only *shell command text*
/// needs backslashes, so this is the one place that converts.
fn shell_path(os: GuestOs, path: &str) -> String {
    match os {
        GuestOs::Linux => path.to_string(),
        GuestOs::Windows => path.replace('/', "\\"),
    }
}

/// How many trivial execs in a row the guest agent must answer before an
/// instance counts as ready. One answer is not enough: right after a boot
/// or an agent refresh the agent can answer once and then drop the next
/// exec ("agent channel closed during exec", or exit 137).
pub const AGENT_STEADY_STREAK: usize = 3;

/// The trivial exec [`Guest::await_steady_agent`] probes with.
fn probe_argv(os: GuestOs) -> &'static [&'static str] {
    match os {
        GuestOs::Linux => &["true"],
        GuestOs::Windows => &["cmd.exe", "/C", "exit 0"],
    }
}

/// A machine, bound to the guest OS running inside it.
///
/// Cheap to construct and to copy: borrow a transport and make one where
/// you need it. The guest OS is cached because it is the fact every path
/// and shell decision below turns on.
#[derive(Clone, Copy)]
pub struct Guest<'a> {
    t: &'a dyn Transport,
    os: GuestOs,
}

impl<'a> Guest<'a> {
    pub fn new(t: &'a dyn Transport) -> Guest<'a> {
        let os = t.os();
        Guest { t, os }
    }

    /// Where the config-weave binary lives inside this guest.
    fn bin(&self) -> &'static str {
        bin_for(self.os)
    }

    /// Copy the config-weave binary in and prove it runs.
    ///
    /// *When* to call this is caller policy, and the two callers differ
    /// for good reason: the runner prepares once per group up front, so a
    /// failure can error every test in that group, while a scenario
    /// prepares lazily per machine — one that only ever calls
    /// `machine.exec` should not pay to copy a binary it will not use.
    ///
    /// `whose` names the machine in the failure; only the caller knows
    /// whether that is a test target or a scenario machine name.
    pub fn prepare(&self, binaries: &BinaryResolver, whose: &str) -> Result<(), Diag> {
        let bin = self.bin();
        let host = binaries.resolve(self.os)?;
        self.t.copy_in(&host, bin)?;

        // The container payload mount and vmlab's file transfer both
        // preserve the executable bit; chmod defensively anyway for odd
        // umasks. Best-effort — an image without chmod surfaces at the
        // smoke test below. Windows has no execute bit.
        if self.os == GuestOs::Linux {
            let _ = self.t.exec(&["chmod", "+x", bin]);
        }

        let smoke = self.t.exec(&[bin, "version"])?;
        if smoke.exit_code != 0 {
            return Err(Diag::bare(format!(
                "the config-weave binary failed to run inside {whose} (exit {}): {} — \
                 host/guest architecture mismatch?",
                smoke.exit_code,
                super::output::output_tail(&smoke.stderr, "(no output)")
            )));
        }
        Ok(())
    }

    /// Block until the guest agent answers a trivial exec successfully
    /// [`AGENT_STEADY_STREAK`] times in a row, pausing `pause` between
    /// probes. Any failure restarts the count. Gives up once `deadline`
    /// has passed, naming `whose` machine and the last failure.
    pub fn await_steady_agent(
        &self,
        whose: &str,
        deadline: Duration,
        pause: Duration,
    ) -> Result<(), Diag> {
        let start = Instant::now();
        let mut streak = 0;
        let mut last = None;
        loop {
            match self.t.exec(probe_argv(self.os)) {
                Ok(o) if o.exit_code == 0 => {
                    streak += 1;
                    if streak == AGENT_STEADY_STREAK {
                        return Ok(());
                    }
                }
                Ok(o) => {
                    streak = 0;
                    last = Some(format!("probe exited {}: {}", o.exit_code, tail_of(&o)));
                }
                Err(d) => {
                    streak = 0;
                    last = Some(d.message);
                }
            }
            if start.elapsed() >= deadline {
                return Err(Diag::bare(format!(
                    "{whose}: the guest agent did not answer {AGENT_STEADY_STREAK} execs in a \
                     row within {}s (last failure: {})",
                    deadline.as_secs_f64(),
                    last.as_deref().unwrap_or("none")
                )));
            }
            std::thread::sleep(pause);
        }
    }

    /// A working directory for one test, named by its path-safe slug.
    /// Grouped tests share an instance, so each needs its own.
    pub fn test_work(&self, slug: &str) -> Result<Workdir<'a>, Diag> {
        self.work(&format!("t/{slug}"))
    }

    /// A working directory for one scenario apply on `machine`. `n` is the
    /// machine's apply counter, so successive applies never collide.
    pub fn scenario_work(&self, machine: &str, n: usize) -> Result<Workdir<'a>, Diag> {
        self.work(&format!("s/{machine}-{n}"))
    }

    /// Build and create a working directory at `sub` under the root.
    ///
    /// Creating it here is what makes [`Workdir`] worth having: you cannot
    /// hold one without the directory existing, so nothing can stage into
    /// a directory that was never made.
    fn work(&self, sub: &str) -> Result<Workdir<'a>, Diag> {
        let dir = format!("{}/{sub}", root_for(self.os));
        self.mkdir(&dir)?;
        Ok(Workdir {
            guest: *self,
            playbook: format!("{dir}/playbook"),
            facts: format!("{dir}/facts.json"),
            dir,
        })
    }

    fn mkdir(&self, dir: &str) -> Result<(), Diag> {
        let out = match self.os {
            GuestOs::Linux => self.t.exec(&["mkdir", "-p", dir])?,
            GuestOs::Windows => {
                let win = shell_path(self.os, dir);
                let script = format!("if not exist {win} md {win}");
                self.t.exec(&["cmd.exe", "/C", &script])?
            }
        };
        if out.exit_code != 0 {
            return Err(Diag::bare(format!(
                "cannot create the guest working directory {dir} (exit {}): {}",
                out.exit_code,
                super::output::output_tail(&out.stderr, "(no output)")
            )));
        }
        Ok(())
    }

    fn remove_file(&self, path: &str) -> Result<(), Diag> {
        let out = match self.os {
            GuestOs::Linux => self.t.exec(&["rm", "-f", path])?,
            GuestOs::Windows => {
                let script = format!("del /f /q {}", shell_path(self.os, path));
                self.t.exec(&["cmd.exe", "/C", &script])?
            }
        };
        if out.exit_code != 0 {
            return Err(Diag::bare(format!(
                "cannot remove {path} from the guest (exit {}): {}",
                out.exit_code,
                super::output::output_tail(&out.stderr, "(no output)")
            )));
        }
        Ok(())
    }
}

/// A shell script ready to run in a [`Workdir`], from [`Workdir::script`].
pub struct ShellScript<'a> {
    guest: Guest<'a>,
    argv: [String; 3],
    /// The guest file the script was staged into, if it needed one.
    staged: Option<String>,
}

impl ShellScript<'_> {
    pub fn run(&self) -> Result<ExecOutput, Diag> {
        let argv: Vec<&str> = self.argv.iter().map(String::as_str).collect();
        self.guest.t.exec(&argv)
    }

    /// Delete the staged file, if there is one.
    pub fn remove(self) -> Result<(), Diag> {
        match &self.staged {
            Some(path) => self.guest.remove_file(path),
            None => Ok(()),
        }
    }
}

/// A directory inside a guest that exists, with the things that go in it.
///
/// Obtained from [`Guest::test_work`] or [`Guest::scenario_work`], which
/// are the only two naming conventions the testlab has — keeping both
/// here means neither caller concatenates a guest path.
pub struct Workdir<'a> {
    guest: Guest<'a>,
    dir: String,
    playbook: String,
    facts: String,
}

impl<'a> Workdir<'a> {
    /// The config-weave binary, shared across every working directory in
    /// this guest.
    fn bin(&self) -> &'static str {
        self.guest.bin()
    }

    /// Copy a host playbook directory in. Everything below runs against
    /// what this staged.
    pub fn stage(&self, host: &Path) -> Result<(), Diag> {
        self.guest.t.copy_in(host, &self.playbook)
    }

    /// Run a shell script once with this directory as its working
    /// directory, then remove whatever [`Workdir::script`] staged for it,
    /// best effort. `name` names the staged file.
    pub fn exec_in(&self, name: &str, script: &str) -> Result<ExecOutput, Diag> {
        let staged = self.script(name, script)?;
        let out = staged.run();
        // A leftover batch file in a throwaway workdir harms nothing.
        let _ = staged.remove();
        out
    }

    /// Prepare a shell script to run, possibly repeatedly, with this
    /// directory as its working directory.
    ///
    /// A guest exec's working directory is unspecified, so it has to be
    /// pinned — and *which* shell does the pinning, with which quoting, is
    /// this module's business rather than the caller's. On linux that is
    /// an `sh -c` argv. On windows the script becomes `<name>.cmd` in this
    /// directory, run as `cmd.exe /C <path>`: passed as an argv element
    /// instead, its double quotes reached programs escaped as `\"`, so
    /// `powershell -Command "..."` saw one string literal and did nothing.
    /// A batch file is not quite a `/C` line: an undefined `%VAR%` expands
    /// to nothing instead of staying literal, a literal `%` is `%%`, and a
    /// `for` variable is `%%i`. `@echo off` keeps command echo out of the
    /// output.
    pub fn script(&self, name: &str, script: &str) -> Result<ShellScript<'a>, Diag> {
        match self.guest.os {
            GuestOs::Linux => Ok(ShellScript {
                guest: self.guest,
                argv: [
                    "sh".into(),
                    "-c".into(),
                    format!("cd {} || exit 1\n{script}", self.dir),
                ],
                staged: None,
            }),
            GuestOs::Windows => {
                let body = format!(
                    "@echo off\r\ncd /d {} || exit /b 1\r\n{}\r\n",
                    shell_path(self.guest.os, &self.dir),
                    script.replace("\r\n", "\n").replace('\n', "\r\n")
                );
                let mut file = tempfile::NamedTempFile::new()
                    .map_err(|e| Diag::bare(format!("cannot create the {name} temp file: {e}")))?;
                file.write_all(body.as_bytes())
                    .map_err(|e| Diag::bare(format!("cannot write the {name} temp file: {e}")))?;
                let path = format!("{}/{name}.cmd", self.dir);
                let copied = self.guest.t.copy_in(file.path(), &path);
                let staged = ShellScript {
                    guest: self.guest,
                    argv: [
                        "cmd.exe".into(),
                        "/C".into(),
                        shell_path(self.guest.os, &path),
                    ],
                    staged: Some(path),
                };
                if let Err(e) = copied {
                    // A partial copy still holds the script.
                    let _ = staged.remove();
                    return Err(e);
                }
                Ok(staged)
            }
        }
    }

    /// Run a PowerShell script in the guest without putting it on the
    /// command line, the way `shell::powershell` does on a host: the
    /// script plus its exit trailer is copied in as `script.ps1`, the exec
    /// carries only a bootstrap naming that file, and the file is removed
    /// afterwards whatever the exec did.
    pub fn powershell(&self, script: &str) -> PsRun {
        let (_host_dir, host) = match write_ps_script(script) {
            Ok(written) => written,
            Err(e) => {
                return PsRun {
                    out: Err(Diag::bare(e)),
                    cleanup: Ok(()),
                };
            }
        };
        let path = format!("{}/script.ps1", self.dir);
        let out = self.guest.t.copy_in(&host, &path).and_then(|()| {
            let (exe, args) =
                powershell_argv("powershell", &shell_path(self.guest.os, &path), script);
            let mut argv = vec![exe.as_str()];
            argv.extend(args.iter().map(String::as_str));
            self.guest.t.exec(&argv)
        });
        // Attempted even when the copy or the exec failed: a partial copy
        // still holds the script.
        let cleanup = self.guest.remove_file(&path);
        PsRun { out, cleanup }
    }

    /// Run config-weave over the staged playbook and parse its report.
    ///
    /// Always `--json --continue-on-error`: every step has to report, or
    /// the runner's expectation table stops being total. `label` names
    /// this run in diagnostics — the runner has three and needs to say
    /// which one produced no report.
    pub fn run(
        &self,
        mode: &str,
        play: Option<&str>,
        jobs: Option<usize>,
        label: &str,
    ) -> Result<RunOut, Diag> {
        let mut argv = vec![self.bin(), mode, self.playbook.as_str()];
        if let Some(p) = play {
            argv.push(p);
        }
        argv.extend(["--json", "--continue-on-error"]);
        let jobs = jobs.map(|j| j.to_string());
        if let Some(j) = &jobs {
            argv.extend(["--jobs", j]);
        }

        let out = self.guest.t.exec(&argv)?;
        let report = serde_json::from_str(out.stdout.trim()).map_err(|_| {
            Diag::bare(format!(
                "the {label} run produced no parseable report (exit {}): {}",
                out.exit_code,
                tail_of(&out)
            ))
        })?;
        Ok(RunOut {
            report,
            stderr: out.stderr,
        })
    }

    /// Run one gatherer through the `__gather` protocol.
    ///
    /// `label` names the gatherer in diagnostics: a test names its gathers
    /// itself, so the key is not always what the reader expects to see.
    pub fn gather(
        &self,
        key: &str,
        params: Option<&serde_json::Value>,
        label: &str,
    ) -> Result<GatherOutcome, Diag> {
        let mut argv = vec![self.bin(), "__gather", self.playbook.as_str(), key];
        let params_json;
        if let Some(p) = params {
            params_json = p.to_string();
            argv.extend(["--params-json", &params_json]);
        }

        let out = self.guest.t.exec(&argv)?;
        let parsed: serde_json::Value = serde_json::from_str(out.stdout.trim()).map_err(|_| {
            Diag::bare(format!(
                "gather '{label}' produced no parseable protocol output (exit {}): {}",
                out.exit_code,
                tail_of(&out)
            ))
        })?;

        if parsed["ok"] == serde_json::Value::Bool(true) {
            Ok(GatherOutcome::Ok(parsed["value"].clone()))
        } else {
            Ok(GatherOutcome::Refused(
                parsed["error"]
                    .as_str()
                    .unwrap_or("(no error message)")
                    .to_string(),
            ))
        }
    }

    /// Run a verify script through the `__verify` protocol, with `facts`
    /// staged alongside for it to read.
    pub fn verify(
        &self,
        host_script: &Path,
        facts: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<VerifyOutcome, Diag> {
        let mut file = tempfile::NamedTempFile::new()
            .map_err(|e| Diag::bare(format!("cannot create the facts temp file: {e}")))?;
        file.write_all(
            serde_json::Value::Object(facts.clone())
                .to_string()
                .as_bytes(),
        )
        .map_err(|e| Diag::bare(format!("cannot write the facts temp file: {e}")))?;
        self.guest.t.copy_in(file.path(), &self.facts)?;

        let script = self.staged_script(host_script)?;
        let out = self
            .guest
            .t
            .exec(&[self.bin(), "__verify", &script, "--facts", &self.facts])?;

        // The protocol is the exit code: 0 passed, 1 the assertions did
        // not hold, anything else the script never got that far.
        match out.exit_code {
            0 => Ok(VerifyOutcome::Passed),
            1 => Ok(VerifyOutcome::Failed(super::output::output_tail(
                &out.stdout,
                "(no output)",
            ))),
            code => Err(Diag::bare(format!(
                "the verify script broke inside the guest (exit {code}): {}",
                tail_of(&out)
            ))),
        }
    }

    /// Where a host script living under some `pkgs/<package>/…` ends up
    /// once its playbook has been staged here.
    fn staged_script(&self, host: &Path) -> Result<String, Diag> {
        let comps: Vec<&str> = host
            .iter()
            .map(|c| c.to_str().unwrap_or_default())
            .collect();
        // …/pkgs/<pkg>/<rel> — find the pkgs component from the right.
        let idx = comps
            .iter()
            .rposition(|c| *c == "pkgs")
            .ok_or_else(|| Diag::bare(format!("script {} is not under pkgs/", host.display())))?;
        Ok(format!(
            "{}/pkgs/{}",
            self.playbook,
            comps[idx + 1..].join("/")
        ))
    }
}

/// A scripted [`Transport`] for unit tests here and in the scenario host
/// module, which drives a guest through the same seam.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    /// The second adapter at the [`Transport`] seam: records what it was
    /// asked to do and replays canned outputs. Everything above the seam
    /// is exercised through this, with no instance anywhere.
    pub(crate) struct FakeGuest {
        os: GuestOs,
        argvs: RefCell<Vec<Vec<String>>>,
        copies: RefCell<Vec<(PathBuf, String)>>,
        /// What each copied file held at copy time — a host temp file may
        /// be gone by the time a test looks.
        bodies: RefCell<Vec<Option<String>>>,
        /// Every exec and copy in call order, so a test can assert one
        /// happened before the other.
        trail: RefCell<Vec<String>>,
        replies: RefCell<VecDeque<Result<ExecOutput, Diag>>>,
        /// What an exec beyond the queue gets; a silent success if unset.
        otherwise: Option<ExecOutput>,
    }

    impl FakeGuest {
        pub(crate) fn new(os: GuestOs) -> FakeGuest {
            FakeGuest {
                os,
                argvs: RefCell::new(Vec::new()),
                copies: RefCell::new(Vec::new()),
                bodies: RefCell::new(Vec::new()),
                trail: RefCell::new(Vec::new()),
                replies: RefCell::new(VecDeque::new()),
                otherwise: None,
            }
        }

        /// Answer every exec beyond the queue with `out` instead of a
        /// silent success.
        pub(crate) fn otherwise(mut self, out: ExecOutput) -> FakeGuest {
            self.otherwise = Some(out);
            self
        }

        /// Queue results for the next execs, in order. Anything beyond the
        /// queue succeeds silently, unless [`FakeGuest::otherwise`] says.
        pub(crate) fn replying(self, outs: impl IntoIterator<Item = ExecOutput>) -> FakeGuest {
            self.queue(outs);
            self
        }

        /// As [`FakeGuest::replying`], but callable once a `Guest` already
        /// borrows the fake — which is how the protocol tests queue a reply
        /// *after* the workdir's mkdir has taken one.
        pub(crate) fn queue(&self, outs: impl IntoIterator<Item = ExecOutput>) {
            self.replies.borrow_mut().extend(outs.into_iter().map(Ok));
        }

        /// Queue a transport failure for the next unanswered exec.
        pub(crate) fn breaking(self, why: &str) -> FakeGuest {
            self.replies.borrow_mut().push_back(Err(Diag::bare(why)));
            self
        }

        pub(crate) fn bodies(&self) -> Vec<Option<String>> {
            self.bodies.borrow().clone()
        }

        pub(crate) fn trail(&self) -> Vec<String> {
            self.trail.borrow().clone()
        }

        /// Every exec's argv, in call order.
        pub(crate) fn argvs(&self) -> Vec<Vec<String>> {
            self.argvs.borrow().clone()
        }

        pub(crate) fn copies(&self) -> Vec<(PathBuf, String)> {
            self.copies.borrow().clone()
        }
    }

    impl Transport for FakeGuest {
        fn os(&self) -> GuestOs {
            self.os
        }

        fn exec(&self, argv: &[&str]) -> Result<ExecOutput, Diag> {
            self.argvs
                .borrow_mut()
                .push(argv.iter().map(|s| s.to_string()).collect());
            self.trail
                .borrow_mut()
                .push(format!("exec {}", argv.join(" ")));
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Ok(self.otherwise.clone().unwrap_or_else(|| exited(0))))
        }

        fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag> {
            self.copies
                .borrow_mut()
                .push((src.to_path_buf(), dest.to_string()));
            self.bodies
                .borrow_mut()
                .push(std::fs::read_to_string(src).ok());
            self.trail.borrow_mut().push(format!("copy {dest}"));
            Ok(())
        }
    }

    pub(crate) fn exited(code: i32) -> ExecOutput {
        ExecOutput {
            exit_code: code,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    pub(crate) fn failed(code: i32, stderr: &str) -> ExecOutput {
        ExecOutput {
            exit_code: code,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// A run that printed `stdout` and exited 0.
    pub(crate) fn printed(stdout: &str) -> ExecOutput {
        ExecOutput {
            exit_code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use crate::testlab::backend::ExecOutput;
    use std::path::{Path, PathBuf};

    /// `locate_binary` requires an explicit path to exist, so hand it a
    /// real (empty) file — the guest never actually runs it here.
    fn binaries() -> (tempfile::NamedTempFile, BinaryResolver) {
        let f = tempfile::NamedTempFile::new().expect("temp binary");
        let p = f.path().to_path_buf();
        (f, BinaryResolver::new(Some(p.clone()), Some(p)))
    }

    #[test]
    fn the_binary_location_differs_by_guest_os() {
        let linux = FakeGuest::new(GuestOs::Linux);
        let windows = FakeGuest::new(GuestOs::Windows);
        assert_eq!(Guest::new(&linux).bin(), "/weave/config-weave");
        assert_eq!(Guest::new(&windows).bin(), "C:/weave/config-weave.exe");
    }

    #[test]
    fn preparing_a_linux_guest_copies_then_chmods_then_smoke_tests() {
        let (_f, bins) = binaries();
        let fake = FakeGuest::new(GuestOs::Linux);
        Guest::new(&fake)
            .prepare(&bins, "the group")
            .expect("prepare");

        assert_eq!(fake.copies().len(), 1);
        assert_eq!(fake.copies()[0].1, "/weave/config-weave");
        // Order matters: the executable bit has to be set before the
        // smoke test tries to run it.
        assert_eq!(
            fake.argvs(),
            vec![
                vec!["chmod", "+x", "/weave/config-weave"],
                vec!["/weave/config-weave", "version"],
            ]
        );
    }

    #[test]
    fn preparing_a_windows_guest_skips_chmod() {
        let (_f, bins) = binaries();
        let fake = FakeGuest::new(GuestOs::Windows);
        Guest::new(&fake)
            .prepare(&bins, "the group")
            .expect("prepare");

        assert_eq!(fake.copies()[0].1, "C:/weave/config-weave.exe");
        // Windows has no execute bit — the smoke test is the only exec.
        assert_eq!(
            fake.argvs(),
            vec![vec!["C:/weave/config-weave.exe", "version"]]
        );
    }

    #[test]
    fn a_failing_smoke_test_names_the_machine_and_suspects_the_architecture() {
        let (_f, bins) = binaries();
        // chmod succeeds, the smoke test does not.
        let fake =
            FakeGuest::new(GuestOs::Linux).replying([exited(0), failed(126, "Exec format error")]);

        let err = Guest::new(&fake)
            .prepare(&bins, "'dc1'")
            .expect_err("a nonzero smoke test must fail prepare");

        assert!(
            err.message.contains("'dc1'"),
            "names the machine: {}",
            err.message
        );
        assert!(
            err.message.contains("126"),
            "carries the exit code: {}",
            err.message
        );
        assert!(
            err.message.contains("Exec format error"),
            "carries the guest's stderr: {}",
            err.message
        );
        assert!(
            err.message.contains("architecture mismatch"),
            "suggests the usual cause: {}",
            err.message
        );
    }

    #[test]
    fn a_chmod_that_fails_is_not_fatal_on_its_own() {
        let (_f, bins) = binaries();
        // An image without chmod: the smoke test is what decides.
        let fake =
            FakeGuest::new(GuestOs::Linux).replying([failed(127, "chmod: not found"), exited(0)]);

        Guest::new(&fake)
            .prepare(&bins, "the group")
            .expect("a failing chmod is best-effort");
    }

    // ----------------------------------------------------------- readiness

    fn await_agent(fake: &FakeGuest, deadline: Duration) -> Result<(), Diag> {
        Guest::new(fake).await_steady_agent("machine 'box'", deadline, Duration::ZERO)
    }

    #[test]
    fn a_flaky_agent_is_ready_after_three_answers_in_a_row() {
        // What a freshly booted agent does: drops execs, then settles.
        let fake = FakeGuest::new(GuestOs::Linux)
            .breaking("agent channel closed during exec")
            .replying([failed(137, "")]);
        await_agent(&fake, Duration::from_secs(60)).expect("ready");

        // Two failures, then exactly the three successes the streak needs.
        assert_eq!(fake.argvs().len(), 5);
        assert!(fake.argvs().iter().all(|a| a == &["true"]));
    }

    #[test]
    fn a_failure_mid_streak_restarts_the_count() {
        let fake = FakeGuest::new(GuestOs::Linux).replying([
            exited(0),
            exited(0),
            failed(137, ""),
            exited(0),
            exited(0),
        ]);
        await_agent(&fake, Duration::from_secs(60)).expect("ready");
        // 2 good + 1 bad, then a fresh streak of 3: the two queued and one
        // more.
        assert_eq!(fake.argvs().len(), 6);
    }

    #[test]
    fn a_windows_agent_is_probed_through_cmd() {
        let fake = FakeGuest::new(GuestOs::Windows);
        await_agent(&fake, Duration::from_secs(60)).expect("ready");
        assert_eq!(fake.argvs(), vec![vec!["cmd.exe", "/C", "exit 0"]; 3]);
    }

    #[test]
    fn an_agent_that_never_settles_fails_naming_the_machine() {
        let fake = FakeGuest::new(GuestOs::Linux).otherwise(failed(137, "Killed"));
        let err =
            await_agent(&fake, Duration::from_millis(20)).expect_err("never ready must time out");
        assert!(err.message.contains("machine 'box'"), "{}", err.message);
        assert!(err.message.contains("3 execs in a row"), "{}", err.message);
        assert!(err.message.contains("within 0.02s"), "{}", err.message);
        assert!(
            err.message.contains("probe exited 137: Killed"),
            "{}",
            err.message
        );
    }

    // ------------------------------------------------------------ workdirs

    #[test]
    fn the_two_naming_conventions_sit_under_one_root() {
        let linux = FakeGuest::new(GuestOs::Linux);
        let g = Guest::new(&linux);

        let t = g.test_work("0-core__a").expect("test workdir");
        assert_eq!(t.playbook, "/weave/t/0-core__a/playbook");
        assert_eq!(t.facts, "/weave/t/0-core__a/facts.json");

        let s = g.scenario_work("dc1", 2).expect("scenario workdir");
        assert_eq!(s.playbook, "/weave/s/dc1-2/playbook");

        // Every path a guest ever sees is under the one root the container
        // payload mount exposes — `copy_in` rejects anything else.
        for p in [
            t.playbook.as_str(),
            t.facts.as_str(),
            s.playbook.as_str(),
            t.bin(),
        ] {
            assert!(p.starts_with("/weave/"), "{p} escapes the root");
        }
    }

    #[test]
    fn grouped_tests_and_successive_applies_never_collide() {
        let linux = FakeGuest::new(GuestOs::Linux);
        let g = Guest::new(&linux);

        let a = g.test_work("0-core__a").unwrap();
        let b = g.test_work("1-core__b").unwrap();
        assert_ne!(a.playbook, b.playbook, "each test gets its own dir");
        assert_eq!(a.bin(), b.bin(), "the binary is shared across the group");

        let first = g.scenario_work("dc1", 1).unwrap();
        let second = g.scenario_work("dc1", 2).unwrap();
        assert_ne!(
            first.playbook, second.playbook,
            "each apply gets its own dir"
        );
    }

    #[test]
    fn making_a_workdir_creates_it_linux() {
        let fake = FakeGuest::new(GuestOs::Linux);
        Guest::new(&fake).test_work("0-core__a").expect("workdir");
        assert_eq!(
            fake.argvs(),
            vec![vec!["mkdir", "-p", "/weave/t/0-core__a"]]
        );
    }

    #[test]
    fn making_a_workdir_creates_it_windows() {
        let fake = FakeGuest::new(GuestOs::Windows);
        Guest::new(&fake).test_work("0-core__a").expect("workdir");
        // cmd has no mkdir -p; the guard is the idempotence, and the path
        // has to be backslashed for the shell.
        assert_eq!(
            fake.argvs(),
            vec![vec![
                "cmd.exe",
                "/C",
                "if not exist C:\\weave\\t\\0-core__a md C:\\weave\\t\\0-core__a"
            ]]
        );
    }

    #[test]
    fn a_workdir_that_cannot_be_created_fails_loudly() {
        let fake = FakeGuest::new(GuestOs::Linux).replying([failed(1, "Read-only file system")]);
        let err = Guest::new(&fake)
            .test_work("0-core__a")
            .err()
            .expect("a nonzero mkdir must fail");
        assert!(
            err.message.contains("/weave/t/0-core__a"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains("Read-only file system"),
            "{}",
            err.message
        );
    }

    #[test]
    fn staging_puts_the_playbook_where_it_belongs() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let g = Guest::new(&fake);
        let wd = g.test_work("0-core__a").unwrap();
        wd.stage(Path::new("/host/synth")).unwrap();

        assert_eq!(
            fake.copies(),
            vec![(
                PathBuf::from("/host/synth"),
                "/weave/t/0-core__a/playbook".into()
            )]
        );
    }

    #[test]
    fn a_pinned_script_runs_in_the_workdir_linux() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        wd.exec_in("setup", "touch marker").unwrap();

        // `|| exit 1` so a failed cd cannot let the script run somewhere
        // unexpected.
        assert_eq!(
            fake.argvs()[1],
            vec!["sh", "-c", "cd /weave/t/0-core__a || exit 1\ntouch marker"]
        );
    }

    const QUOTED: &str =
        "powershell -NoProfile -Command \"Set-Content -Path C:\\cw.txt -Value 'ok'\"";

    #[test]
    fn a_windows_script_runs_as_a_staged_batch_file() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        wd.exec_in("setup", QUOTED).unwrap();

        let path = "C:/weave/t/0-core__a/setup.cmd";
        assert_eq!(
            fake.trail()[1..],
            [
                format!("copy {path}"),
                "exec cmd.exe /C C:\\weave\\t\\0-core__a\\setup.cmd".to_string(),
                "exec cmd.exe /C del /f /q C:\\weave\\t\\0-core__a\\setup.cmd".to_string(),
            ]
        );
        // The quotes travel in the file, never through an argv element.
        assert!(
            fake.argvs()
                .iter()
                .flatten()
                .all(|a| !a.contains("Set-Content")),
            "{:?}",
            fake.argvs()
        );
        // /d so cd crosses drives, || exit /b so a failed cd stops the
        // script, and CRLF line endings for cmd.
        assert_eq!(
            fake.bodies(),
            vec![Some(format!(
                "@echo off\r\ncd /d C:\\weave\\t\\0-core__a || exit /b 1\r\n{QUOTED}\r\n"
            ))]
        );
    }

    #[test]
    fn a_multi_line_windows_script_gets_crlf_line_endings() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        wd.exec_in("setup", "echo one\necho two\r\necho three")
            .unwrap();
        let body = fake.bodies()[0].clone().unwrap();
        assert!(
            body.ends_with("echo one\r\necho two\r\necho three\r\n"),
            "{body:?}"
        );
    }

    #[test]
    fn a_staged_script_reports_its_exit_and_is_still_removed() {
        // mkdir ok, the batch file exits 3.
        let fake = FakeGuest::new(GuestOs::Windows).replying([exited(0), failed(3, "nope")]);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        let out = wd.exec_in("setup", "exit /b 3").unwrap();
        assert_eq!(out.exit_code, 3);
        assert_eq!(
            fake.argvs().last().unwrap()[2],
            "del /f /q C:\\weave\\t\\0-core__a\\setup.cmd"
        );
    }

    #[test]
    fn a_staged_script_can_run_repeatedly() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        let script = wd.script("ready", "exit /b 0").unwrap();
        script.run().unwrap();
        script.run().unwrap();
        script.remove().unwrap();
        // Copied once, run twice, removed once.
        assert_eq!(fake.copies().len(), 1);
        assert_eq!(fake.copies()[0].1, "C:/weave/t/0-core__a/ready.cmd");
        assert_eq!(fake.argvs().len(), 4);
    }

    #[test]
    fn a_verify_script_maps_into_the_staged_playbook() {
        let linux = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&linux).test_work("0-core__t").unwrap();
        assert_eq!(
            wd.staged_script(Path::new("/host/playbook/pkgs/core/tests/verify.ws"))
                .unwrap(),
            "/weave/t/0-core__t/playbook/pkgs/core/tests/verify.ws"
        );

        let windows = FakeGuest::new(GuestOs::Windows);
        let wwd = Guest::new(&windows).test_work("0-core__t").unwrap();
        assert_eq!(
            wwd.staged_script(Path::new("/host/playbook/pkgs/core/tests/verify.ws"))
                .unwrap(),
            "C:/weave/t/0-core__t/playbook/pkgs/core/tests/verify.ws"
        );

        assert!(
            wd.staged_script(Path::new("/elsewhere/verify.ws")).is_err(),
            "a script outside pkgs/ has no staged location"
        );
    }

    // ------------------------------------------------------------ protocol

    const EMPTY_REPORT: &str = r#"{"playbook":"p","version":"0","play":"test","mode":"apply","exit_code":0,"duration_secs":0.0,"gathered":[],"steps":[]}"#;

    // Every protocol test below indexes argvs from [1] — [0] is the mkdir
    // that creating the workdir performs.

    #[test]
    fn a_run_always_asks_for_json_and_continue_on_error() {
        // Otherwise a halted run reports nothing for later steps and the
        // expectation table stops being total.
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([printed(EMPTY_REPORT)]);
        wd.run("apply", Some("test"), None, "first apply").unwrap();

        assert_eq!(
            fake.argvs()[1],
            vec![
                "/weave/config-weave",
                "apply",
                "/weave/t/0-core__a/playbook",
                "test",
                "--json",
                "--continue-on-error",
            ]
        );
    }

    #[test]
    fn a_run_forwards_jobs_and_omits_the_play_when_there_is_none() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([printed(EMPTY_REPORT)]);
        wd.run("check", None, Some(4), "check").unwrap();

        let argv = &fake.argvs()[1];
        assert!(!argv.contains(&"test".to_string()), "no play: {argv:?}");
        assert_eq!(&argv[argv.len() - 2..], &["--jobs", "4"]);
    }

    #[test]
    fn an_unparseable_report_names_the_run_and_prefers_stderr() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([ExecOutput {
            exit_code: 2,
            stdout: "half a report".into(),
            stderr: "playbook.wcl:3 unknown resource".into(),
        }]);
        let err = wd
            .run("apply", Some("test"), None, "second apply")
            .err()
            .expect("unparseable output is an error");

        assert!(err.message.contains("second apply"), "{}", err.message);
        assert!(err.message.contains("exit 2"), "{}", err.message);
        // stderr is where a config-weave failure explains itself.
        assert!(err.message.contains("unknown resource"), "{}", err.message);
    }

    #[test]
    fn an_unparseable_report_falls_back_to_stdout_when_stderr_is_empty() {
        // A panic lands on stdout with nothing on stderr.
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([printed("thread 'main' panicked")]);
        let err = wd.run("apply", None, None, "check").err().expect("error");
        assert!(err.message.contains("panicked"), "{}", err.message);
    }

    #[test]
    fn a_gather_omits_params_when_there_are_none() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([printed(r#"{"ok":true,"value":{}}"#)]);
        wd.gather("core.facts", None, "facts").unwrap();

        assert_eq!(
            fake.argvs()[1],
            vec![
                "/weave/config-weave",
                "__gather",
                "/weave/t/0-core__a/playbook",
                "core.facts",
            ]
        );
    }

    #[test]
    fn a_gather_passes_params_as_json_when_there_are_some() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([printed(r#"{"ok":true,"value":{}}"#)]);
        let params = serde_json::json!({ "path": "/etc/hosts" });
        wd.gather("core.facts", Some(&params), "facts").unwrap();

        let argv = &fake.argvs()[1];
        assert_eq!(argv[argv.len() - 2], "--params-json");
        assert_eq!(argv[argv.len() - 1], r#"{"path":"/etc/hosts"}"#);
    }

    #[test]
    fn a_gatherer_that_refuses_is_data_not_an_error() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([printed(r#"{"ok":false,"error":"no such file"}"#)]);

        match wd.gather("core.facts", None, "facts").expect("not an Err") {
            GatherOutcome::Refused(why) => assert_eq!(why, "no such file"),
            GatherOutcome::Ok(_) => panic!("a refusing gatherer must not read as Ok"),
        }
    }

    #[test]
    fn a_gather_whose_output_will_not_parse_is_an_error() {
        // The distinction that matters: a refusal is data, a broken
        // transport or protocol is not.
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([ExecOutput {
            exit_code: 127,
            stdout: String::new(),
            stderr: "config-weave: not found".into(),
        }]);
        let err = wd
            .gather("core.facts", None, "facts")
            .err()
            .expect("unparseable output is an error, not a refusal");
        assert!(err.message.contains("'facts'"), "{}", err.message);
        assert!(err.message.contains("not found"), "{}", err.message);
    }

    #[test]
    fn verify_triages_on_the_exit_code() {
        let script = Path::new("/host/playbook/pkgs/core/tests/verify.ws");
        let facts = serde_json::Map::new();

        let pass = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&pass).test_work("0-core__a").unwrap();
        pass.queue([exited(0)]);
        assert!(matches!(
            wd.verify(script, &facts).unwrap(),
            VerifyOutcome::Passed
        ));

        let fail = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fail).test_work("0-core__a").unwrap();
        fail.queue([ExecOutput {
            exit_code: 1,
            stdout: "expected 3, got 4".into(),
            stderr: String::new(),
        }]);
        match wd.verify(script, &facts).unwrap() {
            VerifyOutcome::Failed(why) => assert!(why.contains("expected 3"), "{why}"),
            VerifyOutcome::Passed => panic!("exit 1 is a failed assertion"),
        }

        // Anything else means the script never got as far as an answer,
        // which is an error rather than a verdict.
        let broke = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&broke).test_work("0-core__a").unwrap();
        broke.queue([failed(2, "syntax error")]);
        let err = wd.verify(script, &facts).err().expect("exit 2 is an error");
        assert!(err.message.contains("syntax error"), "{}", err.message);
    }

    #[test]
    fn verify_stages_the_facts_and_names_the_staged_script() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        fake.queue([exited(0)]);
        let mut facts = serde_json::Map::new();
        facts.insert("os".into(), serde_json::json!({ "id": "debian" }));
        wd.verify(Path::new("/host/pkgs/core/v.ws"), &facts)
            .unwrap();

        // The facts file is copied in under this workdir...
        let copies = fake.copies();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].1, "/weave/t/0-core__a/facts.json");

        // ...and the script is named at its staged location, not its host one.
        assert_eq!(
            fake.argvs()[1],
            vec![
                "/weave/config-weave",
                "__verify",
                "/weave/t/0-core__a/playbook/pkgs/core/v.ws",
                "--facts",
                "/weave/t/0-core__a/facts.json",
            ]
        );
    }

    const SECRET: &str = "$pw = 'MARKER-hunter2'\nWrite-Output $pw";

    #[test]
    fn a_powershell_script_never_reaches_an_exec_argv() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).scenario_work("dc1", 3).expect("workdir");
        let run = wd.powershell(SECRET);
        assert!(run.out.is_ok() && run.cleanup.is_ok());

        assert!(
            fake.argvs().iter().flatten().all(|a| !a.contains("MARKER")),
            "{:?}",
            fake.argvs()
        );
        // The script, with the exit trailer, went in as a file instead.
        assert_eq!(
            fake.bodies(),
            vec![Some(format!(
                "{SECRET}{}",
                crate::hostapi::shell::PS_EXIT_TRAILER
            ))]
        );
    }

    #[test]
    fn a_powershell_script_is_copied_in_then_bootstrapped_then_removed() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).scenario_work("dc1", 3).expect("workdir");
        wd.powershell(SECRET).out.expect("ran");

        let path = "C:/weave/s/dc1-3/script.ps1";
        assert_eq!(fake.copies()[0].1, path);
        // The bootstrap names the guest path, as Windows writes it — not
        // a host path.
        let boot =
            ". ([scriptblock]::Create([IO.File]::ReadAllText('C:\\weave\\s\\dc1-3\\script.ps1')))";
        assert_eq!(
            fake.trail(),
            vec![
                "exec cmd.exe /C if not exist C:\\weave\\s\\dc1-3 md C:\\weave\\s\\dc1-3"
                    .to_string(),
                format!("copy {path}"),
                format!("exec powershell -NoProfile -NonInteractive -Command {boot}"),
                "exec cmd.exe /C del /f /q C:\\weave\\s\\dc1-3\\script.ps1".to_string(),
            ]
        );
    }

    #[test]
    fn a_powershell_script_that_reads_input_gets_it_piped() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).scenario_work("dc1", 1).expect("workdir");
        wd.powershell("$input | Out-String").out.expect("ran");
        assert!(fake.argvs()[1][4].starts_with("$input | . "));
    }

    #[test]
    fn a_powershell_exec_that_breaks_is_still_cleaned_up() {
        // mkdir succeeds, the powershell exec's transport fails.
        let fake = FakeGuest::new(GuestOs::Windows)
            .replying([exited(0)])
            .breaking("agent went away");
        let wd = Guest::new(&fake).scenario_work("dc1", 1).expect("workdir");
        let run = wd.powershell(SECRET);

        let err = run.out.err().expect("the exec failure is the result");
        assert!(err.message.contains("agent went away"), "{}", err.message);
        assert!(run.cleanup.is_ok());
        let last = fake.argvs().pop().unwrap();
        assert_eq!(
            last[..3],
            ["cmd.exe", "/C", "del /f /q C:\\weave\\s\\dc1-1\\script.ps1"]
        );
    }

    #[test]
    fn a_failing_powershell_cleanup_does_not_mask_the_scripts_result() {
        // mkdir ok, the script exits 7, the delete fails.
        let fake = FakeGuest::new(GuestOs::Windows).replying([
            exited(0),
            failed(7, "script said no"),
            failed(1, "Access is denied."),
        ]);
        let wd = Guest::new(&fake).scenario_work("dc1", 1).expect("workdir");
        let run = wd.powershell(SECRET);

        let out = run.out.expect("a nonzero exit is data");
        assert_eq!(out.exit_code, 7);
        assert_eq!(out.stderr, "script said no");
        let err = run.cleanup.expect_err("the delete failed");
        assert!(err.message.contains("Access is denied."), "{}", err.message);
        assert!(
            err.message.contains("C:/weave/s/dc1-1/script.ps1"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_powershell_script_on_linux_is_removed_with_rm() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).scenario_work("box", 2).expect("workdir");
        wd.powershell(SECRET).out.expect("ran");
        let argvs = fake.argvs();
        assert!(argvs[1][4].contains("ReadAllText('/weave/s/box-2/script.ps1')"));
        assert_eq!(argvs[2], ["rm", "-f", "/weave/s/box-2/script.ps1"]);
    }
}
