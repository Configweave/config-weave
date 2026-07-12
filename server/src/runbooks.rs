//! Runbook browsing and editing: a runbook is an immediate child
//! directory of the server's root that contains a `playbook.wcl`, or a
//! playbook dir under a remote repository's `runbooks_subdir` (merged
//! in repos.wcl order, local names shadowing remote ones — the same
//! policy as packages). Every file path from a client resolves against
//! the runbook root and is prefix-checked after canonicalization — the
//! traversal guard.

use std::path::{Path, PathBuf};

use axum::Extension;
use axum::extract::{Path as UrlPath, Query};
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::SharedState;

/// Directories never shown in the tree or served as files.
const IGNORED_DIRS: [&str; 4] = [".git", "node_modules", "target", ".vmlab"];

/// A runbook name straight from the URL: a single path component.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A runbook dir under `root`, guarded by name validity + manifest
/// presence.
fn runbook_dir_in(root: &Path, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    let dir = root.join(name);
    (dir.is_dir() && dir.join("playbook.wcl").is_file()).then(|| dir.canonicalize().unwrap_or(dir))
}

/// A runbook dir from any source, with its source tag; a local runbook
/// wins over remote copies of the same name (mirror of
/// `packages::resolve_package`).
pub(crate) fn resolve_runbook(state: &SharedState, name: &str) -> Option<(PathBuf, String)> {
    if let Some(dir) = runbook_dir_in(&state.root, name) {
        return Some((dir, crate::repos::LOCAL_SOURCE.into()));
    }
    let repos = state.repos.lock().unwrap().clone();
    for repo in &repos {
        if let Some(dir) =
            crate::repos::runbooks_root(state, repo).and_then(|root| runbook_dir_in(&root, name))
        {
            return Some((dir, repo.name.clone()));
        }
    }
    None
}

/// Resolve a runbook by name; `None` when no source provides it.
pub(crate) fn runbook_dir(state: &SharedState, name: &str) -> Option<PathBuf> {
    resolve_runbook(state, name).map(|(dir, _)| dir)
}

/// Canonicalize `path` and require it stays under `root` (a runbook or
/// a repo package dir). For writes the file may not exist yet, so the
/// parent is what gets canonicalized.
pub(crate) fn resolve_in(root: &Path, rel: &str, must_exist: bool) -> Result<PathBuf, String> {
    if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
        return Err("invalid path".into());
    }
    let joined = root.join(rel);
    let canonical = if must_exist {
        joined
            .canonicalize()
            .map_err(|_| "no such file".to_string())?
    } else {
        let parent = joined.parent().ok_or_else(|| "invalid path".to_string())?;
        let parent = parent
            .canonicalize()
            .map_err(|_| "no such directory".to_string())?;
        parent.join(
            joined
                .file_name()
                .ok_or_else(|| "invalid path".to_string())?,
        )
    };
    if !canonical.starts_with(root) {
        return Err("path escapes the root".into());
    }
    Ok(canonical)
}

/// Merge every runbook source: the local root first, then each remote
/// repo's runbooks root in repos.wcl order (so local runbooks shadow
/// remote ones). A repo whose cache is missing or unreadable is
/// skipped — the view must not die because one remote is broken.
pub(crate) fn scan_runbook_sources(state: &SharedState) -> crate::packages::ScanResult {
    let local = crate::packages::scan_dir_for(&state.root, "playbook.wcl").unwrap_or_default();
    let mut sources = vec![(crate::repos::LOCAL_SOURCE.to_string(), local)];
    let repos = state.repos.lock().unwrap().clone();
    for repo in &repos {
        let Some(root) = crate::repos::runbooks_root(state, repo) else {
            continue;
        };
        match crate::packages::scan_dir_for(&root, "playbook.wcl") {
            Ok(found) => sources.push((repo.name.clone(), found)),
            Err(e) => tracing::warn!(repo = %repo.name, "skipping repository runbooks: {e}"),
        }
    }
    crate::packages::merge_sources(sources)
}

/// GET /api/runbooks — the merged runbook list, each entry tagged with
/// its source, plus the shadowed-name collisions.
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    let scan = scan_runbook_sources(&state);
    let runbooks: Vec<Value> = scan
        .entries
        .iter()
        .map(|e| json!({ "name": e.name, "source": e.source }))
        .collect();
    let shadowed: Vec<Value> = scan
        .shadowed
        .iter()
        .map(|(name, by, source)| json!({ "name": name, "by": by, "source": source }))
        .collect();
    ok(json!({ "runbooks": runbooks, "shadowed": shadowed }))
}

/// One node of the file tree.
pub(crate) fn tree_node(path: &Path, name: &str) -> Option<Value> {
    if path.is_dir() {
        if IGNORED_DIRS.contains(&name) || name.starts_with('.') {
            return None;
        }
        let mut children: Vec<Value> = std::fs::read_dir(path)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let child_name = e.file_name().to_string_lossy().into_owned();
                tree_node(&e.path(), &child_name)
            })
            .collect();
        // Directories first, then files, both alphabetical.
        children.sort_by_key(|v| {
            (
                !v["dir"].as_bool().unwrap_or(false),
                v["name"].as_str().unwrap_or_default().to_string(),
            )
        });
        Some(json!({ "name": name, "dir": true, "children": children }))
    } else {
        Some(json!({ "name": name, "dir": false }))
    }
}

/// GET /api/runbooks/{rb}/tree
pub async fn tree(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some(dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    match tree_node(&dir, &rb) {
        Some(root) => ok(root["children"].clone()),
        None => err(StatusCode::INTERNAL_SERVER_ERROR, "cannot read the runbook"),
    }
}

#[derive(Deserialize)]
pub struct FileQuery {
    pub path: String,
}

/// Read one file under a canonical workspace root.
pub(crate) fn read_file_at(root: &Path, rel: &str) -> Response {
    let file = match resolve_in(root, rel, true) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match std::fs::read_to_string(&file) {
        Ok(content) => ok(json!({ "path": rel, "content": content })),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "not a UTF-8 text file")
        }
        Err(e) => err(StatusCode::NOT_FOUND, format!("cannot read: {e}")),
    }
}

/// GET /api/runbooks/{rb}/file?path=…
pub async fn file_get(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    Query(q): Query<FileQuery>,
    _claims: RequireClaims,
) -> Response {
    let Some(dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    // The runbook dir itself is under the canonical root by construction.
    let dir = dir.canonicalize().unwrap_or(dir);
    read_file_at(&dir, &q.path)
}

#[derive(Deserialize)]
pub struct FileWrite {
    pub content: String,
}

/// Atomic write via tmp + rename.
fn write_atomic(file: &Path, content: &str) -> Result<(), String> {
    let tmp = file.with_extension("weave-tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("cannot write: {e}"))?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write: {e}")
    })
}

/// Atomically write one file under a canonical workspace root.
pub(crate) fn write_file_at(root: &Path, rel: &str, content: &str) -> Response {
    let file = match resolve_in(root, rel, false) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match write_atomic(&file, content) {
        Ok(()) => ok(json!({ "path": rel })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// PUT /api/runbooks/{rb}/file?path=… — body `{content}`; atomic write.
pub async fn file_put(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    Query(q): Query<FileQuery>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<FileWrite>,
) -> Response {
    let Some(dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    let dir = dir.canonicalize().unwrap_or(dir);
    write_file_at(&dir, &q.path, &body.content)
}

/// Shell out to the config-weave CLI with `--json` and hand back its
/// stdout object (exit codes 0 and 2 both carry a valid JSON body).
pub(crate) async fn cli_json(state: &SharedState, args: &[&str]) -> Result<Value, String> {
    let out = tokio::process::Command::new(&state.config_weave)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("cannot run {}: {e}", state.config_weave))?;
    serde_json::from_slice(&out.stdout).map_err(|_| {
        format!(
            "{} produced no JSON: {}",
            state.config_weave,
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// POST /api/runbooks/{rb}/validate — full validation, diagnostics back.
pub async fn validate(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some(dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    match cli_json(&state, &["validate", &dir.to_string_lossy(), "--json"]).await {
        Ok(v) => ok(v),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// GET /api/runbooks/{rb}/inventory — plays, packages, tests, scenarios.
pub async fn inventory(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some(dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such runbook");
    };
    match cli_json(&state, &["list", &dir.to_string_lossy(), "--json"]).await {
        Ok(v) => ok(v),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ------------------------------------------------- graphical editors

/// Shell out to a hidden DocJson subcommand: one JSON object in on
/// stdin, one out on stdout (errors ride in-band as `ok:false`).
async fn cli_stdin_json(state: &SharedState, arg: &str, input: &Value) -> Result<Value, String> {
    use tokio::io::AsyncWriteExt as _;
    let mut child = tokio::process::Command::new(&state.config_weave)
        .arg(arg)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", state.config_weave))?;
    let payload = serde_json::to_vec(input).map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&payload).await.map_err(|e| e.to_string())?;
    drop(stdin);
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("{arg}: {e}"))?;
    serde_json::from_slice(&out.stdout).map_err(|_| {
        format!(
            "{arg} produced no JSON: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Which DocJson kind a file path edits, if any.
fn doc_kind(path: &str) -> Option<&'static str> {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base {
        "playbook.wcl" => Some("playbook"),
        "package.wcl" => Some("package"),
        _ => None,
    }
}

/// Deterministic content hash for the concurrent-edit guard (FNV-1a).
fn content_hash(content: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Resolve + read the file a doc request names under a canonical
/// workspace root. The Err side carries a ready HTTP response; its size
/// is fine for this cold path.
#[allow(clippy::result_large_err)]
fn doc_target_at(root: &Path, path: &str) -> Result<(PathBuf, String, &'static str), Response> {
    let Some(kind) = doc_kind(path) else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "only playbook.wcl and package.wcl files have a visual editor",
        ));
    };
    let file = match resolve_in(root, path, false) {
        Ok(f) => f,
        Err(e) => return Err(err(StatusCode::BAD_REQUEST, e)),
    };
    let on_disk = std::fs::read_to_string(&file).unwrap_or_default();
    Ok((file, on_disk, kind))
}

/// The canonical dir of a runbook, as a doc/file workspace root.
#[allow(clippy::result_large_err)]
fn runbook_root(state: &SharedState, rb: &str) -> Result<PathBuf, Response> {
    match runbook_dir(state, rb) {
        Some(dir) => Ok(dir.canonicalize().unwrap_or(dir)),
        None => Err(err(StatusCode::NOT_FOUND, "no such runbook")),
    }
}

#[derive(Deserialize)]
pub struct DocParse {
    pub path: String,
    /// Unsaved buffer content; None = read the file.
    pub content: Option<String>,
}

/// Source → DocJson (+ the on-disk content hash the eventual save must
/// present), under a canonical workspace root.
pub(crate) async fn doc_parse_at(state: &SharedState, root: &Path, body: DocParse) -> Response {
    let (_, on_disk, kind) = match doc_target_at(root, &body.path) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let source = body.content.unwrap_or_else(|| on_disk.clone());
    let input = json!({ "kind": kind, "source": source });
    match cli_stdin_json(state, "__wcl-inspect", &input).await {
        Ok(mut v) => {
            if let Some(map) = v.as_object_mut() {
                map.insert("base_hash".into(), content_hash(&on_disk).into());
            }
            ok(v)
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// POST /api/runbooks/{rb}/doc/parse
pub async fn doc_parse(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<DocParse>,
) -> Response {
    match runbook_root(&state, &rb) {
        Ok(root) => doc_parse_at(&state, &root, body).await,
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct DocRender {
    pub path: String,
    pub doc: Value,
    /// Sync base; None = the on-disk file (dry-run preview, no write).
    pub base_content: Option<String>,
}

/// DocJson → canonical WCL, without writing (mode switches and
/// previews), under a canonical workspace root.
pub(crate) async fn doc_render_at(state: &SharedState, root: &Path, body: DocRender) -> Response {
    let (_, on_disk, kind) = match doc_target_at(root, &body.path) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let base = body.base_content.unwrap_or(on_disk);
    let input = json!({ "kind": kind, "base_source": base, "doc": body.doc });
    match cli_stdin_json(state, "__wcl-render", &input).await {
        Ok(v) => ok(v),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// POST /api/runbooks/{rb}/doc/render
pub async fn doc_render(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<DocRender>,
) -> Response {
    match runbook_root(&state, &rb) {
        Ok(root) => doc_render_at(&state, &root, body).await,
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct DocSave {
    pub path: String,
    pub doc: Value,
    /// Hash from doc/parse; a mismatch means someone else changed the
    /// file since — 409 instead of a blind merge.
    pub base_hash: Option<String>,
}

/// Render against the on-disk file and write atomically; returns the
/// canonical content so the editor can sync its text buffer.
pub(crate) async fn doc_save_at(state: &SharedState, root: &Path, body: DocSave) -> Response {
    let (file, on_disk, kind) = match doc_target_at(root, &body.path) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Some(expected) = &body.base_hash
        && *expected != content_hash(&on_disk)
    {
        return err(
            StatusCode::CONFLICT,
            "the file changed on disk since it was opened — reload before saving",
        );
    }
    let input = json!({ "kind": kind, "base_source": on_disk, "doc": body.doc });
    let result = match cli_stdin_json(state, "__wcl-render", &input).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if result["ok"].as_bool() != Some(true) {
        // Render diagnostics are the caller's to display.
        return ok(result);
    }
    let content = result["source"].as_str().unwrap_or_default().to_string();
    match write_atomic(&file, &content) {
        Ok(()) => ok(json!({
            "ok": true,
            "path": body.path,
            "content": content,
            "base_hash": content_hash(&content),
        })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// PUT /api/runbooks/{rb}/doc
pub async fn doc_save(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<DocSave>,
) -> Response {
    match runbook_root(&state, &rb) {
        Ok(root) => doc_save_at(&state, &root, body).await,
        Err(resp) => resp,
    }
}

/// GET /api/templates — scaffold sources for "new script" actions.
pub async fn templates(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
) -> Response {
    match cli_json(&state, &["__templates"]).await {
        Ok(v) => ok(v),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("ok.wcl"), "x").unwrap();

        assert!(resolve_in(&root, "ok.wcl", true).is_ok());
        assert!(resolve_in(&root, "../escape", false).is_err());
        assert!(resolve_in(&root, "/abs", false).is_err());
        assert!(resolve_in(&root, "a/../../b", false).is_err());
        assert!(resolve_in(&root, "", true).is_err());
    }

    #[test]
    fn resolve_rejects_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();

        // A symlink pointing out of the runbook canonicalizes outside the
        // root and must be refused.
        assert!(resolve_in(&root, "link.txt", true).is_err());
    }

    #[test]
    fn runbook_names_are_single_components() {
        assert!(valid_name("my-runbook"));
        assert!(valid_name("pkgs_v2"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name(""));
    }

    #[test]
    fn runbook_scan_finds_only_playbook_dirs_and_merge_shadows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("real")).unwrap();
        std::fs::write(root.join("real/playbook.wcl"), "x").unwrap();
        std::fs::create_dir(root.join("not-a-runbook")).unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join(".hidden/playbook.wcl"), "x").unwrap();

        let found = crate::packages::scan_dir_for(root, "playbook.wcl").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "real");

        // Local shadows remote on a name collision, first repo wins next.
        let merged = crate::packages::merge_sources(vec![
            ("local".into(), found.clone()),
            ("repo-a".into(), found.clone()),
            ("repo-b".into(), found),
        ]);
        assert_eq!(merged.entries.len(), 1);
        assert_eq!(merged.entries[0].source, "local");
        assert_eq!(
            merged.shadowed,
            vec![
                ("real".into(), "local".into(), "repo-a".into()),
                ("real".into(), "local".into(), "repo-b".into()),
            ]
        );
    }

    #[test]
    fn runbook_dir_in_requires_a_manifest_and_canonicalizes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("rb")).unwrap();
        assert!(runbook_dir_in(root, "rb").is_none());
        std::fs::write(root.join("rb/playbook.wcl"), "x").unwrap();
        let dir = runbook_dir_in(root, "rb").unwrap();
        assert_eq!(dir, root.join("rb").canonicalize().unwrap());
        assert!(runbook_dir_in(root, "../rb").is_none());
    }
}
