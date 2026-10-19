//! Host-side preparation for one test run: locate a static config-weave
//! binary to copy into the instance, and synthesize a minimal playbook
//! directory (one play, the test's steps, the packages they reference)
//! that the copied binary executes in there.

use std::path::{Path, PathBuf};

use crate::diag::Diag;
use crate::model::{CoarseType, Package, Playbook, TestDecl};

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
        if let Some(p) = &self.cache.lock().expect("binary resolver cache poisoned")[slot] {
            return Ok(p.clone());
        }
        let explicit = match os {
            GuestOs::Linux => self.explicit_linux.as_deref(),
            GuestOs::Windows => self.explicit_windows.as_deref(),
        };
        let p = locate_binary(explicit, os)?;
        self.cache.lock().expect("binary resolver cache poisoned")[slot] = Some(p.clone());
        Ok(p)
    }
}

/// The environment `locate_binary` searches, gathered once so tests can
/// inject it instead of mutating the process environment.
pub struct SearchRoots {
    /// `$CONFIG_WEAVE_TEST_BINARY[_WINDOWS]` for the guest OS, if set.
    env: Option<String>,
    /// The running executable with symlinks resolved.
    exe: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, else `$HOME/.local/share` — where `just install`
    /// puts `config-weave/bin/`.
    data_home: Option<PathBuf>,
}

impl SearchRoots {
    pub fn from_env(os: GuestOs) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let exe = std::env::current_exe()
            .ok()
            .map(|e| std::fs::canonicalize(&e).unwrap_or(e));
        let data_home = var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|h| Path::new(&h).join(".local/share")));
        SearchRoots {
            env: var(env_var_for(os)),
            exe,
            data_home,
        }
    }
}

fn env_var_for(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Linux => "CONFIG_WEAVE_TEST_BINARY",
        GuestOs::Windows => "CONFIG_WEAVE_TEST_BINARY_WINDOWS",
    }
}

/// Find the binary to copy into instances of `os`. Resolution order:
/// explicit `--binary`/`--binary-windows` / the matching
/// `$CONFIG_WEAVE_TEST_BINARY[_WINDOWS]` (trusted — the in-instance
/// smoke test catches mistakes), the running executable when it is a
/// static ELF (linux only), the workspace's cross-build artifacts, the
/// platform binary beside the running executable, then the `just install`
/// data dir.
pub fn locate_binary(explicit: Option<&Path>, os: GuestOs) -> Result<PathBuf, Diag> {
    locate_binary_in(explicit, os, &SearchRoots::from_env(os))
}

fn locate_binary_in(
    explicit: Option<&Path>,
    os: GuestOs,
    roots: &SearchRoots,
) -> Result<PathBuf, Diag> {
    let flag = match os {
        GuestOs::Linux => "--binary",
        GuestOs::Windows => "--binary-windows",
    };
    let env_var = env_var_for(os);
    if let Some(p) = explicit {
        return if p.is_file() {
            Ok(p.to_path_buf())
        } else {
            Err(Diag::bare(format!("{flag} {} does not exist", p.display())))
        };
    }
    if let Some(env) = &roots.env {
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

    let (platform_file, valid): (&str, fn(&Path) -> bool) = match os {
        GuestOs::Linux => ("config-weave-linux-x86_64", is_static_elf),
        GuestOs::Windows => ("config-weave-windows-x86_64.exe", is_pe),
    };
    let mut searched: Vec<PathBuf> = Vec::new();

    let exe = roots.exe.as_ref();
    if os == GuestOs::Linux
        && let Some(exe) = exe
    {
        searched.push(exe.clone());
        if is_static_elf(exe) {
            return Ok(exe.clone());
        }
    }

    // Dev loop: a dynamically linked target/{debug,release} build is
    // running, but `just release` artifacts may exist in the workspace.
    if let Some(exe) = exe
        && let Some(ws) = exe.ancestors().nth(3)
    {
        let candidates = match os {
            GuestOs::Linux => vec![
                ws.join("target-cross/x86_64-unknown-linux-musl/release/config-weave"),
                ws.join("dist").join(platform_file),
            ],
            GuestOs::Windows => vec![
                ws.join("target-cross/x86_64-pc-windows-gnu/release/config-weave.exe"),
                ws.join("dist").join(platform_file),
            ],
        };
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for c in candidates {
            if c.is_file() && valid(&c) {
                let mtime = std::fs::metadata(&c)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, c.clone()));
                }
            }
            searched.push(c);
        }
        if let Some((_, p)) = best {
            return Ok(p);
        }
    }

    // An installed release: `just install` puts both platform binaries in
    // one folder and symlinks the linux one onto PATH, so the running exe
    // (symlink resolved) sits beside its windows sibling.
    let installed = [
        exe.and_then(|e| e.parent()).map(|d| d.join(platform_file)),
        roots
            .data_home
            .as_ref()
            .map(|d| d.join("config-weave/bin").join(platform_file)),
    ];
    for c in installed.into_iter().flatten() {
        if searched.contains(&c) {
            continue;
        }
        if c.is_file() && valid(&c) {
            return Ok(c);
        }
        searched.push(c);
    }

    let searched = searched
        .iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    let lead = match os {
        GuestOs::Linux => {
            "the running binary is dynamically linked and no static artifact was found"
        }
        GuestOs::Windows => "no windows test binary was found",
    };
    Err(Diag::bare(format!(
        "{lead}; build one with `just release` (or install one with `just install`) \
         and pass {flag} dist/{platform_file} (or set {env_var}); searched:\n{searched}"
    )))
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
        // The built-in package travels inside the config-weave binary the
        // instance already runs, so there is nothing to copy.
        if !p.is_builtin() {
            copy_dir(&p.dir, &dir.path().join("pkgs").join(name))?;
        }
    }
    copy_playbook_lib(pb, dir.path())?;

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
    props: &wscript_std::DynValue,
) -> Result<(SynthesizedTest, String), Diag> {
    let (package, resource) = key
        .split_once('.')
        .ok_or_else(|| Diag::bare(format!("resource key '{key}' must be 'package.resource'")))?;

    let dir = tempfile::tempdir()
        .map_err(|e| Diag::bare(format!("cannot create a synthesis tempdir: {e}")))?;
    let p = pb
        .packages
        .get(package)
        .ok_or_else(|| Diag::bare(format!("scenario references unknown package '{package}'")))?;
    if !p.is_builtin() {
        copy_dir(&p.dir, &dir.path().join("pkgs").join(package))?;
    }
    copy_playbook_lib(pb, dir.path())?;

    // A scenario prop is a wscript string either way, but a symbol param
    // only validates written as `:name` — so the schema decides. An
    // unknown resource is left to the in-guest validation to name.
    let params = p
        .resources
        .get(resource)
        .map(|r| r.params.as_slice())
        .or_else(|| p.composites.get(resource).map(|c| c.params.as_slice()))
        .unwrap_or_default();
    let symbols: Vec<&str> = params
        .iter()
        .filter(|d| d.ty == CoarseType::Symbol)
        .map(|d| d.name.as_str())
        .collect();

    let step = "step";
    let mut out = String::new();
    out.push_str(&format!(
        "playbook \"weave-scenario\" {{\n  description = {}\n  version = \"0.0.0\"\n\n",
        wcl_str(&format!("Synthesized for {key}"))
    ));
    out.push_str(&format!(
        "  play \"{PLAY}\" {{\n    description = \"scenario step\"\n"
    ));
    out.push_str(&format!(
        "\n    step \"{step}\" {{\n      description = \"apply {key}\"\n      resource = \"{key}\"\n"
    ));
    out.push_str("      ");
    out.push_str(&render_props(props, &symbols)?);
    out.push('\n');
    out.push_str("    }\n  }\n}\n");

    std::fs::write(dir.path().join("playbook.wcl"), out)
        .map_err(|e| Diag::bare(format!("cannot write the synthesized playbook: {e}")))?;
    Ok((SynthesizedTest { dir }, step.to_string()))
}

/// Synthesize a playbook carrying one package and a step-less play, for a
/// scenario's `gather` — `__gather <dir> <package>.<gatherer>` needs a
/// valid playbook dir containing the package.
pub fn synthesize_gather(pb: &Playbook, package: &str) -> Result<SynthesizedTest, Diag> {
    let dir = tempfile::tempdir()
        .map_err(|e| Diag::bare(format!("cannot create a synthesis tempdir: {e}")))?;
    let p = pb
        .packages
        .get(package)
        .ok_or_else(|| Diag::bare(format!("scenario references unknown package '{package}'")))?;
    if !p.is_builtin() {
        copy_dir(&p.dir, &dir.path().join("pkgs").join(package))?;
    }
    copy_playbook_lib(pb, dir.path())?;
    let out = format!(
        "playbook \"weave-scenario\" {{\n  description = {}\n  version = \"0.0.0\"\n\n  \
         play \"{PLAY}\" {{\n    description = \"gather only — no steps\"\n  }}\n}}\n",
        wcl_str(&format!("Synthesized to gather from {package}"))
    );
    std::fs::write(dir.path().join("playbook.wcl"), out)
        .map_err(|e| Diag::bare(format!("cannot write the synthesized playbook: {e}")))?;
    Ok(SynthesizedTest { dir })
}

/// Render a wscript map value as a WCL `properties { … }` block. Resource
/// params are scalars (string/int/float/bool); anything else is rendered
/// as a string for a best-effort pass. A top-level string for a key named
/// in `symbols` (the resource's symbol-typed params) renders as `:name`.
fn render_props(props: &wscript_std::DynValue, symbols: &[&str]) -> Result<String, Diag> {
    use wscript_std::DynValue;
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
        let rendered = match v {
            DynValue::String(sym) if symbols.contains(&k.as_str()) => {
                // Checked here because a bad one would otherwise surface as
                // a WCL parse error inside the guest, far from the cause.
                if !crate::engine::vars::is_identifier(sym) {
                    return Err(Diag::bare(format!(
                        "property '{k}' is a symbol param, and {sym:?} is not a valid symbol \
                         name (letters, digits and _, not starting with a digit)"
                    )));
                }
                format!(":{sym}")
            }
            _ => render_value(v),
        };
        out.push_str(&format!("        {k} = {rendered}\n"));
    }
    out.push_str("      }");
    Ok(out)
}

fn render_value(v: &wscript_std::DynValue) -> String {
    use wscript_std::DynValue;
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

/// Copy the playbook's own `lib/` alongside the packages. Package dirs
/// come across whole (so their `lib/` rides along), but a resource may
/// also `use` a playbook-level helper — without this the script compiles
/// on the host and then fails to resolve its import inside the instance.
fn copy_playbook_lib(pb: &Playbook, dest_root: &Path) -> Result<(), Diag> {
    let lib = pb.root.join("lib");
    if lib.is_dir() {
        copy_dir(&lib, &dest_root.join("lib"))?;
    }
    Ok(())
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
    script = "resources/touch.ws"
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
            pkg_dir.join("resources/touch.ws"),
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
        use wscript_std::DynValue;

        // Scalars render to WCL literals; strings are quote-escaped.
        assert_eq!(render_value(&DynValue::Int(42)), "42");
        assert_eq!(render_value(&DynValue::Bool(true)), "true");
        assert_eq!(render_value(&DynValue::String("a\"b".into())), "\"a\\\"b\"");

        // A single-key map renders a deterministic properties block.
        let mut m = HashMap::new();
        m.insert("path".to_string(), DynValue::String("/tmp/x".into()));
        let block = render_props(&DynValue::Map(m), &[]).unwrap();
        assert!(block.starts_with("properties {"), "{block}");
        assert!(block.contains("path = \"/tmp/x\""), "{block}");

        // Null props → an empty block.
        assert_eq!(render_props(&DynValue::Null, &[]).unwrap(), "properties {}");
    }

    #[test]
    fn a_symbol_param_renders_as_a_symbol_and_a_string_param_stays_quoted() {
        use std::collections::HashMap;
        use wscript_std::DynValue;

        let mut m = HashMap::new();
        m.insert("ensure".to_string(), DynValue::String("absent".into()));
        m.insert("path".to_string(), DynValue::String("absent".into()));
        let block = render_props(&DynValue::Map(m), &["ensure"]).unwrap();
        assert!(block.contains("ensure = :absent"), "{block}");
        assert!(block.contains("path = \"absent\""), "{block}");
    }

    #[test]
    fn a_symbol_param_that_is_not_an_identifier_is_refused() {
        use std::collections::HashMap;
        use wscript_std::DynValue;

        for bad in ["not-present", "1st", "", "a b"] {
            let mut m = HashMap::new();
            m.insert("ensure".to_string(), DynValue::String(bad.into()));
            let err = render_props(&DynValue::Map(m), &["ensure"]).unwrap_err();
            assert!(err.message.contains("'ensure'"), "{}", err.message);
            assert!(
                err.message.contains("not a valid symbol"),
                "{}",
                err.message
            );
        }
    }

    #[test]
    fn a_synthesized_scenario_step_with_a_symbol_param_validates() {
        use std::collections::HashMap;
        use wscript_std::DynValue;

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

  resource "file" {
    description = "A file"
    script = "resources/file.ws"
    param "path" {
      description = "Where"
      type = "string"
      required = true
    }
    param "ensure" {
      description = "Present or absent"
      type = "symbol"
      symbol "present" { description = "exists" }
      symbol "absent" { description = "does not exist" }
    }
  }
}
"#,
        )
        .unwrap();
        std::fs::write(
            pkg_dir.join("resources/file.ws"),
            r#"use value

fn check(params: Value) -> CheckResult {
    CheckResult::AlreadyConfigured
}

fn apply(params: Value) -> ApplyResult {
    ApplyResult::Success
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

        let mut m = HashMap::new();
        m.insert("path".to_string(), DynValue::String("/tmp/x".into()));
        m.insert("ensure".to_string(), DynValue::String("absent".into()));
        let (synth, _) = synthesize_resource(&pb, "tlab.file", &DynValue::Map(m)).unwrap();

        // Quoted, this failed validation with "write :absent, not \"absent\"".
        let again = crate::model::load(synth.dir.path());
        assert!(
            again.diags.is_empty(),
            "synthesized scenario playbook must validate: {:?}",
            again.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        let diags = crate::engine::validate(&again.playbook.unwrap());
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
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

    /// Minimal 64-bit LE ELF header + one program header.
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

    #[test]
    fn static_elf_detection_on_crafted_headers() {
        assert!(is_static_elf_bytes(&elf(1))); // PT_LOAD only
        assert!(!is_static_elf_bytes(&elf(3))); // PT_INTERP
        assert!(!is_static_elf_bytes(b"#!/bin/sh\n"));
    }

    /// A dev-build layout (`<ws>/target/debug/config-weave`, dynamic), an
    /// install dir with the running exe's siblings, and a data home.
    struct Layout {
        _dir: tempfile::TempDir,
        ws: PathBuf,
        dev_exe: PathBuf,
        install: PathBuf,
        data_home: PathBuf,
    }

    fn layout() -> Layout {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let dev_exe = ws.join("target/debug/config-weave");
        let install = dir.path().join("opt/cw/bin");
        let data_home = dir.path().join("data");
        for d in [dev_exe.parent().unwrap(), &install, &data_home] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&dev_exe, elf(3)).unwrap();
        Layout {
            _dir: dir,
            ws,
            dev_exe,
            install,
            data_home,
        }
    }

    fn put(path: &Path, bytes: &[u8]) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        path.to_path_buf()
    }

    fn roots(env: Option<&Path>, exe: &Path, data_home: &Path) -> SearchRoots {
        SearchRoots {
            env: env.map(|p| p.display().to_string()),
            exe: Some(exe.to_path_buf()),
            data_home: Some(data_home.to_path_buf()),
        }
    }

    #[test]
    fn windows_binary_found_in_the_data_dir() {
        let l = layout();
        let want = put(
            &l.data_home
                .join("config-weave/bin/config-weave-windows-x86_64.exe"),
            b"MZ",
        );
        let r = roots(None, &l.dev_exe, &l.data_home);
        assert_eq!(locate_binary_in(None, GuestOs::Windows, &r).unwrap(), want);
    }

    #[test]
    fn windows_binary_found_beside_the_running_exe_before_the_data_dir() {
        let l = layout();
        let exe = put(&l.install.join("config-weave-linux-x86_64"), &elf(3));
        let beside = put(&l.install.join("config-weave-windows-x86_64.exe"), b"MZ");
        put(
            &l.data_home
                .join("config-weave/bin/config-weave-windows-x86_64.exe"),
            b"MZ",
        );
        let r = roots(None, &exe, &l.data_home);
        assert_eq!(
            locate_binary_in(None, GuestOs::Windows, &r).unwrap(),
            beside
        );
    }

    #[test]
    fn installed_linux_binary_must_be_static() {
        let l = layout();
        let data = l
            .data_home
            .join("config-weave/bin/config-weave-linux-x86_64");
        put(&data, &elf(3));
        let r = roots(None, &l.dev_exe, &l.data_home);
        assert!(locate_binary_in(None, GuestOs::Linux, &r).is_err());
        put(&data, &elf(1));
        assert_eq!(locate_binary_in(None, GuestOs::Linux, &r).unwrap(), data);
    }

    #[test]
    fn workspace_artifacts_and_explicit_paths_keep_precedence() {
        let l = layout();
        put(
            &l.dev_exe.with_file_name("config-weave-windows-x86_64.exe"),
            b"MZ",
        );
        let data = put(
            &l.data_home
                .join("config-weave/bin/config-weave-windows-x86_64.exe"),
            b"MZ",
        );
        let dist = put(&l.ws.join("dist/config-weave-windows-x86_64.exe"), b"MZ");
        let r = roots(None, &l.dev_exe, &l.data_home);
        assert_eq!(locate_binary_in(None, GuestOs::Windows, &r).unwrap(), dist);

        let env_bin = put(&l.ws.join("env.exe"), b"MZ");
        let r = roots(Some(&env_bin), &l.dev_exe, &l.data_home);
        assert_eq!(
            locate_binary_in(None, GuestOs::Windows, &r).unwrap(),
            env_bin
        );
        assert_eq!(
            locate_binary_in(Some(&data), GuestOs::Windows, &r).unwrap(),
            data
        );
    }

    #[test]
    fn not_found_names_every_searched_path() {
        let l = layout();
        let r = roots(None, &l.dev_exe, &l.data_home);
        let err = locate_binary_in(None, GuestOs::Windows, &r).unwrap_err();
        for p in [
            l.ws.join("target-cross/x86_64-pc-windows-gnu/release/config-weave.exe"),
            l.ws.join("dist/config-weave-windows-x86_64.exe"),
            l.dev_exe.with_file_name("config-weave-windows-x86_64.exe"),
            l.data_home
                .join("config-weave/bin/config-weave-windows-x86_64.exe"),
        ] {
            assert!(
                err.message.contains(&p.display().to_string()),
                "{} missing from: {}",
                p.display(),
                err.message
            );
        }
        assert!(err.message.starts_with("no windows test binary was found"));

        let err = locate_binary_in(None, GuestOs::Linux, &r).unwrap_err();
        assert!(
            err.message.contains(&l.dev_exe.display().to_string()),
            "{}",
            err.message
        );
    }
}
