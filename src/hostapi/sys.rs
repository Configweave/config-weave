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
