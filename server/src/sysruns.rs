//! System runs: check/apply against an inventory system, mirroring the
//! test RunManager's mechanics (event ring buffer, bus topic, cancel
//! Notify, synthesized terminal event) with two drivers:
//!
//! - **remote** systems: the playbook runs locally on the server
//!   (`config-weave check/apply --json --events-ndjson`); the system's
//!   connection details are injected as `system_*` vars via a 0600
//!   var-file so playbook wscripts can connect out themselves.
//! - **direct** systems: the matching static config-weave build and the
//!   playbook are staged onto the target over the system's transport,
//!   run there (stdout → `<stage>/report.json`, events streamed), the
//!   report fetched back, and the staging dir removed.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_server::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::io::{AsyncBufReadExt as _, BufReader};

use crate::systems::{SystemDef, SystemKind};
use crate::transport::{ExecSpec, Transport, stage_dir};

/// Same catch-up cap as the test runs.
const EVENT_BUFFER_CAP: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Check,
    Apply,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Check => "check",
            Action::Apply => "apply",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SysRunStatus {
    Running,
    Succeeded,
    Failed,
    RebootRequired,
    Error,
    Cancelled,
}

fn status_from_exit(code: Option<i32>) -> SysRunStatus {
    match code {
        Some(0) => SysRunStatus::Succeeded,
        Some(1) => SysRunStatus::Failed,
        Some(3) => SysRunStatus::RebootRequired,
        _ => SysRunStatus::Error,
    }
}

/// What `POST /api/systems/{name}/runs` accepts.
#[derive(Debug, Clone, Deserialize)]
pub struct SysRunRequest {
    pub action: Action,
    /// Direct systems: leave the staging dir on the target for debugging.
    #[serde(default)]
    pub keep: bool,
}

pub struct SysRun {
    pub id: String,
    pub request: SysRunRequest,
    /// Snapshot of the system at start — later edits don't affect a
    /// running run.
    pub system: SystemDef,
    inner: Mutex<SysRunInner>,
    cancel: tokio::sync::Notify,
}

struct SysRunInner {
    status: SysRunStatus,
    /// Coarse progress for the sidebar (deploy phase or "run").
    phase: String,
    events: VecDeque<Value>,
    dropped_events: usize,
    report: Option<Value>,
    exit_code: Option<i32>,
}

impl SysRun {
    pub fn snapshot(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "id": self.id,
            "system": self.system.name,
            "kind": self.system.kind,
            "action": self.request.action,
            "keep": self.request.keep,
            "playbook": self.system.playbook,
            "play": self.system.play,
            "status": inner.status,
            "phase": inner.phase,
            "exit_code": inner.exit_code,
            "events": inner.events,
            "dropped_events": inner.dropped_events,
            "report": inner.report,
        })
    }

    pub fn summary(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "id": self.id,
            "system": self.system.name,
            "action": self.request.action,
            "status": inner.status,
            "phase": inner.phase,
            "exit_code": inner.exit_code,
        })
    }

    pub fn status(&self) -> SysRunStatus {
        self.inner.lock().unwrap().status
    }

    pub fn cancel(&self) {
        self.cancel.notify_waiters();
    }

    fn set_phase(&self, phase: &str) {
        self.inner.lock().unwrap().phase = phase.to_string();
    }
}

/// Everything a driver needs besides the run itself.
pub struct SysRunContext {
    pub config_weave: String,
    pub runbook_dir: PathBuf,
    pub deploy_binary: Option<PathBuf>,
    pub events: EventBus,
}

#[derive(Default)]
pub struct SysRunManager {
    runs: Mutex<HashMap<String, Arc<SysRun>>>,
}

impl SysRunManager {
    pub fn get(&self, id: &str) -> Option<Arc<SysRun>> {
        self.runs.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Value> {
        let mut runs: Vec<Arc<SysRun>> = self.runs.lock().unwrap().values().cloned().collect();
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        runs.iter().map(|r| r.summary()).collect()
    }

    /// One running run per system: overlapping applies would race.
    pub fn running_for(&self, system: &str) -> bool {
        self.runs
            .lock()
            .unwrap()
            .values()
            .any(|r| r.system.name == system && r.status() == SysRunStatus::Running)
    }

    pub fn start(
        &self,
        request: SysRunRequest,
        system: SystemDef,
        ctx: SysRunContext,
    ) -> Result<Arc<SysRun>, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let run = Arc::new(SysRun {
            id: id.clone(),
            request,
            system,
            inner: Mutex::new(SysRunInner {
                status: SysRunStatus::Running,
                phase: "starting".into(),
                events: VecDeque::new(),
                dropped_events: 0,
                report: None,
                exit_code: None,
            }),
            cancel: tokio::sync::Notify::new(),
        });
        self.runs.lock().unwrap().insert(id, run.clone());

        let task_run = run.clone();
        tokio::spawn(async move {
            let outcome = match task_run.system.kind {
                SystemKind::Remote => drive_remote(&task_run, &ctx).await,
                SystemKind::Direct => drive_direct(&task_run, &ctx).await,
            };
            settle(&task_run, &ctx.events, outcome);
        });
        Ok(run)
    }
}

/// A driver's terminal state: exit code of the run, or an error message,
/// or cancellation.
enum Outcome {
    Finished {
        exit_code: i32,
        report: Option<Value>,
    },
    Cancelled,
    Failed(String),
}

fn settle(run: &Arc<SysRun>, bus: &EventBus, outcome: Outcome) {
    let topic = format!("sysrun:{}", run.id);
    let mut error_event = None;
    let (status, exit_code) = {
        let mut inner = run.inner.lock().unwrap();
        match outcome {
            Outcome::Finished { exit_code, report } => {
                inner.exit_code = Some(exit_code);
                inner.report = report;
                inner.status = status_from_exit(Some(exit_code));
            }
            Outcome::Cancelled => inner.status = SysRunStatus::Cancelled,
            Outcome::Failed(message) => {
                inner.status = SysRunStatus::Error;
                error_event = Some(json!({
                    "event": "deploy_error",
                    "phase": inner.phase,
                    "message": message,
                }));
            }
        }
        (inner.status, inner.exit_code)
    };
    if let Some(event) = error_event {
        push_event(run, event.clone());
        bus.publish(&topic, event);
    }
    let closed = json!({ "event": "run_closed", "status": status, "exit_code": exit_code });
    push_event(run, closed.clone());
    bus.publish(&topic, closed);
}

fn push_event(run: &Arc<SysRun>, event: Value) {
    let mut inner = run.inner.lock().unwrap();
    if inner.events.len() >= EVENT_BUFFER_CAP {
        inner.events.pop_front();
        inner.dropped_events += 1;
    }
    inner.events.push_back(event);
}

/// Parse-or-wrap one output line, buffer it, publish it.
fn relay_line(run: &Arc<SysRun>, bus: &EventBus, topic: &str, line: &str) {
    let event = serde_json::from_str::<Value>(line)
        .ok()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({ "event": "raw", "line": line }));
    push_event(run, event.clone());
    bus.publish(topic, event);
}

/// Server-synthesized deploy progress marker.
fn deploy_phase(run: &Arc<SysRun>, bus: &EventBus, topic: &str, phase: &str) {
    run.set_phase(phase);
    let event = json!({ "event": "deploy_phase", "phase": phase });
    push_event(run, event.clone());
    bus.publish(topic, event);
}

// -------------------------------------------------------- remote driver

/// The flat WCL var-file injecting the system's connection details;
/// values are emitted through wcl_lang's printer so any password is
/// quoted correctly.
fn system_var_file(sys: &SystemDef) -> String {
    use wcl_lang::{ast, edit, format as wclformat};
    let mut src = ast::Source {
        items: Vec::new(),
        trailing_trivia: Vec::new(),
    };
    let mut push = |name: &str, expr: ast::Expr| {
        // A var-file is flat `name = value` fields; reuse the block
        // builder's field synthesis by building one throwaway block.
        let block = edit::build_block("x", &[], vec![], vec![(name.to_string(), expr)]);
        if let Some(ast::Item::Field(f)) = block.items.into_iter().next() {
            src.items.push(ast::Item::Field(f));
        }
    };
    let t = &sys.transport;
    push("system_name", edit::string_literal_expr(&sys.name));
    push("system_host", edit::string_literal_expr(&t.host));
    push("system_port", ast::Expr::I64(i64::from(t.effective_port())));
    push("system_user", edit::string_literal_expr(&t.user));
    push(
        "system_password",
        edit::string_literal_expr(t.password.as_deref().unwrap_or("")),
    );
    push(
        "system_private_key",
        edit::string_literal_expr(t.private_key.as_deref().unwrap_or("")),
    );
    push(
        "system_transport",
        edit::string_literal_expr(t.kind.as_str()),
    );
    push("system_os", edit::string_literal_expr(sys.os.as_str()));
    wclformat::to_source(&src)
}

async fn drive_remote(run: &Arc<SysRun>, ctx: &SysRunContext) -> Outcome {
    let topic = format!("sysrun:{}", run.id);
    run.set_phase("run");

    // 0600 var-file so credentials never hit the process list; the guard
    // must outlive the child.
    let var_file = match write_var_file(&run.system) {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(e),
    };

    let mut cmd = tokio::process::Command::new(&ctx.config_weave);
    cmd.arg(run.request.action.as_str())
        .arg(&ctx.runbook_dir)
        .arg(&run.system.play)
        .args(["--json", "--events-ndjson", "--continue-on-error"])
        .arg("--var-file")
        .arg(var_file.path());
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(format!("cannot spawn {}: {e}", ctx.config_weave)),
    };
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let stdout_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = BufReader::new(stdout).read_to_string(&mut buf).await;
        buf
    });

    let mut lines = BufReader::new(stderr).lines();
    let mut cancelled = false;
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => relay_line(run, &ctx.events, &topic, &line),
                _ => break,
            },
            _ = run.cancel.notified(), if !cancelled => {
                cancelled = true;
                let _ = child.start_kill();
            }
        }
    }

    let exit = child.wait().await.ok();
    let stdout = stdout_task.await.unwrap_or_default();
    drop(var_file);
    if cancelled {
        return Outcome::Cancelled;
    }
    Outcome::Finished {
        exit_code: exit.and_then(|s| s.code()).unwrap_or(-1),
        report: serde_json::from_str(stdout.trim()).ok(),
    }
}

fn write_var_file(sys: &SystemDef) -> Result<tempfile::NamedTempFile, String> {
    use std::io::Write as _;
    let mut f =
        tempfile::NamedTempFile::with_suffix(".wcl").map_err(|e| format!("var-file: {e}"))?;
    f.write_all(system_var_file(sys).as_bytes())
        .map_err(|e| format!("var-file: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("var-file: {e}"))?;
    }
    Ok(f)
}

// -------------------------------------------------------- direct driver

async fn drive_direct(run: &Arc<SysRun>, ctx: &SysRunContext) -> Outcome {
    let topic = format!("sysrun:{}", run.id);
    let sys = &run.system;

    let Some(binary) = &ctx.deploy_binary else {
        return Outcome::Failed(format!(
            "no deploy binary registered for {}; pass --deploy-binary {}=PATH",
            sys.binary_key(),
            sys.binary_key(),
        ));
    };

    let transport = match Transport::for_system(sys) {
        Ok(t) => t,
        Err(e) => return Outcome::Failed(e),
    };

    deploy_phase(run, &ctx.events, &topic, "connect");
    if let Err(e) = transport.probe().await {
        return Outcome::Failed(e);
    }

    let stage = stage_dir(sys.os, &run.id);
    let exe = match sys.os {
        crate::systems::TargetOs::Linux => format!("{stage}/config-weave"),
        crate::systems::TargetOs::Windows => format!("{stage}/config-weave.exe"),
    };
    let playbook_dest = format!("{stage}/playbook");
    let report_path = format!("{stage}/report.json");

    // Any failure below still attempts cleanup (unless keep).
    let result: Outcome = async {
        deploy_phase(run, &ctx.events, &topic, "stage_binary");
        transport.mkdir(&stage).await?;
        transport.copy_file_in(binary, &exe, true).await?;

        deploy_phase(run, &ctx.events, &topic, "stage_playbook");
        transport
            .copy_dir_in(&ctx.runbook_dir, &playbook_dest)
            .await?;

        deploy_phase(run, &ctx.events, &topic, "run");
        let spec = ExecSpec {
            program: exe.clone(),
            args: vec![
                run.request.action.as_str().to_string(),
                playbook_dest.clone(),
                sys.play.clone(),
                "--json".into(),
                "--events-ndjson".into(),
                "--continue-on-error".into(),
                "--no-color".into(),
            ],
            stdout_to: Some(report_path.clone()),
        };
        let run_ref = run.clone();
        let bus = ctx.events.clone();
        let topic_line = topic.clone();
        let mut on_line = move |line: String| relay_line(&run_ref, &bus, &topic_line, &line);
        let exec = transport
            .exec_stream(&spec, &mut on_line, &run.cancel)
            .await;

        match exec {
            Err(e) if e == "cancelled" => Ok(Outcome::Cancelled),
            Err(e) => Err(e),
            Ok(exit_code) => {
                deploy_phase(run, &ctx.events, &topic, "fetch_report");
                let report = transport
                    .fetch_file(&report_path)
                    .await
                    .ok()
                    .and_then(|s| serde_json::from_str(s.trim()).ok());
                Ok(Outcome::Finished { exit_code, report })
            }
        }
    }
    .await
    .map_err(Outcome::Failed)
    .unwrap_or_else(|o| o);

    if run.request.keep {
        let kept = json!({ "event": "stage_kept", "stage": stage, "host": sys.transport.host });
        push_event(run, kept.clone());
        ctx.events.publish(&topic, kept);
    } else {
        deploy_phase(run, &ctx.events, &topic, "cleanup");
        if let Err(e) = transport.remove_dir(&stage).await {
            let warn = json!({ "event": "raw", "line": format!("cleanup warning: {e}") });
            push_event(run, warn.clone());
            ctx.events.publish(&topic, warn);
        }
    }
    result
}

// ---------------------------------------------------------------- routes

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};

use crate::runbooks::runbook_dir;
use crate::state::SharedState;

/// POST /api/systems/{name}/runs — `{action, keep?}` → `{id}`.
pub async fn create(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(request): axum::Json<SysRunRequest>,
) -> Response {
    let Some(system) = state
        .systems
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.name == name)
        .cloned()
    else {
        return err(StatusCode::NOT_FOUND, "no such system");
    };
    if state.sysruns.running_for(&name) {
        return err(
            StatusCode::CONFLICT,
            "a run against this system is already in progress",
        );
    }
    let Some(dir) = runbook_dir(&state, &system.playbook) else {
        return err(
            StatusCode::CONFLICT,
            format!(
                "the system's runbook '{}' no longer exists",
                system.playbook
            ),
        );
    };
    let ctx = SysRunContext {
        config_weave: state.config_weave.clone(),
        runbook_dir: dir,
        deploy_binary: state.deploy_binaries.get(&system.binary_key()).cloned(),
        events: state.events.clone(),
    };
    match state.sysruns.start(request, system, ctx) {
        Ok(run) => ok(json!({ "id": run.id })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// GET /api/system-runs
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    ok(state.sysruns.list())
}

/// GET /api/system-runs/{id}
pub async fn get(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match state.sysruns.get(&id) {
        Some(run) => ok(run.snapshot()),
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

/// POST /api/system-runs/{id}/cancel
pub async fn cancel(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match state.sysruns.get(&id) {
        Some(run) => {
            if run.status() == SysRunStatus::Running {
                run.cancel();
            }
            ok(json!({ "id": id, "status": run.status() }))
        }
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::{SystemKind, TargetOs, TransportConfig, TransportKind};

    fn sys() -> SystemDef {
        SystemDef {
            name: "edge".into(),
            description: None,
            playbook: "net".into(),
            play: "router".into(),
            kind: SystemKind::Remote,
            os: TargetOs::Linux,
            arch: "x86_64".into(),
            transport: TransportConfig {
                kind: TransportKind::Ssh,
                host: "10.0.0.1".into(),
                port: None,
                user: "admin".into(),
                password: Some("it's a secret".into()),
                private_key: None,
                use_tls: false,
            },
        }
    }

    #[test]
    fn var_file_is_flat_quoted_wcl() {
        let text = system_var_file(&sys());
        assert!(text.contains("system_name = \"edge\""));
        assert!(text.contains("system_host = \"10.0.0.1\""));
        assert!(text.contains("system_port = 22"));
        assert!(text.contains("system_transport = \"ssh\""));
        // The password with a quote-hostile char survives quoting.
        assert!(text.contains("it's a secret"));
        // No block syntax — a var-file is flat fields only.
        assert!(!text.contains('{'));
    }

    #[test]
    fn exit_codes_map_to_statuses() {
        assert_eq!(status_from_exit(Some(0)), SysRunStatus::Succeeded);
        assert_eq!(status_from_exit(Some(1)), SysRunStatus::Failed);
        assert_eq!(status_from_exit(Some(3)), SysRunStatus::RebootRequired);
        assert_eq!(status_from_exit(Some(2)), SysRunStatus::Error);
        assert_eq!(status_from_exit(None), SysRunStatus::Error);
    }
}
