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
    let script = dir.path().join("pkgs/core/resources/file_present.wisp");
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
    let script = dir.path().join("pkgs/core/resources/file_present.wisp");
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
    assert!(stderr.contains("missing required parameter 'path'"), "{stderr}");
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
    assert!(stderr.contains("missing required field 'description'"), "{stderr}");
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
