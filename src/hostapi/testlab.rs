//! The `testlab` wscript host module: the driver API a scenario script gets.
//!
//! A scenario is `fn run(lab: Lab) -> Result[bool, string]`. The script
//! provisions VMs/containers (`lab.provision`), applies config-weave inside
//! them (`machine.apply_resource` / `apply`), reboots (`machine.reboot`),
//! and inspects results — so multi-stage flows (set up a DC, reboot, apply
//! again to finish) are expressed directly. All state lives behind opaque
//! `Rc<RefCell<…>>` handles; scenarios run single-threaded host-side, so
//! `Rc` (not `Arc`) is correct.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use wscript::{Module, Script};
use wscript_std::DynValue;

use crate::diag::Diag;
use crate::model::Playbook;
use crate::report::JsonRunReport;
use crate::testlab::backend::{GuestOs, TestInstance, TestLab};
use crate::testlab::runner::GuestPaths;
use crate::testlab::synth::{self, BinaryResolver};

// --------------------------------------------------------------- data types

/// Outcome of one resource step inside a machine.
#[derive(Script, Clone)]
pub struct StepResult {
    /// `StepStatus::id()` form: not_configured | configured |
    /// already_configured | reboot_required | error | skipped | not_run.
    pub status: String,
    pub message: String,
    /// False only when the step errored.
    pub ok: bool,
}

/// Result of `machine.exec`.
#[derive(Script, Clone)]
pub struct ExecOut {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
}

/// A whole-playbook run report, queried by step name.
#[derive(Script)]
#[script(name = "RunReport")]
#[script(opaque)]
pub struct RunReport {
    ok: bool,
    steps: Vec<(String, StepResult)>,
}

// ----------------------------------------------------------------- handles

/// The lab handle a scenario's `run(lab)` receives.
#[derive(Script)]
#[script(name = "Lab")]
#[script(opaque)]
pub struct Lab {
    state: Rc<RefCell<LabState>>,
}

/// A provisioned machine handle.
#[derive(Script)]
#[script(name = "Machine")]
#[script(opaque)]
pub struct Machine {
    state: Rc<RefCell<LabState>>,
    name: String,
}

// ------------------------------------------------------------------- state

struct MachineState {
    instance: Box<dyn TestInstance>,
    /// Binary copied in + smoke-tested.
    prepared: bool,
    /// Per-machine working-dir counter for synthesized applies.
    counter: usize,
}

/// Everything a running scenario shares. Lives behind every handle's `Rc`.
pub struct LabState {
    lab: Box<dyn TestLab>,
    playbook: Rc<Playbook>,
    /// The scenario package's directory, for resolving `apply(dir)` paths.
    pkg_dir: PathBuf,
    binaries: BinaryResolver,
    machines: HashMap<String, MachineState>,
    quiet: bool,
}

impl LabState {
    pub fn new(
        lab: Box<dyn TestLab>,
        playbook: Rc<Playbook>,
        pkg_dir: PathBuf,
        binaries: BinaryResolver,
        quiet: bool,
    ) -> LabState {
        LabState {
            lab,
            playbook,
            pkg_dir,
            binaries,
            machines: HashMap::new(),
            quiet,
        }
    }

    /// Tear the lab down (called by the runner after `run` returns).
    pub fn teardown(&mut self) -> Result<(), Diag> {
        self.lab.teardown()
    }

    pub fn handle(&self) -> String {
        self.lab.handle()
    }
}

/// Wrap shared state in a `Lab` value to pass into `run(lab)`.
pub fn lab_value(state: Rc<RefCell<LabState>>) -> Lab {
    Lab { state }
}

// --------------------------------------------------------------- internals

fn ds(d: Diag) -> String {
    d.message
}

/// Per-apply guest paths: a fresh working dir under `/weave/s` (or
/// `C:/weave/s`), so successive applies on one machine never collide.
fn scenario_dir(os: GuestOs, name: &str, n: usize) -> (String, String) {
    let root = match os {
        GuestOs::Linux => "/weave/s",
        GuestOs::Windows => "C:/weave/s",
    };
    let dir = format!("{root}/{name}-{n}");
    let playbook = format!("{dir}/playbook");
    (dir, playbook)
}

fn mkdir_guest(instance: &dyn TestInstance, os: GuestOs, dir: &str) -> Result<(), Diag> {
    let out = match os {
        GuestOs::Linux => instance.exec(&["mkdir", "-p", dir])?,
        GuestOs::Windows => {
            let win = dir.replace('/', "\\");
            let script = format!("if not exist {win} md {win}");
            instance.exec(&["cmd.exe", "/C", &script])?
        }
    };
    if out.exit_code != 0 {
        return Err(Diag::bare(format!(
            "cannot create guest dir {dir} (exit {}): {}",
            out.exit_code,
            out.stderr.trim()
        )));
    }
    Ok(())
}

/// Copy the config-weave binary into a machine and smoke-test it, once.
fn ensure_prepared(state: &mut LabState, name: &str) -> Result<(), Diag> {
    let os = state
        .machines
        .get(name)
        .ok_or_else(|| Diag::bare(format!("no machine '{name}'")))?
        .instance
        .os();
    if state.machines[name].prepared {
        return Ok(());
    }
    let bin = GuestPaths::bin_for(os);
    let host = state.binaries.resolve(os)?;
    let ms = state.machines.get_mut(name).unwrap();
    ms.instance.copy_in(&host, bin)?;
    if os == GuestOs::Linux {
        let _ = ms.instance.exec(&["chmod", "+x", bin]);
    }
    let smoke = ms.instance.exec(&[bin, "version"])?;
    if smoke.exit_code != 0 {
        return Err(Diag::bare(format!(
            "the config-weave binary failed to run inside '{name}' (exit {}): {} — \
             host/image architecture mismatch?",
            smoke.exit_code,
            smoke.stderr.trim()
        )));
    }
    ms.prepared = true;
    Ok(())
}

fn step_result(js: Option<&crate::report::JsonRunStep>) -> StepResult {
    match js {
        Some(s) => StepResult {
            status: s.status.clone(),
            message: s.message.clone().unwrap_or_default(),
            ok: s.status != "error",
        },
        None => StepResult {
            status: "not_run".to_string(),
            message: "step missing from the run report".to_string(),
            ok: false,
        },
    }
}

/// Run `config-weave {mode} <dir> <play> --json` in a machine and return
/// the parsed report.
fn run_in_guest(
    instance: &dyn TestInstance,
    bin: &str,
    mode: &str,
    playbook: &str,
    play: Option<&str>,
) -> Result<JsonRunReport, Diag> {
    let mut argv = vec![bin, mode, playbook];
    if let Some(p) = play {
        argv.push(p);
    }
    argv.extend(["--json", "--continue-on-error"]);
    let out = instance.exec(&argv)?;
    serde_json::from_str(out.stdout.trim()).map_err(|_| {
        let tail = if out.stderr.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        };
        Diag::bare(format!(
            "the {mode} run produced no parseable report (exit {}): {}",
            out.exit_code,
            tail.trim()
        ))
    })
}

/// Apply or check a single synthesized resource and return its step result.
fn apply_resource(
    state: &mut LabState,
    name: &str,
    key: &str,
    props: &DynValue,
    mode: &str,
) -> Result<StepResult, Diag> {
    ensure_prepared(state, name)?;
    let (synthd, step_name) = synth::synthesize_resource(&state.playbook, key, props)?;
    let os = state.machines[name].instance.os();
    let bin = GuestPaths::bin_for(os);

    // A fresh working dir per apply.
    let n = {
        let ms = state.machines.get_mut(name).unwrap();
        ms.counter += 1;
        ms.counter
    };
    let (dir, pb_path) = scenario_dir(os, name, n);
    let ms = &state.machines[name];
    mkdir_guest(ms.instance.as_ref(), os, &dir)?;
    ms.instance.copy_in(synthd.dir.path(), &pb_path)?;
    let report = run_in_guest(ms.instance.as_ref(), bin, mode, &pb_path, Some(synth::PLAY))?;
    Ok(step_result(report.steps.iter().find(|s| s.name == step_name)))
}

/// Apply or check a whole authored playbook directory (relative to the
/// scenario's package) and return a queryable report.
fn apply_playbook(
    state: &mut LabState,
    name: &str,
    dir: &str,
    mode: &str,
) -> Result<RunReport, Diag> {
    ensure_prepared(state, name)?;
    let host_dir = state.pkg_dir.join(dir);
    if !host_dir.is_dir() {
        return Err(Diag::bare(format!(
            "playbook dir '{dir}' not found under {}",
            state.pkg_dir.display()
        )));
    }
    let os = state.machines[name].instance.os();
    let bin = GuestPaths::bin_for(os);
    let n = {
        let ms = state.machines.get_mut(name).unwrap();
        ms.counter += 1;
        ms.counter
    };
    let (gdir, pb_path) = scenario_dir(os, name, n);
    let ms = &state.machines[name];
    mkdir_guest(ms.instance.as_ref(), os, &gdir)?;
    ms.instance.copy_in(&host_dir, &pb_path)?;
    let report = run_in_guest(ms.instance.as_ref(), bin, mode, &pb_path, None)?;
    let steps = report
        .steps
        .iter()
        .map(|s| (s.name.clone(), step_result(Some(s))))
        .collect();
    Ok(RunReport {
        ok: report.exit_code == 0,
        steps,
    })
}

// --------------------------------------------------------------- registration

/// Build the `testlab` host module (the scenario driver API).
pub fn module() -> Module {
    let mut m = Module::new("testlab");
    m.doc("Scenario driver API: provision machines, apply config-weave, reboot, assert");

    // -- Lab -----------------------------------------------------------------
    m.ty::<Lab>()
        .method("log", |l: &Lab, msg: &str| {
            let st = l.state.borrow();
            if !st.quiet {
                eprintln!("⟳ [scenario] {msg}");
            }
        })
        .method(
            "machine",
            |l: &Lab, name: &str| -> Result<Machine, String> {
                // Bring the declared VM up on first reference (idempotent on
                // the vmlab side), then return a handle bound to its name.
                let mut st = l.state.borrow_mut();
                if !st.machines.contains_key(name) {
                    let instance = st.lab.machine(name).map_err(ds)?;
                    st.machines.insert(
                        name.to_string(),
                        MachineState {
                            instance,
                            prepared: false,
                            counter: 0,
                        },
                    );
                }
                Ok(Machine {
                    state: l.state.clone(),
                    name: name.to_string(),
                })
            },
        );

    // -- Machine -------------------------------------------------------------
    m.ty::<Machine>()
        .method("name", |m: &Machine| m.name.clone())
        .method(
            "exec",
            |m: &Machine, cmd: String, args: Vec<String>| -> Result<ExecOut, String> {
                let st = m.state.borrow();
                let inst = machine_inst(&st, &m.name)?;
                let mut argv: Vec<&str> = vec![cmd.as_str()];
                argv.extend(args.iter().map(String::as_str));
                inst.exec(&argv).map(into_exec).map_err(ds)
            },
        )
        .method(
            "powershell",
            |m: &Machine, script: &str| -> Result<ExecOut, String> {
                let st = m.state.borrow();
                let inst = machine_inst(&st, &m.name)?;
                inst.exec(&[
                    "powershell",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    script,
                ])
                .map(into_exec)
                .map_err(ds)
            },
        )
        .method(
            "copy_in",
            |m: &Machine, host: String, dest: String| -> Result<(), String> {
                let st = m.state.borrow();
                let inst = machine_inst(&st, &m.name)?;
                inst.copy_in(std::path::Path::new(&host), &dest).map_err(ds)
            },
        )
        .method("reboot", |m: &Machine| -> Result<(), String> {
            let st = m.state.borrow();
            let inst = machine_inst(&st, &m.name)?;
            inst.reboot().map_err(ds)
        })
        .method(
            "wait_ready",
            |m: &Machine, secs: i64| -> Result<(), String> {
                let st = m.state.borrow();
                let inst = machine_inst(&st, &m.name)?;
                inst.wait_ready(secs.max(0) as u64).map_err(ds)
            },
        )
        .method(
            "apply_resource",
            |m: &Machine, key: String, props: DynValue| -> Result<StepResult, String> {
                let mut st = m.state.borrow_mut();
                apply_resource(&mut st, &m.name, &key, &props, "apply").map_err(ds)
            },
        )
        .method(
            "check_resource",
            |m: &Machine, key: String, props: DynValue| -> Result<StepResult, String> {
                let mut st = m.state.borrow_mut();
                apply_resource(&mut st, &m.name, &key, &props, "check").map_err(ds)
            },
        )
        .method("apply", |m: &Machine, dir: &str| -> Result<RunReport, String> {
            let mut st = m.state.borrow_mut();
            apply_playbook(&mut st, &m.name, dir, "apply").map_err(ds)
        })
        .method("check", |m: &Machine, dir: &str| -> Result<RunReport, String> {
            let mut st = m.state.borrow_mut();
            apply_playbook(&mut st, &m.name, dir, "check").map_err(ds)
        });

    // -- RunReport -----------------------------------------------------------
    m.ty::<RunReport>()
        .method("ok", |r: &RunReport| r.ok)
        .method("step", |r: &RunReport, name: &str| -> Result<StepResult, String> {
            r.steps
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, s)| s.clone())
                .ok_or_else(|| format!("no step '{name}' in the run report"))
        });

    m
}

fn into_exec(o: crate::testlab::backend::ExecOutput) -> ExecOut {
    ExecOut {
        exit_code: o.exit_code as i64,
        stdout: o.stdout,
        stderr: o.stderr,
    }
}

/// Borrow a provisioned machine's instance from shared state.
fn machine_inst<'a>(st: &'a LabState, name: &str) -> Result<&'a dyn TestInstance, String> {
    st.machines
        .get(name)
        .map(|ms| ms.instance.as_ref())
        .ok_or_else(|| format!("no machine '{name}'"))
}
