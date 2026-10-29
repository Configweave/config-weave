//! Wait steps: poll a probe until it reports ready, or give up.
//!
//! A wait step names a probe (`wait_for`) rather than a resource. Its
//! lifecycle is mode-aware like a resource's:
//!
//! - **check** runs the probe once and never waits. Ready reports
//!   `AlreadyConfigured`; not ready reports `NotConfigured`, naming how long
//!   apply would wait.
//! - **apply** runs the probe every `interval` until it returns true
//!   (`AlreadyConfigured` on the first attempt, `Configured` after any wait)
//!   or `timeout` passes (`Error`). A probe error counts as "not ready" and
//!   is kept for the timeout report, unless `fail_on_error` makes it fatal.
//!
//! The scheduler runs wait steps on their own threads, outside the worker
//! pool, and cancels them through [`Cancel`] when the run halts: a waiter
//! hours from its deadline must not hold the halt open.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use wscript_std::DynValue;

use super::status::{Mode, StepStatus};

/// How often a wait step polls when it declares no `interval`.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);

/// The shortest `interval` a wait step may declare.
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// The shortest `timeout` a wait step may declare.
pub const MIN_TIMEOUT: Duration = Duration::from_secs(1);

/// A wait step's polling policy, evaluated by the planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitSpec {
    pub interval: Duration,
    pub timeout: Duration,
    pub fail_on_error: bool,
}

/// Validate one wait field's value: `interval` and `timeout` are durations
/// (nanoseconds once converted) of at least a second, `fail_on_error` a
/// bool. The loader calls this on static values and the planner on every
/// value, so both report the same message.
pub fn check_field(name: &str, value: &DynValue) -> Result<(), String> {
    match name {
        "interval" | "timeout" => {
            duration_field(name, value)?;
            Ok(())
        }
        "fail_on_error" => match value {
            DynValue::Bool(_) => Ok(()),
            other => Err(format!(
                "'fail_on_error' must be a bool, got {}",
                crate::model::CoarseType::describe(other)
            )),
        },
        _ => Ok(()),
    }
}

fn duration_field(name: &str, value: &DynValue) -> Result<Duration, String> {
    let floor = if name == "interval" {
        MIN_INTERVAL
    } else {
        MIN_TIMEOUT
    };
    let DynValue::Int(ns) = value else {
        return Err(format!(
            "'{name}' must be a duration such as 30s or 2h, got {}",
            crate::model::CoarseType::describe(value)
        ));
    };
    let d = Duration::from_nanos(u64::try_from(*ns).unwrap_or(0));
    if d < floor {
        return Err(format!(
            "'{name}' must be at least {}, got {}",
            human(floor),
            human(d)
        ));
    }
    Ok(d)
}

/// Build the spec from the step's evaluated wait fields. A missing
/// `interval` takes [`DEFAULT_INTERVAL`]; a missing `timeout` is an error
/// the loader already reported, repeated here so the step fails rather
/// than waiting forever.
pub fn spec(
    interval: Option<&DynValue>,
    timeout: Option<&DynValue>,
    fail_on_error: Option<&DynValue>,
) -> Result<WaitSpec, String> {
    let interval = match interval {
        Some(v) => duration_field("interval", v)?,
        None => DEFAULT_INTERVAL,
    };
    let timeout = match timeout {
        Some(v) => duration_field("timeout", v)?,
        None => return Err("a wait step needs a 'timeout'".into()),
    };
    let fail_on_error = match fail_on_error {
        None => false,
        Some(DynValue::Bool(b)) => *b,
        Some(other) => {
            check_field("fail_on_error", other)?;
            false
        }
    };
    Ok(WaitSpec {
        interval,
        timeout,
        fail_on_error,
    })
}

/// A cancellation flag a sleeping waiter wakes on. The scheduler trips it
/// when the run halts; every waiter then returns at its next pause.
#[derive(Default)]
pub struct Cancel {
    cancelled: Mutex<bool>,
    wake: Condvar,
}

impl Cancel {
    pub fn new() -> Arc<Cancel> {
        Arc::new(Cancel::default())
    }

    pub fn cancel(&self) {
        let mut c = self.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        *c = true;
        self.wake.notify_all();
    }

    /// Sleep for `d` or until cancelled. Returns whether the wait was
    /// cancelled.
    pub fn sleep(&self, d: Duration) -> bool {
        let guard = self.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = self
            .wake
            .wait_timeout_while(guard, d, |cancelled| !*cancelled)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// One unsuccessful probe attempt, reported for live progress.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub attempt: u32,
    pub elapsed: Duration,
    /// The probe's error, when it failed rather than reporting not ready.
    pub error: Option<String>,
}

/// Run the wait lifecycle. `probe` runs the probe script once.
pub fn run(
    mut probe: impl FnMut() -> Result<bool, String>,
    spec: &WaitSpec,
    mode: Mode,
    cancel: &Cancel,
    on_attempt: &dyn Fn(&Attempt),
) -> (StepStatus, Option<String>) {
    let started = Instant::now();
    let deadline = started + spec.timeout;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let error = match probe() {
            Ok(true) if attempt == 1 => return (StepStatus::AlreadyConfigured, None),
            Ok(true) => {
                return (
                    StepStatus::Configured,
                    Some(format!(
                        "ready after {} ({attempt} attempts)",
                        human(started.elapsed())
                    )),
                );
            }
            Ok(false) => None,
            Err(e) if spec.fail_on_error => {
                return (StepStatus::Error, Some(format!("probe failed: {e}")));
            }
            Err(e) => Some(e),
        };
        if mode == Mode::Check {
            let state = match &error {
                Some(e) => format!("probe failed: {e}"),
                None => "not ready".to_string(),
            };
            return (
                StepStatus::NotConfigured,
                Some(format!(
                    "{state}; apply waits up to {}",
                    human(spec.timeout)
                )),
            );
        }
        let now = Instant::now();
        if now >= deadline {
            let last = match &error {
                Some(e) => format!("; last probe failed: {e}"),
                None => String::new(),
            };
            return (
                StepStatus::Error,
                Some(format!(
                    "not ready after {} ({attempt} attempts){last}",
                    human(started.elapsed())
                )),
            );
        }
        on_attempt(&Attempt {
            attempt,
            elapsed: now - started,
            error,
        });
        // The last pause is cut short so one final attempt lands on the
        // deadline rather than past it.
        if cancel.sleep(spec.interval.min(deadline - now)) {
            return (
                StepStatus::NotRun,
                Some(format!(
                    "wait cancelled after {} because the run halted",
                    human(started.elapsed())
                )),
            );
        }
    }
}

/// A duration as a person writes it: `2h`, `4m12s`, `45s`, `250ms`.
pub fn human(d: Duration) -> String {
    let secs = d.as_secs();
    if secs == 0 {
        return format!("{}ms", d.as_millis());
    }
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 || out.is_empty() {
        out.push_str(&format!("{s}s"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn quick(timeout_ms: u64) -> WaitSpec {
        WaitSpec {
            interval: Duration::from_millis(10),
            timeout: Duration::from_millis(timeout_ms),
            fail_on_error: false,
        }
    }

    fn ready_on(n: u32) -> impl FnMut() -> Result<bool, String> {
        let calls = Cell::new(0);
        move || {
            calls.set(calls.get() + 1);
            Ok(calls.get() >= n)
        }
    }

    #[test]
    fn ready_on_first_attempt_is_already_configured() {
        let c = Cancel::new();
        let (status, msg) = run(ready_on(1), &quick(1000), Mode::Apply, &c, &|_| {});
        assert_eq!(status, StepStatus::AlreadyConfigured);
        assert_eq!(msg, None);
    }

    #[test]
    fn ready_after_waiting_is_configured() {
        let c = Cancel::new();
        let seen = Cell::new(0);
        let (status, msg) = run(ready_on(3), &quick(5000), Mode::Apply, &c, &|a| {
            seen.set(a.attempt)
        });
        assert_eq!(status, StepStatus::Configured);
        assert!(msg.unwrap().contains("(3 attempts)"));
        assert_eq!(seen.get(), 2);
    }

    #[test]
    fn check_mode_never_waits() {
        let c = Cancel::new();
        let started = Instant::now();
        let spec = WaitSpec {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(7200),
            fail_on_error: false,
        };
        let (status, msg) = run(|| Ok(false), &spec, Mode::Check, &c, &|_| {});
        assert_eq!(status, StepStatus::NotConfigured);
        assert_eq!(msg.as_deref(), Some("not ready; apply waits up to 2h"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn timeout_is_an_error_carrying_the_last_probe_error() {
        let c = Cancel::new();
        let (status, msg) = run(
            || Err("connection refused".into()),
            &quick(50),
            Mode::Apply,
            &c,
            &|_| {},
        );
        assert_eq!(status, StepStatus::Error);
        let msg = msg.unwrap();
        assert!(msg.starts_with("not ready after"), "{msg}");
        assert!(
            msg.ends_with("last probe failed: connection refused"),
            "{msg}"
        );
    }

    #[test]
    fn fail_on_error_stops_at_the_first_probe_error() {
        let c = Cancel::new();
        let spec = WaitSpec {
            fail_on_error: true,
            ..quick(5000)
        };
        let calls = Cell::new(0);
        let (status, msg) = run(
            || {
                calls.set(calls.get() + 1);
                Err("boom".into())
            },
            &spec,
            Mode::Apply,
            &c,
            &|_| {},
        );
        assert_eq!(status, StepStatus::Error);
        assert_eq!(msg.as_deref(), Some("probe failed: boom"));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn cancel_wakes_a_sleeping_waiter() {
        let c = Cancel::new();
        let spec = WaitSpec {
            interval: Duration::from_secs(3600),
            timeout: Duration::from_secs(7200),
            fail_on_error: false,
        };
        let waker = Arc::clone(&c);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            waker.cancel();
        });
        let started = Instant::now();
        let (status, msg) = run(|| Ok(false), &spec, Mode::Apply, &c, &|_| {});
        t.join().unwrap();
        assert_eq!(status, StepStatus::NotRun);
        assert!(msg.unwrap().contains("the run halted"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn field_checks() {
        assert!(check_field("interval", &DynValue::Int(1_000_000_000)).is_ok());
        assert_eq!(
            check_field("interval", &DynValue::Int(500_000_000)).unwrap_err(),
            "'interval' must be at least 1s, got 500ms"
        );
        assert!(check_field("timeout", &DynValue::String("2h".into())).is_err());
        assert!(check_field("fail_on_error", &DynValue::Int(1)).is_err());
        assert!(spec(None, None, None).is_err());
        let s = spec(None, Some(&DynValue::Int(7_200_000_000_000)), None).unwrap();
        assert_eq!(s.interval, DEFAULT_INTERVAL);
        assert_eq!(s.timeout, Duration::from_secs(7200));
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(Duration::from_secs(7200)), "2h");
        assert_eq!(human(Duration::from_secs(252)), "4m12s");
        assert_eq!(human(Duration::from_secs(3661)), "1h1m1s");
        assert_eq!(human(Duration::from_millis(250)), "250ms");
    }
}
