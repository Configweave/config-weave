//! The package repository: a dedicated `--packages-dir` of package dirs
//! (each containing a `package.wcl`) served in the UI. Repo packages can
//! be doc-viewed (via the extended `list --json` inventory), tested, and
//! copied into a runbook's `pkgs/`.
//!
//! The CLI only understands playbook dirs, so the repo is wrapped in a
//! synthesized tempdir playbook (`playbook "package-repo" { … }` +
//! `pkgs/<name>` symlinks). Nothing bind-mounts that wrapper: the
//! testlab's synthesize step copies packages (dereferencing symlinks)
//! into its own per-test playbook before anything reaches an instance.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runbooks::{cli_json, runbook_dir, valid_name};
use crate::runs::{RunContext, RunRequest};
use crate::state::SharedState;

/// The wrapper playbook, rebuilt when the repo's fingerprint changes.
struct Wrapper {
    dir: tempfile::TempDir,
    fingerprint: Vec<(String, SystemTime)>,
}

#[derive(Default)]
pub struct WrapperCache(Mutex<Option<Wrapper>>);

/// Scan the repo: package dirs (containing package.wcl) + their mtimes.
fn scan_repo(packages_dir: &Path) -> Result<Vec<(String, SystemTime)>, String> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(packages_dir)
        .map_err(|e| format!("cannot read packages dir: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let manifest = entry.path().join("package.wcl");
        if entry.path().is_dir() && valid_name(&name) && manifest.is_file() {
            let mtime = std::fs::metadata(&manifest)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            found.push((name, mtime));
        }
    }
    found.sort();
    Ok(found)
}

/// The wrapper playbook dir, rebuilding when the repo changed. Handing
/// out a PathBuf (not the TempDir) is safe: a rebuild mid-run only swaps
/// symlinks whose targets — the real package dirs — are untouched.
fn ensure_wrapper(state: &SharedState) -> Result<PathBuf, String> {
    let packages_dir = state
        .packages_dir
        .as_ref()
        .ok_or("package repository not configured")?;
    let fingerprint = scan_repo(packages_dir)?;

    let mut cache = state.pkg_wrapper.0.lock().unwrap();
    if let Some(w) = cache.as_ref()
        && w.fingerprint == fingerprint
    {
        return Ok(w.dir.path().to_path_buf());
    }

    let dir = tempfile::TempDir::with_prefix("weave-pkg-repo-")
        .map_err(|e| format!("cannot create wrapper: {e}"))?;
    std::fs::write(
        dir.path().join("playbook.wcl"),
        "playbook \"package-repo\" {\n  description = \"weave-server package repository\"\n}\n",
    )
    .map_err(|e| format!("cannot create wrapper: {e}"))?;
    let pkgs = dir.path().join("pkgs");
    std::fs::create_dir(&pkgs).map_err(|e| format!("cannot create wrapper: {e}"))?;
    #[cfg(unix)]
    for (name, _) in &fingerprint {
        std::os::unix::fs::symlink(packages_dir.join(name), pkgs.join(name))
            .map_err(|e| format!("cannot link package {name}: {e}"))?;
    }

    let path = dir.path().to_path_buf();
    *cache = Some(Wrapper { dir, fingerprint });
    Ok(path)
}

/// The repo inventory via `list --json`; a broken package yields a
/// visible error instead of a dead view.
async fn repo_inventory(state: &SharedState) -> Result<Value, Response> {
    let wrapper = match ensure_wrapper(state) {
        Ok(w) => w,
        Err(e) if e == "package repository not configured" => {
            return Err(err(StatusCode::NOT_FOUND, e));
        }
        Err(e) => return Err(err(StatusCode::INTERNAL_SERVER_ERROR, e)),
    };
    match cli_json(state, &["list", &wrapper.to_string_lossy(), "--json"]).await {
        Ok(v) => Ok(v),
        Err(e) => Err(ok(json!({ "packages": [], "error": e }))),
    }
}

/// GET /api/packages
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    match repo_inventory(&state).await {
        Ok(inv) => ok(json!({ "packages": inv["packages"] })),
        Err(resp) => resp,
    }
}

/// GET /api/packages/{name}
pub async fn detail(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match repo_inventory(&state).await {
        Ok(inv) => {
            let found = inv["packages"]
                .as_array()
                .and_then(|pkgs| pkgs.iter().find(|p| p["name"] == name.as_str()));
            match found {
                Some(pkg) => ok(pkg.clone()),
                None => err(StatusCode::NOT_FOUND, "no such package"),
            }
        }
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct AddRequest {
    pub runbook: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// POST /api/packages/{name}/add-to-runbook — copy (never symlink: the
/// runbook editor refuses symlink escapes) the real repo dir into
/// `<runbook>/pkgs/<name>`.
pub async fn add_to_runbook(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(req): axum::Json<AddRequest>,
) -> Response {
    let Some(packages_dir) = state.packages_dir.clone() else {
        return err(StatusCode::NOT_FOUND, "package repository not configured");
    };
    let src = packages_dir.join(&name);
    if !valid_name(&name) || !src.join("package.wcl").is_file() {
        return err(StatusCode::NOT_FOUND, "no such package");
    }
    let Some(rb_dir) = runbook_dir(&state, &req.runbook) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    let dest = rb_dir.join("pkgs").join(&name);
    if dest.exists() {
        if !req.overwrite {
            return err(StatusCode::CONFLICT, "package already in the runbook");
        }
        if let Err(e) = std::fs::remove_dir_all(&dest) {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot replace: {e}"),
            );
        }
    }
    if let Err(e) = crate::transport::copy_dir_filtered(&src, &dest) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("cannot copy: {e}"));
    }
    ok(json!({ "runbook": req.runbook, "package": name, "path": format!("pkgs/{name}") }))
}

#[derive(Deserialize)]
pub struct PkgTestRequest {
    pub test: Option<String>,
    pub backend: Option<String>,
    pub image: Option<String>,
    #[serde(default)]
    pub keep: bool,
}

/// POST /api/packages/{name}/test — run the package's tests (or one
/// test) inside the wrapper playbook. `pkgs:` in the run label cannot
/// collide with a real runbook (`:` fails valid_name).
pub async fn run_tests(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(req): axum::Json<PkgTestRequest>,
) -> Response {
    let wrapper = match ensure_wrapper(&state) {
        Ok(w) => w,
        Err(e) if e == "package repository not configured" => {
            return err(StatusCode::NOT_FOUND, e);
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if !valid_name(&name) || !wrapper.join("pkgs").join(&name).exists() {
        return err(StatusCode::NOT_FOUND, "no such package");
    }
    let request = RunRequest {
        runbook: format!("pkgs:{name}"),
        filter: Some(match &req.test {
            Some(t) => format!("{name}:{t}"),
            None => name.clone(),
        }),
        backend: req.backend,
        image: req.image,
        keep: req.keep,
    };
    let ctx = RunContext {
        config_weave: state.config_weave.clone(),
        runbook_dir: wrapper,
        test_binary: state.test_binary.clone(),
        test_binary_windows: state.test_binary_windows.clone(),
        events: state.events.clone(),
    };
    match state.runs.start(request, ctx) {
        Ok(run) => ok(json!({ "id": run.id })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_only_valid_package_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("linux_files")).unwrap();
        std::fs::write(tmp.path().join("linux_files/package.wcl"), "x").unwrap();
        std::fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
        std::fs::write(tmp.path().join(".hidden/package.wcl"), "x").unwrap();
        std::fs::create_dir_all(tmp.path().join("no_manifest")).unwrap();
        std::fs::write(tmp.path().join("stray.txt"), "x").unwrap();

        let found = scan_repo(tmp.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "linux_files");
    }
}
