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
    run_with_env(args, &[])
}

fn run_with_env(args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args);
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

const PLAYBOOK: &str = r#"playbook "Testlab Fixture" {
  description = "Fixture playbook for testlab gate tests"
  version = "1.0.0"
}
"#;

const PACKAGE: &str = r#"package "tlab" {
  description = "Testlab fixture package"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.wscript"
  }

  resource "file_present" {
    description = "Ensure a file exists with the given content"
    script = "resources/file_present.wscript"

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
    verify = "tests/verify.wscript"

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
    std::fs::write(pkg.join("resources/file_present.wscript"), RESOURCE).unwrap();
    std::fs::write(pkg.join("gatherers/os_info.wscript"), GATHERER).unwrap();
    std::fs::write(pkg.join("tests/verify.wscript"), VERIFY).unwrap();
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
            "backend = \"chroot\"\n    image = \"debian:12\"",
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown test backend 'chroot'"), "{stderr}");
    assert!(stderr.contains("'docker', 'vmlab'"), "{stderr}");
}

#[test]
fn vmlab_backend_is_accepted_by_validation() {
    let dir = fixture_with(|s| {
        s.replace(
            "image = \"debian:12\"",
            "backend = \"vmlab\"\n    image = \"x86_64/linux-modern\"",
        )
    });
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 0, "{stderr}");
}

/// Helper: rewrite the fixture so the `converges` test joins group `g`,
/// then inject a sibling test in the same group built by `sibling`.
fn fixture_with_group_sibling(sibling: &str) -> tempfile::TempDir {
    let inject = format!("{sibling}\n\n  test \"converges\" {{");
    fixture_with(|s| {
        s.replace(
            "  test \"converges\" {\n    description = \"file_present creates the file and is idempotent\"\n    image = \"debian:12\"",
            "  test \"converges\" {\n    description = \"file_present creates the file and is idempotent\"\n    image = \"debian:12\"\n    group = \"g\"",
        )
        .replace("  test \"converges\" {", &inject)
    })
}

#[test]
fn group_with_mismatched_image_fails() {
    // A sibling in group "g" on a different image: a group provisions one
    // instance, so the members must agree.
    let dir = fixture_with_group_sibling(
        r#"  test "sibling" {
    description = "Same group, different image"
    image = "alpine:3"
    group = "g"

    step "create" {
      description = "Create a marker file"
      resource = "file_present"
      properties {
        path = "/var/tmp/sib.txt"
        content = "hi"
      }
    }
  }"#,
    );
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("group 'g'"), "{stderr}");
    assert!(stderr.contains("must agree"), "{stderr}");
}

#[test]
fn group_with_matching_backend_and_image_validates() {
    let dir = fixture_with_group_sibling(
        r#"  test "sibling" {
    description = "Same group, same image"
    image = "debian:12"
    group = "g"

    step "create" {
      description = "Create a marker file"
      resource = "file_present"
      properties {
        path = "/var/tmp/sib.txt"
        content = "hi"
      }
    }
  }"#,
    );
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 0, "{stderr}");
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
    let dir = fixture_with(|s| s.replace("tests/verify.wscript", "tests/missing.wscript"));
    let (code, _, stderr) = validate(dir.path());
    assert_eq!(code, 2);
    assert!(
        stderr.contains("verify script 'tests/missing.wscript' does not exist"),
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
    let verify = dir.path().join("pkgs/tlab/tests/verify.wscript");
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
    let script = dir.path().join("verify.wscript");
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
    let script = dir.path().join("verify.wscript");
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
    let script = dir.path().join("verify.wscript");
    std::fs::write(&script, "fn nothing() -> bool { true }\n").unwrap();
    let (code, _, stderr) = run(&["__verify", script.to_str().unwrap()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("verify"), "{stderr}");
}

// ----------------------------------------------------------- CLI surface

#[test]
fn test_without_any_tests_exits_2() {
    // Drop the test block entirely and re-close the package.
    let dir = fixture_with(|s| {
        let start = s.find("  test \"converges\"").unwrap();
        format!("{}}}\n", &s[..start])
    });
    let (code, _, stderr) = run(&["test", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(
        stderr.contains("no package declares any tests or scenarios"),
        "{stderr}"
    );
}

#[test]
fn test_with_bad_filter_lists_available() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let (code, _, stderr) = run(&["test", dir.path().to_str().unwrap(), "tlab:nope"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("nothing matches 'tlab:nope'"), "{stderr}");
    assert!(stderr.contains("tlab:converges"), "{stderr}");
}

#[test]
fn test_with_unknown_backend_override_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let (code, _, stderr) = run(&["test", dir.path().to_str().unwrap(), "--backend", "chroot"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown test backend 'chroot'"), "{stderr}");
}

#[test]
fn vmlab_backend_without_a_cli_exits_2() {
    // A vmlab test selects the vmlab backend; with discovery pointed at
    // a nonexistent CLI, the run fails fast with one clear diagnostic.
    let dir = fixture_with(|s| {
        s.replace(
            "image = \"debian:12\"",
            "backend = \"vmlab\"\n    image = \"x86_64/linux-modern\"",
        )
    });
    let (code, _, stderr) = run_with_env(
        &["test", dir.path().to_str().unwrap()],
        &[("CONFIG_WEAVE_VMLAB_CMD", "/nonexistent/vmlabctl")],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("/nonexistent/vmlabctl"), "{stderr}");
    assert!(stderr.contains("vmlab"), "{stderr}");
}

// ------------------------------------------------------------ docker-gated
// The real thing: containers, three engine runs, verify scripts. Run via
// `just test-lab`, which cross-builds the musl artifact and points
// CONFIG_WEAVE_TEST_BINARY at it. `#[ignore]` keeps `just test`
// docker-free; each test also skips with a message when the binary or a
// container CLI is missing.

/// The static binary for in-container runs, or a skip message.
fn lab_binary() -> Option<String> {
    match std::env::var("CONFIG_WEAVE_TEST_BINARY") {
        Ok(p) if Path::new(&p).is_file() => Some(p),
        Ok(p) => {
            eprintln!("skipping: CONFIG_WEAVE_TEST_BINARY={p} does not exist");
            None
        }
        Err(_) => {
            eprintln!("skipping: CONFIG_WEAVE_TEST_BINARY is not set (run via `just test-lab`)");
            None
        }
    }
}

fn docker_available() -> bool {
    for cmd in ["docker", "podman"] {
        if Command::new(cmd)
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    eprintln!("skipping: no working docker/podman");
    false
}

fn run_lab(dir: &Path, binary: &str, extra: &[&str]) -> (i32, String, String) {
    let mut args = vec!["test", dir.to_str().unwrap(), "--binary", binary, "--json"];
    args.extend_from_slice(extra);
    run(&args)
}

#[test]
#[ignore = "needs docker and a static binary (just test-lab)"]
fn lab_converge_test_passes() {
    let Some(binary) = lab_binary() else { return };
    if !docker_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    let (code, stdout, stderr) = run_lab(dir.path(), &binary, &[]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&format!("{stdout}{stderr}"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(v["mode"], "test");
    let t = &v["tests"][0];
    assert_eq!(t["outcome"], "passed", "{stdout}");
    let s = &t["steps"][0];
    assert_eq!(s["check"], "not_configured", "{stdout}");
    assert_eq!(s["apply"], "configured", "{stdout}");
    assert_eq!(s["second_apply"], "already_configured", "{stdout}");
    assert_eq!(t["gathers"][0]["failures"], serde_json::json!([]));
    assert_eq!(t["verify"]["passed"], true, "{stdout}");
}

#[test]
#[ignore = "needs docker and a static binary (just test-lab)"]
fn lab_grouped_tests_share_one_instance() {
    let Some(binary) = lab_binary() else { return };
    if !docker_available() {
        return;
    }
    // Two tests in group "shared": the first creates /var/tmp/tlab.txt, the
    // second's verify reads it back. The second only passes if both ran in
    // the SAME container — proof the group shares one instance.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkgs/tlab");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::create_dir_all(pkg.join("gatherers")).unwrap();
    std::fs::create_dir_all(pkg.join("tests")).unwrap();
    std::fs::write(dir.path().join("playbook.wcl"), PLAYBOOK).unwrap();
    std::fs::write(pkg.join("resources/file_present.wscript"), RESOURCE).unwrap();
    std::fs::write(pkg.join("gatherers/os_info.wscript"), GATHERER).unwrap();
    std::fs::write(pkg.join("tests/verify.wscript"), VERIFY).unwrap();
    std::fs::write(
        pkg.join("tests/shared.wscript"),
        r#"use value
use fs

fn verify(facts: Value) -> Result[bool, string] {
    Ok(fs::read("/var/tmp/tlab.txt")? == "hello")
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "tlab" {
  description = "Testlab fixture package"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.wscript"
  }

  resource "file_present" {
    description = "Ensure a file exists with the given content"
    script = "resources/file_present.wscript"

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

  test "creates" {
    description = "First grouped test creates the shared file"
    image = "debian:12"
    group = "shared"
    verify = "tests/verify.wscript"

    step "create" {
      description = "Create a marker file"
      resource = "file_present"
      properties {
        path = "/var/tmp/tlab.txt"
        content = "hello"
      }
    }
  }

  test "reuses" {
    description = "Second grouped test sees the first test's file"
    image = "debian:12"
    group = "shared"
    verify = "tests/shared.wscript"

    gather "os" {
      description = "OS facts inside the container"
      from = "os_info"
      expect {
        family = "linux"
      }
    }
  }
}
"#,
    )
    .unwrap();

    let (code, stdout, stderr) = run_lab(dir.path(), &binary, &[]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&format!("{stdout}{stderr}"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    let tests = v["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 2, "{stdout}");
    // Selection order is preserved: creates first, reuses second.
    assert_eq!(tests[0]["name"], "creates", "{stdout}");
    assert_eq!(tests[0]["outcome"], "passed", "{stdout}");
    assert_eq!(tests[1]["name"], "reuses", "{stdout}");
    assert_eq!(tests[1]["outcome"], "passed", "{stdout}");
    assert_eq!(tests[1]["verify"]["passed"], true, "{stdout}");
}

#[test]
#[ignore = "needs docker and a static binary (just test-lab)"]
fn lab_expected_error_test_passes() {
    let Some(binary) = lab_binary() else { return };
    if !docker_available() {
        return;
    }
    // file_present errors on an empty path; expect = "error" makes that
    // the passing outcome.
    let dir = fixture_with(|s| {
        s.replace(
            "  test \"converges\" {",
            r#"  test "rejects_bad_path" {
    description = "file_present errors on an empty path"
    image = "debian:12"

    step "bad" {
      description = "Apply with an invalid path"
      resource = "file_present"
      expect = "error"
      properties {
        path = ""
      }
    }
  }

  test "converges" {"#,
        )
    });

    let (code, stdout, stderr) = run_lab(dir.path(), &binary, &["tlab:rejects_bad_path"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&format!("{stdout}{stderr}"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(v["tests"][0]["outcome"], "passed", "{stdout}");
    assert_eq!(v["tests"][0]["steps"][0]["apply"], "error", "{stdout}");
}

#[test]
#[ignore = "needs docker and a static binary (just test-lab)"]
fn lab_non_idempotent_resource_fails_second_apply() {
    let Some(binary) = lab_binary() else { return };
    if !docker_available() {
        return;
    }
    // The signature regression test: apply works but check never
    // recognizes the applied state, so a fresh process re-applies and
    // the second apply reports `configured` instead of
    // `already_configured` — the runner must fail the test.
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let pkg = dir.path().join("pkgs/tlab");
    std::fs::write(
        pkg.join("resources/amnesiac.wscript"),
        r#"use value
use fs

fn check(params: Value) -> CheckResult {
    CheckResult::NotConfigured
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    fs::write("/var/tmp/amnesiac.txt", "x")?
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();
    let manifest = pkg.join("package.wcl");
    let src = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        src.replace(
            "  test \"converges\" {",
            r#"  resource "amnesiac" {
    description = "Applies but never remembers"
    script = "resources/amnesiac.wscript"
  }

  test "amnesiac_is_caught" {
    description = "A non-idempotent resource must fail the test"
    image = "debian:12"

    step "apply-it" {
      description = "Apply the amnesiac resource"
      resource = "amnesiac"
    }
  }

  test "converges" {"#,
        ),
    )
    .unwrap();

    let (code, stdout, stderr) = run_lab(dir.path(), &binary, &["tlab:amnesiac_is_caught"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&format!("{stdout}{stderr}"));
    assert_eq!(code, 1, "{stdout}{stderr}");
    let t = &v["tests"][0];
    assert_eq!(t["outcome"], "failed", "{stdout}");
    // Run 2's internal re-check already catches it (apply claims success,
    // check still disagrees -> error); the second apply stays broken too.
    let failures = t["steps"][0]["failures"].as_array().unwrap();
    assert!(!failures.is_empty(), "{stdout}");
}

#[test]
#[ignore = "needs docker and a static binary (just test-lab)"]
fn lab_setup_preconfigures_state() {
    let Some(binary) = lab_binary() else { return };
    if !docker_available() {
        return;
    }
    // setup writes the file beforehand; expect = "already_configured"
    // asserts check sees it across all three runs.
    let dir = fixture_with(|s| {
        s.replace(
            "  test \"converges\" {",
            r#"  test "preconfigured" {
    description = "Setup pre-creates the file, check must see it"
    image = "debian:12"
    setup = "mkdir -p /var/tmp && printf hello > /var/tmp/pre.txt"

    step "already" {
      description = "Nothing to do"
      resource = "file_present"
      expect = "already_configured"
      properties {
        path = "/var/tmp/pre.txt"
        content = "hello"
      }
    }
  }

  test "converges" {"#,
        )
    });

    let (code, stdout, stderr) = run_lab(dir.path(), &binary, &["tlab:preconfigured"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&format!("{stdout}{stderr}"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(v["tests"][0]["outcome"], "passed", "{stdout}");
}

#[test]
#[ignore = "needs docker and a static binary (just test-lab)"]
fn lab_keep_leaves_a_container() {
    let Some(binary) = lab_binary() else { return };
    if !docker_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    let (code, stdout, stderr) = run_lab(dir.path(), &binary, &["tlab:converges", "--keep"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&format!("{stdout}{stderr}"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    let kept = v["tests"][0]["kept"].as_str().expect(&stdout);
    // "container <id> (image debian:12)"
    let id = kept.split_whitespace().nth(1).unwrap();
    let inspect = Command::new("docker")
        .args(["inspect", id])
        .output()
        .unwrap();
    assert!(inspect.status.success(), "kept container should exist");
    let _ = Command::new("docker").args(["rm", "-f", id]).output();
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
