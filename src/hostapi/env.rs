//! The `env` module (PRD §7): process environment, PATH-style helpers,
//! identity (hostname, user, home) and elevation.

use wisp::Module;

fn is_elevated_impl() -> bool {
    #[cfg(unix)]
    {
        // Effective UID 0 == root.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(windows)]
    {
        super::windows_impl::is_elevated()
    }
}

pub fn module() -> Module {
    let mut m = Module::new("env");
    m.doc("Process environment and host identity");

    m.doc_next("Read an environment variable (None when unset)");
    m.fn_("get", |name: &str| -> Option<String> {
        std::env::var(name).ok()
    });
    m.doc_next("Set an environment variable for this process and its children");
    m.fn_("set", |name: &str, value: &str| {
        // Safety: scripts run one per worker thread but env mutation is
        // process-global; this mirrors what shell-based config tools do.
        unsafe { std::env::set_var(name, value) };
    });
    m.doc_next("Remove an environment variable from this process");
    m.fn_("unset", |name: &str| {
        unsafe { std::env::remove_var(name) };
    });
    m.doc_next("Split a PATH-style list on the platform separator");
    m.fn_("path_split", |value: &str| -> Vec<String> {
        std::env::split_paths(value)
            .map(|p| p.display().to_string())
            .collect()
    });
    m.doc_next("Join paths into a PATH-style list with the platform separator");
    m.fn_(
        "path_join",
        |parts: Vec<String>| -> Result<String, String> {
            std::env::join_paths(parts.iter())
                .map(|s| s.to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        },
    );
    m.doc_next("Hostname of this machine");
    m.fn_("hostname", || -> String {
        sysinfo::System::host_name().unwrap_or_default()
    });
    m.doc_next("Name of the current user");
    m.fn_("current_user", || -> String {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default()
    });
    m.doc_next("Home directory of the current user");
    m.fn_("home_dir", || -> String {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default()
    });
    m.doc_next("Whether the process runs elevated (root / Administrator)");
    m.fn_("is_elevated", is_elevated_impl);
    m
}
