//! `config-weave-pipeline` — a CI/CD primitives daemon. It loads
//! `pipeline.wcl` (a `pipelines.wcl` inventory under `--dir`), and runs
//! triggered pipelines: ordered steps that are either shell scripts (local
//! or over ssh/winrm) or config-weave plays (shelled out to the CLI).
//! Triggers are manual (API), git webhooks, or cron schedules. Auth is
//! forge-auth machine + user JWTs (RS256/JWKS).
//!
//! Built on forge-server (REST + SSE/WS events + a pluggable JWT
//! validator); play execution shells out to the `config-weave` CLI with
//! `--json --events-ndjson` and relays the event stream to the bus.

mod auth;
mod exec;
mod pipelines;
mod runs;
mod scheduler;
mod secrets;
mod state;
mod trigger;
mod webhooks;

use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use axum::Extension;
use axum::routing::{get, post};
use clap::Parser;
use forge_server::ForgeApp;

use state::{PipelineState, SharedState};

#[derive(Parser)]
#[command(
    name = "config-weave-pipeline",
    version,
    about = "CI/CD primitives daemon for config-weave"
)]
struct Args {
    /// Directory holding `pipelines.wcl` (created/served here).
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Root under which a play step's `playbook` name resolves to a
    /// playbook dir. Default: `playbooks/` inside --dir (created on demand).
    #[arg(long)]
    playbooks_dir: Option<PathBuf>,
    /// Address to bind. Non-loopback binds are refused without forge-auth
    /// configured, unless --no-auth.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
    /// TCP port.
    #[arg(long, default_value_t = 8770)]
    port: u16,
    /// The config-weave CLI to shell out to for play steps (default:
    /// `config-weave` next to this binary, else on PATH).
    #[arg(long)]
    config_weave: Option<String>,
    /// forge-auth issuer URL (the `iss` claim validated on tokens).
    #[arg(long, env = "FORGE_AUTH_ISSUER")]
    forge_issuer: Option<String>,
    /// forge-auth JWKS URL (default: `{forge_issuer}/.well-known/jwks.json`).
    #[arg(long, env = "FORGE_AUTH_JWKS_URL")]
    forge_jwks_url: Option<String>,
    /// Required token audience (the `aud` claim). Unset = audience not
    /// checked (forge-auth leaves `aud` policy to the resource server).
    #[arg(long, env = "FORGE_AUTH_AUDIENCE")]
    forge_audience: Option<String>,
    /// Allow a non-loopback bind with no auth (dangerous).
    #[arg(long)]
    no_auth: bool,
}

/// `config-weave` beside this executable wins (installed layout), then PATH.
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
    init_tracing();

    let root = match args.dir.canonicalize() {
        Ok(r) if r.is_dir() => r,
        _ => {
            eprintln!(
                "config-weave-pipeline: --dir {} is not a directory",
                args.dir.display()
            );
            return ExitCode::from(2);
        }
    };

    // A malformed pipelines.wcl refuses startup: a later save would
    // regenerate (and so clobber) a file we could not fully read.
    let pipelines_path = root.join("pipelines.wcl");
    let loaded = match pipelines::load(&pipelines_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("config-weave-pipeline: {e}");
            return ExitCode::from(2);
        }
    };

    let playbooks_dir = match &args.playbooks_dir {
        Some(d) => match d.canonicalize() {
            Ok(c) if c.is_dir() => c,
            _ => {
                eprintln!(
                    "config-weave-pipeline: --playbooks-dir {} is not a directory",
                    d.display()
                );
                return ExitCode::from(2);
            }
        },
        None => {
            let default = root.join("playbooks");
            match std::fs::create_dir_all(&default).and_then(|_| default.canonicalize()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "config-weave-pipeline: cannot use default playbooks dir {}: {e}",
                        default.display()
                    );
                    return ExitCode::from(2);
                }
            }
        }
    };

    // Auth: forge-auth RS256/JWKS when an issuer is configured; otherwise a
    // loopback bind runs open (anonymous), a non-loopback bind is refused
    // unless --no-auth.
    let mut app = ForgeApp::new("config-weave-pipeline").with_events();
    let auth_enabled = if let Some(issuer) = &args.forge_issuer {
        let jwks_url = args
            .forge_jwks_url
            .clone()
            .unwrap_or_else(|| format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/')));
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        match auth::ForgeAuthValidator::connect(
            issuer.clone(),
            jwks_url,
            args.forge_audience.clone(),
            http,
        )
        .await
        {
            Ok(validator) => {
                app = app.auth_validator(validator);
                true
            }
            Err(e) => {
                eprintln!("config-weave-pipeline: forge-auth setup failed: {e}");
                return ExitCode::from(2);
            }
        }
    } else if !args.bind.is_loopback() && !args.no_auth {
        eprintln!(
            "config-weave-pipeline: refusing a non-loopback bind ({}) with no auth — set \
             --forge-issuer (forge-auth) or pass --no-auth to opt in",
            args.bind
        );
        return ExitCode::from(2);
    } else {
        false
    };

    let state: SharedState = Arc::new(PipelineState {
        pipelines_path,
        pipelines: std::sync::Mutex::new(loaded),
        playbooks_dir,
        config_weave: args.config_weave.unwrap_or_else(default_config_weave),
        runs: runs::PipelineRunManager::default(),
        events: app.event_bus(),
    });
    scheduler::spawn(state.clone());

    app = app
        .route(
            "/api/pipelines",
            get(pipelines::list).post(pipelines::create),
        )
        .route(
            "/api/pipelines/{name}",
            get(pipelines::get)
                .put(pipelines::update)
                .delete(pipelines::delete),
        )
        .route("/api/pipelines/{name}/trigger", post(trigger::trigger))
        .route(
            "/api/pipelines/{name}/secrets",
            get(pipelines::list_secrets),
        )
        .route(
            "/api/pipelines/{name}/secrets/{secret}",
            axum::routing::put(pipelines::set_secret).delete(pipelines::delete_secret),
        )
        .route("/api/runs", get(trigger::list_runs))
        .route("/api/runs/{id}", get(trigger::get_run))
        .route("/api/runs/{id}/cancel", post(trigger::cancel_run))
        // Open (no JWT): authenticated per trigger by webhook_secret.
        .route(
            "/api/webhooks/pipelines/{name}/{trigger}",
            post(webhooks::webhook),
        );

    let router = match app.try_router() {
        Ok(r) => r.layer(Extension(state)),
        Err(e) => {
            eprintln!("config-weave-pipeline: {e}");
            return ExitCode::from(2);
        }
    };

    println!(
        "config-weave-pipeline: serving pipelines from {} on http://{}:{} (auth {})",
        root.display(),
        args.bind,
        args.port,
        if auth_enabled { "enabled" } else { "disabled" }
    );

    let listener = match tokio::net::TcpListener::bind((args.bind, args.port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "config-weave-pipeline: cannot bind {}:{}: {e}",
                args.bind, args.port
            );
            return ExitCode::from(2);
        }
    };
    match axum::serve(listener, router).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("config-weave-pipeline: {e}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
