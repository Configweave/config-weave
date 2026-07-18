//! The `config-weave pkg` command flows. Errors are `Diag`s (exit 2);
//! per-repo sync trouble during add/search degrades to warnings so one
//! broken private repo never blocks the rest.

use std::collections::HashMap;
use std::path::Path;

use super::git::{self, SyncOutcome};
use super::{InstalledPkg, PkgFile, RepoDef, cache_dir, packages_root, repo_wcl_path, store};
use crate::diag::Diag;

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Load repo.wcl, seeding the stdlib repo when no repositories are
/// registered (missing file included). Only `add` and `search` seed —
/// the other commands operate on what is already recorded.
fn ensure_repos(dir: &Path) -> Result<PkgFile, Diag> {
    let path = repo_wcl_path(dir);
    let mut file = store::load(&path)?.unwrap_or_default();
    if file.repos.is_empty() {
        let stdlib = super::stdlib_default();
        println!("seeded package repo '{}' ({})", stdlib.name, stdlib.url);
        file.repos.push(stdlib);
        store::save(&path, &file)?;
    }
    Ok(file)
}

/// After the cache dir first appears, nudge (never edit) the playbook's
/// .gitignore.
fn gitignore_note(dir: &Path) {
    let ignore = dir.join(".gitignore");
    let covered = std::fs::read_to_string(&ignore)
        .map(|s| s.lines().any(|l| l.trim() == ".repo-cache/"))
        .unwrap_or(false);
    if !covered {
        println!("note: add '.repo-cache/' to {}", ignore.display());
    }
}

/// Sync every repo, returning the outcome per repo name. Failures and
/// dirty skips become warnings; the repo still participates with its
/// cached checkout when one exists.
fn sync_all(dir: &Path, repos: &[RepoDef]) -> HashMap<String, Result<SyncOutcome, String>> {
    let fresh_cache = !dir.join(".repo-cache").exists();
    let mut outcomes = HashMap::new();
    for repo in repos {
        let outcome = git::sync_repo(repo, &cache_dir(dir, &repo.name));
        match &outcome {
            Ok(SyncOutcome::Synced) => {}
            Ok(SyncOutcome::Skipped(msg)) => {
                eprintln!(
                    "warning: repo '{}': {msg} — using the cached copy as-is",
                    repo.name
                );
            }
            Err(e) => eprintln!("warning: repo '{}' could not be synced: {e}", repo.name),
        }
        outcomes.insert(repo.name.clone(), outcome);
    }
    if fresh_cache && dir.join(".repo-cache").exists() {
        gitignore_note(dir);
    }
    outcomes
}

// ------------------------------------------------------------- pkg repo

pub fn repo_add(
    dir: &Path,
    name: &str,
    url: &str,
    branch: Option<&str>,
    subdir: Option<&str>,
) -> Result<(), Diag> {
    let path = repo_wcl_path(dir);
    let mut file = store::load(&path)?.unwrap_or_default();
    if file.repos.iter().any(|r| r.name == name) {
        return Err(Diag::bare(format!(
            "repository '{name}' is already registered"
        )));
    }
    let repo = RepoDef {
        name: name.into(),
        url: url.into(),
        subdir: subdir.map(Into::into),
        branch: branch.map(Into::into),
    };
    store::validate_repo(&repo).map_err(|e| Diag::bare(format!("repo '{name}': {e}")))?;
    file.repos.push(repo.clone());
    store::save(&path, &file)?;
    println!("registered repo '{name}' -> {url}");

    // Sync now so a bad URL or missing credentials surface here, not on
    // the first `pkg add`. The entry stays either way — credentials are
    // often fixable without re-registering.
    let cache = cache_dir(dir, name);
    match git::sync_repo(&repo, &cache) {
        Ok(_) => {
            let commit = git::head_commit(&cache).map_err(Diag::bare)?;
            println!("synced ({})", short(&commit));
            gitignore_note(dir);
            Ok(())
        }
        Err(e) => Err(Diag::bare(format!(
            "repo '{name}' registered but could not be synced: {e}"
        ))),
    }
}

pub fn repo_remove(dir: &Path, name: &str) -> Result<(), Diag> {
    let path = repo_wcl_path(dir);
    let mut file = store::load(&path)?
        .ok_or_else(|| Diag::bare(format!("no repository '{name}' in {}", path.display())))?;
    if !file.repos.iter().any(|r| r.name == name) {
        return Err(Diag::bare(format!(
            "no repository '{name}' in {}",
            path.display()
        )));
    }
    let dependents: Vec<&str> = file
        .packages
        .iter()
        .filter(|p| p.repo == name)
        .map(|p| p.name.as_str())
        .collect();
    if !dependents.is_empty() {
        eprintln!(
            "warning: packages installed from it remain: {} — 'pkg update' will fail for them \
             until the repo is re-added or they are removed",
            dependents.join(", ")
        );
    }
    file.repos.retain(|r| r.name != name);
    store::save(&path, &file)?;
    let _ = std::fs::remove_dir_all(cache_dir(dir, name));
    println!("removed repo '{name}'");
    Ok(())
}

pub fn repo_list(dir: &Path) -> Result<(), Diag> {
    let path = repo_wcl_path(dir);
    let Some(file) = store::load(&path)? else {
        println!(
            "no repositories registered ({} missing) — 'pkg add <package>' seeds the stdlib",
            path.display()
        );
        return Ok(());
    };
    // Local state only — no network.
    let rows: Vec<[String; 5]> = file
        .repos
        .iter()
        .map(|r| {
            let cache = cache_dir(dir, &r.name);
            let state = if !cache.join(".git").exists() {
                "not synced".to_string()
            } else if git::is_dirty(&cache).unwrap_or(false) {
                "dirty".to_string()
            } else {
                git::head_commit(&cache)
                    .map(|c| short(&c).to_string())
                    .unwrap_or_else(|_| "?".into())
            };
            [
                r.name.clone(),
                r.url.clone(),
                r.branch.clone().unwrap_or_else(|| "-".into()),
                r.subdir.clone().unwrap_or_else(|| "-".into()),
                state,
            ]
        })
        .collect();
    print_table(&["NAME", "URL", "BRANCH", "SUBDIR", "CACHE"], &rows);
    Ok(())
}

// ------------------------------------------------------------------ add

pub fn add(dir: &Path, package: &str) -> Result<(), Diag> {
    let path = repo_wcl_path(dir);
    let mut file = ensure_repos(dir)?;
    if let Some(installed) = file.packages.iter().find(|p| p.name == package) {
        return Err(Diag::bare(format!(
            "package '{package}' is already installed (from '{}' @ {}) — use 'pkg update {package}'",
            installed.repo,
            short(&installed.commit)
        )));
    }
    if !store::valid_name(package) {
        return Err(Diag::bare(format!("invalid package name '{package}'")));
    }

    let outcomes = sync_all(dir, &file.repos);
    let holders: Vec<&RepoDef> = file
        .repos
        .iter()
        .filter(|r| {
            packages_root(dir, r)
                .map(|root| root.join(package).join("package.wcl").is_file())
                .unwrap_or(false)
        })
        .collect();

    let Some(winner) = holders.first() else {
        let searched: Vec<&str> = file.repos.iter().map(|r| r.name.as_str()).collect();
        let failed: Vec<&str> = file
            .repos
            .iter()
            .filter(|r| matches!(outcomes.get(&r.name), Some(Err(_))))
            .map(|r| r.name.as_str())
            .collect();
        let mut msg = format!(
            "package '{package}' not found in any registered repo (searched: {}) — try 'pkg search'",
            searched.join(", ")
        );
        if !failed.is_empty() {
            msg.push_str(&format!(" (could not sync: {})", failed.join(", ")));
        }
        return Err(Diag::bare(msg));
    };
    for shadowed in &holders[1..] {
        println!(
            "note: '{package}' also exists in repo '{}' (shadowed by '{}')",
            shadowed.name, winner.name
        );
    }

    let dest = dir.join("pkgs").join(package);
    if dest.exists() {
        return Err(Diag::bare(format!(
            "{} already exists but is not a tracked package — remove it first",
            dest.display()
        )));
    }
    let src = packages_root(dir, winner)
        .expect("holder has a packages root")
        .join(package);
    super::copy_dir_filtered(&src, &dest)
        .map_err(|e| Diag::bare(format!("cannot copy into {}: {e}", dest.display())))?;
    let commit = git::head_commit(&cache_dir(dir, &winner.name)).map_err(Diag::bare)?;
    file.packages.push(InstalledPkg {
        name: package.into(),
        repo: winner.name.clone(),
        commit: commit.clone(),
    });
    store::save(&path, &file)?;
    println!(
        "installed '{package}' from '{}' @ {}",
        winner.name,
        short(&commit)
    );
    Ok(())
}

// --------------------------------------------------------------- remove

pub fn remove(dir: &Path, package: &str) -> Result<(), Diag> {
    let path = repo_wcl_path(dir);
    let mut file = store::load(&path)?.unwrap_or_default();
    if !file.packages.iter().any(|p| p.name == package) {
        let mut msg = format!("package '{package}' is not installed");
        if dir.join("pkgs").join(package).exists() {
            msg.push_str(&format!(
                " (pkgs/{package} exists but was not installed by 'pkg add' — delete it manually)"
            ));
        }
        return Err(Diag::bare(msg));
    }
    let dest = dir.join("pkgs").join(package);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .map_err(|e| Diag::bare(format!("cannot remove {}: {e}", dest.display())))?;
    } else {
        println!("note: pkgs/{package} was already gone; removing its entry");
    }
    file.packages.retain(|p| p.name != package);
    store::save(&path, &file)?;
    println!("removed '{package}'");
    Ok(())
}

// --------------------------------------------------------------- update

pub fn update(dir: &Path, package: Option<&str>) -> Result<(), Diag> {
    let path = repo_wcl_path(dir);
    let mut file = store::load(&path)?.unwrap_or_default();
    let targets: Vec<InstalledPkg> = match package {
        Some(name) => {
            let Some(p) = file.packages.iter().find(|p| p.name == name) else {
                return Err(Diag::bare(format!("package '{name}' is not installed")));
            };
            vec![p.clone()]
        }
        None => file.packages.clone(),
    };
    if targets.is_empty() {
        println!("no packages installed");
        return Ok(());
    }

    // Sync each involved repo once.
    let mut outcomes: HashMap<String, Result<SyncOutcome, String>> = HashMap::new();
    for pkg in &targets {
        if outcomes.contains_key(&pkg.repo) {
            continue;
        }
        let outcome = match file.repos.iter().find(|r| r.name == pkg.repo) {
            Some(repo) => git::sync_repo(repo, &cache_dir(dir, &repo.name)),
            None => Err(format!(
                "repo '{}' is not registered — 'pkg repo add' it or 'pkg remove' its packages",
                pkg.repo
            )),
        };
        outcomes.insert(pkg.repo.clone(), outcome);
    }

    let mut errored = false;
    let mut changed = false;
    for pkg in &targets {
        match outcomes.get(&pkg.repo).expect("synced above") {
            Err(e) => {
                eprintln!("{}: error: {e}", pkg.name);
                errored = true;
            }
            // Never copy from a dirty cache — the recorded commit would lie.
            Ok(SyncOutcome::Skipped(msg)) => {
                println!("{}: skipped ({msg})", pkg.name);
            }
            Ok(SyncOutcome::Synced) => {
                let repo = file
                    .repos
                    .iter()
                    .find(|r| r.name == pkg.repo)
                    .expect("outcome exists only for registered repos");
                let cache = cache_dir(dir, &repo.name);
                let tip = git::head_commit(&cache).map_err(Diag::bare)?;
                if tip == pkg.commit {
                    println!("{}: up to date ({})", pkg.name, short(&tip));
                    continue;
                }
                let src = match packages_root(dir, repo).map(|r| r.join(&pkg.name)) {
                    Some(s) if s.join("package.wcl").is_file() => s,
                    _ => {
                        eprintln!(
                            "{}: error: no longer present in repo '{}' — 'pkg remove' it \
                             to keep the local copy unmanaged",
                            pkg.name, repo.name
                        );
                        errored = true;
                        continue;
                    }
                };
                // Delete-then-copy so files removed upstream vanish too.
                let dest = dir.join("pkgs").join(&pkg.name);
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)
                        .map_err(|e| Diag::bare(format!("cannot clear {}: {e}", dest.display())))?;
                }
                super::copy_dir_filtered(&src, &dest)
                    .map_err(|e| Diag::bare(format!("cannot copy into {}: {e}", dest.display())))?;
                println!("{}: {} -> {}", pkg.name, short(&pkg.commit), short(&tip));
                if let Some(entry) = file.packages.iter_mut().find(|p| p.name == pkg.name) {
                    entry.commit = tip;
                }
                changed = true;
            }
        }
    }
    if changed {
        store::save(&path, &file)?;
    }
    if errored {
        return Err(Diag::bare("some packages could not be updated".to_string()));
    }
    Ok(())
}

// --------------------------------------------------------------- search

pub fn search(dir: &Path, term: &str) -> Result<(), Diag> {
    let file = ensure_repos(dir)?;
    sync_all(dir, &file.repos);
    let needle = term.to_lowercase();

    let mut rows: Vec<[String; 3]> = Vec::new();
    for repo in &file.repos {
        let Some(root) = packages_root(dir, repo) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("package.wcl").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| store::valid_name(n))
            .collect();
        names.sort();
        for name in names {
            let description = package_description(&root.join(&name).join("package.wcl"));
            if !name.to_lowercase().contains(&needle)
                && !description.to_lowercase().contains(&needle)
            {
                continue;
            }
            let marker = match file.packages.iter().find(|p| p.name == name) {
                Some(p) if p.repo == repo.name => "  [installed]".to_string(),
                Some(p) => format!("  [installed from {}]", p.repo),
                None => String::new(),
            };
            rows.push([repo.name.clone(), name, format!("{description}{marker}")]);
        }
    }
    if rows.is_empty() {
        println!("no packages matching '{term}'");
        return Ok(());
    }
    print_table(&["REPO", "PACKAGE", "DESCRIPTION"], &rows);
    Ok(())
}

/// Best-effort description straight from the AST (no schema validation
/// — search must tolerate packages this binary version can't fully load).
fn package_description(path: &Path) -> String {
    let Ok(source) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let Ok(ast) = wcl_lang::parse_for_edit(&source, "package.wcl") else {
        return String::new();
    };
    crate::model::inspect_ast::extract_package(&ast)
        .map(|p| p.description)
        .unwrap_or_default()
}

fn print_table<const N: usize>(headers: &[&str; N], rows: &[[String; N]]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let print_row = |cells: &[&str]| {
        let line = cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", line.trim_end());
    };
    print_row(&headers.map(|h| h));
    for row in rows {
        print_row(&row.iter().map(|c| c.as_str()).collect::<Vec<_>>());
    }
}
