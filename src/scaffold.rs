//! Authoring support (PRD §13): `wispi` dumps the host API as `.wispi`
//! interface files plus a starter `wisp.toml`; `init` scaffolds a working
//! skeleton playbook so the new-playbook path is init → edit → validate →
//! check.

use std::path::Path;

use crate::diag::Diag;

/// `config-weave wispi [outdir]`: emit the complete host API interface
/// (all modules, both platforms, CheckResult/ApplyResult, ComObject,
/// CmdOutput, …) and a starter wisp.toml referencing it.
pub fn wispi(outdir: &Path) -> Result<(), Diag> {
    std::fs::create_dir_all(outdir)
        .map_err(|e| Diag::bare(format!("cannot create {}: {e}", outdir.display())))?;
    let ctx = crate::hostapi::context();
    let wispi_path = outdir.join("weave.wispi");
    ctx.write_interface(&wispi_path)
        .map_err(|e| Diag::bare(format!("cannot write {}: {e}", wispi_path.display())))?;

    // Don't clobber an existing manifest; authors may have customised it.
    let toml_path = outdir.join("wisp.toml");
    if !toml_path.exists() {
        std::fs::write(&toml_path, "interfaces = [\"weave.wispi\"]\n")
            .map_err(|e| Diag::bare(format!("cannot write {}: {e}", toml_path.display())))?;
    }
    Ok(())
}

/// `config-weave init <dir>`: scaffold a skeleton playbook.
pub fn init(dir: &Path) -> Result<(), Diag> {
    if dir.join("playbook.wcl").exists() {
        return Err(Diag::bare(format!(
            "{} already contains a playbook.wcl",
            dir.display()
        )));
    }
    let write = |rel: &str, content: &str| -> Result<(), Diag> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Diag::bare(format!("cannot create {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, content)
            .map_err(|e| Diag::bare(format!("cannot write {}: {e}", path.display())))
    };

    write("playbook.wcl", PLAYBOOK)?;
    write("pkgs/example/package.wcl", PACKAGE)?;
    write("pkgs/example/resources/file_present.wisp", RESOURCE)?;
    write("pkgs/example/gatherers/os_info.wisp", GATHERER)?;
    write("pkgs/example/tests/greeting_verify.wisp", VERIFY)?;
    write("lib/README.md", LIB_README)?;
    write("pkgs/example/lib/README.md", LIB_README)?;

    // Authoring support: the LSP and `wisp check` pick these up.
    wispi(dir)?;
    Ok(())
}

const PLAYBOOK: &str = r#"playbook "My Playbook" {
  description = "Describe what this playbook converges"
  version = "0.1.0"

  gather "os" {
    description = "Operating system facts"
    from = "example.os_info"
  }

  vars {
    work_root = "/tmp/my-playbook"
    greeting_file = $"${work_root}/hello.txt"
  }

  play "baseline" {
    description = "A starter play with one step"

    step "greeting" {
      description = "Ensure the greeting file exists"
      resource = "example.file_present"
      condition = os.family != "plan9"
      properties {
        path = greeting_file
        content = "hello from config-weave"
      }
    }
  }
}
"#;

const PACKAGE: &str = r#"package "example" {
  description = "Example package scaffolded by config-weave init"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.wisp"
  }

  resource "file_present" {
    description = "Ensure a file exists with the given content"
    script = "resources/file_present.wisp"
    concurrency = "parallel"

    param "path" {
      description = "Absolute path of the file"
      type = "string"
      required = true
    }
    param "content" {
      description = "Desired file content"
      type = "string"
      default = ""
    }
  }

  // Run with `config-weave test <playbook-dir>` (needs docker or
  // podman). Steps default to expect = "converge": check reports
  // not_configured, apply succeeds, and a second apply proves
  // idempotence. Other expectations: already_configured, error, skip,
  // reboot_required. The optional verify script runs inside the
  // container for custom assertions.
  test "greeting_converges" {
    description = "file_present creates the greeting file and is idempotent"
    image = "debian:12"
    verify = "tests/greeting_verify.wisp"

    step "greet" {
      description = "Create the greeting file"
      resource = "file_present"
      properties {
        path = "/tmp/my-playbook/hello.txt"
        content = "hello from config-weave"
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
    if fs::read(p)? == param_str(params, "content", "") {
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

// Custom test assertions, run inside the test container after the apply
// runs. `facts` holds the results of the test's gather checks (empty
// here). Returning Ok(false) or Err fails the test with the message.
fn verify(facts: Value) -> Result[bool, string] {
    Ok(fs::read("/tmp/my-playbook/hello.txt")? == "hello from config-weave")
}
"#;

const LIB_README: &str = "Shared wisp helpers live here. They are compiled during validation;\n\
script-to-script imports arrive with wisp's module system (v2 roadmap).\n";
