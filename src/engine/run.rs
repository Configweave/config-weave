//! Step execution (PRD §9): plan → schedule → lifecycle.
//!
//! Conditions and properties are evaluated **once, on the scheduler
//! thread, in declaration order** (the WCL document is thread-bound);
//! workers receive pure data. The scheduler owns the DAG and dispatches
//! ready steps to a pool of worker threads — one wscript VM world per
//! worker — honouring concurrency classes:
//!
//! - `parallel`: no restriction
//! - `exclusive`: at most one in-flight step per resource type
//! - `global`: drain all in-flight steps, run solo, resume
//!
//! Independently of the class, a resource may name a `lock` group: at most
//! one in-flight step per (package, lock) across every resource type in
//! that package. A step starts only when both its class and its lock
//! allow; one waiting on either is passed over, never blocking the scan.
//!
//! Wait steps (`wait_for`) sit outside all of that: each runs on its own
//! thread rather than a pool worker, so any number may wait without
//! starving the pool, and they are always `parallel`. They still count as
//! in flight, so a `global` step drains them and a sequential play
//! (`parallel = false`) runs them one step at a time like anything else.
//!
//! Halting (Error without --continue-on-error, or RebootRequired in
//! apply) stops dispatching and lets in-flight resource steps finish — no
//! mid-flight cancellation of a half-run script. Wait steps are the
//! exception: they are cancelled at their next pause and report `NotRun`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wcl_lang::{Block, Value};
use wscript::Context;
use wscript_std::DynValue;

use crate::convert::{FieldValueError, field_value_dyn};
use crate::diag::Diag;
use crate::hostapi::{ApplyResult, CheckResult, log};
use crate::model::{CompositeFrame, Concurrency, ParamDecl, Play, Playbook, Step, StepKind};

use super::dag;
use super::events::{Event, EventSink, Phase};
use super::gather::apply_param_defaults;
use super::scripts::{CompiledProbe, CompiledResource, EntryKind, ScriptSet};
use super::status::*;
use super::vars::VarStore;
use super::wait::{self, Cancel, WaitSpec};

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
        /// `(package, lock name)` — lock names are package-scoped.
        lock: Option<(String, String)>,
        params: DynValue,
    },
    /// A wait step: poll the probe until ready or the timeout passes.
    Wait {
        probe_key: String,
        params: DynValue,
        spec: WaitSpec,
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
    let mut planner = Planner {
        pb,
        store,
        step_blocks: &step_blocks,
        container_blocks: &container_blocks,
        container_cache: HashMap::new(),
        arg_cache: HashMap::new(),
    };
    let plans: Vec<Plan> = steps.iter().map(|step| planner.plan(step)).collect();
    drop(planner);
    drop(doc);

    // ---- dispatch phase.
    let workers = if play.parallel { opts.jobs.max(1) } else { 1 };
    let reports = schedule(
        &steps,
        &plans,
        &dag,
        scripts,
        ctx,
        opts,
        workers,
        !play.parallel,
    );

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

/// Outcome of evaluating one composite invocation: the argument map its
/// body runs under, or `None` when its condition ruled it out.
type Args = Result<Option<HashMap<String, DynValue>>, String>;

/// The plan phase's working state. Conditions and arguments are memoized
/// per path, so a container or a composite invocation is evaluated once
/// however many steps sit under it.
struct Planner<'a, 'd> {
    pb: &'a Playbook,
    store: &'a VarStore,
    step_blocks: &'a HashMap<(Vec<String>, String), Block<'d>>,
    container_blocks: &'a HashMap<Vec<String>, Block<'d>>,
    container_cache: HashMap<Vec<String>, Result<bool, String>>,
    /// Keyed by the invocation's full `container_path` (its own segment
    /// included), which is unique within a play.
    arg_cache: HashMap<Vec<String>, Args>,
}

impl Planner<'_, '_> {
    fn plan(&mut self, step: &Step) -> Plan {
        // Only *playbook* containers carry inheritable conditions; a
        // composite contributes path segments but no container blocks.
        for depth in 1..=step.playbook_path().len() {
            let path = step.playbook_path()[..depth].to_vec();
            let result = match self.container_cache.get(&path) {
                Some(r) => r.clone(),
                None => {
                    let r = match self.container_blocks.get(&path) {
                        Some(cb) => eval_condition(cb),
                        None => Err("container block not found".into()),
                    };
                    self.container_cache.insert(path, r.clone());
                    r
                }
            };
            match result {
                Ok(true) => {}
                Ok(false) => return Plan::Skip(Some("container condition is false".into())),
                Err(e) => return Plan::Fail(e),
            }
        }

        if step.frames.is_empty() {
            let key = (step.container_path.clone(), step.name.clone());
            let Some(block) = self.step_blocks.get(&key) else {
                return Plan::Fail("step block not found at run time".into());
            };
            return plan_from_block(self.pb, step, block);
        }
        self.plan_composite(step)
    }

    /// Plan a step expanded from a composite: walk the invocation chain to
    /// build the innermost argument scope, then evaluate the step's own
    /// block inside it.
    fn plan_composite(&mut self, step: &Step) -> Plan {
        let last = step.frames.len() - 1;
        let args = match self.args(step, last) {
            Ok(Some(a)) => a,
            Ok(None) => return Plan::Skip(Some("composite condition is false".into())),
            Err(e) => return Plan::Fail(e),
        };
        let frame = &step.frames[last];
        self.in_composite(frame, &args, |doc_block| {
            let Some(block) = composite_step_block(&doc_block, &step.name) else {
                return Plan::Fail(format!(
                    "step '{}' not found in composite '{}' at run time",
                    step.name, frame.composite
                ));
            };
            plan_from_block(self.pb, step, &block)
        })
    }

    /// The argument map for the invocation at frame `k` of `step`.
    fn args(&mut self, step: &Step, k: usize) -> Args {
        let cut = step.container_path.len() - (step.frames.len() - k) + 1;
        let key = step.container_path[..cut].to_vec();
        if let Some(cached) = self.arg_cache.get(&key) {
            return cached.clone();
        }
        let result = self.eval_args(step, k);
        self.arg_cache.insert(key, result.clone());
        result
    }

    fn eval_args(&mut self, step: &Step, k: usize) -> Args {
        let frame = &step.frames[k];
        let decl = self
            .pb
            .composite(&frame.package, &frame.composite)
            .ok_or_else(|| format!("composite '{}' declaration missing", frame.composite))?;
        let params = decl.params.clone();

        if k == 0 {
            // The outermost invocation is an ordinary step block in the
            // playbook document, already indexed.
            let scope = step.playbook_path().to_vec();
            let block = self
                .step_blocks
                .get(&(scope, frame.step.clone()))
                .ok_or_else(|| format!("step '{}' not found at run time", frame.step))?;
            return invocation_args(block, &params);
        }

        // Otherwise it was invoked from the body of the enclosing
        // composite, so its block lives in that composite's document.
        let outer = match self.args(step, k - 1)? {
            Some(a) => a,
            None => return Ok(None),
        };
        let outer_frame = &step.frames[k - 1];
        let step_name = frame.step.clone();
        let composite = outer_frame.composite.clone();
        self.in_composite(outer_frame, &outer, |doc_block| {
            let block = composite_step_block(&doc_block, &step_name).ok_or_else(|| {
                format!("step '{step_name}' not found in composite '{composite}'")
            })?;
            invocation_args(&block, &params)
        })
    }

    /// Open the document declaring `frame`'s composite with `args` bound
    /// and hand the composite's block to `f`. The document lives for the
    /// duration of the call, which is why this is a closure rather than a
    /// returned value.
    fn in_composite<T>(
        &self,
        frame: &CompositeFrame,
        args: &HashMap<String, DynValue>,
        f: impl FnOnce(Block<'_>) -> T,
    ) -> T
    where
        T: FromPlanError,
    {
        let Some((source, root)) = self.pb.composite_source(&frame.package) else {
            return T::from_error(format!("package '{}' not loaded", frame.package));
        };
        let (filename, import) = if frame.package.is_empty() {
            ("playbook.wcl".to_string(), crate::vocab::PLAYBOOK_IMPORT)
        } else {
            (
                format!("pkgs/{}/package.wcl", frame.package),
                crate::vocab::PACKAGE_IMPORT,
            )
        };
        let wcl_args: HashMap<String, Value> = args
            .iter()
            .map(|(k, v)| (k.clone(), crate::convert::dyn_to_wcl(v)))
            .collect();
        let doc = match self
            .store
            .open_composite(source, &filename, root, import, &wcl_args)
        {
            Ok(d) => d,
            Err(e) => return T::from_error(format!("reopening {filename}: {}", e.message)),
        };
        // Composites are declared inside the document's root block.
        let root = if frame.package.is_empty() {
            doc.block("playbook")
        } else {
            doc.block("package")
        };
        let found = root.and_then(|r| {
            r.blocks().find(|b| {
                b.kind() == "composite"
                    && crate::model::label_string(b).as_deref() == Some(frame.composite.as_str())
            })
        });
        let Some(block) = found else {
            return T::from_error(format!(
                "composite '{}' not found in {filename} at run time",
                frame.composite
            ));
        };
        f(block)
    }
}

/// Lets `in_composite` report a failure in whichever shape its caller
/// wants, without every call site repeating the match.
trait FromPlanError {
    fn from_error(message: String) -> Self;
}

impl FromPlanError for Plan {
    fn from_error(message: String) -> Plan {
        Plan::Fail(message)
    }
}

impl FromPlanError for Args {
    fn from_error(message: String) -> Args {
        Err(message)
    }
}

/// The `step` block of a composite body, by name. Composite bodies are
/// flat — no containers — so this is a direct child lookup.
fn composite_step_block<'a>(composite: &Block<'a>, name: &str) -> Option<Block<'a>> {
    composite
        .blocks()
        .find(|b| b.kind() == "step" && crate::model::label_string(b).as_deref() == Some(name))
}

/// Evaluate one composite invocation's condition and properties into the
/// argument map its body runs under.
fn invocation_args(block: &Block<'_>, params: &[ParamDecl]) -> Args {
    match eval_condition(block) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut args = read_properties(block)?;
    apply_param_defaults(&mut args, params).map_err(|errors| errors.join("; "))?;
    Ok(Some(args))
}

/// The final step of the chain: condition, properties, declared defaults.
fn plan_from_block(pb: &Playbook, step: &Step, block: &Block<'_>) -> Plan {
    match eval_condition(block) {
        Ok(true) => {}
        Ok(false) => return Plan::Skip(None),
        Err(e) => return Plan::Fail(e),
    }
    if step.kind == StepKind::Wait {
        return plan_wait(pb, step, block);
    }
    let Some(decl) = pb.resource(&step.package, &step.resource) else {
        return Plan::Fail("resource declaration missing".into());
    };
    let mut params = match read_properties(block) {
        Ok(p) => p,
        Err(e) => return Plan::Fail(e),
    };
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
        lock: decl.lock.clone().map(|l| (step.package.clone(), l)),
        params: DynValue::Map(params),
    }
}

/// A wait step's plan: the probe's properties with defaults applied, and
/// the polling policy evaluated in the step's scope (so `timeout` may come
/// from a variable or a composite argument).
fn plan_wait(pb: &Playbook, step: &Step, block: &Block<'_>) -> Plan {
    let Some(decl) = pb.probe(&step.package, &step.resource) else {
        return Plan::Fail("probe declaration missing".into());
    };
    let mut params = match read_properties(block) {
        Ok(p) => p,
        Err(e) => return Plan::Fail(e),
    };
    if let Err(errors) = apply_param_defaults(&mut params, &decl.params) {
        return Plan::Fail(errors.join("; "));
    }
    let mut fields: HashMap<&str, DynValue> = HashMap::new();
    for f in block.fields() {
        let name = f.name();
        let Some(key) = ["interval", "timeout", "fail_on_error"]
            .into_iter()
            .find(|k| *k == name)
        else {
            continue;
        };
        match field_value_dyn(&f) {
            Ok(fv) => {
                fields.insert(key, fv.value);
            }
            Err(FieldValueError::Convert(e)) => return Plan::Fail(format!("'{key}': {e}")),
            Err(FieldValueError::Unresolved(e) | FieldValueError::Eval(e)) => {
                return Plan::Fail(format!("'{key}': {e}"));
            }
        }
    }
    match wait::spec(
        fields.get("interval"),
        fields.get("timeout"),
        fields.get("fail_on_error"),
    ) {
        Ok(spec) => Plan::Wait {
            probe_key: step.target(),
            params: DynValue::Map(params),
            spec,
        },
        Err(e) => Plan::Fail(e),
    }
}

fn read_properties(block: &Block<'_>) -> Result<HashMap<String, DynValue>, String> {
    let mut params: HashMap<String, DynValue> = HashMap::new();
    let Some(props) = block.blocks().find(|b| b.kind() == "properties") else {
        return Ok(params);
    };
    for f in props.fields() {
        match field_value_dyn(&f) {
            Ok(fv) => {
                params.insert(f.name().to_string(), fv.value);
            }
            Err(FieldValueError::Convert(e)) => {
                return Err(format!("property '{}': {e}", f.name()));
            }
            Err(FieldValueError::Unresolved(e) | FieldValueError::Eval(e)) => {
                return Err(format!("property '{}': {e}", f.name()));
            }
        }
    }
    Ok(params)
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
    /// The pool worker that ran the step; `None` for a wait step, which
    /// runs on its own thread.
    worker: Option<usize>,
    status: StepStatus,
    message: Option<String>,
    duration: Duration,
}

#[allow(clippy::too_many_arguments)]
fn schedule(
    steps: &[&Step],
    plans: &[Plan],
    dag: &dag::StepDag,
    scripts: &ScriptSet,
    ctx: &Context,
    opts: &RunOptions,
    workers: usize,
    sequential: bool,
) -> Vec<StepReport> {
    let n = steps.len();
    let mut reports: Vec<Option<StepReport>> = (0..n).map(|_| None).collect();
    let mut dispatched = vec![false; n];
    let mut completed = vec![false; n];
    let mut halted = false;
    let mut in_flight = 0usize;
    let mut running_exclusive: HashSet<String> = HashSet::new();
    let mut held_locks: HashSet<(String, String)> = HashSet::new();
    let mut global_running = false;
    let cancel = Cancel::new();
    let mut waiter_handles = Vec::new();

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
                            worker: Some(worker_id),
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
    // Waiters each take a clone; the scheduler only ever receives while
    // something is in flight, so holding this one open cannot block it.
    let wait_done = done_tx;
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
                            cancel.cancel();
                        }
                        progressed = true;
                    }
                    Plan::Wait {
                        probe_key,
                        params,
                        spec,
                    } => {
                        if global_running || (sequential && in_flight > 0) {
                            continue;
                        }
                        dispatched[i] = true;
                        in_flight += 1;
                        let Some(probe) = scripts.probes.get(probe_key) else {
                            let _ = wait_done.send(Done {
                                idx: i,
                                worker: None,
                                status: StepStatus::Error,
                                message: Some(format!("no compiled probe '{probe_key}'")),
                                duration: Duration::ZERO,
                            });
                            progressed = true;
                            continue;
                        };
                        waiter_handles.push(spawn_waiter(
                            WaitJob {
                                idx: i,
                                probe_key: probe_key.clone(),
                                unit: probe.unit.clone(),
                                ready: probe.ready,
                                params: params.clone(),
                                spec: *spec,
                                step_label: steps[i].name.clone(),
                                mode: opts.mode,
                            },
                            ctx.clone(),
                            opts.events.clone(),
                            std::sync::Arc::clone(&cancel),
                            wait_done.clone(),
                        ));
                        progressed = true;
                    }
                    Plan::Run {
                        resource_key,
                        concurrency,
                        lock,
                        params,
                    } => {
                        // Concurrency gating. A sequential play has one worker,
                        // but a wait step runs off the pool, so it is counted
                        // here instead.
                        if global_running
                            || idle_workers.is_empty()
                            || (sequential && in_flight > 0)
                        {
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
                        if lock.as_ref().is_some_and(|l| held_locks.contains(l)) {
                            continue;
                        }
                        let Some(worker) = idle_workers.pop_front() else {
                            break;
                        };
                        dispatched[i] = true;
                        in_flight += 1;
                        if *concurrency == Concurrency::Exclusive {
                            running_exclusive.insert(resource_key.clone());
                        }
                        if let Some(l) = lock {
                            held_locks.insert(l.clone());
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
                tracing::warn!(
                    "scheduler: pending steps with no satisfiable dependencies; marking them not-run"
                );
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
        if let Some(w) = done.worker {
            idle_workers.push_back(w);
        }
        completed[done.idx] = true;
        if let Plan::Run {
            resource_key,
            concurrency,
            lock,
            ..
        } = &plans[done.idx]
        {
            if *concurrency == Concurrency::Exclusive {
                running_exclusive.remove(resource_key);
            }
            if let Some(l) = lock {
                held_locks.remove(l);
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
        if halted {
            cancel.cancel();
        }
        let report = step_report(steps[done.idx], done.status, done.message, done.duration);
        (opts.events)(Event::StepFinished {
            idx: done.idx,
            report: report.clone(),
        });
        reports[done.idx] = Some(report);
    }

    drop(job_txs);
    drop(wait_done);
    for handle in worker_handles {
        if handle.join().is_err() {
            tracing::error!("worker thread panicked");
        }
    }
    for handle in waiter_handles {
        if handle.join().is_err() {
            tracing::error!("wait thread panicked");
        }
    }

    reports
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.unwrap_or_else(|| {
                quick_report(
                    steps[i],
                    StepStatus::Error,
                    Some("internal scheduler error: step never reported".into()),
                )
            })
        })
        .collect()
}

fn quick_report(step: &Step, status: StepStatus, message: Option<String>) -> StepReport {
    step_report(step, status, message, Duration::ZERO)
}

/// The one place a `StepReport` is built, so a step message — which comes
/// straight from a resource script and may quote a parameter it was given
/// — is scrubbed of decrypted secrets before it reaches the event stream,
/// the report or the log.
/// Everything a wait step's thread needs, as owned data.
struct WaitJob {
    idx: usize,
    probe_key: String,
    unit: wscript::CompiledUnit,
    ready: EntryKind,
    params: DynValue,
    spec: WaitSpec,
    step_label: String,
    mode: Mode,
}

/// Run one wait step on a thread of its own, reporting through `done`.
fn spawn_waiter(
    job: WaitJob,
    ctx: Context,
    events: EventSink,
    cancel: std::sync::Arc<Cancel>,
    done: mpsc::Sender<Done>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _guard = crate::hostapi::worker_init();
        let started = Instant::now();
        events(Event::StepStarted {
            idx: job.idx,
            name: job.step_label.clone(),
        });
        crate::logging::install_step_sink(&job.step_label, &job.probe_key);
        events(Event::StepPhase {
            idx: job.idx,
            name: job.step_label.clone(),
            phase: Phase::Waiting,
        });
        let probe = CompiledProbe {
            unit: job.unit,
            ready: job.ready,
        };
        let on_attempt = |a: &wait::Attempt| {
            let error = a.error.as_deref().map(scrub);
            tracing::debug!(
                step = %job.step_label,
                probe = %job.probe_key,
                attempt = a.attempt,
                elapsed_secs = a.elapsed.as_secs_f64(),
                error = error.as_deref().unwrap_or(""),
                "probe not ready"
            );
            events(Event::StepWaiting {
                idx: job.idx,
                name: job.step_label.clone(),
                attempt: a.attempt,
                elapsed: a.elapsed,
                timeout: job.spec.timeout,
                error,
            });
        };
        let (status, message) = wait::run(
            || call_ready(&probe, &ctx, job.params.clone()),
            &job.spec,
            job.mode,
            &cancel,
            &on_attempt,
        );
        log::clear_sink();
        let _ = done.send(Done {
            idx: job.idx,
            worker: None,
            status,
            message,
            duration: started.elapsed(),
        });
    })
}

/// Scrub decrypted secrets from text bound for events or the log.
fn scrub(text: &str) -> String {
    if crate::secrets::redact::active() {
        crate::secrets::redact::scrub(text)
    } else {
        text.to_string()
    }
}

fn step_report(
    step: &Step,
    status: StepStatus,
    message: Option<String>,
    duration: Duration,
) -> StepReport {
    let message = match message {
        Some(m) if crate::secrets::redact::active() => Some(crate::secrets::redact::scrub(&m)),
        other => other,
    };
    StepReport {
        name: step.name.clone(),
        container_path: step.container_path.clone(),
        resource: step.target(),
        status,
        message,
        duration,
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
    let mut vm = crate::hostapi::vm(ctx);
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

fn call_ready(probe: &CompiledProbe, ctx: &Context, params: DynValue) -> Result<bool, String> {
    let mut vm = crate::hostapi::vm(ctx);
    match probe.ready {
        EntryKind::Plain => vm
            .call_unit(&probe.unit, "ready", (params,))
            .map_err(|e| e.to_string()),
        EntryKind::Fallible => vm
            .call_unit::<_, Result<bool, String>>(&probe.unit, "ready", (params,))
            .map_err(|e| e.to_string())
            .and_then(|r| r),
    }
}

fn call_apply(
    res: &CompiledResource,
    ctx: &Context,
    params: DynValue,
) -> Result<ApplyResult, String> {
    let mut vm = crate::hostapi::vm(ctx);
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
