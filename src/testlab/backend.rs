//! The backend seam: anything that can run a disposable instance of an
//! image, with just enough surface for the test runner — copy files in,
//! exec argv, tear down. Docker implements it in v1; vmlab slots in
//! later as a second implementation.

use std::path::Path;

use crate::diag::Diag;

/// Output of one exec inside an instance. A nonzero exit code is data
/// for the caller, never an `Err` — errors mean the transport failed.
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait TestBackend {
    /// Backend id as written in `backend = "…"` test fields.
    fn name(&self) -> &'static str;

    /// Provision a running instance of `image`, ready for `exec`. With
    /// `keep`, automatic teardown is disabled for post-mortem debugging.
    fn provision(&self, image: &str, keep: bool) -> Result<Box<dyn TestInstance>, Diag>;
}

pub trait TestInstance {
    /// Copy a host file or directory tree to `dest` inside the instance,
    /// creating parent directories.
    fn copy_in(&self, src: &Path, dest: &str) -> Result<(), Diag>;

    /// Run argv inside the instance (working directory `/weave`).
    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, Diag>;

    /// Human-readable handle for `--keep` messages.
    fn handle(&self) -> String;

    /// Tear down the instance; no-op when kept or already gone.
    fn teardown(&mut self) -> Result<(), Diag>;
}
