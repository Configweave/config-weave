//! Stage 5 of the validation pipeline (PRD §8): compile **every** wisp
//! script in the playbook against the full host context before anything
//! runs, and enforce the entry-point contracts:
//!
//! ```text
//! resources:  fn check(params: Value) -> CheckResult   (or Result[CheckResult, string])
//!             fn apply(params: Value) -> ApplyResult   (or Result[ApplyResult, string])
//! gatherers:  fn gather(params: Value) -> Value        (or Result[Value, string])
//! verifies:   fn verify(facts: Value) -> bool          (or Result[bool, string])
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wisp::{CompiledUnit, Context, UnitExt};
use wisp_std::DynValue;

use crate::diag::Diag;
use crate::hostapi::{ApplyResult, CheckResult};
use crate::model::Playbook;

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

    for pkg in pb.packages.values() {
        for res in pkg.resources.values() {
            if let Some((unit, source)) = compile_one(ctx, &res.script, &mut diags) {
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
            if let Some((unit, source)) = compile_one(ctx, &g.script, &mut diags) {
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
            if let Some((unit, source)) = compile_one(ctx, script, &mut diags) {
                entry_kind::<bool>(&unit, "verify", script, &source, &mut diags);
            }
        }
        compile_lib(ctx, &pkg.dir.join("lib"), &mut diags);
    }
    compile_lib(ctx, &pb.root.join("lib"), &mut diags);

    if diags.is_empty() {
        Ok(ScriptSet {
            resources,
            gatherers,
        })
    } else {
        Err(diags)
    }
}

fn compile_one(
    ctx: &Context,
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
    match ctx.compile(&source) {
        Ok(unit) => Some((unit, source)),
        Err(wisp::Error::Compile(ds)) => {
            diags.extend(Diag::from_wisp(&ds, path, &source));
            None
        }
        Err(e) => {
            diags.push(Diag::bare(format!("{}: {e}", path.display())));
            None
        }
    }
}

/// Shared wisp code under `lib/` must compile too. wisp v1 has no
/// script-to-script imports, so lib files are compiled standalone; once
/// wisp ships imports, these folders become resolution roots.
fn compile_lib(ctx: &Context, dir: &Path, diags: &mut Vec<Diag>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wisp"))
        .collect();
    paths.sort();
    for path in paths {
        compile_one(ctx, &path, diags);
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
    R: wisp::FromValue + wisp::ScriptType + 'static,
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
