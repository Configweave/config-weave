//! The package repository: a dedicated `--packages-dir` of package dirs
//! (each containing a `package.wcl`), merged with the cached clones of
//! the remote repositories (`repos.wcl`), served in the UI. Repo
//! packages can be doc-viewed (via the extended `list --json`
//! inventory), tested, and copied into a runbook's `pkgs/`. Each package
//! carries a `source` tag — `"local"` or a remote repo's name; on a name
//! collision the local package shadows the remote one, and remote
//! packages are read-only (sync to update, or copy into a playbook).
//!
//! The CLI only understands playbook dirs, so the merged repo is wrapped
//! in a synthesized tempdir playbook (`playbook "package-repo" { … }` +
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

use axum::extract::Query;

use crate::runbooks::{
    DocParse, DocRender, DocSave, FileQuery, FileWrite, cli_json, doc_parse_at, doc_render_at,
    doc_save_at, read_file_at, resolve_in, runbook_dir, tree_node, valid_name, write_file_at,
};
use crate::runs::{RunContext, RunRequest};
use crate::state::SharedState;

/// The wrapper playbook, rebuilt when the merged repo's fingerprint
/// changes (a package appearing, disappearing, switching source dir,
/// or its manifest being touched).
struct Wrapper {
    dir: tempfile::TempDir,
    fingerprint: Vec<(String, PathBuf, SystemTime)>,
}

#[derive(Default)]
pub struct WrapperCache(Mutex<Option<Wrapper>>);

/// A scanned package dir: name, real path, manifest mtime.
type PkgDirEntry = (String, PathBuf, SystemTime);

/// Scan one source dir: package dirs (containing package.wcl) with
/// their real paths and manifest mtimes.
fn scan_dir(dir: &Path) -> Result<Vec<PkgDirEntry>, String> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read packages dir: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let manifest = entry.path().join("package.wcl");
        if entry.path().is_dir() && valid_name(&name) && manifest.is_file() {
            let mtime = std::fs::metadata(&manifest)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            found.push((name, entry.path(), mtime));
        }
    }
    found.sort();
    Ok(found)
}

/// One package in the merged repo.
struct SourceEntry {
    name: String,
    dir: PathBuf,
    mtime: SystemTime,
    source: String,
}

struct ScanResult {
    entries: Vec<SourceEntry>,
    /// Name collisions: `(name, winning source, shadowed source)`.
    shadowed: Vec<(String, String, String)>,
}

/// First occurrence of a name wins: earlier sources shadow later ones.
/// Split from state so the policy is unit-testable.
fn merge_sources(sources: Vec<(String, Vec<PkgDirEntry>)>) -> ScanResult {
    let mut result = ScanResult {
        entries: Vec::new(),
        shadowed: Vec::new(),
    };
    let mut winner: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (source, found) in sources {
        for (name, dir, mtime) in found {
            if let Some(by) = winner.get(&name) {
                result.shadowed.push((name, by.clone(), source.clone()));
                continue;
            }
            winner.insert(name.clone(), source.clone());
            result.entries.push(SourceEntry {
                name,
                dir,
                mtime,
                source: source.clone(),
            });
        }
    }
    result.entries.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Merge all sources: the local --packages-dir first, then each remote
/// repo's cache in repos.wcl order (so local packages shadow remote
/// ones). A repo whose cache is missing (clone pending/failed) or
/// unreadable is skipped — the view must not die because one remote is
/// broken.
fn scan_sources(state: &SharedState) -> Result<ScanResult, String> {
    let packages_dir = state
        .packages_dir
        .as_ref()
        .ok_or("package repository not configured")?;

    let mut sources = vec![(
        crate::repos::LOCAL_SOURCE.to_string(),
        scan_dir(packages_dir)?,
    )];
    let repos = state.repos.lock().unwrap().clone();
    for repo in &repos {
        let Some(root) = crate::repos::packages_root(state, repo) else {
            continue;
        };
        match scan_dir(&root) {
            Ok(found) => sources.push((repo.name.clone(), found)),
            Err(e) => tracing::warn!(repo = %repo.name, "skipping repository: {e}"),
        }
    }
    Ok(merge_sources(sources))
}

/// A package dir from any source, with its source tag; local wins over
/// remote copies of the same name.
fn resolve_package(state: &SharedState, name: &str) -> Option<(PathBuf, String)> {
    if !valid_name(name) {
        return None;
    }
    if let Some(dir) = state
        .packages_dir
        .as_deref()
        .and_then(|d| package_dir_in(d, name))
    {
        return Some((dir, crate::repos::LOCAL_SOURCE.into()));
    }
    let repos = state.repos.lock().unwrap().clone();
    for repo in &repos {
        if let Some(dir) =
            crate::repos::packages_root(state, repo).and_then(|root| package_dir_in(&root, name))
        {
            return Some((dir, repo.name.clone()));
        }
    }
    None
}

/// The wrapper playbook dir, rebuilding when the merged repo changed.
/// Handing out a PathBuf (not the TempDir) is safe: a rebuild mid-run
/// only swaps symlinks whose targets — the real package dirs — are
/// untouched.
fn ensure_wrapper(state: &SharedState) -> Result<PathBuf, String> {
    let scan = scan_sources(state)?;
    let fingerprint: Vec<(String, PathBuf, SystemTime)> = scan
        .entries
        .iter()
        .map(|e| (e.name.clone(), e.dir.clone(), e.mtime))
        .collect();

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
    for entry in &scan.entries {
        std::os::unix::fs::symlink(&entry.dir, pkgs.join(&entry.name))
            .map_err(|e| format!("cannot link package {}: {e}", entry.name))?;
    }

    let path = dir.path().to_path_buf();
    *cache = Some(Wrapper { dir, fingerprint });
    Ok(path)
}

/// The repo inventory via `list --json`, with each package's `source`
/// joined back on (the CLI only sees the wrapper's symlinks) and the
/// shadowed-name collisions alongside; a broken package yields a
/// visible error instead of a dead view.
async fn repo_inventory(state: &SharedState) -> Result<Value, Response> {
    let wrapper = match ensure_wrapper(state) {
        Ok(w) => w,
        Err(e) if e == "package repository not configured" => {
            return Err(err(StatusCode::NOT_FOUND, e));
        }
        Err(e) => return Err(err(StatusCode::INTERNAL_SERVER_ERROR, e)),
    };
    let mut inv = match cli_json(state, &["list", &wrapper.to_string_lossy(), "--json"]).await {
        Ok(v) => v,
        Err(e) => return Err(ok(json!({ "packages": [], "error": e }))),
    };
    if let Ok(scan) = scan_sources(state) {
        let sources: std::collections::HashMap<&str, &str> = scan
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.source.as_str()))
            .collect();
        if let Some(pkgs) = inv["packages"].as_array_mut() {
            for pkg in pkgs {
                if let Some(source) = pkg["name"].as_str().and_then(|n| sources.get(n)) {
                    pkg["source"] = json!(source);
                }
            }
        }
        inv["shadowed"] = scan
            .shadowed
            .iter()
            .map(|(name, by, source)| json!({ "name": name, "by": by, "source": source }))
            .collect();
    }
    Ok(inv)
}

/// GET /api/packages
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    match repo_inventory(&state).await {
        Ok(inv) => ok(json!({ "packages": inv["packages"], "shadowed": inv["shadowed"] })),
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

// ------------------------------------------------------ repo editing

/// A repo package dir, guarded by name validity + manifest presence.
/// Split from state so the check is unit-testable.
fn package_dir_in(packages_dir: &Path, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    let dir = packages_dir.join(name);
    (dir.is_dir() && dir.join("package.wcl").is_file()).then(|| dir.canonicalize().unwrap_or(dir))
}

/// The read-only guard for write endpoints: resolve the package and
/// refuse anything not from the local repository.
fn writable_package_dir(state: &SharedState, name: &str) -> Result<PathBuf, (StatusCode, String)> {
    let Some((dir, source)) = resolve_package(state, name) else {
        return Err((StatusCode::NOT_FOUND, "no such package".into()));
    };
    if source != crate::repos::LOCAL_SOURCE {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "package '{name}' is read-only: it comes from repository '{source}' — \
                 sync the repository to update it, or copy it into a playbook to edit"
            ),
        ));
    }
    Ok(dir)
}

// ------------------------------------------------------- API docs

/// Read and extract a package dir's package.wcl in-process
/// (parse_for_edit into DocJson, never the CLI). Split from the
/// handlers so it is unit-testable. Extraction fails closed, so a
/// manifest the editors cannot represent is a 422 with the diags, not
/// a blank doc.
fn package_doc_at(dir: &Path) -> Result<weave_docjson::docjson::PackageDoc, (StatusCode, String)> {
    let manifest = dir.join("package.wcl");
    let source = std::fs::read_to_string(&manifest).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot read package.wcl: {e}"),
        )
    })?;
    let ast = wcl_lang::parse_for_edit(&source, "package.wcl").map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("package.wcl does not parse: {e}"),
        )
    })?;
    weave_docjson::inspect_ast::extract_package(&ast)
        .map_err(|diags| (StatusCode::UNPROCESSABLE_ENTITY, diags.join("; ")))
}

/// GET /api/packages/{name}/docs — a repo package's API docs.
pub async fn docs(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some((dir, _)) = resolve_package(&state, &name) else {
        return err(StatusCode::NOT_FOUND, "no such package");
    };
    match package_doc_at(&dir) {
        Ok(doc) => ok(json!({ "doc": doc })),
        Err((code, msg)) => err(code, msg),
    }
}

/// GET /api/playbooks/{rb}/packages/{name}/docs — an installed copy's
/// API docs.
pub async fn runbook_docs(
    Extension(state): Extension<SharedState>,
    UrlPath((rb, name)): UrlPath<(String, String)>,
    _claims: RequireClaims,
) -> Response {
    let Some(rb_dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    let rb_dir = rb_dir.canonicalize().unwrap_or(rb_dir);
    let dir = match resolve_in(&rb_dir, &format!("pkgs/{name}"), true) {
        Ok(d) if d.join("package.wcl").is_file() => d,
        _ => {
            return err(
                StatusCode::NOT_FOUND,
                "package not installed in this runbook",
            );
        }
    };
    match package_doc_at(&dir) {
        Ok(doc) => ok(json!({ "doc": doc })),
        Err((code, msg)) => err(code, msg),
    }
}

/// GET /api/packages/{name}/tree
pub async fn tree(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some((dir, _)) = resolve_package(&state, &name) else {
        return err(StatusCode::NOT_FOUND, "no such package");
    };
    match tree_node(&dir, &name) {
        Some(root) => ok(root["children"].clone()),
        None => err(StatusCode::INTERNAL_SERVER_ERROR, "cannot read the package"),
    }
}

/// GET /api/packages/{name}/file?path=…
pub async fn file_get(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<FileQuery>,
    _claims: RequireClaims,
) -> Response {
    let Some((dir, _)) = resolve_package(&state, &name) else {
        return err(StatusCode::NOT_FOUND, "no such package");
    };
    read_file_at(&dir, &q.path)
}

/// PUT /api/packages/{name}/file?path=… — edits the repo in place; the
/// wrapper cache invalidates via the manifest's mtime fingerprint.
/// Remote packages are read-only (403).
pub async fn file_put(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<FileQuery>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<FileWrite>,
) -> Response {
    let dir = match writable_package_dir(&state, &name) {
        Ok(d) => d,
        Err((code, msg)) => return err(code, msg),
    };
    write_file_at(&dir, &q.path, &body.content)
}

/// POST /api/packages/{name}/doc/parse
pub async fn doc_parse(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<DocParse>,
) -> Response {
    let Some((dir, _)) = resolve_package(&state, &name) else {
        return err(StatusCode::NOT_FOUND, "no such package");
    };
    doc_parse_at(&state, &dir, body).await
}

/// POST /api/packages/{name}/doc/render
pub async fn doc_render(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<DocRender>,
) -> Response {
    let Some((dir, _)) = resolve_package(&state, &name) else {
        return err(StatusCode::NOT_FOUND, "no such package");
    };
    doc_render_at(&state, &dir, body).await
}

/// PUT /api/packages/{name}/doc — remote packages are read-only (403).
pub async fn doc_save(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<DocSave>,
) -> Response {
    let dir = match writable_package_dir(&state, &name) {
        Ok(d) => d,
        Err((code, msg)) => return err(code, msg),
    };
    doc_save_at(&state, &dir, body).await
}

/// POST /api/runbooks/{rb}/packages/{name}/import — copy an installed
/// package into the repository: the rescue path for packages that only
/// exist inside a runbook.
pub async fn import_to_repo(
    Extension(state): Extension<SharedState>,
    UrlPath((rb, name)): UrlPath<(String, String)>,
    _claims: RequireClaims,
) -> Response {
    let Some(packages_dir) = state.packages_dir.clone() else {
        return err(StatusCode::NOT_FOUND, "package repository not configured");
    };
    let Some(rb_dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    let rb_dir = rb_dir.canonicalize().unwrap_or(rb_dir);
    let src = match resolve_in(&rb_dir, &format!("pkgs/{name}"), true) {
        Ok(s) => s,
        Err(_) => {
            return err(
                StatusCode::NOT_FOUND,
                "package not installed in this runbook",
            );
        }
    };
    if !src.join("package.wcl").is_file() {
        return err(
            StatusCode::NOT_FOUND,
            "package not installed in this runbook",
        );
    }
    let dest = packages_dir.join(&name);
    if dest.exists() {
        return err(StatusCode::CONFLICT, "package already in the repository");
    }
    match crate::transport::copy_dir_filtered(&src, &dest) {
        Ok(()) => ok(json!({ "imported": name })),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot import: {e}"),
        ),
    }
}

/// DELETE /api/runbooks/{rb}/packages/{name} — remove an installed
/// package copy (the inverse of add-to-runbook).
pub async fn remove_from_runbook(
    Extension(state): Extension<SharedState>,
    UrlPath((rb, name)): UrlPath<(String, String)>,
    _claims: RequireClaims,
) -> Response {
    let Some(rb_dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    let rb_dir = rb_dir.canonicalize().unwrap_or(rb_dir);
    // Refuse symlinked entries outright: remove_dir_all through a link
    // would reach outside the runbook.
    let raw = rb_dir.join("pkgs").join(&name);
    if std::fs::symlink_metadata(&raw)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return err(
            StatusCode::BAD_REQUEST,
            "refusing to remove a symlinked package",
        );
    }
    let target = match resolve_in(&rb_dir, &format!("pkgs/{name}"), true) {
        Ok(t) => t,
        Err(_) => {
            return err(
                StatusCode::NOT_FOUND,
                "package not installed in this runbook",
            );
        }
    };
    if !target.join("package.wcl").is_file() {
        return err(
            StatusCode::NOT_FOUND,
            "package not installed in this runbook",
        );
    }
    match std::fs::remove_dir_all(&target) {
        Ok(()) => ok(json!({ "removed": name })),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot remove: {e}"),
        ),
    }
}

#[derive(Deserialize)]
pub struct AddRequest {
    pub playbook: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// POST /api/packages/{name}/add-to-playbook — copy (never symlink: the
/// runbook editor refuses symlink escapes) the real source dir — local
/// repo or a remote's cache — into `<runbook>/pkgs/<name>`.
pub async fn add_to_runbook(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(req): axum::Json<AddRequest>,
) -> Response {
    if state.packages_dir.is_none() {
        return err(StatusCode::NOT_FOUND, "package repository not configured");
    }
    let Some((src, _)) = resolve_package(&state, &name) else {
        return err(StatusCode::NOT_FOUND, "no such package");
    };
    let Some(rb_dir) = runbook_dir(&state, &req.playbook) else {
        return err(StatusCode::NOT_FOUND, "no such playbook");
    };
    let dest = rb_dir.join("pkgs").join(&name);
    if dest.exists() {
        if !req.overwrite {
            return err(StatusCode::CONFLICT, "package already in the playbook");
        }
        if let Err(e) = std::fs::remove_dir_all(&dest) {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot replace: {e}"),
            );
        }
    }
    if let Err(e) = crate::transport::copy_dir_filtered(&src, &dest) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot copy: {e}"),
        );
    }
    ok(json!({ "playbook": req.playbook, "package": name, "path": format!("pkgs/{name}") }))
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

        let found = scan_dir(tmp.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "linux_files");
        assert_eq!(found[0].1, tmp.path().join("linux_files"));
    }

    #[test]
    fn merge_prefers_earlier_sources_and_reports_shadowing() {
        let t = SystemTime::UNIX_EPOCH;
        let entry = |name: &str, dir: &str| (name.to_string(), PathBuf::from(dir), t);
        let merged = merge_sources(vec![
            ("local".into(), vec![entry("demo", "/local/demo")]),
            (
                "stdlib".into(),
                vec![
                    entry("demo", "/cache/demo"),
                    entry("linux_files", "/cache/lf"),
                ],
            ),
            ("mirror".into(), vec![entry("linux_files", "/mirror/lf")]),
        ]);

        assert_eq!(merged.entries.len(), 2);
        assert_eq!(merged.entries[0].name, "demo");
        assert_eq!(merged.entries[0].source, "local");
        assert_eq!(merged.entries[0].dir, PathBuf::from("/local/demo"));
        assert_eq!(merged.entries[1].name, "linux_files");
        assert_eq!(merged.entries[1].source, "stdlib");
        assert_eq!(
            merged.shadowed,
            vec![
                (
                    "demo".to_string(),
                    "local".to_string(),
                    "stdlib".to_string()
                ),
                (
                    "linux_files".to_string(),
                    "stdlib".to_string(),
                    "mirror".to_string()
                ),
            ]
        );
    }

    #[test]
    fn package_doc_extracts_from_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.wcl"),
            r#"package "demo" {
  description = "d"
  gatherer "os_info" {
    description = "facts"
    script = "gatherers/os_info.wscript"
  }
  resource "file_present" {
    description = "r"
    script = "resources/f.wscript"
    param "path" {
      description = "p"
      type = "string"
      required = true
    }
    param "content" {
      description = "c"
      type = "string"
      default = ""
    }
  }
}"#,
        )
        .unwrap();

        let doc = package_doc_at(tmp.path()).unwrap();
        assert_eq!(doc.name, "demo");
        assert_eq!(doc.gatherers[0].name, "os_info");
        let r = &doc.resources[0];
        assert_eq!(r.script, "resources/f.wscript");
        assert_eq!(r.params[0].required, Some(true));
        assert_eq!(r.params[1].required, None);
    }

    #[test]
    fn package_doc_reports_broken_manifests() {
        let tmp = tempfile::tempdir().unwrap();

        let (code, _) = package_doc_at(tmp.path()).unwrap_err();
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);

        std::fs::write(tmp.path().join("package.wcl"), "package {{{").unwrap();
        let (code, msg) = package_doc_at(tmp.path()).unwrap_err();
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(msg.contains("does not parse"), "{msg}");
    }

    #[test]
    fn package_dir_resolution_is_guarded() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("good")).unwrap();
        std::fs::write(tmp.path().join("good/package.wcl"), "x").unwrap();
        std::fs::create_dir_all(tmp.path().join("no_manifest")).unwrap();

        assert!(package_dir_in(tmp.path(), "good").is_some());
        assert!(package_dir_in(tmp.path(), "no_manifest").is_none());
        assert!(package_dir_in(tmp.path(), "absent").is_none());
        assert!(package_dir_in(tmp.path(), "../good").is_none());
        assert!(package_dir_in(tmp.path(), "a/b").is_none());
        assert!(package_dir_in(tmp.path(), "").is_none());
    }
}
