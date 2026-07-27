//! M1 gate tests: the sample playbook validates; introduced errors fail
//! with diagnostics and exit code 2.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample")
}

/// Copy the sample playbook into a temp dir so tests can break it.
fn copy_sample(to: &Path) {
    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let dest = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_dir(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), &dest).unwrap();
            }
        }
    }
    copy_dir(&sample(), to);
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn sample_validates() {
    let (code, stdout, stderr) = run(&["validate", sample().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("Sample Baseline"), "{stdout}");
}

#[test]
fn list_shows_plays() {
    let (code, stdout, _) = run(&["list", sample().to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("baseline"));
    assert!(stdout.contains("noop"));
}

#[test]
fn script_typo_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let script = dir.path().join("pkgs/core/resources/file_present.ws");
    let src = std::fs::read_to_string(&script).unwrap();
    std::fs::write(&script, src.replace("log::info", "log::inof")).unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("inof"), "{stderr}");
}

#[test]
fn bad_entrypoint_signature_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let script = dir.path().join("pkgs/core/resources/file_present.ws");
    let src = std::fs::read_to_string(&script).unwrap();
    // Rename check so the contract is unsatisfied.
    std::fs::write(&script, src.replace("fn check(", "fn checkk(")).unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("check"), "{stderr}");
}

#[test]
fn unknown_property_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let pb = dir.path().join("playbook.wcl");
    let src = std::fs::read_to_string(&pb).unwrap();
    std::fs::write(
        &pb,
        src.replace("path = marker_a", "path = marker_a\n        bogus = 1"),
    )
    .unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("bogus"), "{stderr}");
}

#[test]
fn missing_required_param_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let pb = dir.path().join("playbook.wcl");
    let src = std::fs::read_to_string(&pb).unwrap();
    // Drop the required `path` property from make-a.
    std::fs::write(&pb, src.replace("path = marker_a\n", "")).unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("missing required parameter 'path'"),
        "{stderr}"
    );
}

#[test]
fn missing_description_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let pb = dir.path().join("playbook.wcl");
    let src = std::fs::read_to_string(&pb).unwrap();
    std::fs::write(
        &pb,
        src.replace("      description = \"Create the first marker file\"\n", ""),
    )
    .unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("missing required field 'description'"),
        "{stderr}"
    );
}

#[test]
fn requires_cycle_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let pb = dir.path().join("playbook.wcl");
    let src = std::fs::read_to_string(&pb).unwrap();
    std::fs::write(
        &pb,
        src.replace(
            "condition = is_linux",
            "condition = is_linux\n      requires = [\"make-b\"]",
        ),
    )
    .unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("cycle"), "{stderr}");
}

#[test]
fn unknown_resource_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());
    let pb = dir.path().join("playbook.wcl");
    let src = std::fs::read_to_string(&pb).unwrap();
    std::fs::write(&pb, src.replace("core.file_present", "core.nope")).unwrap();

    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no resource 'nope'"), "{stderr}");
}

// ------------------------------------------------- enumerated symbol params

/// miette wraps diagnostics inside box art, so a message can break across
/// lines mid-phrase. Collapse the gutter and whitespace before asserting.
fn flat(s: &str) -> String {
    s.replace(['\u{2502}', '\u{00d7}'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A copy of the sample whose `core.file_present` grows `param_body`, and
/// whose `make-a` step gains `prop_line` in its `properties` block.
/// Returns the temp dir (kept alive by the caller).
fn sample_with_param(param_body: &str, prop_line: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());

    let pkg = dir.path().join("pkgs/core/package.wcl");
    let src = std::fs::read_to_string(&pkg).unwrap();
    let anchor = "    param \"content\" {\n      description = \"File content\"\n      \
                  type = \"string\"\n      default = \"\"\n    }\n";
    assert!(src.contains(anchor), "sample package.wcl drifted");
    std::fs::write(&pkg, src.replace(anchor, &format!("{anchor}{param_body}"))).unwrap();

    if let Some(prop) = prop_line {
        let pb = dir.path().join("playbook.wcl");
        let src = std::fs::read_to_string(&pb).unwrap();
        std::fs::write(
            &pb,
            src.replace(
                "        content = \"alpha\"\n",
                &format!("        content = \"alpha\"\n        {prop}\n"),
            ),
        )
        .unwrap();
    }
    dir
}

/// `sample_with_param` for the symbol tests, which all set `ensure`.
fn sample_with_symbol_param(param_body: &str, prop: Option<&str>) -> tempfile::TempDir {
    sample_with_param(param_body, prop.map(|p| format!("ensure = {p}")).as_deref())
}

const ENSURE_PARAM: &str = r#"    param "ensure" {
      description = "Desired state"
      type = "symbol"
      default = :present
      symbol "present" { description = "Create it" }
      symbol "absent"  { description = "Remove it" }
    }
"#;

#[test]
fn declared_symbol_is_accepted() {
    let dir = sample_with_symbol_param(ENSURE_PARAM, Some(":absent"));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn undeclared_symbol_fails_validation() {
    let dir = sample_with_symbol_param(ENSURE_PARAM, Some(":presnt"));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(flat(&stderr).contains("not a declared symbol"), "{stderr}");
    // The diagnostic names the whole legal set, not just the failure.
    assert!(
        flat(&stderr).contains(":present") && flat(&stderr).contains(":absent"),
        "{stderr}"
    );
}

#[test]
fn symbol_default_outside_its_own_set_fails_validation() {
    let param = ENSURE_PARAM.replace("default = :present", "default = :nope");
    let dir = sample_with_symbol_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("default for parameter 'ensure' is not a declared symbol"),
        "{stderr}"
    );
}

#[test]
fn symbol_blocks_on_a_non_symbol_param_fail_validation() {
    let param = ENSURE_PARAM
        .replace("type = \"symbol\"", "type = \"string\"")
        .replace("default = :present", "default = \"present\"");
    let dir = sample_with_symbol_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("declares symbol values but its type is string"),
        "{stderr}"
    );
}

#[test]
fn duplicate_symbol_fails_validation() {
    let param = ENSURE_PARAM.replace(
        "symbol \"absent\"  { description = \"Remove it\" }",
        "symbol \"present\" { description = \"Again\" }",
    );
    let dir = sample_with_symbol_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("duplicate symbol ':present'"),
        "{stderr}"
    );
}

/// Enumeration is opt-in: a symbol param that declares no values keeps
/// accepting any token, which is what every symbol param did before the
/// `symbol` block existed.
#[test]
fn symbol_param_without_declared_values_stays_open() {
    let param = "    param \"ensure\" {\n      description = \"Desired state\"\n      \
                 type = \"symbol\"\n      default = :present\n    }\n";
    let dir = sample_with_symbol_param(param, Some(":anything_at_all"));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

/// A symbol param must be *written* as `:name`. Both spellings reach
/// scripts as the same text, so nothing but the source form distinguishes
/// them — accepting the string would leave two ways to say one thing.
#[test]
fn string_spelling_of_a_symbol_fails_validation() {
    let dir = sample_with_symbol_param(ENSURE_PARAM, Some("\"absent\""));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("is a symbol: write :absent, not \"absent\""),
        "{stderr}"
    );
}

/// The rule holds even when the param enumerates nothing.
#[test]
fn string_spelling_fails_for_an_open_symbol_param_too() {
    let param = "    param \"ensure\" {\n      description = \"Desired state\"\n      \
                 type = \"symbol\"\n      default = :present\n    }\n";
    let dir = sample_with_symbol_param(param, Some("\"whatever\""));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("is a symbol: write :whatever"),
        "{stderr}"
    );
}

#[test]
fn string_spelling_of_a_symbol_default_fails_validation() {
    let param = ENSURE_PARAM.replace("default = :present", "default = \"present\"");
    let dir = sample_with_symbol_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("default for parameter 'ensure' is a symbol"),
        "{stderr}"
    );
}

// ------------------------------------------------- symbol-typed `returns`

/// A copy of the sample whose `core.os_info` gatherer declares `returns`
/// blocks, and whose `file_present_converges` test expects `family =
/// <expect>` (the sample's own expectation, rewritten).
fn sample_with_returns(returns_body: &str, expect: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_sample(dir.path());

    let pkg = dir.path().join("pkgs/core/package.wcl");
    let src = std::fs::read_to_string(&pkg).unwrap();
    let anchor = "    script = \"gatherers/os_info.ws\"\n";
    assert!(src.contains(anchor), "sample package.wcl drifted");
    let src = src.replace(anchor, &format!("{anchor}{returns_body}"));

    let expect_anchor = "        family = \"linux\"\n";
    assert!(src.contains(expect_anchor), "sample expect block drifted");
    std::fs::write(
        &pkg,
        src.replace(expect_anchor, &format!("        family = {expect}\n")),
    )
    .unwrap();
    dir
}

const FAMILY_RETURNS: &str = r#"    returns "family" {
      description = "OS family"
      type = "symbol"
      symbol "linux"   { description = "Any Linux" }
      symbol "windows" { description = "Any Windows" }
    }
"#;

#[test]
fn a_symbol_returns_key_accepts_the_symbol_spelling() {
    let dir = sample_with_returns(FAMILY_RETURNS, ":linux");
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

/// A symbol-typed fact binds as a WCL symbol, so an expectation written as
/// a string would compare against a spelling the variable space never
/// holds — the same rule symbol params already enforce.
#[test]
fn a_string_expectation_of_a_symbol_returns_key_fails_validation() {
    let dir = sample_with_returns(FAMILY_RETURNS, "\"linux\"");
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("is a symbol: write :linux, not \"linux\""),
        "{stderr}"
    );
}

#[test]
fn an_expectation_outside_a_returns_symbol_set_fails_validation() {
    let dir = sample_with_returns(FAMILY_RETURNS, ":plan9");
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    let out = flat(&stderr);
    assert!(out.contains("is not a declared symbol"), "{stderr}");
    assert!(
        out.contains(":linux") && out.contains(":windows"),
        "{stderr}"
    );
}

#[test]
fn symbol_blocks_on_a_non_symbol_returns_key_fail_validation() {
    let returns = FAMILY_RETURNS.replace("type = \"symbol\"", "type = \"string\"");
    let dir = sample_with_returns(&returns, "\"linux\"");
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("returns key 'family' declares symbol values"),
        "{stderr}"
    );
}

/// Enumeration stays opt-in on the returns side too, and an undeclared
/// expectation key is still fine — a gathered map may carry dynamic keys.
#[test]
fn an_open_symbol_returns_key_accepts_any_token() {
    let returns = "    returns \"family\" { description = \"OS family\" type = \"symbol\" }\n";
    let dir = sample_with_returns(returns, ":anything_at_all");
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn duplicate_symbol_on_a_returns_key_fails_validation() {
    let returns = FAMILY_RETURNS.replace(
        "symbol \"windows\" { description = \"Any Windows\" }",
        "symbol \"linux\" { description = \"Again\" }",
    );
    let dir = sample_with_returns(&returns, ":linux");
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("duplicate symbol ':linux' for returns key 'family'"),
        "{stderr}"
    );
}

// ------------------------------------------------------- duration params

const MAX_AGE_PARAM: &str = r#"    param "max_age" {
      description = "Refresh when the last update is older than this"
      type = "duration"
      default = 24h
    }
"#;

/// A `duration` param is authored as a bare WCL unit literal. The
/// `properties` block is `@schemaless`, so the loader has to resolve the
/// literal against `std.Duration` itself — see `convert::field_value_dyn`.
#[test]
fn a_duration_unit_literal_is_accepted() {
    let dir = sample_with_param(MAX_AGE_PARAM, Some("max_age = 30min"));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

/// The quoted spelling is not a duration — that would reintroduce the
/// hand-rolled `"30m"` parsing the unit type exists to remove.
#[test]
fn a_quoted_duration_fails_validation() {
    let dir = sample_with_param(MAX_AGE_PARAM, Some("max_age = \"30min\""));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("expects duration, got string"),
        "{stderr}"
    );
}

/// A literal from another unit family reports the *unit* problem, not a
/// coarse type mismatch — the retry against `std.Duration` must not
/// swallow the original diagnostic.
#[test]
fn a_unit_from_another_family_reports_the_unit_error() {
    let dir = sample_with_param(MAX_AGE_PARAM, Some("max_age = 4GiB"));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    let out = flat(&stderr);
    assert!(out.contains("GiB"), "{stderr}");
    assert!(!out.contains("expects duration, got"), "{stderr}");
}

/// Nanoseconds are `std.Duration`'s base unit, so a bare integer is what a
/// script receives — but authoring one bypasses the units entirely and is
/// indistinguishable from a mistake, so `int` is not a duration.
#[test]
fn a_bare_integer_is_not_a_duration() {
    let dir = sample_with_param(MAX_AGE_PARAM, Some("max_age = 1800"));
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn symbol_blocks_on_a_duration_param_fail_validation() {
    let param = MAX_AGE_PARAM.replace(
        "      default = 24h\n",
        "      default = 24h\n      symbol \"fast\" { description = \"Nope\" }\n",
    );
    let dir = sample_with_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("declares symbol values but its type is duration"),
        "{stderr}"
    );
}

#[test]
fn a_quoted_duration_default_fails_validation() {
    let param = MAX_AGE_PARAM.replace("default = 24h", "default = \"24h\"");
    let dir = sample_with_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        flat(&stderr).contains("default for parameter 'max_age' does not match"),
        "{stderr}"
    );
}

#[test]
fn an_unknown_param_type_names_duration_in_its_diagnostic() {
    let param = MAX_AGE_PARAM.replace("type = \"duration\"", "type = \"timespan\"");
    let dir = sample_with_param(&param, None);
    let (code, _, stderr) = run(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(flat(&stderr).contains("or duration"), "{stderr}");
}
