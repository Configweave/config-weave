//! The embedded WCL vocabulary for playbooks and packages.
//!
//! Config Weave ships its schema as WCL system imports (the same mechanism
//! wdoc uses for its stdlib). The engine appends the appropriate
//! `import <weave/…>` line to user sources when opening them, so playbook
//! authors never write the import themselves and all spans in user files
//! stay untouched (the import is appended at the *end* of the source).

use wcl_lang::{FileLoader, Registry, disk_loader};

/// Schema for `playbook.wcl`.
pub const PLAYBOOK_VOCAB: &str = include_str!("vocab/playbook.wcl");

/// Schema for `package.wcl`.
pub const PACKAGE_VOCAB: &str = include_str!("vocab/package.wcl");

/// Schema for `services.wcl` (weave-server's service inventory; the CLI
/// only serves the vocab so there is one source of truth).
pub const SERVICES_VOCAB: &str = include_str!("vocab/services.wcl");

/// Schema for `repos.wcl` (weave-server's remote package repositories).
pub const REPOS_VOCAB: &str = include_str!("vocab/repos.wcl");

/// Schema for `pipeline.wcl` (the config-weave-pipeline daemon; the CLI
/// only serves the vocab so there is one source of truth).
pub const PIPELINE_VOCAB: &str = include_str!("vocab/pipeline.wcl");

/// Registry-relative names the engine appends as imports.
pub const PLAYBOOK_IMPORT: &str = "weave/playbook.wcl";
pub const PACKAGE_IMPORT: &str = "weave/package.wcl";
pub const VARS_IMPORT: &str = "weave/vars.wcl";
pub const SERVICES_IMPORT: &str = "weave/services.wcl";
pub const REPOS_IMPORT: &str = "weave/repos.wcl";
pub const PIPELINE_IMPORT: &str = "weave/pipeline.wcl";

/// Build the system-import loader. `vars` is the generated variables file
/// (gatherer results and overrides as `let` declarations); pass `None`
/// during validation, where no variables are bound.
pub fn loader(vars: Option<String>) -> FileLoader {
    let mut reg = Registry::new();
    reg.register(PLAYBOOK_IMPORT, PLAYBOOK_VOCAB);
    reg.register(PACKAGE_IMPORT, PACKAGE_VOCAB);
    reg.register(SERVICES_IMPORT, SERVICES_VOCAB);
    reg.register(REPOS_IMPORT, REPOS_VOCAB);
    reg.register(PIPELINE_IMPORT, PIPELINE_VOCAB);
    if let Some(v) = vars {
        reg.register(VARS_IMPORT, v);
    }
    reg.loader(disk_loader())
}

/// Append a system import to a user source without disturbing its spans.
pub fn with_import(source: &str, import: &str, vars: bool) -> String {
    let mut s = String::with_capacity(source.len() + 64);
    s.push_str(source);
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&format!("import <{import}>\n"));
    if vars {
        s.push_str(&format!("import <{VARS_IMPORT}>\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_import_leaves_the_source_prefix_untouched() {
        // Spans in user files stay valid because the source is an
        // unchanged prefix of the augmented text.
        let src = "play \"demo\" {\n}\n";
        let out = with_import(src, PLAYBOOK_IMPORT, false);
        assert!(out.starts_with(src));
        assert_eq!(out, format!("{src}import <{PLAYBOOK_IMPORT}>\n"));
    }

    #[test]
    fn with_import_terminates_an_unterminated_source() {
        let out = with_import("play \"demo\" {}", PACKAGE_IMPORT, false);
        assert_eq!(
            out,
            format!("play \"demo\" {{}}\nimport <{PACKAGE_IMPORT}>\n")
        );
    }

    #[test]
    fn with_import_appends_the_vars_import_when_asked() {
        let out = with_import("x\n", PLAYBOOK_IMPORT, true);
        assert!(out.ends_with(&format!(
            "import <{PLAYBOOK_IMPORT}>\nimport <{VARS_IMPORT}>\n"
        )));
    }

    #[test]
    fn loader_serves_the_embedded_vocab_and_optional_vars() {
        let sys = |name: &str| std::path::Path::new(wcl_lang::SYSTEM_IMPORT_ROOT).join(name);
        let l = loader(Some("let x = 1\n".into()));
        assert_eq!(l(&sys(PLAYBOOK_IMPORT)).unwrap(), PLAYBOOK_VOCAB);
        assert_eq!(l(&sys(PACKAGE_IMPORT)).unwrap(), PACKAGE_VOCAB);
        assert_eq!(l(&sys(SERVICES_IMPORT)).unwrap(), SERVICES_VOCAB);
        assert_eq!(l(&sys(REPOS_IMPORT)).unwrap(), REPOS_VOCAB);
        assert_eq!(l(&sys(PIPELINE_IMPORT)).unwrap(), PIPELINE_VOCAB);
        assert_eq!(l(&sys(VARS_IMPORT)).unwrap(), "let x = 1\n");
        let no_vars = loader(None);
        assert!(no_vars(&sys(VARS_IMPORT)).is_err());
    }
}
