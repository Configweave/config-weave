//! The run manager: spawns `config-weave test … --json --events-ndjson`
//! as a child process, relays its stderr NDJSON events to the forge event
//! bus (topic `run:{id}`) and a per-run catch-up buffer, captures the
//! final stdout report, and handles cancel + orphan-instance cleanup.
//!
//! The cancellation contract comes from the CLI flag's docs: killing the
//! child does not run its teardown, so the server cleans up instances it
//! saw in `instance_ready` events that never got a `group_teardown`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_server::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::io::{AsyncBufReadExt as _, BufReader};

/// Per-run catch-up buffer cap; older events are dropped (the final
/// report carries the authoritative results anyway).
const EVENT_BUFFER_CAP: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Passed,
    Failed,
    Error,
    Cancelled,
}

/// Mirror of the CLI's `AttachInfo` payload in `instance_ready` events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attach {
    Docker {
        container_id: String,
        image: String,
        cli: String,
    },
    Vmlab {
        lab_dir: String,
        lab: String,
        machine: String,
        template: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct InstanceInfo {
    pub group: usize,
    #[serde(flatten)]
    pub attach: Attach,
    pub torn_down: bool,
}

/// What `POST /api/runs` accepts.
#[derive(Debug, Clone, Deserialize)]
pub struct RunRequest {
    pub runbook: String,
    pub filter: Option<String>,
    pub backend: Option<String>,
    pub image: Option<String>,
    #[serde(default)]
    pub keep: bool,
}

pub struct Run {
    pub id: String,
    pub request: RunRequest,
    inner: Mutex<RunInner>,
    cancel: tokio::sync::Notify,
}

struct RunInner {
    status: RunStatus,
    events: VecDeque<Value>,
    dropped_events: usize,
    instances: Vec<InstanceInfo>,
    report: Option<Value>,
    exit_code: Option<i32>,
}

impl Run {
    /// Status + buffered events + report, for `GET /api/runs/{id}`.
    pub fn snapshot(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "id": self.id,
            "runbook": self.request.runbook,
            "filter": self.request.filter,
            "backend": self.request.backend,
            "image": self.request.image,
            "keep": self.request.keep,
            "status": inner.status,
            "exit_code": inner.exit_code,
            "instances": inner.instances,
            "events": inner.events,
            "dropped_events": inner.dropped_events,
            "report": inner.report,
        })
    }

    pub fn summary(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "id": self.id,
            "runbook": self.request.runbook,
            "filter": self.request.filter,
            "status": inner.status,
            "exit_code": inner.exit_code,
            // Live (not torn down) instances — kept ones show a badge.
            "kept_alive": inner.instances.iter().filter(|i| !i.torn_down).count(),
        })
    }

    pub fn status(&self) -> RunStatus {
        self.inner.lock().unwrap().status
    }

    /// Request cancellation; the reader task kills the child and cleans up.
    pub fn cancel(&self) {
        self.cancel.notify_waiters();
    }
}

/// Everything `start` needs besides the request itself.
pub struct RunContext {
    pub config_weave: String,
    pub runbook_dir: PathBuf,
    pub test_binary: Option<PathBuf>,
    pub test_binary_windows: Option<PathBuf>,
    pub events: EventBus,
}

#[derive(Default)]
pub struct RunManager {
    runs: Mutex<HashMap<String, Arc<Run>>>,
}

impl RunManager {
    pub fn get(&self, id: &str) -> Option<Arc<Run>> {
        self.runs.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Value> {
        let mut runs: Vec<Arc<Run>> = self.runs.lock().unwrap().values().cloned().collect();
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        runs.iter().map(|r| r.summary()).collect()
    }

    /// The docker attach info for `container_id`, if any run produced it.
    /// This is the terminal route's authorization check: only containers
    /// the server itself saw come up are reachable.
    pub fn docker_instance(&self, container_id: &str) -> Option<(String, String)> {
        for run in self.runs.lock().unwrap().values() {
            let inner = run.inner.lock().unwrap();
            for inst in &inner.instances {
                if let Attach::Docker {
                    container_id: id,
                    cli,
                    ..
                } = &inst.attach
                    && id == container_id
                {
                    return Some((cli.clone(), id.clone()));
                }
            }
        }
        None
    }

    /// The vmlab attach info for machine `machine` of run `run_id`.
    pub fn vmlab_instance(&self, run_id: &str, machine: &str) -> Option<(String, String)> {
        let run = self.get(run_id)?;
        let inner = run.inner.lock().unwrap();
        for inst in &inner.instances {
            if let Attach::Vmlab {
                lab, machine: m, ..
            } = &inst.attach
                && m == machine
            {
                return Some((lab.clone(), m.clone()));
            }
        }
        None
    }

    /// Spawn the child and its reader task; returns immediately.
    pub fn start(&self, request: RunRequest, ctx: RunContext) -> Result<Arc<Run>, String> {
        let id = uuid::Uuid::new_v4().to_string();

        let mut cmd = tokio::process::Command::new(&ctx.config_weave);
        cmd.arg("test").arg(&ctx.runbook_dir);
        if let Some(filter) = &request.filter {
            cmd.arg(filter);
        }
        cmd.args(["--json", "--events-ndjson"]);
        if let Some(backend) = &request.backend {
            cmd.args(["--backend", backend]);
        }
        if let Some(image) = &request.image {
            cmd.args(["--image", image]);
        }
        if request.keep {
            cmd.arg("--keep");
        }
        if let Some(bin) = &ctx.test_binary {
            cmd.arg("--binary").arg(bin);
        }
        if let Some(bin) = &ctx.test_binary_windows {
            cmd.arg("--binary-windows").arg(bin);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot spawn {}: {e}", ctx.config_weave))?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let run = Arc::new(Run {
            id: id.clone(),
            request,
            inner: Mutex::new(RunInner {
                status: RunStatus::Running,
                events: VecDeque::new(),
                dropped_events: 0,
                instances: Vec::new(),
                report: None,
                exit_code: None,
            }),
            cancel: tokio::sync::Notify::new(),
        });
        self.runs.lock().unwrap().insert(id, run.clone());

        // The final report arrives on stdout when the child exits; read it
        // concurrently so a large report can never deadlock the pipe.
        let stdout_task = tokio::spawn(async move {
            let mut buf = String::new();
            let _ = BufReader::new(stdout).read_to_string(&mut buf).await;
            buf
        });

        let task_run = run.clone();
        let bus = ctx.events.clone();
        tokio::spawn(async move {
            drive_run(task_run, child, stderr, stdout_task, bus).await;
        });

        Ok(run)
    }
}

/// The per-run reader task: stream stderr events until the child exits
/// (or is cancelled), then settle status, report, and stray instances.
async fn drive_run(
    run: Arc<Run>,
    mut child: tokio::process::Child,
    stderr: tokio::process::ChildStderr,
    stdout_task: tokio::task::JoinHandle<String>,
    bus: EventBus,
) {
    let topic = format!("run:{}", run.id);
    let mut lines = BufReader::new(stderr).lines();
    let mut cancelled = false;

    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => handle_event_line(&run, &bus, &topic, &line),
                Ok(None) => break,
                Err(_) => break,
            },
            _ = run.cancel.notified(), if !cancelled => {
                cancelled = true;
                let _ = child.start_kill();
                // Keep draining until the pipe closes so nothing is lost.
            }
        }
    }

    let exit = child.wait().await.ok();
    let stdout = stdout_task.await.unwrap_or_default();
    let report: Option<Value> = serde_json::from_str(stdout.trim()).ok();

    let status = {
        let mut inner = run.inner.lock().unwrap();
        inner.exit_code = exit.as_ref().and_then(|s| s.code());
        inner.report = report;
        inner.status = if cancelled {
            RunStatus::Cancelled
        } else {
            match inner.exit_code {
                Some(0) => RunStatus::Passed,
                Some(1) => RunStatus::Failed,
                _ => RunStatus::Error,
            }
        };
        inner.status
    };

    // A killed child never tears its instances down (Drop does not run on
    // SIGKILL) — clean up what we saw come up, unless the user asked to
    // keep instances for post-mortem debugging.
    if cancelled && !run.request.keep {
        cleanup_instances(&run).await;
    }

    // Server-generated terminal event so a consumer always sees the end,
    // even when the child died before its own `run_finished`.
    let closed = json!({
        "event": "run_closed",
        "status": status,
        "exit_code": run.inner.lock().unwrap().exit_code,
    });
    push_event(&run, closed.clone());
    bus.publish(&topic, closed);
}

/// One stderr line: parse, track instances, buffer, publish.
fn handle_event_line(run: &Arc<Run>, bus: &EventBus, topic: &str, line: &str) {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        // Not an event (stray print) — surface it as a raw line.
        let wrapped = json!({ "event": "raw", "line": line });
        push_event(run, wrapped.clone());
        bus.publish(topic, wrapped);
        return;
    };

    match event["event"].as_str() {
        Some("instance_ready") => {
            if let (Some(group), Ok(attach)) = (
                event["group"].as_u64(),
                serde_json::from_value::<Attach>(event["attach"].clone()),
            ) {
                run.inner.lock().unwrap().instances.push(InstanceInfo {
                    group: group as usize,
                    attach,
                    torn_down: false,
                });
            }
        }
        Some("group_teardown") => {
            // Kept instances (--keep) stay alive on purpose — they remain
            // attachable for post-mortem debugging, so only a real
            // teardown clears them.
            if event["kept"].as_bool() != Some(true)
                && let Some(group) = event["group"].as_u64()
            {
                let mut inner = run.inner.lock().unwrap();
                for inst in &mut inner.instances {
                    if inst.group == group as usize {
                        inst.torn_down = true;
                    }
                }
            }
        }
        _ => {}
    }

    push_event(run, event.clone());
    bus.publish(topic, event);
}

fn push_event(run: &Arc<Run>, event: Value) {
    let mut inner = run.inner.lock().unwrap();
    if inner.events.len() >= EVENT_BUFFER_CAP {
        inner.events.pop_front();
        inner.dropped_events += 1;
    }
    inner.events.push_back(event);
}

// ---------------------------------------------------------------- routes

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};

use crate::state::SharedState;

/// POST /api/runs — start a test run, returns `{id}` immediately.
pub async fn create(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
    axum::Json(request): axum::Json<RunRequest>,
) -> Response {
    // Same runbook resolution as the file routes: children of root only.
    let dir = state.root.join(&request.runbook);
    if request.runbook.contains('/')
        || request.runbook.starts_with('.')
        || !dir.join("playbook.wcl").is_file()
    {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    }
    let ctx = RunContext {
        config_weave: state.config_weave.clone(),
        runbook_dir: dir,
        test_binary: state.test_binary.clone(),
        test_binary_windows: state.test_binary_windows.clone(),
        events: state.events.clone(),
    };
    match state.runs.start(request, ctx) {
        Ok(run) => ok(json!({ "id": run.id })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// GET /api/runs
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    ok(state.runs.list())
}

/// GET /api/runs/{id} — status, buffered events, report.
pub async fn get(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match state.runs.get(&id) {
        Some(run) => ok(run.snapshot()),
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

/// POST /api/runs/{id}/cancel
pub async fn cancel(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match state.runs.get(&id) {
        Some(run) => {
            if run.status() == RunStatus::Running {
                run.cancel();
            }
            ok(json!({ "id": id, "status": run.status() }))
        }
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

/// POST /api/runs/{id}/teardown — remove a finished run's kept (or
/// orphaned) instances on demand: the debug flow's cleanup button.
pub async fn teardown(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some(run) = state.runs.get(&id) else {
        return err(StatusCode::NOT_FOUND, "no such run");
    };
    if run.status() == RunStatus::Running {
        return err(StatusCode::CONFLICT, "run is still running");
    }
    cleanup_instances(&run).await;
    // Nudge any open RunView so the troubleshoot tabs disappear live.
    let event = json!({ "event": "instances_torn_down" });
    push_event(&run, event.clone());
    state.events.publish(&format!("run:{id}"), event);
    ok(run.snapshot())
}

/// Remove instances the killed child left behind: `docker rm -f` for
/// containers, `vmlab destroy` (cwd = lab dir) + dir removal for VMs.
async fn cleanup_instances(run: &Arc<Run>) {
    let stray: Vec<Attach> = {
        let inner = run.inner.lock().unwrap();
        inner
            .instances
            .iter()
            .filter(|i| !i.torn_down)
            .map(|i| i.attach.clone())
            .collect()
    };
    for attach in stray {
        match attach {
            Attach::Docker {
                container_id, cli, ..
            } => {
                let _ = tokio::process::Command::new(&cli)
                    .args(["rm", "-f", &container_id])
                    .output()
                    .await;
            }
            Attach::Vmlab { lab_dir, .. } => {
                let vmlab =
                    std::env::var("CONFIG_WEAVE_VMLAB_CMD").unwrap_or_else(|_| "vmlab".into());
                let _ = tokio::process::Command::new(vmlab)
                    .arg("destroy")
                    .current_dir(&lab_dir)
                    .output()
                    .await;
                let _ = tokio::fs::remove_dir_all(&lab_dir).await;
            }
        }
    }
    let mut inner = run.inner.lock().unwrap();
    for inst in &mut inner.instances {
        inst.torn_down = true;
    }
}
