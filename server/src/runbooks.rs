//! Runbook browsing and editing: a runbook is an immediate child
//! directory of the server's root that contains a `playbook.wcl`.
//! Every file path from a client resolves against the runbook root and
//! is prefix-checked after canonicalization — the traversal guard.

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

/// Resolve a runbook by name; `None` when it does not exist under root.
pub(crate) fn runbook_dir(state: &SharedState, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    let dir = state.root.join(name);
    (dir.is_dir() && dir.join("playbook.wcl").is_file()).then_some(dir)
}

/// Canonicalize `path` and require it stays under `root`. For writes the
/// file may not exist yet, so the parent is what gets canonicalized.
fn resolve_in(root: &Path, rel: &str, must_exist: bool) -> Result<PathBuf, String> {
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
        return Err("path escapes the runbook".into());
    }
    Ok(canonical)
}

/// GET /api/runbooks — every child dir of root with a playbook.wcl.
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    let mut runbooks = Vec::new();
    let entries = match std::fs::read_dir(&state.root) {
        Ok(e) => e,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot read root: {e}"),
            );
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() && !name.starts_with('.') && path.join("playbook.wcl").is_file() {
            runbooks.push(json!({ "name": name }));
        }
    }
    runbooks.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ok(runbooks)
}

/// One node of the file tree.
fn tree_node(path: &Path, name: &str) -> Option<Value> {
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
    let file = match resolve_in(&dir, &q.path, true) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match std::fs::read_to_string(&file) {
        Ok(content) => ok(json!({ "path": q.path, "content": content })),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "not a UTF-8 text file")
        }
        Err(e) => err(StatusCode::NOT_FOUND, format!("cannot read: {e}")),
    }
}

#[derive(Deserialize)]
pub struct FileWrite {
    pub content: String,
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
    let file = match resolve_in(&dir, &q.path, false) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let tmp = file.with_extension("weave-tmp");
    if let Err(e) = std::fs::write(&tmp, &body.content) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot write: {e}"),
        );
    }
    if let Err(e) = std::fs::rename(&tmp, &file) {
        let _ = std::fs::remove_file(&tmp);
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot write: {e}"),
        );
    }
    ok(json!({ "path": q.path }))
}

/// Shell out to the config-weave CLI with `--json` and hand back its
/// stdout object (exit codes 0 and 2 both carry a valid JSON body).
async fn cli_json(state: &SharedState, args: &[&str]) -> Result<Value, String> {
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
}
