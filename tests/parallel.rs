//! M5 gate: a playbook with declared exclusive/global resources runs
//! correctly under --jobs 8 with stable output. The probe resources
//! record overlap by writing lock-style marker files while running.

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

/// A package whose resources track concurrent execution: each apply
/// creates `<witness_dir>/running-<class>-<n>`, sleeps, samples how many
/// sibling "running" markers exist, records the count, then removes its
/// marker. Overlap shows up as a sampled count > 1.
fn write_playbook(root: &Path, witness: &Path) {
    let pkg = root.join("pkgs/par");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::create_dir_all(witness).unwrap();

    let resource = |name: &str, concurrency: &str| {
        format!(
            r#"
  resource "{name}" {{
    description = "Concurrency probe ({concurrency})"
    script = "resources/probe.ws"
    concurrency = "{concurrency}"

    param "id" {{
      description = "Step identity"
      type = "string"
      required = true
    }}
    param "witness_dir" {{
      description = "Where overlap is recorded"
      type = "string"
      required = true
    }}
  }}"#
        )
    };

    std::fs::write(
        pkg.join("package.wcl"),
        format!(
            r#"package "par" {{
  description = "Concurrency probes"
{}{}{}
}}
"#,
            resource("free", "parallel"),
            resource("locked", "exclusive"),
            resource("solo", "global"),
        ),
    )
    .unwrap();

    // The probe sleeps via shell to give real overlap a window.
    std::fs::write(
        pkg.join("resources/probe.ws"),
        r#"use value
use fs
use path
use shell

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn done_marker(params: Value) -> string {
    path::join(p(params, "witness_dir"), "done-" + p(params, "id"))
}

fn check(params: Value) -> CheckResult {
    if fs::exists(done_marker(params)) {
        CheckResult::AlreadyConfigured
    } else {
        CheckResult::NotConfigured
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let dir = p(params, "witness_dir")
    let id = p(params, "id")
    let mine = path::join(dir, "running-" + id)
    fs::write(mine, "x")?

    shell::run("sleep 0.3", Value::Null)?

    // Sample concurrent runners while still holding our marker.
    let entries = fs::list_dir(dir)?
    let running = 0
    for e in entries {
        if e.starts_with("running-") { running = running + 1 }
    }
    fs::write(path::join(dir, "sample-" + id), str(running))?

    fs::delete(mine)?
    fs::write(done_marker(params), "x")?
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();

    let step = |name: &str, resource: &str, extra: &str| {
        format!(
            r#"    step "{name}" {{
      description = "probe {name}"
      resource = "par.{resource}"
      {extra}
      properties {{
        id = "{name}"
        witness_dir = "{witness}"
      }}
    }}
"#,
            witness = witness.display()
        )
    };

    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Parallel" {{
  description = "Concurrency class behaviour"
  version = "0.1.0"

  play "mixed" {{
    description = "parallel, exclusive and global steps together"
{}{}{}{}{}{}
  }}
}}
"#,
            step("free-1", "free", ""),
            step("free-2", "free", ""),
            step("excl-1", "locked", ""),
            step("excl-2", "locked", ""),
            step("solo-1", "solo", ""),
            step("free-3", "free", ""),
        ),
    )
    .unwrap();
}

fn sample(witness: &Path, id: &str) -> i64 {
    std::fs::read_to_string(witness.join(format!("sample-{id}")))
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or(-1)
}

#[test]
fn concurrency_classes_enforced_under_jobs_8() {
    let dir = tempfile::tempdir().unwrap();
    let witness = dir.path().join("witness");
    write_playbook(dir.path(), &witness);

    let (code, stdout, stderr) = run_in(dir.path(), &["apply", ".", "mixed", "--jobs", "8"]);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");

    // Every step applied.
    for id in ["free-1", "free-2", "excl-1", "excl-2", "solo-1", "free-3"] {
        assert!(
            witness.join(format!("done-{id}")).exists(),
            "{id} did not complete"
        );
    }

    // The global step ran completely alone.
    assert_eq!(sample(&witness, "solo-1"), 1, "global step overlapped");

    // The two exclusive steps never overlapped each other: while one ran,
    // at most one 'running-excl-*' marker existed. Their samples count
    // all runners (parallel steps may overlap them), so check pairwise
    // exclusion via the lock markers they would both have held: if both
    // sampled >= 2 runners that included each other we cannot tell from
    // counts alone — instead assert their samples never *both* see the
    // other's marker by checking the stronger invariant below.
    // Stronger invariant: re-run with only exclusive steps.
    let dir2 = tempfile::tempdir().unwrap();
    let witness2 = dir2.path().join("witness");
    let pkg_play = |w: &Path| {
        format!(
            r#"playbook "Parallel" {{
  description = "Concurrency class behaviour"
  version = "0.1.0"

  play "excl" {{
    description = "only exclusive steps"
{}{}{}
  }}
}}
"#,
            exclusive_step("e1", w),
            exclusive_step("e2", w),
            exclusive_step("e3", w),
        )
    };
    write_playbook(dir2.path(), &witness2); // for pkgs/
    std::fs::write(dir2.path().join("playbook.wcl"), pkg_play(&witness2)).unwrap();

    let (code, stdout, _) = run_in(dir2.path(), &["apply", ".", "excl", "--jobs", "8"]);
    assert_eq!(code, 0, "{stdout}");
    for id in ["e1", "e2", "e3"] {
        assert_eq!(
            sample(&witness2, id),
            1,
            "exclusive steps overlapped ({id})"
        );
    }

    // Deterministic report: step lines appear in declaration order.
    let i1 = stdout.find("e1").unwrap();
    let i2 = stdout.find("e2").unwrap();
    let i3 = stdout.find("e3").unwrap();
    assert!(
        i1 < i2 && i2 < i3,
        "report not in declaration order: {stdout}"
    );
}

fn exclusive_step(name: &str, witness: &Path) -> String {
    format!(
        r#"    step "{name}" {{
      description = "probe {name}"
      resource = "par.locked"
      properties {{
        id = "{name}"
        witness_dir = "{witness}"
      }}
    }}
"#,
        witness = witness.display()
    )
}

/// Parallel steps genuinely overlap (sanity check that the pool works).
#[test]
fn parallel_steps_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let witness = dir.path().join("witness");
    write_playbook(dir.path(), &witness);
    // Only the free steps.
    std::fs::write(
        dir.path().join("playbook.wcl"),
        format!(
            r#"playbook "Parallel" {{
  description = "Concurrency class behaviour"
  version = "0.1.0"

  play "free" {{
    description = "only parallel steps"
{}{}{}
  }}
}}
"#,
            free_step("f1", &witness),
            free_step("f2", &witness),
            free_step("f3", &witness),
        ),
    )
    .unwrap();

    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "free", "--jobs", "8"]);
    assert_eq!(code, 0, "{stdout}");
    let max_seen = ["f1", "f2", "f3"]
        .iter()
        .map(|id| sample(&witness, id))
        .max()
        .unwrap();
    assert!(
        max_seen >= 2,
        "expected at least two parallel steps to overlap, max sample {max_seen}"
    );
}

fn free_step(name: &str, witness: &Path) -> String {
    format!(
        r#"    step "{name}" {{
      description = "probe {name}"
      resource = "par.free"
      properties {{
        id = "{name}"
        witness_dir = "{witness}"
      }}
    }}
"#,
        witness = witness.display()
    )
}

/// A step may tighten its resource's class: a parallel resource forced
/// global on one step runs alone.
#[test]
fn step_level_tightening() {
    let dir = tempfile::tempdir().unwrap();
    let witness = dir.path().join("witness");
    write_playbook(dir.path(), &witness);
    std::fs::write(
        dir.path().join("playbook.wcl"),
        format!(
            r#"playbook "Parallel" {{
  description = "Concurrency class behaviour"
  version = "0.1.0"

  play "tighten" {{
    description = "a parallel resource tightened to global"
{}{}    step "tightened" {{
      description = "forced global"
      resource = "par.free"
      concurrency = "global"
      properties {{
        id = "tightened"
        witness_dir = "{witness}"
      }}
    }}
  }}
}}
"#,
            free_step("f1", &witness),
            free_step("f2", &witness),
            witness = witness.display()
        ),
    )
    .unwrap();

    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "tighten", "--jobs", "8"]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        sample(&witness, "tightened"),
        1,
        "tightened step overlapped"
    );
}

/// play parallel = false forces sequential execution even with --jobs 8.
#[test]
fn play_level_sequential() {
    let dir = tempfile::tempdir().unwrap();
    let witness = dir.path().join("witness");
    write_playbook(dir.path(), &witness);
    std::fs::write(
        dir.path().join("playbook.wcl"),
        format!(
            r#"playbook "Parallel" {{
  description = "Concurrency class behaviour"
  version = "0.1.0"

  play "seq" {{
    description = "forced sequential"
    parallel = false
{}{}{}
  }}
}}
"#,
            free_step("s1", &witness),
            free_step("s2", &witness),
            free_step("s3", &witness),
        ),
    )
    .unwrap();

    let (code, stdout, _) = run_in(dir.path(), &["apply", ".", "seq", "--jobs", "8"]);
    assert_eq!(code, 0, "{stdout}");
    for id in ["s1", "s2", "s3"] {
        assert_eq!(sample(&witness, id), 1, "sequential play overlapped ({id})");
    }
}

const COMPOSITE_WRAP_B: &str = r#"
  composite "wrap_b" {
    description = "Invokes b from inside the package"
    arg "id" { description = "Step identity" type = "string" required = true }
    arg "group" { description = "Overlap group" type = "string" required = true }
    arg "sleep" { description = "Hold time" type = "string" required = true }
    arg "witness_dir" { description = "Witness dir" type = "string" required = true }

    step "inner" {
      description = "b, unqualified"
      resource = "b"
      properties {
        id = args.id
        group = args.group
        sleep = args.sleep
        witness_dir = args.witness_dir
      }
    }
  }"#;

/// Lock groups serialise across resource types within a package. `par.a`
/// and `par.b` are different resources sharing `lock = "db"`; `other.c`
/// declares the same lock name in another package, so it is a different
/// group; `par.free` has no lock. Each probe marks itself running under
/// its `group`, sleeps `sleep` seconds, then samples both its group's
/// runners and all runners. `lock-b` reaches `par.b` through a composite,
/// so its expanded inner step must carry `b`'s lock too.
#[test]
fn lock_group_serialises_across_resource_types() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let witness = root.join("witness");
    std::fs::create_dir_all(&witness).unwrap();

    let resource = |name: &str, extra: &str| {
        format!(
            r#"
  resource "{name}" {{
    description = "Lock probe"
    script = "resources/probe.ws"
    {extra}

    param "id" {{
      description = "Step identity"
      type = "string"
      required = true
    }}
    param "group" {{
      description = "Overlap group"
      type = "string"
      required = true
    }}
    param "sleep" {{
      description = "Seconds to hold the marker"
      type = "string"
      required = true
    }}
    param "witness_dir" {{
      description = "Where overlap is recorded"
      type = "string"
      required = true
    }}
  }}"#
        )
    };
    let probe = r#"use value
use fs
use path
use shell

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> CheckResult {
    if fs::exists(path::join(p(params, "witness_dir"), "done-" + p(params, "id"))) {
        CheckResult::AlreadyConfigured
    } else {
        CheckResult::NotConfigured
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let dir = p(params, "witness_dir")
    let id = p(params, "id")
    let prefix = "running-" + p(params, "group") + "-"
    let mine = path::join(dir, prefix + id)
    fs::write(mine, "x")?

    shell::run("sleep " + p(params, "sleep"), Value::Null)?

    let entries = fs::list_dir(dir)?
    let group = 0
    let total = 0
    for e in entries {
        if e.starts_with(prefix) { group = group + 1 }
        if e.starts_with("running-") { total = total + 1 }
    }
    fs::write(path::join(dir, "sample-" + id), str(group))?
    fs::write(path::join(dir, "total-" + id), str(total))?

    fs::delete(mine)?
    fs::write(path::join(dir, "done-" + id), "x")?
    Ok(ApplyResult::Success)
}
"#;
    for (pkg, body) in [
        (
            "par",
            format!(
                "{}{}{}{}",
                resource("a", r#"lock = "db""#),
                // A lock composes with a concurrency class.
                resource("b", "concurrency = \"exclusive\"\n    lock = \"db\""),
                resource("free", ""),
                COMPOSITE_WRAP_B,
            ),
        ),
        ("other", resource("c", r#"lock = "db""#)),
    ] {
        let d = root.join("pkgs").join(pkg);
        std::fs::create_dir_all(d.join("resources")).unwrap();
        std::fs::write(d.join("resources/probe.ws"), probe).unwrap();
        std::fs::write(
            d.join("package.wcl"),
            format!("package \"{pkg}\" {{\n  description = \"Lock probes\"\n{body}\n}}\n"),
        )
        .unwrap();
    }

    let step = |name: &str, resource: &str, group: &str, sleep: &str| {
        format!(
            r#"    step "{name}" {{
      description = "probe {name}"
      resource = "{resource}"
      properties {{
        id = "{name}"
        group = "{group}"
        sleep = "{sleep}"
        witness_dir = "{witness}"
      }}
    }}
"#,
            witness = witness.display()
        )
    };
    // `lock-b` is declared before the unlocked steps: while it waits on the
    // lock they must still be dispatched past it.
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Locks" {{
  description = "Lock group behaviour"
  version = "0.1.0"

  play "locks" {{
    description = "two resource types sharing a lock, plus bystanders"
{}{}{}{}
  }}
}}
"#,
            step("lock-a", "par.a", "par-db", "0.4"),
            step("lock-b", "par.wrap_b", "par-db", "0.4"),
            step("other-c", "other.c", "other-db", "0.15"),
            step("free-1", "par.free", "free", "0.15"),
        ),
    )
    .unwrap();

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "locks", "--jobs", "8"]);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    let read = |kind: &str, id: &str| -> i64 {
        std::fs::read_to_string(witness.join(format!("{kind}-{id}")))
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(-1)
    };

    for id in ["lock-a", "lock-b"] {
        assert_eq!(read("sample", id), 1, "{id} overlapped its lock group");
    }
    // Same lock name in another package, and an unlocked step: both ran
    // alongside the first holder of par's lock.
    assert!(
        read("total", "other-c") >= 2,
        "other.c did not run in parallel"
    );
    assert!(
        read("total", "free-1") >= 2,
        "free step did not run in parallel"
    );
}
