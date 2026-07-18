//! `config-weave pkg` end-to-end against a local file:// fixture repo —
//! no network. The fixture repo is registered FIRST, so the stdlib
//! seeding path (which would clone GitHub) never triggers.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A bare "remote" with pkgs/demo seeded through a work clone.
/// Returns (bare url, seed clone path).
fn fixture_remote(root: &Path) -> (String, PathBuf) {
    let bare = root.join("remote.git");
    let seed = root.join("seed");
    git(
        &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
        root,
    );
    git(
        &["clone", bare.to_str().unwrap(), seed.to_str().unwrap()],
        root,
    );
    std::fs::create_dir_all(seed.join("pkgs/demo/resources")).unwrap();
    std::fs::write(
        seed.join("pkgs/demo/package.wcl"),
        r#"package "demo" {
  description = "Manages demonstration widgets"

  resource "widget" {
    description = "Ensure a widget exists"
    script = "resources/widget.wscript"
    concurrency = "parallel"
    param "name" { description = "Widget name" type = "string" required = true }
  }
}
"#,
    )
    .unwrap();
    std::fs::write(seed.join("pkgs/demo/resources/widget.wscript"), "// stub\n").unwrap();
    push(&seed, "seed");
    (format!("file://{}", bare.display()), seed)
}

fn push(seed: &Path, msg: &str) {
    git(&["add", "-A"], seed);
    git(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@test",
            "commit",
            "-m",
            msg,
        ],
        seed,
    );
    git(&["push", "origin", "main"], seed);
}

/// Run `config-weave pkg …` in `dir`; returns (exit code, stdout, stderr).
fn pkg(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .arg("pkg")
        .args(args)
        .args(["--dir", dir.to_str().unwrap()])
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn repo_wcl(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("pkgs/repo.wcl")).unwrap()
}

#[test]
fn pkg_lifecycle_against_a_local_fixture_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, seed) = fixture_remote(tmp.path());
    let pb = tmp.path().join("pb");
    std::fs::create_dir_all(&pb).unwrap();

    // Register the fixture repo — this also syncs it immediately.
    let (code, stdout, stderr) = pkg(&pb, &["repo", "add", "fixture", &url, "--subdir", "pkgs"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("registered repo 'fixture'"), "{stdout}");
    assert!(stdout.contains("synced ("), "{stdout}");
    assert!(pb.join(".repo-cache/fixture/.git").exists());

    // list shows the cache state.
    let (code, stdout, _) = pkg(&pb, &["repo", "list"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("fixture") && stdout.contains("pkgs"),
        "{stdout}"
    );

    // add installs, records provenance, and never seeds the stdlib
    // (the fixture repo already exists).
    let (code, stdout, stderr) = pkg(&pb, &["add", "demo"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("installed 'demo' from 'fixture'"),
        "{stdout}"
    );
    assert!(!stdout.contains("seeded"), "{stdout}");
    assert!(pb.join("pkgs/demo/package.wcl").exists());
    assert!(pb.join("pkgs/demo/resources/widget.wscript").exists());
    let recorded = repo_wcl(&pb);
    assert!(recorded.contains("package \"demo\""), "{recorded}");
    assert!(recorded.contains("repo = \"fixture\""), "{recorded}");
    let commit_line = recorded
        .lines()
        .find(|l| l.trim_start().starts_with("commit ="))
        .expect("commit recorded");
    let commit = commit_line.split('"').nth(1).unwrap();
    assert_eq!(commit.len(), 40, "full sha recorded: {commit_line}");

    // Second add errors.
    let (code, _, stderr) = pkg(&pb, &["add", "demo"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("already installed"), "{stderr}");

    // Update with nothing new.
    let (code, stdout, _) = pkg(&pb, &["update"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("demo: up to date"), "{stdout}");

    // Remote moves; update re-copies and re-records.
    std::fs::write(
        seed.join("pkgs/demo/resources/widget.wscript"),
        "// improved\n",
    )
    .unwrap();
    push(&seed, "improve widget");
    let (code, stdout, stderr) = pkg(&pb, &["update", "demo"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("demo: ") && stdout.contains(" -> "),
        "{stdout}"
    );
    let script = std::fs::read_to_string(pb.join("pkgs/demo/resources/widget.wscript")).unwrap();
    assert_eq!(script, "// improved\n");
    assert!(!repo_wcl(&pb).contains(commit), "commit was re-recorded");

    // Search matches names and descriptions, marks installed.
    let (code, stdout, _) = pkg(&pb, &["search", "widget"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("demo") && stdout.contains("[installed]"),
        "{stdout}"
    );
    let (code, stdout, _) = pkg(&pb, &["search", "zzz-nomatch"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("no packages matching"), "{stdout}");

    // Remove deletes the dir and the entry; a second remove errors.
    let (code, _, stderr) = pkg(&pb, &["remove", "demo"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!pb.join("pkgs/demo").exists());
    assert!(!repo_wcl(&pb).contains("package \"demo\""));
    let (code, _, stderr) = pkg(&pb, &["remove", "demo"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("not installed"), "{stderr}");

    // Unknown package lists the searched repos.
    let (code, _, stderr) = pkg(&pb, &["add", "nosuch"]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("not found") && stderr.contains("fixture"),
        "{stderr}"
    );
}

#[test]
fn untracked_dirs_are_never_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, _seed) = fixture_remote(tmp.path());
    let pb = tmp.path().join("pb");
    std::fs::create_dir_all(pb.join("pkgs/demo")).unwrap();
    std::fs::write(pb.join("pkgs/demo/handmade.txt"), "mine\n").unwrap();

    let (code, _, stderr) = pkg(&pb, &["repo", "add", "fixture", &url, "--subdir", "pkgs"]);
    assert_eq!(code, 0, "{stderr}");

    // add refuses to overwrite an untracked pkgs/demo.
    let (code, _, stderr) = pkg(&pb, &["add", "demo"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert!(pb.join("pkgs/demo/handmade.txt").exists());

    // remove refuses to delete a dir it does not track.
    let (code, _, stderr) = pkg(&pb, &["remove", "demo"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("delete it manually"), "{stderr}");
    assert!(pb.join("pkgs/demo/handmade.txt").exists());
}
