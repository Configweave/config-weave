//! The `fs` module: file IO for scripts (PRD §7). Richer than wisp-std's
//! `fs` — metadata, glob, temp files/dirs, symlinks — so Config Weave
//! registers this module instead of the stdlib one.

use std::collections::HashMap;
use std::path::Path;

use wisp::Module;
use wisp_std::DynValue;

fn err(e: std::io::Error) -> String {
    e.to_string()
}

pub fn module() -> Module {
    let mut m = Module::new("fs");
    m.doc("File IO (capability: filesystem access)");

    m.doc_next("Read a file as text");
    m.fn_("read", |path: &str| -> Result<String, String> {
        std::fs::read_to_string(path).map_err(err)
    });
    m.doc_next("Read a file as bytes");
    m.fn_("read_bytes", |path: &str| -> Result<Vec<i64>, String> {
        std::fs::read(path)
            .map(|b| b.into_iter().map(|x| x as i64).collect())
            .map_err(err)
    });
    m.doc_next("Write text to a file, replacing its contents");
    m.fn_("write", |path: &str, content: &str| -> Result<(), String> {
        std::fs::write(path, content).map_err(err)
    });
    m.doc_next("Write bytes to a file, replacing its contents");
    m.fn_(
        "write_bytes",
        |path: &str, content: Vec<i64>| -> Result<(), String> {
            let bytes: Vec<u8> = content.iter().map(|b| *b as u8).collect();
            std::fs::write(path, bytes).map_err(err)
        },
    );
    m.doc_next("Append text to a file, creating it if absent");
    m.fn_("append", |path: &str, content: &str| -> Result<(), String> {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map_err(err)
    });
    m.doc_next("Copy a file");
    m.fn_("copy", |from: &str, to: &str| -> Result<(), String> {
        std::fs::copy(from, to).map(|_| ()).map_err(err)
    });
    m.doc_next("Move (rename) a file or directory");
    m.fn_("move", |from: &str, to: &str| -> Result<(), String> {
        std::fs::rename(from, to).map_err(err)
    });
    m.doc_next("Delete a file or symlink");
    m.fn_("delete", |path: &str| -> Result<(), String> {
        std::fs::remove_file(path).map_err(err)
    });
    m.doc_next("Delete a directory and everything in it");
    m.fn_("delete_dir", |path: &str| -> Result<(), String> {
        std::fs::remove_dir_all(path).map_err(err)
    });
    m.doc_next("Create a directory, including missing parents");
    m.fn_("mkdir", |path: &str| -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(err)
    });
    m.doc_next("Whether a path exists");
    m.fn_("exists", |path: &str| Path::new(path).exists());
    m.doc_next("Whether a path is a file");
    m.fn_("is_file", |path: &str| Path::new(path).is_file());
    m.doc_next("Whether a path is a directory");
    m.fn_("is_dir", |path: &str| Path::new(path).is_dir());
    m.doc_next("List entries of a directory (sorted names)");
    m.fn_("list_dir", |path: &str| -> Result<Vec<String>, String> {
        let mut entries: Vec<String> = std::fs::read_dir(path)
            .map_err(err)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        Ok(entries)
    });
    m.doc_next(
        "File metadata as a map: size, modified (unix secs), readonly, is_file, is_dir, \
         is_symlink, mode (unix permission bits; 0 elsewhere)",
    );
    m.fn_("metadata", |path: &str| -> Result<DynValue, String> {
        let meta = std::fs::symlink_metadata(path).map_err(err)?;
        let mut map = HashMap::new();
        map.insert("size".into(), DynValue::Int(meta.len() as i64));
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        map.insert("modified".into(), DynValue::Int(modified));
        map.insert(
            "readonly".into(),
            DynValue::Bool(meta.permissions().readonly()),
        );
        map.insert("is_file".into(), DynValue::Bool(meta.is_file()));
        map.insert("is_dir".into(), DynValue::Bool(meta.is_dir()));
        map.insert(
            "is_symlink".into(),
            DynValue::Bool(meta.file_type().is_symlink()),
        );
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() as i64
        };
        #[cfg(not(unix))]
        let mode = 0i64;
        map.insert("mode".into(), DynValue::Int(mode));
        Ok(DynValue::Map(map))
    });
    m.doc_next("Expand a glob pattern to matching paths (sorted)");
    m.fn_("glob", |pattern: &str| -> Result<Vec<String>, String> {
        let paths = glob::glob(pattern).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for p in paths {
            out.push(p.map_err(|e| e.to_string())?.display().to_string());
        }
        out.sort();
        Ok(out)
    });
    m.doc_next("Create a fresh temporary file and return its path");
    m.fn_("temp_file", || -> Result<String, String> {
        let f = tempfile::NamedTempFile::new().map_err(err)?;
        let (_, path) = f.keep().map_err(|e| e.to_string())?;
        Ok(path.display().to_string())
    });
    m.doc_next("Create a fresh temporary directory and return its path");
    m.fn_("temp_dir", || -> Result<String, String> {
        let d = tempfile::TempDir::new().map_err(err)?;
        Ok(d.keep().display().to_string())
    });
    m.doc_next("Create a symlink at `link` pointing to `target`");
    m.fn_("symlink", |target: &str, link: &str| -> Result<(), String> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).map_err(err)
        }
        #[cfg(windows)]
        {
            if Path::new(target).is_dir() {
                std::os::windows::fs::symlink_dir(target, link).map_err(err)
            } else {
                std::os::windows::fs::symlink_file(target, link).map_err(err)
            }
        }
    });
    m.doc_next("Read the target of a symlink");
    m.fn_("read_link", |path: &str| -> Result<String, String> {
        std::fs::read_link(path)
            .map(|p| p.display().to_string())
            .map_err(err)
    });
    m
}
