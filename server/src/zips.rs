//! Playbook zip download/upload: `GET /api/playbooks/{rb}/download`
//! produces a self-contained archive (installed `pkgs/` included) under
//! a `<rb>/` top folder; `POST /api/playbooks/upload` creates a new
//! runbook from one. Runbooks are small, so archives are buffered in
//! memory.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use axum::Extension;
use axum::extract::{Path as UrlPath, Query};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use forge_server::{RequireClaims, err, ok};
use serde::Deserialize;
use serde_json::json;
use zip::write::SimpleFileOptions;

use crate::runbooks::{runbook_dir, valid_name};
use crate::state::SharedState;

/// Same skip list as copy_dir_filtered: VCS/build clutter and dot-dirs
/// stay out of archives in both directions.
const EXCLUDED_DIRS: [&str; 4] = [".git", "node_modules", "target", ".vmlab"];

fn excluded_dir(name: &str) -> bool {
    EXCLUDED_DIRS.contains(&name) || name.starts_with('.')
}

// -------------------------------------------------------------- download

/// Zip a directory with entries under a `<top>/` folder, skipping
/// excluded dirs and symlinks. Split from the handler for unit tests.
fn zip_dir(dir: &Path, top: &str) -> Result<Vec<u8>, String> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    add_dir(&mut writer, dir, top, options)?;
    Ok(writer
        .finish()
        .map_err(|e| format!("cannot build archive: {e}"))?
        .into_inner())
}

fn add_dir(
    writer: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    dir: &Path,
    prefix: &str,
    options: SimpleFileOptions,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry
            .metadata()
            .map_err(|e| format!("cannot stat {name}: {e}"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if excluded_dir(&name) {
                continue;
            }
            add_dir(writer, &entry.path(), &format!("{prefix}/{name}"), options)?;
        } else {
            let mut file = std::fs::File::open(entry.path())
                .map_err(|e| format!("cannot read {name}: {e}"))?;
            writer
                .start_file(format!("{prefix}/{name}"), options)
                .and_then(|()| std::io::copy(&mut file, writer).map_err(Into::into))
                .map_err(|e| format!("cannot archive {name}: {e}"))?;
        }
    }
    Ok(())
}

/// GET /api/playbooks/{rb}/download — a raw zip response, not the JSON
/// envelope (the UI hands it straight to the browser as a file).
pub async fn download(
    Extension(state): Extension<SharedState>,
    UrlPath(rb): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let Some(dir) = runbook_dir(&state, &rb) else {
        return err(StatusCode::NOT_FOUND, "no such playbook");
    };
    match zip_dir(&dir, &rb) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{rb}.zip\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ---------------------------------------------------------------- upload

#[derive(Deserialize)]
pub struct UploadQuery {
    pub name: Option<String>,
}

/// Archive junk that must not defeat the single-top-folder detection
/// (macOS Finder zips ship a `__MACOSX/` shadow tree).
fn junk_entry(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "__MACOSX" || s == ".DS_Store"
    })
}

/// The archive layout: playbook.wcl at the root (`prefix` empty), or
/// inside exactly one top-level folder (`prefix` = that folder). The
/// folder name doubles as the default runbook name.
fn detect_prefix(names: &[PathBuf]) -> Result<Option<String>, String> {
    if names.iter().any(|n| n == Path::new("playbook.wcl")) {
        return Ok(None);
    }
    let mut tops = std::collections::BTreeSet::new();
    for name in names {
        if let Some(std::path::Component::Normal(first)) = name.components().next() {
            tops.insert(first.to_string_lossy().into_owned());
        }
    }
    if tops.len() == 1 {
        let top = tops.into_iter().next().expect("len == 1");
        if names
            .iter()
            .any(|n| *n == Path::new(&top).join("playbook.wcl"))
        {
            return Ok(Some(top));
        }
    }
    Err("no playbook.wcl found in the archive (expected at the zip root or inside a single top-level folder)".into())
}

/// Validate and extract an uploaded runbook zip; returns the new
/// runbook's name. Extraction goes into a dot-prefixed tempdir inside
/// `root` (same filesystem, invisible to the runbook listing) and is
/// renamed into place only when everything succeeded.
fn extract_runbook(
    root: &Path,
    bytes: &[u8],
    name: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| (StatusCode::BAD_REQUEST, "not a zip archive".to_string()))?;

    // enclosed_name() is the zip-slip guard: absolute paths and `..`
    // components come back as None and the whole upload is refused.
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("broken archive: {e}")))?;
        let path = entry.enclosed_name().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("unsafe path in archive: {}", entry.name()),
            )
        })?;
        names.push(path);
    }
    let real: Vec<PathBuf> = names.iter().filter(|n| !junk_entry(n)).cloned().collect();
    let prefix = detect_prefix(&real).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let name = match name {
        Some(n) => n.to_string(),
        None => prefix.clone().ok_or((
            StatusCode::BAD_REQUEST,
            "the archive has no top-level folder to name the playbook after — pass ?name=…"
                .to_string(),
        ))?,
    };
    if !valid_name(&name) {
        return Err((
            StatusCode::BAD_REQUEST,
            "playbook name must be alphanumeric with - _ . and not start with '.'".to_string(),
        ));
    }
    let dest = root.join(&name);
    if dest.exists() {
        return Err((
            StatusCode::CONFLICT,
            format!("a playbook named '{name}' already exists"),
        ));
    }

    let tmp = tempfile::TempDir::with_prefix_in(".weave-upload-", root).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot create staging dir: {e}"),
        )
    })?;
    let extract_err = |e: String| (StatusCode::INTERNAL_SERVER_ERROR, e);
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("broken archive: {e}")))?;
        let path = entry.enclosed_name().expect("checked above");
        if junk_entry(&path) {
            continue;
        }
        let rel = match &prefix {
            Some(p) => match path.strip_prefix(p) {
                Ok(r) if r.as_os_str().is_empty() => continue, // the top folder itself
                Ok(r) => r.to_path_buf(),
                Err(_) => continue, // stray sibling of the top folder
            },
            None => path,
        };
        // The same clutter the download side skips, plus symlink entries
        // (extracting attacker-controlled links invites escapes).
        if rel.components().any(|c| {
            matches!(c, std::path::Component::Normal(n)
                if excluded_dir(&n.to_string_lossy()))
        }) {
            continue;
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            continue;
        }
        let target = tmp.path().join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| extract_err(format!("cannot extract {}: {e}", rel.display())))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| extract_err(format!("cannot extract {}: {e}", rel.display())))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|e| extract_err(format!("cannot extract {}: {e}", rel.display())))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| extract_err(format!("cannot extract {}: {e}", rel.display())))?;
    }

    std::fs::rename(tmp.path(), &dest)
        .map_err(|e| extract_err(format!("cannot move the playbook into place: {e}")))?;
    // The staging dir was renamed away; disarm the TempDir cleanup.
    let _ = tmp.keep();
    Ok(name)
}

/// POST /api/playbooks/upload?name=… — body: raw zip bytes.
pub async fn upload(
    Extension(state): Extension<SharedState>,
    Query(q): Query<UploadQuery>,
    _claims: RequireClaims,
    body: axum::body::Bytes,
) -> Response {
    match extract_runbook(&state.root, &body, q.name.as_deref()) {
        Ok(name) => ok(json!({ "name": name })),
        Err((code, msg)) => err(code, msg),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn build_zip(entries: &[(&str, Option<&str>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, content) in entries {
            match content {
                Some(c) => {
                    writer.start_file(name.to_string(), options).unwrap();
                    writer.write_all(c.as_bytes()).unwrap();
                }
                None => {
                    writer.add_directory(name.to_string(), options).unwrap();
                }
            }
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn download_upload_round_trips_including_pkgs() {
        let src = tempfile::tempdir().unwrap();
        let rb = src.path().join("demo");
        std::fs::create_dir_all(rb.join("pkgs/linux_files/resources")).unwrap();
        std::fs::create_dir_all(rb.join(".git")).unwrap();
        std::fs::write(rb.join("playbook.wcl"), "playbook \"demo\" {}\n").unwrap();
        std::fs::write(rb.join("pkgs/linux_files/package.wcl"), "pkg").unwrap();
        std::fs::write(rb.join("pkgs/linux_files/resources/f.wisp"), "res").unwrap();
        std::fs::write(rb.join(".git/HEAD"), "ref").unwrap();

        let bytes = zip_dir(&rb, "demo").unwrap();
        let root = tempfile::tempdir().unwrap();
        let name = extract_runbook(root.path(), &bytes, None).unwrap();
        assert_eq!(name, "demo");
        let out = root.path().join("demo");
        assert_eq!(
            std::fs::read_to_string(out.join("playbook.wcl")).unwrap(),
            "playbook \"demo\" {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("pkgs/linux_files/resources/f.wisp")).unwrap(),
            "res"
        );
        assert!(!out.join(".git").exists());
        // No staging leftovers.
        let strays: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "demo")
            .collect();
        assert!(strays.is_empty(), "{strays:?}");
    }

    #[test]
    fn root_level_layout_needs_an_explicit_name() {
        let bytes = build_zip(&[("playbook.wcl", Some("playbook \"x\" {}\n"))]);
        let root = tempfile::tempdir().unwrap();
        let e = extract_runbook(root.path(), &bytes, None).unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
        assert!(e.1.contains("?name="), "{}", e.1);

        let name = extract_runbook(root.path(), &bytes, Some("uploaded")).unwrap();
        assert_eq!(name, "uploaded");
        assert!(root.path().join("uploaded/playbook.wcl").is_file());
    }

    #[test]
    fn existing_names_conflict() {
        let bytes = build_zip(&[("demo/playbook.wcl", Some("x"))]);
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("demo")).unwrap();
        let e = extract_runbook(root.path(), &bytes, None).unwrap_err();
        assert_eq!(e.0, StatusCode::CONFLICT);
    }

    #[test]
    fn archives_without_a_playbook_are_rejected() {
        let bytes = build_zip(&[("demo/readme.md", Some("x"))]);
        let root = tempfile::tempdir().unwrap();
        let e = extract_runbook(root.path(), &bytes, None).unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
        assert!(e.1.contains("playbook.wcl"), "{}", e.1);

        let not_a_zip = extract_runbook(root.path(), b"nope", None).unwrap_err();
        assert_eq!(not_a_zip.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn traversal_and_junk_entries_are_handled() {
        let evil = build_zip(&[
            ("demo/playbook.wcl", Some("x")),
            ("../evil.txt", Some("boom")),
        ]);
        let root = tempfile::tempdir().unwrap();
        let e = extract_runbook(root.path(), &evil, None).unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
        assert!(e.1.contains("unsafe path"), "{}", e.1);

        // Finder junk must not break single-folder detection.
        let mac = build_zip(&[
            ("demo/playbook.wcl", Some("x")),
            ("__MACOSX/demo/._playbook.wcl", Some("junk")),
        ]);
        let name = extract_runbook(root.path(), &mac, None).unwrap();
        assert_eq!(name, "demo");
        assert!(!root.path().join("demo/__MACOSX").exists());
    }
}
