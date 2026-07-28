//! The built-in `weave` package: resources config-weave ships itself.
//!
//! Its `package.wcl` and scripts are compiled into the binary and loaded
//! through exactly the same path as a package on disk — so `validate`,
//! `docs`, `list` and the run path need no special case beyond knowing
//! that a script may come from memory rather than a file.
//!
//! The package name is reserved: a `pkgs/weave/` folder is rejected, or a
//! playbook could quietly shadow `weave.execute` with something else.

/// Reserved package name.
pub const PACKAGE: &str = "weave";

/// The embedded `package.wcl`, loaded like any other package manifest.
pub const PACKAGE_WCL: &str = include_str!("builtin/package.wcl");

/// Display path for the manifest in diagnostics. Not a real path — the
/// angle brackets say so, matching how WCL names its system imports.
pub const PACKAGE_PATH: &str = "<weave>/package.wcl";

/// Every embedded script, by the name a `script = "…"` field or a `use`
/// import gives it. The resolver consults this before the filesystem, so
/// the shared helper resolves for the built-ins and nowhere else.
pub const SCRIPTS: &[(&str, &str)] = &[
    ("execute.ws", include_str!("builtin/execute.ws")),
    ("execute_once.ws", include_str!("builtin/execute_once.ws")),
    ("lib.ws", include_str!("builtin/lib.ws")),
];

/// The embedded source registered under `name`, if there is one.
pub fn script(name: &str) -> Option<&'static str> {
    SCRIPTS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}
