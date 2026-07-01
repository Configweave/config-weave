//! The `service` module (PRD §7): query/start/stop services and set the
//! startup type, via the Service Control Manager. Windows-only in v1;
//! Linux uses `shell::run("systemctl …")`. Registered on every platform;
//! calls fail at runtime off Windows.

use wscript::Module;

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the 'service' module is only available on Windows (v1); use \
                           shell::run(\"systemctl ...\") on Linux";

pub fn module() -> Module {
    let mut m = Module::new("service");
    m.doc("Windows service management via the SCM (Windows only in v1)");

    m.doc_next(
        "Service status: running | stopped | start_pending | stop_pending | paused | \
         pause_pending | continue_pending",
    );
    #[cfg(windows)]
    m.fn_("status", |name: &str| -> Result<String, String> {
        win::status(name)
    });
    #[cfg(not(windows))]
    m.fn_("status", |_: &str| -> Result<String, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Start a service (no-op when already running)");
    #[cfg(windows)]
    m.fn_("start", |name: &str| -> Result<(), String> {
        win::start(name)
    });
    #[cfg(not(windows))]
    m.fn_("start", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Stop a service (no-op when already stopped)");
    #[cfg(windows)]
    m.fn_("stop", |name: &str| -> Result<(), String> {
        win::stop(name)
    });
    #[cfg(not(windows))]
    m.fn_("stop", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Set startup type: automatic | manual | disabled");
    #[cfg(windows)]
    m.fn_(
        "set_startup",
        |name: &str, mode: &str| -> Result<(), String> { win::set_startup(name, mode) },
    );
    #[cfg(not(windows))]
    m.fn_("set_startup", |_: &str, _: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Startup type of a service: automatic | manual | disabled");
    #[cfg(windows)]
    m.fn_("startup", |name: &str| -> Result<String, String> {
        win::startup(name)
    });
    #[cfg(not(windows))]
    m.fn_("startup", |_: &str| -> Result<String, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Services::{
        ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_TYPE,
        OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
        QueryServiceStatus, SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_AUTO_START,
        SERVICE_CHANGE_CONFIG, SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_STOP,
        SERVICE_DEMAND_START, SERVICE_DISABLED, SERVICE_ERROR, SERVICE_NO_CHANGE,
        SERVICE_PAUSE_PENDING, SERVICE_PAUSED, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS,
        SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_START_TYPE, SERVICE_STATUS,
        SERVICE_STOP, SERVICE_STOP_PENDING, SERVICE_STOPPED, StartServiceW,
    };
    use windows::core::{HSTRING, PCWSTR};

    struct Handle(SC_HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }

    fn open(name: &str, access: u32) -> Result<(Handle, Handle), String> {
        unsafe {
            let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
                .map_err(|e| format!("opening the service control manager: {e}"))?;
            let scm = Handle(scm);
            let wide = HSTRING::from(name);
            let svc = OpenServiceW(scm.0, PCWSTR(wide.as_ptr()), access)
                .map_err(|e| format!("opening service '{name}': {e}"))?;
            Ok((scm, Handle(svc)))
        }
    }

    pub fn status(name: &str) -> Result<String, String> {
        let (_scm, svc) = open(name, SERVICE_QUERY_STATUS)?;
        let mut st = SERVICE_STATUS::default();
        unsafe {
            QueryServiceStatus(svc.0, &mut st).map_err(|e| format!("querying '{name}': {e}"))?;
        }
        Ok(match st.dwCurrentState {
            SERVICE_STOPPED => "stopped",
            SERVICE_START_PENDING => "start_pending",
            SERVICE_STOP_PENDING => "stop_pending",
            SERVICE_RUNNING => "running",
            SERVICE_CONTINUE_PENDING => "continue_pending",
            SERVICE_PAUSE_PENDING => "pause_pending",
            SERVICE_PAUSED => "paused",
            _ => "unknown",
        }
        .to_string())
    }

    pub fn start(name: &str) -> Result<(), String> {
        if status(name)? == "running" {
            return Ok(());
        }
        let (_scm, svc) = open(name, SERVICE_START)?;
        unsafe { StartServiceW(svc.0, None).map_err(|e| format!("starting '{name}': {e}")) }
    }

    pub fn stop(name: &str) -> Result<(), String> {
        if status(name)? == "stopped" {
            return Ok(());
        }
        let (_scm, svc) = open(name, SERVICE_STOP)?;
        let mut st = SERVICE_STATUS::default();
        unsafe {
            ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st)
                .map_err(|e| format!("stopping '{name}': {e}"))
        }
    }

    fn start_type(mode: &str) -> Result<SERVICE_START_TYPE, String> {
        Ok(match mode {
            "automatic" => SERVICE_AUTO_START,
            "manual" => SERVICE_DEMAND_START,
            "disabled" => SERVICE_DISABLED,
            other => {
                return Err(format!(
                    "unknown startup type '{other}' (automatic, manual, disabled)"
                ));
            }
        })
    }

    pub fn set_startup(name: &str, mode: &str) -> Result<(), String> {
        let ty = start_type(mode)?;
        let (_scm, svc) = open(name, SERVICE_CHANGE_CONFIG)?;
        unsafe {
            ChangeServiceConfigW(
                svc.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                ty,
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
            .map_err(|e| format!("configuring '{name}': {e}"))
        }
    }

    pub fn startup(name: &str) -> Result<String, String> {
        let (_scm, svc) = open(name, SERVICE_QUERY_CONFIG)?;
        let mut needed = 0u32;
        unsafe {
            // First call sizes the buffer.
            let _ = QueryServiceConfigW(svc.0, None, 0, &mut needed);
            let mut buf = vec![0u8; needed as usize];
            let config = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            QueryServiceConfigW(svc.0, Some(config), needed, &mut needed)
                .map_err(|e| format!("querying config of '{name}': {e}"))?;
            Ok(match (*config).dwStartType {
                SERVICE_AUTO_START => "automatic",
                SERVICE_DEMAND_START => "manual",
                SERVICE_DISABLED => "disabled",
                _ => "other",
            }
            .to_string())
        }
    }
}
