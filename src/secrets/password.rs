//! Where a password comes from.
//!
//! Three sources, never a prompt: a missing password is always an error
//! so an automated run fails loudly instead of blocking on a terminal
//! that isn't there.

use std::io::Read;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::diag::Diag;

/// Environment variable holding the password.
pub const ENV_VAR: &str = "CONFIG_WEAVE_PASSWORD";
/// Environment variable holding the *new* password for `secrets rekey`.
pub const ENV_VAR_NEW: &str = "CONFIG_WEAVE_NEW_PASSWORD";

/// The password inputs, as parsed from the command line. Held globally on
/// the CLI so `check`/`apply`/`test` and the `secrets` subcommands share
/// one spelling.
#[derive(Debug, Clone, Default)]
pub struct PasswordArgs {
    pub stdin: bool,
    pub file: Option<PathBuf>,
}

/// A password, wiped on drop.
pub type Password = Zeroizing<String>;

impl PasswordArgs {
    /// Resolve the password, or explain every way it could have been
    /// supplied. Precedence is `--password-stdin` > `--password-file` >
    /// `$CONFIG_WEAVE_PASSWORD`; giving more than one is an error rather
    /// than a silent pick, because the wrong one silently produces
    /// "wrong password" much later.
    pub fn resolve(&self) -> Result<Password, Diag> {
        self.resolve_from(ENV_VAR, "--password-stdin", "--password-file")
    }

    fn resolve_from(
        &self,
        env_var: &str,
        stdin_flag: &str,
        file_flag: &str,
    ) -> Result<Password, Diag> {
        let env = std::env::var(env_var).ok().filter(|s| !s.is_empty());
        let given = [self.stdin, self.file.is_some(), env.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if given > 1 {
            return Err(Diag::bare(format!(
                "more than one password source given — use exactly one of \
                 {stdin_flag}, {file_flag} PATH or ${env_var}"
            )));
        }

        if self.stdin {
            return read_stdin(stdin_flag);
        }
        if let Some(path) = &self.file {
            return read_file(path);
        }
        if let Some(v) = env {
            return Ok(Zeroizing::new(v));
        }

        Err(Diag::bare(format!(
            "this playbook has encrypted values but no password was supplied \
             — set ${env_var}, or pass {stdin_flag} or {file_flag} PATH"
        )))
    }
}

/// The *new* password for `secrets rekey`: `--new-password-file` or
/// `$CONFIG_WEAVE_NEW_PASSWORD`. Reading it from stdin is not offered,
/// since stdin is already how the old password may arrive.
pub fn resolve_new(file: Option<&Path>) -> Result<Password, Diag> {
    let env = std::env::var(ENV_VAR_NEW).ok().filter(|s| !s.is_empty());
    match (file, env) {
        (Some(_), Some(_)) => Err(Diag::bare(format!(
            "more than one new-password source given — use either \
             --new-password-file PATH or ${ENV_VAR_NEW}"
        ))),
        (Some(path), None) => read_file(path),
        (None, Some(v)) => Ok(Zeroizing::new(v)),
        (None, None) => Err(Diag::bare(format!(
            "no new password supplied — set ${ENV_VAR_NEW} or pass \
             --new-password-file PATH"
        ))),
    }
}

fn read_stdin(flag: &str) -> Result<Password, Diag> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| Diag::bare(format!("cannot read the password from stdin: {e}")))?;
    finish(
        buf,
        &format!("the password read from stdin ({flag}) is empty"),
    )
}

fn read_file(path: &Path) -> Result<Password, Diag> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| Diag::bare(format!("cannot read {}: {e}", path.display())))?;
    finish(raw, &format!("{} is empty", path.display()))
}

/// Strip one trailing newline (`\n` or `\r\n`) — `echo pw | …` and a
/// password file written by an editor both end in one, and neither means
/// it as part of the password.
fn finish(mut s: String, empty_msg: &str) -> Result<Password, Diag> {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    if s.is_empty() {
        return Err(Diag::bare(empty_msg.to_string()));
    }
    Ok(Zeroizing::new(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_one_trailing_newline() {
        assert_eq!(*finish("pw\n".into(), "e").unwrap(), "pw");
        assert_eq!(*finish("pw\r\n".into(), "e").unwrap(), "pw");
        assert_eq!(*finish("pw\n\n".into(), "e").unwrap(), "pw\n");
        assert_eq!(*finish("pw".into(), "e").unwrap(), "pw");
    }

    #[test]
    fn an_empty_password_is_rejected() {
        assert!(finish("\n".into(), "boom").is_err());
    }

    #[test]
    fn reads_a_password_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pw");
        std::fs::write(&path, "hunter2\n").unwrap();
        let args = PasswordArgs {
            stdin: false,
            file: Some(path),
        };
        assert_eq!(*args.resolve().unwrap(), "hunter2");
    }

    #[test]
    fn missing_password_names_every_source() {
        // Uses a variable name no other test sets, so this stays
        // independent of the ambient environment.
        let args = PasswordArgs::default();
        let err = args
            .resolve_from(
                "CONFIG_WEAVE_PASSWORD_TEST_UNSET",
                "--password-stdin",
                "--password-file",
            )
            .unwrap_err();
        assert!(err.message.contains("--password-stdin"), "{}", err.message);
        assert!(err.message.contains("--password-file"), "{}", err.message);
        assert!(
            err.message.contains("CONFIG_WEAVE_PASSWORD_TEST_UNSET"),
            "{}",
            err.message
        );
    }

    #[test]
    fn two_sources_are_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pw");
        std::fs::write(&path, "x").unwrap();
        let args = PasswordArgs {
            stdin: true,
            file: Some(path),
        };
        let err = args
            .resolve_from(
                "CONFIG_WEAVE_PASSWORD_TEST_UNSET",
                "--password-stdin",
                "--password-file",
            )
            .unwrap_err();
        assert!(err.message.contains("more than one"), "{}", err.message);
    }
}
