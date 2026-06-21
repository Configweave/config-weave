//! M3 gate: a bootstrap playbook downloads, verifies, extracts and
//! installs something real — hermetically, against a local HTTP server.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::Digest;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_config-weave"))
}

/// Build a tar.gz containing tool/hello.sh, returning (bytes, sha256 hex).
fn make_artifact() -> (Vec<u8>, String) {
    let mut tar_bytes = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        let content = b"#!/bin/sh\necho hello from the installed tool\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "tool/hello.sh", content.as_slice())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    let digest = sha2::Sha256::digest(&tar_bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    (tar_bytes, hex)
}

/// Minimal single-shot HTTP server for the artifact.
fn serve(artifact: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        // Serve a handful of requests then exit (idempotence reruns).
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/gzip\r\nconnection: close\r\n\r\n",
                    artifact.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(&artifact);
        }
    });
    (format!("http://{addr}/tool.tar.gz"), handle)
}

fn write_playbook(root: &Path, url: &str, sha: &str, install_dir: &Path) {
    let pkg = root.join("pkgs/boot");
    std::fs::create_dir_all(pkg.join("resources")).unwrap();
    std::fs::write(
        root.join("playbook.wcl"),
        format!(
            r#"playbook "Bootstrap" {{
  description = "Download, verify, extract, install"
  version = "0.1.0"

  vars {{
    tool_url = "{url}"
    tool_sha256 = "{sha}"
    tool_dir = "{install}"
  }}

  play "bootstrap" {{
    description = "Install the tool"

    step "install-tool" {{
      description = "Fetch and install the tool"
      resource = "boot.fetch_install"
      properties {{
        url = tool_url
        sha256 = tool_sha256
        install_dir = tool_dir
      }}
    }}
  }}
}}
"#,
            install = install_dir.display()
        ),
    )
    .unwrap();
    std::fs::write(
        pkg.join("package.wcl"),
        r#"package "boot" {
  description = "Bootstrap installer"

  resource "fetch_install" {
    description = "Download a tar.gz, verify its sha256, extract and install"
    script = "resources/fetch_install.wscript"

    param "url" {
      description = "Artifact URL"
      type = "string"
      required = true
    }
    param "sha256" {
      description = "Expected sha256 of the artifact"
      type = "string"
      required = true
    }
    param "install_dir" {
      description = "Installation directory"
      type = "string"
      required = true
    }
  }
}
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("resources/fetch_install.wscript"),
        r#"use value
use fs
use path
use http
use hash
use archive
use shell
use log

fn p(params: Value, key: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    ""
}

fn installed_marker(params: Value) -> string {
    path::join(p(params, "install_dir"), "tool/hello.sh")
}

fn check(params: Value) -> CheckResult {
    if fs::exists(installed_marker(params)) {
        CheckResult::AlreadyConfigured
    } else {
        CheckResult::NotConfigured
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let dest = fs::temp_file()?
    let bytes = http::download(p(params, "url"), dest, Value::Null)?
    log::info(fmt("downloaded {} bytes", bytes))

    let actual = hash::sha256_file(dest)?
    if actual != p(params, "sha256") {
        return Err(fmt("sha256 mismatch: expected {}, got {}", p(params, "sha256"), actual))
    }
    log::info("checksum verified")

    let dir = p(params, "install_dir")
    fs::mkdir(dir)?
    let entries = archive::extract_tar_gz(dest, dir)?
    log::info(fmt("extracted {} entries", entries))
    fs::delete(dest)?

    // Prove the installed tool actually runs.
    let out = shell::bash(fmt("sh {}", installed_marker(params)), Value::Null)?
    if !out.success {
        return Err(fmt("installed tool failed: {}", out.stderr))
    }
    print("tool says: " + out.stdout)
    Ok(ApplyResult::Success)
}
"#,
    )
    .unwrap();
}

#[test]
fn bootstrap_downloads_verifies_extracts_installs() {
    let (artifact, sha) = make_artifact();
    let (url, _server) = serve(artifact);

    let dir = tempfile::tempdir().unwrap();
    let install_dir = dir.path().join("opt");
    write_playbook(dir.path(), &url, &sha, &install_dir);

    let out = Command::new(bin())
        .args(["apply", ".", "bootstrap"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("[         configured]"), "{stdout}");
    assert!(install_dir.join("tool/hello.sh").exists());
    // Script logs (including redirected print) land on stderr, not stdout.
    assert!(stderr.contains("checksum verified"), "{stderr}");
    assert!(stderr.contains("tool says"), "{stderr}");

    // Re-apply: already configured, nothing downloaded again.
    let out = Command::new(bin())
        .args(["apply", ".", "bootstrap"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout.contains("already configured"), "{stdout}");
}

#[test]
fn bad_checksum_fails_step() {
    let (artifact, _) = make_artifact();
    let (url, _server) = serve(artifact);

    let dir = tempfile::tempdir().unwrap();
    let install_dir = dir.path().join("opt");
    write_playbook(dir.path(), &url, "deadbeef", &install_dir);

    let out = Command::new(bin())
        .args(["apply", ".", "bootstrap"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("sha256 mismatch"), "{stdout}");
    assert!(!install_dir.join("tool/hello.sh").exists());
}
