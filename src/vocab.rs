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

/// Registry-relative names the engine appends as imports.
pub const PLAYBOOK_IMPORT: &str = "weave/playbook.wcl";
pub const PACKAGE_IMPORT: &str = "weave/package.wcl";
pub const VARS_IMPORT: &str = "weave/vars.wcl";

/// Build the system-import loader. `vars` is the generated variables file
/// (gatherer results and overrides as `let` declarations); pass `None`
/// during validation, where no variables are bound.
pub fn loader(vars: Option<String>) -> FileLoader {
    let mut reg = Registry::new();
    reg.register(PLAYBOOK_IMPORT, PLAYBOOK_VOCAB);
    reg.register(PACKAGE_IMPORT, PACKAGE_VOCAB);
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
