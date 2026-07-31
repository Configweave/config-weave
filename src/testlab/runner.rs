//! The per-test orchestration: provision an instance, copy the binary
//! and a synthesized playbook in, drive the engine through three runs
//! (check, apply, apply), and evaluate expectations from the parsed
//! `--json` reports.
//!
//! Why three runs: the apply run already embeds the engine's internal
//! check→apply→re-check, proving convergence within one process. The
//! second apply proves **cross-process idempotence** — a fresh process's
//! check must recognize the applied state cold — and that re-apply is a
//! true no-op (a resource whose check wrongly reports `not_configured`
//! re-applies and surfaces as `configured`, failing the test).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use wscript::{UnitExt, Vm};

use crate::convert::dyn_to_json;
use crate::diag::Diag;
use crate::engine::status::StepStatus;
use crate::hostapi::testlab::{Lab, LabState, lab_value};
use crate::model::{Expect, Package, Playbook, ScenarioDecl, TestDecl, TestTarget};
use crate::report::JsonRunReport;

use super::events::{TestEvent, TestEventSink, TestPhase, tail_chunk};
use super::guest::{GatherOutcome, Guest, VerifyOutcome, Workdir};
use super::report::{TestGatherResult, TestOutcome, TestReport, TestStepResult, VerifyResult};
use super::synth;
use super::synth::BinaryResolver;
use super::vmlab::{VmlabBackend, VmlabInstance};

pub struct RunnerOptions {
    /// Resolves the static binary copied into instances, per guest OS.
    pub binaries: synth::BinaryResolver,
    /// Leave instances running for post-mortem debugging.
    pub keep: bool,
    /// Forwarded to the in-instance check/apply runs.
    pub jobs: Option<usize>,
    /// Receives every lifecycle event; the human progress renderer and
    /// the `--events-ndjson` emitter are both sinks (see testlab::events).
    pub sink: TestEventSink,
    /// Max container groups running at once.
    pub container_cap: usize,
    /// Max VM groups running at once — kept small, VMs are heavy.
    pub vm_cap: usize,
}

/// A path-safe per-test identifier: selection index plus the package and
/// test names with anything outside `[A-Za-z0-9._-]` collapsed to `_`.
fn test_slug(idx: usize, package: &str, test: &str) -> String {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("{idx}-{}__{}", sanitize(package), sanitize(test))
}

/// Expected status per run, by expect class. `None` = unasserted.
fn expectations(e: Expect) -> [Option<StepStatus>; 3] {
    use StepStatus::*;
    match e {
        Expect::Converge => [
            Some(NotConfigured),
            Some(Configured),
            Some(AlreadyConfigured),
        ],
        Expect::AlreadyConfigured => [Some(AlreadyConfigured); 3],
        Expect::Error => [None, Some(Error), None],
        Expect::Skip => [Some(Skipped); 3],
        Expect::RebootRequired => [None, Some(RebootRequired), None],
    }
}

const RUN_LABELS: [&str; 3] = ["check", "first apply", "second apply"];

/// One shared-instance unit of work: what to provision, and the ordered
/// tests that run sequentially inside that one instance. The `usize` in
/// each tuple is the test's index in the original selection — used to
/// restore output order after parallel execution.
pub struct GroupSpec<'a> {
    pub target: TestTarget,
    /// Guest RAM override; grouped tests are validated to agree on it.
    pub memory: Option<String>,
    pub tests: Vec<(usize, &'a Package, &'a TestDecl)>,
}

/// Run every group, with independent groups executing in parallel under
/// per-kind caps — containers and VMs throttled separately, since VMs cost
/// far more host resources. Returns one report per test, restored to the
/// original selection order.
pub fn run_groups(
    pb: &Playbook,
    groups: Vec<GroupSpec<'_>>,
    backend: &VmlabBackend,
    opts: &RunnerOptions,
) -> Vec<TestReport> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Bucket groups by machine kind so each cap throttles only its own kind
    // of instance; both buckets drain concurrently. The enumeration index is
    // the group id events refer to.
    let mut containers: Vec<(usize, &GroupSpec)> = Vec::new();
    let mut vms: Vec<(usize, &GroupSpec)> = Vec::new();
    for (gid, g) in groups.iter().enumerate() {
        match g.target {
            TestTarget::Container(_) => containers.push((gid, g)),
            TestTarget::Vm(_) => vms.push((gid, g)),
        }
    }

    // Cursors live for the whole scope; the per-bucket workers share them.
    let container_cursor = AtomicUsize::new(0);
    let vm_cursor = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, TestReport)>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        for (bucket, cap, cursor) in [
            (&containers, opts.container_cap, &container_cursor),
            (&vms, opts.vm_cap, &vm_cursor),
        ] {
            let workers = cap.max(1).min(bucket.len());
            for _ in 0..workers {
                let results = &results;
                s.spawn(move || {
                    loop {
                        let i = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some((gid, group)) = bucket.get(i) else {
                            break;
                        };
                        let reports = run_group(pb, *gid, group, backend, opts);
                        results.lock().unwrap().extend(reports);
                    }
                });
            }
        }
    });

    let mut out = results.into_inner().unwrap();
    out.sort_by_key(|(idx, _)| *idx);
    out.into_iter().map(|(_, r)| r).collect()
}

/// The instance kind a target provisions, as it appears in reports and
/// events.
fn machine_kind(target: &TestTarget) -> &'static str {
    match target {
        TestTarget::Container(_) => "container",
        TestTarget::Vm(_) => "vm",
    }
}

/// A short label for a group's progress/diagnostic lines.
fn group_label(group: &GroupSpec) -> String {
    match group.tests.first() {
        Some((_, _, t)) if t.group.is_some() => {
            format!("group {}", t.group.as_deref().unwrap_or_default())
        }
        Some((_, pkg, t)) => format!("{}:{}", pkg.name, t.name),
        None => "group".into(),
    }
}

/// Provision one instance, copy the binary in and smoke-test it once, then
/// drive each test sequentially against the shared instance. Provision or
/// smoke failure errors every test in the group; a single test's transport
/// trouble errors only that test and the rest of the group proceeds.
fn run_group(
    pb: &Playbook,
    gid: usize,
    group: &GroupSpec,
    backend: &VmlabBackend,
    opts: &RunnerOptions,
) -> Vec<(usize, TestReport)> {
    let target = &group.target;
    let kind = machine_kind(target);
    let source = target.reference().to_string();
    let label = group_label(group);

    // One report per test, defaulting to Passed.
    let mut reports: Vec<(usize, TestReport)> = group
        .tests
        .iter()
        .map(|(idx, pkg, test)| {
            (
                *idx,
                TestReport {
                    package: pkg.name.clone(),
                    name: test.name.clone(),
                    machine_kind: kind,
                    source: source.clone(),
                    outcome: TestOutcome::Passed,
                    steps: Vec::new(),
                    gathers: Vec::new(),
                    verify: None,
                    error: None,
                    kept: None,
                    duration: Duration::default(),
                },
            )
        })
        .collect();

    // A group-level failure errors every member test; the events keep the
    // GUI from waiting on tests that already died.
    let fail_all = |reports: &mut Vec<(usize, TestReport)>, d: &Diag| {
        for (_, r) in reports.iter_mut() {
            r.outcome = TestOutcome::Error;
            r.error = Some(d.message.clone());
            (opts.sink)(TestEvent::TestFinished {
                package: r.package.clone(),
                test: r.name.clone(),
                outcome: r.outcome.as_str(),
                duration_secs: 0.0,
                error: r.error.clone(),
            });
        }
    };

    (opts.sink)(TestEvent::GroupProvisioning {
        group: gid,
        label: label.clone(),
        machine_kind: kind,
        source: source.clone(),
    });
    let mut instance = match backend.provision(target, group.memory.as_deref(), opts.keep) {
        Ok(i) => i,
        Err(d) => {
            fail_all(&mut reports, &d);
            return reports;
        }
    };
    // Eagerly, once per group: a binary that will not run in this instance
    // errors every test in the group rather than each of them separately.
    if let Err(d) = Guest::new(&instance).prepare(&opts.binaries, &target.to_string()) {
        fail_all(&mut reports, &d);
        if !opts.keep {
            let _ = instance.teardown();
        }
        return reports;
    }
    (opts.sink)(TestEvent::InstanceReady {
        group: gid,
        label: label.clone(),
        machine_kind: kind,
        source: source.clone(),
        attach: instance.attach_info(),
    });

    let kept_handle = opts.keep.then(|| instance.handle());

    for (slot, (idx, pkg, test)) in group.tests.iter().enumerate() {
        let report = &mut reports[slot].1;
        report.kept = kept_handle.clone();
        (opts.sink)(TestEvent::TestStarted {
            package: pkg.name.clone(),
            test: test.name.clone(),
            group: Some(gid),
        });
        let ctx = TestCtx {
            sink: &opts.sink,
            package: &pkg.name,
            test: &test.name,
        };
        let t0 = Instant::now();
        let slug = test_slug(*idx, &pkg.name, &test.name);
        match synth::synthesize(pb, pkg, test) {
            Ok(synth) => match drive_one(test, &instance, opts, &ctx, &synth, &slug, report) {
                Ok(()) => {
                    if report.steps.iter().any(|s| !s.failures.is_empty())
                        || report.gathers.iter().any(|g| !g.failures.is_empty())
                        || report.verify.as_ref().is_some_and(|v| !v.passed)
                    {
                        report.outcome = TestOutcome::Failed;
                    }
                }
                Err(d) => {
                    report.outcome = TestOutcome::Error;
                    report.error = Some(d.message);
                }
            },
            Err(d) => {
                report.outcome = TestOutcome::Error;
                report.error = Some(d.message);
            }
        }
        report.duration = t0.elapsed();
        (opts.sink)(TestEvent::TestFinished {
            package: pkg.name.clone(),
            test: test.name.clone(),
            outcome: report.outcome.as_str(),
            duration_secs: report.duration.as_secs_f64(),
            error: report.error.clone(),
        });
    }

    let teardown_warning = if opts.keep {
        None
    } else {
        // Don't mask test results behind a teardown failure; surface it.
        instance.teardown().err().map(|d| d.message)
    };
    (opts.sink)(TestEvent::GroupTeardown {
        group: gid,
        label,
        kept: opts.keep,
        handle: kept_handle,
        warning: teardown_warning,
    });

    reports
}

/// Per-test event context threaded through the drive functions in place
/// of the old free-text progress callback.
struct TestCtx<'a> {
    sink: &'a TestEventSink,
    package: &'a str,
    test: &'a str,
}

impl TestCtx<'_> {
    fn phase(&self, phase: TestPhase) {
        (self.sink)(TestEvent::Phase {
            package: self.package.to_string(),
            test: self.test.to_string(),
            phase,
        });
    }

    /// Emit an exec's captured stderr as a log event (tail-truncated);
    /// empty output stays silent.
    fn log(&self, context: &str, output: &str) {
        if output.trim().is_empty() {
            return;
        }
        let (chunk, truncated) = tail_chunk(output);
        (self.sink)(TestEvent::Log {
            package: self.package.to_string(),
            test: self.test.to_string(),
            context: context.to_string(),
            stream: "stderr",
            chunk,
            truncated,
        });
    }
}

/// Everything that happens for one test inside the (already prepared)
/// shared instance: its own working dir, setup, the synthesized playbook,
/// gathers, the three-run protocol, and verify.
fn drive_one(
    test: &TestDecl,
    instance: &VmlabInstance,
    opts: &RunnerOptions,
    ctx: &TestCtx,
    synthesized: &synth::SynthesizedTest,
    slug: &str,
    report: &mut TestReport,
) -> Result<(), Diag> {
    // Its own working dir, created here — setup cd's into it below.
    let wd = Guest::new(instance).test_work(slug)?;

    if let Some(setup) = &test.setup {
        ctx.phase(TestPhase::Setup);
        let out = wd.exec_in(setup)?;
        ctx.log("setup", &out.stderr);
        if out.exit_code != 0 {
            return Err(Diag::bare(format!(
                "setup failed (exit {}): {}",
                out.exit_code,
                tail(&out.stderr)
            )));
        }
    }

    wd.stage(synthesized.dir.path())?;

    let facts = run_gathers(test, ctx, &mut report.gathers, &wd)?;
    run_steps(test, opts, ctx, &mut report.steps, &wd)?;
    report.verify = run_verify(test, ctx, &facts, &wd)?;
    Ok(())
}

/// Run every gather test through `__gather`, assert expectations, and
/// collect results into the facts map handed to verify().
fn run_gathers(
    test: &TestDecl,
    ctx: &TestCtx,
    results: &mut Vec<TestGatherResult>,
    wd: &Workdir,
) -> Result<serde_json::Map<String, serde_json::Value>, Diag> {
    let mut facts = serde_json::Map::new();
    for g in &test.gathers {
        ctx.phase(TestPhase::Gather {
            name: g.name.clone(),
        });
        let key = format!("{}.{}", g.package, g.gatherer);
        let params = (!g.params.is_empty()).then(|| {
            serde_json::Value::Object(
                g.params
                    .iter()
                    .map(|(k, v)| (k.clone(), dyn_to_json(v)))
                    .collect(),
            )
        });

        let mut failures = Vec::new();
        // A gatherer that refuses is a failing test, not a broken run —
        // the transport trouble that would be a broken run is the `?`.
        match wd.gather(&key, params.as_ref(), &g.name)? {
            GatherOutcome::Ok(value) => {
                for (k, want) in &g.expect {
                    let want = dyn_to_json(want);
                    match value.get(k) {
                        Some(got) if *got == want => {}
                        Some(got) => failures.push(format!(
                            "gather '{}': expected {k} = {want}, got {got}",
                            g.name
                        )),
                        None => failures.push(format!(
                            "gather '{}': expected {k} = {want}, but the value has no such key",
                            g.name
                        )),
                    }
                }
                facts.insert(g.name.clone(), value);
            }
            GatherOutcome::Refused(why) => {
                failures.push(format!("gather '{}' failed: {why}", g.name));
            }
        }
        (ctx.sink)(TestEvent::GatherResult {
            package: ctx.package.to_string(),
            test: ctx.test.to_string(),
            gather: g.name.clone(),
            failures: failures.clone(),
        });
        results.push(TestGatherResult {
            name: g.name.clone(),
            failures,
        });
    }
    Ok(facts)
}

/// The three engine runs and the expectation table.
fn run_steps(
    test: &TestDecl,
    opts: &RunnerOptions,
    ctx: &TestCtx,
    results: &mut Vec<TestStepResult>,
    wd: &Workdir,
) -> Result<(), Diag> {
    if test.steps.is_empty() {
        return Ok(());
    }

    const RUN_PHASES: [TestPhase; 3] = [
        TestPhase::Check,
        TestPhase::FirstApply,
        TestPhase::SecondApply,
    ];
    const RUN_IDS: [&str; 3] = ["check", "first_apply", "second_apply"];

    let mut reports: Vec<JsonRunReport> = Vec::with_capacity(3);
    for (i, mode) in ["check", "apply", "apply"].iter().enumerate() {
        ctx.phase(RUN_PHASES[i].clone());
        let out = wd.run(mode, Some(synth::PLAY), opts.jobs, RUN_LABELS[i])?;
        ctx.log(RUN_IDS[i], &out.stderr);
        reports.push(out.report);
    }

    for s in &test.steps {
        let by_run: Vec<Option<&crate::report::JsonRunStep>> = reports
            .iter()
            .map(|r| r.steps.iter().find(|js| js.name == s.name))
            .collect();
        let status_of = |i: usize| by_run[i].and_then(|js| StepStatus::from_id(&js.status));
        let mut failures = Vec::new();
        for (i, want) in expectations(s.expect).iter().enumerate() {
            let Some(want) = want else { continue };
            match status_of(i) {
                Some(got) if got == *want => {}
                Some(got) => {
                    let mut f = format!(
                        "step '{}': expected {} after {}, got {}",
                        s.name,
                        want.id(),
                        RUN_LABELS[i],
                        got.id()
                    );
                    if let Some(msg) = by_run[i].and_then(|js| js.message.as_deref()) {
                        f.push_str(&format!(" — {msg}"));
                    }
                    failures.push(f);
                }
                None => failures.push(format!(
                    "step '{}' is missing from the {} run's report",
                    s.name, RUN_LABELS[i]
                )),
            }
        }
        (ctx.sink)(TestEvent::StepResult {
            package: ctx.package.to_string(),
            test: ctx.test.to_string(),
            step: s.name.clone(),
            expect: s.expect.as_str(),
            check: status_of(0).map(|st| st.id()),
            apply: status_of(1).map(|st| st.id()),
            second_apply: status_of(2).map(|st| st.id()),
            failures: failures.clone(),
        });
        results.push(TestStepResult {
            name: s.name.clone(),
            expect: s.expect,
            check: status_of(0),
            apply: status_of(1),
            second_apply: status_of(2),
            failures,
        });
    }
    Ok(())
}

/// Run the verify script (if any) through `__verify`, feeding it the
/// gathered facts.
fn run_verify(
    test: &TestDecl,
    ctx: &TestCtx,
    facts: &serde_json::Map<String, serde_json::Value>,
    wd: &Workdir,
) -> Result<Option<VerifyResult>, Diag> {
    let Some(verify) = &test.verify else {
        return Ok(None);
    };
    ctx.phase(TestPhase::Verify);

    // A script that broke is the `?` — only its verdict lands here.
    let (passed, message) = match wd.verify(verify, facts)? {
        VerifyOutcome::Passed => (true, None),
        VerifyOutcome::Failed(why) => (false, Some(why)),
    };
    (ctx.sink)(TestEvent::VerifyResult {
        package: ctx.package.to_string(),
        test: ctx.test.to_string(),
        passed,
        message: message.clone(),
    });
    Ok(Some(VerifyResult { passed, message }))
}

/// First interesting line(s) of command output for diagnostics.
fn tail(s: &str) -> String {
    super::output::output_tail(s, "(no output)")
}

// ------------------------------------------------------------- scenarios

/// One scenario to run: its package and declaration. Scenarios always
/// drive a declared vmlab lab of VMs.
pub struct ScenarioUnit<'a> {
    pub package: &'a Package,
    pub scenario: &'a ScenarioDecl,
}

/// How a scenario's `run` ended.
enum ScenarioEnd {
    /// `run` returned `false`, or returned `Err(msg)` (a failed assertion).
    Failed(String),
    /// Environmental: provisioning, compile, or transport trouble.
    Error(String),
}

/// Run each scenario sequentially (each may bring up several machines, so
/// they are not parallelized). Returns one `TestReport` per scenario,
/// reusing the test report shape for uniform formatting.
#[allow(clippy::too_many_arguments)]
pub fn run_scenarios(
    pb: &Rc<Playbook>,
    scenarios: Vec<ScenarioUnit<'_>>,
    backend: &VmlabBackend,
    bin_linux: Option<std::path::PathBuf>,
    bin_windows: Option<std::path::PathBuf>,
    keep: bool,
    quiet: bool,
    sink: &TestEventSink,
) -> Vec<TestReport> {
    scenarios
        .into_iter()
        .map(|u| {
            if !quiet {
                eprintln!(
                    "⟳ scenario {}:{} — {}",
                    u.package.name, u.scenario.name, u.scenario.description
                );
            }
            sink(TestEvent::TestStarted {
                package: u.package.name.clone(),
                test: u.scenario.name.clone(),
                group: None,
            });
            let report = run_one_scenario(
                pb,
                &u,
                backend,
                bin_linux.clone(),
                bin_windows.clone(),
                keep,
                quiet,
            );
            sink(TestEvent::TestFinished {
                package: report.package.clone(),
                test: report.name.clone(),
                outcome: report.outcome.as_str(),
                duration_secs: report.duration.as_secs_f64(),
                error: report.error.clone(),
            });
            report
        })
        .collect()
}

fn run_one_scenario(
    pb: &Rc<Playbook>,
    u: &ScenarioUnit<'_>,
    backend: &VmlabBackend,
    bin_linux: Option<std::path::PathBuf>,
    bin_windows: Option<std::path::PathBuf>,
    keep: bool,
    quiet: bool,
) -> TestReport {
    let t0 = Instant::now();
    let mut report = TestReport {
        package: u.package.name.clone(),
        name: u.scenario.name.clone(),
        machine_kind: "vm",
        source: "(scenario)".to_string(),
        outcome: TestOutcome::Passed,
        steps: Vec::new(),
        gathers: Vec::new(),
        verify: None,
        error: None,
        kept: None,
        duration: Duration::default(),
    };

    let lab = match backend.open_lab(&u.scenario.lab, keep) {
        Ok(l) => l,
        Err(d) => {
            report.outcome = TestOutcome::Error;
            report.error = Some(d.message);
            report.duration = t0.elapsed();
            return report;
        }
    };
    let state = LabState::new(
        lab,
        pb.clone(),
        u.package.dir.clone(),
        BinaryResolver::new(bin_linux, bin_windows),
        quiet,
    );
    let rc = Rc::new(RefCell::new(state));

    let roots = vec![u.package.dir.join("lib"), pb.root.join("lib")];
    match drive_scenario(&rc, &u.scenario.script, roots) {
        Ok(true) => {}
        Ok(false) => {
            report.outcome = TestOutcome::Failed;
            report.error = Some("scenario run() returned false".to_string());
        }
        Err(ScenarioEnd::Failed(msg)) => {
            report.outcome = TestOutcome::Failed;
            report.error = Some(msg);
        }
        Err(ScenarioEnd::Error(msg)) => {
            report.outcome = TestOutcome::Error;
            report.error = Some(msg);
        }
    }

    if keep {
        report.kept = Some(rc.borrow().handle());
    } else if let Err(d) = rc.borrow_mut().teardown()
        && !quiet
    {
        eprintln!("⚠ scenario {} teardown: {}", report.name, d.message);
    }
    report.duration = t0.elapsed();
    report
}

/// Compile and run a scenario's driver script against a live lab.
fn drive_scenario(
    rc: &Rc<RefCell<LabState>>,
    script: &Path,
    roots: Vec<std::path::PathBuf>,
) -> Result<bool, ScenarioEnd> {
    let source = std::fs::read_to_string(script)
        .map_err(|e| ScenarioEnd::Error(format!("cannot read {}: {e}", script.display())))?;
    let ctx = crate::hostapi::scenario_context();
    // Same `lib/` roots stage-5 validation compiled this driver with, so a
    // scenario that imports a helper behaves identically here.
    let resolver = crate::engine::scripts::WeaveResolver::new(roots);
    let unit = ctx
        .compile_entry(&script.display().to_string(), &source, &resolver)
        .map_err(|f| {
            let msgs: Vec<String> = f.diags.iter().map(|d| d.message.clone()).collect();
            ScenarioEnd::Error(format!("{}: {}", script.display(), msgs.join("; ")))
        })?
        .unit;
    let mut vm = Vm::new(&ctx);

    // Contract is validated in stage 5; dispatch on which signature compiled.
    if unit.fn_handle::<(Lab,), bool>("run").is_ok() {
        return vm
            .call_unit::<(Lab,), bool>(&unit, "run", (lab_value(rc.clone()),))
            .map_err(|e| ScenarioEnd::Error(wscript_err(e)));
    }
    if unit
        .fn_handle::<(Lab,), Result<bool, String>>("run")
        .is_ok()
    {
        return match vm.call_unit::<(Lab,), Result<bool, String>>(
            &unit,
            "run",
            (lab_value(rc.clone()),),
        ) {
            Ok(Ok(b)) => Ok(b),
            Ok(Err(msg)) => Err(ScenarioEnd::Failed(msg)),
            Err(e) => Err(ScenarioEnd::Error(wscript_err(e))),
        };
    }
    Err(ScenarioEnd::Error(
        "scenario must define `fn run(lab: Lab) -> bool`".to_string(),
    ))
}

/// Render a wscript error (compile or runtime) into a one-line message.
fn wscript_err(e: wscript::Error) -> String {
    match e {
        wscript::Error::Compile(ds) => ds
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("; "),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expectation_table_matches_the_design() {
        use StepStatus::*;
        assert_eq!(
            expectations(Expect::Converge),
            [
                Some(NotConfigured),
                Some(Configured),
                Some(AlreadyConfigured)
            ]
        );
        assert_eq!(expectations(Expect::Error), [None, Some(Error), None]);
        assert_eq!(expectations(Expect::Skip), [Some(Skipped); 3]);
    }

    // The guest path scheme and the verify-script mapping moved to
    // `testlab::guest`, and are tested there against a fake transport —
    // which covers the Windows branches this file never could.

    #[test]
    fn test_slug_is_path_safe_and_unique_per_index() {
        assert_eq!(
            test_slug(2, "my pkg", "weird/name!"),
            "2-my_pkg__weird_name_"
        );
        // The index disambiguates even when sanitized names collide.
        assert_ne!(test_slug(0, "p", "a/b"), test_slug(1, "p", "a/b"));
    }
}
