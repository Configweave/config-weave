//! `weave-server` — the config-weave web GUI: browse runbooks in a
//! folder, edit WCL/wisp files, run package tests with live progress,
//! attach a terminal to docker test containers and VNC to vmlab VMs.
//!
//! Built on forge-server (REST + SSE/WS events + JWT + embedded SPA);
//! test execution shells out to the `config-weave` CLI with
//! `--json --events-ndjson` and relays the event stream to the bus.

mod desktop;
mod runbooks;
mod runs;
mod state;
mod term;

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
    /// Serve the frontend from a directory instead of the embedded build
    /// (dev: point at web-ui/dist while iterating, or use `pnpm dev`).
    #[arg(long)]
    frontend_dir: Option<PathBuf>,
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

    let state: SharedState = Arc::new(ServerState {
        root: root.clone(),
        config_weave: args.config_weave.unwrap_or_else(default_config_weave),
        test_binary: args.test_binary,
        test_binary_windows: args.test_binary_windows,
        runs: runs::RunManager::default(),
        events: app.event_bus(),
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
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/{id}", get(runs::get))
        .route("/api/runs/{id}/cancel", post(runs::cancel))
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
