//! The `env` module (PRD §7): process environment, PATH-style helpers,
//! identity (hostname, user, home) and elevation.

use wscript::Module;

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

/// Split a PATH-style list on the platform separator.
fn path_split(value: &str) -> Vec<String> {
    std::env::split_paths(value)
        .map(|p| p.display().to_string())
        .collect()
}

/// Join paths into a PATH-style list with the platform separator.
fn path_join(parts: Vec<String>) -> Result<String, String> {
    std::env::join_paths(parts.iter())
        .map(|s| s.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

/// An environment variable, or None when it is unset or empty.
fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The name and home directory of the effective user, from the passwd
/// database. Unix only.
#[cfg(unix)]
fn passwd_entry() -> Option<(String, String)> {
    use std::ffi::CStr;

    // Safety: getpwuid_r writes only into `pwd` and `buf`, which live for the
    // whole call. The strings read from `pwd` are copied before return.
    unsafe {
        let uid = libc::geteuid();
        let mut buf = vec![0u8; 1024];
        loop {
            let mut pwd: libc::passwd = std::mem::zeroed();
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let rc = libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            );
            if rc == libc::ERANGE && buf.len() < (1 << 20) {
                buf.resize(buf.len() * 2, 0);
                continue;
            }
            if rc != 0 || result.is_null() {
                return None;
            }
            let name = CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned();
            let dir = CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned();
            return Some((name, dir));
        }
    }
}

#[cfg(not(unix))]
fn passwd_entry() -> Option<(String, String)> {
    None
}

/// Name of the current user: `$USER`, then `$USERNAME`, then the passwd
/// entry of the effective uid. Empty when none of them has a value.
fn current_user() -> String {
    env_value("USER")
        .or_else(|| env_value("USERNAME"))
        .or_else(|| passwd_entry().map(|(name, _)| name))
        .unwrap_or_default()
}

/// Home directory: `$HOME`, then `$USERPROFILE`, then the passwd entry of the
/// effective uid. Empty when none of them has a value.
fn home_dir() -> String {
    env_value("HOME")
        .or_else(|| env_value("USERPROFILE"))
        .or_else(|| passwd_entry().map(|(_, dir)| dir))
        .unwrap_or_default()
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
        // Safety: set_var requires that no other thread reads or writes the
        // environment concurrently. Worker threads can race here (and with
        // getenv in libc/std), so a torn read is theoretically possible;
        // accepted because env mutation is a rare, script-authored action and
        // this mirrors what shell-based config tools do.
        unsafe { std::env::set_var(name, value) };
    });
    m.doc_next("Remove an environment variable from this process");
    m.fn_("unset", |name: &str| {
        // Safety: same precondition and rationale as `set` above.
        unsafe { std::env::remove_var(name) };
    });
    m.doc_next("Split a PATH-style list on the platform separator");
    m.fn_("path_split", path_split);
    m.doc_next("Join paths into a PATH-style list with the platform separator");
    m.fn_("path_join", path_join);
    m.doc_next("Hostname of this machine");
    m.fn_("hostname", || -> String {
        sysinfo::System::host_name().unwrap_or_default()
    });
    m.doc_next("Name of the current user");
    m.fn_("current_user", current_user);
    m.doc_next("Home directory of the current user");
    m.fn_("home_dir", home_dir);
    m.doc_next("Whether the process runs elevated (root / Administrator)");
    m.fn_("is_elevated", is_elevated_impl);
    m
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    const PROBE: &str = "CW_ENV_PROBE";

    /// Child-process body. It runs only when the parent sets `CW_ENV_PROBE`,
    /// so a plain `cargo test` run skips it. It prints the two values.
    #[test]
    fn probe_identity() {
        if std::env::var_os(PROBE).is_none() {
            return;
        }
        println!("CW_PROBE_USER={}", super::current_user());
        println!("CW_PROBE_HOME={}", super::home_dir());
    }

    /// Run `probe_identity` in a child process. The child's environment is
    /// the parent's minus `remove`, plus `set`. The parent env stays intact.
    fn probe(remove: &[&str], set: &[(&str, &str)]) -> (String, String) {
        let mut cmd = Command::new(std::env::current_exe().expect("test binary path"));
        cmd.args([
            "env::tests::probe_identity",
            "--nocapture",
            "--test-threads=1",
        ]);
        cmd.env(PROBE, "1");
        for name in remove {
            cmd.env_remove(name);
        }
        for (name, value) in set {
            cmd.env(name, value);
        }
        let out = cmd.output().expect("run child test binary");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "child failed: {stdout}");
        // libtest prints "test <name> ... " before the child's own output,
        // so the key is searched anywhere on the line.
        let field = |key: &str| -> String {
            let at = stdout
                .find(key)
                .unwrap_or_else(|| panic!("no {key} in child output: {stdout}"));
            stdout[at + key.len()..]
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        };
        (field("CW_PROBE_USER="), field("CW_PROBE_HOME="))
    }

    /// The passwd facts as the OS reports them, read with `id` and `getent`
    /// so the test does not reuse the code under test.
    fn passwd_facts() -> (String, String) {
        let name = String::from_utf8(
            Command::new("id")
                .arg("-un")
                .output()
                .expect("run id")
                .stdout,
        )
        .expect("utf8")
        .trim()
        .to_string();
        let entry = Command::new("getent")
            .args(["passwd", &name])
            .output()
            .expect("run getent");
        let line = String::from_utf8(entry.stdout).expect("utf8");
        let home = line.trim().split(':').nth(5).unwrap_or("").to_string();
        (name, home)
    }

    #[test]
    fn current_user_and_home_fall_back_to_passwd_without_env() {
        let (want_user, want_home) = passwd_facts();
        assert!(!want_user.is_empty(), "id -un gave no name");
        assert!(!want_home.is_empty(), "getent gave no home");

        let (user, home) = probe(&["USER", "USERNAME", "HOME", "USERPROFILE"], &[]);
        assert_eq!(user, want_user, "current_user with USER and USERNAME unset");
        assert_eq!(home, want_home, "home_dir with HOME and USERPROFILE unset");
    }

    #[test]
    fn empty_env_values_count_as_unset() {
        let (want_user, want_home) = passwd_facts();
        let (user, home) = probe(&["USERNAME", "USERPROFILE"], &[("USER", ""), ("HOME", "")]);
        assert_eq!(user, want_user, "current_user with USER=\"\"");
        assert_eq!(home, want_home, "home_dir with HOME=\"\"");
    }

    #[test]
    fn path_list_round_trip() {
        let parts = vec![
            "/usr/local/bin".to_string(),
            "/opt/with space".to_string(),
            "relative/dir".to_string(),
        ];
        let joined = super::path_join(parts.clone()).expect("join");
        assert_eq!(super::path_split(&joined), parts);
    }

    #[test]
    fn set_env_values_still_win() {
        let (user, home) = probe(
            &[],
            &[("USER", "alice-test"), ("HOME", "/tmp/alice-test-home")],
        );
        assert_eq!(user, "alice-test");
        assert_eq!(home, "/tmp/alice-test-home");
    }
}
