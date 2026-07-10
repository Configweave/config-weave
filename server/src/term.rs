//! `WS /api/term/docker/{container}` — an interactive shell inside a
//! test container, speaking forge's terminal widget protocol.
//!
//! Unlike forge's generic `/api/term` (which this server deliberately
//! does not mount), the target is pinned by the URL: only containers the
//! run manager saw come up in `instance_ready` events are reachable, and
//! the PTY runs `docker exec` into that container — never a host shell.
//! The client still opens with a `start` frame (`mode: "local"`).

use std::io::Write as _;
use std::sync::Arc;

use axum::Extension;
use axum::extract::{Path as UrlPath, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use forge_core::widgets::TermConfig;
use forge_server::widgets::WsStream;
use forge_server::{RequireClaims, err};

use crate::state::SharedState;

/// Container ids are hex, 12–64 chars; the run manager match is the real
/// authorization, this just keeps garbage out of the shell script.
fn valid_container_id(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Write a tiny wrapper script for the PTY: forge's `TermConfig::shell`
/// takes a single program, so the docker-exec argv lives in a file.
fn write_exec_script(cli: &str, container: &str) -> std::io::Result<tempfile::TempPath> {
    let mut file = tempfile::Builder::new()
        .prefix("weave-term-")
        .suffix(".sh")
        .tempfile()?;
    // Inside the container: prefer bash, fall back to sh.
    writeln!(
        file,
        "#!/bin/sh\nexec {cli} exec -it -w /weave {container} \
         sh -c 'command -v bash >/dev/null 2>&1 && exec bash || exec sh'"
    )?;
    file.flush()?;
    let path = file.into_temp_path();
    let mut perms = std::fs::metadata(&path)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms)?;
    Ok(path)
}

pub async fn docker_term(
    ws: WebSocketUpgrade,
    UrlPath(container): UrlPath<String>,
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
) -> Response {
    if !valid_container_id(&container) {
        return err(StatusCode::BAD_REQUEST, "invalid container id");
    }
    let Some((cli, container_id)) = state.runs.docker_instance(&container) else {
        return err(
            StatusCode::NOT_FOUND,
            "no such test container (only instances of known runs are attachable)",
        );
    };

    ws.on_upgrade(move |socket| async move {
        let script = match write_exec_script(&cli, &container_id) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("cannot prepare the terminal wrapper: {e}");
                return;
            }
        };
        let config = Arc::new(TermConfig {
            shell: Some(script.to_string_lossy().into_owned()),
            allow_local: true,
            allow_ssh: false,
            allow_hosts: None,
        });
        forge_core::widgets::term::session(WsStream(socket), config).await;
        // `script` (TempPath) drops here, removing the wrapper file.
        drop(script);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_ids_are_hex_only() {
        assert!(valid_container_id(&"a1".repeat(12)));
        assert!(!valid_container_id("short"));
        assert!(!valid_container_id("$(rm -rf /)aaaaaaaaaaaa"));
        assert!(!valid_container_id(&"g".repeat(20)));
    }

    #[test]
    fn wrapper_script_is_executable_and_pins_the_container() {
        let path = write_exec_script("docker", &"ab".repeat(16)).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(&format!("docker exec -it -w /weave {}", "ab".repeat(16))));
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o700);
    }
}
