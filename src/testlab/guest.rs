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

use crate::diag::Diag;

use super::backend::{GuestOs, Transport};
use super::synth::BinaryResolver;

/// Where the config-weave binary lives inside a guest of `os`. Shared: it
/// is copied once per instance and every test grouped onto that instance
/// invokes this one path.
pub fn bin_for(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Linux => "/weave/config-weave",
        GuestOs::Windows => "C:/weave/config-weave.exe",
    }
}

/// A machine, bound to the guest OS running inside it.
///
/// Cheap to construct: borrow a transport and make one where you need it.
/// The guest OS is cached because it is the fact every path and shell
/// decision below turns on.
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
}
