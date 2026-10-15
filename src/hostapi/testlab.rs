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
use std::path::{Path, PathBuf};
use std::rc::Rc;

use wscript::{Module, Script};
use wscript_std::DynValue;

use crate::diag::Diag;
use crate::model::Playbook;
use crate::testlab::backend::Transport;
use crate::testlab::guest::{GatherOutcome, Guest};
use crate::testlab::synth::{self, BinaryResolver};
use crate::testlab::vmlab::{VmlabInstance, VmlabLab};

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
    instance: VmlabInstance,
    /// Binary copied in + smoke-tested.
    prepared: bool,
    /// Per-machine working-dir counter for synthesized applies.
    counter: usize,
}

/// Everything a running scenario shares. Lives behind every handle's `Rc`.
pub struct LabState {
    lab: VmlabLab,
    playbook: Rc<Playbook>,
    /// The scenario package's directory, for resolving `apply(dir)` paths.
    pkg_dir: PathBuf,
    binaries: BinaryResolver,
    machines: HashMap<String, MachineState>,
    quiet: bool,
}

impl LabState {
    pub fn new(
        lab: VmlabLab,
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

/// The next working-dir number for `name`, so successive applies on one
/// machine never collide. The machine is known to exist — `ensure_prepared`
/// or `machine_inst` runs first at every call site.
fn bump(state: &mut LabState, name: &str) -> usize {
    let ms = state.machines.get_mut(name).expect("machine present");
    ms.counter += 1;
    ms.counter
}

/// Copy the config-weave binary into a machine and smoke-test it, once.
fn ensure_prepared(state: &mut LabState, name: &str) -> Result<(), Diag> {
    let ms = state
        .machines
        .get(name)
        .ok_or_else(|| Diag::bare(format!("no machine '{name}'")))?;
    if ms.prepared {
        return Ok(());
    }
    // Lazily, per machine: a scenario that only ever calls `machine.exec`
    // on this one must not pay to copy a binary it will never use.
    Guest::new(&ms.instance).prepare(&state.binaries, &format!("'{name}'"))?;
    state.machines.get_mut(name).unwrap().prepared = true;
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

    // A fresh working dir per apply.
    let n = bump(state, name);
    let ms = &state.machines[name];
    let wd = Guest::new(&ms.instance).scenario_work(name, n)?;
    wd.stage(synthd.dir.path())?;
    let out = wd.run(mode, Some(synth::PLAY), None, mode)?;
    Ok(step_result(
        out.report.steps.iter().find(|s| s.name == step_name),
    ))
}

/// Run one gatherer in-guest through the `__gather` protocol and return
/// the gathered value.
fn gather_fact(
    state: &mut LabState,
    name: &str,
    key: &str,
    params: &DynValue,
) -> Result<DynValue, Diag> {
    ensure_prepared(state, name)?;
    let (package, _gatherer) = key
        .split_once('.')
        .ok_or_else(|| Diag::bare(format!("gatherer key '{key}' must be 'package.gatherer'")))?;
    let synthd = synth::synthesize_gather(&state.playbook, package)?;

    let n = bump(state, name);
    let ms = &state.machines[name];
    let wd = Guest::new(&ms.instance).scenario_work(name, n)?;
    wd.stage(synthd.dir.path())?;

    let params = (!matches!(params, DynValue::Null)).then(|| crate::convert::dyn_to_json(params));

    // A scenario asserts by calling this, so a gatherer's refusal is an
    // error the script sees — the opposite of the runner, which records it
    // and carries on.
    match wd.gather(key, params.as_ref(), key)? {
        GatherOutcome::Ok(value) => crate::convert::json_to_dyn(&value).map_err(Diag::bare),
        GatherOutcome::Refused(why) => Err(Diag::bare(format!("gather '{key}' failed: {why}"))),
    }
}

/// Apply or check a whole authored playbook directory (relative to the
/// scenario's package) and return a queryable report. `play` is the
/// `opts.play` of `apply_with`/`check_with`; without one the playbook's
/// only play runs.
fn apply_playbook(
    state: &mut LabState,
    name: &str,
    dir: &str,
    mode: &str,
    play: Option<String>,
) -> Result<RunReport, Diag> {
    let host_dir = state.pkg_dir.join(dir);
    if !host_dir.is_dir() {
        return Err(Diag::bare(format!(
            "playbook dir '{dir}' not found under {}",
            state.pkg_dir.display()
        )));
    }
    // Resolved before the binary is copied in: a scenario that names the
    // wrong play should fail without paying for a machine round trip.
    let play = match play {
        Some(p) => p,
        None => only_play(&host_dir, dir)?,
    };
    ensure_prepared(state, name)?;
    let n = bump(state, name);
    let ms = &state.machines[name];
    run_playbook(&ms.instance, name, n, &host_dir, mode, &play)
}

/// Stage `host_dir` into a fresh working dir on `machine` and run `play`
/// from it. Split out over the transport so the argv is unit-testable.
fn run_playbook(
    t: &dyn Transport,
    machine: &str,
    n: usize,
    host_dir: &Path,
    mode: &str,
    play: &str,
) -> Result<RunReport, Diag> {
    let wd = Guest::new(t).scenario_work(machine, n)?;
    wd.stage(host_dir)?;
    let out = wd.run(mode, Some(play), None, mode)?;
    let steps = out
        .report
        .steps
        .iter()
        .map(|s| (s.name.clone(), step_result(Some(s))))
        .collect();
    Ok(RunReport {
        ok: out.report.exit_code == 0,
        steps,
    })
}

/// The play `apply(dir)`/`check(dir)` run: the playbook's only one.
/// `config-weave apply|check` require a play, and with several there is
/// no right guess, so the error names them and points at `apply_with`.
fn only_play(host_dir: &Path, dir: &str) -> Result<String, Diag> {
    let loaded = crate::model::load(host_dir);
    let Some(pb) = loaded.playbook else {
        let why: Vec<String> = loaded.diags.into_iter().map(|d| d.message).collect();
        return Err(Diag::bare(format!(
            "cannot read the plays of playbook dir '{dir}': {}",
            why.join("; ")
        )));
    };
    let names: Vec<&str> = pb.plays.iter().map(|p| p.name.as_str()).collect();
    match names.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(Diag::bare(format!(
            "playbook dir '{dir}' declares no play to run"
        ))),
        many => Err(Diag::bare(format!(
            "playbook dir '{dir}' declares {} plays ({}); name one with \
             apply_with/check_with(dir, {{play: \"name\"}})",
            many.len(),
            many.join(", ")
        ))),
    }
}

/// The `opts` of `apply_with`/`check_with`: null or a map with `play`.
fn parse_play_opts(opts: &DynValue) -> Result<Option<String>, String> {
    let mut play = None;
    match opts {
        DynValue::Null => {}
        DynValue::Map(m) => {
            for (k, v) in m {
                match (k.as_str(), v) {
                    ("play", DynValue::String(p)) if !p.is_empty() => play = Some(p.clone()),
                    ("play", other) => {
                        return Err(format!(
                            "option 'play' must be a non-empty play name, got {other:?}"
                        ));
                    }
                    (other, _) => {
                        return Err(format!("unknown option '{other}' (expected play)"));
                    }
                }
            }
        }
        other => return Err(format!("options must be a map or null, got {other:?}")),
    }
    Ok(play)
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
                let mut st = m.state.borrow_mut();
                machine_inst(&st, &m.name)?;
                // A fresh working dir per script, like an apply: the
                // script file lands there rather than on the command line.
                let n = bump(&mut st, &m.name);
                let inst = machine_inst(&st, &m.name)?;
                let wd = Guest::new(inst).scenario_work(&m.name, n).map_err(ds)?;
                let run = wd.powershell(script);
                // A leftover script file is worth a warning, never worth
                // replacing the script's own result.
                if let Err(e) = run.cleanup
                    && !st.quiet
                {
                    eprintln!("⚠ [scenario] {}: {}", m.name, e.message);
                }
                run.out.map(into_exec).map_err(ds)
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
            "gather",
            |m: &Machine, key: String, params: DynValue| -> Result<DynValue, String> {
                let mut st = m.state.borrow_mut();
                gather_fact(&mut st, &m.name, &key, &params).map_err(ds)
            },
        )
        .method(
            "check_resource",
            |m: &Machine, key: String, props: DynValue| -> Result<StepResult, String> {
                let mut st = m.state.borrow_mut();
                apply_resource(&mut st, &m.name, &key, &props, "check").map_err(ds)
            },
        )
        .method(
            "apply",
            |m: &Machine, dir: &str| -> Result<RunReport, String> {
                let mut st = m.state.borrow_mut();
                apply_playbook(&mut st, &m.name, dir, "apply", None).map_err(ds)
            },
        )
        .method(
            "apply_with",
            |m: &Machine, dir: String, opts: DynValue| -> Result<RunReport, String> {
                let play = parse_play_opts(&opts)?;
                let mut st = m.state.borrow_mut();
                apply_playbook(&mut st, &m.name, &dir, "apply", play).map_err(ds)
            },
        )
        .method(
            "check",
            |m: &Machine, dir: &str| -> Result<RunReport, String> {
                let mut st = m.state.borrow_mut();
                apply_playbook(&mut st, &m.name, dir, "check", None).map_err(ds)
            },
        )
        .method(
            "check_with",
            |m: &Machine, dir: String, opts: DynValue| -> Result<RunReport, String> {
                let play = parse_play_opts(&opts)?;
                let mut st = m.state.borrow_mut();
                apply_playbook(&mut st, &m.name, &dir, "check", play).map_err(ds)
            },
        );

    // -- RunReport -----------------------------------------------------------
    m.ty::<RunReport>()
        .method("ok", |r: &RunReport| r.ok)
        .method(
            "step",
            |r: &RunReport, name: &str| -> Result<StepResult, String> {
                r.steps
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, s)| s.clone())
                    .ok_or_else(|| format!("no step '{name}' in the run report"))
            },
        );

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
fn machine_inst<'a>(st: &'a LabState, name: &str) -> Result<&'a VmlabInstance, String> {
    st.machines
        .get(name)
        .map(|ms| &ms.instance)
        .ok_or_else(|| format!("no machine '{name}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testlab::backend::GuestOs;
    use crate::testlab::guest::fake::{FakeGuest, printed};

    const REPORT: &str = r#"{"playbook":"p","version":"0","play":"site","mode":"apply","exit_code":0,"duration_secs":0.0,"gathered":[],"steps":[]}"#;

    /// A playbook dir declaring `plays`, each with no steps.
    fn playbook_dir(plays: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut src = String::from("playbook \"p\" {\n  description = \"p\"\n");
        for p in plays {
            src.push_str(&format!(
                "  play \"{p}\" {{\n    description = \"d\"\n  }}\n"
            ));
        }
        src.push_str("}\n");
        std::fs::write(dir.path().join("playbook.wcl"), src).unwrap();
        dir
    }

    #[test]
    fn a_playbook_run_names_the_play() {
        // `config-weave apply|check` require `<PLAYBOOK_DIR> <PLAY>`; the
        // run used to omit the play and could never succeed.
        let fake = FakeGuest::new(GuestOs::Linux);
        let host = tempfile::tempdir().unwrap();
        fake.queue([printed(""), printed(REPORT)]);
        let report = run_playbook(&fake, "dc1", 2, host.path(), "apply", "site").unwrap();
        assert!(report.ok);

        assert_eq!(
            fake.argvs()[1],
            vec![
                "/weave/config-weave",
                "apply",
                "/weave/s/dc1-2/playbook",
                "site",
                "--json",
                "--continue-on-error",
            ]
        );
    }

    #[test]
    fn the_only_play_is_the_default() {
        let dir = playbook_dir(&["site"]);
        assert_eq!(only_play(dir.path(), "promote").unwrap(), "site");
    }

    #[test]
    fn several_plays_are_named_rather_than_guessed() {
        let dir = playbook_dir(&["forest", "member"]);
        let err = only_play(dir.path(), "promote").unwrap_err();
        assert!(err.message.contains("'promote'"), "{}", err.message);
        assert!(err.message.contains("forest, member"), "{}", err.message);
        assert!(err.message.contains("apply_with"), "{}", err.message);

        let none = playbook_dir(&[]);
        let err = only_play(none.path(), "empty").unwrap_err();
        assert!(err.message.contains("no play"), "{}", err.message);
    }

    #[test]
    fn play_opts_take_a_name_and_reject_anything_else() {
        use std::collections::HashMap;
        assert_eq!(parse_play_opts(&DynValue::Null).unwrap(), None);

        let mut m = HashMap::new();
        m.insert("play".to_string(), DynValue::String("member".into()));
        assert_eq!(
            parse_play_opts(&DynValue::Map(m)).unwrap().as_deref(),
            Some("member")
        );

        let mut bad = HashMap::new();
        bad.insert("plya".to_string(), DynValue::String("member".into()));
        let err = parse_play_opts(&DynValue::Map(bad)).unwrap_err();
        assert!(err.contains("unknown option 'plya'"), "{err}");

        let mut empty = HashMap::new();
        empty.insert("play".to_string(), DynValue::String(String::new()));
        assert!(parse_play_opts(&DynValue::Map(empty)).is_err());
    }
}
