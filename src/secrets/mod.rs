//! Password-encrypted values in a playbook.
//!
//! An author writes `secret("hunter2")` anywhere an expression is legal —
//! a `vars` entry, a step's `properties`, a gather's `params`, a
//! `condition`. `config-weave secrets encrypt` rewrites each call **in
//! place** to `secret("CWENC1.…")`; `check`/`apply`/`test` decrypt lazily
//! at evaluation with a password from the environment, stdin or a file.
//! A call still holding a plaintext is a validation error, so an
//! un-encrypted secret cannot be run — or committed — by accident.
//!
//! Layout: [`crypto`] is the blob format, [`scan`] finds call sites,
//! [`rewrite`] edits them in place, [`password`] resolves the password,
//! [`env`] registers the WCL builtin and [`redact`] keeps decrypted values
//! out of output.

pub mod crypto;
pub mod env;
pub mod password;
pub mod redact;
pub mod rewrite;
pub mod scan;

use std::path::{Path, PathBuf};

use crate::diag::{Diag, wcl_span};

pub use env::SecretCtx;
pub use password::PasswordArgs;
pub use scan::{SecretCall, State};

/// Read and scan `<dir>/playbook.wcl`.
fn open_playbook(dir: &Path) -> Result<(PathBuf, String, Vec<SecretCall>), Vec<Diag>> {
    let path = dir.join("playbook.wcl");
    let source = std::fs::read_to_string(&path)
        .map_err(|e| vec![Diag::bare(format!("cannot read {}: {e}", path.display()))])?;
    let calls = scan::scan_source(&source, &path.display().to_string())
        .map_err(|e| vec![Diag::from_parse(e)])?;
    Ok((path, source, calls))
}

/// Diagnostics for `secret()` calls that a run cannot proceed with: a
/// value still in the clear, or a call this feature cannot rewrite.
///
/// Called from `engine::validate`, which is the gate `check`, `apply`,
/// `validate`, `test` and `docs` all share.
pub fn validate_calls(source: &str, path: &Path, calls: &[SecretCall]) -> Vec<Diag> {
    calls
        .iter()
        .filter_map(|c| match &c.state {
            State::Encrypted(_) => None,
            State::Plaintext(_) => Some(Diag::spanned(
                "this secret has not been encrypted yet",
                "run `config-weave secrets encrypt` to encrypt it in place",
                path,
                source,
                wcl_span(c.span),
            )),
            State::Invalid(why) => Some(Diag::spanned(
                why.clone(),
                "unsupported secret() call",
                path,
                source,
                wcl_span(c.span),
            )),
        })
        .collect()
}

/// Reject `secret()` outside `playbook.wcl`. Packages are distributed via
/// git and shared across playbooks, so a package-local secret would be
/// encrypted under a password its consumers do not have.
pub fn reject_calls(source: &str, path: &Path, calls: &[SecretCall], why: &str) -> Vec<Diag> {
    calls
        .iter()
        .map(|c| {
            Diag::spanned(
                format!("secret() is not allowed here — {why}"),
                "move this value to the playbook's `vars` block",
                path,
                source,
                wcl_span(c.span),
            )
        })
        .collect()
}

/// True when the source has at least one `secret()` call, i.e. a run needs
/// a password. A playbook without secrets never asks for one.
pub fn needs_password(calls: &[SecretCall]) -> bool {
    !calls.is_empty()
}

/// Decrypt every encrypted value, returning the plaintexts in call order.
///
/// A run does this **up front** rather than leaving it to the evaluator.
/// WCL is lazy, so a secret that no step happens to reference is never
/// evaluated — a wrong password would then produce a clean, successful run
/// and only bite later, when some other step started using the value. It
/// also warms the key cache (one Argon2 pass) and registers every
/// plaintext with the redactor, so scrubbing covers values a script
/// obtained by some route other than a property.
pub fn unlock_all(
    source: &str,
    path: &Path,
    calls: &[SecretCall],
    ctx: &SecretCtx,
    on_mismatch: &str,
) -> Result<Vec<String>, Vec<Diag>> {
    let mut out = Vec::new();
    for call in calls {
        let State::Encrypted(blob) = &call.state else {
            continue;
        };
        match ctx.decrypt(blob) {
            Ok(v) => out.push(v),
            Err(e) => {
                return Err(vec![Diag::spanned(
                    format!("cannot decrypt this value: {e}"),
                    on_mismatch,
                    path,
                    source,
                    wcl_span(call.span),
                )]);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// `config-weave secrets …`
// ---------------------------------------------------------------------

/// Everything the three subcommands share: reject unrewritable calls, and
/// prove the password against every value already encrypted.
struct Session {
    path: PathBuf,
    source: String,
    calls: Vec<SecretCall>,
}

impl Session {
    fn open(dir: &Path) -> Result<Session, Vec<Diag>> {
        let (path, source, calls) = open_playbook(dir)?;
        let invalid: Vec<Diag> = calls
            .iter()
            .filter_map(|c| match &c.state {
                State::Invalid(why) => Some(Diag::spanned(
                    why.clone(),
                    "unsupported secret() call",
                    &path,
                    &source,
                    wcl_span(c.span),
                )),
                _ => None,
            })
            .collect();
        if !invalid.is_empty() {
            return Err(invalid);
        }
        Ok(Session {
            path,
            source,
            calls,
        })
    }

    fn encrypted(&self) -> impl Iterator<Item = (&SecretCall, &String)> {
        self.calls.iter().filter_map(|c| match &c.state {
            State::Encrypted(b) => Some((c, b)),
            _ => None,
        })
    }

    fn plaintext(&self) -> impl Iterator<Item = (&SecretCall, &String)> {
        self.calls.iter().filter_map(|c| match &c.state {
            State::Plaintext(p) => Some((c, p)),
            _ => None,
        })
    }

    /// Decrypt every already-encrypted value. This *is* the "same
    /// password" rule: a new secret can only be added to a playbook by
    /// someone who can already read the ones in it.
    fn unlock_all(&self, ctx: &SecretCtx, on_mismatch: &str) -> Result<Vec<String>, Vec<Diag>> {
        unlock_all(&self.source, &self.path, &self.calls, ctx, on_mismatch)
    }

    /// The salt to keep using, so one file needs one Argon2 pass.
    fn existing_salt(&self) -> Option<crypto::Salt> {
        self.encrypted()
            .find_map(|(_, b)| crypto::blob_salt(b).ok())
    }

    fn commit(&self, edits: Vec<(wcl_lang::ast::Span, String)>) -> Result<(), Vec<Diag>> {
        let name = self.path.display().to_string();
        let out = rewrite::splice(&self.source, &edits, &name).map_err(|d| vec![d])?;
        rewrite::write_atomic(&self.path, &out).map_err(|d| vec![d])
    }
}

/// `config-weave secrets encrypt` — encrypt every plaintext `secret()` in
/// place, after proving the password against the values already there.
pub fn encrypt(dir: &Path, args: &PasswordArgs) -> Result<String, Vec<Diag>> {
    let s = Session::open(dir)?;
    if s.calls.is_empty() {
        return Ok(format!("no secret() values in {}", s.path.display()));
    }

    let password = args.resolve().map_err(|d| vec![d])?;
    let ctx = SecretCtx::new(password);
    s.unlock_all(
        &ctx,
        "the password does not match the values already encrypted here — \
         use `config-weave secrets rekey` to change the password",
    )?;

    let plaintext: Vec<(&SecretCall, &String)> = s.plaintext().collect();
    if plaintext.is_empty() {
        return Ok(format!(
            "{}: all {} secret(s) already encrypted; password verified",
            s.path.display(),
            s.calls.len()
        ));
    }

    let salt = match s.existing_salt() {
        Some(salt) => salt,
        None => crypto::random_salt().map_err(|e| vec![Diag::bare(e)])?,
    };

    let mut edits = Vec::new();
    for (call, value) in &plaintext {
        let blob = ctx
            .encrypt(&salt, value)
            .map_err(|e| vec![Diag::bare(format!("cannot encrypt: {e}"))])?;
        edits.push((call.span, rewrite::render_call(&blob)));
    }
    let n = edits.len();
    s.commit(edits)?;
    Ok(format!("{}: encrypted {n} secret(s)", s.path.display()))
}

/// `config-weave secrets decrypt` — put the plaintext back so a value can
/// be edited. Leaves already-plaintext calls alone.
pub fn decrypt(dir: &Path, args: &PasswordArgs) -> Result<String, Vec<Diag>> {
    let s = Session::open(dir)?;
    let encrypted: Vec<(&SecretCall, &String)> = s.encrypted().collect();
    if encrypted.is_empty() {
        return Ok(format!(
            "no encrypted secret() values in {}",
            s.path.display()
        ));
    }

    let password = args.resolve().map_err(|d| vec![d])?;
    let ctx = SecretCtx::new(password);
    let plaintexts = s.unlock_all(&ctx, "wrong password")?;

    let edits: Vec<_> = encrypted
        .iter()
        .zip(&plaintexts)
        .map(|((call, _), value)| (call.span, rewrite::render_call(value)))
        .collect();
    let n = edits.len();
    s.commit(edits)?;
    Ok(format!(
        "{}: decrypted {n} secret(s) — they are now in the clear on disk",
        s.path.display()
    ))
}

/// `config-weave secrets rekey` — change the password. Re-encrypts every
/// value under a fresh salt, and sweeps up any still-plaintext ones in the
/// same pass.
pub fn rekey(
    dir: &Path,
    args: &PasswordArgs,
    new_file: Option<&Path>,
) -> Result<String, Vec<Diag>> {
    let s = Session::open(dir)?;
    if s.calls.is_empty() {
        return Ok(format!("no secret() values in {}", s.path.display()));
    }

    let new_password = password::resolve_new(new_file).map_err(|d| vec![d])?;

    // Only ask for the old password when there is something to unlock.
    let encrypted: Vec<(&SecretCall, &String)> = s.encrypted().collect();
    let mut values: Vec<(wcl_lang::ast::Span, String)> = Vec::new();
    if !encrypted.is_empty() {
        let old = args.resolve().map_err(|d| vec![d])?;
        let old_ctx = SecretCtx::new(old);
        let plaintexts = s.unlock_all(&old_ctx, "wrong old password")?;
        for ((call, _), value) in encrypted.iter().zip(plaintexts) {
            values.push((call.span, value));
        }
    }
    for (call, value) in s.plaintext() {
        values.push((call.span, value.clone()));
    }

    let salt = crypto::random_salt().map_err(|e| vec![Diag::bare(e)])?;
    let new_ctx = SecretCtx::new(new_password);
    let mut edits = Vec::new();
    for (span, value) in &values {
        let blob = new_ctx
            .encrypt(&salt, value)
            .map_err(|e| vec![Diag::bare(format!("cannot encrypt: {e}"))])?;
        edits.push((*span, rewrite::render_call(&blob)));
    }
    let n = edits.len();
    s.commit(edits)?;
    Ok(format!(
        "{}: re-encrypted {n} secret(s) under the new password",
        s.path.display()
    ))
}
