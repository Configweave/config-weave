//! Remote package repositories: `{root}/repos.wcl` — git repos cloned
//! into `{root}/.repo-cache/<name>` whose packages merge into the
//! package repository (read-only, tagged by source). Loaded through the
//! embedded vocabulary and regenerated from structs on every GUI edit,
//! same as the services inventory. A missing file is seeded with the
//! stdlib repo; a present file (even empty) is respected, so removing
//! the stdlib sticks.

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
    #[serde(default)]
    pub branch: Option<String>,
}

/// The default entry seeded into a missing repos.wcl.
pub fn stdlib_default() -> RepoDef {
    RepoDef {
        name: "stdlib".into(),
        url: "https://github.com/Configweave/config-weave-pkgs.git".into(),
        subdir: Some("pkgs".into()),
        branch: None,
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
        branch: str_field("branch")?,
        name,
    })
}

/// Structural checks shared by load and the create handler. The name
/// becomes a cache directory and a source tag; the subdir joins under
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
        && (sub.is_empty()
            || sub.starts_with('/')
            || sub.split('/').any(|c| c == ".." || c.is_empty()))
    {
        return Err("subdir must be a relative path without '..'".into());
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
        if let Some(branch) = &repo.branch {
            fields.push(("branch".into(), edit::string_literal_expr(branch)));
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
        " Config Weave package repositories — managed by weave-server.",
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
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

// ------------------------------------------------------------------- git

/// Run git with stderr captured; auth prompts are disabled so a private
/// URL fails fast instead of hanging the handler.
async fn git(args: &[&str]) -> Result<(), String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "git {} failed: {}",
        args.first().copied().unwrap_or(""),
        stderr.trim()
    ))
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

/// Bring the cache up to date: shallow fetch + hard reset (robust with
/// depth-1 clones, and discards any stray edits in the read-only cache).
/// A missing cache is cloned.
async fn sync_repo(repo: &RepoDef, dest: &Path) -> Result<(), String> {
    if !dest.join(".git").exists() {
        if dest.exists() {
            // A half-made cache (interrupted clone) would wedge forever.
            std::fs::remove_dir_all(dest)
                .map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
        }
        return clone_repo(repo, dest).await;
    }
    let dest_s = dest.to_string_lossy().into_owned();
    let mut fetch = vec!["-C", &dest_s, "fetch", "--depth", "1", "origin"];
    if let Some(b) = &repo.branch {
        fetch.push(b);
    }
    git(&fetch).await?;
    git(&["-C", &dest_s, "reset", "--hard", "FETCH_HEAD"]).await
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
                Ok(()) => {
                    tracing::info!(repo = %repo.name, url = %repo.url, "cloned package repository")
                }
                Err(e) => {
                    tracing::warn!(repo = %repo.name, url = %repo.url, "initial clone failed: {e}")
                }
            }
        }
    });
}

// ------------------------------------------------------------- handlers

/// Count the package dirs (containing package.wcl) under a repo's
/// packages root — a cheap badge for the list view.
fn count_packages(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            valid_name(&e.file_name().to_string_lossy()) && e.path().join("package.wcl").is_file()
        })
        .count()
}

fn repo_json(state: &SharedState, repo: &RepoDef) -> Value {
    let root = packages_root(state, repo);
    json!({
        "name": repo.name,
        "url": repo.url,
        "subdir": repo.subdir,
        "branch": repo.branch,
        "cloned": root.is_some(),
        "packages": root.as_deref().map(count_packages),
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
    let rows: Vec<Value> = repos.iter().map(|r| repo_json(&state, r)).collect();
    ok(rows)
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
    let mut body = repo_json(&state, &def);
    if let Some(e) = clone_error {
        body["error"] = json!(e);
    }
    ok(body)
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
        Ok(()) => ok(repo_json(&state, &def)),
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
            Ok(()) => json!({ "name": def.name, "ok": true }),
            Err(e) => json!({ "name": def.name, "ok": false, "error": e }),
        });
    }
    ok(results)
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
                branch: Some("stable".into()),
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
}
