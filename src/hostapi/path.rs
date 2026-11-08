//! The `path` module: pure string path manipulation, platform-aware
//! separators, no IO (PRD §7).

use std::path::{Component, Path, PathBuf};

use wscript::Module;

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
    m.fn_("filename", |p: &str| -> String { filename_of(p) });
    m.doc_next("Extension of the final component, without the dot");
    m.fn_("extension", |p: &str| -> String { extension_of(p) });
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

fn filename_of(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn extension_of(p: &str) -> String {
    Path::new(p)
        .extension()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(p: &str) -> String {
        normalize(Path::new(p)).display().to_string()
    }

    #[test]
    fn normalize_keeps_leading_parent_dirs() {
        assert_eq!(norm("../../a"), "../../a");
        assert_eq!(norm("../.."), "../..");
        assert_eq!(norm("../a/../../b"), "../../b");
    }

    #[test]
    fn normalize_pops_normal_components_before_parent_dirs() {
        assert_eq!(norm("a/../.."), "..");
        assert_eq!(norm("a/b/../../c"), "c");
        assert_eq!(norm("a/.."), ".");
    }

    #[test]
    fn normalize_drops_parent_dirs_at_root() {
        assert_eq!(norm("/.."), "/");
        assert_eq!(norm("/../../a"), "/a");
        assert_eq!(norm("/a/../../b"), "/b");
    }

    #[test]
    fn normalize_resolves_current_dirs() {
        assert_eq!(norm("/a/./b/../c"), "/a/c");
        assert_eq!(norm("./a/./b"), "a/b");
        assert_eq!(norm("a/b/"), "a/b");
    }

    #[test]
    fn normalize_empty_is_dot() {
        assert_eq!(norm(""), ".");
        assert_eq!(norm("."), ".");
        assert_eq!(norm("./."), ".");
    }

    #[test]
    fn filename_edge_cases() {
        assert_eq!(filename_of("a/b.txt"), "b.txt");
        assert_eq!(filename_of("a/b/"), "b");
        assert_eq!(filename_of(".hidden"), ".hidden");
        assert_eq!(filename_of("a/.."), "");
        assert_eq!(filename_of("/"), "");
        assert_eq!(filename_of(""), "");
    }

    #[test]
    fn extension_edge_cases() {
        assert_eq!(extension_of("a/b.txt"), "txt");
        assert_eq!(extension_of("a.tar.gz"), "gz");
        assert_eq!(extension_of(".bashrc"), "");
        assert_eq!(extension_of("noext"), "");
        assert_eq!(extension_of("dir.d/file"), "");
        assert_eq!(extension_of(""), "");
    }
}
