//! The `sys` module (PRD §7): OS and hardware facts. Gatherer fodder —
//! gatherers are ordinary wscript scripts with no special powers.

use wscript::Module;

/// `std::env::consts::FAMILY` is "unix"/"windows"; the PRD's platform
/// conditions want linux/windows/macos.
const fn family() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn module() -> Module {
    let mut m = Module::new("sys");
    m.doc("Operating system and hardware facts");

    m.doc_next("OS family: linux, windows or macos");
    m.fn_("family", || -> String { family().to_string() });
    m.doc_next("OS name (distribution name on Linux)");
    m.fn_("os_name", || -> String {
        sysinfo::System::name().unwrap_or_else(|| std::env::consts::OS.to_string())
    });
    m.doc_next("OS version string");
    m.fn_("os_version", || -> String {
        sysinfo::System::os_version().unwrap_or_default()
    });
    m.doc_next("Kernel version string");
    m.fn_("kernel_version", || -> String {
        sysinfo::System::kernel_version().unwrap_or_default()
    });
    m.doc_next("CPU architecture (x86_64, aarch64, ...)");
    m.fn_("arch", || -> String { std::env::consts::ARCH.to_string() });
    m.doc_next("Number of logical CPUs");
    m.fn_("cpu_count", || -> i64 {
        std::thread::available_parallelism()
            .map(|n| n.get() as i64)
            .unwrap_or(1)
    });
    m.doc_next("Total physical memory in bytes");
    m.fn_("total_memory", || -> i64 {
        let mut s = sysinfo::System::new();
        s.refresh_memory();
        s.total_memory() as i64
    });
    m.doc_next("Available memory in bytes");
    m.fn_("available_memory", || -> i64 {
        let mut s = sysinfo::System::new();
        s.refresh_memory();
        s.available_memory() as i64
    });
    m
}

#[cfg(test)]
mod tests {
    /// Run a script through the real sys module. It returns
    /// "<total> <available> <kernel version>"; the kernel version is last
    /// because it may contain spaces.
    fn facts() -> (i64, i64, String) {
        let ctx = crate::hostapi::context();
        let unit = ctx
            .compile(
                r#"
use sys

fn run(x: int) -> Result[string, string] {
    let t = sys::total_memory()
    let a = sys::available_memory()
    let k = sys::kernel_version()
    Ok("{t} {a} {k}")
}
"#,
            )
            .unwrap_or_else(|e| panic!("compile: {e}"));
        let mut vm = crate::hostapi::vm(&ctx);
        let out: Result<String, String> = vm.call_unit(&unit, "run", (0i64,)).unwrap();
        let out = out.unwrap_or_else(|e| panic!("script error: {e}"));
        let mut parts = out.splitn(3, ' ');
        let total = parts.next().unwrap().parse().unwrap();
        let available = parts.next().unwrap().parse().unwrap();
        (total, available, parts.next().unwrap_or("").to_string())
    }

    #[test]
    fn memory_is_positive_and_ordered() {
        let (total, available, _) = facts();
        assert!(available > 0, "available_memory was {available}");
        assert!(
            total >= available,
            "total {total} is below available {available}"
        );
    }

    #[test]
    fn family_and_arch_are_known_values() {
        let ctx = crate::hostapi::context();
        let unit = ctx
            .compile(
                r#"
use sys

fn run(x: int) -> Result[string, string] {
    let f = sys::family()
    let a = sys::arch()
    Ok("{f} {a}")
}
"#,
            )
            .unwrap_or_else(|e| panic!("compile: {e}"));
        let mut vm = crate::hostapi::vm(&ctx);
        let out: Result<String, String> = vm.call_unit(&unit, "run", (0i64,)).unwrap();
        let out = out.unwrap();
        let family = super::family();
        assert!(["linux", "macos", "windows"].contains(&family), "{family}");
        assert_eq!(out, format!("{family} {}", std::env::consts::ARCH));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_version_is_set_on_linux() {
        let (_, _, kernel) = facts();
        assert!(!kernel.is_empty(), "kernel_version is empty");
    }
}
