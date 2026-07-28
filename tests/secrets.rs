//! Encrypted playbook values: the `secret("…")` marker, the
//! `secrets encrypt|decrypt|rekey` rewrite, password handling and
//! redaction of decrypted values from output.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PW: &str = "correct horse battery staple";
const PW2: &str = "a different password entirely";
const SECRET: &str = "hunter2-the-database-password";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

/// Run with no password in the environment at all.
fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    run_env(dir, args, &[])
}

fn run_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(dir)
        .env_remove("CONFIG_WEAVE_PASSWORD")
        .env_remove("CONFIG_WEAVE_NEW_PASSWORD");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Run with the password piped in on stdin.
fn run_stdin(dir: &Path, args: &[&str], password: &str) -> (i32, String, String) {
    let mut child = Command::new(bin())
        .args(args)
        .current_dir(dir)
        .env_remove("CONFIG_WEAVE_PASSWORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(format!("{password}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn pw(dir: &Path) -> [(&'static str, String); 1] {
    let _ = dir;
    [("CONFIG_WEAVE_PASSWORD", PW.to_string())]
}

fn with_pw<'a>(p: &'a [(&'static str, String); 1]) -> Vec<(&'a str, &'a str)> {
    p.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// A package whose one resource writes whatever `value` it is given to
/// `path`, and logs it — so a test can prove both that the decrypted
/// plaintext reached the script and that it is scrubbed from output.
fn write_package(root: &Path) {
    let pkg = root.join("pkgs/probe");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "probe" {
  description = "Records the value it is handed"

  resource "echo" {
    description = "Writes `value` to `path`"
    script = "resources/echo.ws"

    param "path" {
      description = "Where to write the value"
      type = "string"
      required = true
    }
    param "value" {
      description = "The value to write"
      type = "string"
      required = true
    }
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("resources/echo.ws"),
        r#"use value
use fs
use log

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> Result[CheckResult, string] {
    let want = p(params, "value")
    log::info("echo saw value=" + want)
    let path = p(params, "path")
    if fs::exists(path) {
        let got = fs::read(path)?
        if got == want { return Ok(CheckResult::AlreadyConfigured) }
    }
    Ok(CheckResult::NotConfigured)
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    fs::write(p(params, "path"), p(params, "value"))?
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();
}

/// A playbook whose single step feeds a `secret()` value to `probe.echo`.
/// The comment and the odd spacing are load-bearing: they prove the
/// rewrite is a byte splice, not a reformat.
fn write_playbook(root: &Path, secrets: &str) {
    write_package(root);
    let out = root.join("out.txt");
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Secretive" {{
  description = "Uses an encrypted value"
  version = "0.1.0"

  // A comment that must survive `secrets encrypt` verbatim.
  vars {{
{secrets}
  }}

  play "p" {{
    description = "one step"
    step "s" {{
      description = "feed the secret to the resource"
      resource = "probe.echo"
      properties {{
        path  = "{}"
        value = db_password
      }}
    }}
  }}
}}
"#,
            out.display()
        ),
    )
    .unwrap();
}

fn playbook_src(root: &Path) -> String {
    std::fs::read_to_string(root.join("playbook.wcl")).unwrap()
}

fn fixture(secrets: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_playbook(dir.path(), secrets);
    dir
}

fn plain_fixture() -> tempfile::TempDir {
    fixture(&format!("    db_password = secret(\"{SECRET}\")"))
}

/// Encrypt with `PW` and return the resulting source.
fn encrypt(dir: &Path) -> String {
    let (code, stdout, stderr) = run_env(
        dir,
        &["secrets", "encrypt", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    playbook_src(dir)
}

// ------------------------------------------------------------ validation

#[test]
fn an_unencrypted_secret_fails_check_and_names_the_fix() {
    let dir = plain_fixture();
    let (code, _, stderr) = run_in(dir.path(), &["check", ".", "p"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("has not been encrypted yet"), "{stderr}");
    assert!(stderr.contains("config-weave secrets encrypt"), "{stderr}");
}

#[test]
fn an_unencrypted_secret_fails_apply_and_validate_too() {
    let dir = plain_fixture();
    for args in [["apply", ".", "p"], ["validate", ".", ""]] {
        let args: Vec<&str> = args.iter().copied().filter(|a| !a.is_empty()).collect();
        let (code, _, stderr) = run_in(dir.path(), &args);
        assert_eq!(code, 2, "{:?}: {stderr}", args);
        assert!(stderr.contains("has not been encrypted yet"), "{stderr}");
    }
}

#[test]
fn a_secret_call_that_cannot_be_rewritten_is_rejected() {
    let dir = fixture("    other = \"x\"\n    db_password = secret(other)");
    let (code, _, stderr) = run_in(dir.path(), &["validate", "."]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("plain string literal"), "{stderr}");
}

#[test]
fn secret_is_rejected_inside_a_package() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let pkg = dir.path().join("pkgs/probe/package.wcl");
    // `default` is a declared field, so the only complaint is the one
    // under test.
    let src = std::fs::read_to_string(&pkg).unwrap().replace(
        r#"      required = true
    }
  }
}"#,
        r#"      default = secret("nope")
    }
  }
}"#,
    );
    std::fs::write(&pkg, src).unwrap();
    let (code, _, stderr) = run_env(
        dir.path(),
        &["validate", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("secret() is not allowed here"), "{stderr}");
    assert!(
        stderr.contains("validation failed with 1 error"),
        "{stderr}"
    );
}

#[test]
fn a_playbook_with_no_secrets_never_asks_for_a_password() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let out = dir.path().join("out.txt");
    std::fs::write(
        dir.path().join("playbook.wcl"),
        format!(
            r#"playbook "Plain" {{
  description = "no secrets here"
  play "p" {{
    description = "one step"
    step "s" {{
      description = "plain value"
      resource = "probe.echo"
      properties {{ path = "{}" value = "public" }}
    }}
  }}
}}
"#,
            out.display()
        ),
    )
    .unwrap();
    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "public");
}

// --------------------------------------------------------------- encrypt

#[test]
fn encrypt_rewrites_in_place_and_changes_nothing_else() {
    let dir = plain_fixture();
    let before = playbook_src(dir.path());
    let after = encrypt(dir.path());

    assert!(!after.contains(SECRET), "plaintext survived:\n{after}");
    assert!(after.contains("secret(\"CWENC1."), "{after}");
    assert!(
        after.contains("// A comment that must survive `secrets encrypt` verbatim."),
        "the comment was reformatted:\n{after}"
    );

    // Everything outside the one call is byte-identical.
    let strip = |s: &str| {
        let at = s.find("secret(\"").unwrap();
        let end = s[at..].find("\")").unwrap() + at + 2;
        format!("{}{}", &s[..at], &s[end..])
    };
    assert_eq!(strip(&before), strip(&after));
}

#[test]
fn encrypt_is_idempotent_and_verifies_the_password() {
    let dir = plain_fixture();
    let once = encrypt(dir.path());
    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["secrets", "encrypt", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("already encrypted"), "{stdout}");
    assert!(stdout.contains("password verified"), "{stdout}");
    assert_eq!(playbook_src(dir.path()), once, "a no-op rewrote the file");
}

#[test]
fn encrypting_a_new_secret_requires_the_same_password() {
    let dir = plain_fixture();
    encrypt(dir.path());

    // Author adds a second secret by hand.
    let src = playbook_src(dir.path()).replace(
        "  }\n\n  play",
        "    api_token = secret(\"tok-abcdefgh\")\n  }\n\n  play",
    );
    std::fs::write(dir.path().join("playbook.wcl"), &src).unwrap();

    // Wrong password: refused, nothing written, and the fix is named.
    let (code, _, stderr) = run_env(
        dir.path(),
        &["secrets", "encrypt", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW2)],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("does not match"), "{stderr}");
    assert!(stderr.contains("secrets rekey"), "{stderr}");
    assert_eq!(
        playbook_src(dir.path()),
        src,
        "a refused encrypt wrote anyway"
    );

    // Right password: the new one is encrypted, the old one untouched.
    let after = encrypt(dir.path());
    assert!(!after.contains("tok-abcdefgh"), "{after}");
    assert_eq!(after.matches("secret(\"CWENC1.").count(), 2, "{after}");
}

#[test]
fn encrypt_reuses_the_existing_salt() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let src = playbook_src(dir.path()).replace(
        "  }\n\n  play",
        "    api_token = secret(\"tok-abcdefgh\")\n  }\n\n  play",
    );
    std::fs::write(dir.path().join("playbook.wcl"), src).unwrap();
    let after = encrypt(dir.path());

    let salts: Vec<&str> = after
        .match_indices("secret(\"CWENC1.")
        .map(|(at, m)| {
            let rest = &after[at + m.len()..];
            &rest[..rest.find('.').unwrap()]
        })
        .collect();
    assert_eq!(salts.len(), 2);
    assert_eq!(salts[0], salts[1], "a second pass minted a new salt");
}

// ------------------------------------------------------------ check/apply

#[test]
fn the_decrypted_value_reaches_the_resource() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let env = pw(dir.path());
    let (code, stdout, stderr) = run_env(dir.path(), &["apply", ".", "p"], &with_pw(&env));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        SECRET
    );

    // And the run converges: a re-check sees the same decrypted value.
    let (code, stdout, _) = run_env(dir.path(), &["check", ".", "p"], &with_pw(&env));
    assert_eq!(code, 0);
    assert!(stdout.contains("already configured"), "{stdout}");
}

#[test]
fn the_password_can_come_from_stdin_or_a_file() {
    let dir = plain_fixture();
    encrypt(dir.path());

    let (code, stdout, stderr) =
        run_stdin(dir.path(), &["apply", ".", "p", "--password-stdin"], PW);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        SECRET
    );

    let pw_file = dir.path().join("pw.txt");
    std::fs::write(&pw_file, format!("{PW}\n")).unwrap();
    let (code, stdout, stderr) = run_in(
        dir.path(),
        &[
            "check",
            ".",
            "p",
            "--password-file",
            pw_file.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("already configured"), "{stdout}");
}

#[test]
fn a_missing_password_names_every_source() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let (code, _, stderr) = run_in(dir.path(), &["check", ".", "p"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("CONFIG_WEAVE_PASSWORD"), "{stderr}");
    assert!(stderr.contains("--password-stdin"), "{stderr}");
    assert!(stderr.contains("--password-file"), "{stderr}");
}

#[test]
fn a_wrong_password_fails_the_run() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["check", ".", "p"],
        &[("CONFIG_WEAVE_PASSWORD", PW2)],
    );
    assert_ne!(code, 0, "{stdout}{stderr}");
    assert!(
        stderr.contains("wrong password") || stdout.contains("wrong password"),
        "{stdout}{stderr}"
    );
}

/// WCL evaluates lazily, so a secret no step references is never
/// decrypted by the evaluator. The password is verified up front instead,
/// or a wrong one would produce a clean run and only bite later.
#[test]
fn a_wrong_password_fails_even_when_the_secret_is_unused() {
    let dir = fixture(&format!(
        "    db_password = \"public\"\n    unused = secret(\"{SECRET}\")"
    ));
    encrypt(dir.path());

    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["check", ".", "p"],
        &[("CONFIG_WEAVE_PASSWORD", PW2)],
    );
    assert_eq!(code, 2, "{stdout}{stderr}");
    assert!(stderr.contains("wrong password"), "{stderr}");

    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["check", ".", "p"],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
}

#[test]
fn two_password_sources_at_once_are_refused() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let pw_file = dir.path().join("pw.txt");
    std::fs::write(&pw_file, PW).unwrap();
    let (code, _, stderr) = run_env(
        dir.path(),
        &[
            "check",
            ".",
            "p",
            "--password-file",
            pw_file.to_str().unwrap(),
        ],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("more than one password source"), "{stderr}");
}

// -------------------------------------------------------------- redaction

#[test]
fn a_decrypted_value_is_scrubbed_from_the_log_and_reports() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let log = dir.path().join("run.ndjson");
    let env = pw(dir.path());
    let (code, stdout, stderr) = run_env(
        dir.path(),
        &[
            "check",
            ".",
            "p",
            "--json",
            "--log-file",
            log.to_str().unwrap(),
        ],
        &with_pw(&env),
    );
    assert_eq!(code, 0, "{stdout}{stderr}");

    // The script logged `echo saw value=<secret>`; the log must show the
    // mask instead.
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("echo saw value="),
        "log has no script line:\n{logged}"
    );
    assert!(
        !logged.contains(SECRET),
        "the log leaked the secret:\n{logged}"
    );
    assert!(logged.contains("***"), "{logged}");

    assert!(
        !stdout.contains(SECRET),
        "--json output leaked the secret:\n{stdout}"
    );
    assert!(
        !stderr.contains(SECRET),
        "stderr leaked the secret:\n{stderr}"
    );
}

#[test]
fn generated_docs_never_carry_a_blob() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let out = dir.path().join("docs");
    let env = pw(dir.path());
    // `docs` shells out to `wcl`, which may not be installed; the emitted
    // source is written first either way.
    let _ = run_env(
        dir.path(),
        &["docs", ".", out.to_str().unwrap()],
        &with_pw(&env),
    );
    let emitted = out.join("_weave_docs.wcl");
    if let Ok(src) = std::fs::read_to_string(&emitted) {
        assert!(!src.contains(SECRET), "{src}");
        assert!(!src.contains("CWENC1."), "docs published the blob:\n{src}");
        assert!(src.contains("secret(…)"), "{src}");
    }
}

// ------------------------------------------------------------ decrypt/rekey

#[test]
fn decrypt_round_trips_back_to_the_original_source() {
    let dir = plain_fixture();
    let before = playbook_src(dir.path());
    encrypt(dir.path());
    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["secrets", "decrypt", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(playbook_src(dir.path()), before);
}

#[test]
fn decrypt_refuses_a_wrong_password() {
    let dir = plain_fixture();
    let encrypted = encrypt(dir.path());
    let (code, _, stderr) = run_env(
        dir.path(),
        &["secrets", "decrypt", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW2)],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("wrong password"), "{stderr}");
    assert_eq!(
        playbook_src(dir.path()),
        encrypted,
        "a failed decrypt wrote anyway"
    );
}

#[test]
fn rekey_swaps_the_password() {
    let dir = plain_fixture();
    encrypt(dir.path());

    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["secrets", "rekey", "."],
        &[
            ("CONFIG_WEAVE_PASSWORD", PW),
            ("CONFIG_WEAVE_NEW_PASSWORD", PW2),
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("re-encrypted 1"), "{stdout}");

    // Old password no longer works, new one does.
    let (code, _, _) = run_env(
        dir.path(),
        &["check", ".", "p"],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_ne!(code, 0, "the old password still decrypts after a rekey");

    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["apply", ".", "p"],
        &[("CONFIG_WEAVE_PASSWORD", PW2)],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        SECRET
    );
}

#[test]
fn rekey_also_sweeps_up_a_still_plaintext_value() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let src = playbook_src(dir.path()).replace(
        "  }\n\n  play",
        "    api_token = secret(\"tok-abcdefgh\")\n  }\n\n  play",
    );
    std::fs::write(dir.path().join("playbook.wcl"), src).unwrap();

    let (code, stdout, stderr) = run_env(
        dir.path(),
        &["secrets", "rekey", "."],
        &[
            ("CONFIG_WEAVE_PASSWORD", PW),
            ("CONFIG_WEAVE_NEW_PASSWORD", PW2),
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("re-encrypted 2"), "{stdout}");
    let after = playbook_src(dir.path());
    assert!(!after.contains("tok-abcdefgh"), "{after}");
    assert_eq!(after.matches("secret(\"CWENC1.").count(), 2, "{after}");
}

#[test]
fn rekey_without_a_new_password_is_an_error() {
    let dir = plain_fixture();
    encrypt(dir.path());
    let (code, _, stderr) = run_env(
        dir.path(),
        &["secrets", "rekey", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("CONFIG_WEAVE_NEW_PASSWORD"), "{stderr}");
    assert!(stderr.contains("--new-password-file"), "{stderr}");
}

#[test]
fn secrets_commands_work_on_a_playbook_that_does_not_validate() {
    // `secrets encrypt` must not be blocked by the very error it clears,
    // so it reads the raw source rather than loading the model.
    let dir = plain_fixture();
    let src = playbook_src(dir.path())
        .replace(r#"resource = "probe.echo""#, r#"resource = "probe.nope""#);
    std::fs::write(dir.path().join("playbook.wcl"), src).unwrap();

    let (code, _, _) = run_env(
        dir.path(),
        &["validate", "."],
        &[("CONFIG_WEAVE_PASSWORD", PW)],
    );
    assert_eq!(code, 2, "the fixture should not validate");

    let after = encrypt(dir.path());
    assert!(!after.contains(SECRET), "{after}");
}
