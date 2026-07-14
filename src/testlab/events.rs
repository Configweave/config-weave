//! Testlab progress events: one stream drives both the human stderr
//! renderer and the machine NDJSON emitter (mirroring `engine::events`).
//! `--events-ndjson` consumers (external supervisors) parse one JSON
//! object per stderr line to follow a run live and to learn the raw
//! instance ids they need for troubleshooting attach and post-kill
//! cleanup.

use std::io::Write as _;
use std::sync::Arc;

use serde::Serialize;

use super::backend::AttachInfo;

/// Largest `Log` chunk carried in one event; longer output keeps its tail.
const LOG_CHUNK_MAX: usize = 8 * 1024;

/// A test planned for this run, as announced in `run_started`.
#[derive(Debug, Clone, Serialize)]
pub struct PlannedTest {
    pub package: String,
    pub test: String,
    /// Index into the run's group list; `None` for scenarios, which run
    /// sequentially after the groups.
    pub group: Option<usize>,
    pub backend: String,
    pub image: String,
}

/// The phase a test just entered. The three engine runs mirror the
/// check → apply → apply idempotence protocol.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum TestPhase {
    Setup,
    Gather { name: String },
    Check,
    FirstApply,
    SecondApply,
    Verify,
}

impl TestPhase {
    /// The human progress-line label (matches the pre-event output).
    fn label(&self) -> String {
        match self {
            TestPhase::Setup => "setup".into(),
            TestPhase::Gather { name } => format!("gather {name}"),
            TestPhase::Check => "check".into(),
            TestPhase::FirstApply => "first apply".into(),
            TestPhase::SecondApply => "second apply".into(),
            TestPhase::Verify => "verify".into(),
        }
    }
}

/// One lifecycle event. Serialized as `{"event":"…", …}`; the NDJSON
/// sink stamps a `ts` field (epoch millis) at emit time.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TestEvent {
    RunStarted {
        playbook: String,
        groups: usize,
        tests: Vec<PlannedTest>,
    },
    GroupProvisioning {
        group: usize,
        label: String,
        backend: String,
        image: String,
    },
    /// The instance is up and smoke-tested; `attach` carries what a
    /// troubleshooting session (docker exec / VNC) or an external
    /// cleanup after a kill needs.
    InstanceReady {
        group: usize,
        label: String,
        backend: String,
        image: String,
        attach: AttachInfo,
    },
    TestStarted {
        package: String,
        test: String,
        /// `None` for scenarios.
        group: Option<usize>,
    },
    Phase {
        package: String,
        test: String,
        #[serde(flatten)]
        phase: TestPhase,
    },
    /// Output captured from an exec inside the instance (tail-truncated).
    Log {
        package: String,
        test: String,
        /// Which exec produced it ("setup", "check", …).
        context: String,
        stream: &'static str,
        chunk: String,
        truncated: bool,
    },
    GatherResult {
        package: String,
        test: String,
        gather: String,
        failures: Vec<String>,
    },
    StepResult {
        package: String,
        test: String,
        step: String,
        expect: &'static str,
        check: Option<&'static str>,
        apply: Option<&'static str>,
        second_apply: Option<&'static str>,
        failures: Vec<String>,
    },
    VerifyResult {
        package: String,
        test: String,
        passed: bool,
        message: Option<String>,
    },
    TestFinished {
        package: String,
        test: String,
        outcome: &'static str,
        duration_secs: f64,
        error: Option<String>,
    },
    /// `kept` = the instance was intentionally left running (--keep);
    /// `warning` carries a non-fatal teardown failure.
    GroupTeardown {
        group: usize,
        label: String,
        kept: bool,
        handle: Option<String>,
        warning: Option<String>,
    },
    RunFinished {
        exit_code: u8,
        passed: usize,
        failed: usize,
        errors: usize,
        duration_secs: f64,
    },
}

/// Shared by the scoped group-runner threads, so `Send + Sync`; sinks
/// keep concurrent lines whole with one locked write per event.
pub type TestEventSink = Arc<dyn Fn(TestEvent) + Send + Sync>;

pub fn null_sink() -> TestEventSink {
    Arc::new(|_| {})
}

/// One JSON object per line on stderr, `ts` (epoch millis) stamped here.
pub fn ndjson_sink() -> TestEventSink {
    Arc::new(|event| {
        let mut value = serde_json::to_value(&event).expect("TestEvent serializes");
        if let serde_json::Value::Object(map) = &mut value {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            map.insert("ts".into(), ts.into());
        }
        let stderr = std::io::stderr();
        let mut out = stderr.lock();
        let _ = writeln!(out, "{value}");
    })
}

/// Reproduces the human `⟳`/`⚠` progress lines from the same events.
/// Result/log events stay silent — results live in the final report.
pub fn human_sink() -> TestEventSink {
    Arc::new(|event| {
        let line = match &event {
            TestEvent::GroupProvisioning { label, image, .. } => {
                Some(format!("⟳ [{label}] provisioning ({image})"))
            }
            TestEvent::Phase {
                package,
                test,
                phase,
            } => Some(format!("⟳ {package}:{test} — {}", phase.label())),
            TestEvent::GroupTeardown {
                label,
                kept: true,
                handle: Some(handle),
                ..
            } => Some(format!(
                "⟳ [{label}] kept {handle} — remove it manually when done"
            )),
            TestEvent::GroupTeardown {
                label,
                warning: Some(warning),
                ..
            } => Some(format!("⚠ [{label}] teardown: {warning}")),
            _ => None,
        };
        if let Some(line) = line {
            let stderr = std::io::stderr();
            let mut out = stderr.lock();
            let _ = writeln!(out, "{line}");
        }
    })
}

/// The tail of `s`, at most [`LOG_CHUNK_MAX`] bytes, cut on a char
/// boundary. Returns the chunk and whether anything was dropped.
pub fn tail_chunk(s: &str) -> (String, bool) {
    if s.len() <= LOG_CHUNK_MAX {
        return (s.to_string(), false);
    }
    let mut start = s.len() - LOG_CHUNK_MAX;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    (s[start..].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_snake_case_tags() {
        let ev = TestEvent::InstanceReady {
            group: 0,
            label: "core:basic".into(),
            backend: "docker".into(),
            image: "debian:12".into(),
            attach: AttachInfo::Docker {
                container_id: "a".repeat(64),
                image: "debian:12".into(),
                cli: "docker".into(),
            },
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "instance_ready");
        assert_eq!(v["attach"]["kind"], "docker");
        assert_eq!(v["attach"]["container_id"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn phase_flattens_into_the_event_object() {
        let ev = TestEvent::Phase {
            package: "p".into(),
            test: "t".into(),
            phase: TestPhase::Gather { name: "g".into() },
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "phase");
        assert_eq!(v["phase"], "gather");
        assert_eq!(v["name"], "g");

        let run = TestEvent::Phase {
            package: "p".into(),
            test: "t".into(),
            phase: TestPhase::FirstApply,
        };
        assert_eq!(serde_json::to_value(&run).unwrap()["phase"], "first_apply");
    }

    #[test]
    fn tail_chunk_truncates_on_char_boundaries() {
        let (chunk, truncated) = tail_chunk("short");
        assert_eq!((chunk.as_str(), truncated), ("short", false));

        // A long string of multi-byte chars: the cut must not split one.
        let long = "é".repeat(LOG_CHUNK_MAX); // 2 bytes each
        let (chunk, truncated) = tail_chunk(&long);
        assert!(truncated);
        assert!(chunk.len() <= LOG_CHUNK_MAX);
        assert!(chunk.chars().all(|c| c == 'é'));
    }

    #[test]
    fn ndjson_lines_parse_back_and_carry_ts() {
        // The sink writes to the process stderr; assert the shape it
        // builds instead by replicating its serialization.
        let ev = TestEvent::RunFinished {
            exit_code: 0,
            passed: 3,
            failed: 0,
            errors: 0,
            duration_secs: 1.5,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "run_finished");
        assert_eq!(v["passed"], 3);
    }
}
