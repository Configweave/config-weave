//! Stage 5 of the validation pipeline (PRD §8): compile **every** wscript
//! script in the playbook against the full host context before anything
//! runs, and enforce the entry-point contracts:
//!
//! ```text
//! resources:  fn check(params: Value) -> CheckResult   (or Result[CheckResult, string])
//!             fn apply(params: Value) -> ApplyResult   (or Result[ApplyResult, string])
//! gatherers:  fn gather(params: Value) -> Value        (or Result[Value, string])
//! verifies:   fn verify(facts: Value) -> bool          (or Result[bool, string])
//! ```
//!
//! Scripts may `use` shared helpers from `lib/` (see [`WeaveResolver`]);
//! the whole import graph compiles to one unit, of which only the entry
//! file's functions are exported — so the contracts above are unaffected.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wscript::{CompiledUnit, Context, ImportSpec, ResolvedSource, SourceResolver, UnitExt};
use wscript_std::DynValue;

use crate::diag::Diag;
use crate::hostapi::{ApplyResult, CheckResult};
use crate::model::Playbook;

/// Resolves `use` imports for a script: the importing file's own
/// directory first, then each root — the declaring package's `lib/`, then
/// the playbook's `lib/` (PRD §6: package `lib/` is visible to that
/// package, playbook `lib/` to all of them). A registered host module
/// always wins over a file, so `use fs` still means the host API.
///
/// wscript ships an `FsResolver` that does exactly this, but it looks for
/// `{name}.wscript`; this repo standardised on the `.ws` extension, so
/// resolution is re-implemented over that. Path imports
/// (`use "./helpers.ws"`) carry their own extension and resolve relative
/// to the importing file.
pub struct WeaveResolver {
    roots: Vec<PathBuf>,
}

impl WeaveResolver {
    pub fn new(roots: Vec<PathBuf>) -> WeaveResolver {
        WeaveResolver { roots }
    }

    /// Rediscover a script's roots from its path, for the two places that
    /// compile a script without the playbook model in hand: the `__verify`
    /// subcommand (which runs inside a disposable instance, given only a
    /// script path). Every ancestor directory holding a `lib/` becomes a
    /// root, nearest first — which in the standard layout is the package's
    /// `lib/` then the playbook's, the same order [`compile_all`] uses.
    pub fn for_script(script: &Path) -> WeaveResolver {
        let mut roots = Vec::new();
        let mut dir = script.parent();
        while let Some(d) = dir {
            let lib = d.join("lib");
            if lib.is_dir() && !roots.contains(&lib) {
                roots.push(lib);
            }
            dir = d.parent();
        }
        WeaveResolver { roots }
    }
}

impl SourceResolver for WeaveResolver {
    fn resolve(&self, from: &str, spec: ImportSpec) -> Result<ResolvedSource, String> {
        let from_dir = Path::new(from)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let candidates: Vec<PathBuf> = match &spec {
            ImportSpec::Path(p) => vec![from_dir.join(p)],
            ImportSpec::Name(n) => {
                let file = format!("{n}.ws");
                std::iter::once(from_dir)
                    .chain(self.roots.iter().cloned())
                    .map(|d| d.join(&file))
                    .collect()
            }
        };
        for cand in &candidates {
            if !cand.is_file() {
                continue;
            }
            let src =
                std::fs::read_to_string(cand).map_err(|e| format!("{}: {e}", cand.display()))?;
            // Canonical path is the dedup key for the import graph; the
            // display path is what diagnostics name.
            let key = cand
                .canonicalize()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| cand.to_string_lossy().into_owned());
            return Ok(ResolvedSource {
                key,
                path: cand.to_string_lossy().into_owned(),
                src,
            });
        }
        Err(format!(
            "no such script (looked for {})",
            candidates
                .iter()
                .map(|c| c.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Whether a script entry point returns the result enum directly or
/// wrapped in `Result[…, string]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Plain,
    Fallible,
}

/// A compiled resource script with its verified entry points.
pub struct CompiledResource {
    pub unit: CompiledUnit,
    pub check: EntryKind,
    pub apply: EntryKind,
}

/// A compiled gatherer script.
pub struct CompiledGatherer {
    pub unit: CompiledUnit,
    pub gather: EntryKind,
}

/// Every compiled script in the playbook, keyed by `package.name`.
/// Test verify scripts compile (stage 5 catches broken ones) but are
/// not retained — they only ever run inside instances via `__verify`.
pub struct ScriptSet {
    pub resources: HashMap<String, CompiledResource>,
    pub gatherers: HashMap<String, CompiledGatherer>,
}

/// Compile all scripts; either every script compiles and satisfies its
/// contract, or the full diagnostic list comes back.
pub fn compile_all(pb: &Playbook, ctx: &Context) -> Result<ScriptSet, Vec<Diag>> {
    let mut diags = Vec::new();
    let mut resources = HashMap::new();
    let mut gatherers = HashMap::new();
    // Scenario scripts compile against an augmented context (the `testlab`
    // module on top of the host API); built lazily, only when present.
    let mut scenario_ctx: Option<Context> = None;

    for pkg in pb.packages.values() {
        // Every script of this package resolves imports against its own
        // `lib/` first, then the playbook's.
        let resolver = WeaveResolver::new(vec![pkg.dir.join("lib"), pb.root.join("lib")]);

        for res in pkg.resources.values() {
            if let Some((unit, source)) = compile_one(ctx, &resolver, &res.script, &mut diags) {
                let check =
                    entry_kind::<CheckResult>(&unit, "check", &res.script, &source, &mut diags);
                let apply =
                    entry_kind::<ApplyResult>(&unit, "apply", &res.script, &source, &mut diags);
                if let (Some(check), Some(apply)) = (check, apply) {
                    resources.insert(
                        format!("{}.{}", pkg.name, res.name),
                        CompiledResource { unit, check, apply },
                    );
                }
            }
        }
        for g in pkg.gatherers.values() {
            if let Some((unit, source)) = compile_one(ctx, &resolver, &g.script, &mut diags) {
                let gather =
                    entry_kind::<DynValue>(&unit, "gather", &g.script, &source, &mut diags);
                if let Some(gather) = gather {
                    gatherers.insert(
                        format!("{}.{}", pkg.name, g.name),
                        CompiledGatherer { unit, gather },
                    );
                }
            }
        }
        for t in &pkg.tests {
            let Some(script) = &t.verify else {
                continue;
            };
            if let Some((unit, source)) = compile_one(ctx, &resolver, script, &mut diags) {
                entry_kind::<bool>(&unit, "verify", script, &source, &mut diags);
            }
        }
        for s in &pkg.scenarios {
            let sctx = scenario_ctx.get_or_insert_with(crate::hostapi::scenario_context);
            if let Some((unit, source)) = compile_one(sctx, &resolver, &s.script, &mut diags) {
                check_run_contract(&unit, &s.script, &source, &mut diags);
            }
        }
        compile_lib(ctx, &resolver, &pkg.dir.join("lib"), &mut diags);
    }
    // Playbook-level helpers see only each other.
    let root_resolver = WeaveResolver::new(vec![pb.root.join("lib")]);
    compile_lib(ctx, &root_resolver, &pb.root.join("lib"), &mut diags);

    if diags.is_empty() {
        Ok(ScriptSet {
            resources,
            gatherers,
        })
    } else {
        Err(diags)
    }
}

/// Compile one script and everything it imports. The whole graph becomes
/// a single `CompiledUnit` of which only the entry file exports functions,
/// so the entry-point contracts are checked against this script alone.
/// The returned source is the entry file's, for the contract diagnostics
/// that point at it.
fn compile_one(
    ctx: &Context,
    resolver: &WeaveResolver,
    path: &Path,
    diags: &mut Vec<Diag>,
) -> Option<(CompiledUnit, String)> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            diags.push(Diag::bare(format!("cannot read {}: {e}", path.display())));
            return None;
        }
    };
    match ctx.compile_entry(&path.display().to_string(), &source, resolver) {
        Ok(compiled) => Some((compiled.unit, source)),
        Err(failure) => {
            // Spans address the whole import graph, so the source map
            // decides which file each diagnostic is rendered against.
            diags.extend(Diag::from_wscript(
                &failure.diags,
                &failure.sources,
                &failure.source_map,
            ));
            None
        }
    }
}

/// Shared wscript code under `lib/` (`*.ws`) must compile too, and may
/// import its siblings — `compile_one` passes the same resolver, so a
/// helper that `use`s another helper validates here rather than failing
/// later inside the resource that imports it.
fn compile_lib(ctx: &Context, resolver: &WeaveResolver, dir: &Path, diags: &mut Vec<Diag>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ws"))
        .collect();
    paths.sort();
    for path in paths {
        compile_one(ctx, resolver, &path, diags);
    }
}

/// Verify a scenario script exports `run(lab: Lab) -> bool` (or the
/// `Result[bool, string]` variant).
fn check_run_contract(unit: &CompiledUnit, path: &Path, source: &str, diags: &mut Vec<Diag>) {
    use crate::hostapi::testlab::Lab;
    if unit.fn_handle::<(Lab,), bool>("run").is_ok() {
        return;
    }
    if let Err(e) = unit.fn_handle::<(Lab,), Result<bool, String>>("run") {
        diags.push(Diag::spanned(
            format!("scenario script does not satisfy the 'run(lab: Lab) -> bool' contract: {e}"),
            "this script",
            path,
            source,
            (0, 0),
        ));
    }
}

/// Verify `name` is exported with one of the two accepted signatures.
fn entry_kind<R>(
    unit: &CompiledUnit,
    name: &str,
    path: &Path,
    source: &str,
    diags: &mut Vec<Diag>,
) -> Option<EntryKind>
where
    R: wscript::FromValue + wscript::ScriptType + 'static,
{
    if unit.fn_handle::<(DynValue,), R>(name).is_ok() {
        return Some(EntryKind::Plain);
    }
    match unit.fn_handle::<(DynValue,), Result<R, String>>(name) {
        Ok(_) => Some(EntryKind::Fallible),
        Err(e) => {
            diags.push(Diag::spanned(
                format!("script does not satisfy the '{name}' contract: {e}"),
                "this script",
                path,
                source,
                (0, 0),
            ));
            None
        }
    }
}
