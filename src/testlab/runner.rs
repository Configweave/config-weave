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

use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

use crate::convert::dyn_to_json;
use crate::diag::Diag;
use crate::engine::status::StepStatus;
use crate::model::{Expect, Package, Playbook, TestDecl};
use crate::report::JsonRunReport;

use super::backend::{GuestOs, TestBackend, TestInstance};
use super::report::{TestGatherResult, TestOutcome, TestReport, TestStepResult, VerifyResult};
use super::synth;

pub struct RunnerOptions {
    /// Resolves the static binary copied into instances, per guest OS.
    pub binaries: synth::BinaryResolver,
    /// Leave instances running for post-mortem debugging.
    pub keep: bool,
    /// Forwarded to the in-instance check/apply runs.
    pub jobs: Option<usize>,
    /// Suppress stderr progress lines (JSON output mode).
    pub quiet: bool,
    /// `--image`: run every test against this image instead of its own
    /// (matrix runs across distros).
    pub image_override: Option<String>,
}

/// The in-instance locations everything runs from, per guest OS. Forward
/// slashes throughout — Windows APIs accept them everywhere these paths
/// go (only shell command text needs backslashes, and that is the
/// instance's concern).
pub struct GuestPaths {
    pub bin: &'static str,
    pub playbook: &'static str,
    pub facts: &'static str,
    /// The directory holding all of the above.
    pub dir: &'static str,
}

impl GuestPaths {
    pub fn for_os(os: GuestOs) -> &'static GuestPaths {
        match os {
            GuestOs::Linux => &GuestPaths {
                bin: "/weave/config-weave",
                playbook: "/weave/playbook",
                facts: "/weave/facts.json",
                dir: "/weave",
            },
            GuestOs::Windows => &GuestPaths {
                bin: "C:/weave/config-weave.exe",
                playbook: "C:/weave/playbook",
                facts: "C:/weave/facts.json",
                dir: "C:/weave",
            },
        }
    }
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

/// Run one test to a report; environmental trouble becomes an `Error`
/// outcome rather than aborting the whole `config-weave test` run.
pub fn run_test(
    pb: &Playbook,
    pkg: &Package,
    test: &TestDecl,
    backend: &dyn TestBackend,
    opts: &RunnerOptions,
) -> TestReport {
    let start = Instant::now();
    let progress = |msg: &str| {
        if !opts.quiet {
            eprintln!("⟳ {}:{} — {msg}", pkg.name, test.name);
        }
    };
    let mut report = TestReport {
        package: pkg.name.clone(),
        name: test.name.clone(),
        // The backend actually driving the test (--backend can override
        // the declared one).
        backend: backend.name().to_string(),
        image: opts
            .image_override
            .clone()
            .unwrap_or_else(|| test.image.clone()),
        outcome: TestOutcome::Passed,
        steps: Vec::new(),
        gathers: Vec::new(),
        verify: None,
        error: None,
        kept: None,
        duration: start.elapsed(),
    };
    if let Err(d) = run_test_inner(pb, pkg, test, backend, opts, &progress, &mut report) {
        report.outcome = TestOutcome::Error;
        report.error = Some(d.message);
    } else if report.steps.iter().any(|s| !s.failures.is_empty())
        || report.gathers.iter().any(|g| !g.failures.is_empty())
        || report.verify.as_ref().is_some_and(|v| !v.passed)
    {
        report.outcome = TestOutcome::Failed;
    }
    report.duration = start.elapsed();
    report
}

fn run_test_inner(
    pb: &Playbook,
    pkg: &Package,
    test: &TestDecl,
    backend: &dyn TestBackend,
    opts: &RunnerOptions,
    progress: &dyn Fn(&str),
    report: &mut TestReport,
) -> Result<(), Diag> {
    let synthesized = synth::synthesize(pb, pkg, test)?;

    progress(&format!("provisioning ({})", report.image));
    let mut instance = backend.provision(&report.image.clone(), opts.keep)?;
    if opts.keep {
        report.kept = Some(instance.handle());
    }

    let result = drive(
        test,
        instance.as_mut(),
        opts,
        progress,
        &synthesized,
        report,
    );

    if opts.keep {
        progress(&format!(
            "kept {} — remove it manually when done",
            instance.handle()
        ));
    } else {
        instance.teardown()?;
    }
    result
}

/// Everything that happens inside a provisioned instance.
fn drive(
    test: &TestDecl,
    instance: &mut dyn TestInstance,
    opts: &RunnerOptions,
    progress: &dyn Fn(&str),
    synthesized: &synth::SynthesizedTest,
    report: &mut TestReport,
) -> Result<(), Diag> {
    let os = instance.os();
    let paths = GuestPaths::for_os(os);
    let binary = opts.binaries.resolve(os)?;

    instance.copy_in(&binary, paths.bin)?;
    // docker cp preserves the executable bit; chmod defensively for
    // backends/umasks that do not. Best-effort: images without chmod
    // surface at the smoke test below. Windows has no execute bit.
    if os == GuestOs::Linux {
        let _ = instance.exec(&["chmod", "+x", paths.bin]);
    }
    let smoke = instance.exec(&[paths.bin, "version"])?;
    if smoke.exit_code != 0 {
        return Err(Diag::bare(format!(
            "the test binary failed to run inside '{}' (exit {}): {} — host/image \
             architecture mismatch?",
            report.image,
            smoke.exit_code,
            tail(&smoke.stderr)
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

    instance.copy_in(synthesized.dir.path(), paths.playbook)?;

    let facts = run_gathers(test, instance, progress, &mut report.gathers, paths)?;
    run_steps(test, instance, opts, progress, &mut report.steps, paths)?;
    report.verify = run_verify(test, instance, progress, &facts, paths)?;
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
        let mut argv = vec![paths.bin, "__gather", paths.playbook, &key];
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
            paths.playbook,
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
    instance.copy_in(facts_file.path(), paths.facts)?;

    let script = in_container_script(verify, paths)?;
    let out = instance.exec(&[paths.bin, "__verify", &script, "--facts", paths.facts])?;
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
    let lines: Vec<&str> = s.trim().lines().rev().take(3).collect();
    let t: Vec<&str> = lines.into_iter().rev().collect();
    if t.is_empty() {
        "(no output)".into()
    } else {
        t.join(" / ")
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
        let linux = GuestPaths::for_os(GuestOs::Linux);
        assert_eq!(
            in_container_script(p, linux).unwrap(),
            "/weave/playbook/pkgs/core/tests/verify.wisp"
        );
        let windows = GuestPaths::for_os(GuestOs::Windows);
        assert_eq!(
            in_container_script(p, windows).unwrap(),
            "C:/weave/playbook/pkgs/core/tests/verify.wisp"
        );
        assert!(in_container_script(Path::new("/elsewhere/verify.wisp"), linux).is_err());
    }

    #[test]
    fn guest_paths_per_os() {
        let l = GuestPaths::for_os(GuestOs::Linux);
        assert_eq!(l.bin, "/weave/config-weave");
        let w = GuestPaths::for_os(GuestOs::Windows);
        assert_eq!(w.bin, "C:/weave/config-weave.exe");
        assert_eq!(w.dir, "C:/weave");
    }
}
