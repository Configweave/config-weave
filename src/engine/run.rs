//! Step execution (PRD §9): plan → schedule → lifecycle.
//!
//! Conditions and properties are evaluated **once, on the scheduler
//! thread, in declaration order** (the WCL document is thread-bound);
//! workers receive pure data. The scheduler owns the DAG and dispatches
//! ready steps to a pool of worker threads — one wisp VM world per
//! worker — honouring concurrency classes:
//!
//! - `parallel`: no restriction
//! - `exclusive`: at most one in-flight step per resource type
//! - `global`: drain all in-flight steps, run solo, resume
//!
//! Halting (Error without --continue-on-error, or RebootRequired in
//! apply) stops dispatching and lets in-flight steps finish — no
//! mid-flight cancellation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wcl_lang::{Block, Value};
use wisp::{Context, Vm};
use wisp_std::DynValue;

use crate::convert::wcl_to_dyn;
use crate::diag::Diag;
use crate::hostapi::{ApplyResult, CheckResult, log};
use crate::model::{Concurrency, Play, Playbook, Step};

use super::dag;
use super::events::{Event, EventSink, Phase};
use super::gather::apply_param_defaults;
use super::scripts::{CompiledResource, EntryKind, ScriptSet};
use super::status::*;
use super::vars::VarStore;

pub struct RunOptions {
    pub mode: Mode,
    pub continue_on_error: bool,
    pub jobs: usize,
    pub events: EventSink,
}

pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(8)
}

/// What the planner decided for one step.
enum Plan {
    Skip(Option<String>),
    Fail(String),
    Run {
        resource_key: String,
        concurrency: Concurrency,
        params: DynValue,
    },
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
    let doc = store.open_playbook(pb).map_err(|d| vec![d])?;
    let play_block = find_play_block(&doc, &play.name).ok_or_else(|| {
        vec![Diag::bare(format!(
            "play '{}' not found at run time",
            play.name
        ))]
    })?;

    let mut step_blocks: HashMap<(Vec<String>, String), Block<'_>> = HashMap::new();
    let mut container_blocks: HashMap<Vec<String>, Block<'_>> = HashMap::new();
    index_blocks(
        &play_block,
        &mut Vec::new(),
        &mut step_blocks,
        &mut container_blocks,
    );

    let steps: Vec<&Step> = play.steps();
    let dag = dag::build(play)?;

    // ---- plan phase: evaluate conditions and properties in order.
    let mut container_cache: HashMap<Vec<String>, Result<bool, String>> = HashMap::new();
    let plans: Vec<Plan> = steps
        .iter()
        .map(|step| {
            plan_step(
                pb,
                step,
                &step_blocks,
                &container_blocks,
                &mut container_cache,
            )
        })
        .collect();
    drop(doc);

    // ---- dispatch phase.
    let workers = if play.parallel { opts.jobs.max(1) } else { 1 };
    let reports = schedule(&steps, &plans, &dag, scripts, ctx, opts, workers);

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
        steps: reports,
        duration: started.elapsed(),
    })
}

fn plan_step(
    pb: &Playbook,
    step: &Step,
    step_blocks: &HashMap<(Vec<String>, String), Block<'_>>,
    container_blocks: &HashMap<Vec<String>, Block<'_>>,
    container_cache: &mut HashMap<Vec<String>, Result<bool, String>>,
) -> Plan {
    let key = (step.container_path.clone(), step.name.clone());
    let Some(block) = step_blocks.get(&key) else {
        return Plan::Fail("step block not found at run time".into());
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
            Ok(false) => return Plan::Skip(Some("container condition is false".into())),
            Err(e) => return Plan::Fail(e.clone()),
        }
    }
    match eval_condition(block) {
        Ok(true) => {}
        Ok(false) => return Plan::Skip(None),
        Err(e) => return Plan::Fail(e),
    }

    let Some(decl) = pb.resource(&step.package, &step.resource) else {
        return Plan::Fail("resource declaration missing".into());
    };
    let mut params: HashMap<String, DynValue> = HashMap::new();
    if let Some(props) = block.blocks().find(|b| b.kind() == "properties") {
        for f in props.fields() {
            match f.value() {
                Ok(v) => match wcl_to_dyn(v) {
                    Ok(dv) => {
                        params.insert(f.name().to_string(), dv);
                    }
                    Err(e) => return Plan::Fail(format!("property '{}': {e}", f.name())),
                },
                Err(e) => return Plan::Fail(format!("property '{}': {e}", f.name())),
            }
        }
    }
    if let Err(errors) = apply_param_defaults(&mut params, &decl.params) {
        return Plan::Fail(errors.join("; "));
    }

    // A step may tighten its resource's concurrency class, never loosen.
    let concurrency = step
        .concurrency
        .map_or(decl.concurrency, |c| c.max(decl.concurrency));

    Plan::Run {
        resource_key: format!("{}.{}", step.package, step.resource),
        concurrency,
        params: DynValue::Map(params),
    }
}

// ------------------------------------------------------------ scheduler

struct Job {
    idx: usize,
    resource_key: String,
    params: DynValue,
    step_label: String,
    mode: Mode,
}

struct Done {
    idx: usize,
    worker: usize,
    status: StepStatus,
    message: Option<String>,
    duration: Duration,
}

fn schedule(
    steps: &[&Step],
    plans: &[Plan],
    dag: &dag::StepDag,
    scripts: &ScriptSet,
    ctx: &Context,
    opts: &RunOptions,
    workers: usize,
) -> Vec<StepReport> {
    let n = steps.len();
    let mut reports: Vec<Option<StepReport>> = (0..n).map(|_| None).collect();
    let mut dispatched = vec![false; n];
    let mut completed = vec![false; n];
    let mut halted = false;
    let mut in_flight = 0usize;
    let mut running_exclusive: HashSet<String> = HashSet::new();
    let mut global_running = false;

    let (done_tx, done_rx) = mpsc::channel::<Done>();
    let (job_txs, worker_handles): (Vec<_>, Vec<_>) = (0..workers)
        .map(|worker_id| {
            let (tx, rx) = mpsc::channel::<Job>();
            let done = done_tx.clone();
            let handle = std::thread::spawn({
                let scripts: HashMap<String, _> = scripts
                    .resources
                    .iter()
                    .map(|(k, v)| (k.clone(), (v.unit.clone(), v.check, v.apply)))
                    .collect();
                let ctx = ctx.clone();
                let events = opts.events.clone();
                move || {
                    let _guard = crate::hostapi::worker_init();
                    while let Ok(job) = rx.recv() {
                        let started = Instant::now();
                        events(Event::StepStarted {
                            idx: job.idx,
                            name: job.step_label.clone(),
                        });
                        let label = job.step_label.clone();
                        crate::logging::install_step_sink(&label, &job.resource_key);
                        let phase_events = events.clone();
                        let phase_idx = job.idx;
                        let phase_name = job.step_label.clone();
                        let on_phase = move |phase: Phase| {
                            phase_events(Event::StepPhase {
                                idx: phase_idx,
                                name: phase_name.clone(),
                                phase,
                            });
                        };
                        let (status, message) = match scripts.get(&job.resource_key) {
                            Some((unit, check, apply)) => {
                                let res = CompiledResource {
                                    unit: unit.clone(),
                                    check: *check,
                                    apply: *apply,
                                };
                                run_lifecycle(&res, &ctx, job.params.clone(), job.mode, &on_phase)
                            }
                            None => (
                                StepStatus::Error,
                                Some(format!("no compiled resource '{}'", job.resource_key)),
                            ),
                        };
                        log::clear_sink();
                        let _ = done.send(Done {
                            idx: job.idx,
                            worker: worker_id,
                            status,
                            message,
                            duration: started.elapsed(),
                        });
                    }
                }
            });
            (tx, handle)
        })
        .unzip();
    drop(done_tx);
    let mut idle_workers: VecDeque<usize> = (0..workers).collect();

    let deps_satisfied = |i: usize, completed: &[bool]| dag.deps[i].iter().all(|&d| completed[d]);

    loop {
        // Dispatch everything currently allowed.
        let mut progressed = true;
        while progressed {
            progressed = false;
            for i in 0..n {
                if dispatched[i] || !deps_satisfied(i, &completed) {
                    continue;
                }
                // Halted: complete remaining steps as NotRun immediately.
                if halted {
                    dispatched[i] = true;
                    completed[i] = true;
                    reports[i] = Some(quick_report(steps[i], StepStatus::NotRun, None));
                    (opts.events)(Event::StepResolved {
                        idx: i,
                        name: steps[i].name.clone(),
                        status: StepStatus::NotRun,
                    });
                    progressed = true;
                    continue;
                }
                // Blocked by a failed/not-run dependency (apply mode).
                if opts.mode == Mode::Apply {
                    let blocked = dag.deps[i].iter().any(|&d| {
                        matches!(
                            reports[d].as_ref().map(|r| r.status),
                            Some(StepStatus::Error) | Some(StepStatus::NotRun)
                        )
                    });
                    if blocked {
                        dispatched[i] = true;
                        completed[i] = true;
                        reports[i] = Some(quick_report(
                            steps[i],
                            StepStatus::NotRun,
                            Some("a required step did not complete".into()),
                        ));
                        (opts.events)(Event::StepResolved {
                            idx: i,
                            name: steps[i].name.clone(),
                            status: StepStatus::NotRun,
                        });
                        progressed = true;
                        continue;
                    }
                }
                match &plans[i] {
                    Plan::Skip(msg) => {
                        dispatched[i] = true;
                        completed[i] = true;
                        reports[i] = Some(quick_report(steps[i], StepStatus::Skipped, msg.clone()));
                        (opts.events)(Event::StepResolved {
                            idx: i,
                            name: steps[i].name.clone(),
                            status: StepStatus::Skipped,
                        });
                        progressed = true;
                    }
                    Plan::Fail(msg) => {
                        dispatched[i] = true;
                        completed[i] = true;
                        reports[i] =
                            Some(quick_report(steps[i], StepStatus::Error, Some(msg.clone())));
                        (opts.events)(Event::StepResolved {
                            idx: i,
                            name: steps[i].name.clone(),
                            status: StepStatus::Error,
                        });
                        if !opts.continue_on_error {
                            halted = true;
                        }
                        progressed = true;
                    }
                    Plan::Run {
                        resource_key,
                        concurrency,
                        params,
                    } => {
                        // Concurrency gating.
                        if global_running || idle_workers.is_empty() {
                            continue;
                        }
                        match concurrency {
                            Concurrency::Parallel => {}
                            Concurrency::Exclusive => {
                                if running_exclusive.contains(resource_key) {
                                    continue;
                                }
                            }
                            Concurrency::Global => {
                                // Solo: wait until nothing is in flight.
                                if in_flight > 0 {
                                    continue;
                                }
                            }
                        }
                        let Some(worker) = idle_workers.pop_front() else {
                            break;
                        };
                        dispatched[i] = true;
                        in_flight += 1;
                        if *concurrency == Concurrency::Exclusive {
                            running_exclusive.insert(resource_key.clone());
                        }
                        if *concurrency == Concurrency::Global {
                            global_running = true;
                        }
                        let _ = job_txs[worker].send(Job {
                            idx: i,
                            resource_key: resource_key.clone(),
                            params: params.clone(),
                            step_label: steps[i].name.clone(),
                            mode: opts.mode,
                        });
                        progressed = true;
                        // A global step blocks everything else this round.
                        if global_running {
                            break;
                        }
                    }
                }
            }
            if global_running {
                break;
            }
        }

        if in_flight == 0 {
            // Nothing running; if nothing is dispatchable either, finish.
            let any_pending = (0..n).any(|i| !dispatched[i]);
            if !any_pending {
                break;
            }
            // Pending steps but nothing in flight and no dispatch occurred:
            // only possible if every pending step waits on a dependency
            // that never completed — defensive guard against deadlock.
            let any_ready = (0..n).any(|i| !dispatched[i] && deps_satisfied(i, &completed));
            if !any_ready {
                for i in 0..n {
                    if !dispatched[i] {
                        dispatched[i] = true;
                        completed[i] = true;
                        reports[i] = Some(quick_report(
                            steps[i],
                            StepStatus::NotRun,
                            Some("dependency never completed".into()),
                        ));
                    }
                }
                break;
            }
            continue;
        }

        // Wait for one completion.
        let Ok(done) = done_rx.recv() else { break };
        in_flight -= 1;
        idle_workers.push_back(done.worker);
        completed[done.idx] = true;
        if let Plan::Run {
            resource_key,
            concurrency,
            ..
        } = &plans[done.idx]
        {
            if *concurrency == Concurrency::Exclusive {
                running_exclusive.remove(resource_key);
            }
            if *concurrency == Concurrency::Global {
                global_running = false;
            }
        }
        match done.status {
            StepStatus::Error if !opts.continue_on_error => halted = true,
            StepStatus::RebootRequired if opts.mode == Mode::Apply => halted = true,
            _ => {}
        }
        let report = StepReport {
            name: steps[done.idx].name.clone(),
            container_path: steps[done.idx].container_path.clone(),
            resource: format!("{}.{}", steps[done.idx].package, steps[done.idx].resource),
            status: done.status,
            message: done.message,
            duration: done.duration,
        };
        (opts.events)(Event::StepFinished {
            idx: done.idx,
            report: report.clone(),
        });
        reports[done.idx] = Some(report);
    }

    drop(job_txs);
    for handle in worker_handles {
        let _ = handle.join();
    }

    reports.into_iter().map(|r| r.unwrap()).collect()
}

fn quick_report(step: &Step, status: StepStatus, message: Option<String>) -> StepReport {
    StepReport {
        name: step.name.clone(),
        container_path: step.container_path.clone(),
        resource: format!("{}.{}", step.package, step.resource),
        status,
        message,
        duration: Duration::ZERO,
    }
}

// ------------------------------------------------------------ lifecycle

/// The PRD §9 lifecycle for one step, mode-aware.
fn run_lifecycle(
    res: &CompiledResource,
    ctx: &Context,
    params: DynValue,
    mode: Mode,
    on_phase: &dyn Fn(Phase),
) -> (StepStatus, Option<String>) {
    on_phase(Phase::Checking);
    let check1 = match call_check(res, ctx, params.clone()) {
        Ok(c) => c,
        Err(e) => return (StepStatus::Error, Some(e)),
    };
    match (mode, check1) {
        (_, CheckResult::AlreadyConfigured) => (StepStatus::AlreadyConfigured, None),
        (_, CheckResult::RebootRequired) => (StepStatus::RebootRequired, None),
        (Mode::Check, CheckResult::NotConfigured) => (StepStatus::NotConfigured, None),
        (Mode::Apply, CheckResult::NotConfigured) => {
            on_phase(Phase::Applying);
            match call_apply(res, ctx, params.clone()) {
                Err(e) => (StepStatus::Error, Some(e)),
                Ok(ApplyResult::RebootRequired) => (StepStatus::RebootRequired, None),
                Ok(ApplyResult::Success) => {
                    on_phase(Phase::Rechecking);
                    match call_check(res, ctx, params) {
                        Ok(CheckResult::AlreadyConfigured) => (StepStatus::Configured, None),
                        Ok(other) => (
                            StepStatus::Error,
                            Some(format!(
                                "apply claimed success but the re-check disagrees ({other:?})"
                            )),
                        ),
                        Err(e) => (StepStatus::Error, Some(format!("re-check failed: {e}"))),
                    }
                }
            }
        }
    }
}

fn call_check(
    res: &CompiledResource,
    ctx: &Context,
    params: DynValue,
) -> Result<CheckResult, String> {
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

fn call_apply(
    res: &CompiledResource,
    ctx: &Context,
    params: DynValue,
) -> Result<ApplyResult, String> {
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
