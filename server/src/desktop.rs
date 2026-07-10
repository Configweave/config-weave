//! `WS /api/desktop/vnc/{run}/{machine}` — the console of a vmlab-backed
//! test VM, speaking forge's desktop widget protocol. The target is
//! pinned by the URL (the client's `connect` frame host/port are
//! ignored): the run manager maps run + machine to the lab name, and the
//! VM's QEMU VNC unix socket is resolved by vmlab's path convention —
//! `$XDG_RUNTIME_DIR/vmlab/labs/<lab>/vms/<vm>/vnc.sock`, falling back
//! to `/tmp/vmlab-<uid>` (mirrors vmlab/src/paths.rs; no vmlab dep).

use std::path::PathBuf;

use axum::Extension;
use axum::extract::{Path as UrlPath, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use forge_core::widgets::vnc::session_over;
use forge_server::widgets::WsStream;
use forge_server::{RequireClaims, err};

use crate::state::SharedState;

/// vmlab's runtime dir convention (paths.rs `runtime_dir`).
fn vmlab_runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("vmlab"),
        // SAFETY-free libc call: effective uid for the tmp fallback.
        None => PathBuf::from(format!("/tmp/vmlab-{}", unsafe { libc_geteuid() })),
    }
}

/// Minimal `geteuid` shim so we don't pull the libc crate for one call.
unsafe fn libc_geteuid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

/// The VM's VNC socket per vmlab's layout.
fn vnc_socket(lab: &str, vm: &str) -> PathBuf {
    vmlab_runtime_dir()
        .join("labs")
        .join(lab)
        .join("vms")
        .join(vm)
        .join("vnc.sock")
}

pub async fn vnc(
    ws: WebSocketUpgrade,
    UrlPath((run_id, machine)): UrlPath<(String, String)>,
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
) -> Response {
    let Some((lab, vm)) = state.runs.vmlab_instance(&run_id, &machine) else {
        return err(
            StatusCode::NOT_FOUND,
            "no such vmlab instance in that run (VNC attaches only to known test VMs)",
        );
    };
    let sock = vnc_socket(&lab, &vm);
    if !sock.exists() {
        return err(
            StatusCode::CONFLICT,
            format!("{lab}/{vm} has no VNC socket (powered off?)"),
        );
    }

    ws.on_upgrade(move |socket| async move {
        let mut stream = WsStream(socket);
        match tokio::net::UnixStream::connect(&sock).await {
            // QEMU's vnc.sock does an auth-less RFB handshake — no password.
            Ok(unix) => session_over(stream, unix, None).await,
            Err(e) => {
                // Protocol-shaped failure: an error frame, then close.
                use forge_core::widgets::proto::DesktopServerMsg;
                use forge_core::widgets::{WidgetMsg, WidgetStream as _};
                let msg = DesktopServerMsg::Error {
                    message: format!("cannot open VNC socket: {e}"),
                };
                let text = serde_json::to_string(&msg).expect("DesktopServerMsg serializes");
                let _ = stream.send(WidgetMsg::Text(text)).await;
                let _ = stream.send(WidgetMsg::Close).await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_follows_vmlab_convention() {
        // With XDG_RUNTIME_DIR set (the normal case) the path is under it.
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            let expected = PathBuf::from(runtime).join("vmlab/labs/cw-test-x/vms/box/vnc.sock");
            assert_eq!(vnc_socket("cw-test-x", "box"), expected);
        } else {
            let path = vnc_socket("cw-test-x", "box");
            assert!(path.starts_with("/tmp"));
            assert!(path.ends_with("labs/cw-test-x/vms/box/vnc.sock"));
        }
    }
}
