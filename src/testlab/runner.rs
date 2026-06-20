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
use std::io::Write as _;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use wisp::{UnitExt, Vm};

use crate::convert::dyn_to_json;
use crate::diag::Diag;
use crate::engine::status::StepStatus;
use crate::hostapi::testlab::{Lab, LabState, lab_value};
use crate::model::{Expect, Package, Playbook, ScenarioDecl, TestDecl};
use crate::report::JsonRunReport;

use super::backend::{GuestOs, TestBackend, TestInstance};
use super::report::{TestGatherResult, TestOutcome, TestReport, TestStepResult, VerifyResult};
use super::synth;
use super::synth::BinaryResolver;

pub struct RunnerOptions {
    /// Resolves the static binary copied into instances, per guest OS.
    pub binaries: synth::BinaryResolver,
    /// Leave instances running for post-mortem debugging.
    pub keep: bool,
    /// Forwarded to the in-instance check/apply runs.
    pub jobs: Option<usize>,
    /// Suppress stderr progress lines (JSON output mode).
    pub quiet: bool,
    /// Max docker groups (containers) running at once.
    pub docker_cap: usize,
    /// Max vmlab groups (VMs) running at once — kept small, VMs are heavy.
    pub vmlab_cap: usize,
}

/// The in-instance locations everything runs from, per guest OS. Forward
/// slashes throughout — Windows APIs accept them everywhere these paths
/// go (only shell command text needs backslashes, and that is the
/// instance's concern).
///
/// The binary is copied once per group to the shared `bin` path; each
/// test gets its own working `dir` (and the playbook/facts under it) so
/// grouped tests sharing one instance never clobber each other's files.
pub struct GuestPaths {
    pub bin: &'static str,
    pub playbook: String,
    pub facts: String,
    /// The per-test working directory holding the playbook and facts.
    pub dir: String,
}

impl GuestPaths {
    /// The shared binary path for `os` — copied once per group.
    pub fn bin_for(os: GuestOs) -> &'static str {
        match os {
            GuestOs::Linux => "/weave/config-weave",
            GuestOs::Windows => "C:/weave/config-weave.exe",
        }
    }

    /// Per-test paths under a working dir named by `slug` (a path-safe
    /// per-test identifier).
    pub fn for_test(os: GuestOs, slug: &str) -> GuestPaths {
        let root = match os {
            GuestOs::Linux => "/weave/t",
            GuestOs::Windows => "C:/weave/t",
        };
        let dir = format!("{root}/{slug}");
        GuestPaths {
            bin: GuestPaths::bin_for(os),
            playbook: format!("{dir}/playbook"),
            facts: format!("{dir}/facts.json"),
            dir,
        }
    }
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

/// One shared-instance unit of work: a backend, the image to provision,
/// and the ordered tests that run sequentially inside that one instance.
/// The `usize` in each tuple is the test's index in the original
/// selection — used to restore output order after parallel execution.
pub struct GroupSpec<'a> {
    pub backend: &'a dyn TestBackend,
    pub image: String,
    pub tests: Vec<(usize, &'a Package, &'a TestDecl)>,
}

/// Run every group, with independent groups executing in parallel under
/// per-backend caps — containers and VMs throttled separately, since VMs
/// cost far more host resources. Returns one report per test, restored to
/// the original selection order.
pub fn run_groups(
    pb: &Playbook,
    groups: Vec<GroupSpec<'_>>,
    opts: &RunnerOptions,
) -> Vec<TestReport> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Bucket groups by backend so each cap throttles only its own kind of
    // instance; both buckets drain concurrently.
    let mut docker: Vec<&GroupSpec> = Vec::new();
    let mut vmlab: Vec<&GroupSpec> = Vec::new();
    for g in &groups {
        match g.backend.name() {
            "vmlab" => vmlab.push(g),
            _ => docker.push(g),
        }
    }

    // Cursors live for the whole scope; the per-bucket workers share them.
    let docker_cursor = AtomicUsize::new(0);
    let vmlab_cursor = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, TestReport)>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        for (bucket, cap, cursor) in [
            (&docker, opts.docker_cap, &docker_cursor),
            (&vmlab, opts.vmlab_cap, &vmlab_cursor),
        ] {
            let workers = cap.max(1).min(bucket.len());
            for _ in 0..workers {
                let results = &results;
                s.spawn(move || {
                    loop {
                        let i = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(group) = bucket.get(i) else { break };
                        let reports = run_group(pb, group, opts);
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
fn run_group(pb: &Playbook, group: &GroupSpec, opts: &RunnerOptions) -> Vec<(usize, TestReport)> {
    let backend = group.backend;
    let image = group.image.clone();

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
                    backend: backend.name().to_string(),
                    image: image.clone(),
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

    let group_progress = |msg: &str| {
        if !opts.quiet {
            eprintln!("⟳ [{}] {msg}", group_label(group));
        }
    };
    let fail_all = |reports: &mut Vec<(usize, TestReport)>, d: &Diag| {
        for (_, r) in reports.iter_mut() {
            r.outcome = TestOutcome::Error;
            r.error = Some(d.message.clone());
        }
    };

    group_progress(&format!("provisioning ({image})"));
    let mut instance = match backend.provision(&image, opts.keep) {
        Ok(i) => i,
        Err(d) => {
            fail_all(&mut reports, &d);
            return reports;
        }
    };
    if let Err(d) = prepare_instance(instance.as_mut(), opts, &image) {
        fail_all(&mut reports, &d);
        if !opts.keep {
            let _ = instance.teardown();
        }
        return reports;
    }

    let kept_handle = opts.keep.then(|| instance.handle());

    for (slot, (idx, pkg, test)) in group.tests.iter().enumerate() {
        let report = &mut reports[slot].1;
        report.kept = kept_handle.clone();
        let progress = |msg: &str| {
            if !opts.quiet {
                eprintln!("⟳ {}:{} — {msg}", pkg.name, test.name);
            }
        };
        let t0 = Instant::now();
        let slug = test_slug(*idx, &pkg.name, &test.name);
        match synth::synthesize(pb, pkg, test) {
            Ok(synth) => {
                match drive_one(
                    test,
                    instance.as_mut(),
                    opts,
                    &progress,
                    &synth,
                    &slug,
                    report,
                ) {
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
                }
            }
            Err(d) => {
                report.outcome = TestOutcome::Error;
                report.error = Some(d.message);
            }
        }
        report.duration = t0.elapsed();
    }

    if opts.keep {
        group_progress(&format!(
            "kept {} — remove it manually when done",
            instance.handle()
        ));
    } else if let Err(d) = instance.teardown() {
        // Don't mask test results behind a teardown failure; surface it.
        if !opts.quiet {
            eprintln!("⚠ [{}] teardown: {}", group_label(group), d.message);
        }
    }

    reports
}

/// Copy the binary into the shared bin path and smoke-test it. Done once
/// per group, before any test runs.
fn prepare_instance(
    instance: &mut dyn TestInstance,
    opts: &RunnerOptions,
    image: &str,
) -> Result<(), Diag> {
    let os = instance.os();
    let bin = GuestPaths::bin_for(os);
    let binary = opts.binaries.resolve(os)?;

    instance.copy_in(&binary, bin)?;
    // docker cp preserves the executable bit; chmod defensively for
    // backends/umasks that do not. Best-effort: images without chmod
    // surface at the smoke test below. Windows has no execute bit.
    if os == GuestOs::Linux {
        let _ = instance.exec(&["chmod", "+x", bin]);
    }
    let smoke = instance.exec(&[bin, "version"])?;
    if smoke.exit_code != 0 {
        return Err(Diag::bare(format!(
            "the test binary failed to run inside '{image}' (exit {}): {} — host/image \
             architecture mismatch?",
            smoke.exit_code,
            tail(&smoke.stderr)
        )));
    }
    Ok(())
}

/// Everything that happens for one test inside the (already prepared)
/// shared instance: its own working dir, setup, the synthesized playbook,
/// gathers, the three-run protocol, and verify.
fn drive_one(
    test: &TestDecl,
    instance: &mut dyn TestInstance,
    opts: &RunnerOptions,
    progress: &dyn Fn(&str),
    synthesized: &synth::SynthesizedTest,
    slug: &str,
    report: &mut TestReport,
) -> Result<(), Diag> {
    let os = instance.os();
    let paths = GuestPaths::for_test(os, slug);

    // The per-test working dir must exist before setup cd's into it.
    let mkdir = match os {
        GuestOs::Linux => instance.exec(&["mkdir", "-p", &paths.dir])?,
        GuestOs::Windows => {
            let win = paths.dir.replace('/', "\\");
            let script = format!("if not exist {win} md {win}");
            instance.exec(&["cmd.exe", "/C", &script])?
        }
    };
    if mkdir.exit_code != 0 {
        return Err(Diag::bare(format!(
            "cannot create the per-test working dir {} (exit {}): {}",
            paths.dir,
            mkdir.exit_code,
            tail(&mkdir.stderr)
        )));
    }

    if let Some(setup) = &test.setup {
        progress("setup");
        let script;
        let argv: [&str; 3] = match os {
            // The exec working directory is backend-dependent; pin it.
            GuestOs::Linux => {
                script = format!("cd {} || exit 1\n{setup}", paths.dir);
                ["sh", "-c", &script]
            }
            GuestOs::Windows => {
                script = format!("cd /d {} && {setup}", paths.dir.replace('/', "\\"));
                ["cmd.exe", "/C", &script]
            }
        };
        let out = instance.exec(&argv)?;
        if out.exit_code != 0 {
            return Err(Diag::bare(format!(
                "setup failed (exit {}): {}",
                out.exit_code,
                tail(&out.stderr)
            )));
        }
    }

    instance.copy_in(synthesized.dir.path(), &paths.playbook)?;

    let facts = run_gathers(test, instance, progress, &mut report.gathers, &paths)?;
    run_steps(test, instance, opts, progress, &mut report.steps, &paths)?;
    report.verify = run_verify(test, instance, progress, &facts, &paths)?;
    Ok(())
}

/// Run every gather test through `__gather`, assert expectations, and
/// collect results into the facts map handed to verify().
fn run_gathers(
    test: &TestDecl,
    instance: &mut dyn TestInstance,
    progress: &dyn Fn(&str),
    results: &mut Vec<TestGatherResult>,
    paths: &GuestPaths,
) -> Result<serde_json::Map<String, serde_json::Value>, Diag> {
    let mut facts = serde_json::Map::new();
    for g in &test.gathers {
        progress(&format!("gather {}", g.name));
        let key = format!("{}.{}", g.package, g.gatherer);
        let mut argv = vec![paths.bin, "__gather", paths.playbook.as_str(), &key];
        let params_json;
        if !g.params.is_empty() {
            let map: serde_json::Map<String, serde_json::Value> = g
                .params
                .iter()
                .map(|(k, v)| (k.clone(), dyn_to_json(v)))
                .collect();
            params_json = serde_json::Value::Object(map).to_string();
            argv.extend(["--params-json", &params_json]);
        }
        let out = instance.exec(&argv)?;
        let parsed: serde_json::Value = serde_json::from_str(out.stdout.trim()).map_err(|_| {
            Diag::bare(format!(
                "gather '{}' produced no parseable protocol output (exit {}): {}",
                g.name,
                out.exit_code,
                tail(if out.stderr.is_empty() {
                    &out.stdout
                } else {
                    &out.stderr
                })
            ))
        })?;

        let mut failures = Vec::new();
        if parsed["ok"] == serde_json::Value::Bool(true) {
            let value = &parsed["value"];
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
            facts.insert(g.name.clone(), value.clone());
        } else {
            failures.push(format!(
                "gather '{}' failed: {}",
                g.name,
                parsed["error"].as_str().unwrap_or("(no error message)")
            ));
        }
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
    instance: &mut dyn TestInstance,
    opts: &RunnerOptions,
    progress: &dyn Fn(&str),
    results: &mut Vec<TestStepResult>,
    paths: &GuestPaths,
) -> Result<(), Diag> {
    if test.steps.is_empty() {
        return Ok(());
    }

    let jobs = opts.jobs.map(|j| j.to_string());
    let mut reports: Vec<JsonRunReport> = Vec::with_capacity(3);
    for (i, mode) in ["check", "apply", "apply"].iter().enumerate() {
        progress(RUN_LABELS[i]);
        // Always --continue-on-error so every step reports and the
        // expectation table stays total; dependents of errored steps
        // still come back `not_run` per the engine's requires semantics.
        let mut argv = vec![
            paths.bin,
            mode,
            paths.playbook.as_str(),
            synth::PLAY,
            "--json",
            "--continue-on-error",
        ];
        if let Some(j) = &jobs {
            argv.extend(["--jobs", j]);
        }
        let out = instance.exec(&argv)?;
        let parsed: JsonRunReport = serde_json::from_str(out.stdout.trim()).map_err(|_| {
            Diag::bare(format!(
                "the {} run produced no parseable report (exit {}): {}",
                RUN_LABELS[i],
                out.exit_code,
                tail(if out.stderr.is_empty() {
                    &out.stdout
                } else {
                    &out.stderr
                })
            ))
        })?;
        reports.push(parsed);
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
    instance: &mut dyn TestInstance,
    progress: &dyn Fn(&str),
    facts: &serde_json::Map<String, serde_json::Value>,
    paths: &GuestPaths,
) -> Result<Option<VerifyResult>, Diag> {
    let Some(verify) = &test.verify else {
        return Ok(None);
    };
    progress("verify");

    let mut facts_file = tempfile::NamedTempFile::new()
        .map_err(|e| Diag::bare(format!("cannot create the facts temp file: {e}")))?;
    facts_file
        .write_all(
            serde_json::Value::Object(facts.clone())
                .to_string()
                .as_bytes(),
        )
        .map_err(|e| Diag::bare(format!("cannot write the facts temp file: {e}")))?;
    instance.copy_in(facts_file.path(), &paths.facts)?;

    let script = in_container_script(verify, paths)?;
    let out = instance.exec(&[
        paths.bin,
        "__verify",
        &script,
        "--facts",
        paths.facts.as_str(),
    ])?;
    match out.exit_code {
        0 => Ok(Some(VerifyResult {
            passed: true,
            message: None,
        })),
        1 => Ok(Some(VerifyResult {
            passed: false,
            message: Some(tail(&out.stdout)),
        })),
        code => Err(Diag::bare(format!(
            "the verify script broke inside the container (exit {code}): {}",
            tail(if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            })
        ))),
    }
}

/// Map a host verify path (absolute, under some pkgs/<name>/) to its
/// location inside the synthesized playbook copy.
fn in_container_script(verify: &Path, paths: &GuestPaths) -> Result<String, Diag> {
    // …/pkgs/<pkg>/<rel> — find the pkgs component from the right.
    let comps: Vec<&str> = verify
        .iter()
        .map(|c| c.to_str().unwrap_or_default())
        .collect();
    let idx = comps.iter().rposition(|c| *c == "pkgs").ok_or_else(|| {
        Diag::bare(format!(
            "verify script {} is not under pkgs/",
            verify.display()
        ))
    })?;
    Ok(format!(
        "{}/pkgs/{}",
        paths.playbook,
        comps[idx + 1..].join("/")
    ))
}

/// First interesting line(s) of command output for diagnostics.
fn tail(s: &str) -> String {
    super::output::output_tail(s, "(no output)")
}

// ------------------------------------------------------------- scenarios

/// One scenario to run: its package, declaration, and resolved backend.
pub struct ScenarioUnit<'a> {
    pub package: &'a Package,
    pub scenario: &'a ScenarioDecl,
    pub backend: &'a dyn TestBackend,
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
pub fn run_scenarios(
    pb: &Rc<Playbook>,
    scenarios: Vec<ScenarioUnit<'_>>,
    bin_linux: Option<std::path::PathBuf>,
    bin_windows: Option<std::path::PathBuf>,
    keep: bool,
    quiet: bool,
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
            run_one_scenario(pb, &u, bin_linux.clone(), bin_windows.clone(), keep, quiet)
        })
        .collect()
}

fn run_one_scenario(
    pb: &Rc<Playbook>,
    u: &ScenarioUnit<'_>,
    bin_linux: Option<std::path::PathBuf>,
    bin_windows: Option<std::path::PathBuf>,
    keep: bool,
    quiet: bool,
) -> TestReport {
    let t0 = Instant::now();
    let mut report = TestReport {
        package: u.package.name.clone(),
        name: u.scenario.name.clone(),
        backend: u.backend.name().to_string(),
        image: "(scenario)".to_string(),
        outcome: TestOutcome::Passed,
        steps: Vec::new(),
        gathers: Vec::new(),
        verify: None,
        error: None,
        kept: None,
        duration: Duration::default(),
    };

    let lab = match u.backend.open_lab(&u.scenario.lab, keep) {
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

    match drive_scenario(&rc, &u.scenario.script) {
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
    } else if let Err(d) = rc.borrow_mut().teardown() {
        if !quiet {
            eprintln!("⚠ scenario {} teardown: {}", report.name, d.message);
        }
    }
    report.duration = t0.elapsed();
    report
}

/// Compile and run a scenario's driver script against a live lab.
fn drive_scenario(rc: &Rc<RefCell<LabState>>, script: &Path) -> Result<bool, ScenarioEnd> {
    let source = std::fs::read_to_string(script)
        .map_err(|e| ScenarioEnd::Error(format!("cannot read {}: {e}", script.display())))?;
    let ctx = crate::hostapi::scenario_context();
    let unit = ctx
        .compile(&source)
        .map_err(|e| ScenarioEnd::Error(format!("{}: {}", script.display(), wisp_err(e))))?;
    let mut vm = Vm::new(&ctx);

    // Contract is validated in stage 5; dispatch on which signature compiled.
    if unit.fn_handle::<(Lab,), bool>("run").is_ok() {
        return vm
            .call_unit::<(Lab,), bool>(&unit, "run", (lab_value(rc.clone()),))
            .map_err(|e| ScenarioEnd::Error(wisp_err(e)));
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
            Err(e) => Err(ScenarioEnd::Error(wisp_err(e))),
        };
    }
    Err(ScenarioEnd::Error(
        "scenario must define `fn run(lab: Lab) -> bool`".to_string(),
    ))
}

/// Render a wisp error (compile or runtime) into a one-line message.
fn wisp_err(e: wisp::Error) -> String {
    match e {
        wisp::Error::Compile(ds) => ds
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

    #[test]
    fn verify_path_maps_into_the_container() {
        let p = Path::new("/host/playbook/pkgs/core/tests/verify.wisp");
        let linux = GuestPaths::for_test(GuestOs::Linux, "0-core__t");
        assert_eq!(
            in_container_script(p, &linux).unwrap(),
            "/weave/t/0-core__t/playbook/pkgs/core/tests/verify.wisp"
        );
        let windows = GuestPaths::for_test(GuestOs::Windows, "0-core__t");
        assert_eq!(
            in_container_script(p, &windows).unwrap(),
            "C:/weave/t/0-core__t/playbook/pkgs/core/tests/verify.wisp"
        );
        assert!(in_container_script(Path::new("/elsewhere/verify.wisp"), &linux).is_err());
    }

    #[test]
    fn guest_paths_are_per_test_under_a_shared_bin() {
        // The binary path is shared (copied once per group); playbook and
        // facts live under a distinct per-test working dir.
        assert_eq!(GuestPaths::bin_for(GuestOs::Linux), "/weave/config-weave");
        assert_eq!(
            GuestPaths::bin_for(GuestOs::Windows),
            "C:/weave/config-weave.exe"
        );

        let a = GuestPaths::for_test(GuestOs::Linux, "0-core__a");
        let b = GuestPaths::for_test(GuestOs::Linux, "1-core__b");
        assert_eq!(a.bin, b.bin, "the binary is shared across grouped tests");
        assert_ne!(a.dir, b.dir, "each test gets its own working dir");
        assert_eq!(a.dir, "/weave/t/0-core__a");
        assert_eq!(a.playbook, "/weave/t/0-core__a/playbook");
        assert_eq!(a.facts, "/weave/t/0-core__a/facts.json");

        let w = GuestPaths::for_test(GuestOs::Windows, "0-core__a");
        assert_eq!(w.dir, "C:/weave/t/0-core__a");
    }

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
