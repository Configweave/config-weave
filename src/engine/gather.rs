//! The gatherer phase (PRD §9 step 2). All invocations are independent by
//! definition, so unique executions run concurrently — deduplicated by
//! `(gatherer, canonicalised params)` — and any failure aborts the run
//! before step execution.

use std::collections::HashMap;

use wscript::Vm;
use wscript_std::DynValue;

use crate::convert::{
    FieldValueError, canonicalise, dyn_to_wcl_returns, field_value_dyn, returns_symbol_violations,
};
use crate::diag::{Diag, wcl_span};
use crate::model::{ParamDecl, Playbook};

use super::events::{Event, EventSink};
use super::scripts::{EntryKind, ScriptSet};
use super::vars::{Origin, VarStore};

/// Run every gather invocation and bind results into `store`.
/// `store` already carries `--var` / `--var-file` overrides, which gather
/// params may reference.
pub fn run(
    pb: &Playbook,
    scripts: &ScriptSet,
    ctx: &wscript::Context,
    store: &mut VarStore,
    events: &EventSink,
) -> Result<(), Vec<Diag>> {
    // Evaluate every invocation's params against the override-only scope.
    let doc = store.open_playbook(pb).map_err(|d| vec![d])?;
    let Some(pb_block) = doc.block("playbook") else {
        return Err(vec![Diag::bare("playbook block disappeared at run time")]);
    };

    let mut diags = Vec::new();
    // (invocation name, gatherer key, params, dedup key)
    let mut invocations: Vec<(String, String, DynValue, String)> = Vec::new();

    for block in pb_block.blocks().filter(|b| b.kind() == "gather") {
        let label = crate::model::label_string(&block);
        let Some(inv) = pb
            .gathers
            .iter()
            .find(|g| Some(g.name.as_str()) == label.as_deref())
        else {
            continue;
        };
        let decl_params = &pb
            .packages
            .get(&inv.package)
            .and_then(|p| p.gatherers.get(&inv.gatherer))
            .map(|g| g.params.clone())
            .unwrap_or_default();

        let mut params: HashMap<String, DynValue> = HashMap::new();
        if let Some(pblock) = block.blocks().find(|b| b.kind() == "params") {
            for f in pblock.fields() {
                match field_value_dyn(&f) {
                    Ok(fv) => {
                        params.insert(f.name().to_string(), fv.value);
                    }
                    Err(FieldValueError::Convert(e)) => diags.push(Diag::spanned(
                        format!("gather '{}' param '{}': {e}", inv.name, f.name()),
                        "here",
                        &pb.root.join("playbook.wcl"),
                        &pb.source,
                        wcl_span(f.span()),
                    )),
                    Err(FieldValueError::Unresolved(e) | FieldValueError::Eval(e)) => {
                        diags.push(Diag::from_eval(
                            e,
                            &pb.root.join("playbook.wcl"),
                            &pb.source,
                        ));
                        diags.push(Diag::bare(format!(
                            "gather '{}' params must not reference gatherer results \
                             (gatherers run before variables resolve)",
                            inv.name
                        )));
                    }
                }
            }
        }
        if let Err(es) = apply_param_defaults(&mut params, decl_params) {
            for e in es {
                diags.push(Diag::bare(format!("gather '{}': {e}", inv.name)));
            }
        }

        let key = format!("{}.{}", inv.package, inv.gatherer);
        let dedup = format!("{key}:{}", canonicalise(&DynValue::Map(params.clone())));
        invocations.push((inv.name.clone(), key, DynValue::Map(params), dedup));
    }
    if !diags.is_empty() {
        return Err(diags);
    }

    // Deduplicate executions; remember which invocations share them.
    let mut unique: Vec<(String, DynValue, String)> = Vec::new(); // gatherer key, params, dedup
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (_, key, params, dedup) in &invocations {
        if !seen.contains_key(dedup) {
            seen.insert(dedup.clone(), unique.len());
            unique.push((key.clone(), params.clone(), dedup.clone()));
        }
    }

    // Run unique executions concurrently, one VM per thread.
    events(Event::GatherStarted {
        unique: unique.len(),
    });
    let results: Vec<Result<DynValue, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = unique
            .iter()
            .map(|(key, params, _)| {
                scope.spawn(move || run_single(scripts, ctx, key, params.clone()))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("gatherer thread panicked".into()))
            })
            .collect()
    });

    events(Event::GatherFinished);

    let mut by_dedup: HashMap<&str, &Result<DynValue, String>> = HashMap::new();
    for ((_, _, dedup), result) in unique.iter().zip(results.iter()) {
        by_dedup.insert(dedup.as_str(), result);
    }

    for (name, key, _, dedup) in &invocations {
        match by_dedup.get(dedup.as_str()) {
            Some(Ok(value)) => {
                // A `returns` key declared `symbol` binds as a real WCL
                // symbol, and its declared set is enforced here — an
                // out-of-set value is a bug in the gatherer script.
                let returns = returns_for(pb, key);
                for why in returns_symbol_violations(value, &returns) {
                    diags.push(Diag::bare(format!("gatherer '{key}': {why}")));
                }
                store.insert(name, Origin::Gatherer, dyn_to_wcl_returns(value, &returns));
            }
            Some(Err(e)) => {
                diags.push(Diag::bare(format!(
                    "gatherer '{key}' (for variable '{name}') failed: {e}"
                )));
            }
            None => {}
        }
    }

    if diags.is_empty() { Ok(()) } else { Err(diags) }
}

/// The `returns` declarations of the gatherer a `"package.gatherer"` key
/// names; empty when either half doesn't resolve (the loader has already
/// reported that).
fn returns_for(pb: &Playbook, key: &str) -> Vec<crate::model::ReturnDecl> {
    let Some((pkg, gatherer)) = key.split_once('.') else {
        return Vec::new();
    };
    pb.packages
        .get(pkg)
        .and_then(|p| p.gatherers.get(gatherer))
        .map(|g| g.returns.clone())
        .unwrap_or_default()
}

/// Run one gatherer on a fresh VM: the per-execution body of the gather
/// phase, also driven directly by the `__gather` test-protocol
/// subcommand.
pub fn run_single(
    scripts: &ScriptSet,
    ctx: &wscript::Context,
    key: &str,
    params: DynValue,
) -> Result<DynValue, String> {
    let Some(g) = scripts.gatherers.get(key) else {
        return Err(format!("no compiled gatherer '{key}'"));
    };
    let _worker = crate::hostapi::worker_init();
    crate::logging::install_gatherer_sink(key);
    let mut vm = Vm::new(ctx);
    match g.gather {
        EntryKind::Plain => vm
            .call_unit(&g.unit, "gather", (params,))
            .map_err(|e| e.to_string()),
        EntryKind::Fallible => vm
            .call_unit::<_, Result<DynValue, String>>(&g.unit, "gather", (params,))
            .map_err(|e| e.to_string())
            .and_then(|r| r),
    }
}

/// Fill in declared defaults and enforce required/type at run time.
pub fn apply_param_defaults(
    params: &mut HashMap<String, DynValue>,
    decls: &[ParamDecl],
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for decl in decls {
        match params.get(&decl.name) {
            Some(v) => {
                if !decl.ty.matches(v) {
                    errors.push(format!(
                        "parameter '{}' expects {}, got {}",
                        decl.name,
                        decl.ty.as_str(),
                        crate::model::CoarseType::describe(v)
                    ));
                } else if let Some(why) = decl.symbol_violation(v) {
                    // Values that came from variables skip the load-time
                    // check (they don't evaluate statically), so the
                    // declared symbol set is enforced here too.
                    errors.push(format!(
                        "parameter '{}' is not a declared symbol: {why}",
                        decl.name
                    ));
                }
            }
            None => {
                if let Some(d) = &decl.default {
                    params.insert(decl.name.clone(), d.clone());
                } else if decl.required {
                    errors.push(format!("missing required parameter '{}'", decl.name));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
