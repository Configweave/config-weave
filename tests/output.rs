//! M6 gate: JSON output is schema-stable and consumed by a test harness;
//! NDJSON file logging carries step context; stdout stays clean in JSON
//! mode.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

fn write_playbook(root: &Path, marker: &Path) {
    let pkg = root.join("pkgs/probe");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Output" {{
  description = "Output mode probes"
  version = "2.0.0"

  play "p" {{
    description = "one real step, one skipped"

    step "make" {{
      description = "creates the marker"
      resource = "probe.marker"
      properties {{
        path = "{marker}"
      }}
    }}

    container "grouped" {{
      description = "a container"

      step "never" {{
        description = "always skipped"
        resource = "probe.marker"
        condition = false
        properties {{
          path = "/nonexistent"
        }}
      }}
    }}
  }}
}}
"#,
            marker = marker.display()
        ),
    )
    .unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "probe" {
  description = "Output probe"

  resource "marker" {
    description = "Ensure a marker file exists"
    script = "resources/marker.ws"

    param "path" {
      description = "Marker path"
      type = "string"
      required = true
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
use log

fn p(params: Value) -> string {
    if let Some(v) = params.get("path") {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> CheckResult {
    if fs::exists(p(params)) { CheckResult::AlreadyConfigured } else { CheckResult::NotConfigured }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    log::info("writing marker")
    print("raw print output")
    fs::write(p(params), "x")?
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();
}

#[test]
fn json_output_is_schema_stable() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m");
    write_playbook(dir.path(), &marker);

    let out = Command::new(bin())
        .args(["apply", ".", "p", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");

    // Stdout is exactly one JSON object — nothing else.
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));

    assert_eq!(v["playbook"], "Output");
    assert_eq!(v["version"], "2.0.0");
    assert_eq!(v["play"], "p");
    assert_eq!(v["mode"], "apply");
    assert_eq!(v["exit_code"], 0);
    assert!(v["duration_secs"].is_number());

    let steps = v["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["name"], "make");
    assert_eq!(steps[0]["status"], "configured");
    assert_eq!(steps[0]["resource"], "probe.marker");
    assert_eq!(steps[1]["name"], "never");
    assert_eq!(steps[1]["status"], "skipped");
    assert_eq!(steps[1]["container_path"][0], "grouped");

    // Script log/print output went to stderr, never stdout.
    assert!(stderr.contains("writing marker"), "{stderr}");
    assert!(stderr.contains("raw print output"), "{stderr}");
}

#[test]
fn ndjson_log_file_carries_step_context() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m");
    write_playbook(dir.path(), &marker);
    let log_path = dir.path().join("run.ndjson");

    let out = Command::new(bin())
        .args([
            "apply",
            ".",
            "p",
            "--log-file",
            log_path.to_str().unwrap(),
            "--log-level",
            "debug",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));

    let log = std::fs::read_to_string(&log_path).expect("log file written");
    let mut saw_step_line = false;
    for line in log.lines() {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("log line is not JSON: {e}\n{line}"));
        if v["fields"]["message"] == "writing marker" {
            assert_eq!(v["fields"]["step"], "make");
            assert_eq!(v["fields"]["resource"], "probe.marker");
            saw_step_line = true;
        }
    }
    assert!(saw_step_line, "no step-context log line found:\n{log}");
}

#[test]
fn plain_mode_auto_selected_without_tty() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m");
    write_playbook(dir.path(), &marker);

    // Tests run without a TTY, so no --no-color flag is needed.
    let out = Command::new(bin())
        .args(["check", ".", "p"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains('\x1b'),
        "ANSI codes in non-TTY output: {stdout:?}"
    );
    assert!(stdout.contains("[     not configured]"), "{stdout}");
}

#[test]
fn uncreatable_log_file_exits_2_naming_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m");
    write_playbook(dir.path(), &marker);
    // A regular file cannot be the parent directory of the log file.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let log_path = blocker.join("x.log");

    let out = Command::new(bin())
        .args(["apply", ".", "p", "--log-file", log_path.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(stderr.contains(log_path.to_str().unwrap()), "{stderr}");
}

#[test]
fn log_level_is_checked_without_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("m");
    write_playbook(dir.path(), &marker);

    let out = Command::new(bin())
        .args(["validate", ".", "--log-level", "bogus"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("bogus"), "{stderr}");
}

/// A playbook with one package whose gatherer `chatty` logs at debug and
/// warn level.
fn write_gatherer_log_playbook(root: &Path) {
    let pkg = root.join("pkgs/probe");
    std::fs::create_dir_all(pkg.join("gatherers")).unwrap();
    std::fs::write(
        root.join("playbook.wcl"),
        r#"playbook "GatherLogs" {
  description = "Gatherer log levels"
  version = "0.1.0"

  gather "chatty" {
    description = "Probe"
    from = "probe.chatty"
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "probe" {
  description = "Gatherer log probe"

  gatherer "chatty" {
    description = "Logs at debug and warn level"
    script = "gatherers/chatty.ws"
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("gatherers/chatty.ws"),
        r#"use value
use log

fn gather(params: Value) -> Value {
    log::debug("debug detail")
    log::warn("warn detail")
    Value::Map(#{ "state": Value::String("done") })
}
"#,
    )
    .unwrap();
}

/// Run the gatherer once with the given file log level and return the
/// parsed NDJSON lines.
fn gather_with_log_level(root: &Path, level: &str) -> Vec<serde_json::Value> {
    let log_path = root.join(format!("{level}.ndjson"));
    let out = Command::new(bin())
        .args([
            "__gather",
            ".",
            "probe.chatty",
            "--log-file",
            log_path.to_str().unwrap(),
            "--log-level",
            level,
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    let log = std::fs::read_to_string(&log_path).expect("log file written");
    log.lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("log line is not JSON: {e}\n{line}"))
        })
        .collect()
}

fn find_line<'a>(lines: &'a [serde_json::Value], message: &str) -> Option<&'a serde_json::Value> {
    lines.iter().find(|v| v["fields"]["message"] == message)
}

#[test]
fn gatherer_log_lines_keep_their_level() {
    let dir = tempfile::tempdir().unwrap();
    write_gatherer_log_playbook(dir.path());

    let debug_lines = gather_with_log_level(dir.path(), "debug");
    let debug = find_line(&debug_lines, "debug detail")
        .unwrap_or_else(|| panic!("no debug line at --log-level debug: {debug_lines:?}"));
    assert_eq!(debug["level"], "DEBUG", "{debug}");
    assert!(
        debug["fields"]["gatherer"].as_str().unwrap_or("").contains("chatty"),
        "{debug}"
    );
    let warn = find_line(&debug_lines, "warn detail")
        .unwrap_or_else(|| panic!("no warn line: {debug_lines:?}"));
    assert_eq!(warn["level"], "WARN", "{warn}");

    let info_lines = gather_with_log_level(dir.path(), "info");
    assert!(
        find_line(&info_lines, "debug detail").is_none(),
        "debug line leaked at --log-level info: {info_lines:?}"
    );
    let warn = find_line(&info_lines, "warn detail")
        .unwrap_or_else(|| panic!("no warn line at info: {info_lines:?}"));
    assert_eq!(warn["level"], "WARN", "{warn}");
}
