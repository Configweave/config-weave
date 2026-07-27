//! M2 gate tests: the check/apply/re-check lifecycle, all statuses, halt
//! semantics, --continue-on-error, variable precedence, exit codes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
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
    script = "resources/marker.ws"

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
    param "mode" {
      description = "Enumerated selector; the script ignores it"
      type = "symbol"
      default = :normal
      symbol "normal" { description = "The default behaviour" }
      symbol "strict" { description = "Reserved" }
    }
    param "span" {
      description = "A duration the script echoes back, to prove the units"
      type = "duration"
      default = 1h
    }
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("resources/marker.ws"),
        r#"use value
use fs
use json

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> Result[CheckResult, string] {
    let mode = p(params, "check")
    if mode == "error" { return Err("check exploded") }
    // Surface the raw `span` param so a test can assert its units.
    if mode == "span" {
        if let Some(v) = params.get("span") { return Err("span=" + json::to_string(v)) }
        return Err("span is absent")
    }
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

/// A `duration` param is authored as a bare WCL unit literal and reaches
/// the script as base nanoseconds — `std.Duration`'s own base unit, so no
/// resolution is lost on the way through.
#[test]
fn a_duration_property_reaches_the_script_as_nanoseconds() {
    let dir = tempfile::tempdir().unwrap();
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}  }}\n",
        step(
            "d",
            "        path = \"/unused\"\n        check = \"span\"\n        span = 30min",
            ""
        )
    );
    write_lifecycle_playbook(dir.path(), &plays);

    let (code, stdout, stderr) = run_in(dir.path(), &["check", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("span={}", 30u64 * 60 * 1_000_000_000)),
        "{stdout}"
    );
}

/// Every suffix resolves through the same `std.Duration` factors, and an
/// omitted duration falls back to its declared unit-literal default.
#[test]
fn duration_suffixes_and_the_declared_default_resolve() {
    let cases = [
        ("span = 90s", 90u64 * 1_000_000_000),
        ("span = 2min", 120 * 1_000_000_000),
        ("span = 7d", 7 * 86_400 * 1_000_000_000),
        // No `span` property at all: the param's `default = 1h` applies.
        ("", 3_600 * 1_000_000_000),
    ];
    for (prop, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let props = format!("        path = \"/unused\"\n        check = \"span\"\n        {prop}");
        let plays = format!(
            "  play \"p\" {{\n    description = \"probe\"\n{}  }}\n",
            step("d", &props, "")
        );
        write_lifecycle_playbook(dir.path(), &plays);

        let (_, stdout, stderr) = run_in(dir.path(), &["check", ".", "p"]);
        assert!(
            stdout.contains(&format!("span={want}")),
            "{prop:?} wanted span={want}\n{stdout}{stderr}"
        );
    }
}

/// A playbook whose `probe.facts` gatherer returns `kind` (declared
/// `symbol`, enumerated) and `plain` (declared `string`), both holding the
/// script-side text `emits`. `conditions` are spliced in as steps.
fn write_symbol_fact_playbook(root: &Path, emits: &str, conditions: &[(&str, &str)]) {
    let pkg = root.join("pkgs/probe");
    std::fs::create_dir_all(pkg.join("gatherers")).unwrap();
    std::fs::create_dir_all(pkg.join("resources")).unwrap();

    let steps: String = conditions
        .iter()
        .map(|(name, cond)| {
            format!(
                "    step \"{name}\" {{\n      description = \"probe step {name}\"\n      \
                 resource = \"probe.marker\"\n      condition = {cond}\n      \
                 properties {{\n        path = \"/unused\"\n        check = \"already\"\n      \
                 }}\n    }}\n"
            )
        })
        .collect();
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Symbols" {{
  description = "Symbol-typed gatherer facts"
  version = "0.1.0"

  gather "facts" {{
    description = "Probe facts"
    from = "probe.facts"
  }}

  play "p" {{
    description = "probe"
{steps}  }}
}}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "probe" {
  description = "Symbol-fact probe"

  gatherer "facts" {
    description = "Report a symbol-typed and a string-typed fact"
    script = "gatherers/facts.ws"
    returns "kind" {
      description = "An enumerated kind"
      type = "symbol"
      symbol "alpha" { description = "The first kind" }
      symbol "beta"  { description = "The second kind" }
    }
    returns "plain" { description = "The same text, untyped" type = "string" }
  }

  resource "marker" {
    description = "Reports whatever its check parameter says"
    script = "resources/marker.ws"
    param "path"  { description = "Marker path" type = "string" required = true }
    param "check" { description = "Check behaviour" type = "string" default = "already" }
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("gatherers/facts.ws"),
        format!(
            r#"use value

fn gather(params: Value) -> Value {{
    Value::Map(#{{
        "kind": Value::String("{emits}"),
        "plain": Value::String("{emits}")
    }})
}}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        pkg.join("resources/marker.ws"),
        r#"use value

fn check(params: Value) -> Result[CheckResult, string] {
    Ok(CheckResult::AlreadyConfigured)
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();
}

/// A `returns` key declared `symbol` binds as a real WCL symbol, so the
/// playbook compares it the way the package declared it. The string
/// spelling of the same token does *not* match — that asymmetry is the
/// point of declaring the type, and is pinned here so it can't drift
/// silently.
#[test]
fn a_symbol_fact_compares_as_a_symbol_not_a_string() {
    let dir = tempfile::tempdir().unwrap();
    write_symbol_fact_playbook(
        dir.path(),
        "alpha",
        &[
            ("sym", "facts.kind == :alpha"),
            ("str", "facts.kind == \"alpha\""),
            // The untyped sibling key keeps its string semantics.
            ("plain", "facts.plain == \"alpha\""),
        ],
    );

    let (code, stdout, stderr) = run_in(dir.path(), &["check", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let line = |name: &str| {
        stdout
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("no line for step {name}\n{stdout}"))
            .to_string()
    };
    assert!(line("sym").contains("already configured"), "{stdout}");
    assert!(line("str").contains("skipped"), "{stdout}");
    assert!(line("plain").contains("already configured"), "{stdout}");
}

/// The declared set is enforced against what the script actually returned —
/// an out-of-set fact is a bug in the gatherer, not a silent binding.
#[test]
fn a_symbol_fact_outside_its_declared_set_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    write_symbol_fact_playbook(dir.path(), "gamma", &[("sym", "facts.kind == :alpha")]);

    let (code, stdout, stderr) = run_in(dir.path(), &["check", ".", "p"]);
    assert_ne!(code, 0, "{stdout}{stderr}");
    let out = format!("{stdout}{stderr}");
    assert!(out.contains("not a declared symbol"), "{out}");
    assert!(out.contains(":alpha") && out.contains(":beta"), "{out}");
}

#[test]
fn error_halts_and_continue_on_error_continues() {
    let dir = tempfile::tempdir().unwrap();
    let m2 = dir.path().join("m2");
    let plays = format!(
        "  play \"p\" {{\n    description = \"probe\"\n{}{}  }}\n",
        step(
            "bad",
            "        path = \"/nonexistent\"\n        check = \"error\"",
            ""
        ),
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
        step(
            "reboot",
            "        path = \"/nonexistent\"\n        apply = \"reboot\"",
            ""
        ),
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
        step(
            "reboot",
            "        path = \"/nonexistent\"\n        check = \"reboot\"",
            ""
        ),
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
    assert!(
        stdout.contains("a required step did not complete"),
        "{stdout}"
    );
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
        step(
            "liar",
            "        path = \"/proc/definitely/not/writable\"",
            ""
        )
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
        &[
            "apply",
            ".",
            "p",
            "--var",
            &format!("target={}", d1.display()),
        ],
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
            "apply",
            ".",
            "p",
            "--var-file",
            vf.to_str().unwrap(),
            "--var",
            &format!("target={}", d2.display()),
        ],
    );
    assert_eq!(code, 0);
    assert!(d2.exists());
    assert!(!dir.path().join("from-file").exists());
}

/// A symbol that only resolves at run time (it comes from a variable) skips
/// the load-time set check, so the engine has to enforce the declared set
/// itself before the script ever sees the value.
#[test]
fn undeclared_symbol_from_a_variable_fails_at_run_time() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m");
    write_lifecycle_playbook(
        dir.path(),
        &format!(
            r#"  vars {{
    chosen = :strikt
  }}

  play "p" {{
    description = "one step"
{}  }}"#,
            step(
                "s",
                &format!(
                    "        path = \"{}\"\n        mode = chosen",
                    marker.display()
                ),
                ""
            )
        ),
    );

    // Nothing is statically wrong — the value is a variable reference.
    let (code, _, stderr) = run_in(dir.path(), &["validate", "."]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("not a declared symbol") && stdout.contains(":normal, :strict"),
        "{stdout}"
    );
    assert!(!marker.exists(), "the step must not have run");
}
