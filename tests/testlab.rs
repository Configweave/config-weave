//! Testlab gate tests: `test` blocks in package.wcl validate (or fail
//! with the right diagnostics), the in-container protocol subcommands
//! work on the host, and — docker-gated, `--ignored` — the full
//! `config-weave test` flow runs real containers.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const PLAYBOOK: &str = r#"playbook "Testlab Fixture" {
  description = "Fixture playbook for testlab gate tests"
  version = "1.0.0"
}
"#;

const PACKAGE: &str = r#"package "tlab" {
  description = "Testlab fixture package"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.wisp"
  }

  resource "file_present" {
    description = "Ensure a file exists with the given content"
    script = "resources/file_present.wisp"

    param "path" {
      description = "Absolute path of the file"
      type = "string"
      required = true
    }
    param "content" {
      description = "File content"
      type = "string"
      default = ""
    }
  }

  test "converges" {
    description = "file_present creates the file and is idempotent"
    image = "debian:12"
    verify = "tests/verify.wisp"

    step "create" {
      description = "Create a marker file"
      resource = "file_present"
      properties {
        path = "/var/tmp/tlab.txt"
        content = "hello"
      }
    }

    gather "os" {
      description = "OS facts inside the container"
      from = "os_info"
      expect {
        family = "linux"
      }
    }
  }
}
"#;

const RESOURCE: &str = r#"use value
use fs
use path
use log

fn param_str(params: Value, key: string, fallback: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() {
            return s
        }
    }
    fallback
}

fn check(params: Value) -> Result[CheckResult, string] {
    let p = param_str(params, "path", "")
    if p == "" {
        return Err("missing 'path' parameter")
    }
    if !fs::exists(p) {
        return Ok(CheckResult::NotConfigured)
    }
    let want = param_str(params, "content", "")
    let have = fs::read(p)?
    if have == want {
        Ok(CheckResult::AlreadyConfigured)
    } else {
        Ok(CheckResult::NotConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let p = param_str(params, "path", "")
    log::info("writing " + p)
    fs::mkdir(path::parent(p))?
    fs::write(p, param_str(params, "content", ""))?
    Ok(ApplyResult::Success)
}
"#;

const GATHERER: &str = r#"use value
use sys

fn gather(params: Value) -> Value {
    Value::Map(#{
        "family": Value::String(sys::family()),
        "name": Value::String(sys::os_name()),
        "version": Value::String(sys::os_version()),
        "arch": Value::String(sys::arch()),
        "cpus": Value::Int(sys::cpu_count())
    })
}
"#;

const VERIFY: &str = r#"use value
use fs

fn verify(facts: Value) -> Result[bool, string] {
    Ok(fs::read("/var/tmp/tlab.txt")? == "hello")
}
"#;

/// Write the fixture playbook into `dir`.
fn write_fixture(dir: &Path) {
    let pkg = dir.join("pkgs/tlab");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::create_dir_all(pkg.join("gatherers")).unwrap();
    std::fs::create_dir_all(pkg.join("tests")).unwrap();
    std::fs::write(dir.join("playbook.wcl"), PLAYBOOK).unwrap();
    std::fs::write(pkg.join("package.wcl"), PACKAGE).unwrap();
    std::fs::write(pkg.join("resources/file_present.wisp"), RESOURCE).unwrap();
    std::fs::write(pkg.join("gatherers/os_info.wisp"), GATHERER).unwrap();
    std::fs::write(pkg.join("tests/verify.wisp"), VERIFY).unwrap();
}

/// Fixture with `package.wcl` rewritten through `f`.
fn fixture_with(f: impl FnOnce(&str) -> String) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let pkg = dir.path().join("pkgs/tlab/package.wcl");
    let src = std::fs::read_to_string(&pkg).unwrap();
    std::fs::write(&pkg, f(&src)).unwrap();
    dir
}

fn validate(dir: &Path) -> (i32, String, String) {
    run(&["validate", dir.to_str().unwrap()])
}

// ------------------------------------------------------------ validation

#[test]
fn fixture_validates() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let (code, stdout, stderr) = validate(dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("Testlab Fixture"), "{stdout}");
}

#[test]
fn missing_test_description_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "    description = \"file_present creates the file and is idempotent\"\n",
            "",
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(
        stderr.contains("missing required field 'description'"),
        "{stderr}"
    );
}

#[test]
fn invalid_expect_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "resource = \"file_present\"",
            "resource = \"file_present\"\n      expect = \"explodes\"",
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("invalid expect 'explodes'"), "{stderr}");
}

#[test]
fn unknown_backend_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "image = \"debian:12\"",
            "backend = \"vmlab\"\n    image = \"debian:12\"",
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown test backend 'vmlab'"), "{stderr}");
}

#[test]
fn unknown_resource_in_test_fails() {
    let dir = fixture_with(|s| s.replace("resource = \"file_present\"", "resource = \"nope\""));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("no resource 'nope'"), "{stderr}");
}

#[test]
fn unknown_package_in_test_fails() {
    let dir =
        fixture_with(|s| s.replace("resource = \"file_present\"", "resource = \"other.thing\""));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown package 'other'"), "{stderr}");
}

#[test]
fn missing_verify_script_fails() {
    let dir = fixture_with(|s| s.replace("tests/verify.wisp", "tests/missing.wisp"));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(
        stderr.contains("verify script 'tests/missing.wisp' does not exist"),
        "{stderr}"
    );
}

#[test]
fn variable_reference_in_test_properties_fails() {
    let dir = fixture_with(|s| s.replace("path = \"/var/tmp/tlab.txt\"", "path = some_var"));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    // miette may wrap the message, so match a short fragment.
    assert!(stderr.contains("variable-free playbook"), "{stderr}");
}

#[test]
fn unknown_test_property_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "content = \"hello\"",
            "content = \"hello\"\n        bogus = 1",
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown parameter 'bogus'"), "{stderr}");
}

#[test]
fn test_property_type_mismatch_fails() {
    let dir = fixture_with(|s| s.replace("path = \"/var/tmp/tlab.txt\"", "path = 42"));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("expects string, got int"), "{stderr}");
}

#[test]
fn missing_required_test_property_fails() {
    let dir = fixture_with(|s| s.replace("path = \"/var/tmp/tlab.txt\"\n", ""));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(
        stderr.contains("missing required parameter 'path'"),
        "{stderr}"
    );
}

#[test]
fn success_step_requiring_failing_step_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "    gather \"os\" {",
            r#"    step "boom" {
      description = "Expected to error"
      resource = "file_present"
      expect = "error"
      properties {
        path = ""
      }
    }

    step "after" {
      description = "Depends on the failing step"
      resource = "file_present"
      requires = ["boom"]
      properties {
        path = "/var/tmp/after.txt"
      }
    }

    gather "os" {"#,
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("could never pass"), "{stderr}");
}

#[test]
fn broken_verify_signature_fails() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let verify = dir.path().join("pkgs/tlab/tests/verify.wisp");
    let src = std::fs::read_to_string(&verify).unwrap();
    std::fs::write(&verify, src.replace("fn verify(", "fn verifyy(")).unwrap();
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("verify"), "{stderr}");
}

#[test]
fn empty_test_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "  test \"converges\" {",
            r#"  test "empty" {
    description = "Declares nothing"
    image = "debian:12"
  }

  test "converges" {"#,
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(
        stderr.contains("test 'empty' declares no steps and no gathers"),
        "{stderr}"
    );
}

#[test]
fn duplicate_test_name_fails() {
    let dir = fixture_with(|s| {
        let test_block =
            s[s.find("  test \"converges\"").unwrap()..s.rfind('}').unwrap()].to_string();
        s.replace(
            "  test \"converges\" {",
            &format!("{test_block}\n\n  test \"converges\" {{"),
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("duplicate test 'converges'"), "{stderr}");
}

// ------------------------------------------------- in-container protocol
// `__gather` and `__verify` are what the testlab runner execs inside the
// container; both are host-runnable, so they test without docker.

#[test]
fn gather_one_prints_value_json() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let (code, stdout, stderr) = run(&["__gather", dir.path().to_str().unwrap(), "tlab.os_info"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(v["ok"], true, "{stdout}");
    assert!(v["value"]["family"].is_string(), "{stdout}");
    assert!(v["value"]["cpus"].is_i64(), "{stdout}");
}

#[test]
fn gather_one_unknown_gatherer_reports_error_json() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let (code, stdout, _) = run(&["__gather", dir.path().to_str().unwrap(), "tlab.nope"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(v["ok"], false, "{stdout}");
    assert!(
        v["error"].as_str().unwrap().contains("tlab.nope"),
        "{stdout}"
    );
}

#[test]
fn gather_one_rejects_bad_params_json() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let (code, stdout, _) = run(&[
        "__gather",
        dir.path().to_str().unwrap(),
        "tlab.os_info",
        "--params-json",
        "[1,2]",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(v["ok"], false, "{stdout}");
    assert!(
        v["error"].as_str().unwrap().contains("JSON object"),
        "{stdout}"
    );
}

#[test]
fn run_verify_passes_and_fails_on_state() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("checked.txt");
    let script = dir.path().join("verify.wisp");
    std::fs::write(
        &script,
        format!(
            r#"use value
use fs

fn verify(facts: Value) -> Result[bool, string] {{
    Ok(fs::read("{}")? == "expected")
}}
"#,
            target.display()
        ),
    )
    .unwrap();

    // No file yet: fs::read errors → verify fails with the message.
    let (code, stdout, _) = run(&["__verify", script.to_str().unwrap()]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("verify failed"), "{stdout}");

    std::fs::write(&target, "expected").unwrap();
    let (code, stdout, _) = run(&["__verify", script.to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("verify passed"), "{stdout}");

    std::fs::write(&target, "unexpected").unwrap();
    let (code, stdout, _) = run(&["__verify", script.to_str().unwrap()]);
    assert_eq!(code, 1, "{stdout}");
}

#[test]
fn run_verify_reads_facts_file() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("verify.wisp");
    std::fs::write(
        &script,
        r#"use value

fn verify(facts: Value) -> bool {
    if let Some(os) = facts.get("os") {
        if let Some(family) = os.get("family") {
            return family.as_string() == Some("linux")
        }
    }
    false
}
"#,
    )
    .unwrap();
    let facts = dir.path().join("facts.json");
    std::fs::write(&facts, r#"{"os":{"family":"linux"}}"#).unwrap();

    let (code, stdout, stderr) = run(&[
        "__verify",
        script.to_str().unwrap(),
        "--facts",
        facts.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stdout: {stdout} stderr: {stderr}");

    std::fs::write(&facts, r#"{"os":{"family":"windows"}}"#).unwrap();
    let (code, _, _) = run(&[
        "__verify",
        script.to_str().unwrap(),
        "--facts",
        facts.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
}

#[test]
fn run_verify_bad_contract_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("verify.wisp");
    std::fs::write(&script, "fn nothing() -> bool { true }\n").unwrap();
    let (code, _, stderr) = run(&["__verify", script.to_str().unwrap()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("verify"), "{stderr}");
}

#[test]
fn duplicate_step_name_in_test_fails() {
    let dir = fixture_with(|s| {
        s.replace(
            "    gather \"os\" {",
            r#"    step "create" {
      description = "Same name again"
      resource = "file_present"
      properties {
        path = "/var/tmp/other.txt"
      }
    }

    gather "os" {"#,
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(
        stderr.contains("duplicate step name 'create' in test 'converges'"),
        "{stderr}"
    );
}
