//! Step execution: the check → apply → re-check lifecycle (PRD §9).
//! M2 executes sequentially in a stable topological order; the M5
//! scheduler replaces the dispatch loop, not the lifecycle.

use std::collections::HashMap;
use std::time::Instant;

use wcl_lang::{Block, Value};
use wisp::{Context, Vm};
use wisp_std::DynValue;

use crate::convert::wcl_to_dyn;
use crate::diag::Diag;
use crate::hostapi::{ApplyResult, CheckResult, log};
use crate::model::{Play, Playbook, Step};

use super::dag;
use super::gather::apply_param_defaults;
use super::scripts::{CompiledResource, EntryKind, ScriptSet};
use super::status::*;
use super::vars::VarStore;

pub struct RunOptions {
    pub mode: Mode,
    pub continue_on_error: bool,
}

/// Execute one play. `store` must already hold gatherer results and
/// overrides. Returns the deterministic, declaration-ordered report.
pub fn run_play(
    pb: &Playbook,
    play: &Play,
    scripts: &ScriptSet,
    ctx: &Context,
    store: &VarStore,
    opts: &RunOptions,
) -> Result<RunReport, Vec<Diag>> {
    let started = Instant::now();
    crate::hostapi::redirect_print_to_log();
    let doc = store.open_playbook(pb).map_err(|d| vec![d])?;
    let play_block = find_play_block(&doc, &play.name)
        .ok_or_else(|| vec![Diag::bare(format!("play '{}' not found at run time", play.name))])?;

    // Index step blocks by (container path, step name).
    let mut step_blocks: HashMap<(Vec<String>, String), Block<'_>> = HashMap::new();
    let mut container_blocks: HashMap<Vec<String>, Block<'_>> = HashMap::new();
    index_blocks(&play_block, &mut Vec::new(), &mut step_blocks, &mut container_blocks);

    let steps: Vec<&Step> = play.steps();
    let dag = dag::build(play).map_err(|e| e)?;

    let mut reports: Vec<Option<StepReport>> = steps.iter().map(|_| None).collect();
    let mut done = vec![false; steps.len()];
    let mut halted = false;
    let mut container_condition_cache: HashMap<Vec<String>, Result<bool, String>> = HashMap::new();

    // Stable sequential dispatch: repeatedly run the first
    // declaration-order step whose dependencies completed.
    loop {
        let Some(idx) = next_ready(&steps, &dag, &done) else {
            break;
        };
        let step = steps[idx];
        done[idx] = true;

        if halted {
            reports[idx] = Some(report_for(step, StepStatus::NotRun, None, started));
            continue;
        }

        // In apply mode a failed/not-run dependency blocks dependents.
        if opts.mode == Mode::Apply {
            let blocked = dag.deps[idx].iter().any(|&d| {
                matches!(
                    reports[d].as_ref().map(|r| r.status),
                    Some(StepStatus::Error) | Some(StepStatus::NotRun)
                )
            });
            if blocked {
                reports[idx] = Some(report_for(
                    step,
                    StepStatus::NotRun,
                    Some("a required step did not complete".into()),
                    started,
                ));
                continue;
            }
        }

        let key = (step.container_path.clone(), step.name.clone());
        let Some(block) = step_blocks.get(&key) else {
            reports[idx] = Some(report_for(
                step,
                StepStatus::Error,
                Some("step block not found at run time".into()),
                started,
            ));
            if !opts.continue_on_error {
                halted = true;
            }
            continue;
        };

        let report = execute_step(
            pb,
            step,
            block,
            &container_blocks,
            &mut container_condition_cache,
            scripts,
            ctx,
            opts,
        );
        match report.status {
            StepStatus::Error if !opts.continue_on_error => halted = true,
            StepStatus::RebootRequired if opts.mode == Mode::Apply => halted = true,
            _ => {}
        }
        reports[idx] = Some(report);
    }

    let gathered = pb
        .gathers
        .iter()
        .map(|g| GatherReport {
            name: g.name.clone(),
            gatherer: format!("{}.{}", g.package, g.gatherer),
        })
        .collect();

    Ok(RunReport {
        playbook: pb.name.clone(),
        version: pb.version.clone(),
        play: play.name.clone(),
        mode: opts.mode,
        gathered,
        steps: reports.into_iter().map(|r| r.unwrap()).collect(),
        duration: started.elapsed(),
    })
}

fn next_ready(steps: &[&Step], dag: &dag::StepDag, done: &[bool]) -> Option<usize> {
    (0..steps.len()).find(|&i| !done[i] && dag.deps[i].iter().all(|&d| done[d]))
}

fn report_for(
    step: &Step,
    status: StepStatus,
    message: Option<String>,
    _started: Instant,
) -> StepReport {
    StepReport {
        name: step.name.clone(),
        container_path: step.container_path.clone(),
        resource: format!("{}.{}", step.package, step.resource),
        status,
        message,
        duration: std::time::Duration::ZERO,
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_step(
    pb: &Playbook,
    step: &Step,
    block: &Block<'_>,
    container_blocks: &HashMap<Vec<String>, Block<'_>>,
    container_cache: &mut HashMap<Vec<String>, Result<bool, String>>,
    scripts: &ScriptSet,
    ctx: &Context,
    opts: &RunOptions,
) -> StepReport {
    let started = Instant::now();
    let mut finish = |status: StepStatus, message: Option<String>| StepReport {
        name: step.name.clone(),
        container_path: step.container_path.clone(),
        resource: format!("{}.{}", step.package, step.resource),
        status,
        message,
        duration: started.elapsed(),
    };

    // Container conditions (outermost first), then the step's own.
    for depth in 1..=step.container_path.len() {
        let path = step.container_path[..depth].to_vec();
        let result = container_cache
            .entry(path.clone())
            .or_insert_with(|| match container_blocks.get(&path) {
                Some(cb) => eval_condition(cb),
                None => Err("container block not found".into()),
            });
        match result {
            Ok(true) => {}
            Ok(false) => return finish(StepStatus::Skipped, Some("container condition is false".into())),
            Err(e) => return finish(StepStatus::Error, Some(e.clone())),
        }
    }
    match eval_condition(block) {
        Ok(true) => {}
        Ok(false) => return finish(StepStatus::Skipped, None),
        Err(e) => return finish(StepStatus::Error, Some(e)),
    }

    // Properties → DynValue map with declared defaults applied.
    let decl = match pb.resource(&step.package, &step.resource) {
        Some(d) => d,
        None => return finish(StepStatus::Error, Some("resource declaration missing".into())),
    };
    let mut params: HashMap<String, DynValue> = HashMap::new();
    if let Some(props) = block.blocks().find(|b| b.kind() == "properties") {
        for f in props.fields() {
            match f.value() {
                Ok(v) => match wcl_to_dyn(v) {
                    Ok(dv) => {
                        params.insert(f.name().to_string(), dv);
                    }
                    Err(e) => {
                        return finish(
                            StepStatus::Error,
                            Some(format!("property '{}': {e}", f.name())),
                        );
                    }
                },
                Err(e) => {
                    return finish(
                        StepStatus::Error,
                        Some(format!("property '{}': {e}", f.name())),
                    );
                }
            }
        }
    }
    if let Err(errors) = apply_param_defaults(&mut params, &decl.params) {
        return finish(StepStatus::Error, Some(errors.join("; ")));
    }
    let params = DynValue::Map(params);

    let resource_key = format!("{}.{}", step.package, step.resource);
    let Some(compiled) = scripts.resources.get(&resource_key) else {
        return finish(StepStatus::Error, Some(format!("no compiled resource '{resource_key}'")));
    };

    // Route script logs with step context while this step runs.
    let step_label = format!("{}", step.name);
    log::set_sink(Box::new(move |level, msg| {
        eprintln!("    [{step_label}] {}: {msg}", level.as_str());
    }));
    let result = run_lifecycle(compiled, ctx, params, opts.mode);
    log::clear_sink();

    let (status, message) = result;
    finish(status, message)
}

/// The PRD §9 lifecycle for one step, mode-aware.
fn run_lifecycle(
    res: &CompiledResource,
    ctx: &Context,
    params: DynValue,
    mode: Mode,
) -> (StepStatus, Option<String>) {
    let check1 = match call_check(res, ctx, params.clone()) {
        Ok(c) => c,
        Err(e) => return (StepStatus::Error, Some(e)),
    };
    match (mode, check1) {
        (_, CheckResult::AlreadyConfigured) => (StepStatus::AlreadyConfigured, None),
        (_, CheckResult::RebootRequired) => (StepStatus::RebootRequired, None),
        (Mode::Check, CheckResult::NotConfigured) => (StepStatus::NotConfigured, None),
        (Mode::Apply, CheckResult::NotConfigured) => {
            match call_apply(res, ctx, params.clone()) {
                Err(e) => (StepStatus::Error, Some(e)),
                Ok(ApplyResult::RebootRequired) => (StepStatus::RebootRequired, None),
                Ok(ApplyResult::Success) => match call_check(res, ctx, params) {
                    Ok(CheckResult::AlreadyConfigured) => (StepStatus::Configured, None),
                    Ok(other) => (
                        StepStatus::Error,
                        Some(format!(
                            "apply claimed success but the re-check disagrees ({other:?})"
                        )),
                    ),
                    Err(e) => (StepStatus::Error, Some(format!("re-check failed: {e}"))),
                },
            }
        }
    }
}

fn call_check(res: &CompiledResource, ctx: &Context, params: DynValue) -> Result<CheckResult, String> {
    let mut vm = Vm::new(ctx);
    match res.check {
        EntryKind::Plain => vm
            .call_unit(&res.unit, "check", (params,))
            .map_err(|e| e.to_string()),
        EntryKind::Fallible => vm
            .call_unit::<_, Result<CheckResult, String>>(&res.unit, "check", (params,))
            .map_err(|e| e.to_string())
            .and_then(|r| r),
    }
}

fn call_apply(res: &CompiledResource, ctx: &Context, params: DynValue) -> Result<ApplyResult, String> {
    let mut vm = Vm::new(ctx);
    match res.apply {
        EntryKind::Plain => vm
            .call_unit(&res.unit, "apply", (params,))
            .map_err(|e| e.to_string()),
        EntryKind::Fallible => vm
            .call_unit::<_, Result<ApplyResult, String>>(&res.unit, "apply", (params,))
            .map_err(|e| e.to_string())
            .and_then(|r| r),
    }
}

/// Evaluate a block's `condition` field; absent means true.
fn eval_condition(block: &Block<'_>) -> Result<bool, String> {
    let Some(f) = block.fields().find(|f| f.name() == "condition") else {
        return Ok(true);
    };
    match f.value() {
        Ok(Value::Bool(b)) => Ok(*b),
        Ok(other) => Err(format!("condition must evaluate to a bool, got {other:?}")),
        Err(e) => Err(format!("condition failed to evaluate: {e}")),
    }
}

fn find_play_block<'a>(doc: &'a wcl_lang::Document, name: &str) -> Option<Block<'a>> {
    let pb = doc.block("playbook")?;
    pb.blocks()
        .filter(|b| b.kind() == "play")
        .find(|b| crate::model::label_string(b).as_deref() == Some(name))
}

/// Recursively index step and container blocks under a play by container
/// path, mirroring the model loader's walk.
fn index_blocks<'a>(
    parent: &Block<'a>,
    path: &mut Vec<String>,
    steps: &mut HashMap<(Vec<String>, String), Block<'a>>,
    containers: &mut HashMap<Vec<String>, Block<'a>>,
) {
    for block in parent.blocks() {
        match block.kind() {
            "step" => {
                if let Some(name) = crate::model::label_string(&block) {
                    steps.insert((path.clone(), name), block);
                }
            }
            "container" => {
                if let Some(name) = crate::model::label_string(&block) {
                    path.push(name);
                    containers.insert(path.clone(), block.clone());
                    index_blocks(&block.clone(), path, steps, containers);
                    path.pop();
                }
            }
            _ => {}
        }
    }
}
