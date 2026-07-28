//! The built-in `weave` package: `execute` (guard script + action script)
//! and `execute_once` (run once per host, recorded so it never repeats).
//!
//! Linux only — both resources default to bash, and the record's file form
//! is what a non-Windows host uses.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

/// Run in `dir` with the record root pointed at a scratch directory, so
/// nothing here touches /var/lib.
fn run_in(dir: &Path, state: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .args(args)
        .current_dir(dir)
        .env("CONFIG_WEAVE_STATE_DIR", state)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A playbook with no packages of its own: the built-in `weave` package is
/// always available, with or without a `pkgs/` folder.
fn write_playbook(root: &Path, steps: &str) {
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            "playbook \"Execute\" {{\n  description = \"Built-in execute probes\"\n  \
             version = \"0.1.0\"\n\n  play \"p\" {{\n    description = \"probe\"\n\
             {steps}  }}\n}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_guard_decides_whether_the_action_runs() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    let target = root.join("made");
    write_playbook(
        root,
        &format!(
            r#"    step "s" {{
      description = "make the file"
      resource = "weave.execute"
      properties {{
        check = "test -f {t}"
        run = "touch {t}"
      }}
    }}
"#,
            t = target.display()
        ),
    );

    // check mode reports the work and changes nothing.
    let (code, stdout, stderr) = run_in(root, state.path(), &["check", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("not configured"), "{stdout}");
    assert!(!target.exists());

    // apply runs the action, and the re-check agrees.
    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("[         configured]"), "{stdout}");
    assert!(target.exists());

    // Second apply: the guard is satisfied, so the action is skipped.
    let (code, stdout, _) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("already configured"), "{stdout}");
}

/// The re-check is the whole point of the two-script form: an action that
/// does not satisfy its own guard is a fire-and-forget command, and the
/// engine refuses to call that converged.
#[test]
fn an_action_that_does_not_satisfy_the_guard_fails_the_step() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(
        root,
        r#"    step "s" {
      description = "never satisfies its guard"
      resource = "weave.execute"
      properties {
        check = "false"
        run = "true"
      }
    }
"#,
    );

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("re-check disagrees"), "{stdout}");
}

#[test]
fn a_failing_action_reports_its_status_and_output() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(
        root,
        r#"    step "s" {
      description = "fails loudly"
      resource = "weave.execute"
      properties {
        check = "false"
        run = "echo 'went wrong' >&2; exit 3"
      }
    }
"#,
    );

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("exited 3"), "{stdout}");
    assert!(stdout.contains("went wrong"), "{stdout}");
}

/// `cwd` and `env` reach the script, and the guard reads the same ones.
#[test]
fn cwd_and_env_reach_both_scripts() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    write_playbook(
        root,
        r#"    step "s" {
      description = "writes where it was told, with what it was given"
      resource = "weave.execute"
      properties {
        check = "test -f ./stamp"
        run = "printf '%s' \"$GREETING\" > ./stamp"
        cwd = "work"
        env = { GREETING: "hello" }
      }
    }
"#,
    );

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(work.join("stamp")).unwrap(),
        "hello"
    );
}

/// A declared exit status means "it worked, but reboot first": the play
/// halts with exit code 3 rather than treating the status as a failure.
/// The status here is a small one because Unix truncates an exit status to
/// 0-255 — the Windows installer convention of 3010 would arrive as 194.
#[test]
fn a_declared_reboot_status_halts_the_play() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(
        root,
        r#"    step "s" {
      description = "asks for a reboot"
      resource = "weave.execute"
      properties {
        check = "false"
        run = "exit 42"
        reboot_on = [42, 1641]
      }
    }
"#,
    );

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 3, "{stdout}{stderr}");
    assert!(stdout.contains("reboot required"), "{stdout}");
}

// -------------------------------------------------------- execute_once

fn once_playbook(root: &Path, id: &str, script: &str) {
    write_playbook(
        root,
        &format!(
            r#"    step "s" {{
      description = "run it once"
      resource = "weave.execute_once"
      properties {{
        id = "{id}"
        run = "{script}"
      }}
    }}
"#
        ),
    );
}

#[test]
fn execute_once_runs_once_and_records_that_it_did() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    let counter = root.join("runs");
    once_playbook(
        root,
        "bootstrap_v1",
        &format!("echo x >> {}", counter.display()),
    );

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "x\n");

    let stamp = state.path().join("once/bootstrap_v1");
    assert!(stamp.exists(), "no record at {}", stamp.display());
    // The record carries a digest of what ran — forensics only, since it
    // never gates anything.
    assert!(
        std::fs::read_to_string(&stamp).unwrap().contains("sha256="),
        "record has no digest"
    );

    // Second apply: already recorded, so the script does not run again.
    let (code, stdout, _) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("already configured"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "x\n");
}

/// The record is keyed by `id` alone. Editing the script does not run it
/// again — that is what the resource is for, so it must not drift.
#[test]
fn editing_the_script_does_not_run_it_again() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    let counter = root.join("runs");
    once_playbook(
        root,
        "bootstrap_v1",
        &format!("echo first >> {}", counter.display()),
    );
    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");

    once_playbook(
        root,
        "bootstrap_v1",
        &format!("echo second >> {}", counter.display()),
    );
    let (code, stdout, _) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("already configured"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "first\n");

    // A new id is a new record, and does run.
    once_playbook(
        root,
        "bootstrap_v2",
        &format!("echo second >> {}", counter.display()),
    );
    let (code, stdout, _) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn a_failing_script_leaves_no_record() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    once_playbook(root, "doomed", "exit 1");

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(!state.path().join("once/doomed").exists());
}

#[test]
fn an_id_that_would_escape_the_record_directory_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    once_playbook(root, "../escape", "true");

    let (code, stdout, stderr) = run_in(root, state.path(), &["apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("'id' must not contain"), "{stdout}");
}

// ------------------------------------------------------------ the package

/// The name is reserved: a local `pkgs/weave/` would shadow the built-ins.
#[test]
fn a_local_package_may_not_be_called_weave() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(root, "");
    std::fs::create_dir_all(root.join("pkgs/weave")).unwrap();
    std::fs::write(
        root.join("pkgs/weave/package.wcl"),
        "package \"weave\" {\n  description = \"An impostor\"\n}\n",
    )
    .unwrap();

    let (code, stdout, stderr) = run_in(root, state.path(), &["validate", "."]);
    assert_ne!(code, 0, "{stdout}{stderr}");
    assert!(
        stderr.contains("reserved name 'weave'") || stdout.contains("reserved name 'weave'"),
        "{stdout}{stderr}"
    );
}

/// The built-ins are documented like any other resource, which is what
/// `list` and `docs` render from.
#[test]
fn the_builtin_package_shows_up_in_list() {
    let d = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(root, "");

    let (code, stdout, stderr) = run_in(root, state.path(), &["list", ".", "--json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let listed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let text = listed.to_string();
    assert!(text.contains("weave"), "{stdout}");
    assert!(text.contains("execute_once"), "{stdout}");
}
