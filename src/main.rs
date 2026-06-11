mod convert;
mod diag;
mod engine;
mod hostapi;
mod model;
mod report;
mod vocab;

use engine::status::Mode;
use engine::vars::{Origin, VarStore};

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use diag::Diag;

/// Exit codes per PRD §9.
const EXIT_OK: u8 = 0;
#[allow(dead_code)]
const EXIT_STEP_ERROR: u8 = 1;
const EXIT_VALIDATION: u8 = 2;
#[allow(dead_code)]
const EXIT_REBOOT_REQUIRED: u8 = 3;

#[derive(Parser)]
#[command(name = "config-weave", version, about = "Single-binary configuration management")]
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
    Check { playbook_dir: PathBuf, play: String },
    /// Apply all unconfigured steps in a play.
    Apply { playbook_dir: PathBuf, play: String },
    /// List all plays defined in the playbook.
    List { playbook_dir: PathBuf },
    /// Full validation pipeline, no execution.
    Validate { playbook_dir: PathBuf },
    /// Generate wdoc documentation (default outdir: <dir>/docs/).
    Docs {
        playbook_dir: PathBuf,
        outdir: Option<PathBuf>,
    },
    /// Emit .wispi interface files for the host API plus a starter wisp.toml.
    Wispi { outdir: Option<PathBuf> },
    /// Scaffold a skeleton playbook.
    Init { dir: PathBuf },
    /// Print version information.
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match &cli.command {
        Command::Validate { playbook_dir } => cmd_validate(playbook_dir),
        Command::List { playbook_dir } => cmd_list(playbook_dir),
        Command::Version => {
            println!("config-weave {}", env!("CARGO_PKG_VERSION"));
            EXIT_OK
        }
        Command::Check { playbook_dir, play } => cmd_run(&cli, playbook_dir, play, Mode::Check),
        Command::Apply { playbook_dir, play } => cmd_run(&cli, playbook_dir, play, Mode::Apply),
        Command::Docs { .. } | Command::Wispi { .. } | Command::Init { .. } => {
            eprintln!("error: not implemented yet (lands in M7)");
            EXIT_VALIDATION
        }
    };
    ExitCode::from(code)
}

/// Load + full validation; returns the playbook only when clean.
fn load_validated(dir: &PathBuf) -> Result<model::Playbook, Vec<Diag>> {
    let loaded: model::Loaded = model::load(dir);
    let mut diags = loaded.diags;
    let Some(pb) = loaded.playbook else {
        return Err(diags);
    };
    diags.extend(engine::validate(&pb));
    if diags.is_empty() {
        Ok(pb)
    } else {
        Err(diags)
    }
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

fn cmd_validate(dir: &PathBuf) -> u8 {
    match load_validated(dir) {
        Ok(pb) => {
            let steps: usize = pb.plays.iter().map(|p| p.steps().len()).sum();
            println!(
                "ok: playbook '{}' v{} — {} package(s), {} play(s), {} step(s)",
                pb.name,
                pb.version,
                pb.packages.len(),
                pb.plays.len(),
                steps
            );
            EXIT_OK
        }
        Err(diags) => {
            print_diags(&diags);
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

fn cmd_run(cli: &Cli, dir: &PathBuf, play: &str, mode: Mode) -> u8 {
    let pb = match load_validated(dir) {
        Ok(pb) => pb,
        Err(diags) => {
            print_diags(&diags);
            return EXIT_VALIDATION;
        }
    };
    let store = match override_store(cli) {
        Ok(s) => s,
        Err(diags) => {
            print_diags(&diags);
            return EXIT_VALIDATION;
        }
    };
    match engine::execute(&pb, play, mode, cli.continue_on_error, store) {
        Ok(run_report) => {
            print!("{}", report::plain(&run_report));
            run_report.exit_code()
        }
        Err(diags) => {
            print_diags(&diags);
            EXIT_VALIDATION
        }
    }
}

fn cmd_list(dir: &PathBuf) -> u8 {
    let loaded = model::load(dir);
    let Some(pb) = loaded.playbook else {
        print_diags(&loaded.diags);
        return EXIT_VALIDATION;
    };
    if !loaded.diags.is_empty() {
        print_diags(&loaded.diags);
        return EXIT_VALIDATION;
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
