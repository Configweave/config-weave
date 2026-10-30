//! Wait steps: `wait_for` polls a probe until it reports ready, or the
//! step's `timeout` passes.
//!
//! Linux only — `weave.command` defaults to bash.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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

/// A playbook whose body (probes, composites, plays) is `body`.
fn write_playbook(root: &Path, body: &str) {
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            "playbook \"Wait\" {{\n  description = \"Wait steps\"\n  version = \"0.1.0\"\n\n\
             {body}}}\n"
        ),
    )
    .unwrap();
}

/// A package `net` with a `file_exists` probe taking a `path` property.
fn write_net_package(root: &Path) {
    let dir = root.join("pkgs/net");
    std::fs::create_dir_all(dir.join("probes")).unwrap();
    std::fs::write(
        dir.join("package.wcl"),
        r#"package "net" {
  description = "Network readiness"

  probe "file_exists" {
    description = "Ready once a file exists"
    script = "probes/file_exists.ws"
    param "path" {
      description = "File to look for"
      type = "string"
      required = true
    }
  }

  resource "noop" {
    description = "Does nothing"
    script = "probes/noop.ws"
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("probes/file_exists.ws"),
        "use value\nuse fs\n\nfn ready(params: Value) -> bool {\n    \
         fs::exists(params.get(\"path\").unwrap().as_string().unwrap())\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("probes/noop.ws"),
        "use value\n\nfn check(params: Value) -> CheckResult { CheckResult::AlreadyConfigured }\n\
         fn apply(params: Value) -> ApplyResult { ApplyResult::Success }\n",
    )
    .unwrap();
}

/// Diagnostics with miette's line wrapping undone: long messages wrap
/// across `│`-prefixed lines.
fn flatten(text: &str) -> String {
    text.replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Create `path` after `delay`, from a background thread.
fn touch_later(path: PathBuf, delay: Duration) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        std::fs::write(path, "").unwrap();
    })
}

#[test]
fn a_package_probe_is_checked_once_and_polled_on_apply() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write_net_package(root);
    let flag = root.join("flag");
    write_playbook(
        root,
        &format!(
            r#"  play "p" {{
    description = "wait for the flag"
    step "flag" {{
      description = "wait for the flag file"
      wait_for = "net.file_exists"
      interval = 1s
      timeout = 30s
      properties {{ path = "{}" }}
    }}
  }}
"#,
            flag.display()
        ),
    );

    // check never waits: one probe, reported as not configured.
    let started = Instant::now();
    let (code, stdout, stderr) = run_in(root, &["--no-color", "check", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("not configured] flag (net.file_exists)")
            && stdout.contains("not ready; apply waits up to 30s"),
        "{stdout}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));

    // apply polls until the file appears.
    let toucher = touch_later(flag.clone(), Duration::from_millis(1500));
    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    toucher.join().unwrap();
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("[         configured] flag"), "{stdout}");
    assert!(stdout.contains("ready after"), "{stdout}");

    // Already ready: the first attempt passes.
    let (code, stdout, _) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("[ already configured] flag"), "{stdout}");
}

#[test]
fn a_timeout_fails_the_step_with_the_last_probe_error() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    std::fs::create_dir_all(root.join("probes")).unwrap();
    std::fs::write(
        root.join("probes/refused.ws"),
        "use value\n\nfn ready(params: Value) -> Result[bool, string] {\n    \
         Err(\"connection refused\")\n}\n",
    )
    .unwrap();
    write_playbook(
        root,
        r#"  probe "refused" {
    description = "Always fails"
    script = "probes/refused.ws"
  }
  play "p" {
    description = "never ready"
    step "db" {
      description = "wait for the database"
      wait_for = "refused"
      interval = 1s
      timeout = 2s
    }
    step "after" {
      description = "needs the database"
      resource = "weave.execute"
      requires = ["db"]
      properties {
        check = "false"
        run = "true"
      }
    }
  }
"#,
    );

    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stdout.contains("[              error] db (refused)")
            && stdout.contains("last probe failed: connection refused"),
        "{stdout}"
    );
    assert!(stdout.contains("[            not run] after"), "{stdout}");
}

#[test]
fn fail_on_error_makes_a_probe_error_fatal_at_once() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(
        root,
        r#"  play "p" {
    description = "bad probe"
    step "bad" {
      description = "cannot even start"
      wait_for = "weave.command"
      interval = 1s
      timeout = 1h
      fail_on_error = true
      properties {
        command = "true"
        cwd = "/definitely/not/a/directory"
      }
    }
  }
"#,
    );

    let started = Instant::now();
    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("probe failed:"), "{stdout}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// R1: a halt must not wait out an in-flight waiter's timeout.
#[test]
fn a_halt_cancels_waiting_steps() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write_playbook(
        root,
        r#"  play "p" {
    description = "one failure, one long wait"
    step "wait" {
      description = "would wait an hour"
      wait_for = "weave.command"
      interval = 1s
      timeout = 1h
      properties { command = "false" }
    }
    step "boom" {
      description = "fails after the wait has started"
      resource = "weave.execute"
      properties {
        check = "sleep 1; false"
        run = "exit 7"
      }
    }
  }
"#,
    );

    let started = Instant::now();
    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("[              error] boom"), "{stdout}");
    assert!(
        stdout.contains("[            not run] wait") && stdout.contains("the run halted"),
        "{stdout}"
    );
    assert!(started.elapsed() < Duration::from_secs(15));
}

/// R2: waiters run outside the worker pool, so with one worker the step
/// that makes them ready still runs.
#[test]
fn waiters_do_not_hold_pool_workers() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let flag = root.join("flag");
    let wait = |name: &str| {
        format!(
            r#"    step "{name}" {{
      description = "wait for the flag"
      wait_for = "weave.command"
      interval = 1s
      timeout = 20s
      properties {{ command = "test -f {f}" }}
    }}
"#,
            f = flag.display()
        )
    };
    write_playbook(
        root,
        &format!(
            r#"  play "p" {{
    description = "two waiters and the step they wait for"
{}{}    step "make" {{
      description = "creates the flag"
      resource = "weave.execute"
      properties {{
        check = "test -f {f}"
        run = "sleep 1; touch {f}"
      }}
    }}
  }}
"#,
            wait("a"),
            wait("b"),
            f = flag.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["--no-color", "--jobs", "1", "apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("[         configured] a")
            && stdout.contains("[         configured] b")
            && stdout.contains("[         configured] make"),
        "{stdout}"
    );
}

/// A sequential play runs one step at a time, waiters included: the step
/// after a waiter does not start while it waits.
#[test]
fn a_sequential_play_does_not_run_alongside_a_waiter() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let flag = root.join("flag");
    write_playbook(
        root,
        &format!(
            r#"  play "p" {{
    description = "sequential"
    parallel = false
    step "wait" {{
      description = "waits for a flag only the next step makes"
      wait_for = "weave.command"
      interval = 1s
      timeout = 2s
      properties {{ command = "test -f {f}" }}
    }}
    step "make" {{
      description = "creates the flag"
      resource = "weave.execute"
      properties {{
        check = "test -f {f}"
        run = "touch {f}"
      }}
    }}
  }}
"#,
            f = flag.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("[              error] wait"), "{stdout}");
    assert!(stdout.contains("[            not run] make"), "{stdout}");
    assert!(!flag.exists());
}

#[test]
fn a_composite_may_wait_with_a_timeout_from_its_arguments() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let flag = root.join("flag");
    std::fs::write(&flag, "").unwrap();
    write_playbook(
        root,
        &format!(
            r#"  composite "ready_then_mark" {{
    description = "wait, then do something"
    arg "path" {{
      description = "flag file"
      type = "string"
      required = true
    }}
    arg "patience" {{
      description = "how long to wait"
      type = "duration"
      default = 5s
    }}
    step "wait" {{
      description = "wait for the flag"
      wait_for = "weave.command"
      interval = 1s
      timeout = args.patience
      properties {{ command = $"test -f ${{args.path}}" }}
    }}
    step "mark" {{
      description = "after the wait"
      resource = "weave.execute"
      requires = ["wait"]
      properties {{
        check = "true"
        run = "true"
      }}
    }}
  }}
  play "p" {{
    description = "composite with a wait"
    step "svc" {{
      description = "invoke it"
      resource = "ready_then_mark"
      properties {{ path = "{}" }}
    }}
  }}
"#,
            flag.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("[ already configured] svc/wait (weave.command)"),
        "{stdout}"
    );
}

#[test]
fn list_json_reports_probes() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write_net_package(root);
    write_playbook(
        root,
        r#"  play "p" {
    description = "empty"
  }
"#,
    );
    let (code, stdout, stderr) = run_in(root, &["--json", "list", "."]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let net = v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "net")
        .unwrap();
    assert_eq!(net["probes"][0]["name"], "file_exists");
    assert_eq!(net["probes"][0]["params"][0]["name"], "path");
    let weave = v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "weave")
        .unwrap();
    assert_eq!(weave["probes"][0]["name"], "command");
}

/// Validation: each case is a play body (plus optional extra declarations)
/// and a fragment the diagnostics must contain.
#[test]
fn validation_rejects_malformed_wait_steps() {
    let cases: &[(&str, &str)] = &[
        (
            r#"    step "s" {
      description = "no timeout"
      wait_for = "weave.command"
      properties { command = "true" }
    }
"#,
            "waits with no 'timeout'",
        ),
        (
            r#"    step "s" {
      description = "too eager"
      wait_for = "weave.command"
      interval = 100ms
      timeout = 1min
      properties { command = "true" }
    }
"#,
            "'interval' must be at least 1s, got 100ms",
        ),
        (
            r#"    step "s" {
      description = "both"
      resource = "weave.execute"
      wait_for = "weave.command"
      timeout = 1min
    }
"#,
            "names both 'resource' and 'wait_for'",
        ),
        (
            r#"    step "s" {
      description = "neither"
    }
"#,
            "names neither 'resource' nor 'wait_for'",
        ),
        (
            r#"    step "s" {
      description = "wait field on a resource"
      resource = "weave.execute"
      timeout = 1min
      properties {
        check = "true"
        run = "true"
      }
    }
"#,
            "declares 'timeout', which only a wait step (wait_for) takes",
        ),
        (
            r#"    step "s" {
      description = "tightened"
      wait_for = "weave.command"
      timeout = 1min
      concurrency = "exclusive"
      properties { command = "true" }
    }
"#,
            "a wait step is always 'parallel'",
        ),
        (
            r#"    step "s" {
      description = "probe as resource"
      resource = "weave.command"
    }
"#,
            "names the probe 'weave.command' as a resource",
        ),
        (
            r#"    step "s" {
      description = "resource as probe"
      wait_for = "weave.execute"
      timeout = 1min
    }
"#,
            "package 'weave' has no probe 'execute'",
        ),
        (
            r#"    step "s" {
      description = "unknown property"
      wait_for = "weave.command"
      timeout = 1min
      properties {
        command = "true"
        bogus = 1
      }
    }
"#,
            "bogus",
        ),
        (
            r#"    step "s" {
      description = "missing required property"
      wait_for = "weave.command"
      timeout = 1min
    }
"#,
            "command",
        ),
        (
            r#"    step "s" {
      description = "unknown playbook probe"
      wait_for = "nope"
      timeout = 1min
    }
"#,
            "'nope', which is not a playbook probe",
        ),
    ];
    for (play, want) in cases {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write_playbook(
            root,
            &format!("  play \"p\" {{\n    description = \"case\"\n{play}  }}\n"),
        );
        let (code, stdout, stderr) = run_in(root, &["validate", "."]);
        assert_eq!(
            code, 2,
            "expected a validation failure for:\n{play}\n{stdout}{stderr}"
        );
        let flat = flatten(&format!("{stdout} {stderr}"));
        assert!(
            flat.contains(want),
            "wanted '{want}' for:\n{play}\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
        );
    }
}

#[test]
fn validation_checks_the_probe_contract_and_namespace() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write_net_package(root);
    // A probe whose script exports the wrong entry point, and a name taken
    // by both a probe and a resource.
    std::fs::write(
        root.join("pkgs/net/probes/file_exists.ws"),
        "use value\n\nfn check(params: Value) -> bool { true }\n",
    )
    .unwrap();
    let pkg = root.join("pkgs/net/package.wcl");
    let src = std::fs::read_to_string(&pkg).unwrap();
    std::fs::write(
        &pkg,
        src.replace(
            "  resource \"noop\" {",
            "  probe \"noop\" {\n    description = \"clash\"\n    script = \"probes/noop.ws\"\n  }\n\n  resource \"noop\" {",
        ),
    )
    .unwrap();
    write_playbook(
        root,
        r#"  play "p" {
    description = "empty"
  }
"#,
    );
    let (code, stdout, stderr) = run_in(root, &["validate", "."]);
    assert_eq!(code, 2, "{stdout}{stderr}");
    let flat = flatten(&stderr);
    assert!(
        flat.contains("does not satisfy the 'ready' contract"),
        "{stderr}"
    );
    assert!(
        flat.contains("'noop' is declared as both a probe and a resource"),
        "{stderr}"
    );
}

#[test]
fn a_test_step_cannot_wait() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write_net_package(root);
    let pkg = root.join("pkgs/net/package.wcl");
    let src = std::fs::read_to_string(&pkg).unwrap();
    let src = src.trim_end().trim_end_matches('}').to_string()
        + r#"
  test "t" {
    description = "waits"
    image = "debian:12"
    step "w" {
      description = "wait"
      wait_for = "file_exists"
      timeout = 1min
      properties { path = "/x" }
    }
  }
}
"#;
    std::fs::write(&pkg, src).unwrap();
    write_playbook(
        root,
        r#"  play "p" {
    description = "empty"
  }
"#,
    );
    let (code, stdout, stderr) = run_in(root, &["validate", "."]);
    assert_eq!(code, 2, "{stdout}{stderr}");
    assert!(
        flatten(&stderr).contains("declares a wait; a test step"),
        "{stderr}"
    );
}

/// A command the shell cannot find (exit 127) is a probe error, not "not
/// ready": `check` names it, and `fail_on_error` stops on it at once rather
/// than a typo waiting out the whole timeout.
#[test]
fn a_command_the_shell_cannot_find_is_a_probe_error() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let step = |fail_on_error: &str| {
        format!(
            r#"  play "p" {{
    description = "typo"
    step "db" {{
      description = "misspelt command"
      wait_for = "weave.command"
      interval = 1s
      timeout = 1h
{fail_on_error}      properties {{ command = "pg_isredy_no_such_command" }}
    }}
  }}
"#
        )
    };

    write_playbook(root, &step(""));
    let (code, stdout, stderr) = run_in(root, &["--no-color", "check", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("probe failed: the probe command exited 127")
            && stdout.contains("command not found"),
        "{stdout}"
    );

    write_playbook(root, &step("      fail_on_error = true\n"));
    let started = Instant::now();
    let (code, stdout, stderr) = run_in(root, &["--no-color", "apply", ".", "p"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("exited 127"), "{stdout}");
    assert!(started.elapsed() < Duration::from_secs(10));
}
