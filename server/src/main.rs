//! `weave-server` — the config-weave web GUI: browse runbooks in a
//! folder, edit WCL/wisp files, run package tests with live progress,
//! attach a terminal to docker test containers and VNC to vmlab VMs.
//!
//! Built on forge-server (REST + SSE/WS events + JWT + embedded SPA);
//! test execution shells out to the `config-weave` CLI with
//! `--json --events-ndjson` and relays the event stream to the bus.

mod desktop;
mod packages;
mod runbooks;
mod runs;
mod state;
mod sysruns;
mod systems;
mod term;
mod transport;

use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use axum::Extension;
use axum::routing::{get, post};
use clap::Parser;
use forge_server::ForgeApp;

use state::{ServerState, SharedState};

#[derive(rust_embed::RustEmbed)]
#[folder = "../web-ui/dist"]
struct Assets;

#[derive(Parser)]
#[command(
    name = "weave-server",
    version,
    about = "Web UI server for config-weave"
)]
struct Args {
    /// Directory whose child directories (each with a playbook.wcl) are
    /// the runbooks.
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Address to bind. Non-loopback binds are refused without auth
    /// (set FORGE_JWT_SECRET + FORGE_AUTH_USERS) unless --no-auth.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
    /// TCP port.
    #[arg(long, default_value_t = 8765)]
    port: u16,
    /// Allow a non-loopback bind with no login (dangerous: the terminal
    /// widget execs into test containers).
    #[arg(long)]
    no_auth: bool,
    /// The config-weave CLI to shell out to (default: `config-weave`
    /// next to this binary, else on PATH).
    #[arg(long)]
    config_weave: Option<String>,
    /// Static linux binary forwarded to `config-weave test --binary`.
    #[arg(long)]
    test_binary: Option<PathBuf>,
    /// Windows binary forwarded to `--binary-windows`.
    #[arg(long)]
    test_binary_windows: Option<PathBuf>,
    /// Static config-weave build deployed to direct systems, as
    /// KEY=PATH with KEY `{os}-{arch}` (e.g. linux-x86_64=dist/...).
    /// Repeatable. Unset keys fall back to --test-binary(-windows),
    /// then to `just release` artifacts found near this executable.
    #[arg(long, value_name = "KEY=PATH")]
    deploy_binary: Vec<String>,
    /// The package repository: a folder of package dirs (each with a
    /// package.wcl), e.g. a config-weave-pkgs checkout's pkgs/ folder.
    #[arg(long)]
    packages_dir: Option<PathBuf>,
    /// Serve the frontend from a directory instead of the embedded build
    /// (dev: point at web-ui/dist while iterating, or use `pnpm dev`).
    #[arg(long)]
    frontend_dir: Option<PathBuf>,
}

/// The deploy-binary registry: explicit --deploy-binary pairs win, then
/// --test-binary(-windows) for the x86_64 keys, then the freshest
/// `just release` artifact near this executable (same candidate paths
/// the testlab probes).
fn deploy_binaries(args: &Args) -> Result<std::collections::HashMap<String, PathBuf>, String> {
    let mut map = std::collections::HashMap::new();
    for pair in &args.deploy_binary {
        let Some((key, path)) = pair.split_once('=') else {
            return Err(format!("--deploy-binary '{pair}' is not KEY=PATH"));
        };
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(format!("--deploy-binary {key}: {} does not exist", path.display()));
        }
        map.insert(key.to_string(), path);
    }
    let mut fallback = |key: &str, explicit: &Option<PathBuf>, candidates: [&str; 2]| {
        if map.contains_key(key) {
            return;
        }
        if let Some(p) = explicit
            && p.is_file()
        {
            map.insert(key.into(), p.clone());
            return;
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(ws) = exe.ancestors().nth(3)
        {
            let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
            for c in candidates {
                let c = ws.join(c);
                if c.is_file() {
                    let mtime = std::fs::metadata(&c)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                        best = Some((mtime, c));
                    }
                }
            }
            if let Some((_, p)) = best {
                map.insert(key.into(), p);
            }
        }
    };
    fallback(
        "linux-x86_64",
        &args.test_binary,
        [
            "target-cross/x86_64-unknown-linux-musl/release/config-weave",
            "dist/config-weave-linux-x86_64",
        ],
    );
    fallback(
        "windows-x86_64",
        &args.test_binary_windows,
        [
            "target-cross/x86_64-pc-windows-gnu/release/config-weave.exe",
            "dist/config-weave-windows-x86_64.exe",
        ],
    );
    Ok(map)
}

/// `config-weave` beside this executable wins (installed layout), then
/// whatever PATH resolves.
fn default_config_weave() -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(sibling) = exe.parent().map(|d| d.join("config-weave"))
        && sibling.is_file()
    {
        return sibling.to_string_lossy().into_owned();
    }
    "config-weave".into()
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    let root = match args.dir.canonicalize() {
        Ok(r) if r.is_dir() => r,
        _ => {
            eprintln!(
                "weave-server: --dir {} is not a directory",
                args.dir.display()
            );
            return ExitCode::from(2);
        }
    };

    // forge reads auth entirely from env (FORGE_JWT_SECRET etc. or the
    // .env in cwd, loaded by ForgeApp::new). The bind policy on top:
    // secure by default for non-loopback binds.
    let mut app = ForgeApp::new("weave-server").with_events();
    let auth_enabled = std::env::var("FORGE_JWT_SECRET").is_ok();
    if auth_enabled {
        app = app.auth_from_env();
    } else if !args.bind.is_loopback() && !args.no_auth {
        eprintln!(
            "weave-server: refusing a non-loopback bind ({}) with no login — set \
             FORGE_JWT_SECRET + FORGE_AUTH_USERS (≥32-char secret), or pass --no-auth \
             to opt in (the terminal widget gives shell access to test containers)",
            args.bind
        );
        return ExitCode::from(2);
    }

    // A malformed systems.wcl refuses startup: a later GUI save would
    // regenerate (and so clobber) a file we could not fully read.
    let systems_path = root.join("systems.wcl");
    let loaded_systems = match systems::load(&systems_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };
    let deploy = match deploy_binaries(&args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };
    let packages_dir = match &args.packages_dir {
        None => None,
        Some(d) => match d.canonicalize() {
            Ok(c) if c.is_dir() => Some(c),
            _ => {
                eprintln!(
                    "weave-server: --packages-dir {} is not a directory",
                    d.display()
                );
                return ExitCode::from(2);
            }
        },
    };

    let state: SharedState = Arc::new(ServerState {
        root: root.clone(),
        config_weave: args.config_weave.unwrap_or_else(default_config_weave),
        test_binary: args.test_binary,
        test_binary_windows: args.test_binary_windows,
        runs: runs::RunManager::default(),
        events: app.event_bus(),
        systems_path,
        systems: std::sync::Mutex::new(loaded_systems),
        deploy_binaries: deploy,
        sysruns: sysruns::SysRunManager::default(),
        packages_dir,
        pkg_wrapper: packages::WrapperCache::default(),
    });

    app = app
        .route("/api/runbooks", get(runbooks::list))
        .route("/api/runbooks/{rb}/tree", get(runbooks::tree))
        .route(
            "/api/runbooks/{rb}/file",
            get(runbooks::file_get).put(runbooks::file_put),
        )
        .route("/api/runbooks/{rb}/validate", post(runbooks::validate))
        .route("/api/runbooks/{rb}/inventory", get(runbooks::inventory))
        .route("/api/systems", get(systems::list).post(systems::create))
        .route(
            "/api/systems/{name}",
            axum::routing::put(systems::update).delete(systems::delete),
        )
        .route("/api/systems/{name}/runs", post(sysruns::create))
        .route("/api/system-runs", get(sysruns::list))
        .route("/api/system-runs/{id}", get(sysruns::get))
        .route("/api/system-runs/{id}/cancel", post(sysruns::cancel))
        .route("/api/packages", get(packages::list))
        .route("/api/packages/{name}", get(packages::detail))
        .route(
            "/api/packages/{name}/add-to-runbook",
            post(packages::add_to_runbook),
        )
        .route("/api/packages/{name}/test", post(packages::run_tests))
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/{id}", get(runs::get))
        .route("/api/runs/{id}/cancel", post(runs::cancel))
        .route("/api/runs/{id}/teardown", post(runs::teardown))
        .route("/api/term/docker/{container}", get(term::docker_term))
        .route("/api/desktop/vnc/{run}/{machine}", get(desktop::vnc));

    app = match &args.frontend_dir {
        Some(dir) => app.frontend_dir(dir),
        None => app.frontend_embedded::<Assets>(),
    };

    let router = match app.try_router() {
        Ok(r) => r.layer(Extension(state)),
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };

    init_tracing();
    println!(
        "weave-server: serving runbooks from {} on http://{}:{} (auth {})",
        root.display(),
        args.bind,
        args.port,
        if auth_enabled { "enabled" } else { "disabled" }
    );

    let listener = match tokio::net::TcpListener::bind((args.bind, args.port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("weave-server: cannot bind {}:{}: {e}", args.bind, args.port);
            return ExitCode::from(2);
        }
    };
    match axum::serve(listener, router).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("weave-server: {e}");
            ExitCode::FAILURE
        }
    }
}

/// forge-server's `serve()` normally initializes tracing; we serve the
/// router ourselves, so do the minimal equivalent here.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")),
        )
        .try_init();
}
