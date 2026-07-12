//! Pipeline runs: mirror weave-server's run managers (event ring buffer,
//! bus topic `pipeline:{id}`, cancel Notify, synthesized terminal event).
//! A run executes a pipeline's ordered steps; execution itself lives in
//! `exec.rs`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_server::EventBus;
use serde::Serialize;
use serde_json::{Value, json};

use crate::pipelines::PipelineDef;

/// Per-run catch-up buffer cap; older events drop (the step_results carry
/// the authoritative per-step outcome anyway).
const EVENT_BUFFER_CAP: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Error,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Error => "error",
            RunStatus::Cancelled => "cancelled",
        }
    }
}

pub struct PipelineRun {
    pub id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub pipeline: String,
    /// The trigger that started this run (a trigger name, "manual", or
    /// "webhook:<name>").
    pub trigger: String,
    pub properties: HashMap<String, String>,
    /// Snapshot of the pipeline at start — later edits don't affect it.
    pub def: PipelineDef,
    inner: Mutex<Inner>,
    /// Interrupts an in-flight step (a child process / transport exec).
    pub cancel: tokio::sync::Notify,
    /// Latches a cancel request so it is honoured at the next step boundary
    /// too (Notify only wakes waiters registered at notify time).
    cancelled: std::sync::atomic::AtomicBool,
}

struct Inner {
    status: RunStatus,
    /// Coarse progress: the current step's name, or a lifecycle marker.
    phase: String,
    events: VecDeque<Value>,
    dropped_events: usize,
    /// One entry per step attempted, in order.
    step_results: Vec<Value>,
}

impl PipelineRun {
    pub fn snapshot(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "id": self.id,
            "started_at": self.started_at,
            "pipeline": self.pipeline,
            "trigger": self.trigger,
            "properties": self.properties,
            "status": inner.status,
            "phase": inner.phase,
            "steps": inner.step_results,
            "events": inner.events,
            "dropped_events": inner.dropped_events,
        })
    }

    pub fn summary(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({
            "id": self.id,
            "started_at": self.started_at,
            "pipeline": self.pipeline,
            "trigger": self.trigger,
            "status": inner.status,
            "phase": inner.phase,
        })
    }

    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.cancel.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_phase(&self, phase: &str) {
        self.inner.lock().unwrap().phase = phase.to_string();
    }

    /// Buffer an event and publish it to the run topic.
    pub fn emit(&self, bus: &EventBus, event: Value) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.events.len() >= EVENT_BUFFER_CAP {
                inner.events.pop_front();
                inner.dropped_events += 1;
            }
            inner.events.push_back(event.clone());
        }
        bus.publish(format!("pipeline:{}", self.id), event);
    }

    pub fn record_step(&self, result: Value) {
        self.inner.lock().unwrap().step_results.push(result);
    }

    fn set_status(&self, status: RunStatus) {
        self.inner.lock().unwrap().status = status;
    }
}

/// Everything a run needs to execute besides the run itself.
pub struct RunContext {
    pub config_weave: String,
    pub playbooks_dir: PathBuf,
    pub events: EventBus,
}

#[derive(Default)]
pub struct PipelineRunManager {
    runs: Mutex<HashMap<String, Arc<PipelineRun>>>,
}

impl PipelineRunManager {
    pub fn get(&self, id: &str) -> Option<Arc<PipelineRun>> {
        self.runs.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Value> {
        let mut runs: Vec<Arc<PipelineRun>> = self.runs.lock().unwrap().values().cloned().collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        runs.iter().map(|r| r.summary()).collect()
    }

    /// Start a run: insert it, spawn the executor, return the handle.
    pub fn start(
        &self,
        def: PipelineDef,
        properties: HashMap<String, String>,
        trigger: String,
        ctx: RunContext,
    ) -> Arc<PipelineRun> {
        let id = uuid::Uuid::new_v4().to_string();
        let run = Arc::new(PipelineRun {
            id: id.clone(),
            started_at: chrono::Utc::now(),
            pipeline: def.name.clone(),
            trigger,
            properties,
            def,
            inner: Mutex::new(Inner {
                status: RunStatus::Running,
                phase: "starting".into(),
                events: VecDeque::new(),
                dropped_events: 0,
                step_results: Vec::new(),
            }),
            cancel: tokio::sync::Notify::new(),
            cancelled: std::sync::atomic::AtomicBool::new(false),
        });
        self.runs.lock().unwrap().insert(id, run.clone());
        tracing::info!(
            pipeline = %run.pipeline,
            run_id = %run.id,
            trigger = %run.trigger,
            "pipeline run started"
        );

        let task_run = run.clone();
        tokio::spawn(async move {
            let status = crate::exec::run_pipeline(&task_run, &ctx).await;
            settle(&task_run, &ctx.events, status);
        });
        run
    }
}

fn settle(run: &Arc<PipelineRun>, bus: &EventBus, status: RunStatus) {
    run.set_status(status);
    run.set_phase("done");
    tracing::info!(
        pipeline = %run.pipeline,
        run_id = %run.id,
        trigger = %run.trigger,
        status = status.as_str(),
        "pipeline run finished"
    );
    run.emit(bus, json!({ "event": "run_closed", "status": status }));
}
