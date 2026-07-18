mod comdispatch;
mod convert;
mod diag;
mod docsgen;
mod engine;
mod hostapi;
mod logging;
mod model;
mod report;
mod scaffold;
mod testlab;
mod vocab;

use engine::status::Mode;
use engine::vars::{Origin, VarStore};

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use diag::Diag;

/// Exit codes per PRD §9. Exit 3 (reboot required) is produced by
/// `RunReport::exit_code()` in `engine/status.rs`.
const EXIT_OK: u8 = 0;
const EXIT_STEP_ERROR: u8 = 1;
const EXIT_VALIDATION: u8 = 2;

#[derive(Parser)]
#[command(
    name = "config-weave",
    version,
    about = "Single-binary configuration management"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Override a playbook variable (KEY=VALUE). Repeatable.
    #[arg(long = "var", global = true, value_name = "KEY=VALUE")]
    vars: Vec<String>,

    /// Merge a WCL file's top-level variables into scope.
    #[arg(long = "var-file", global = true, value_name = "PATH")]
    var_file: Option<PathBuf>,

    /// Worker pool size (default: min(cpu_count, 8)).
    #[arg(long, global = true, value_name = "N")]
    jobs: Option<usize>,

    /// Continue dispatching steps after an Error.
    #[arg(long, global = true)]
    continue_on_error: bool,

    /// JSON output mode (single object on stdout at completion).
    #[arg(long, global = true)]
    json: bool,

    /// Plain ASCII output (also auto-selected when not a TTY).
    #[arg(long, global = true)]
    no_color: bool,

    /// Enable NDJSON file logging.
    #[arg(long, global = true, value_name = "PATH")]
    log_file: Option<PathBuf>,

    /// File log level (independent of terminal verbosity).
    #[arg(long, global = true, value_name = "LEVEL", default_value = "info")]
    log_level: String,

    /// Increase terminal verbosity (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Command {
    /// Report configuration status of all steps (never mutates).
    Check {
        playbook_dir: PathBuf,
        play: String,
        /// Stream one machine-readable JSON event per line to stderr
        /// (run/gather lifecycle and per-step phase progress; stdout
        /// still carries the final --json report).
        #[arg(long)]
        events_ndjson: bool,
    },
    /// Apply all unconfigured steps in a play.
    Apply {
        playbook_dir: PathBuf,
        play: String,
        /// Stream one machine-readable JSON event per line to stderr
        /// (run/gather lifecycle and per-step phase progress; stdout
        /// still carries the final --json report).
        #[arg(long)]
        events_ndjson: bool,
    },
    /// List all plays defined in the playbook.
    List { playbook_dir: PathBuf },
    /// Full validation pipeline, no execution.
    Validate { playbook_dir: PathBuf },
    /// Run package convergence tests in disposable instances.
    Test {
        playbook_dir: PathBuf,
        /// Only run matching tests: a package name or `package:test`.
        filter: Option<String>,
        /// Override every test's backend ("docker" or "vmlab").
        #[arg(long, value_name = "NAME")]
        backend: Option<String>,
        /// Run every test against this image instead of its own.
        #[arg(long, value_name = "IMAGE")]
        image: Option<String>,
        /// Leave instances running for post-mortem debugging.
        #[arg(long)]
        keep: bool,
        /// Static linux config-weave binary to copy into instances.
        #[arg(long, value_name = "PATH")]
        binary: Option<PathBuf>,
        /// Windows config-weave binary for windows guests (vmlab).
        #[arg(long, value_name = "PATH")]
        binary_windows: Option<PathBuf>,
        /// Max docker test groups (containers) to run at once
        /// (default: min(cpu_count, 8)).
        #[arg(long, value_name = "N")]
        docker_jobs: Option<usize>,
        /// Max vmlab test groups (VMs) to run at once — kept small since
        /// VMs are heavy (default: 2).
        #[arg(long, value_name = "N")]
        vmlab_jobs: Option<usize>,
        /// Stream one machine-readable JSON event per line to stderr
        /// (lifecycle, per-phase progress, and raw instance attach info;
        /// stdout still carries the final report). A supervisor that
        /// kills this process must clean up instances itself using the
        /// ids in `instance_ready` events.
        #[arg(long)]
        events_ndjson: bool,
    },
    /// Generate wdoc documentation (default outdir: <dir>/docs/).
    Docs {
        playbook_dir: PathBuf,
        outdir: Option<PathBuf>,
        /// After rendering, serve the site with `wcl wdoc serve`
        /// (watch-rebuild dev server with live reload).
        #[arg(long)]
        serve: bool,
        /// Listen address for --serve (default 127.0.0.1:8080).
        #[arg(long, requires = "serve")]
        addr: Option<String>,
        /// Document only the packages — skip the playbook's plays,
        /// variables and gathered facts.
        #[arg(long)]
        pkg_only: bool,
    },
    /// Emit .wscripti interface files for the host API plus a starter wscript.toml.
    Wscripti { outdir: Option<PathBuf> },
    /// Scaffold a skeleton playbook.
    Init { dir: PathBuf },
    /// Print version information.
    Version,
    /// (internal) Run one gatherer and print its value as JSON.
    /// Part of the in-container test protocol.
    #[command(name = "__gather", hide = true)]
    GatherOne {
        playbook_dir: PathBuf,
        /// Gatherer key as `package.gatherer`.
        gatherer: String,
        /// Gatherer params as a JSON object.
        #[arg(long, value_name = "JSON")]
        params_json: Option<String>,
    },
    /// (internal) Compile and run a verify script against the host API.
    /// Part of the in-container test protocol.
    #[command(name = "__verify", hide = true)]
    RunVerify {
        script: PathBuf,
        /// JSON file with the facts map passed to verify().
        #[arg(long, value_name = "PATH")]
        facts: Option<PathBuf>,
    },
    /// (internal) stdin `{kind, source}` → stdout `{ok, doc | diags}`:
    /// structural DocJson extraction from one WCL source for external
    /// tooling. Content-based (no file IO), so it works on unsaved
    /// buffers; kind is "playbook" or "package".
    #[command(name = "__wcl-inspect", hide = true)]
    WclInspect,
    /// (internal) stdin `{kind, base_source, doc}` → stdout
    /// `{ok, source | diags}`: sync a DocJson onto the base source's
    /// AST (comments survive) and print the canonical WCL.
    #[command(name = "__wcl-render", hide = true)]
    WclRender,
    /// (internal) Scaffold templates as JSON for external tooling's
    /// "new script" actions.
    #[command(name = "__templates", hide = true)]
    Templates,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    logging::set_verbosity(cli.verbose);
    // Held for the whole process so buffered NDJSON lines flush on exit.
    let _log_guard = match logging::init(cli.log_file.as_deref(), &cli.log_level) {
        Ok(g) => g,
        Err(d) => {
            eprintln!("{}", d.rendered);
            return ExitCode::from(EXIT_VALIDATION);
        }
    };
    let code = match &cli.command {
        Command::Validate { playbook_dir } => cmd_validate(&cli, playbook_dir),
        Command::List { playbook_dir } => cmd_list(&cli, playbook_dir),
        Command::Version => {
            println!("config-weave {}", env!("CARGO_PKG_VERSION"));
            EXIT_OK
        }
        Command::Check {
            playbook_dir,
            play,
            events_ndjson,
        } => cmd_run(&cli, playbook_dir, play, Mode::Check, *events_ndjson),
        Command::Apply {
            playbook_dir,
            play,
            events_ndjson,
        } => cmd_run(&cli, playbook_dir, play, Mode::Apply, *events_ndjson),
        Command::Test {
            playbook_dir,
            filter,
            backend,
            image,
            keep,
            binary,
            binary_windows,
            docker_jobs,
            vmlab_jobs,
            events_ndjson,
        } => cmd_test(
            &cli,
            playbook_dir,
            filter.as_deref(),
            backend.as_deref(),
            image.as_deref(),
            *keep,
            binary.as_deref(),
            binary_windows.as_deref(),
            *docker_jobs,
            *vmlab_jobs,
            *events_ndjson,
        ),
        Command::Docs {
            playbook_dir,
            outdir,
            serve,
            addr,
            pkg_only,
        } => cmd_docs(
            playbook_dir,
            outdir.as_deref(),
            *serve,
            addr.as_deref(),
            *pkg_only,
        ),
        Command::Wscripti { outdir } => {
            let dir = outdir.clone().unwrap_or_else(|| PathBuf::from("."));
            match scaffold::wscripti(&dir) {
                Ok(()) => {
                    println!(
                        "wrote {} and wscript.toml",
                        dir.join("weave.wscripti").display()
                    );
                    EXIT_OK
                }
                Err(d) => {
                    eprintln!("{}", d.rendered);
                    EXIT_VALIDATION
                }
            }
        }
        Command::GatherOne {
            playbook_dir,
            gatherer,
            params_json,
        } => cmd_gather_one(playbook_dir, gatherer, params_json.as_deref()),
        Command::RunVerify { script, facts } => cmd_run_verify(script, facts.as_deref()),
        Command::WclInspect => cmd_wcl_inspect(),
        Command::WclRender => cmd_wcl_render(),
        Command::Templates => {
            println!(
                "{}",
                serde_json::json!({
                    "playbook": scaffold::PLAYBOOK,
                    "package": scaffold::PACKAGE,
                    "resource_script": scaffold::RESOURCE,
                    "gatherer_script": scaffold::GATHERER,
                    "verify_script": scaffold::VERIFY,
                })
            );
            EXIT_OK
        }
        Command::Init { dir } => match scaffold::init(dir) {
            Ok(()) => {
                println!(
                    "scaffolded a playbook in {} — next: edit, then `config-weave validate {}`",
                    dir.display(),
                    dir.display()
                );
                EXIT_OK
            }
            Err(d) => {
                eprintln!("{}", d.rendered);
                EXIT_VALIDATION
            }
        },
    };
    ExitCode::from(code)
}

/// Load + full validation; returns the playbook only when clean.
fn load_validated(dir: &std::path::Path) -> Result<model::Playbook, Vec<Diag>> {
    let loaded: model::Loaded = model::load(dir);
    let mut diags = loaded.diags;
    let Some(pb) = loaded.playbook else {
        return Err(diags);
    };
    diags.extend(engine::validate(&pb));
    if diags.is_empty() { Ok(pb) } else { Err(diags) }
}

fn print_diags(diags: &[Diag]) {
    for d in diags {
        eprintln!("{}", d.rendered);
    }
    eprintln!(
        "validation failed with {} error{}",
        diags.len(),
        if diags.len() == 1 { "" } else { "s" }
    );
}

/// Unwrap a `Result<T, Vec<Diag>>`, or print the diagnostics and return
/// `EXIT_VALIDATION` from the enclosing command function.
macro_rules! load_or_exit {
    ($expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(diags) => {
                print_diags(&diags);
                return EXIT_VALIDATION;
            }
        }
    };
}

fn cmd_validate(cli: &Cli, dir: &std::path::Path) -> u8 {
    match load_validated(dir) {
        Ok(pb) => {
            let steps: usize = pb.plays.iter().map(|p| p.steps().len()).sum();
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "playbook": pb.name,
                        "version": pb.version,
                        "packages": pb.packages.len(),
                        "plays": pb.plays.len(),
                        "steps": steps,
                        "diags": [],
                    })
                );
            } else {
                println!(
                    "ok: playbook '{}' v{} — {} package(s), {} play(s), {} step(s)",
                    pb.name,
                    pb.version,
                    pb.packages.len(),
                    pb.plays.len(),
                    steps
                );
            }
            EXIT_OK
        }
        Err(diags) => {
            if cli.json {
                let items: Vec<serde_json::Value> = diags
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "message": d.message,
                            "rendered": d.rendered,
                        })
                    })
                    .collect();
                println!("{}", serde_json::json!({ "ok": false, "diags": items }));
            } else {
                print_diags(&diags);
            }
            EXIT_VALIDATION
        }
    }
}

/// Build the override store from `--var` / `--var-file` flags.
fn override_store(cli: &Cli) -> Result<VarStore, Vec<Diag>> {
    let mut store = VarStore::new();
    if let Some(path) = &cli.var_file {
        for (name, value) in engine::vars::load_var_file(path)? {
            store.insert(&name, Origin::VarFile, value);
        }
    }
    for flag in &cli.vars {
        let (name, value) = engine::vars::parse_var_flag(flag).map_err(|d| vec![d])?;
        store.insert(&name, Origin::Var, value);
    }
    Ok(store)
}

fn cmd_run(cli: &Cli, dir: &std::path::Path, play: &str, mode: Mode, events_ndjson: bool) -> u8 {
    let pb = load_or_exit!(load_validated(dir));
    let store = load_or_exit!(override_store(cli));
    let mode_out = report::select_mode(cli.json, cli.no_color);
    // NDJSON and the rich progress line both write stderr; the event
    // stream wins (same exclusivity as `test --events-ndjson`).
    let sink = if events_ndjson {
        report::run_ndjson_sink()
    } else {
        report::progress_sink(mode_out)
    };
    match engine::execute(
        &pb,
        play,
        mode,
        cli.continue_on_error,
        cli.jobs,
        store,
        sink,
    ) {
        Ok(run_report) => {
            match mode_out {
                report::OutputMode::Json => println!("{}", report::json(&run_report)),
                report::OutputMode::Plain => print!("{}", report::plain(&run_report)),
                report::OutputMode::Rich => print!("{}", report::rich(&run_report)),
            }
            run_report.exit_code()
        }
        Err(diags) => {
            print_diags(&diags);
            EXIT_VALIDATION
        }
    }
}

/// Split a test filter into `(package, test)` selectors: no filter selects
/// everything, `pkg` selects one package, `pkg:test` selects one test.
fn parse_test_filter(filter: Option<&str>) -> (Option<&str>, Option<&str>) {
    match filter {
        Some(f) => match f.split_once(':') {
            Some((p, t)) => (Some(p), Some(t)),
            None => (Some(f), None),
        },
        None => (None, None),
    }
}

/// The `(package, test)` pairs matching the `(package, test)` selectors.
fn select_matching_tests<'a>(
    pb: &'a model::Playbook,
    fpkg: Option<&str>,
    ftest: Option<&str>,
) -> Vec<(&'a model::Package, &'a model::TestDecl)> {
    pb.packages
        .values()
        .filter(|pkg| fpkg.is_none_or(|p| p == pkg.name))
        .flat_map(|pkg| pkg.tests.iter().map(move |t| (pkg, t)))
        .filter(|(_, t)| ftest.is_none_or(|n| n == t.name))
        .collect()
}

/// The `(package, scenario)` pairs matching the selectors. Scenarios share
/// the `package:name` selection syntax with tests.
fn select_matching_scenarios<'a>(
    pb: &'a model::Playbook,
    fpkg: Option<&str>,
    fname: Option<&str>,
) -> Vec<(&'a model::Package, &'a model::ScenarioDecl)> {
    pb.packages
        .values()
        .filter(|pkg| fpkg.is_none_or(|p| p == pkg.name))
        .flat_map(|pkg| pkg.scenarios.iter().map(move |s| (pkg, s)))
        .filter(|(_, s)| fname.is_none_or(|n| n == s.name))
        .collect()
}

/// Discovered backends, keyed by name (`docker` / `vmlab`).
type Backends<'a> = Vec<(&'a str, Box<dyn testlab::backend::TestBackend>)>;

/// Probe each named backend once, up front. On a discovery failure the
/// error is printed and `Err(EXIT_VALIDATION)` returned so the caller can
/// fail fast before any test runs.
fn discover_backends(
    needed: std::collections::BTreeSet<&str>,
    quiet: bool,
) -> Result<Backends<'_>, u8> {
    let mut backends: Backends = Vec::new();
    for name in needed {
        let discovered: Result<Box<dyn testlab::backend::TestBackend>, diag::Diag> = match name {
            "docker" => testlab::docker::DockerBackend::discover(quiet)
                .map(|b| Box::new(b) as Box<dyn testlab::backend::TestBackend>),
            "vmlab" => testlab::vmlab::VmlabBackend::discover(quiet)
                .map(|b| Box::new(b) as Box<dyn testlab::backend::TestBackend>),
            other => unreachable!("backend '{other}' survived validation"),
        };
        match discovered {
            Ok(b) => backends.push((name, b)),
            Err(d) => {
                eprintln!("{}", d.rendered);
                return Err(EXIT_VALIDATION);
            }
        }
    }
    Ok(backends)
}

/// Bucket selected tests into shared-instance groups: tests with a
/// non-empty `group` share one instance (keyed per package, since a group
/// provisions one instance from one image on one backend); ungrouped tests
/// each get their own. Selection order is preserved via the carried index
/// so output is stable despite parallel execution.
fn bucket_groups<'a>(
    selected: &[(&'a model::Package, &'a model::TestDecl)],
    backends: &'a Backends<'_>,
    backend_override: Option<&str>,
    image_override: Option<&str>,
) -> Vec<testlab::runner::GroupSpec<'a>> {
    let backend_for = |t: &model::TestDecl| -> &'a dyn testlab::backend::TestBackend {
        let name = backend_override.unwrap_or(t.backend.as_str());
        backends
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| b.as_ref())
            .expect("backend discovered above")
    };
    let effective_image = |t: &model::TestDecl| -> String {
        image_override
            .map(str::to_string)
            .unwrap_or_else(|| t.image.clone())
    };
    let mut groups: Vec<testlab::runner::GroupSpec> = Vec::new();
    let mut group_index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for (idx, (pkg, t)) in selected.iter().enumerate() {
        let member = (idx, *pkg, *t);
        match &t.group {
            Some(g) => {
                let key = (pkg.name.clone(), g.clone());
                match group_index.get(&key) {
                    Some(&gi) => groups[gi].tests.push(member),
                    None => {
                        group_index.insert(key, groups.len());
                        groups.push(testlab::runner::GroupSpec {
                            backend: backend_for(t),
                            image: effective_image(t),
                            tests: vec![member],
                        });
                    }
                }
            }
            None => groups.push(testlab::runner::GroupSpec {
                backend: backend_for(t),
                image: effective_image(t),
                tests: vec![member],
            }),
        }
    }
    groups
}

/// `config-weave test`: run package convergence tests in disposable
/// backend instances (PRD-extension; see docs/notes.md, testlab section).
#[allow(clippy::too_many_arguments)]
fn cmd_test(
    cli: &Cli,
    dir: &std::path::Path,
    filter: Option<&str>,
    backend_override: Option<&str>,
    image_override: Option<&str>,
    keep: bool,
    binary: Option<&std::path::Path>,
    binary_windows: Option<&std::path::Path>,
    docker_jobs: Option<usize>,
    vmlab_jobs: Option<usize>,
    events_ndjson: bool,
) -> u8 {
    let pb = std::rc::Rc::new(load_or_exit!(load_validated(dir)));

    let (fpkg, ftest) = parse_test_filter(filter);
    let selected = select_matching_tests(&pb, fpkg, ftest);
    let scenarios_sel = select_matching_scenarios(&pb, fpkg, ftest);
    if selected.is_empty() && scenarios_sel.is_empty() {
        let available: Vec<String> = pb
            .packages
            .values()
            .flat_map(|p| {
                let tests = p
                    .tests
                    .iter()
                    .map(move |t| format!("{}:{}", p.name, t.name));
                let scens = p
                    .scenarios
                    .iter()
                    .map(move |s| format!("{}:{}", p.name, s.name));
                tests.chain(scens)
            })
            .collect();
        if available.is_empty() {
            eprintln!("error: no package declares any tests or scenarios");
        } else {
            eprintln!(
                "error: nothing matches '{}' (available: {})",
                filter.unwrap_or("*"),
                available.join(", ")
            );
        }
        return EXIT_VALIDATION;
    }

    if let Some(b) = backend_override
        && b != "docker"
        && b != "vmlab"
    {
        eprintln!("error: unknown test backend '{b}' (supported: 'docker', 'vmlab')");
        return EXIT_VALIDATION;
    }

    let mode_out = report::select_mode(cli.json, cli.no_color);
    // NDJSON claims stderr for events, so the human progress lines and
    // the backends' raw prints go quiet exactly as in JSON output mode.
    let quiet = mode_out == report::OutputMode::Json || events_ndjson;
    let sink: testlab::events::TestEventSink = if events_ndjson {
        testlab::events::ndjson_sink()
    } else if quiet {
        testlab::events::null_sink()
    } else {
        testlab::events::human_sink()
    };

    // Discover each backend the selected tests actually use, once, up
    // front — a broken environment fails fast with exit 2 instead of
    // erroring every test. Tests on other backends never probe.
    let needed: std::collections::BTreeSet<&str> = selected
        .iter()
        .map(|(_, t)| backend_override.unwrap_or(t.backend.as_str()))
        .chain(
            scenarios_sel
                .iter()
                .map(|_| backend_override.unwrap_or("vmlab")),
        )
        .collect();
    let backends = match discover_backends(needed, quiet) {
        Ok(b) => b,
        Err(code) => return code,
    };

    let opts = testlab::runner::RunnerOptions {
        binaries: testlab::synth::BinaryResolver::new(
            binary.map(std::path::Path::to_path_buf),
            binary_windows.map(std::path::Path::to_path_buf),
        ),
        keep,
        jobs: cli.jobs,
        sink: sink.clone(),
        docker_cap: docker_jobs.unwrap_or_else(engine::run::default_jobs).max(1),
        vmlab_cap: vmlab_jobs.unwrap_or(2).max(1),
    };

    let groups = bucket_groups(&selected, &backends, backend_override, image_override);

    // Announce the full plan up front so a consumer can size its progress
    // view; scenarios follow the groups with no group index.
    let mut planned: Vec<testlab::events::PlannedTest> = groups
        .iter()
        .enumerate()
        .flat_map(|(gid, g)| {
            g.tests
                .iter()
                .map(move |(_, pkg, t)| testlab::events::PlannedTest {
                    package: pkg.name.clone(),
                    test: t.name.clone(),
                    group: Some(gid),
                    backend: g.backend.name().to_string(),
                    image: g.image.clone(),
                })
        })
        .collect();
    planned.extend(
        scenarios_sel
            .iter()
            .map(|(pkg, s)| testlab::events::PlannedTest {
                package: pkg.name.clone(),
                test: s.name.clone(),
                group: None,
                backend: "vmlab".to_string(),
                image: "(scenario)".to_string(),
            }),
    );
    sink(testlab::events::TestEvent::RunStarted {
        playbook: pb.name.clone(),
        groups: groups.len(),
        tests: planned,
    });

    let start = std::time::Instant::now();
    let mut tests = testlab::runner::run_groups(&pb, groups, &opts);

    // Scenarios run after the (parallel) test groups, sequentially — each
    // may bring up several machines. They reuse the test report shape.
    if !scenarios_sel.is_empty() {
        let backend_for = |name: &str| -> &dyn testlab::backend::TestBackend {
            let n = backend_override.unwrap_or(name);
            backends
                .iter()
                .find(|(bn, _)| *bn == n)
                .map(|(_, b)| b.as_ref())
                .expect("backend discovered above")
        };
        let units: Vec<testlab::runner::ScenarioUnit> = scenarios_sel
            .iter()
            .map(|(pkg, s)| testlab::runner::ScenarioUnit {
                package: pkg,
                scenario: s,
                backend: backend_for("vmlab"),
            })
            .collect();
        let scenario_reports = testlab::runner::run_scenarios(
            &pb,
            units,
            binary.map(std::path::Path::to_path_buf),
            binary_windows.map(std::path::Path::to_path_buf),
            keep,
            quiet,
            &sink,
        );
        tests.extend(scenario_reports);
    }

    let run_report = testlab::report::TestRunReport {
        playbook: pb.name.clone(),
        tests,
        duration: start.elapsed(),
    };
    // The terminal event goes out before the stdout report so a streaming
    // consumer sees the end even if it never reads stdout.
    let count = |o: testlab::report::TestOutcome| {
        run_report.tests.iter().filter(|t| t.outcome == o).count()
    };
    sink(testlab::events::TestEvent::RunFinished {
        exit_code: run_report.exit_code(),
        passed: count(testlab::report::TestOutcome::Passed),
        failed: count(testlab::report::TestOutcome::Failed),
        errors: count(testlab::report::TestOutcome::Error),
        duration_secs: run_report.duration.as_secs_f64(),
    });
    match mode_out {
        report::OutputMode::Json => println!("{}", report::test_json(&run_report)),
        report::OutputMode::Plain => print!("{}", report::test_plain(&run_report)),
        report::OutputMode::Rich => print!("{}", report::test_rich(&run_report)),
    }
    run_report.exit_code()
}

/// `__gather`: run one gatherer of a validated playbook and print
/// `{"ok":true,"value":…}` / `{"ok":false,"error":…}` on stdout. The
/// testlab runner execs this inside the container; host-runnable too.
fn cmd_gather_one(dir: &std::path::Path, gatherer: &str, params_json: Option<&str>) -> u8 {
    fn fail(error: String) -> u8 {
        println!("{}", serde_json::json!({ "ok": false, "error": error }));
        EXIT_OK // protocol succeeded; the JSON carries the outcome
    }

    let pb = match load_validated(dir) {
        Ok(pb) => pb,
        Err(diags) => {
            print_diags(&diags);
            return EXIT_VALIDATION;
        }
    };
    let Some((pkg, gname)) = gatherer.split_once('.') else {
        return fail(format!(
            "gatherer must be 'package.gatherer', got '{gatherer}'"
        ));
    };
    let Some(decl_params) = pb
        .packages
        .get(pkg)
        .and_then(|p| p.gatherers.get(gname))
        .map(|g| g.params.clone())
    else {
        return fail(format!("no gatherer '{gatherer}' in the playbook"));
    };

    let mut params = std::collections::HashMap::new();
    if let Some(json) = params_json {
        let parsed: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => return fail(format!("invalid --params-json: {e}")),
        };
        match convert::json_to_dyn(&parsed) {
            Ok(wscript_std::DynValue::Map(m)) => params.extend(m),
            Ok(_) => return fail("--params-json must be a JSON object".into()),
            Err(e) => return fail(format!("invalid --params-json: {e}")),
        }
    }
    if let Err(es) = engine::gather::apply_param_defaults(&mut params, &decl_params) {
        return fail(format!("gatherer '{gatherer}': {}", es.join("; ")));
    }

    let ctx = hostapi::context();
    let scripts = match engine::scripts::compile_all(&pb, &ctx) {
        Ok(s) => s,
        Err(diags) => {
            print_diags(&diags);
            return EXIT_VALIDATION;
        }
    };
    match engine::gather::run_single(&scripts, &ctx, gatherer, wscript_std::DynValue::Map(params)) {
        Ok(value) => {
            println!(
                "{}",
                serde_json::json!({ "ok": true, "value": convert::dyn_to_json(&value) })
            );
            EXIT_OK
        }
        Err(e) => fail(e),
    }
}

/// `__verify`: compile a wscript verify script against the host API and run
/// `verify(facts)`. Exit 0 = passed, 1 = failed (message on stdout),
/// 2 = the script is broken. The testlab runner execs this inside the
/// container; host-runnable too.
fn cmd_run_verify(script: &std::path::Path, facts: Option<&std::path::Path>) -> u8 {
    use wscript::UnitExt;
    use wscript_std::DynValue;

    let source = match std::fs::read_to_string(script) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", script.display());
            return EXIT_VALIDATION;
        }
    };
    let ctx = hostapi::context();
    let unit = match ctx.compile(&source) {
        Ok(u) => u,
        Err(wscript::Error::Compile(ds)) => {
            print_diags(&Diag::from_wscript(&ds, script, &source));
            return EXIT_VALIDATION;
        }
        Err(e) => {
            eprintln!("error: {}: {e}", script.display());
            return EXIT_VALIDATION;
        }
    };

    let facts_value = match facts {
        Some(path) => {
            let json = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: cannot read {}: {e}", path.display());
                    return EXIT_VALIDATION;
                }
            };
            let parsed: serde_json::Value = match serde_json::from_str(&json) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: invalid facts file {}: {e}", path.display());
                    return EXIT_VALIDATION;
                }
            };
            match convert::json_to_dyn(&parsed) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: invalid facts file {}: {e}", path.display());
                    return EXIT_VALIDATION;
                }
            }
        }
        None => DynValue::Map(std::collections::HashMap::new()),
    };

    let _worker = hostapi::worker_init();
    let mut vm = wscript::Vm::new(&ctx);
    let outcome: Result<bool, String> = if unit.fn_handle::<(DynValue,), bool>("verify").is_ok() {
        vm.call_unit(&unit, "verify", (facts_value,))
            .map_err(|e| e.to_string())
    } else if unit
        .fn_handle::<(DynValue,), Result<bool, String>>("verify")
        .is_ok()
    {
        vm.call_unit::<_, Result<bool, String>>(&unit, "verify", (facts_value,))
            .map_err(|e| e.to_string())
            .and_then(|r| r)
    } else {
        eprintln!(
            "error: {} does not satisfy the 'verify' contract: \
                 fn verify(facts: Value) -> bool (or Result[bool, string])",
            script.display()
        );
        return EXIT_VALIDATION;
    };

    match outcome {
        Ok(true) => {
            println!("verify passed");
            EXIT_OK
        }
        Ok(false) => {
            println!("verify failed");
            EXIT_STEP_ERROR
        }
        Err(e) => {
            println!("verify failed: {e}");
            EXIT_STEP_ERROR
        }
    }
}

fn cmd_docs(
    dir: &std::path::Path,
    outdir: Option<&std::path::Path>,
    serve: bool,
    addr: Option<&str>,
    pkg_only: bool,
) -> u8 {
    // Docs share the validation pipeline: a playbook that doesn't
    // validate doesn't document (PRD §12).
    let pb = match load_validated(dir) {
        Ok(pb) => pb,
        Err(diags) => {
            print_diags(&diags);
            return EXIT_VALIDATION;
        }
    };
    let out = outdir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dir.join("docs"));
    match docsgen::generate(&pb, &out, pkg_only) {
        Ok(pages) => println!("rendered {pages} page(s) to {}", out.display()),
        Err(d) => {
            eprintln!("{}", d.rendered);
            return EXIT_VALIDATION;
        }
    }
    if serve && let Err(d) = docsgen::serve(&out, addr) {
        eprintln!("{}", d.rendered);
        return EXIT_VALIDATION;
    }
    EXIT_OK
}

/// Everything the DocJson subcommands need arrives as one JSON object
/// on stdin; results leave as one JSON object on stdout. Errors are
/// in-band (`ok: false`) so callers never have to parse stderr.
fn read_stdin_json() -> Result<serde_json::Value, String> {
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    serde_json::from_str(&buf).map_err(|e| format!("stdin is not JSON: {e}"))
}

fn docjson_fail(diags: Vec<String>) -> u8 {
    println!("{}", serde_json::json!({ "ok": false, "diags": diags }));
    EXIT_OK
}

fn cmd_wcl_inspect() -> u8 {
    let input = match read_stdin_json() {
        Ok(v) => v,
        Err(e) => return docjson_fail(vec![e]),
    };
    let kind = input["kind"].as_str().unwrap_or_default().to_string();
    let source = input["source"].as_str().unwrap_or_default();
    let ast = match wcl_lang::parse_for_edit(source, "buffer.wcl") {
        Ok(a) => a,
        Err(e) => return docjson_fail(vec![format!("{e}")]),
    };
    let doc = match kind.as_str() {
        "playbook" => model::inspect_ast::extract_playbook(&ast).map(serde_json::to_value),
        "package" => model::inspect_ast::extract_package(&ast).map(serde_json::to_value),
        other => return docjson_fail(vec![format!("unknown kind '{other}'")]),
    };
    match doc {
        Ok(Ok(doc)) => {
            println!(
                "{}",
                serde_json::json!({ "ok": true, "kind": kind, "doc": doc })
            );
            EXIT_OK
        }
        Ok(Err(e)) => docjson_fail(vec![format!("serialize: {e}")]),
        Err(diags) => docjson_fail(diags),
    }
}

fn cmd_wcl_render() -> u8 {
    let input = match read_stdin_json() {
        Ok(v) => v,
        Err(e) => return docjson_fail(vec![e]),
    };
    let kind = input["kind"].as_str().unwrap_or_default();
    let base = input["base_source"].as_str().unwrap_or_default();
    let rendered = match kind {
        "playbook" => serde_json::from_value::<model::docjson::PlaybookDoc>(input["doc"].clone())
            .map_err(|e| vec![format!("doc does not match the playbook shape: {e}")])
            .and_then(|doc| model::emit::render_playbook(base, &doc)),
        "package" => serde_json::from_value::<model::docjson::PackageDoc>(input["doc"].clone())
            .map_err(|e| vec![format!("doc does not match the package shape: {e}")])
            .and_then(|doc| model::emit::render_package(base, &doc)),
        other => return docjson_fail(vec![format!("unknown kind '{other}'")]),
    };
    match rendered {
        Ok(source) => {
            println!("{}", serde_json::json!({ "ok": true, "source": source }));
            EXIT_OK
        }
        Err(diags) => docjson_fail(diags),
    }
}

fn cmd_list(cli: &Cli, dir: &std::path::Path) -> u8 {
    let loaded = model::load(dir);
    let Some(pb) = loaded.playbook else {
        print_diags(&loaded.diags);
        return EXIT_VALIDATION;
    };
    if !loaded.diags.is_empty() {
        print_diags(&loaded.diags);
        return EXIT_VALIDATION;
    }
    if cli.json {
        // The full inventory, not just plays: machine consumers need
        // the packages' tests and scenarios to offer runs.
        let plays: Vec<serde_json::Value> = pb
            .plays
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "steps": p.steps().len(),
                })
            })
            .collect();
        let packages: Vec<serde_json::Value> = pb
            .packages
            .values()
            .map(|pkg| {
                let tests: Vec<serde_json::Value> = pkg
                    .tests
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "backend": t.backend,
                            "image": t.image,
                            "group": t.group,
                        })
                    })
                    .collect();
                let scenarios: Vec<serde_json::Value> = pkg
                    .scenarios
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "description": s.description,
                        })
                    })
                    .collect();
                let params_json = |params: &[model::ParamDecl]| -> Vec<serde_json::Value> {
                    params
                        .iter()
                        .map(|p| {
                            serde_json::json!({
                                "name": p.name,
                                "description": p.description,
                                "type": p.ty.as_str(),
                                "required": p.required,
                                "default": p.default.as_ref().map(convert::dyn_to_json),
                            })
                        })
                        .collect()
                };
                let resources: Vec<serde_json::Value> = pkg
                    .resources
                    .values()
                    .map(|r| {
                        serde_json::json!({
                            "name": r.name,
                            "description": r.description,
                            "concurrency": r.concurrency.as_str(),
                            "params": params_json(&r.params),
                        })
                    })
                    .collect();
                let gatherers: Vec<serde_json::Value> = pkg
                    .gatherers
                    .values()
                    .map(|g| {
                        serde_json::json!({
                            "name": g.name,
                            "description": g.description,
                            "params": params_json(&g.params),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "name": pkg.name,
                    "description": pkg.description,
                    "resources": resources,
                    "gatherers": gatherers,
                    "tests": tests,
                    "scenarios": scenarios,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "playbook": pb.name,
                "version": pb.version,
                "description": pb.description,
                "plays": plays,
                "packages": packages,
            })
        );
        return EXIT_OK;
    }
    println!("{} v{} — {}", pb.name, pb.version, pb.description);
    for play in &pb.plays {
        println!(
            "  {}  ({} step{}) — {}",
            play.name,
            play.steps().len(),
            if play.steps().len() == 1 { "" } else { "s" },
            play.description
        );
    }
    EXIT_OK
}
