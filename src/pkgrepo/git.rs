//! Git shell-outs for the repo cache. Everything goes through the `git`
//! binary so the user's ambient credential setup (SSH agent, credential
//! helpers) applies — that is what makes private repos work. Auth
//! prompts are disabled so a private URL without credentials fails fast
//! instead of hanging the command.

use std::path::Path;

use super::RepoDef;

fn git_output(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git (is git installed?): {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "git {} failed: {}",
        args.first()
            .map(|a| if *a == "-C" {
                args.get(2).copied().unwrap_or("")
            } else {
                a
            })
            .unwrap_or(""),
        stderr.trim()
    ))
}

fn git(args: &[&str]) -> Result<(), String> {
    git_output(args).map(|_| ())
}

#[derive(Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    Synced,
    /// The cache has local changes; syncing would destroy them.
    Skipped(String),
}

fn clone_repo(repo: &RepoDef, dest: &Path) -> Result<(), String> {
    let dest_s = dest.to_string_lossy().into_owned();
    let mut args = vec!["clone", "--depth", "1"];
    if let Some(b) = &repo.branch {
        args.extend(["--branch", b]);
    }
    args.extend(["--", repo.url.as_str(), &dest_s]);
    git(&args)
}

fn fetch_repo(repo: &RepoDef, dest_s: &str) -> Result<(), String> {
    let refspec = repo
        .branch
        .as_ref()
        .map(|b| format!("+refs/heads/{b}:refs/remotes/origin/{b}"));
    let mut fetch = vec!["-C", dest_s, "fetch", "--depth", "1", "origin"];
    if let Some(spec) = &refspec {
        fetch.push(spec);
    }
    git(&fetch)
}

/// Bring the cache to the remote tip: clone when absent (clearing any
/// half-made dir from an interrupted clone), fast-forward when clean,
/// refuse when dirty — a dirty cache is somebody's working copy.
pub fn sync_repo(repo: &RepoDef, dest: &Path) -> Result<SyncOutcome, String> {
    if !dest.join(".git").exists() {
        if dest.exists() {
            std::fs::remove_dir_all(dest)
                .map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        return clone_repo(repo, dest).map(|()| SyncOutcome::Synced);
    }
    if is_dirty(dest)? {
        return Ok(SyncOutcome::Skipped(
            "local changes in the cache — discard them or delete the cache dir first".into(),
        ));
    }
    let dest_s = dest.to_string_lossy().into_owned();
    fetch_repo(repo, &dest_s)?;
    git(&["-C", &dest_s, "reset", "--hard", "FETCH_HEAD"])?;
    Ok(SyncOutcome::Synced)
}

pub fn is_dirty(dest: &Path) -> Result<bool, String> {
    let dest_s = dest.to_string_lossy().into_owned();
    let porcelain = git_output(&["-C", &dest_s, "status", "--porcelain"])?;
    Ok(!porcelain.is_empty())
}

/// The exact commit the cache sits on — recorded as install provenance.
pub fn head_commit(dest: &Path) -> Result<String, String> {
    let dest_s = dest.to_string_lossy().into_owned();
    git_output(&["-C", &dest_s, "rev-parse", "HEAD"])
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A bare "remote" seeded through a work clone, plus a RepoDef
    /// pointing at it. Returns (tempdir, def, seed clone path).
    pub(crate) fn fake_remote() -> (tempfile::TempDir, RepoDef, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("remote.git");
        let seed = tmp.path().join("seed");
        sh(
            &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
            tmp.path(),
        );
        sh(
            &["clone", bare.to_str().unwrap(), seed.to_str().unwrap()],
            tmp.path(),
        );
        std::fs::create_dir_all(seed.join("pkgs/demo")).unwrap();
        std::fs::write(
            seed.join("pkgs/demo/package.wcl"),
            "package \"demo\" {\n  description = \"Demo package for tests\"\n}\n",
        )
        .unwrap();
        commit_and_push(&seed, "seed");
        let def = RepoDef {
            name: "fake".into(),
            url: format!("file://{}", bare.display()),
            subdir: Some("pkgs".into()),
            branch: Some("main".into()),
        };
        (tmp, def, seed)
    }

    pub(crate) fn sh(args: &[&str], cwd: &Path) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub(crate) fn commit_and_push(seed: &Path, msg: &str) {
        sh(&["add", "-A"], seed);
        sh(
            &[
                "-c",
                "user.name=seed",
                "-c",
                "user.email=seed@test",
                "commit",
                "-m",
                msg,
            ],
            seed,
        );
        sh(&["push", "origin", "main"], seed);
    }

    #[test]
    fn sync_clones_then_fast_forwards_a_clean_cache() {
        let (tmp, def, seed) = fake_remote();
        let dest = tmp.path().join("cache");
        assert_eq!(sync_repo(&def, &dest).unwrap(), SyncOutcome::Synced);
        assert!(dest.join("pkgs/demo/package.wcl").is_file());
        let first = head_commit(&dest).unwrap();
        assert_eq!(first.len(), 40);

        std::fs::write(seed.join("pkgs/demo/extra.txt"), "more\n").unwrap();
        commit_and_push(&seed, "more");
        assert_eq!(sync_repo(&def, &dest).unwrap(), SyncOutcome::Synced);
        assert!(dest.join("pkgs/demo/extra.txt").is_file());
        assert_ne!(head_commit(&dest).unwrap(), first);
    }

    #[test]
    fn sync_skips_a_dirty_cache_and_recovers_a_gitless_dir() {
        let (tmp, def, _seed) = fake_remote();
        let dest = tmp.path().join("cache");
        sync_repo(&def, &dest).unwrap();

        std::fs::write(dest.join("pkgs/demo/package.wcl"), "local edit\n").unwrap();
        assert!(matches!(
            sync_repo(&def, &dest).unwrap(),
            SyncOutcome::Skipped(_)
        ));

        // A half-made cache (no .git) is cleared and re-cloned.
        std::fs::remove_dir_all(dest.join(".git")).unwrap();
        assert_eq!(sync_repo(&def, &dest).unwrap(), SyncOutcome::Synced);
        assert!(dest.join(".git").exists());
    }
}
