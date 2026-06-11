//! The gatherer phase (PRD §9 step 2). All invocations are independent by
//! definition, so unique executions run concurrently — deduplicated by
//! `(gatherer, canonicalised params)` — and any failure aborts the run
//! before step execution.

use std::collections::HashMap;

use wisp::Vm;
use wisp_std::DynValue;

use crate::convert::{canonicalise, dyn_to_wcl, wcl_to_dyn};
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
    ctx: &wisp::Context,
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
                match f.value() {
                    Ok(v) => match wcl_to_dyn(v) {
                        Ok(dv) => {
                            params.insert(f.name().to_string(), dv);
                        }
                        Err(e) => diags.push(Diag::spanned(
                            format!("gather '{}' param '{}': {e}", inv.name, f.name()),
                            "here",
                            &pb.root.join("playbook.wcl"),
                            &pb.source,
                            wcl_span(f.span()),
                        )),
                    },
                    Err(e) => {
                        diags.push(Diag::from_eval(
                            e.clone(),
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
                let gatherer = scripts.gatherers.get(key);
                scope.spawn(move || -> Result<DynValue, String> {
                    let Some(g) = gatherer else {
                        return Err(format!("no compiled gatherer '{key}'"));
                    };
                    let _worker = crate::hostapi::worker_init();
                    crate::logging::install_gatherer_sink(key);
                    let mut vm = Vm::new(ctx);
                    let outcome: Result<DynValue, String> = match g.gather {
                        EntryKind::Plain => vm
                            .call_unit(&g.unit, "gather", (params.clone(),))
                            .map_err(|e| e.to_string()),
                        EntryKind::Fallible => vm
                            .call_unit::<_, Result<DynValue, String>>(
                                &g.unit,
                                "gather",
                                (params.clone(),),
                            )
                            .map_err(|e| e.to_string())
                            .and_then(|r| r),
                    };
                    outcome
                })
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
                store.insert(name, Origin::Gatherer, dyn_to_wcl(value));
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
