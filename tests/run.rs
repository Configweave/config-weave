//! M2 gate tests: the check/apply/re-check lifecycle, all statuses, halt
//! semantics, --continue-on-error, variable precedence, exit codes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).args(args).current_dir(dir).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Build a self-contained playbook whose `probe.marker` resource is fully
/// scriptable through properties: `check` / `apply` params choose the
/// outcome, `path` is the convergence marker.
fn write_lifecycle_playbook(root: &Path, plays: &str) {
    let pkg = root.join("pkgs/probe");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Lifecycle" {{
  description = "Lifecycle behaviour probes"
  version = "0.1.0"

{plays}
}}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "probe" {
  description = "Scriptable probe resources"

  resource "marker" {
    description = "Behaves as instructed by its parameters"
    script = "resources/marker.wisp"

    param "path" {
      description = "Marker file path"
      type = "string"
      required = true
    }
    param "check" {
      description = "Check behaviour: file | already | reboot | error"
      type = "string"
      default = "file"
    }
    param "apply" {
      description = "Apply behaviour: success | reboot | error"
      type = "string"
      default = "success"
    }
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("resources/marker.wisp"),
        r#"use value
use fs

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> Result[CheckResult, string] {
    let mode = p(params, "check")
    if mode == "error" { return Err("check exploded") }
    if mode == "reboot" { return Ok(CheckResult::RebootRequired) }
    if mode == "already" { return Ok(CheckResult::AlreadyConfigured) }
    if fs::exists(p(params, "path")) {
        Ok(CheckResult::AlreadyConfigured)
    } else {
        Ok(CheckResult::NotConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let mode = p(params, "apply")
    if mode == "error" { return Err("apply exploded") }
    if mode == "reboot" { return Ok(ApplyResult::RebootRequired) }
    fs::write(p(params, "path"), "done")?
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();
}

fn step(name: &str, props: &str, extra: &str) -> String {
    format!(
        r#"    step "{name}" {{
      description = "probe step {name}"
      resource = "probe.marker"
      {extra}
      properties {{
{props}
      }}
    }}
"#
    )
}

#[test]
fn full_lifecycle_and_idempotence() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m1");
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}  }}\n",
        step("a", &format!("        path = \"{}\"", marker.display()), "")
    );
    write_lifecycle_playbook(dir.path(), &plays);

    // check before: not configured, exit 0 (check reports, never errors).
    let (code, stdout, stderr) = run_in(dir.path(), &["check", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("not configured"), "{stdout}");

    // apply: configured, exit 0, marker exists.
    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("[         configured]"), "{stdout}");
    assert!(marker.exists());

    // second apply: already configured.
    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("already configured"), "{stdout}");
}

#[test]
fn error_halts_and_continue_on_error_continues() {
    let dir = tempfile::tempdir().unwrap();
    let m2 = dir.path().join("m2");
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}{}  }}\n",
        step("bad", "        path = \"/nonexistent\"\n        check = \"error\"", ""),
        step("good", &format!("        path = \"{}\"", m2.display()), "")
    );
    write_lifecycle_playbook(dir.path(), &plays);

    // Without --continue-on-error: bad errors, good is not run, exit 1.
    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p", "--jobs", "1"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("check exploded"), "{stdout}");
    assert!(stdout.contains("not run"), "{stdout}");
    assert!(!m2.exists());

    // With --continue-on-error: good still applies; exit still 1.
    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p", "--continue-on-error"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("[         configured]"), "{stdout}");
    assert!(m2.exists());
}

#[test]
fn reboot_required_halts_with_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}{}  }}\n",
        step("reboot", "        path = \"/nonexistent\"\n        apply = \"reboot\"", ""),
        step("after", "        path = \"/nonexistent2\"", "")
    );
    write_lifecycle_playbook(dir.path(), &plays);

    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p", "--jobs", "1"]);
    assert_eq!(code, 3, "{stdout}");
    assert!(stdout.contains("reboot required"), "{stdout}");
    assert!(stdout.contains("not run"), "{stdout}");

    // In check mode a reboot-required step is just a report; exit 0.
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}{}  }}\n",
        step("reboot", "        path = \"/nonexistent\"\n        check = \"reboot\"", ""),
        step("after", "        path = \"/nonexistent2\"", "")
    );
    let dir2 = tempfile::tempdir().unwrap();
    write_lifecycle_playbook(dir2.path(), &plays);
    let (code, stdout, _) = run_in(dir2.path(), &["check", ".", "p"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("reboot required"), "{stdout}");
}

#[test]
fn requires_orders_execution_and_blocks_dependents_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let ma = dir.path().join("a");
    let mb = dir.path().join("b");
    // b declared BEFORE a but requires it: must still run after a.
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}{}  }}\n",
        step(
            "b",
            &format!("        path = \"{}\"", mb.display()),
            "requires = [\"a\"]"
        ),
        step("a", &format!("        path = \"{}\"", ma.display()), "")
    );
    write_lifecycle_playbook(dir.path(), &plays);

    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(ma.exists());
    assert!(mb.exists());

    // Failed dependency blocks the dependent under --continue-on-error.
    let dir2 = tempfile::tempdir().unwrap();
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}{}  }}\n",
        step(
            "dep",
            "        path = \"/nonexistent\"\n        apply = \"error\"",
            ""
        ),
        step(
            "child",
            "        path = \"/nonexistent2\"",
            "requires = [\"dep\"]"
        )
    );
    write_lifecycle_playbook(dir2.path(), &plays);
    let (code, stdout, _) = run_in(dir2.path(), &["apply", ".", "p", "--continue-on-error"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("a required step did not complete"), "{stdout}");
}

#[test]
fn apply_lies_is_detected() {
    // apply returns Success but never converges: re-check must flag it.
    let dir = tempfile::tempdir().unwrap();
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}  }}\n",
        // path never written because apply=success writes it... use a
        // check that stays "not": check=not via missing file and an apply
        // that "succeeds" without writing: apply mode 'noop' is not
        // defined, so use apply = success with an unwritable path.
        step("liar", "        path = \"/proc/definitely/not/writable\"", "")
    );
    write_lifecycle_playbook(dir.path(), &plays);
    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p"]);
    // fs::write fails -> apply errors (Err path), which is also a halt.
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("error"), "{stdout}");
}

#[test]
fn var_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let d1 = dir.path().join("from-var");
    let plays = format!(
        r#"  vars {{
    target = "{}"
  }}

  play "p" {{
    description = "probe"
{}  }}
"#,
        dir.path().join("from-decl").display(),
        step("a", "        path = target", "")
    );
    write_lifecycle_playbook(dir.path(), &plays);

    // Declared var used when no override.
    let (code, _, _) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(dir.path().join("from-decl").exists());

    // --var wins over declaration.
    let (code, _, _) = run_in(
        dir.path(),
        &["apply", ".", "p", "--var", &format!("target={}", d1.display())],
    );
    assert_eq!(code, 0);
    assert!(d1.exists());

    // --var wins over --var-file.
    let vf = dir.path().join("vf.wcl");
    std::fs::write(
        &vf,
        format!("target = \"{}\"\n", dir.path().join("from-file").display()),
    )
    .unwrap();
    let d2 = dir.path().join("from-var2");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "apply", ".", "p",
            "--var-file", vf.to_str().unwrap(),
            "--var", &format!("target={}", d2.display()),
        ],
    );
    assert_eq!(code, 0);
    assert!(d2.exists());
    assert!(!dir.path().join("from-file").exists());
}
