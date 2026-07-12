//! Remote repositories: `{root}/repos.wcl` — git repos cloned into
//! `{root}/.repo-cache/<name>` whose packages and runbooks merge into
//! the package repository and runbook list (tagged by source, editable
//! with commit-and-push write-back). Loaded through the embedded
//! vocabulary and regenerated from structs on every GUI edit, same as
//! the services inventory. A missing file is seeded with the stdlib
//! repo; a present file (even empty) is respected, so removing the
//! stdlib sticks. Sync never touches a repo with local changes or
//! unpushed commits — Commit & push or Discard are the only ways out.

use std::path::{Path, PathBuf};

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wcl_lang::{Document, Environment, Registry, ast, disk_loader, edit, format as wclformat};

use crate::runbooks::valid_name;
use crate::state::SharedState;

/// One source of truth: the vocab embedded in the CLI crate.
const REPOS_VOCAB: &str = include_str!("../../src/vocab/repos.wcl");
const REPOS_IMPORT: &str = "weave/repos.wcl";

/// The source tag for packages from the local --packages-dir; repo names
/// may not collide with it.
pub const LOCAL_SOURCE: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDef {
    pub name: String,
    pub url: String,
    /// Subdirectory of the checkout holding the package dirs (e.g.
    /// "pkgs" for the stdlib); the checkout root when unset.
    #[serde(default)]
    pub subdir: Option<String>,
    /// Subdirectory of the checkout holding runbook dirs ("." for the
    /// checkout root); the repo provides no runbooks when unset.
    #[serde(default)]
    pub runbooks_subdir: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    /// Six-field cron (same dialect as service schedules); a due tick
    /// syncs the repo. Unset = manual/webhook sync only.
    #[serde(default)]
    pub sync_cron: Option<String>,
    /// Enables POST /api/webhooks/repos/{name}: GitHub/Gitea HMAC
    /// (X-Hub-Signature-256) or plain token (X-Gitlab-Token /
    /// X-Weave-Token) against this value.
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

/// The default entry seeded into a missing repos.wcl.
pub fn stdlib_default() -> RepoDef {
    RepoDef {
        name: "stdlib".into(),
        url: "https://github.com/Configweave/config-weave-pkgs.git".into(),
        subdir: Some("pkgs".into()),
        runbooks_subdir: None,
        branch: None,
        sync_cron: None,
        webhook_secret: None,
    }
}

/// The repo's clone under `{root}/.repo-cache` (a dot-dir, so runbook
/// listing and the local package scan never see it).
pub fn cache_dir(state: &SharedState, name: &str) -> PathBuf {
    state.repo_cache.join(name)
}

/// The directory holding the repo's package dirs, only once it exists
/// (the clone may be pending or failed).
pub fn packages_root(state: &SharedState, repo: &RepoDef) -> Option<PathBuf> {
    let mut dir = cache_dir(state, &repo.name);
    if let Some(sub) = &repo.subdir {
        dir = dir.join(sub);
    }
    dir.is_dir().then_some(dir)
}

/// The directory holding the repo's runbook dirs, only when the repo
/// declares one ("." = the checkout root) and the clone exists.
pub fn runbooks_root(state: &SharedState, repo: &RepoDef) -> Option<PathBuf> {
    let sub = repo.runbooks_subdir.as_deref()?;
    let mut dir = cache_dir(state, &repo.name);
    if sub != "." {
        dir = dir.join(sub);
    }
    dir.is_dir().then_some(dir)
}

// ------------------------------------------------------------------ load

fn repos_loader() -> wcl_lang::FileLoader {
    let mut reg = Registry::new();
    reg.register(REPOS_IMPORT, REPOS_VOCAB);
    reg.loader(disk_loader())
}

/// Read and schema-validate `repos.wcl`. `Ok(None)` means the file is
/// missing (the caller seeds the default); a malformed file is an error
/// (a later GUI save would clobber a file we could not fully read).
pub fn load(path: &Path) -> Result<Option<Vec<RepoDef>>, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };

    let mut with_import = source.clone();
    if !with_import.ends_with('\n') {
        with_import.push('\n');
    }
    with_import.push_str(&format!("import <{REPOS_IMPORT}>\n"));

    let env = Environment::new();
    let doc = Document::open_at_with_loader(
        &with_import,
        "repos.wcl",
        path.parent().map(|p| p.to_path_buf()),
        &env,
        repos_loader(),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;

    let schema_errors = doc.schema_errors();
    if !schema_errors.is_empty() {
        let msgs: Vec<String> = schema_errors.iter().map(|e| e.to_string()).collect();
        return Err(format!("{}: {}", path.display(), msgs.join("; ")));
    }

    let mut repos = Vec::new();
    for block in doc.blocks() {
        if block.kind() != "repo" {
            continue;
        }
        repos.push(read_repo(&block).map_err(|e| format!("{}: {e}", path.display()))?);
    }

    let mut seen = std::collections::HashSet::new();
    for repo in &repos {
        if !seen.insert(repo.name.as_str()) {
            return Err(format!(
                "{}: duplicate repository name '{}'",
                path.display(),
                repo.name
            ));
        }
        if let Err(e) = validate(repo) {
            return Err(format!("{}: repo '{}': {e}", path.display(), repo.name));
        }
    }
    Ok(Some(repos))
}

fn read_repo(block: &wcl_lang::Block<'_>) -> Result<RepoDef, String> {
    let name = match block
        .labels()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        Some(wcl_lang::Value::Utf8(s))
        | Some(wcl_lang::Value::Ascii(s))
        | Some(wcl_lang::Value::Identifier(s)) => s,
        _ => return Err("repo block has no name label".into()),
    };
    let str_field = |field: &str| -> Result<Option<String>, String> {
        let Some(f) = block.fields().find(|f| f.name() == field) else {
            return Ok(None);
        };
        match f.value().map_err(|e| e.to_string())?.clone() {
            wcl_lang::Value::Utf8(s)
            | wcl_lang::Value::Ascii(s)
            | wcl_lang::Value::Identifier(s) => Ok(Some(s)),
            other => Err(format!("field '{field}' must be a string, got {other:?}")),
        }
    };
    Ok(RepoDef {
        url: str_field("url")?.ok_or_else(|| format!("repo '{name}': missing field 'url'"))?,
        subdir: str_field("subdir")?,
        runbooks_subdir: str_field("runbooks_subdir")?,
        branch: str_field("branch")?,
        sync_cron: str_field("sync_cron")?,
        webhook_secret: str_field("webhook_secret")?,
        name,
    })
}

/// A subdir joins under the checkout: relative, no '..', no empty
/// components. "." (the checkout root) is allowed.
fn valid_subdir(sub: &str) -> bool {
    sub == "."
        || (!sub.is_empty()
            && !sub.starts_with('/')
            && sub.split('/').all(|c| c != ".." && !c.is_empty()))
}

/// Structural checks shared by load and the create handler. The name
/// becomes a cache directory and a source tag; the subdirs join under
/// the checkout.
fn validate(repo: &RepoDef) -> Result<(), String> {
    if !valid_name(&repo.name) {
        return Err("name must be alphanumeric with - _ . and not start with '.'".into());
    }
    if repo.name == LOCAL_SOURCE {
        return Err(format!(
            "'{LOCAL_SOURCE}' is reserved for the local repository"
        ));
    }
    if repo.url.is_empty() {
        return Err("url must not be empty".into());
    }
    if let Some(sub) = &repo.subdir
        && !valid_subdir(sub)
    {
        return Err("subdir must be a relative path without '..'".into());
    }
    if let Some(sub) = &repo.runbooks_subdir
        && !valid_subdir(sub)
    {
        return Err("runbooks_subdir must be a relative path without '..'".into());
    }
    if let Some(cron) = &repo.sync_cron
        && <cron::Schedule as std::str::FromStr>::from_str(cron).is_err()
    {
        return Err("sync_cron must be a valid six-field cron expression".into());
    }
    if let Some(secret) = &repo.webhook_secret
        && secret.len() < 8
    {
        return Err("webhook_secret must be at least 8 characters".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ save

/// Regenerate `repos.wcl` from the list: fresh AST through the canonical
/// printer, written atomically.
pub fn save(path: &Path, repos: &[RepoDef]) -> Result<(), String> {
    let mut src = ast::Source {
        items: Vec::new(),
        trailing_trivia: Vec::new(),
    };
    for repo in repos {
        let mut fields = vec![("url".into(), edit::string_literal_expr(&repo.url))];
        if let Some(sub) = &repo.subdir {
            fields.push(("subdir".into(), edit::string_literal_expr(sub)));
        }
        if let Some(sub) = &repo.runbooks_subdir {
            fields.push(("runbooks_subdir".into(), edit::string_literal_expr(sub)));
        }
        if let Some(branch) = &repo.branch {
            fields.push(("branch".into(), edit::string_literal_expr(branch)));
        }
        if let Some(cron) = &repo.sync_cron {
            fields.push(("sync_cron".into(), edit::string_literal_expr(cron)));
        }
        if let Some(secret) = &repo.webhook_secret {
            fields.push(("webhook_secret".into(), edit::string_literal_expr(secret)));
        }
        edit::append_top_level_block(
            &mut src,
            edit::build_block(
                "repo",
                &[],
                vec![edit::string_literal_expr(&repo.name)],
                fields,
            ),
        );
    }

    let header = [
        " Config Weave remote repositories — managed by weave-server.",
        " GUI edits regenerate this file; hand edits survive a reload but",
        " not the next GUI save. Delete an entry to stop syncing it (the",
        " stdlib is only seeded when this file is missing).",
    ];
    match src.items.first_mut() {
        Some(ast::Item::Block(b)) => {
            let mut trivia: Vec<ast::Trivia> = header
                .iter()
                .map(|l| ast::Trivia::LineComment(l.to_string()))
                .collect();
            trivia.push(ast::Trivia::BlankLine);
            trivia.append(&mut b.leading_trivia);
            b.leading_trivia = trivia;
        }
        _ => {
            src.trailing_trivia = header
                .iter()
                .map(|l| ast::Trivia::LineComment(l.to_string()))
                .collect();
        }
    }

    let rendered = wclformat::to_source(&src);
    let tmp = path.with_extension("wcl.weave-tmp");
    std::fs::write(&tmp, &rendered).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    // The file may carry webhook secrets — same posture as services.wcl.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot chmod {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

// ------------------------------------------------------------------- git

/// Run git with stderr captured; auth prompts are disabled so a private
/// URL fails fast instead of hanging the handler. Returns trimmed
/// stdout.
async fn git_output(args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "git {} failed: {}",
        args.first()
            .map(|a| if *a == "-C" {
                args.get(2).copied().unwrap_or("")
            } else {
                a
            })
            .unwrap_or(""),
        stderr.trim()
    ))
}

async fn git(args: &[&str]) -> Result<(), String> {
    git_output(args).await.map(|_| ())
}

/// A cache's relationship to its remote: `dirty` = uncommitted edits,
/// `ahead` = committed but unpushed. Either blocks sync.
pub struct RepoStatus {
    pub dirty: bool,
    pub ahead: u32,
}

impl RepoStatus {
    pub fn clean(&self) -> bool {
        !self.dirty && self.ahead == 0
    }
}

pub async fn repo_status(dest: &Path) -> Result<RepoStatus, String> {
    let dest_s = dest.to_string_lossy().into_owned();
    let porcelain = git_output(&["-C", &dest_s, "status", "--porcelain"]).await?;
    // Our clones are single-branch, so @{upstream} is always configured;
    // a hand-mutated cache (detached HEAD) loses the probe — treat as
    // in-sync and let Discard repair it.
    let ahead = match git_output(&["-C", &dest_s, "rev-list", "--count", "@{upstream}..HEAD"]).await
    {
        Ok(count) => count.parse().unwrap_or(0),
        Err(e) => {
            tracing::warn!(dest = %dest.display(), "cannot count unpushed commits: {e}");
            0
        }
    };
    Ok(RepoStatus {
        dirty: !porcelain.is_empty(),
        ahead,
    })
}

async fn clone_repo(repo: &RepoDef, dest: &Path) -> Result<(), String> {
    let dest_s = dest.to_string_lossy().into_owned();
    let mut args = vec!["clone", "--depth", "1"];
    if let Some(b) = &repo.branch {
        args.extend(["--branch", b]);
    }
    args.extend(["--", repo.url.as_str(), &dest_s]);
    git(&args).await
}

/// A sync either fast-forwarded the cache or refused to touch it.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    Synced,
    Skipped(String),
}

/// Shallow-fetch the repo's branch, updating the remote-tracking ref so
/// the `@{upstream}` ahead-probe stays accurate.
async fn fetch_repo(repo: &RepoDef, dest_s: &str) -> Result<(), String> {
    let refspec = repo
        .branch
        .as_ref()
        .map(|b| format!("+refs/heads/{b}:refs/remotes/origin/{b}"));
    let mut fetch = vec!["-C", dest_s, "fetch", "--depth", "1", "origin"];
    if let Some(spec) = &refspec {
        fetch.push(spec);
    }
    git(&fetch).await
}

/// Bring the cache up to date: shallow fetch + hard reset (robust with
/// depth-1 clones, follows force-pushed remotes). A missing cache is
/// cloned. A repo with uncommitted edits or unpushed commits is never
/// touched — the caller surfaces the skip.
pub(crate) async fn sync_repo(repo: &RepoDef, dest: &Path) -> Result<SyncOutcome, String> {
    if !dest.join(".git").exists() {
        if dest.exists() {
            // A half-made cache (interrupted clone) would wedge forever.
            std::fs::remove_dir_all(dest)
                .map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
        }
        return clone_repo(repo, dest).await.map(|()| SyncOutcome::Synced);
    }
    if !repo_status(dest).await?.clean() {
        return Ok(SyncOutcome::Skipped(
            "local changes — commit & push or discard first".into(),
        ));
    }
    let dest_s = dest.to_string_lossy().into_owned();
    fetch_repo(repo, &dest_s).await?;
    git(&["-C", &dest_s, "reset", "--hard", "FETCH_HEAD"]).await?;
    Ok(SyncOutcome::Synced)
}

/// The explicit escape hatch: throw away every local edit and unpushed
/// commit, back to the remote's tip.
pub(crate) async fn discard_repo(repo: &RepoDef, dest: &Path) -> Result<(), String> {
    if !dest.join(".git").exists() {
        return match sync_repo(repo, dest).await? {
            SyncOutcome::Synced => Ok(()),
            SyncOutcome::Skipped(msg) => Err(msg),
        };
    }
    let dest_s = dest.to_string_lossy().into_owned();
    fetch_repo(repo, &dest_s).await?;
    git(&["-C", &dest_s, "reset", "--hard", "FETCH_HEAD"]).await?;
    git(&["-C", &dest_s, "clean", "-fd"]).await
}

/// Commit every local edit (if any) and push to the origin branch.
/// Push rejection (the remote moved) comes back as `Err((409, ...))`;
/// resolving it means Discard or a real checkout — a conflicted rebase
/// would wedge a headless cache.
pub(crate) async fn commit_and_push(
    repo: &RepoDef,
    dest: &Path,
    message: &str,
    identity: &(String, String),
) -> Result<(), (StatusCode, String)> {
    let internal = |e: String| (StatusCode::BAD_GATEWAY, e);
    let status = repo_status(dest).await.map_err(internal)?;
    if status.clean() {
        return Err((StatusCode::BAD_REQUEST, "nothing to commit or push".into()));
    }
    let dest_s = dest.to_string_lossy().into_owned();
    if status.dirty {
        git(&["-C", &dest_s, "add", "-A"]).await.map_err(internal)?;
        let (user, email) = identity;
        let name_cfg = format!("user.name={user}");
        let email_cfg = format!("user.email={email}");
        git(&[
            "-C", &dest_s, "-c", &name_cfg, "-c", &email_cfg, "commit", "-m", message,
        ])
        .await
        .map_err(internal)?;
    }
    let push_ref = match &repo.branch {
        Some(b) => format!("HEAD:refs/heads/{b}"),
        None => "HEAD".into(),
    };
    git(&["-C", &dest_s, "push", "origin", &push_ref])
        .await
        .map_err(|e| {
            let lower = e.to_lowercase();
            if lower.contains("rejected") || lower.contains("fetch first") || lower.contains("non-fast-forward") {
                (
                    StatusCode::CONFLICT,
                    format!(
                        "push rejected — the remote has new commits. Discard local changes, or resolve in a real checkout. ({e})"
                    ),
                )
            } else {
                internal(e)
            }
        })
}

/// Clone any repo whose cache is absent, in the background: the server
/// must start (and keep serving local packages) with no network and no
/// git binary. Already-cloned caches are not touched — updating is the
/// explicit Sync action.
pub fn spawn_initial_clones(state: SharedState) {
    tokio::spawn(async move {
        let repos = state.repos.lock().unwrap().clone();
        for repo in repos {
            let dest = cache_dir(&state, &repo.name);
            if dest.join(".git").exists() {
                continue;
            }
            let _guard = state.repo_git_lock.lock().await;
            match sync_repo(&repo, &dest).await {
                Ok(_) => {
                    tracing::info!(repo = %repo.name, url = %repo.url, "cloned remote repository")
                }
                Err(e) => {
                    tracing::warn!(repo = %repo.name, url = %repo.url, "initial clone failed: {e}")
                }
            }
        }
    });
}

// ------------------------------------------------------------- handlers

/// Count the dirs holding `manifest` under a repo's content root — a
/// cheap badge for the list view.
fn count_dirs_with(dir: &Path, manifest: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            valid_name(&e.file_name().to_string_lossy()) && e.path().join(manifest).is_file()
        })
        .count()
}

pub(crate) async fn repo_json(state: &SharedState, repo: &RepoDef) -> Value {
    let pkgs = packages_root(state, repo);
    let books = runbooks_root(state, repo);
    let cache = cache_dir(state, &repo.name);
    let status = if cache.join(".git").exists() {
        repo_status(&cache).await.ok()
    } else {
        None
    };
    json!({
        "name": repo.name,
        "url": repo.url,
        "subdir": repo.subdir,
        "runbooks_subdir": repo.runbooks_subdir,
        "branch": repo.branch,
        "sync_cron": repo.sync_cron,
        "webhook_secret": repo.webhook_secret,
        "cloned": pkgs.is_some() || cache.join(".git").exists(),
        "packages": pkgs.as_deref().map(|d| count_dirs_with(d, "package.wcl")),
        "runbooks": books.as_deref().map(|d| count_dirs_with(d, "playbook.wcl")),
        "dirty": status.as_ref().is_some_and(|s| s.dirty),
        "ahead": status.as_ref().map_or(0, |s| s.ahead),
    })
}

/// Persist the repo list, restoring `previous` in memory on failure.
fn persist(
    state: &SharedState,
    repos: &mut Vec<RepoDef>,
    previous: Vec<RepoDef>,
) -> Option<String> {
    match save(&state.repos_path, repos) {
        Ok(()) => None,
        Err(e) => {
            *repos = previous;
            Some(e)
        }
    }
}

/// GET /api/repos
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    let repos = state.repos.lock().unwrap().clone();
    let mut rows: Vec<Value> = Vec::with_capacity(repos.len());
    for repo in &repos {
        rows.push(repo_json(&state, repo).await);
    }
    ok(rows)
}

/// GET /api/repos/{name} — the editors poll this for the dirty badge.
pub async fn get_one(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let def = {
        let repos = state.repos.lock().unwrap();
        repos.iter().find(|r| r.name == name).cloned()
    };
    match def {
        Some(def) => ok(repo_json(&state, &def).await),
        None => err(StatusCode::NOT_FOUND, "no such repository"),
    }
}

/// POST /api/repos — body `RepoDef`. The entry persists even when the
/// initial clone fails (Sync retries later); the git error rides along.
pub async fn create(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
    axum::Json(def): axum::Json<RepoDef>,
) -> Response {
    if let Err(e) = validate(&def) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    {
        let mut repos = state.repos.lock().unwrap();
        if repos.iter().any(|r| r.name == def.name) {
            return err(
                StatusCode::CONFLICT,
                "a repository with that name already exists",
            );
        }
        let previous = repos.clone();
        repos.push(def.clone());
        if let Some(e) = persist(&state, &mut repos, previous) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    let clone_error = {
        let _guard = state.repo_git_lock.lock().await;
        sync_repo(&def, &cache_dir(&state, &def.name)).await.err()
    };
    let mut body = repo_json(&state, &def).await;
    if let Some(e) = clone_error {
        body["error"] = json!(e);
    }
    ok(body)
}

/// PUT /api/repos/{name} — body `RepoDef` (the name in the path wins).
/// Edits the definition in place; the cache is untouched, so changing
/// the url/branch takes effect on the next sync.
pub async fn update(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(mut def): axum::Json<RepoDef>,
) -> Response {
    def.name = name;
    if let Err(e) = validate(&def) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    {
        let mut repos = state.repos.lock().unwrap();
        let Some(idx) = repos.iter().position(|r| r.name == def.name) else {
            return err(StatusCode::NOT_FOUND, "no such repository");
        };
        let previous = repos.clone();
        repos[idx] = def.clone();
        if let Some(e) = persist(&state, &mut repos, previous) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    ok(repo_json(&state, &def).await)
}

/// DELETE /api/repos/{name}
pub async fn remove(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    {
        let mut repos = state.repos.lock().unwrap();
        let Some(idx) = repos.iter().position(|r| r.name == name) else {
            return err(StatusCode::NOT_FOUND, "no such repository");
        };
        let previous = repos.clone();
        repos.remove(idx);
        if let Some(e) = persist(&state, &mut repos, previous) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    // The name passed valid_name when the entry was created, so the
    // cache path cannot escape .repo-cache.
    let _guard = state.repo_git_lock.lock().await;
    let _ = std::fs::remove_dir_all(cache_dir(&state, &name));
    ok(json!({ "deleted": name }))
}

/// POST /api/repos/{name}/sync
pub async fn sync_one(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let def = {
        let repos = state.repos.lock().unwrap();
        repos.iter().find(|r| r.name == name).cloned()
    };
    let Some(def) = def else {
        return err(StatusCode::NOT_FOUND, "no such repository");
    };
    let result = {
        let _guard = state.repo_git_lock.lock().await;
        sync_repo(&def, &cache_dir(&state, &def.name)).await
    };
    match result {
        Ok(SyncOutcome::Synced) => ok(repo_json(&state, &def).await),
        Ok(SyncOutcome::Skipped(msg)) => err(StatusCode::CONFLICT, msg),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// POST /api/repos/sync — sync everything, reporting per-repo results.
pub async fn sync_all(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
) -> Response {
    let repos = state.repos.lock().unwrap().clone();
    let mut results = Vec::new();
    for def in &repos {
        let result = {
            let _guard = state.repo_git_lock.lock().await;
            sync_repo(def, &cache_dir(&state, &def.name)).await
        };
        results.push(match result {
            Ok(SyncOutcome::Synced) => json!({ "name": def.name, "ok": true }),
            Ok(SyncOutcome::Skipped(msg)) => {
                json!({ "name": def.name, "ok": false, "skipped": msg })
            }
            Err(e) => json!({ "name": def.name, "ok": false, "error": e }),
        });
    }
    ok(results)
}

#[derive(Deserialize)]
pub struct CommitRequest {
    pub message: String,
}

/// POST /api/repos/{name}/commit — commit every local edit (if any) and
/// push to origin. 400 when there is nothing to push, 409 when the
/// remote moved underneath us.
pub async fn commit(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(req): axum::Json<CommitRequest>,
) -> Response {
    if req.message.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "commit message must not be empty");
    }
    let def = {
        let repos = state.repos.lock().unwrap();
        repos.iter().find(|r| r.name == name).cloned()
    };
    let Some(def) = def else {
        return err(StatusCode::NOT_FOUND, "no such repository");
    };
    let dest = cache_dir(&state, &def.name);
    if !dest.join(".git").exists() {
        return err(StatusCode::CONFLICT, "the repository is not cloned yet");
    }
    let result = {
        let _guard = state.repo_git_lock.lock().await;
        commit_and_push(&def, &dest, req.message.trim(), &state.git_identity).await
    };
    match result {
        Ok(()) => ok(repo_json(&state, &def).await),
        Err((status, e)) => err(status, e),
    }
}

/// POST /api/repos/{name}/discard — throw away every local edit and
/// unpushed commit, back to the remote's tip.
pub async fn discard(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let def = {
        let repos = state.repos.lock().unwrap();
        repos.iter().find(|r| r.name == name).cloned()
    };
    let Some(def) = def else {
        return err(StatusCode::NOT_FOUND, "no such repository");
    };
    let result = {
        let _guard = state.repo_git_lock.lock().await;
        discard_repo(&def, &cache_dir(&state, &def.name)).await
    };
    match result {
        Ok(()) => ok(repo_json(&state, &def).await),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<RepoDef> {
        vec![
            stdlib_default(),
            RepoDef {
                name: "internal".into(),
                url: "git@example.com:ops/weave-packages.git".into(),
                subdir: None,
                runbooks_subdir: Some("books".into()),
                branch: Some("stable".into()),
                sync_cron: Some("0 */5 * * * *".into()),
                webhook_secret: Some("hunter2hunter2".into()),
            },
        ]
    }

    #[test]
    fn save_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.wcl");
        let repos = sample();
        save(&path, &repos).unwrap();
        assert_eq!(load(&path).unwrap(), Some(repos));
    }

    #[test]
    fn missing_file_is_none_but_empty_file_is_respected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.wcl");
        assert_eq!(load(&path).unwrap(), None);
        save(&path, &[]).unwrap();
        assert_eq!(load(&path).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn duplicate_names_are_rejected_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.wcl");
        let mut repos = sample();
        repos.push(repos[0].clone());
        save(&path, &repos).unwrap();
        assert!(load(&path).unwrap_err().contains("duplicate repository"));
    }

    #[test]
    fn stdlib_default_points_at_the_public_pkgs_repo() {
        let def = stdlib_default();
        assert_eq!(def.name, "stdlib");
        assert!(def.url.contains("Configweave/config-weave-pkgs"));
        assert_eq!(def.subdir.as_deref(), Some("pkgs"));
        assert!(validate(&def).is_ok());
    }

    #[test]
    fn validation_rejects_reserved_and_traversal_prone_defs() {
        let mut def = stdlib_default();
        def.name = LOCAL_SOURCE.into();
        assert!(validate(&def).unwrap_err().contains("reserved"));
        def = stdlib_default();
        def.name = "../evil".into();
        assert!(validate(&def).is_err());
        def = stdlib_default();
        def.subdir = Some("../outside".into());
        assert!(validate(&def).unwrap_err().contains("subdir"));
        def = stdlib_default();
        def.url = String::new();
        assert!(validate(&def).unwrap_err().contains("url"));
    }

    #[test]
    fn validation_covers_the_new_fields() {
        let mut def = stdlib_default();
        def.runbooks_subdir = Some("../outside".into());
        assert!(validate(&def).unwrap_err().contains("runbooks_subdir"));
        def = stdlib_default();
        def.runbooks_subdir = Some(".".into());
        assert!(validate(&def).is_ok());
        def = stdlib_default();
        def.sync_cron = Some("not a cron".into());
        assert!(validate(&def).unwrap_err().contains("sync_cron"));
        def = stdlib_default();
        def.sync_cron = Some("0 */15 * * * *".into());
        assert!(validate(&def).is_ok());
        def = stdlib_default();
        def.webhook_secret = Some("short".into());
        assert!(validate(&def).unwrap_err().contains("webhook_secret"));
    }

    #[test]
    fn old_style_files_without_the_new_fields_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.wcl");
        std::fs::write(
            &path,
            "repo \"stdlib\" {\n  url = \"https://example.com/pkgs.git\"\n  subdir = \"pkgs\"\n}\n",
        )
        .unwrap();
        let repos = load(&path).unwrap().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].runbooks_subdir, None);
        assert_eq!(repos[0].sync_cron, None);
        assert_eq!(repos[0].webhook_secret, None);
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.wcl");
        save(&path, &sample()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // ------------------------------------------- git flows (real git)

    /// A bare "remote" seeded through a work clone, plus a RepoDef
    /// pointing at it. Returns (tempdir, def, seed clone path).
    fn fake_remote() -> (tempfile::TempDir, RepoDef, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("remote.git");
        let seed = tmp.path().join("seed");
        let sh = |args: &[&str], cwd: &Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        sh(
            &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
            tmp.path(),
        );
        sh(
            &["clone", bare.to_str().unwrap(), seed.to_str().unwrap()],
            tmp.path(),
        );
        std::fs::create_dir_all(seed.join("books/demo")).unwrap();
        std::fs::write(
            seed.join("books/demo/playbook.wcl"),
            "playbook \"demo\" {}\n",
        )
        .unwrap();
        sh(&["add", "-A"], &seed);
        sh(
            &[
                "-c",
                "user.name=seed",
                "-c",
                "user.email=seed@test",
                "commit",
                "-m",
                "seed",
            ],
            &seed,
        );
        sh(&["push", "origin", "main"], &seed);
        let def = RepoDef {
            name: "fake".into(),
            url: format!("file://{}", bare.display()),
            subdir: None,
            runbooks_subdir: Some("books".into()),
            branch: Some("main".into()),
            sync_cron: None,
            webhook_secret: None,
        };
        (tmp, def, seed)
    }

    const TEST_IDENTITY: (&str, &str) = ("weave-test", "weave-test@localhost");

    fn identity() -> (String, String) {
        (TEST_IDENTITY.0.into(), TEST_IDENTITY.1.into())
    }

    #[tokio::test]
    async fn sync_clones_then_fast_forwards_a_clean_cache() {
        let (tmp, def, seed) = fake_remote();
        let dest = tmp.path().join("cache");
        assert_eq!(sync_repo(&def, &dest).await.unwrap(), SyncOutcome::Synced);
        assert!(dest.join("books/demo/playbook.wcl").is_file());
        assert!(repo_status(&dest).await.unwrap().clean());

        // Remote moves; a clean cache follows it.
        std::fs::write(
            seed.join("books/demo/playbook.wcl"),
            "playbook \"demo2\" {}\n",
        )
        .unwrap();
        let sh = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&seed)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        sh(&["add", "-A"]);
        sh(&[
            "-c",
            "user.name=seed",
            "-c",
            "user.email=seed@test",
            "commit",
            "-m",
            "update",
        ]);
        sh(&["push", "origin", "main"]);
        assert_eq!(sync_repo(&def, &dest).await.unwrap(), SyncOutcome::Synced);
        let content = std::fs::read_to_string(dest.join("books/demo/playbook.wcl")).unwrap();
        assert!(content.contains("demo2"));
    }

    #[tokio::test]
    async fn sync_skips_dirty_and_ahead_caches_and_discard_recovers() {
        let (tmp, def, _seed) = fake_remote();
        let dest = tmp.path().join("cache");
        sync_repo(&def, &dest).await.unwrap();

        // Uncommitted edit → dirty → skipped.
        std::fs::write(dest.join("books/demo/playbook.wcl"), "edited\n").unwrap();
        assert!(matches!(
            sync_repo(&def, &dest).await.unwrap(),
            SyncOutcome::Skipped(_)
        ));

        // Committed but unpushed → ahead → still skipped.
        commit_locally(&dest);
        let status = repo_status(&dest).await.unwrap();
        assert!(!status.dirty);
        assert_eq!(status.ahead, 1);
        assert!(matches!(
            sync_repo(&def, &dest).await.unwrap(),
            SyncOutcome::Skipped(_)
        ));

        // Discard resets to the remote tip and syncs work again.
        discard_repo(&def, &dest).await.unwrap();
        assert!(repo_status(&dest).await.unwrap().clean());
        let content = std::fs::read_to_string(dest.join("books/demo/playbook.wcl")).unwrap();
        assert!(content.contains("playbook"));
        assert_eq!(sync_repo(&def, &dest).await.unwrap(), SyncOutcome::Synced);
    }

    fn commit_locally(dest: &Path) {
        for args in [
            vec!["add", "-A"],
            vec![
                "-c",
                "user.name=local",
                "-c",
                "user.email=local@test",
                "commit",
                "-m",
                "local",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(&args)
                    .current_dir(dest)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }

    #[tokio::test]
    async fn commit_and_push_lands_on_the_remote() {
        let (tmp, def, seed) = fake_remote();
        let dest = tmp.path().join("cache");
        sync_repo(&def, &dest).await.unwrap();

        std::fs::write(
            dest.join("books/demo/playbook.wcl"),
            "playbook \"pushed\" {}\n",
        )
        .unwrap();
        commit_and_push(&def, &dest, "gui edit", &identity())
            .await
            .unwrap();
        assert!(repo_status(&dest).await.unwrap().clean());

        // Nothing left to push.
        let e = commit_and_push(&def, &dest, "again", &identity())
            .await
            .unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);

        // The commit (with our identity) is visible from a fresh pull.
        let out = std::process::Command::new("git")
            .args(["pull", "origin", "main"])
            .current_dir(&seed)
            .output()
            .unwrap();
        assert!(out.status.success());
        let log = std::process::Command::new("git")
            .args(["log", "-1", "--format=%s %an %ae"])
            .current_dir(&seed)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(log.contains("gui edit"), "unexpected log: {log}");
        assert!(log.contains(TEST_IDENTITY.0));
        assert!(log.contains(TEST_IDENTITY.1));
    }

    #[tokio::test]
    async fn rejected_push_is_a_conflict() {
        let (tmp, def, seed) = fake_remote();
        let dest = tmp.path().join("cache");
        sync_repo(&def, &dest).await.unwrap();

        // The remote moves underneath our local edit.
        std::fs::write(seed.join("other.txt"), "remote wins\n").unwrap();
        let sh = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&seed)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        sh(&["add", "-A"]);
        sh(&[
            "-c",
            "user.name=seed",
            "-c",
            "user.email=seed@test",
            "commit",
            "-m",
            "remote",
        ]);
        sh(&["push", "origin", "main"]);

        std::fs::write(dest.join("books/demo/playbook.wcl"), "local edit\n").unwrap();
        let e = commit_and_push(&def, &dest, "will be rejected", &identity())
            .await
            .unwrap_err();
        assert_eq!(e.0, StatusCode::CONFLICT, "got: {}", e.1);
    }
}
