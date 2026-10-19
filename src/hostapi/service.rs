//! The `service` module (PRD §7): query/start/stop services and set the
//! startup type, via the Service Control Manager. Windows-only in v1;
//! Linux uses `shell::run("systemctl …")`. Registered on every platform;
//! calls fail at runtime off Windows.
//!
//! `start`/`stop`/`restart` block until the service reaches the target
//! state, so the engine's post-apply re-check never sees a `*_pending`
//! state. The state/start-type mapping and the wait loop are plain
//! functions over raw SCM numbers so they are tested on every platform;
//! `win` only supplies the SCM calls.

use std::time::{Duration, Instant};

use wscript::Module;
use wscript_std::DynValue;

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the 'service' module is only available on Windows (v1); use \
                           shell::run(\"systemctl ...\") on Linux";

/// How long `start`/`stop` wait for the target state unless `opts.timeout`
/// says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

// SERVICE_STATUS.dwCurrentState values.
const STOPPED: u32 = 1;
const START_PENDING: u32 = 2;
const STOP_PENDING: u32 = 3;
const RUNNING: u32 = 4;
const CONTINUE_PENDING: u32 = 5;
const PAUSE_PENDING: u32 = 6;
const PAUSED: u32 = 7;

// QUERY_SERVICE_CONFIGW.dwStartType values.
const BOOT_START: u32 = 0;
const SYSTEM_START: u32 = 1;
const AUTO_START: u32 = 2;
const DEMAND_START: u32 = 3;
const DISABLED: u32 = 4;

fn state_name(state: u32) -> &'static str {
    match state {
        STOPPED => "stopped",
        START_PENDING => "start_pending",
        STOP_PENDING => "stop_pending",
        RUNNING => "running",
        CONTINUE_PENDING => "continue_pending",
        PAUSE_PENDING => "pause_pending",
        PAUSED => "paused",
        _ => "unknown",
    }
}

/// The name `startup()` reports. `delayed` only means anything for an
/// automatic service.
#[cfg_attr(not(windows), allow(dead_code))]
fn startup_name(start_type: u32, delayed: bool) -> &'static str {
    match start_type {
        AUTO_START if delayed => "automatic_delayed",
        AUTO_START => "automatic",
        DEMAND_START => "manual",
        DISABLED => "disabled",
        BOOT_START => "boot",
        SYSTEM_START => "system",
        _ => "other",
    }
}

/// The start type `set_startup` writes, and the delayed-start flag to set
/// alongside it (`None` leaves the flag alone; it only applies to automatic).
fn parse_startup(mode: &str) -> Result<(u32, Option<bool>), String> {
    Ok(match mode {
        "automatic" => (AUTO_START, Some(false)),
        "automatic_delayed" => (AUTO_START, Some(true)),
        "manual" => (DEMAND_START, None),
        "disabled" => (DISABLED, None),
        other => {
            return Err(format!(
                "unknown startup type '{other}' (automatic, automatic_delayed, manual, disabled)"
            ));
        }
    })
}

/// The `opts` argument of `start_with`/`stop_with`/`restart`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Opts {
    timeout: Duration,
    /// Stop running dependents first (`stop_with`/`restart` only).
    stop_dependents: bool,
}

/// Parse `opts`: null or a map with `timeout` (seconds) and, where
/// `dependents` allows it, `stop_dependents` (bool).
fn parse_opts(opts: &DynValue, dependents: bool) -> Result<Opts, String> {
    let mut out = Opts {
        timeout: DEFAULT_TIMEOUT,
        stop_dependents: false,
    };
    let expected = if dependents {
        "timeout, stop_dependents"
    } else {
        "timeout"
    };
    match opts {
        DynValue::Null => {}
        DynValue::Map(m) => {
            for (k, v) in m {
                match (k.as_str(), v) {
                    ("timeout", DynValue::Int(secs)) if *secs >= 0 => {
                        out.timeout = Duration::from_secs(*secs as u64);
                    }
                    ("timeout", DynValue::Float(secs)) if secs.is_finite() && *secs >= 0.0 => {
                        out.timeout = Duration::from_secs_f64(*secs);
                    }
                    ("timeout", other) => {
                        return Err(format!(
                            "service option 'timeout' must be a non-negative number of seconds, \
                             got {other:?}"
                        ));
                    }
                    ("stop_dependents", DynValue::Bool(b)) if dependents => {
                        out.stop_dependents = *b;
                    }
                    ("stop_dependents", other) if dependents => {
                        return Err(format!(
                            "service option 'stop_dependents' must be a bool, got {other:?}"
                        ));
                    }
                    (other, _) => {
                        return Err(format!(
                            "unknown service option '{other}' (expected {expected})"
                        ));
                    }
                }
            }
        }
        other => {
            return Err(format!(
                "service options must be a map or null, got {other:?}"
            ));
        }
    }
    Ok(out)
}

/// Stop `name`, first stopping each dependent in `deps` that is not
/// already stopped. `deps` is `(name, state)` in the order
/// EnumDependentServicesW returns them, the reverse of start order, which
/// is a safe stop order even when dependents depend on each other.
/// Returns the dependents it stopped, in stop order.
#[cfg_attr(not(windows), allow(dead_code))]
fn stop_tree(
    name: &str,
    deps: &[(String, u32)],
    mut converge: impl FnMut(&str, Target) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let mut stopped = Vec::new();
    for (dep, state) in deps {
        if *state != STOPPED {
            converge(dep, Target::Stopped)
                .map_err(|e| format!("stopping dependent '{dep}' of '{name}': {e}"))?;
            stopped.push(dep.clone());
        }
    }
    converge(name, Target::Stopped)?;
    Ok(stopped)
}

/// `restart` with `stop_dependents`: `stop_tree`, start `name`, then start
/// the dependents it stopped again in reverse stop order.
#[cfg_attr(not(windows), allow(dead_code))]
fn restart_tree(
    name: &str,
    deps: &[(String, u32)],
    mut converge: impl FnMut(&str, Target) -> Result<(), String>,
) -> Result<(), String> {
    let stopped = stop_tree(name, deps, &mut converge)?;
    converge(name, Target::Running)?;
    for dep in stopped.iter().rev() {
        converge(dep, Target::Running)
            .map_err(|e| format!("restarting dependent '{dep}' of '{name}': {e}"))?;
    }
    Ok(())
}

/// The names `dependents()` reports: sorted, so the result does not depend
/// on SCM ordering.
#[cfg_attr(not(windows), allow(dead_code))]
fn sorted_names(deps: &[(String, u32)]) -> Vec<String> {
    let mut names: Vec<String> = deps.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    names
}

/// The state a converging call drives a service to.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Running,
    Stopped,
}

/// The SCM call that moves a service toward its target.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Start,
    Continue,
    Stop,
}

/// What to do on observing `state` while driving toward `target`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Plan {
    Done,
    Act(Action),
    /// A `*_pending` (or unknown) state: wait for it to settle.
    Wait,
}

fn plan(target: Target, state: u32) -> Plan {
    match (target, state) {
        (Target::Running, RUNNING) | (Target::Stopped, STOPPED) => Plan::Done,
        (Target::Running, STOPPED) => Plan::Act(Action::Start),
        (Target::Running, PAUSED) => Plan::Act(Action::Continue),
        (Target::Stopped, RUNNING | PAUSED) => Plan::Act(Action::Stop),
        _ => Plan::Wait,
    }
}

/// Sleep between polls: a tenth of the service's `dwWaitHint`, as the SCM
/// documentation suggests, clamped to 100ms..1s.
fn poll_interval(wait_hint_ms: u32) -> Duration {
    Duration::from_millis(u64::from(wait_hint_ms / 10).clamp(100, 1000))
}

/// One `QueryServiceStatusEx` observation.
#[derive(Clone, Copy, Debug)]
struct Observed {
    state: u32,
    wait_hint_ms: u32,
}

/// Drive a service to `target`: wait out any pending state, issue at most
/// one action, then poll until the target state or `timeout`. Fails early
/// when, after acting and seeing it pending, the service falls back to a
/// state that would need the action again (e.g. it crashed while starting).
/// The SCM, clock and sleep are parameters so the loop is testable anywhere.
fn drive(
    name: &str,
    target: Target,
    timeout: Duration,
    mut query: impl FnMut() -> Result<Observed, String>,
    mut act: impl FnMut(Action) -> Result<(), String>,
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> Result<(), String> {
    let mut acted = false;
    let mut pending_since_act = false;
    loop {
        let obs = query()?;
        match plan(target, obs.state) {
            Plan::Done => return Ok(()),
            Plan::Act(action) if !acted => {
                act(action)?;
                acted = true;
                continue;
            }
            Plan::Act(_) if pending_since_act => {
                return Err(format!(
                    "service '{name}' went back to {} instead of becoming {}",
                    state_name(obs.state),
                    target_name(target)
                ));
            }
            Plan::Act(_) => {}
            Plan::Wait => pending_since_act |= acted,
        }
        let spent = elapsed();
        if spent >= timeout {
            return Err(format!(
                "timed out after {}s waiting for service '{name}' to become {} \
                 (last state: {})",
                timeout.as_secs_f64(),
                target_name(target),
                state_name(obs.state)
            ));
        }
        sleep(poll_interval(obs.wait_hint_ms).min(timeout - spent));
    }
}

fn target_name(target: Target) -> &'static str {
    match target {
        Target::Running => "running",
        Target::Stopped => "stopped",
    }
}

/// `drive` against the real clock.
#[cfg_attr(not(windows), allow(dead_code))]
fn drive_now(
    name: &str,
    target: Target,
    timeout: Duration,
    query: impl FnMut() -> Result<Observed, String>,
    act: impl FnMut(Action) -> Result<(), String>,
) -> Result<(), String> {
    let began = Instant::now();
    drive(
        name,
        target,
        timeout,
        query,
        act,
        || began.elapsed(),
        std::thread::sleep,
    )
}

pub fn module() -> Module {
    let mut m = Module::new("service");
    m.doc("Windows service management via the SCM (Windows only in v1)");

    m.doc_next(
        "Service status: running | stopped | start_pending | stop_pending | paused | \
         pause_pending | continue_pending",
    );
    #[cfg(windows)]
    m.fn_("status", |name: &str| -> Result<String, String> {
        win::status(name)
    });
    #[cfg(not(windows))]
    m.fn_("status", |_: &str| -> Result<String, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next(
        "Start a service and wait (60s) until it is running. Waits out pending states, \
         continues a paused service; no-op when already running",
    );
    #[cfg(windows)]
    m.fn_("start", |name: &str| -> Result<(), String> {
        win::converge(name, Target::Running, DEFAULT_TIMEOUT)
    });
    #[cfg(not(windows))]
    m.fn_("start", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Like start. opts: timeout (secs, default 60)");
    #[cfg(windows)]
    m.fn_(
        "start_with",
        |name: &str, opts: DynValue| -> Result<(), String> {
            win::converge(name, Target::Running, parse_opts(&opts, false)?.timeout)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "start_with",
        |_: &str, opts: DynValue| -> Result<(), String> {
            parse_opts(&opts, false)?;
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next(
        "Stop a service and wait (60s) until it is stopped. Waits out pending states; \
         no-op when already stopped",
    );
    #[cfg(windows)]
    m.fn_("stop", |name: &str| -> Result<(), String> {
        win::converge(name, Target::Stopped, DEFAULT_TIMEOUT)
    });
    #[cfg(not(windows))]
    m.fn_("stop", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next(
        "Like stop. opts: timeout (secs, default 60), stop_dependents (bool, default false: \
         stop running dependents first)",
    );
    #[cfg(windows)]
    m.fn_(
        "stop_with",
        |name: &str, opts: DynValue| -> Result<(), String> {
            let opts = parse_opts(&opts, true)?;
            if !opts.stop_dependents {
                return win::converge(name, Target::Stopped, opts.timeout);
            }
            let deps = win::dependents(name)?;
            stop_tree(name, &deps, |n, t| win::converge(n, t, opts.timeout)).map(|_| ())
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "stop_with",
        |_: &str, opts: DynValue| -> Result<(), String> {
            parse_opts(&opts, true)?;
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next(
        "Stop then start a service, waiting for each state. opts: timeout (secs per phase, \
         default 60), stop_dependents (bool, default false: stop running dependents first, \
         start them again afterwards)",
    );
    #[cfg(windows)]
    m.fn_(
        "restart",
        |name: &str, opts: DynValue| -> Result<(), String> {
            let opts = parse_opts(&opts, true)?;
            let deps = if opts.stop_dependents {
                win::dependents(name)?
            } else {
                Vec::new()
            };
            restart_tree(name, &deps, |n, t| win::converge(n, t, opts.timeout))
        },
    );
    #[cfg(not(windows))]
    m.fn_("restart", |_: &str, opts: DynValue| -> Result<(), String> {
        parse_opts(&opts, true)?;
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next(
        "Names of the services that depend on a service, directly or through another \
         dependent, sorted",
    );
    #[cfg(windows)]
    m.fn_("dependents", |name: &str| -> Result<Vec<String>, String> {
        Ok(sorted_names(&win::dependents(name)?))
    });
    #[cfg(not(windows))]
    m.fn_("dependents", |_: &str| -> Result<Vec<String>, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Set startup type: automatic | automatic_delayed | manual | disabled");
    #[cfg(windows)]
    m.fn_(
        "set_startup",
        |name: &str, mode: &str| -> Result<(), String> { win::set_startup(name, mode) },
    );
    #[cfg(not(windows))]
    m.fn_("set_startup", |_: &str, mode: &str| -> Result<(), String> {
        parse_startup(mode)?;
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next(
        "Startup type of a service: automatic | automatic_delayed | manual | disabled | \
         boot | system",
    );
    #[cfg(windows)]
    m.fn_("startup", |name: &str| -> Result<String, String> {
        win::startup(name)
    });
    #[cfg(not(windows))]
    m.fn_("startup", |_: &str| -> Result<String, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m
}

#[cfg(windows)]
mod win {
    use std::time::Duration;

    use windows::Win32::Foundation::ERROR_MORE_DATA;
    use windows::Win32::System::Services::{
        ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
        ENUM_SERVICE_STATUSW, ENUM_SERVICE_TYPE, EnumDependentServicesW, OpenSCManagerW,
        OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfig2W, QueryServiceConfigW,
        QueryServiceStatusEx, SC_HANDLE, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO,
        SERVICE_CHANGE_CONFIG, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONTROL_CONTINUE,
        SERVICE_CONTROL_STOP, SERVICE_DELAYED_AUTO_START_INFO, SERVICE_ENUMERATE_DEPENDENTS,
        SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_PAUSE_CONTINUE, SERVICE_QUERY_CONFIG,
        SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_START_TYPE, SERVICE_STATE_ALL, SERVICE_STATUS,
        SERVICE_STATUS_PROCESS, SERVICE_STOP, StartServiceW,
    };
    use windows::core::{BOOL, HSTRING, PCWSTR};

    use super::{Action, Observed, Target, drive_now, parse_startup, startup_name, state_name};

    struct Handle(SC_HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }

    fn open(name: &str, access: u32) -> Result<(Handle, Handle), String> {
        unsafe {
            let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
                .map_err(|e| format!("opening the service control manager: {e}"))?;
            let scm = Handle(scm);
            let wide = HSTRING::from(name);
            let svc = OpenServiceW(scm.0, PCWSTR(wide.as_ptr()), access)
                .map_err(|e| format!("opening service '{name}': {e}"))?;
            Ok((scm, Handle(svc)))
        }
    }

    fn query(name: &str, svc: &Handle) -> Result<Observed, String> {
        let mut st = SERVICE_STATUS_PROCESS::default();
        let mut needed = 0u32;
        unsafe {
            let buf = std::slice::from_raw_parts_mut(
                (&mut st as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
                size_of::<SERVICE_STATUS_PROCESS>(),
            );
            QueryServiceStatusEx(svc.0, SC_STATUS_PROCESS_INFO, Some(buf), &mut needed)
                .map_err(|e| format!("querying '{name}': {e}"))?;
        }
        Ok(Observed {
            state: st.dwCurrentState.0,
            wait_hint_ms: st.dwWaitHint,
        })
    }

    pub fn status(name: &str) -> Result<String, String> {
        let (_scm, svc) = open(name, SERVICE_QUERY_STATUS)?;
        Ok(state_name(query(name, &svc)?.state).to_string())
    }

    /// Drive `name` to `target`. Polls through a query-only handle; the
    /// start/stop/continue handle is opened only when an action is needed,
    /// so a no-op call works without control rights.
    pub fn converge(name: &str, target: Target, timeout: Duration) -> Result<(), String> {
        let (_scm, svc) = open(name, SERVICE_QUERY_STATUS)?;
        drive_now(
            name,
            target,
            timeout,
            || query(name, &svc),
            |action| act(name, action),
        )
    }

    fn act(name: &str, action: Action) -> Result<(), String> {
        let access = match action {
            Action::Start => SERVICE_START,
            Action::Continue => SERVICE_PAUSE_CONTINUE,
            Action::Stop => SERVICE_STOP,
        };
        let (_scm, svc) = open(name, access)?;
        let mut st = SERVICE_STATUS::default();
        unsafe {
            match action {
                Action::Start => {
                    StartServiceW(svc.0, None).map_err(|e| format!("starting '{name}': {e}"))
                }
                Action::Continue => ControlService(svc.0, SERVICE_CONTROL_CONTINUE, &mut st)
                    .map_err(|e| format!("continuing '{name}': {e}")),
                Action::Stop => ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st)
                    .map_err(|e| format!("stopping '{name}': {e}")),
            }
        }
    }

    /// `(name, state)` of every service that depends on `name`, in the
    /// SCM's order (reverse start order, so a safe stop order).
    pub fn dependents(name: &str) -> Result<Vec<(String, u32)>, String> {
        let (_scm, svc) = open(name, SERVICE_ENUMERATE_DEPENDENTS)?;
        let mut needed = 0u32;
        let mut count = 0u32;
        // Entries point at strings later in the same buffer, so it is sized
        // in whole entries to keep it aligned. Starts empty: the first call
        // sizes it (ERROR_MORE_DATA), and succeeds outright with no dependents.
        let entry = size_of::<ENUM_SERVICE_STATUSW>();
        let mut buf: Vec<ENUM_SERVICE_STATUSW> = Vec::new();
        unsafe {
            loop {
                let ptr = (!buf.is_empty()).then_some(buf.as_mut_ptr());
                match EnumDependentServicesW(
                    svc.0,
                    SERVICE_STATE_ALL,
                    ptr,
                    (buf.len() * entry) as u32,
                    &mut needed,
                    &mut count,
                ) {
                    Ok(()) => break,
                    // Also covers a dependent added between the two calls.
                    Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => {
                        buf = vec![
                            ENUM_SERVICE_STATUSW::default();
                            (needed as usize).div_ceil(entry)
                        ];
                    }
                    Err(e) => return Err(format!("listing dependents of '{name}': {e}")),
                }
            }
            buf[..count as usize]
                .iter()
                .map(|s| {
                    let dep = s
                        .lpServiceName
                        .to_string()
                        .map_err(|e| format!("reading a dependent name of '{name}': {e}"))?;
                    Ok((dep, s.ServiceStatus.dwCurrentState.0))
                })
                .collect()
        }
    }

    pub fn set_startup(name: &str, mode: &str) -> Result<(), String> {
        let (ty, delayed) = parse_startup(mode)?;
        let (_scm, svc) = open(name, SERVICE_CHANGE_CONFIG)?;
        unsafe {
            ChangeServiceConfigW(
                svc.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                SERVICE_START_TYPE(ty),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
            .map_err(|e| format!("configuring '{name}': {e}"))?;
            if let Some(delayed) = delayed {
                let info = SERVICE_DELAYED_AUTO_START_INFO {
                    fDelayedAutostart: BOOL::from(delayed),
                };
                ChangeServiceConfig2W(
                    svc.0,
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    Some((&info as *const SERVICE_DELAYED_AUTO_START_INFO).cast()),
                )
                .map_err(|e| format!("setting delayed start of '{name}': {e}"))?;
            }
        }
        Ok(())
    }

    pub fn startup(name: &str) -> Result<String, String> {
        let (_scm, svc) = open(name, SERVICE_QUERY_CONFIG)?;
        let mut needed = 0u32;
        unsafe {
            // First call sizes the buffer.
            let _ = QueryServiceConfigW(svc.0, None, 0, &mut needed);
            let mut buf = vec![0u8; needed as usize];
            let config = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            QueryServiceConfigW(svc.0, Some(config), needed, &mut needed)
                .map_err(|e| format!("querying config of '{name}': {e}"))?;
            let start_type = (*config).dwStartType.0;

            let mut info = SERVICE_DELAYED_AUTO_START_INFO::default();
            let buf = std::slice::from_raw_parts_mut(
                (&mut info as *mut SERVICE_DELAYED_AUTO_START_INFO).cast::<u8>(),
                size_of::<SERVICE_DELAYED_AUTO_START_INFO>(),
            );
            QueryServiceConfig2W(
                svc.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                Some(buf),
                &mut needed,
            )
            .map_err(|e| format!("querying delayed start of '{name}': {e}"))?;
            Ok(startup_name(start_type, info.fDelayedAutostart.as_bool()).to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    use super::*;

    /// Run `drive` against a scripted SCM: each query returns the next state
    /// (the last one repeats), each sleep advances a fake clock.
    fn run(target: Target, states: &[u32], timeout: Duration) -> (Result<(), String>, Vec<Action>) {
        let next = Cell::new(0usize);
        let clock = Cell::new(Duration::ZERO);
        let actions = RefCell::new(Vec::new());
        let result = drive(
            "svc",
            target,
            timeout,
            || {
                let i = next.get();
                next.set(i + 1);
                Ok(Observed {
                    state: states[i.min(states.len() - 1)],
                    wait_hint_ms: 2000,
                })
            },
            |action| {
                actions.borrow_mut().push(action);
                Ok(())
            },
            || clock.get(),
            |d| clock.set(clock.get() + d),
        );
        (result, actions.into_inner())
    }

    fn map(pairs: &[(&str, DynValue)]) -> DynValue {
        DynValue::Map(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn names_every_state() {
        assert_eq!(state_name(STOPPED), "stopped");
        assert_eq!(state_name(START_PENDING), "start_pending");
        assert_eq!(state_name(PAUSED), "paused");
        assert_eq!(state_name(99), "unknown");
    }

    #[test]
    fn startup_distinguishes_delayed_boot_and_system() {
        assert_eq!(startup_name(AUTO_START, false), "automatic");
        assert_eq!(startup_name(AUTO_START, true), "automatic_delayed");
        assert_eq!(startup_name(DEMAND_START, true), "manual");
        assert_eq!(startup_name(DISABLED, false), "disabled");
        assert_eq!(startup_name(BOOT_START, false), "boot");
        assert_eq!(startup_name(SYSTEM_START, false), "system");
        assert_eq!(startup_name(42, false), "other");
    }

    #[test]
    fn set_startup_modes_drive_the_delayed_flag() {
        assert_eq!(parse_startup("automatic"), Ok((AUTO_START, Some(false))));
        assert_eq!(
            parse_startup("automatic_delayed"),
            Ok((AUTO_START, Some(true)))
        );
        assert_eq!(parse_startup("manual"), Ok((DEMAND_START, None)));
        assert_eq!(parse_startup("disabled"), Ok((DISABLED, None)));
        assert!(parse_startup("boot").is_err());
    }

    fn timeout(opts: &DynValue) -> Result<Duration, String> {
        parse_opts(opts, false).map(|o| o.timeout)
    }

    #[test]
    fn timeout_defaults_and_parses() {
        assert_eq!(timeout(&DynValue::Null), Ok(DEFAULT_TIMEOUT));
        assert_eq!(
            timeout(&map(&[("timeout", DynValue::Int(5))])),
            Ok(Duration::from_secs(5))
        );
        assert_eq!(
            timeout(&map(&[("timeout", DynValue::Float(1.5))])),
            Ok(Duration::from_millis(1500))
        );
        assert!(timeout(&map(&[("timeout", DynValue::Int(-1))])).is_err());
        assert!(timeout(&map(&[("wait", DynValue::Int(1))])).is_err());
        assert!(timeout(&DynValue::Int(5)).is_err());
    }

    #[test]
    fn stop_dependents_defaults_off_and_parses_where_allowed() {
        assert_eq!(
            parse_opts(&DynValue::Null, true),
            Ok(Opts {
                timeout: DEFAULT_TIMEOUT,
                stop_dependents: false
            })
        );
        assert_eq!(
            parse_opts(
                &map(&[
                    ("stop_dependents", DynValue::Bool(true)),
                    ("timeout", DynValue::Int(3)),
                ]),
                true
            ),
            Ok(Opts {
                timeout: Duration::from_secs(3),
                stop_dependents: true
            })
        );
        let err = parse_opts(&map(&[("stop_dependents", DynValue::Int(1))]), true).unwrap_err();
        assert!(err.contains("must be a bool"), "{err}");
        // start_with does not take it.
        let err =
            parse_opts(&map(&[("stop_dependents", DynValue::Bool(true))]), false).unwrap_err();
        assert!(
            err.contains("unknown service option 'stop_dependents'"),
            "{err}"
        );
        assert!(err.contains("(expected timeout)"), "{err}");
        let err = parse_opts(&map(&[("wait", DynValue::Int(1))]), true).unwrap_err();
        assert!(err.contains("(expected timeout, stop_dependents)"), "{err}");
    }

    /// A fake SCM of several services: `converge` runs the real `drive`
    /// loop against it, and each action settles the service at once.
    /// Records every SCM action as `(service, action)`.
    struct FakeScm {
        states: RefCell<HashMap<String, u32>>,
        log: RefCell<Vec<(String, Action)>>,
    }

    impl FakeScm {
        fn new(services: &[(&str, u32)]) -> Self {
            FakeScm {
                states: RefCell::new(services.iter().map(|(n, s)| (n.to_string(), *s)).collect()),
                log: RefCell::new(Vec::new()),
            }
        }

        fn converge(&self, name: &str, target: Target) -> Result<(), String> {
            drive(
                name,
                target,
                DEFAULT_TIMEOUT,
                || {
                    Ok(Observed {
                        state: self.states.borrow()[name],
                        wait_hint_ms: 0,
                    })
                },
                |action| {
                    self.log.borrow_mut().push((name.to_string(), action));
                    let next = if action == Action::Stop {
                        STOPPED
                    } else {
                        RUNNING
                    };
                    self.states.borrow_mut().insert(name.to_string(), next);
                    Ok(())
                },
                || Duration::ZERO,
                |_| {},
            )
        }

        fn deps(&self, names: &[&str]) -> Vec<(String, u32)> {
            names
                .iter()
                .map(|n| (n.to_string(), self.states.borrow()[*n]))
                .collect()
        }

        fn log(&self) -> Vec<(String, Action)> {
            self.log.borrow().clone()
        }
    }

    fn acts(pairs: &[(&str, Action)]) -> Vec<(String, Action)> {
        pairs.iter().map(|(n, a)| (n.to_string(), *a)).collect()
    }

    #[test]
    fn stop_tree_stops_running_dependents_in_scm_order_first() {
        let scm = FakeScm::new(&[
            ("svc", RUNNING),
            ("outer", RUNNING),
            ("idle", STOPPED),
            ("inner", PAUSED),
        ]);
        // SCM order: reverse start order, so `outer` (which depends on
        // `inner`) comes first.
        let deps = scm.deps(&["outer", "idle", "inner"]);
        let stopped = stop_tree("svc", &deps, |n, t| scm.converge(n, t)).unwrap();
        assert_eq!(stopped, vec!["outer".to_string(), "inner".to_string()]);
        assert_eq!(
            scm.log(),
            acts(&[
                ("outer", Action::Stop),
                ("inner", Action::Stop),
                ("svc", Action::Stop)
            ])
        );
    }

    #[test]
    fn stop_tree_with_no_dependents_is_a_plain_stop() {
        let scm = FakeScm::new(&[("svc", RUNNING)]);
        let stopped = stop_tree("svc", &[], |n, t| scm.converge(n, t)).unwrap();
        assert!(stopped.is_empty());
        assert_eq!(scm.log(), acts(&[("svc", Action::Stop)]));
    }

    #[test]
    fn restart_tree_starts_only_the_stopped_dependents_in_reverse() {
        let scm = FakeScm::new(&[
            ("svc", RUNNING),
            ("outer", RUNNING),
            ("idle", STOPPED),
            ("inner", RUNNING),
        ]);
        let deps = scm.deps(&["outer", "idle", "inner"]);
        restart_tree("svc", &deps, |n, t| scm.converge(n, t)).unwrap();
        assert_eq!(
            scm.log(),
            acts(&[
                ("outer", Action::Stop),
                ("inner", Action::Stop),
                ("svc", Action::Stop),
                ("svc", Action::Start),
                ("inner", Action::Start),
                ("outer", Action::Start),
            ])
        );
        assert_eq!(scm.states.borrow()["idle"], STOPPED);
    }

    #[test]
    fn restart_tree_without_dependents_is_stop_then_start() {
        let scm = FakeScm::new(&[("svc", RUNNING)]);
        restart_tree("svc", &[], |n, t| scm.converge(n, t)).unwrap();
        assert_eq!(
            scm.log(),
            acts(&[("svc", Action::Stop), ("svc", Action::Start)])
        );
    }

    #[test]
    fn a_failing_dependent_stops_the_tree_and_is_named() {
        let scm = FakeScm::new(&[("svc", RUNNING), ("dep", RUNNING)]);
        let deps = scm.deps(&["dep"]);
        let err = stop_tree("svc", &deps, |n, t| {
            if n == "dep" {
                Err("access denied".to_string())
            } else {
                scm.converge(n, t)
            }
        })
        .unwrap_err();
        assert!(err.contains("stopping dependent 'dep' of 'svc'"), "{err}");
        assert!(scm.log().is_empty(), "svc must not be stopped");
    }

    #[test]
    fn dependent_names_are_sorted() {
        let deps = vec![
            ("b".to_string(), RUNNING),
            ("a".to_string(), STOPPED),
            ("c".to_string(), RUNNING),
        ];
        assert_eq!(sorted_names(&deps), vec!["a", "b", "c"]);
    }

    #[test]
    fn poll_interval_is_a_tenth_of_the_hint_clamped() {
        assert_eq!(poll_interval(0), Duration::from_millis(100));
        assert_eq!(poll_interval(5000), Duration::from_millis(500));
        assert_eq!(poll_interval(60_000), Duration::from_secs(1));
    }

    #[test]
    fn start_waits_until_running() {
        let (r, acts) = run(
            Target::Running,
            &[STOPPED, START_PENDING, START_PENDING, RUNNING],
            DEFAULT_TIMEOUT,
        );
        assert_eq!(r, Ok(()));
        assert_eq!(acts, vec![Action::Start]);
    }

    #[test]
    fn start_on_running_is_a_no_op() {
        let (r, acts) = run(Target::Running, &[RUNNING], DEFAULT_TIMEOUT);
        assert_eq!(r, Ok(()));
        assert!(acts.is_empty());
    }

    #[test]
    fn start_continues_a_paused_service() {
        let (r, acts) = run(
            Target::Running,
            &[PAUSED, CONTINUE_PENDING, RUNNING],
            DEFAULT_TIMEOUT,
        );
        assert_eq!(r, Ok(()));
        assert_eq!(acts, vec![Action::Continue]);
    }

    #[test]
    fn start_waits_out_a_pending_stop_first() {
        let (r, acts) = run(
            Target::Running,
            &[STOP_PENDING, STOP_PENDING, STOPPED, START_PENDING, RUNNING],
            DEFAULT_TIMEOUT,
        );
        assert_eq!(r, Ok(()));
        assert_eq!(acts, vec![Action::Start]);
    }

    #[test]
    fn stop_on_stopped_is_a_no_op() {
        let (r, acts) = run(Target::Stopped, &[STOPPED], DEFAULT_TIMEOUT);
        assert_eq!(r, Ok(()));
        assert!(acts.is_empty());
    }

    #[test]
    fn stop_waits_out_a_pending_start_then_stops() {
        let (r, acts) = run(
            Target::Stopped,
            &[START_PENDING, RUNNING, STOP_PENDING, STOPPED],
            DEFAULT_TIMEOUT,
        );
        assert_eq!(r, Ok(()));
        assert_eq!(acts, vec![Action::Stop]);
    }

    #[test]
    fn times_out_naming_the_last_state() {
        let (r, acts) = run(
            Target::Stopped,
            &[RUNNING, STOP_PENDING],
            Duration::from_secs(3),
        );
        let err = r.unwrap_err();
        assert!(err.contains("timed out after 3s"), "{err}");
        assert!(err.contains("last state: stop_pending"), "{err}");
        assert_eq!(acts, vec![Action::Stop]);
    }

    #[test]
    fn fails_when_the_service_falls_back_after_acting() {
        let (r, acts) = run(
            Target::Running,
            &[STOPPED, START_PENDING, STOPPED],
            DEFAULT_TIMEOUT,
        );
        let err = r.unwrap_err();
        assert!(err.contains("went back to stopped"), "{err}");
        assert_eq!(acts, vec![Action::Start]);
    }
}
