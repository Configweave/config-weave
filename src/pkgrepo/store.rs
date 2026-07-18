//! `pkgs/repo.wcl` load/save. Loaded through the embedded vocabulary
//! and regenerated from the structs on every `pkg` command — the file
//! is machine-managed metadata, so hand edits to values survive a
//! reload but comments do not. A missing file is `Ok(None)` (the caller
//! decides whether to seed); a malformed file is an error, because a
//! later save would clobber a file we could not fully read.

use std::path::Path;

use wcl_lang::{Document, Environment, ast, edit, format as wclformat};

use super::{InstalledPkg, PkgFile, RepoDef};
use crate::diag::Diag;
use crate::vocab;

/// Read and schema-validate `pkgs/repo.wcl`.
pub fn load(path: &Path) -> Result<Option<PkgFile>, Diag> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Diag::bare(format!("cannot read {}: {e}", path.display()))),
    };

    let with_import = vocab::with_import(&source, vocab::REPO_IMPORT, false);
    let env = Environment::new();
    let doc = Document::open_at_with_loader(
        &with_import,
        "repo.wcl",
        path.parent().map(|p| p.to_path_buf()),
        &env,
        vocab::loader(None),
    )
    .map_err(|e| Diag::bare(format!("{}: {e}", path.display())))?;

    let schema_errors = doc.schema_errors();
    if !schema_errors.is_empty() {
        let msgs: Vec<String> = schema_errors.iter().map(|e| e.to_string()).collect();
        return Err(Diag::bare(format!(
            "{}: {}",
            path.display(),
            msgs.join("; ")
        )));
    }

    let mut file = PkgFile::default();
    for block in doc.blocks() {
        match block.kind() {
            "repo" => file
                .repos
                .push(read_repo(&block).map_err(|e| at(path, &e))?),
            "package" => file
                .packages
                .push(read_package(&block).map_err(|e| at(path, &e))?),
            _ => {}
        }
    }

    let mut seen = std::collections::HashSet::new();
    for repo in &file.repos {
        if !seen.insert(repo.name.as_str()) {
            return Err(at(path, &format!("duplicate repository '{}'", repo.name)));
        }
        validate_repo(repo).map_err(|e| at(path, &format!("repo '{}': {e}", repo.name)))?;
    }
    let mut seen = std::collections::HashSet::new();
    for pkg in &file.packages {
        if !seen.insert(pkg.name.as_str()) {
            return Err(at(
                path,
                &format!("duplicate installed package '{}'", pkg.name),
            ));
        }
        if !valid_name(&pkg.name) {
            return Err(at(path, &format!("invalid package name '{}'", pkg.name)));
        }
    }
    Ok(Some(file))
}

fn at(path: &Path, msg: &str) -> Diag {
    Diag::bare(format!("{}: {msg}", path.display()))
}

fn block_label(block: &wcl_lang::Block<'_>, kind: &str) -> Result<String, String> {
    match block
        .labels()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        Some(wcl_lang::Value::Utf8(s))
        | Some(wcl_lang::Value::Ascii(s))
        | Some(wcl_lang::Value::Identifier(s)) => Ok(s),
        _ => Err(format!("{kind} block has no name label")),
    }
}

fn str_field(block: &wcl_lang::Block<'_>, field: &str) -> Result<Option<String>, String> {
    let Some(f) = block.fields().find(|f| f.name() == field) else {
        return Ok(None);
    };
    match f.value().map_err(|e| e.to_string())?.clone() {
        wcl_lang::Value::Utf8(s) | wcl_lang::Value::Ascii(s) | wcl_lang::Value::Identifier(s) => {
            Ok(Some(s))
        }
        other => Err(format!("field '{field}' must be a string, got {other:?}")),
    }
}

fn read_repo(block: &wcl_lang::Block<'_>) -> Result<RepoDef, String> {
    let name = block_label(block, "repo")?;
    Ok(RepoDef {
        url: str_field(block, "url")?
            .ok_or_else(|| format!("repo '{name}': missing field 'url'"))?,
        subdir: str_field(block, "subdir")?,
        branch: str_field(block, "branch")?,
        name,
    })
}

fn read_package(block: &wcl_lang::Block<'_>) -> Result<InstalledPkg, String> {
    let name = block_label(block, "package")?;
    let required = |field: &str| -> Result<String, String> {
        str_field(block, field)?.ok_or_else(|| format!("package '{name}': missing field '{field}'"))
    };
    Ok(InstalledPkg {
        repo: required("repo")?,
        commit: required("commit")?,
        name,
    })
}

/// A repo or package name: it becomes a directory under `.repo-cache/`
/// or `pkgs/`, so this doubles as path-traversal protection.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A subdir joins under the checkout: relative, no '..', no empty
/// components. "." (the checkout root) is allowed.
pub fn valid_subdir(sub: &str) -> bool {
    sub == "."
        || (!sub.is_empty()
            && !sub.starts_with('/')
            && sub.split('/').all(|c| c != ".." && !c.is_empty()))
}

/// Structural checks shared by load and `pkg repo add`.
pub fn validate_repo(repo: &RepoDef) -> Result<(), String> {
    if !valid_name(&repo.name) {
        return Err("name must be alphanumeric with - _ . and not start with '.'".into());
    }
    if repo.url.is_empty() {
        return Err("url must not be empty".into());
    }
    if let Some(sub) = &repo.subdir
        && !valid_subdir(sub)
    {
        return Err("subdir must be a relative path without '..'".into());
    }
    Ok(())
}

/// Regenerate `pkgs/repo.wcl` from the structs: fresh AST through the
/// canonical printer, written atomically. Creates `pkgs/` if needed.
pub fn save(path: &Path, file: &PkgFile) -> Result<(), Diag> {
    let mut src = ast::Source {
        items: Vec::new(),
        trailing_trivia: Vec::new(),
    };
    for repo in &file.repos {
        let mut fields = vec![("url".into(), edit::string_literal_expr(&repo.url))];
        if let Some(sub) = &repo.subdir {
            fields.push(("subdir".into(), edit::string_literal_expr(sub)));
        }
        if let Some(branch) = &repo.branch {
            fields.push(("branch".into(), edit::string_literal_expr(branch)));
        }
        edit::append_top_level_block(
            &mut src,
            edit::build_block(
                "repo",
                &[],
                vec![edit::string_literal_expr(&repo.name)],
                fields,
            ),
        );
    }
    for pkg in &file.packages {
        edit::append_top_level_block(
            &mut src,
            edit::build_block(
                "package",
                &[],
                vec![edit::string_literal_expr(&pkg.name)],
                vec![
                    ("repo".into(), edit::string_literal_expr(&pkg.repo)),
                    ("commit".into(), edit::string_literal_expr(&pkg.commit)),
                ],
            ),
        );
    }

    let header = [
        " Config Weave package repositories — managed by `config-weave pkg`.",
        " Commands regenerate this file; hand edits to values survive a",
        " reload, comments do not.",
    ];
    match src.items.first_mut() {
        Some(ast::Item::Block(b)) => {
            let mut trivia: Vec<ast::Trivia> = header
                .iter()
                .map(|l| ast::Trivia::LineComment(l.to_string()))
                .collect();
            trivia.push(ast::Trivia::BlankLine);
            trivia.append(&mut b.leading_trivia);
            b.leading_trivia = trivia;
        }
        _ => {
            src.trailing_trivia = header
                .iter()
                .map(|l| ast::Trivia::LineComment(l.to_string()))
                .collect();
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Diag::bare(format!("cannot create {}: {e}", parent.display())))?;
    }
    let rendered = wclformat::to_source(&src);
    let tmp = path.with_extension("wcl.weave-tmp");
    std::fs::write(&tmp, &rendered)
        .map_err(|e| Diag::bare(format!("cannot write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Diag::bare(format!("cannot write {}: {e}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkgrepo::stdlib_default;

    fn sample() -> PkgFile {
        PkgFile {
            repos: vec![
                stdlib_default(),
                RepoDef {
                    name: "corp".into(),
                    url: "git@github.com:corp/pkgs.git".into(),
                    subdir: None,
                    branch: Some("release".into()),
                },
            ],
            packages: vec![InstalledPkg {
                name: "linux_files".into(),
                repo: "stdlib".into(),
                commit: "a".repeat(40),
            }],
        }
    }

    #[test]
    fn save_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pkgs/repo.wcl");
        save(&path, &sample()).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded, sample());
        // The rendered file carries the managed-by header.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("managed by `config-weave pkg`"));
    }

    #[test]
    fn missing_file_is_none_but_empty_file_is_respected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo.wcl");
        assert!(load(&path).unwrap().is_none());
        std::fs::write(&path, "").unwrap();
        assert_eq!(load(&path).unwrap().unwrap(), PkgFile::default());
    }

    #[test]
    fn minimal_hand_written_file_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo.wcl");
        std::fs::write(&path, "repo \"r\" {\n  url = \"https://x/y.git\"\n}\n").unwrap();
        let file = load(&path).unwrap().unwrap();
        assert_eq!(file.repos.len(), 1);
        assert_eq!(file.repos[0].name, "r");
        assert!(file.repos[0].subdir.is_none() && file.repos[0].branch.is_none());
    }

    #[test]
    fn duplicates_and_bad_defs_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo.wcl");
        std::fs::write(
            &path,
            "repo \"r\" { url = \"a\" }\nrepo \"r\" { url = \"b\" }\n",
        )
        .unwrap();
        assert!(load(&path).unwrap_err().rendered.contains("duplicate"));

        std::fs::write(&path, "repo \"r\" { url = \"a\" subdir = \"../up\" }\n").unwrap();
        assert!(load(&path).unwrap_err().rendered.contains("subdir"));

        std::fs::write(
            &path,
            "package \"p\" { repo = \"r\" commit = \"c\" }\npackage \"p\" { repo = \"r\" commit = \"c\" }\n",
        )
        .unwrap();
        assert!(load(&path).unwrap_err().rendered.contains("duplicate"));
    }

    #[test]
    fn missing_required_fields_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo.wcl");
        std::fs::write(&path, "repo \"r\" {}\n").unwrap();
        assert!(load(&path).unwrap_err().rendered.contains("url"));
        std::fs::write(&path, "package \"p\" { repo = \"r\" }\n").unwrap();
        assert!(load(&path).unwrap_err().rendered.contains("commit"));
    }

    #[test]
    fn names_are_validated() {
        assert!(valid_name("linux_files") && valid_name("a-b.c"));
        assert!(!valid_name("") && !valid_name(".hidden") && !valid_name("a/b"));
        assert!(valid_subdir("pkgs") && valid_subdir(".") && valid_subdir("a/b"));
        assert!(!valid_subdir("/abs") && !valid_subdir("a/../b") && !valid_subdir("a//b"));
    }
}
