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

use std::path::Path;

use crate::diag::Diag;

use super::backend::{ExecOutput, GuestOs, Transport};
use super::synth::BinaryResolver;

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
    pub fn bin(&self) -> &'static str {
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

impl Workdir<'_> {
    /// The config-weave binary, shared across every working directory in
    /// this guest.
    pub fn bin(&self) -> &'static str {
        self.guest.bin()
    }

    /// Where [`Workdir::stage`] puts the playbook.
    pub fn playbook(&self) -> &str {
        &self.playbook
    }

    /// Where [`Workdir::stage_facts`] puts the gathered facts.
    pub fn facts(&self) -> &str {
        &self.facts
    }

    /// Copy a host playbook directory in.
    pub fn stage(&self, host: &Path) -> Result<(), Diag> {
        self.guest.t.copy_in(host, &self.playbook)
    }

    /// Copy the gathered facts in, for a verify script to read.
    pub fn stage_facts(&self, host: &Path) -> Result<(), Diag> {
        self.guest.t.copy_in(host, &self.facts)
    }

    /// Run a shell script with this directory as its working directory.
    ///
    /// A guest exec's working directory is unspecified, so it has to be
    /// pinned — and *which* shell does the pinning, with which quoting, is
    /// this module's business rather than the caller's.
    pub fn exec_in(&self, script: &str) -> Result<ExecOutput, Diag> {
        let pinned;
        let argv: [&str; 3] = match self.guest.os {
            GuestOs::Linux => {
                pinned = format!("cd {} || exit 1\n{script}", self.dir);
                ["sh", "-c", &pinned]
            }
            GuestOs::Windows => {
                pinned = format!("cd /d {} && {script}", shell_path(self.guest.os, &self.dir));
                ["cmd.exe", "/C", &pinned]
            }
        };
        self.guest.t.exec(&argv)
    }

    /// Where a host script living under some `pkgs/<package>/…` ends up
    /// once its playbook has been staged here.
    pub fn staged_script(&self, host: &Path) -> Result<String, Diag> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testlab::backend::ExecOutput;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    /// The second adapter at the [`Transport`] seam: records what it was
    /// asked to do and replays canned outputs. Everything above the seam
    /// is exercised through this, with no instance anywhere.
    struct FakeGuest {
        os: GuestOs,
        argvs: RefCell<Vec<Vec<String>>>,
        copies: RefCell<Vec<(PathBuf, String)>>,
        replies: RefCell<VecDeque<ExecOutput>>,
    }

    impl FakeGuest {
        fn new(os: GuestOs) -> FakeGuest {
            FakeGuest {
                os,
                argvs: RefCell::new(Vec::new()),
                copies: RefCell::new(Vec::new()),
                replies: RefCell::new(VecDeque::new()),
            }
        }

        /// Queue results for the next execs, in order. Anything beyond the
        /// queue succeeds silently.
        fn replying(self, outs: impl IntoIterator<Item = ExecOutput>) -> FakeGuest {
            self.replies.borrow_mut().extend(outs);
            self
        }

        /// Every exec's argv, in call order.
        fn argvs(&self) -> Vec<Vec<String>> {
            self.argvs.borrow().clone()
        }

        fn copies(&self) -> Vec<(PathBuf, String)> {
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
            Ok(self
                .replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| exited(0)))
        }

        fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag> {
            self.copies
                .borrow_mut()
                .push((src.to_path_buf(), dest.to_string()));
            Ok(())
        }
    }

    fn exited(code: i32) -> ExecOutput {
        ExecOutput {
            exit_code: code,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn failed(code: i32, stderr: &str) -> ExecOutput {
        ExecOutput {
            exit_code: code,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

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

    // ------------------------------------------------------------ workdirs

    #[test]
    fn the_two_naming_conventions_sit_under_one_root() {
        let linux = FakeGuest::new(GuestOs::Linux);
        let g = Guest::new(&linux);

        let t = g.test_work("0-core__a").expect("test workdir");
        assert_eq!(t.playbook(), "/weave/t/0-core__a/playbook");
        assert_eq!(t.facts(), "/weave/t/0-core__a/facts.json");

        let s = g.scenario_work("dc1", 2).expect("scenario workdir");
        assert_eq!(s.playbook(), "/weave/s/dc1-2/playbook");

        // Every path a guest ever sees is under the one root the container
        // payload mount exposes — `copy_in` rejects anything else.
        for p in [t.playbook(), t.facts(), s.playbook(), t.bin()] {
            assert!(p.starts_with("/weave/"), "{p} escapes the root");
        }
    }

    #[test]
    fn grouped_tests_and_successive_applies_never_collide() {
        let linux = FakeGuest::new(GuestOs::Linux);
        let g = Guest::new(&linux);

        let a = g.test_work("0-core__a").unwrap();
        let b = g.test_work("1-core__b").unwrap();
        assert_ne!(a.playbook(), b.playbook(), "each test gets its own dir");
        assert_eq!(a.bin(), b.bin(), "the binary is shared across the group");

        let first = g.scenario_work("dc1", 1).unwrap();
        let second = g.scenario_work("dc1", 2).unwrap();
        assert_ne!(
            first.playbook(),
            second.playbook(),
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
    fn staging_puts_the_playbook_and_facts_where_they_belong() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let g = Guest::new(&fake);
        let wd = g.test_work("0-core__a").unwrap();
        wd.stage(Path::new("/host/synth")).unwrap();
        wd.stage_facts(Path::new("/host/facts.json")).unwrap();

        assert_eq!(
            fake.copies(),
            vec![
                (
                    PathBuf::from("/host/synth"),
                    "/weave/t/0-core__a/playbook".into()
                ),
                (
                    PathBuf::from("/host/facts.json"),
                    "/weave/t/0-core__a/facts.json".into()
                ),
            ]
        );
    }

    #[test]
    fn a_pinned_script_runs_in_the_workdir_linux() {
        let fake = FakeGuest::new(GuestOs::Linux);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        wd.exec_in("touch marker").unwrap();

        // `|| exit 1` so a failed cd cannot let the script run somewhere
        // unexpected.
        assert_eq!(
            fake.argvs()[1],
            vec!["sh", "-c", "cd /weave/t/0-core__a || exit 1\ntouch marker"]
        );
    }

    #[test]
    fn a_pinned_script_runs_in_the_workdir_windows() {
        let fake = FakeGuest::new(GuestOs::Windows);
        let wd = Guest::new(&fake).test_work("0-core__a").unwrap();
        wd.exec_in("echo hi").unwrap();

        // /d so cd crosses drives, and && so a failed cd stops the script.
        assert_eq!(
            fake.argvs()[1],
            vec!["cmd.exe", "/C", "cd /d C:\\weave\\t\\0-core__a && echo hi"]
        );
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
}
