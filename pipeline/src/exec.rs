//! The pipeline execution engine: drives a run's ordered steps, resolving
//! secrets/properties, running script steps (local or over ssh/winrm) and
//! play steps (shelling out to the config-weave CLI), streaming output to
//! the run's event topic, and mapping outcomes to a terminal status.
//!
//! Failure semantics: a step whose command exits non-zero fails; if it is
//! `stop_on_failure` (the default) the run stops and the remaining steps
//! are reported `skipped`. An infra error (missing secret, spawn failure,
//! transport probe failure) is a hard `error` that always stops the run.

use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use weave_remote::{ExecSpec, Transport};

use crate::pipelines::{StepDef, TargetDef};
use crate::runs::{PipelineRun, RunContext, RunStatus};
use crate::secrets;

/// One step's outcome.
enum StepOutcome {
    Succeeded,
    /// The command exited non-zero. `status` is the human label
    /// (`failed` / `reboot_required`).
    Failed { exit_code: i32, status: &'static str },
    /// An infra error before/around the command.
    Error(String),
    Cancelled,
}

/// Map a play step's exit code to a step status. Mirrors the CLI's exit
/// codes (0 ok, 1 failed, 3 reboot required).
fn play_status(code: i32) -> StepOutcome {
    match code {
        0 => StepOutcome::Succeeded,
        3 => StepOutcome::Failed {
            exit_code: 3,
            status: "reboot_required",
        },
        other => StepOutcome::Failed {
            exit_code: other,
            status: "failed",
        },
    }
}

pub async fn run_pipeline(run: &PipelineRun, ctx: &RunContext) -> RunStatus {
    let secrets: HashMap<String, String> = run
        .def
        .secrets
        .iter()
        .map(|s| (s.name.clone(), s.value.clone()))
        .collect();
    let props = run.properties.clone();

    let mut overall = RunStatus::Succeeded;
    let mut stop = false;

    // Clone the step list so we don't hold a borrow on run.def across awaits
    // (the run is shared; def is a snapshot and never mutated).
    let steps = run.def.steps.clone();
    for (idx, step) in steps.iter().enumerate() {
        let name = step.name().to_string();

        if stop || run.is_cancelled() {
            let status = if run.is_cancelled() { "cancelled" } else { "skipped" };
            run.record_step(json!({ "index": idx, "name": name, "status": status }));
            run.emit(
                &ctx.events,
                json!({ "event": "step_finished", "index": idx, "name": name, "status": status }),
            );
            continue;
        }

        run.set_phase(&name);
        run.emit(
            &ctx.events,
            json!({ "event": "step_started", "index": idx, "name": name }),
        );

        let outcome = match step {
            StepDef::Script { .. } => run_script(run, ctx, step, &props, &secrets, idx).await,
            StepDef::Play { .. } => run_play(run, ctx, step, &props, &secrets, idx).await,
        };

        let status_label = match &outcome {
            StepOutcome::Succeeded => "succeeded",
            StepOutcome::Failed { status, .. } => status,
            StepOutcome::Error(_) => "error",
            StepOutcome::Cancelled => "cancelled",
        };
        let mut result = json!({ "index": idx, "name": name, "status": status_label });
        if let StepOutcome::Failed { exit_code, .. } = &outcome {
            result["exit_code"] = json!(exit_code);
        }
        if let StepOutcome::Error(msg) = &outcome {
            result["message"] = json!(msg);
        }
        run.record_step(result.clone());
        let mut finished = result;
        finished["event"] = json!("step_finished");
        run.emit(&ctx.events, finished);

        match outcome {
            StepOutcome::Succeeded => {}
            StepOutcome::Cancelled => return RunStatus::Cancelled,
            StepOutcome::Error(_) => {
                overall = RunStatus::Error;
                stop = true;
            }
            StepOutcome::Failed { .. } => {
                overall = RunStatus::Failed;
                if step_stops_on_failure(step) {
                    stop = true;
                }
            }
        }
    }

    if run.is_cancelled() {
        RunStatus::Cancelled
    } else {
        overall
    }
}

fn step_stops_on_failure(step: &StepDef) -> bool {
    match step {
        StepDef::Script {
            stop_on_failure, ..
        }
        | StepDef::Play {
            stop_on_failure, ..
        } => *stop_on_failure,
    }
}

// --------------------------------------------------------- script steps

async fn run_script(
    run: &PipelineRun,
    ctx: &RunContext,
    step: &StepDef,
    props: &HashMap<String, String>,
    secrets: &HashMap<String, String>,
    idx: usize,
) -> StepOutcome {
    let StepDef::Script {
        name,
        on,
        run: body,
        shell,
        env,
        ..
    } = step
    else {
        unreachable!("run_script only called for script steps");
    };
    let ctxlabel = format!("script '{name}'");

    // Resolve env values (literal / prop: / secret:).
    let mut resolved_env: Vec<(String, String)> = Vec::new();
    for (k, v) in env {
        match secrets::resolve(v, props, secrets, &ctxlabel) {
            Ok(val) => resolved_env.push((k.clone(), val)),
            Err(e) => return StepOutcome::Error(e),
        }
    }

    if on == "local" {
        run_local_script(run, ctx, body, shell.as_deref(), &resolved_env, idx).await
    } else {
        let args = RemoteScript {
            target_name: on,
            body,
            env: &resolved_env,
            ctxlabel: &ctxlabel,
            idx,
        };
        run_remote_script(run, ctx, secrets, args).await
    }
}

/// The non-shared inputs a remote script step needs (bundled to keep the
/// function signature small).
struct RemoteScript<'a> {
    target_name: &'a str,
    body: &'a str,
    env: &'a [(String, String)],
    ctxlabel: &'a str,
    idx: usize,
}

/// Split a shell spec into (program, leading args) for `<program> <args> <script>`.
fn shell_invocation(shell: Option<&str>) -> (String, Vec<String>) {
    match shell {
        Some(s) if s.contains("powershell") || s.contains("pwsh") => {
            (s.to_string(), vec!["-NoProfile".into(), "-Command".into()])
        }
        Some(s) => (s.to_string(), vec!["-c".into()]),
        None => ("sh".into(), vec!["-c".into()]),
    }
}

async fn run_local_script(
    run: &PipelineRun,
    ctx: &RunContext,
    body: &str,
    shell: Option<&str>,
    env: &[(String, String)],
    idx: usize,
) -> StepOutcome {
    let (program, args) = shell_invocation(shell);
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args).arg(body);
    // Secrets ride in the environment, never on argv.
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return StepOutcome::Error(format!("cannot spawn {program}: {e}")),
    };
    let mut out = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
    let mut errl = BufReader::new(child.stderr.take().expect("stderr piped")).lines();
    let (mut out_done, mut err_done, mut cancelled) = (false, false, false);

    while !(out_done && err_done) {
        tokio::select! {
            line = out.next_line(), if !out_done => match line {
                Ok(Some(l)) => emit_output(run, ctx, idx, &l),
                _ => out_done = true,
            },
            line = errl.next_line(), if !err_done => match line {
                Ok(Some(l)) => emit_output(run, ctx, idx, &l),
                _ => err_done = true,
            },
            _ = run.cancel.notified(), if !cancelled => {
                cancelled = true;
                let _ = child.start_kill();
            }
        }
    }
    let status = child.wait().await.ok();
    if cancelled || run.is_cancelled() {
        return StepOutcome::Cancelled;
    }
    match status.and_then(|s| s.code()) {
        Some(0) => StepOutcome::Succeeded,
        Some(code) => StepOutcome::Failed {
            exit_code: code,
            status: "failed",
        },
        None => StepOutcome::Error("process terminated by signal".into()),
    }
}

async fn run_remote_script(
    run: &PipelineRun,
    ctx: &RunContext,
    secrets: &HashMap<String, String>,
    args: RemoteScript<'_>,
) -> StepOutcome {
    let RemoteScript {
        target_name,
        body,
        env,
        ctxlabel,
        idx,
    } = args;
    let Some(target) = run.def.targets.iter().find(|t| t.name == target_name) else {
        return StepOutcome::Error(format!("{ctxlabel}: no target named '{target_name}'"));
    };
    let cfg = match secrets::resolve_transport(&target.transport, secrets, ctxlabel) {
        Ok(c) => c,
        Err(e) => return StepOutcome::Error(e),
    };
    let transport = match Transport::new(&cfg, target.os) {
        Ok(t) => t,
        Err(e) => return StepOutcome::Error(format!("{ctxlabel}: {e}")),
    };
    if let Err(e) = transport.probe().await {
        return StepOutcome::Error(format!("{ctxlabel}: {e}"));
    }

    // Prepend env exports into the script body (they ride the encrypted
    // channel; note they are visible in the remote process list).
    let script = remote_script_with_env(target, body, env);
    let spec = remote_spec(target, &script);

    let mut on_line = |line: String| emit_output(run, ctx, idx, &line);
    match transport.exec_stream(&spec, &mut on_line, &run.cancel).await {
        Ok(0) => StepOutcome::Succeeded,
        Ok(code) => StepOutcome::Failed {
            exit_code: code,
            status: "failed",
        },
        Err(e) if e == "cancelled" => StepOutcome::Cancelled,
        Err(e) => StepOutcome::Error(format!("{ctxlabel}: {e}")),
    }
}

fn remote_script_with_env(target: &TargetDef, body: &str, env: &[(String, String)]) -> String {
    use weave_remote::TargetOs;
    let mut prefix = String::new();
    match target.os {
        TargetOs::Linux => {
            for (k, v) in env {
                // POSIX single-quote the value.
                prefix.push_str(&format!("export {k}='{}'\n", v.replace('\'', r"'\''")));
            }
        }
        TargetOs::Windows => {
            for (k, v) in env {
                prefix.push_str(&format!("$env:{k} = '{}'\n", v.replace('\'', "''")));
            }
        }
    }
    format!("{prefix}{body}")
}

fn remote_spec(target: &TargetDef, script: &str) -> ExecSpec {
    use weave_remote::TargetOs;
    match target.os {
        TargetOs::Linux => ExecSpec {
            program: "sh".into(),
            args: vec!["-c".into(), script.to_string()],
            stdout_to: None,
        },
        TargetOs::Windows => ExecSpec {
            program: "powershell".into(),
            args: vec!["-NoProfile".into(), "-Command".into(), script.to_string()],
            stdout_to: None,
        },
    }
}

// ----------------------------------------------------------- play steps

async fn run_play(
    run: &PipelineRun,
    ctx: &RunContext,
    step: &StepDef,
    props: &HashMap<String, String>,
    secrets: &HashMap<String, String>,
    idx: usize,
) -> StepOutcome {
    let StepDef::Play {
        name,
        playbook,
        play,
        action,
        vars,
        ..
    } = step
    else {
        unreachable!("run_play only called for play steps");
    };
    let ctxlabel = format!("play '{name}'");

    let action = if action == "check" { "check" } else { "apply" };
    let playbook_dir = ctx.playbooks_dir.join(playbook);
    if !playbook_dir.is_dir() {
        return StepOutcome::Error(format!(
            "{ctxlabel}: no playbook '{playbook}' under {}",
            ctx.playbooks_dir.display()
        ));
    }

    // Resolve vars and write them to a 0600 var-file (never on argv).
    let mut resolved: Vec<(String, String)> = Vec::new();
    for (k, v) in vars {
        match secrets::resolve(v, props, secrets, &ctxlabel) {
            Ok(val) => resolved.push((k.clone(), val)),
            Err(e) => return StepOutcome::Error(e),
        }
    }
    let var_file = match secrets::write_var_file(&resolved) {
        Ok(f) => f,
        Err(e) => return StepOutcome::Error(e),
    };

    let mut cmd = tokio::process::Command::new(&ctx.config_weave);
    cmd.arg(action)
        .arg(&playbook_dir)
        .arg(play)
        .args(["--json", "--events-ndjson", "--continue-on-error"])
        .arg("--var-file")
        .arg(var_file.path());
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return StepOutcome::Error(format!("{ctxlabel}: cannot spawn config-weave: {e}")),
    };
    // Collect the final JSON report from stdout; stream NDJSON events from stderr.
    let mut stdout_buf = String::new();
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout piped"));
    let mut errl = BufReader::new(child.stderr.take().expect("stderr piped")).lines();
    let mut cancelled = false;
    let mut stdout_done = false;

    loop {
        tokio::select! {
            n = read_line_into(&mut stdout, &mut stdout_buf), if !stdout_done => {
                if matches!(n, Ok(0)) { stdout_done = true; }
            }
            line = errl.next_line() => match line {
                Ok(Some(l)) => relay_event(run, ctx, idx, &l),
                _ => {
                    // stderr closed; drain the rest of stdout, then finish.
                    let _ = tokio::io::AsyncReadExt::read_to_string(&mut stdout, &mut stdout_buf).await;
                    break;
                }
            },
            _ = run.cancel.notified(), if !cancelled => {
                cancelled = true;
                let _ = child.start_kill();
            }
        }
    }

    let status = child.wait().await.ok();
    drop(var_file);
    if cancelled || run.is_cancelled() {
        return StepOutcome::Cancelled;
    }
    // Surface the parsed report as an event so the UI can render it.
    if let Ok(report) = serde_json::from_str::<Value>(stdout_buf.trim()) {
        run.emit(
            &ctx.events,
            json!({ "event": "play_report", "index": idx, "report": report }),
        );
    }
    match status.and_then(|s| s.code()) {
        Some(code) => play_status(code),
        None => StepOutcome::Error(format!("{ctxlabel}: config-weave terminated by signal")),
    }
}

async fn read_line_into(
    reader: &mut (impl AsyncBufReadExt + Unpin),
    buf: &mut String,
) -> std::io::Result<usize> {
    reader.read_line(buf).await
}

// -------------------------------------------------------------- helpers

/// Emit a raw output line from a script step.
fn emit_output(run: &PipelineRun, ctx: &RunContext, idx: usize, line: &str) {
    run.emit(
        &ctx.events,
        json!({ "event": "output", "index": idx, "line": line }),
    );
}

/// Parse-or-wrap one NDJSON line from a play step; forward it under the
/// step index so the UI can attribute it.
fn relay_event(run: &PipelineRun, ctx: &RunContext, idx: usize, line: &str) {
    let inner = serde_json::from_str::<Value>(line)
        .ok()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({ "event": "output", "line": line }));
    run.emit(
        &ctx.events,
        json!({ "event": "play_event", "index": idx, "data": inner }),
    );
}
