//! Package distribution: `pkgs/repo.wcl` records the registered git
//! package repositories and the installed packages with their source
//! repo + exact commit. `config-weave pkg` manages both, shelling out
//! to the `git` binary so the user's ambient credentials work against
//! private repos. Clones cache under `{playbook}/.repo-cache/<name>`.
//!
//! repo.wcl is tooling metadata, not playbook semantics: the model
//! loader never reads it (`load_packages` skips non-directory entries),
//! so a broken repo.wcl breaks only `pkg` commands, never check/apply.

mod git;
mod ops;
mod store;

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoDef {
    pub name: String,
    pub url: String,
    /// Subdirectory of the checkout holding the package dirs (e.g.
    /// "pkgs" for the stdlib); the checkout root when unset.
    pub subdir: Option<String>,
    /// Branch to track; the remote's default branch when unset.
    pub branch: Option<String>,
}

/// One installed package and the provenance recorded at install time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPkg {
    pub name: String,
    pub repo: String,
    pub commit: String,
}

/// The parsed `pkgs/repo.wcl`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PkgFile {
    pub repos: Vec<RepoDef>,
    pub packages: Vec<InstalledPkg>,
}

/// The default entry seeded when `pkg add`/`pkg search` run with no
/// repositories registered. A present file with repos (even hand-emptied
/// later) is respected.
pub fn stdlib_default() -> RepoDef {
    RepoDef {
        name: "stdlib".into(),
        url: "https://github.com/Configweave/config-weave-pkgs.git".into(),
        subdir: Some("pkgs".into()),
        branch: None,
    }
}

pub fn repo_wcl_path(dir: &Path) -> PathBuf {
    dir.join("pkgs").join("repo.wcl")
}

/// The repo's clone under `{playbook}/.repo-cache` (a dot-dir, so the
/// package scan never sees it).
pub fn cache_dir(dir: &Path, repo: &str) -> PathBuf {
    dir.join(".repo-cache").join(repo)
}

/// The directory holding the repo's package dirs, only once the clone
/// exists (it may be pending or failed).
pub fn packages_root(dir: &Path, repo: &RepoDef) -> Option<PathBuf> {
    let mut root = cache_dir(dir, &repo.name);
    match repo.subdir.as_deref() {
        Some(".") | None => {}
        Some(sub) => root = root.join(sub),
    }
    root.is_dir().then_some(root)
}

/// Directories never copied into an installed package.
const EXCLUDED_DIRS: [&str; 4] = [".git", "node_modules", "target", ".vmlab"];

/// Recursive copy that skips VCS/build directories and dot-dirs; never
/// follows or recreates symlinked layouts (files are copied flat).
fn copy_dir_filtered(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            if EXCLUDED_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            copy_dir_filtered(&path, &dest.join(&name))?;
        } else {
            std::fs::copy(&path, dest.join(&name))?;
        }
    }
    Ok(())
}

/// Dispatch for `config-weave pkg`, called from main. Errors render
/// like every other command: diagnostic to stderr, validation exit code.
pub fn cmd_pkg(dir: &Path, action: &crate::PkgCommand) -> u8 {
    let result = match action {
        crate::PkgCommand::Add { package } => ops::add(dir, package),
        crate::PkgCommand::Remove { package } => ops::remove(dir, package),
        crate::PkgCommand::Update { package } => ops::update(dir, package.as_deref()),
        crate::PkgCommand::Search { term } => ops::search(dir, term),
        crate::PkgCommand::Repo { action } => match action {
            crate::PkgRepoCommand::Add {
                name,
                url,
                branch,
                subdir,
            } => ops::repo_add(dir, name, url, branch.as_deref(), subdir.as_deref()),
            crate::PkgRepoCommand::Remove { name } => ops::repo_remove(dir, name),
            crate::PkgRepoCommand::List => ops::repo_list(dir),
        },
    };
    match result {
        Ok(()) => crate::EXIT_OK,
        Err(d) => {
            eprintln!("{}", d.rendered);
            crate::EXIT_VALIDATION
        }
    }
}
