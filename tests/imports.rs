//! Multi-file scripts: resource and gatherer scripts importing shared
//! helpers from a package's `lib/` or the playbook's, and the diagnostics
//! that come out when a helper is broken or missing.

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

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A playbook with one `probe.marker` resource whose script body is
/// supplied by the caller, so each test decides what it imports.
fn fixture(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let marker = root.join("m.txt");
    write(
        root,
        "playbook.wcl",
        &format!(
            r#"playbook "Imports" {{
  description = "Scripts importing shared helpers"
  play "p" {{
    description = "one step"
    step "s" {{
      description = "run the marker resource"
      resource = "probe.marker"
      properties {{ path = "{}" }}
    }}
  }}
}}
"#,
            marker.display()
        ),
    );
    write(
        root,
        "pkgs/probe/package.wcl",
        r#"package "probe" {
  description = "Imports a helper"

  resource "marker" {
    description = "Writes a helper-computed value"
    script = "resources/marker.ws"

    param "path" {
      description = "Marker file path"
      type = "string"
      required = true
    }
  }
}
"#,
    );
    write(root, "pkgs/probe/resources/marker.ws", script);
    dir
}

/// The resource body used by the happy-path tests: it calls
/// `helpers::payload()` and converges on writing that value.
const MARKER_USING_HELPER: &str = r#"use value
use fs
use helpers

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> Result[CheckResult, string] {
    let path = p(params, "path")
    if fs::exists(path) {
        let got = fs::read(path)?
        if got == helpers::payload() { return Ok(CheckResult::AlreadyConfigured) }
    }
    Ok(CheckResult::NotConfigured)
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    fs::write(p(params, "path"), helpers::payload())?
    Ok(ApplyResult::Success)
}
"#;

const HELPER: &str = r#"fn payload() -> string {
    "from-the-helper"
}
"#;

#[test]
fn a_resource_imports_a_helper_from_its_package_lib() {
    let dir = fixture(MARKER_USING_HELPER);
    write(dir.path(), "pkgs/probe/lib/helpers.ws", HELPER);

    let (code, stdout, stderr) = run_in(dir.path(), &["validate", "."]);
    assert_eq!(code, 0, "{stdout}{stderr}");

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "from-the-helper"
    );

    // The convergence contract still holds across the import.
    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("already configured"), "{stdout}");
}

#[test]
fn a_resource_imports_a_helper_from_the_playbook_lib() {
    let dir = fixture(MARKER_USING_HELPER);
    write(dir.path(), "lib/helpers.ws", HELPER);

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "from-the-helper"
    );
}

#[test]
fn a_package_helper_shadows_the_playbook_one() {
    let dir = fixture(MARKER_USING_HELPER);
    write(dir.path(), "lib/helpers.ws", HELPER);
    write(
        dir.path(),
        "pkgs/probe/lib/helpers.ws",
        "fn payload() -> string {\n    \"from-the-package\"\n}\n",
    );

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "from-the-package"
    );
}

#[test]
fn a_path_import_resolves_next_to_the_importing_script() {
    let dir =
        fixture(&MARKER_USING_HELPER.replace("use helpers", r#"use "./helpers.ws" as helpers"#));
    write(dir.path(), "pkgs/probe/resources/helpers.ws", HELPER);

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "from-the-helper"
    );
}

#[test]
fn a_helper_may_import_another_helper() {
    let dir = fixture(MARKER_USING_HELPER);
    write(
        dir.path(),
        "pkgs/probe/lib/helpers.ws",
        "use inner\n\nfn payload() -> string {\n    inner::value()\n}\n",
    );
    write(
        dir.path(),
        "pkgs/probe/lib/inner.ws",
        "fn value() -> string {\n    \"two-deep\"\n}\n",
    );

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "two-deep"
    );
}

#[test]
fn a_missing_helper_fails_validation_at_the_use_line() {
    let dir = fixture(MARKER_USING_HELPER);
    // No lib/ anywhere, so `use helpers` resolves to nothing.
    let (code, _, stderr) = run_in(dir.path(), &["validate", "."]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("unknown module `helpers`"), "{stderr}");
    assert!(stderr.contains("resources/marker.ws"), "{stderr}");
}

/// The payoff of routing diagnostics through the compilation's source
/// map: an error inside an imported helper must be reported against the
/// *helper's* path and text, not the importing script's.
#[test]
fn an_error_inside_a_helper_is_reported_against_the_helper() {
    let dir = fixture(MARKER_USING_HELPER);
    write(
        dir.path(),
        "pkgs/probe/lib/helpers.ws",
        "fn payload() -> string {\n    this_function_does_not_exist()\n}\n",
    );

    let (code, _, stderr) = run_in(dir.path(), &["validate", "."]);
    assert_eq!(code, 2, "{stderr}");
    assert!(
        stderr.contains("lib/helpers.ws"),
        "diagnostic does not name the helper:\n{stderr}"
    );
    assert!(
        stderr.contains("this_function_does_not_exist"),
        "diagnostic does not quote the helper's source:\n{stderr}"
    );
    assert!(
        !stderr.contains("resources/marker.ws"),
        "diagnostic blamed the importing script:\n{stderr}"
    );
}

/// A broken helper is caught even when nothing imports it — `lib/` is
/// still compiled during validation.
#[test]
fn a_broken_helper_fails_validation_even_when_unimported() {
    let dir = fixture(
        &MARKER_USING_HELPER
            .replace("use helpers\n", "")
            .replace("helpers::payload()", "\"inline\""),
    );
    write(
        dir.path(),
        "pkgs/probe/lib/orphan.ws",
        "fn broken() -> string {\n    nope()\n}\n",
    );

    let (code, _, stderr) = run_in(dir.path(), &["validate", "."]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("orphan.ws"), "{stderr}");
}

/// `regex` and `xml` are re-exported from wscript-std alongside `json`
/// and `toml`; this proves both reach a resource script rather than only
/// appearing in the interface file.
#[test]
fn the_regex_and_xml_modules_are_usable_from_a_script() {
    let dir = fixture(
        r#"use value
use fs
use regex
use xml

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn payload() -> Result[string, string] {
    let doc = xml::parse("<cfg><name>weave</name></cfg>")?
    let rendered = xml::to_string(doc)?
    // regex takes (pattern, text), like is_match.
    let found = regex::find("[a-z]+eave", rendered)
    if let Some(m) = found { return Ok(m) }
    Err("no match")
}

fn check(params: Value) -> Result[CheckResult, string] {
    let path = p(params, "path")
    if fs::exists(path) {
        let got = fs::read(path)?
        if got == payload()? { return Ok(CheckResult::AlreadyConfigured) }
    }
    Ok(CheckResult::NotConfigured)
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    fs::write(p(params, "path"), payload()?)?
    Ok(ApplyResult::Success)
}
"#,
    );

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "weave"
    );
}

#[test]
fn a_host_module_still_wins_over_a_same_named_helper() {
    let dir = fixture(MARKER_USING_HELPER);
    write(dir.path(), "pkgs/probe/lib/helpers.ws", HELPER);
    // A file named after a registered host module must not shadow it:
    // `use fs` in the resource script still means the host API.
    write(
        dir.path(),
        "pkgs/probe/lib/fs.ws",
        "fn exists(path: string) -> bool {\n    false\n}\n",
    );

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    // The host `fs::write` ran, so the marker exists.
    assert!(dir.path().join("m.txt").exists());
}
