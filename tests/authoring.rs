//! M7 gate: init → validate → docs produces a browsable site; wispi emits
//! the full host API for the LSP/wisp-check authoring loop.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

/// Resolve a runnable `wcl` binary (for the docs render step): `CONFIG_WEAVE_WCL`
/// if set, otherwise `wcl` from PATH. Returns `None` when neither can be spawned.
fn wcl_bin() -> Option<String> {
    let candidate = std::env::var("CONFIG_WEAVE_WCL").unwrap_or_else(|_| "wcl".into());
    Command::new(&candidate)
        .arg("--version")
        .output()
        .ok()
        .map(|_| candidate)
}

#[test]
fn wispi_emits_full_host_api() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .args(["wispi", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));

    let wispi = std::fs::read_to_string(dir.path().join("weave.wispi")).unwrap();
    // Every module is present on every platform (PRD §7), including the
    // Windows-only ones, plus the contract types.
    for module in [
        "mod log",
        "mod fs",
        "mod path",
        "mod shell",
        "mod http",
        "mod hash",
        "mod archive",
        "mod env",
        "mod sys",
        "mod data",
        "mod json",
        "mod toml",
        "mod registry",
        "mod service",
        "mod com",
    ] {
        assert!(wispi.contains(module), "missing `{module}` in weave.wispi");
    }
    for ty in [
        "enum CheckResult",
        "enum ApplyResult",
        "struct CmdOutput",
        "ComObject",
    ] {
        assert!(wispi.contains(ty), "missing `{ty}` in weave.wispi");
    }

    let manifest = std::fs::read_to_string(dir.path().join("wisp.toml")).unwrap();
    assert!(manifest.contains("weave.wispi"));
}

#[test]
fn init_validate_apply_docs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pb");

    // init
    let out = Command::new(bin())
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("playbook.wcl").exists());
    assert!(root.join("weave.wispi").exists());
    assert!(root.join("wisp.toml").exists());
    assert!(root.join("pkgs/example/package.wcl").exists());
    assert!(root.join("lib").is_dir());

    // init refuses to clobber
    let out = Command::new(bin())
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));

    // validate
    let out = Command::new(bin())
        .args(["validate", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // the scaffold actually converges (write into the temp dir)
    let target = dir.path().join("work");
    let out = Command::new(bin())
        .args([
            "apply",
            root.to_str().unwrap(),
            "baseline",
            "--var",
            &format!("work_root={}", target.display()),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(target.join("hello.txt").exists());

    // docs — `config-weave docs` shells out to the `wcl` CLI to render the
    // emitted wdoc source. Skip the rendered-HTML assertions when no `wcl`
    // binary is available, so `cargo test` does not hard-fail on machines
    // without it installed.
    let Some(wcl) = wcl_bin() else {
        eprintln!("skipping docs assertions: no `wcl` on PATH or CONFIG_WEAVE_WCL");
        return;
    };
    let docs = dir.path().join("site");
    let out = Command::new(bin())
        .args(["docs", root.to_str().unwrap(), docs.to_str().unwrap()])
        .env("CONFIG_WEAVE_WCL", &wcl)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let index = std::fs::read_to_string(docs.join("index.html")).unwrap();
    assert!(index.contains("My Playbook"), "index missing title");
    assert!(
        index.contains("href=\"play_baseline.html\""),
        "index missing play link"
    );
    assert!(
        index.contains("href=\"pkg_example.html\""),
        "index missing package link"
    );

    // Per-play page carries the step table and the DAG diagram (SVG).
    let play = std::fs::read_to_string(docs.join("play_baseline.html")).unwrap();
    assert!(play.contains("greeting"), "play page missing step");
    assert!(
        play.contains("href=\"res_example_file_present.html\""),
        "play page missing resource link"
    );
    assert!(play.contains("svg"), "play page missing DAG diagram");

    let pkg = std::fs::read_to_string(docs.join("pkg_example.html")).unwrap();
    assert!(
        pkg.contains("href=\"res_example_file_present.html\""),
        "package page missing resource link"
    );
    assert!(
        pkg.contains("href=\"test_example_greeting_converges.html\""),
        "package page missing test link"
    );

    // Per-resource page carries the parameter table from the schema.
    let res = std::fs::read_to_string(docs.join("res_example_file_present.html")).unwrap();
    assert!(
        res.contains("Parameters"),
        "resource page missing param table"
    );
    assert!(
        res.contains("Absolute path of the file"),
        "param description missing"
    );
    assert!(
        res.contains("required") || res.contains("yes"),
        "requiredness missing"
    );
}
