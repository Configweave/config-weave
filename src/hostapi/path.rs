//! The `path` module: pure string path manipulation, platform-aware
//! separators, no IO (PRD §7).

use std::path::{Component, Path, PathBuf};

use wisp::Module;

pub fn module() -> Module {
    let mut m = Module::new("path");
    m.doc("Pure path-string manipulation; no IO");

    m.doc_next("Join two path segments with the platform separator");
    m.fn_("join", |a: &str, b: &str| -> String {
        Path::new(a).join(b).display().to_string()
    });
    m.doc_next("Parent directory of a path (empty string at the root)");
    m.fn_("parent", |p: &str| -> String {
        Path::new(p)
            .parent()
            .map(|x| x.display().to_string())
            .unwrap_or_default()
    });
    m.doc_next("Final component of a path");
    m.fn_("filename", |p: &str| -> String {
        Path::new(p)
            .file_name()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    m.doc_next("Extension of the final component, without the dot");
    m.fn_("extension", |p: &str| -> String {
        Path::new(p)
            .extension()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    m.doc_next("Lexically normalize a path (resolve `.` and `..`, no IO)");
    m.fn_("normalize", |p: &str| -> String {
        normalize(Path::new(p)).display().to_string()
    });
    m.doc_next("Make a path absolute against the current directory, then normalize");
    m.fn_("absolutize", |p: &str| -> Result<String, String> {
        let path = Path::new(p);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(path)
        };
        Ok(normalize(&abs).display().to_string())
    });
    m
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}
