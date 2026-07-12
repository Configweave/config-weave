//! `weave-server` — the config-weave web GUI: browse runbooks in a
//! folder, edit WCL/wisp files, run package tests with live progress,
//! attach a terminal to docker test containers and VNC to vmlab VMs.
//!
//! Built on forge-server (REST + SSE/WS events + JWT + embedded SPA);
//! test execution shells out to the `config-weave` CLI with
//! `--json --events-ndjson` and relays the event stream to the bus.

mod desktop;
mod monitoring;
mod packages;
mod pipeline_proxy;
mod repos;
mod runbooks;
mod runs;
mod scheduler;
mod state;
mod sysruns;
mod systems;
mod term;
mod webhooks;
mod zips;

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
    /// Default: `packages/` inside --dir (created when missing).
    #[arg(long)]
    packages_dir: Option<PathBuf>,
    /// Serve the frontend from a directory instead of the embedded build
    /// (dev: point at web-ui/dist while iterating, or use `pnpm dev`).
    #[arg(long)]
    frontend_dir: Option<PathBuf>,
    /// Prometheus base URL: enables the per-service Monitoring tab
    /// (queries are proxied server-side, never from the browser).
    #[arg(long, env = "PROMETHEUS_URL")]
    prometheus_url: Option<url::Url>,
    /// Loki base URL: ships server + run logs to Loki and enables the
    /// per-service Logs tab (also proxied server-side).
    #[arg(long, env = "LOKI_URL")]
    loki_url: Option<url::Url>,
    /// user.name stamped onto commit-and-push commits to remote repos.
    #[arg(long, default_value = "weave-server")]
    git_user_name: String,
    /// user.email stamped onto commit-and-push commits.
    #[arg(long, default_value = "weave-server@localhost")]
    git_user_email: String,
    /// config-weave-pipeline daemon base URL: enables the Pipelines section
    /// (calls are proxied server-side with a forge-auth machine token).
    #[arg(long, env = "PIPELINE_URL")]
    pipeline_url: Option<url::Url>,
    /// A static forge-auth machine token forwarded to the pipeline daemon.
    #[arg(long, env = "PIPELINE_TOKEN")]
    pipeline_token: Option<String>,
    /// Auto-refresh the pipeline machine token via forge-auth's
    /// `refresh_token` grant (needs --pipeline-token-url + --pipeline-client-id).
    #[arg(long, env = "PIPELINE_REFRESH_TOKEN")]
    pipeline_refresh_token: Option<String>,
    /// forge-auth token endpoint for the refresh grant.
    #[arg(long, env = "PIPELINE_TOKEN_URL")]
    pipeline_token_url: Option<String>,
    /// forge-auth client id for the refresh grant.
    #[arg(long, env = "PIPELINE_CLIENT_ID")]
    pipeline_client_id: Option<String>,
    /// forge-auth client secret for the refresh grant (confidential clients).
    #[arg(long, env = "PIPELINE_CLIENT_SECRET")]
    pipeline_client_secret: Option<String>,
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
            return Err(format!(
                "--deploy-binary {key}: {} does not exist",
                path.display()
            ));
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

    // Tracing first (so startup + scheduler logs ship to Loki too), then
    // the metrics recorder (metrics! calls before install are dropped).
    init_tracing(args.loki_url.as_ref());
    let (prom_layer, prom_handle) = monitoring::setup();

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

    // A malformed services.wcl refuses startup: a later GUI save would
    // regenerate (and so clobber) a file we could not fully read.
    let services_path = root.join("services.wcl");
    let loaded_services = match systems::load(&services_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };
    // repos.wcl follows the same rule; a *missing* file is seeded with
    // the stdlib so a fresh server has packages out of the box (a present
    // file — even empty — is respected, so deleting the stdlib sticks).
    let repos_path = root.join("repos.wcl");
    let loaded_repos = match repos::load(&repos_path) {
        Ok(Some(r)) => r,
        Ok(None) => {
            let seeded = vec![repos::stdlib_default()];
            match repos::save(&repos_path, &seeded) {
                Ok(()) => seeded,
                Err(e) => {
                    eprintln!("weave-server: cannot seed {}: {e}", repos_path.display());
                    return ExitCode::from(2);
                }
            }
        }
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };
    let repo_cache = root.join(".repo-cache");
    if let Err(e) = std::fs::create_dir_all(&repo_cache) {
        eprintln!("weave-server: cannot create {}: {e}", repo_cache.display());
        return ExitCode::from(2);
    }

    let deploy = match deploy_binaries(&args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };
    let packages_dir = match &args.packages_dir {
        // An explicit flag must point at a real directory.
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
        // Default: `packages/` inside the served root, created on
        // demand so the Packages section works with zero flags. A
        // creation failure only downgrades to the unconfigured hint.
        None => {
            let default = root.join("packages");
            match std::fs::create_dir_all(&default).and_then(|_| default.canonicalize()) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!(
                        "weave-server: cannot use default packages dir {}: {e}",
                        default.display()
                    );
                    None
                }
            }
        }
    };

    let state: SharedState = Arc::new(ServerState {
        root: root.clone(),
        config_weave: args.config_weave.unwrap_or_else(default_config_weave),
        test_binary: args.test_binary,
        test_binary_windows: args.test_binary_windows,
        runs: runs::RunManager::default(),
        events: app.event_bus(),
        services_path,
        services: std::sync::Mutex::new(loaded_services),
        deploy_binaries: deploy,
        sysruns: sysruns::SysRunManager::default(),
        packages_dir,
        pkg_wrapper: packages::WrapperCache::default(),
        repos_path,
        repos: std::sync::Mutex::new(loaded_repos),
        repo_cache,
        repo_git_lock: tokio::sync::Mutex::new(()),
        git_identity: (args.git_user_name, args.git_user_email),
        prometheus_url: args.prometheus_url,
        loki_url: args.loki_url,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("reqwest client"),
        pipeline: {
            // A refresh config needs the refresh token, its endpoint, and a
            // client id; otherwise fall back to the static token.
            let refresh = match (
                args.pipeline_refresh_token,
                args.pipeline_token_url,
                args.pipeline_client_id,
            ) {
                (Some(refresh_token), Some(token_url), Some(client_id)) => {
                    Some(pipeline_proxy::RefreshConfig {
                        token_url,
                        client_id,
                        client_secret: args.pipeline_client_secret,
                        refresh_token,
                    })
                }
                _ => None,
            };
            pipeline_proxy::PipelineProxy::new(args.pipeline_url, args.pipeline_token, refresh)
        },
    });
    scheduler::spawn(state.clone());
    repos::spawn_initial_clones(state.clone());

    app = app
        .route("/api/playbooks", get(runbooks::list))
        .route("/api/playbooks/{rb}/tree", get(runbooks::tree))
        .route(
            "/api/playbooks/{rb}/file",
            get(runbooks::file_get).put(runbooks::file_put),
        )
        .route("/api/playbooks/{rb}/validate", post(runbooks::validate))
        .route("/api/playbooks/{rb}/inventory", get(runbooks::inventory))
        .route("/api/playbooks/{rb}/doc/parse", post(runbooks::doc_parse))
        .route("/api/playbooks/{rb}/doc/render", post(runbooks::doc_render))
        .route(
            "/api/playbooks/{rb}/doc",
            axum::routing::put(runbooks::doc_save),
        )
        .route("/api/templates", get(runbooks::templates))
        .route(
            "/api/services",
            get(systems::list).post(systems::create_service),
        )
        .route(
            "/api/services/{name}",
            axum::routing::put(systems::update_service).delete(systems::delete_service),
        )
        .route(
            "/api/services/{service}/systems",
            post(systems::create_system),
        )
        .route(
            "/api/services/{service}/systems/{name}",
            axum::routing::put(systems::update_system).delete(systems::delete_system),
        )
        .route(
            "/api/services/{service}/systems/{name}/runs",
            post(sysruns::create),
        )
        .route(
            "/api/services/{service}/schedules",
            post(systems::create_schedule),
        )
        .route(
            "/api/services/{service}/schedules/{name}",
            axum::routing::put(systems::update_schedule).delete(systems::delete_schedule),
        )
        .route(
            "/api/services/{service}/schedules/{name}/run",
            post(scheduler::run_now),
        )
        .route("/api/system-runs", get(sysruns::list))
        .route("/api/system-runs/{id}", get(sysruns::get))
        .route("/api/system-runs/{id}/cancel", post(sysruns::cancel))
        .route("/api/repos", get(repos::list).post(repos::create))
        .route("/api/repos/sync", post(repos::sync_all))
        .route(
            "/api/repos/{name}",
            get(repos::get_one).put(repos::update).delete(repos::remove),
        )
        .route("/api/repos/{name}/sync", post(repos::sync_one))
        .route("/api/repos/{name}/commit", post(repos::commit))
        .route("/api/repos/{name}/discard", post(repos::discard))
        // Open (no claims): authenticated per repo by webhook_secret.
        .route("/api/webhooks/repos/{name}", post(webhooks::webhook))
        .route("/api/playbooks/{rb}/download", get(zips::download))
        .route(
            "/api/playbooks/upload",
            post(zips::upload).layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/api/packages", get(packages::list))
        .route("/api/packages/{name}", get(packages::detail))
        .route(
            "/api/packages/{name}/add-to-playbook",
            post(packages::add_to_runbook),
        )
        .route("/api/packages/{name}/test", post(packages::run_tests))
        .route("/api/packages/{name}/docs", get(packages::docs))
        .route("/api/packages/{name}/tree", get(packages::tree))
        .route(
            "/api/packages/{name}/file",
            get(packages::file_get).put(packages::file_put),
        )
        .route("/api/packages/{name}/doc/parse", post(packages::doc_parse))
        .route(
            "/api/packages/{name}/doc/render",
            post(packages::doc_render),
        )
        .route(
            "/api/packages/{name}/doc",
            axum::routing::put(packages::doc_save),
        )
        .route(
            "/api/playbooks/{rb}/packages/{name}",
            axum::routing::delete(packages::remove_from_runbook),
        )
        .route(
            "/api/playbooks/{rb}/packages/{name}/import",
            post(packages::import_to_repo),
        )
        .route(
            "/api/playbooks/{rb}/packages/{name}/docs",
            get(packages::runbook_docs),
        )
        .route("/api/runs", post(runs::create))
        .route("/api/runs/{id}", get(runs::get))
        .route("/api/runs/{id}/cancel", post(runs::cancel))
        .route("/api/runs/{id}/teardown", post(runs::teardown))
        .route("/api/term/docker/{container}", get(term::docker_term))
        .route("/api/desktop/vnc/{run}/{machine}", get(desktop::vnc))
        // Open (no claims) so Prometheus can scrape.
        .route(
            "/metrics",
            get(move || std::future::ready(prom_handle.render())),
        )
        .route("/api/monitoring/status", get(monitoring::status))
        .route(
            "/api/services/{service}/monitoring/summary",
            get(monitoring::summary),
        )
        .route(
            "/api/services/{service}/monitoring/timeseries",
            get(monitoring::timeseries),
        )
        .route("/api/services/{service}/logs", get(monitoring::logs))
        // Pipelines section: a capability probe + a catch-all reverse proxy
        // to the config-weave-pipeline daemon (machine token attached
        // server-side). The browser calls /api/pipeline/pipelines,
        // /api/pipeline/runs/{id}, etc.
        .route("/api/pipeline-config", get(pipeline_proxy::config))
        .route(
            "/api/pipeline/{*rest}",
            get(pipeline_proxy::proxy)
                .post(pipeline_proxy::proxy)
                .put(pipeline_proxy::proxy)
                .delete(pipeline_proxy::proxy),
        );

    app = match &args.frontend_dir {
        Some(dir) => app.frontend_dir(dir),
        None => app.frontend_embedded::<Assets>(),
    };

    let router = match app.try_router() {
        // Router::layer runs after routing, so the metrics layer sees
        // MatchedPath and groups by route template.
        Ok(r) => r.layer(Extension(state)).layer(prom_layer),
        Err(e) => {
            eprintln!("weave-server: {e}");
            return ExitCode::from(2);
        }
    };

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
/// router ourselves, so build the subscriber here: the usual console
/// fmt layer, plus a Loki push layer when --loki-url is set. Run-output
/// events (target `weave::runlog`, one per engine line) go to Loki only —
/// the console keeps its existing signal.
fn init_tracing(loki: Option<&url::Url>) {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    use tracing_subscriber::{EnvFilter, Layer as _};

    let fmt_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn"))
        .add_directive("weave::runlog=off".parse().expect("static directive"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(fmt_filter);

    // Static labels only ({app, level}); service/system/run_id etc. ride
    // as JSON fields — label cardinality stays flat.
    let loki_layer = loki.and_then(|url| {
        let built = tracing_loki::builder()
            .label("app", "weave-server")
            .and_then(|b| b.build_url(url.clone()));
        match built {
            Ok((layer, task)) => {
                tokio::spawn(task);
                Some(layer.with_filter(EnvFilter::new("info")))
            }
            Err(e) => {
                eprintln!("weave-server: loki logging disabled: {e}");
                None
            }
        }
    });

    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(loki_layer)
        .try_init();
}
