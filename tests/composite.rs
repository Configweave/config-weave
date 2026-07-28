//! Composites: a named, parameterised block of steps invoked like a
//! resource. Covers expansion and reporting, the two param spellings, the
//! playbook-local namespace, nesting, `requires` scoping, and the
//! diagnostics that keep a broken composite from loading.

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

/// The `probe.marker` resource: converges on `path` existing, and appends
/// its own name to `order` so a test can assert execution sequence.
const MARKER_SCRIPT: &str = r#"use value
use fs

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn check(params: Value) -> Result[CheckResult, string] {
    if fs::exists(p(params, "path")) {
        Ok(CheckResult::AlreadyConfigured)
    } else {
        Ok(CheckResult::NotConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    fs::write(p(params, "path"), p(params, "content"))?
    let order = p(params, "order")
    if order != "" {
        fs::append(order, p(params, "tag") + ";")?
    }
    Ok(ApplyResult::Success)
}
"#;

const MARKER_DECL: &str = r#"  resource "marker" {
    description = "Converges on a file existing"
    script = "resources/marker.ws"

    param "path" { description = "Marker file path" type = "string" required = true }
    param "content" { description = "File body" type = "string" default = "" }
    param "order" { description = "Sequence log to append to" type = "string" default = "" }
    param "tag" { description = "What to append to the sequence log" type = "string" default = "" }
  }
"#;

/// Write a playbook whose `probe` package holds the marker resource plus
/// whatever composites the test needs.
fn write_playbook(root: &Path, pkg_extra: &str, playbook_body: &str) {
    let pkg = root.join("pkgs/probe");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::write(pkg.join("resources/marker.ws"), MARKER_SCRIPT).unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        format!(
            "package \"probe\" {{\n  description = \"Probe resources\"\n\n{MARKER_DECL}\n\
             {pkg_extra}}}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            "playbook \"Composites\" {{\n  description = \"Composite behaviour probes\"\n  \
             version = \"0.1.0\"\n\n{playbook_body}}}\n"
        ),
    )
    .unwrap();
}

/// A composite that writes two files under a directory, exercising both
/// argument spellings: `args.dir` (always safe) and the bare `body`.
fn site_composite(dir: &Path) -> String {
    format!(
        r#"  composite "site" {{
    description = "Writes a pair of files for one site"
    arg "dir" {{ description = "Target directory" type = "string" required = true }}
    arg "body" {{ description = "File body" type = "string" default = "hi" }}

    step "conf" {{
      description = "Write the config file"
      resource = "marker"
      properties {{
        path = $"${{args.dir}}/conf"
        content = body
        order = "{order}"
        tag = "conf"
      }}
    }}
    step "index" {{
      description = "Write the index file"
      resource = "probe.marker"
      requires = ["conf"]
      properties {{
        path = $"${{args.dir}}/index"
        content = body
        order = "{order}"
        tag = "index"
      }}
    }}
  }}
"#,
        order = dir.join("order").display()
    )
}

#[test]
fn a_package_composite_expands_into_path_namespaced_steps() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let site = root.join("site");
    std::fs::create_dir_all(&site).unwrap();
    write_playbook(
        root,
        &site_composite(root),
        &format!(
            r#"  play "p" {{
    description = "one composite"
    step "web" {{
      description = "the web site"
      resource = "probe.site"
      properties {{ dir = "{}" body = "hello" }}
    }}
  }}
"#,
            site.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "p", "--json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2, "{stdout}");
    for (i, name) in ["conf", "index"].iter().enumerate() {
        assert_eq!(steps[i]["name"], *name);
        assert_eq!(steps[i]["container_path"][0], "web");
        assert_eq!(steps[i]["resource"], "probe.marker");
        assert_eq!(steps[i]["status"], "configured");
    }

    // Both spellings resolved: `args.dir` built the path, bare `body`
    // supplied the content.
    assert_eq!(std::fs::read_to_string(site.join("conf")).unwrap(), "hello");
    assert_eq!(std::fs::read_to_string(site.join("index")).unwrap(), "hello");
    // The inner `requires` ordered the siblings.
    let order = std::fs::read_to_string(root.join("order")).unwrap();
    assert_eq!(order, "conf;index;");
}

/// The pass-through that motivated binding params twice: a property field
/// shadows a bare outer variable of the same name, so `path = path` is a
/// self-reference and `path = args.path` is the spelling that works.
#[test]
fn the_args_map_survives_a_same_named_property() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let target = root.join("through");
    write_playbook(
        root,
        r#"  composite "pass" {
    description = "Passes a same-named property straight through"
    arg "path" { description = "Where to write" type = "string" required = true }

    step "write" {
      description = "Write it"
      resource = "marker"
      properties { path = args.path }
    }
  }
"#,
        &format!(
            r#"  play "p" {{
    description = "pass-through"
    step "t" {{
      description = "invoke"
      resource = "probe.pass"
      properties {{ path = "{}" }}
    }}
  }}
"#,
            target.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(target.exists(), "{stdout}");
}

#[test]
fn a_playbook_composite_is_referenced_by_bare_name() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let target = root.join("local");
    write_playbook(
        root,
        "",
        &format!(
            r#"  composite "local_write" {{
    description = "Playbook-local composite"
    arg "path" {{ description = "Where to write" type = "string" required = true }}

    step "write" {{
      description = "Write it"
      resource = "probe.marker"
      properties {{ path = args.path }}
    }}
  }}

  play "p" {{
    description = "local composite"
    step "t" {{
      description = "invoke"
      resource = "local_write"
      properties {{ path = "{}" }}
    }}
  }}
"#,
            target.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "p", "--json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["steps"][0]["container_path"][0], "t");
    assert!(target.exists());
}

/// Nesting: the outer invocation's arguments feed the inner one, and the
/// report path carries both invocation names.
#[test]
fn a_nested_composite_threads_arguments_down_the_chain() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let target = root.join("deep");
    write_playbook(
        root,
        r#"  composite "inner" {
    description = "The innermost block"
    arg "path" { description = "Where to write" type = "string" required = true }
    arg "body" { description = "File body" type = "string" default = "" }

    step "write" {
      description = "Write it"
      resource = "marker"
      properties { path = args.path content = args.body }
    }
  }

  composite "outer" {
    description = "Wraps the inner block"
    arg "dir" { description = "Target directory" type = "string" required = true }

    step "nested" {
      description = "Invoke the inner composite"
      resource = "inner"
      properties { path = $"${args.dir}/leaf" body = "deep" }
    }
  }
"#,
        &format!(
            r#"  play "p" {{
    description = "nesting"
    step "top" {{
      description = "invoke the outer composite"
      resource = "probe.outer"
      properties {{ dir = "{}" }}
    }}
  }}
"#,
            target.display()
        ),
    );
    std::fs::create_dir_all(&target).unwrap();

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "p", "--json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let step = &report["steps"][0];
    assert_eq!(step["name"], "write");
    assert_eq!(step["container_path"][0], "top");
    assert_eq!(step["container_path"][1], "nested");
    assert_eq!(
        std::fs::read_to_string(target.join("leaf")).unwrap(),
        "deep"
    );
}

/// Requiring a composite by name means "after the whole block": every step
/// it expanded into has to finish first.
#[test]
fn requiring_a_composite_waits_for_all_of_its_steps() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let site = root.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let order_path = root.join("order");
    write_playbook(
        root,
        &site_composite(root),
        &format!(
            r#"  play "p" {{
    description = "ordering"
    step "web" {{
      description = "the web site"
      resource = "probe.site"
      properties {{ dir = "{site}" }}
    }}
    step "after" {{
      description = "runs last"
      resource = "probe.marker"
      requires = ["web"]
      properties {{
        path = "{site}/after"
        order = "{order}"
        tag = "after"
      }}
    }}
  }}
"#,
            site = site.display(),
            order = order_path.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "p"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let order = std::fs::read_to_string(&order_path).unwrap();
    assert_eq!(order, "conf;index;after;", "{stdout}");
}

/// A composite's own `condition` gates every step it expands into, and it
/// is evaluated in the *invoking* scope — so it can read playbook vars the
/// body itself cannot.
#[test]
fn a_false_condition_on_the_invocation_skips_the_whole_block() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let site = root.join("site");
    std::fs::create_dir_all(&site).unwrap();
    write_playbook(
        root,
        &site_composite(root),
        &format!(
            r#"  vars {{
    enabled = false
  }}

  play "p" {{
    description = "gated"
    step "web" {{
      description = "the web site"
      resource = "probe.site"
      condition = enabled
      properties {{ dir = "{}" }}
    }}
  }}
"#,
            site.display()
        ),
    );

    let (code, stdout, stderr) = run_in(root, &["apply", ".", "p", "--json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    for s in steps {
        assert_eq!(s["status"], "skipped", "{stdout}");
    }
    assert!(!site.join("conf").exists());
}

// ------------------------------------------------------------ diagnostics

fn validate_fails_with(pkg_extra: &str, playbook_body: &str, needle: &str) {
    let d = tempfile::tempdir().unwrap();
    write_playbook(d.path(), pkg_extra, playbook_body);
    let (code, stdout, stderr) = run_in(d.path(), &["validate", "."]);
    assert_ne!(code, 0, "expected failure\n{stdout}{stderr}");
    assert!(
        stderr.contains(needle) || stdout.contains(needle),
        "expected {needle:?} in:\n{stdout}{stderr}"
    );
}

#[test]
fn an_unknown_composite_property_reads_like_a_resource_one() {
    validate_fails_with(
        r#"  composite "c" {
    description = "A composite"
    arg "a" { description = "An argument" type = "string" default = "" }
    step "s" {
      description = "step"
      resource = "marker"
      properties { path = "/tmp/x" }
    }
  }
"#,
        r#"  play "p" {
    description = "bad property"
    step "t" {
      description = "invoke"
      resource = "probe.c"
      properties { nope = "x" }
    }
  }
"#,
        "unknown parameter 'nope' for composite 'probe.c'",
    );
}

#[test]
fn a_missing_required_composite_property_is_reported() {
    validate_fails_with(
        r#"  composite "c" {
    description = "A composite"
    arg "a" { description = "An argument" type = "string" required = true }
    step "s" {
      description = "step"
      resource = "marker"
      properties { path = args.a }
    }
  }
"#,
        r#"  play "p" {
    description = "missing property"
    step "t" {
      description = "invoke"
      resource = "probe.c"
    }
  }
"#,
        "missing required parameter 'a' for composite 'probe.c'",
    );
}

#[test]
fn a_composite_cycle_is_rejected() {
    validate_fails_with(
        r#"  composite "a" {
    description = "Calls b"
    step "s" { description = "step" resource = "b" }
  }

  composite "b" {
    description = "Calls a"
    step "s" { description = "step" resource = "a" }
  }
"#,
        r#"  play "p" {
    description = "cyclic"
    step "t" { description = "invoke" resource = "probe.a" }
  }
"#,
        "composite cycle: probe.a -> probe.b -> probe.a",
    );
}

/// Inner steps are encapsulated: a playbook step cannot reach into a
/// composite and depend on one of its steps by name.
#[test]
fn an_inner_step_is_not_addressable_from_the_playbook() {
    validate_fails_with(
        r#"  composite "c" {
    description = "A composite"
    step "inner" {
      description = "step"
      resource = "marker"
      properties { path = "/tmp/x" }
    }
  }
"#,
        r#"  play "p" {
    description = "reaching in"
    step "t" { description = "invoke" resource = "probe.c" }
    step "after" {
      description = "depends on an inner step"
      resource = "probe.marker"
      requires = ["inner"]
      properties { path = "/tmp/y" }
    }
  }
"#,
        "step 'after' requires unknown step 'inner'",
    );
}

/// The mirror of the rule above: a composite step may only require a
/// sibling of the same composite.
#[test]
fn an_inner_step_cannot_require_a_playbook_step() {
    validate_fails_with(
        r#"  composite "c" {
    description = "A composite"
    step "inner" {
      description = "step"
      resource = "marker"
      requires = ["outside"]
      properties { path = "/tmp/x" }
    }
  }
"#,
        r#"  play "p" {
    description = "reaching out"
    step "outside" {
      description = "a playbook step"
      resource = "probe.marker"
      properties { path = "/tmp/y" }
    }
    step "t" { description = "invoke" resource = "probe.c" }
  }
"#,
        "may only require a sibling step of the same composite",
    );
}

#[test]
fn a_composite_argument_may_not_be_called_args() {
    validate_fails_with(
        r#"  composite "c" {
    description = "A composite"
    arg "args" { description = "Shadows the map" type = "string" default = "" }
    step "s" {
      description = "step"
      resource = "marker"
      properties { path = "/tmp/x" }
    }
  }
"#,
        r#"  play "p" {
    description = "bad param name"
    step "t" { description = "invoke" resource = "probe.c" }
  }
"#,
        "a composite argument cannot be called 'args'",
    );
}

/// `step` is one schema shared with `test`, so the fields that belong only
/// to the other block have to be turned away by the loader.
#[test]
fn a_composite_step_may_not_declare_expect() {
    validate_fails_with(
        r#"  composite "c" {
    description = "A composite"
    step "s" {
      description = "step"
      resource = "marker"
      expect = "error"
      properties { path = "/tmp/x" }
    }
  }
"#,
        r#"  play "p" {
    description = "expect in a composite"
    step "t" { description = "invoke" resource = "probe.c" }
  }
"#,
        "declares 'expect'",
    );
}

#[test]
fn a_resource_and_a_composite_may_not_share_a_name() {
    validate_fails_with(
        r#"  composite "marker" {
    description = "Collides with the resource"
    step "s" {
      description = "step"
      resource = "probe.marker"
      properties { path = "/tmp/x" }
    }
  }
"#,
        r#"  play "p" {
    description = "collision"
    step "t" { description = "invoke" resource = "probe.marker" properties { path = "/tmp/x" } }
  }
"#,
        "declared as both a resource and a composite",
    );
}
