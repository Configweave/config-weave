//! Host-side preparation for one test run: locate a static config-weave
//! binary to copy into the instance, and synthesize a minimal playbook
//! directory (one play, the test's steps, the packages they reference)
//! that the copied binary executes in there.

use std::path::{Path, PathBuf};

use crate::diag::Diag;
use crate::model::{Package, Playbook, TestDecl};

use super::backend::GuestOs;

// ------------------------------------------------------------- binary

/// Lazily resolves the test binary to copy into instances, per guest OS,
/// caching the answer. Lazy because the guest OS is only known once an
/// instance is provisioned, and a run that never touches Windows guests
/// must not demand a Windows artifact.
pub struct BinaryResolver {
    explicit_linux: Option<PathBuf>,
    explicit_windows: Option<PathBuf>,
    /// [linux, windows] successes; failures re-resolve (cheap). A Mutex
    /// (not RefCell) so the resolver is `Sync` and shareable across the
    /// parallel group-runner threads.
    cache: std::sync::Mutex<[Option<PathBuf>; 2]>,
}

impl BinaryResolver {
    pub fn new(explicit_linux: Option<PathBuf>, explicit_windows: Option<PathBuf>) -> Self {
        BinaryResolver {
            explicit_linux,
            explicit_windows,
            cache: std::sync::Mutex::new([None, None]),
        }
    }

    pub fn resolve(&self, os: GuestOs) -> Result<PathBuf, Diag> {
        let slot = match os {
            GuestOs::Linux => 0,
            GuestOs::Windows => 1,
        };
        if let Some(p) = &self.cache.lock().unwrap()[slot] {
            return Ok(p.clone());
        }
        let explicit = match os {
            GuestOs::Linux => self.explicit_linux.as_deref(),
            GuestOs::Windows => self.explicit_windows.as_deref(),
        };
        let p = locate_binary(explicit, os)?;
        self.cache.lock().unwrap()[slot] = Some(p.clone());
        Ok(p)
    }
}

/// Find the binary to copy into instances of `os`. Resolution order:
/// explicit `--binary`/`--binary-windows` / the matching
/// `$CONFIG_WEAVE_TEST_BINARY[_WINDOWS]` (trusted — the in-instance
/// smoke test catches mistakes), the running executable when it is a
/// static ELF (linux only), then the workspace's cross-build artifacts.
pub fn locate_binary(explicit: Option<&Path>, os: GuestOs) -> Result<PathBuf, Diag> {
    let (flag, env_var) = match os {
        GuestOs::Linux => ("--binary", "CONFIG_WEAVE_TEST_BINARY"),
        GuestOs::Windows => ("--binary-windows", "CONFIG_WEAVE_TEST_BINARY_WINDOWS"),
    };
    if let Some(p) = explicit {
        return if p.is_file() {
            Ok(p.to_path_buf())
        } else {
            Err(Diag::bare(format!("{flag} {} does not exist", p.display())))
        };
    }
    if let Ok(env) = std::env::var(env_var)
        && !env.is_empty()
    {
        let p = PathBuf::from(env);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(Diag::bare(format!(
                "{env_var}={} does not exist",
                p.display()
            )))
        };
    }

    let exe = std::env::current_exe().ok();
    if os == GuestOs::Linux
        && let Some(exe) = &exe
        && is_static_elf(exe)
    {
        return Ok(exe.clone());
    }

    // Dev loop: a dynamically linked target/{debug,release} build is
    // running, but `just release` artifacts may exist in the workspace.
    if let Some(exe) = &exe
        && let Some(ws) = exe.ancestors().nth(3)
    {
        let (candidates, valid): (Vec<PathBuf>, fn(&Path) -> bool) = match os {
            GuestOs::Linux => (
                vec![
                    ws.join("target-cross/x86_64-unknown-linux-musl/release/config-weave"),
                    ws.join("dist/config-weave-linux-x86_64"),
                ],
                is_static_elf,
            ),
            GuestOs::Windows => (
                vec![
                    ws.join("target-cross/x86_64-pc-windows-gnu/release/config-weave.exe"),
                    ws.join("dist/config-weave-windows-x86_64.exe"),
                ],
                is_pe,
            ),
        };
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for c in candidates {
            if c.is_file() && valid(&c) {
                let mtime = std::fs::metadata(&c)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, c));
                }
            }
        }
        if let Some((_, p)) = best {
            return Ok(p);
        }
    }

    match os {
        GuestOs::Linux => Err(Diag::bare(
            "the running binary is dynamically linked and no static artifact was found; \
             build one with `just release` and pass --binary dist/config-weave-linux-x86_64 \
             (or set CONFIG_WEAVE_TEST_BINARY)",
        )),
        GuestOs::Windows => Err(Diag::bare(
            "no windows test binary was found; build one with `just release` and pass \
             --binary-windows dist/config-weave-windows-x86_64.exe \
             (or set CONFIG_WEAVE_TEST_BINARY_WINDOWS)",
        )),
    }
}

/// A 64-bit ELF with no `PT_INTERP` program header runs in any container
/// of the same architecture — staticness, not libc, is the criterion
/// (musl-static and static-PIE both qualify).
fn is_static_elf(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    is_static_elf_bytes(&data)
}

/// A PE (Windows) executable: just the `MZ` magic — the `just release`
/// windows-gnu artifact is self-contained, and the in-instance smoke
/// test catches anything subtler.
fn is_pe(path: &Path) -> bool {
    let mut magic = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic))
        .is_ok()
        && &magic == b"MZ"
}

fn is_static_elf_bytes(d: &[u8]) -> bool {
    const PT_INTERP: u32 = 3;
    if d.len() < 64 || &d[..4] != b"\x7fELF" || d[4] != 2 || d[5] != 1 {
        return false; // not a little-endian 64-bit ELF
    }
    let phoff = u64::from_le_bytes(d[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(d[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(d[56..58].try_into().unwrap()) as usize;
    if phnum == 0 || phentsize < 4 {
        return false;
    }
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let Some(bytes) = d.get(off..off + 4) else {
            return false;
        };
        if u32::from_le_bytes(bytes.try_into().unwrap()) == PT_INTERP {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------- synthesis

/// A synthesized playbook directory for one test; the tempdir cleans up
/// with it.
pub struct SynthesizedTest {
    pub dir: tempfile::TempDir,
}

/// The play name every synthesized playbook uses.
pub const PLAY: &str = "test";

/// Build the minimal playbook the instance executes: the test's steps as
/// one play (properties/conditions spliced verbatim from package.wcl)
/// plus a copy of every package they reference. No vars, no gathers —
/// test gathers run separately through `__gather`.
pub fn synthesize(pb: &Playbook, pkg: &Package, test: &TestDecl) -> Result<SynthesizedTest, Diag> {
    let dir = tempfile::tempdir()
        .map_err(|e| Diag::bare(format!("cannot create a synthesis tempdir: {e}")))?;

    let mut needed: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    needed.insert(&pkg.name);
    for s in &test.steps {
        needed.insert(&s.package);
    }
    for g in &test.gathers {
        needed.insert(&g.package);
    }
    for name in needed {
        let p = pb
            .packages
            .get(name)
            .ok_or_else(|| Diag::bare(format!("test references unknown package '{name}'")))?;
        copy_dir(&p.dir, &dir.path().join("pkgs").join(name))?;
    }

    let mut out = String::new();
    out.push_str(&format!(
        "playbook \"weave-test\" {{\n  description = {}\n  version = \"0.0.0\"\n\n",
        wcl_str(&format!("Synthesized for {}:{}", pkg.name, test.name))
    ));
    out.push_str(&format!(
        "  play \"{PLAY}\" {{\n    description = {}\n",
        wcl_str(&test.description)
    ));
    for s in &test.steps {
        out.push_str(&format!(
            "\n    step \"{}\" {{\n      description = {}\n      resource = \"{}.{}\"\n",
            s.name,
            wcl_str(&s.description),
            s.package,
            s.resource
        ));
        if let Some(cond) = &s.condition_src {
            out.push_str(&format!("      condition = {cond}\n"));
        }
        if !s.requires.is_empty() {
            let reqs: Vec<String> = s.requires.iter().map(|r| format!("\"{r}\"")).collect();
            out.push_str(&format!("      requires = [{}]\n", reqs.join(", ")));
        }
        if let Some(props) = &s.properties_src {
            out.push_str("      ");
            out.push_str(props);
            out.push('\n');
        }
        out.push_str("    }\n");
    }
    out.push_str("  }\n}\n");

    std::fs::write(dir.path().join("playbook.wcl"), out)
        .map_err(|e| Diag::bare(format!("cannot write the synthesized playbook: {e}")))?;
    Ok(SynthesizedTest { dir })
}

/// Synthesize a one-step playbook that applies a single resource `key`
/// (`package.resource`) with `props`, for a scenario's `apply_resource` /
/// `check_resource`. Returns the dir plus the synthetic step name (the
/// caller looks it up in the `--json` report).
pub fn synthesize_resource(
    pb: &Playbook,
    key: &str,
    props: &wisp_std::DynValue,
) -> Result<(SynthesizedTest, String), Diag> {
    let (package, _resource) = key
        .split_once('.')
        .ok_or_else(|| Diag::bare(format!("resource key '{key}' must be 'package.resource'")))?;

    let dir = tempfile::tempdir()
        .map_err(|e| Diag::bare(format!("cannot create a synthesis tempdir: {e}")))?;
    let p = pb
        .packages
        .get(package)
        .ok_or_else(|| Diag::bare(format!("scenario references unknown package '{package}'")))?;
    copy_dir(&p.dir, &dir.path().join("pkgs").join(package))?;

    let step = "step";
    let mut out = String::new();
    out.push_str(&format!(
        "playbook \"weave-scenario\" {{\n  description = {}\n  version = \"0.0.0\"\n\n",
        wcl_str(&format!("Synthesized for {key}"))
    ));
    out.push_str(&format!("  play \"{PLAY}\" {{\n    description = \"scenario step\"\n"));
    out.push_str(&format!(
        "\n    step \"{step}\" {{\n      description = \"apply {key}\"\n      resource = \"{key}\"\n"
    ));
    out.push_str("      ");
    out.push_str(&render_props(props)?);
    out.push('\n');
    out.push_str("    }\n  }\n}\n");

    std::fs::write(dir.path().join("playbook.wcl"), out)
        .map_err(|e| Diag::bare(format!("cannot write the synthesized playbook: {e}")))?;
    Ok((SynthesizedTest { dir }, step.to_string()))
}

/// Render a wisp map value as a WCL `properties { … }` block. Resource
/// params are scalars (string/int/float/bool); anything else is rendered
/// as a string for a best-effort pass.
fn render_props(props: &wisp_std::DynValue) -> Result<String, Diag> {
    use wisp_std::DynValue;
    let map = match props {
        DynValue::Map(m) => m,
        DynValue::Null => {
            return Ok("properties {}".to_string());
        }
        other => {
            return Err(Diag::bare(format!(
                "scenario resource properties must be a map, got {other:?}"
            )));
        }
    };
    let mut out = String::from("properties {\n");
    for (k, v) in map {
        out.push_str(&format!("        {k} = {}\n", render_value(v)));
    }
    out.push_str("      }");
    Ok(out)
}

fn render_value(v: &wisp_std::DynValue) -> String {
    use wisp_std::DynValue;
    match v {
        DynValue::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        DynValue::Int(i) => i.to_string(),
        DynValue::Float(f) => f.to_string(),
        DynValue::Bool(b) => b.to_string(),
        DynValue::List(items) => {
            let parts: Vec<String> = items.iter().map(render_value).collect();
            format!("[{}]", parts.join(", "))
        }
        other => format!("\"{other:?}\""),
    }
}

/// WCL string literal from arbitrary text. Quotes/backslashes are
/// replaced, not escaped — only descriptions pass through here, never
/// identity-bearing names (load validation keeps those quote-free).
fn wcl_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "/").replace('"', "'"))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), Diag> {
    let fail = |e: std::io::Error| {
        Diag::bare(format!(
            "cannot copy {} to {}: {e}",
            from.display(),
            to.display()
        ))
    };
    std::fs::create_dir_all(to).map_err(fail)?;
    for entry in std::fs::read_dir(from).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let dest = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest).map_err(fail)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The synthesized directory must itself pass the full validation
    /// pipeline — the invariant that makes in-container exit-2 runs a
    /// bug signal rather than an expected failure mode.
    #[test]
    fn synthesized_playbook_validates() {
        let src = tempfile::tempdir().unwrap();
        let pkg_dir = src.path().join("pkgs/tlab");
        std::fs::create_dir_all(pkg_dir.join("resources")).unwrap();
        std::fs::write(
            src.path().join("playbook.wcl"),
            "playbook \"fixture\" {\n  description = \"f\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            pkg_dir.join("package.wcl"),
            r#"package "tlab" {
  description = "fixture"

  resource "touch" {
    description = "Create a file"
    script = "resources/touch.wisp"
    param "path" {
      description = "Where"
      type = "string"
      required = true
    }
  }

  test "t" {
    description = "Touch converges, with \"quotes\" in the description"
    image = "debian:12"

    step "one" {
      description = "First"
      resource = "touch"
      properties {
        path = "/tmp/one"
      }
    }

    step "two" {
      description = "Second"
      resource = "touch"
      requires = ["one"]
      condition = 1 == 1
      properties {
        path = $"/tmp/${"two"}"
      }
    }
  }
}
"#,
        )
        .unwrap();
        std::fs::write(
            pkg_dir.join("resources/touch.wisp"),
            r#"use value
use fs

fn check(params: Value) -> CheckResult {
    if let Some(p) = params.get("path") {
        if fs::exists(p.as_string().unwrap_or("")) {
            return CheckResult::AlreadyConfigured
        }
    }
    CheckResult::NotConfigured
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    if let Some(p) = params.get("path") {
        fs::write(p.as_string().unwrap_or(""), "x")?
    }
    Ok(ApplyResult::Success)
}
"#,
        )
        .unwrap();

        let loaded = crate::model::load(src.path());
        assert!(
            loaded.diags.is_empty(),
            "fixture must validate: {:?}",
            loaded.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        let pb = loaded.playbook.unwrap();
        let pkg = &pb.packages["tlab"];
        let test = &pkg.tests[0];

        let synth = synthesize(&pb, pkg, test).unwrap();
        let again = crate::model::load(synth.dir.path());
        assert!(
            again.diags.is_empty(),
            "synthesized playbook must validate: {:?}",
            again.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        let spb = again.playbook.unwrap();
        let diags = crate::engine::validate(&spb);
        assert!(
            diags.is_empty(),
            "synthesized playbook must pass engine validation: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        // Shape: one play with both steps, splice preserved.
        let play = spb.play(PLAY).unwrap();
        assert_eq!(play.steps().len(), 2);
        let source = std::fs::read_to_string(synth.dir.path().join("playbook.wcl")).unwrap();
        assert!(source.contains("$\"/tmp/${\"two\"}\""), "{source}");
        assert!(source.contains("requires = [\"one\"]"), "{source}");
        assert!(source.contains("condition = 1 == 1"), "{source}");
    }

    #[test]
    fn renders_scenario_props_and_values() {
        use std::collections::HashMap;
        use wisp_std::DynValue;

        // Scalars render to WCL literals; strings are quote-escaped.
        assert_eq!(render_value(&DynValue::Int(42)), "42");
        assert_eq!(render_value(&DynValue::Bool(true)), "true");
        assert_eq!(render_value(&DynValue::String("a\"b".into())), "\"a\\\"b\"");

        // A single-key map renders a deterministic properties block.
        let mut m = HashMap::new();
        m.insert("path".to_string(), DynValue::String("/tmp/x".into()));
        let block = render_props(&DynValue::Map(m)).unwrap();
        assert!(block.starts_with("properties {"), "{block}");
        assert!(block.contains("path = \"/tmp/x\""), "{block}");

        // Null props → an empty block.
        assert_eq!(render_props(&DynValue::Null).unwrap(), "properties {}");
    }

    #[test]
    fn running_dev_binary_is_dynamic_or_static() {
        // Smoke for the ELF parser: the dev build must parse as an ELF
        // either way, and /bin/sh (glibc, dynamic) must not be static.
        let exe = std::env::current_exe().unwrap();
        let _ = is_static_elf(&exe); // must not panic
        if cfg!(target_os = "linux") && Path::new("/bin/sh").exists() {
            assert!(!is_static_elf(Path::new("/bin/sh")));
        }
    }

    #[test]
    fn pe_detection_on_magic_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("a.exe");
        std::fs::write(&exe, b"MZ\x90\x00rest-of-a-pe").unwrap();
        assert!(is_pe(&exe));
        let not = dir.path().join("not.exe");
        std::fs::write(&not, b"\x7fELF").unwrap();
        assert!(!is_pe(&not));
        assert!(!is_pe(&dir.path().join("missing.exe")));
    }

    #[test]
    fn explicit_windows_binary_is_trusted_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("config-weave.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        assert_eq!(
            locate_binary(Some(&exe), GuestOs::Windows).unwrap(),
            exe.clone()
        );
        let err = locate_binary(Some(&dir.path().join("nope.exe")), GuestOs::Windows).unwrap_err();
        assert!(err.message.contains("--binary-windows"), "{}", err.message);
    }

    #[test]
    fn binary_resolver_caches_per_os() {
        let dir = tempfile::tempdir().unwrap();
        let linux = dir.path().join("config-weave");
        let windows = dir.path().join("config-weave.exe");
        std::fs::write(&linux, b"\x7fELF").unwrap();
        std::fs::write(&windows, b"MZ").unwrap();
        let r = BinaryResolver::new(Some(linux.clone()), Some(windows.clone()));
        assert_eq!(r.resolve(GuestOs::Linux).unwrap(), linux);
        assert_eq!(r.resolve(GuestOs::Windows).unwrap(), windows);
        // Cached: still resolves after the files vanish.
        std::fs::remove_file(&linux).unwrap();
        assert_eq!(r.resolve(GuestOs::Linux).unwrap(), linux);
    }

    #[test]
    fn static_elf_detection_on_crafted_headers() {
        // Minimal 64-bit LE ELF header + one program header.
        fn elf(p_type: u32) -> Vec<u8> {
            let mut d = vec![0u8; 64 + 56];
            d[..4].copy_from_slice(b"\x7fELF");
            d[4] = 2; // 64-bit
            d[5] = 1; // little-endian
            d[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
            d[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
            d[56..58].copy_from_slice(&1u16.to_le_bytes()); // phnum
            d[64..68].copy_from_slice(&p_type.to_le_bytes());
            d
        }
        assert!(is_static_elf_bytes(&elf(1))); // PT_LOAD only
        assert!(!is_static_elf_bytes(&elf(3))); // PT_INTERP
        assert!(!is_static_elf_bytes(b"#!/bin/sh\n"));
    }
}
