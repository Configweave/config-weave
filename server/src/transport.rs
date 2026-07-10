//! Remote transports for direct systems: copy the config-weave binary +
//! playbook to the target and run check/apply there. The API mirrors the
//! testlab's `TestInstance` seam (copy_in/exec) but is async and speaks
//! ssh (openssh client; Linux and Win32-OpenSSH targets) or winrm
//! (shelling out to `pwsh` PSRemoting; requires pwsh + PSWSMan on the
//! server host).
//!
//! Remote commands are structured (`ExecSpec`: program + args + stdout
//! redirect), never free-form shell strings — each transport renders
//! them for the target's shell itself, and Windows payloads go through
//! `powershell -EncodedCommand` so quoting can never break. The run
//! protocol always redirects remote stdout to `<stage>/report.json` and
//! fetches it afterwards, sidestepping stream-separation quirks; the
//! live NDJSON events arrive on the streamed remaining output.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use tokio::io::{AsyncBufReadExt as _, BufReader};

use crate::systems::{SystemDef, TargetOs, TransportConfig, TransportKind};

/// Directories never staged to a target (same set the runbook tree hides).
const EXCLUDED_DIRS: [&str; 4] = [".git", "node_modules", "target", ".vmlab"];

/// A remote command: absolute program path, plain args, optional remote
/// file that receives the program's stdout.
pub struct ExecSpec {
    pub program: String,
    pub args: Vec<String>,
    pub stdout_to: Option<String>,
}

/// The staging directory on a target for one run.
pub fn stage_dir(os: TargetOs, run_id: &str) -> String {
    match os {
        TargetOs::Linux => format!("/tmp/weave-run-{run_id}"),
        TargetOs::Windows => format!("C:/Windows/Temp/weave-run-{run_id}"),
    }
}

pub enum Transport {
    Ssh(SshTransport),
    Winrm(WinrmTransport),
}

impl Transport {
    pub fn for_system(sys: &SystemDef) -> Result<Transport, String> {
        match sys.transport.kind {
            TransportKind::Ssh => Ok(Transport::Ssh(SshTransport::new(&sys.transport, sys.os)?)),
            TransportKind::Winrm => Ok(Transport::Winrm(WinrmTransport::new(&sys.transport)?)),
        }
    }

    /// Cheap connectivity + prerequisites check with a clear diagnostic.
    pub async fn probe(&self) -> Result<(), String> {
        match self {
            Transport::Ssh(t) => t.probe().await,
            Transport::Winrm(t) => t.probe().await,
        }
    }

    pub async fn mkdir(&self, dir: &str) -> Result<(), String> {
        match self {
            Transport::Ssh(t) => t.mkdir(dir).await,
            Transport::Winrm(t) => t.mkdir(dir).await,
        }
    }

    /// Copy one local file to an absolute remote path (parent must exist).
    pub async fn copy_file_in(&self, src: &Path, dest: &str, exec_bit: bool) -> Result<(), String> {
        match self {
            Transport::Ssh(t) => t.copy_file_in(src, dest, exec_bit).await,
            Transport::Winrm(t) => t.copy_file_in(src, dest).await,
        }
    }

    /// Copy a local directory tree to an absolute remote path, excluding
    /// the usual junk dirs.
    pub async fn copy_dir_in(&self, src: &Path, dest: &str) -> Result<(), String> {
        // Both transports ship a filtered staging copy: tar has excludes,
        // but scp -r and Copy-Item -Recurse do not.
        let staged = tempfile::tempdir().map_err(|e| format!("cannot stage: {e}"))?;
        let staged_root = staged.path().join("d");
        copy_dir_filtered(src, &staged_root).map_err(|e| format!("cannot stage: {e}"))?;
        match self {
            Transport::Ssh(t) => t.copy_dir_in(&staged_root, dest).await,
            Transport::Winrm(t) => t.copy_dir_in(&staged_root, dest).await,
        }
    }

    /// Run `spec`, feeding every output line (merged streams) to
    /// `on_line`; returns the remote exit code. `cancel` kills the local
    /// transport process (remote survival is best-effort, documented).
    pub async fn exec_stream(
        &self,
        spec: &ExecSpec,
        on_line: &mut (dyn FnMut(String) + Send),
        cancel: &tokio::sync::Notify,
    ) -> Result<i32, String> {
        let mut cmd = match self {
            Transport::Ssh(t) => t.exec_command(spec),
            Transport::Winrm(t) => t.exec_command(spec)?,
        };
        stream_child(&mut cmd, on_line, cancel).await
    }

    /// Read a remote text file (the fetched report.json).
    pub async fn fetch_file(&self, path: &str) -> Result<String, String> {
        match self {
            Transport::Ssh(t) => t.fetch_file(path).await,
            Transport::Winrm(t) => t.fetch_file(path).await,
        }
    }

    /// Best-effort recursive removal of the staging dir.
    pub async fn remove_dir(&self, dir: &str) -> Result<(), String> {
        match self {
            Transport::Ssh(t) => t.remove_dir(dir).await,
            Transport::Winrm(t) => t.remove_dir(dir).await,
        }
    }
}

// ------------------------------------------------------------------ ssh

pub struct SshTransport {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    key_path: Option<PathBuf>,
    os: TargetOs,
    /// Keeps an inline-PEM key file alive (0600 tempfile).
    _key_temp: Option<tempfile::NamedTempFile>,
}

impl SshTransport {
    fn new(t: &TransportConfig, os: TargetOs) -> Result<SshTransport, String> {
        let (key_path, key_temp) = match &t.private_key {
            None => (None, None),
            Some(k) if k.contains("-----BEGIN") => {
                // Inline PEM: write to a 0600 tempfile for `ssh -i`.
                use std::io::Write as _;
                let mut f = tempfile::NamedTempFile::new()
                    .map_err(|e| format!("cannot write key file: {e}"))?;
                f.write_all(k.as_bytes())
                    .and_then(|_| if k.ends_with('\n') { Ok(()) } else { f.write_all(b"\n") })
                    .map_err(|e| format!("cannot write key file: {e}"))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600))
                        .map_err(|e| format!("cannot chmod key file: {e}"))?;
                }
                (Some(f.path().to_path_buf()), Some(f))
            }
            Some(path) => (Some(PathBuf::from(path)), None),
        };
        Ok(SshTransport {
            host: t.host.clone(),
            port: t.effective_port(),
            user: t.user.clone(),
            password: t.password.clone(),
            key_path,
            os,
            _key_temp: key_temp,
        })
    }

    fn destination(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// The ssh/scp invocation skeleton: `sshpass -e` wraps password auth
    /// (SSHPASS lands in the env, never on the argv).
    fn base_command(&self, program: &str) -> tokio::process::Command {
        let mut cmd = match &self.password {
            Some(pw) => {
                let mut c = tokio::process::Command::new("sshpass");
                c.arg("-e").arg(program);
                c.env("SSHPASS", pw);
                c
            }
            None => tokio::process::Command::new(program),
        };
        let port_flag = if program == "scp" { "-P" } else { "-p" };
        cmd.arg(port_flag).arg(self.port.to_string());
        cmd.args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=15",
        ]);
        if self.password.is_none() {
            cmd.args(["-o", "BatchMode=yes"]);
        }
        if let Some(key) = &self.key_path {
            cmd.arg("-i").arg(key);
        }
        cmd
    }

    /// One remote command, rendered for the target's shell.
    fn ssh_command(&self, remote: &str) -> tokio::process::Command {
        let mut cmd = self.base_command("ssh");
        cmd.arg(self.destination()).arg("--").arg(remote);
        cmd
    }

    /// Render a remote command per target OS: POSIX sh for Linux, a
    /// quoting-proof `powershell -EncodedCommand` for Windows (whose
    /// default OpenSSH shell may be cmd or powershell — encoded payloads
    /// work from either).
    fn render(&self, spec: &ExecSpec) -> String {
        match self.os {
            TargetOs::Linux => sh_exec_line(spec),
            TargetOs::Windows => encoded_powershell(&ps_exec_script(spec)),
        }
    }

    fn exec_command(&self, spec: &ExecSpec) -> tokio::process::Command {
        self.ssh_command(&self.render(spec))
    }

    async fn probe(&self) -> Result<(), String> {
        if self.password.is_some() && which("sshpass").is_none() {
            return Err(
                "ssh password auth needs `sshpass` on the server host (or use key auth)".into(),
            );
        }
        let remote = match self.os {
            TargetOs::Linux => "exit 0".to_string(),
            TargetOs::Windows => encoded_powershell("exit 0"),
        };
        run_ok(self.ssh_command(&remote), "ssh probe").await
    }

    async fn mkdir(&self, dir: &str) -> Result<(), String> {
        let remote = match self.os {
            TargetOs::Linux => format!("mkdir -p {}", sh_quote(dir)),
            TargetOs::Windows => encoded_powershell(&format!(
                "New-Item -ItemType Directory -Force -Path {} | Out-Null",
                ps_quote(dir)
            )),
        };
        run_ok(self.ssh_command(&remote), "mkdir").await
    }

    async fn copy_file_in(&self, src: &Path, dest: &str, exec_bit: bool) -> Result<(), String> {
        let mut scp = self.base_command("scp");
        scp.arg(src).arg(format!("{}:{}", self.destination(), dest));
        run_ok(scp, "scp").await?;
        if exec_bit && self.os == TargetOs::Linux {
            run_ok(
                self.ssh_command(&format!("chmod +x {}", sh_quote(dest))),
                "chmod",
            )
            .await?;
        }
        Ok(())
    }

    async fn copy_dir_in(&self, src: &Path, dest: &str) -> Result<(), String> {
        match self.os {
            TargetOs::Linux => {
                // tar keeps exec bits; ship via a tempfile + remote extract.
                let tgz = tempfile::NamedTempFile::with_suffix(".tgz")
                    .map_err(|e| format!("cannot stage: {e}"))?;
                let mut tar = tokio::process::Command::new("tar");
                tar.arg("-C").arg(src).arg("-czf").arg(tgz.path()).arg(".");
                run_ok(tar, "tar").await?;
                let remote_tgz = format!("{dest}.tgz");
                self.copy_file_in(tgz.path(), &remote_tgz, false).await?;
                run_ok(
                    self.ssh_command(&format!(
                        "mkdir -p {d} && tar -xzf {t} -C {d} && rm -f {t}",
                        d = sh_quote(dest),
                        t = sh_quote(&remote_tgz)
                    )),
                    "remote extract",
                )
                .await
            }
            TargetOs::Windows => {
                // scp -r copies the staged dir as `dest` directly.
                let mut scp = self.base_command("scp");
                scp.arg("-r")
                    .arg(src)
                    .arg(format!("{}:{}", self.destination(), dest));
                run_ok(scp, "scp -r").await
            }
        }
    }

    async fn fetch_file(&self, path: &str) -> Result<String, String> {
        let remote = match self.os {
            TargetOs::Linux => format!("cat {}", sh_quote(path)),
            TargetOs::Windows => {
                encoded_powershell(&format!("Get-Content -Raw {}", ps_quote(path)))
            }
        };
        run_capture(self.ssh_command(&remote), "fetch").await
    }

    async fn remove_dir(&self, dir: &str) -> Result<(), String> {
        let remote = match self.os {
            TargetOs::Linux => format!("rm -rf {}", sh_quote(dir)),
            TargetOs::Windows => encoded_powershell(&format!(
                "Remove-Item -Recurse -Force -ErrorAction SilentlyContinue {}",
                ps_quote(dir)
            )),
        };
        run_ok(self.ssh_command(&remote), "cleanup").await
    }
}

// ---------------------------------------------------------------- winrm

pub struct WinrmTransport {
    host: String,
    port: u16,
    user: String,
    password: String,
    use_tls: bool,
}

impl WinrmTransport {
    fn new(t: &TransportConfig) -> Result<WinrmTransport, String> {
        let Some(password) = t.password.clone() else {
            return Err("winrm transport needs a password".into());
        };
        Ok(WinrmTransport {
            host: t.host.clone(),
            port: t.effective_port(),
            user: t.user.clone(),
            password,
            use_tls: t.use_tls,
        })
    }

    /// The local `pwsh` invocation running `script` after the session
    /// prelude binds `$s`. The password rides in an env var, not argv.
    fn pwsh(&self, script: &str) -> tokio::process::Command {
        let prelude = format!(
            "$ErrorActionPreference = 'Stop'\n\
             $pw = ConvertTo-SecureString -AsPlainText -Force -String $env:WEAVE_WINRM_PASSWORD\n\
             $cred = New-Object System.Management.Automation.PSCredential({user}, $pw)\n\
             $s = New-PSSession -ComputerName {host} -Port {port} -Credential $cred \
             -Authentication Negotiate{tls}\n",
            user = ps_quote(&self.user),
            host = ps_quote(&self.host),
            port = self.port,
            tls = if self.use_tls { " -UseSSL" } else { "" },
        );
        let mut cmd = tokio::process::Command::new("pwsh");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command"]);
        cmd.arg(format!("{prelude}{script}"));
        cmd.env("WEAVE_WINRM_PASSWORD", &self.password);
        cmd
    }

    /// A one-shot remote scriptblock: run, tear the session down, done.
    fn pwsh_invoke(&self, remote: &str) -> tokio::process::Command {
        self.pwsh(&format!(
            "Invoke-Command -Session $s {{ {remote} }}\nRemove-PSSession $s"
        ))
    }

    async fn probe(&self) -> Result<(), String> {
        if which("pwsh").is_none() {
            return Err(
                "winrm transport needs `pwsh` (PowerShell 7 + PSWSMan) on the server host".into(),
            );
        }
        run_ok(
            self.pwsh_invoke("$PSVersionTable.PSVersion.ToString() | Out-Null"),
            "winrm probe",
        )
        .await
    }

    async fn mkdir(&self, dir: &str) -> Result<(), String> {
        run_ok(
            self.pwsh_invoke(&format!(
                "New-Item -ItemType Directory -Force -Path {} | Out-Null",
                ps_quote(dir)
            )),
            "mkdir",
        )
        .await
    }

    async fn copy_file_in(&self, src: &Path, dest: &str) -> Result<(), String> {
        run_ok(
            self.pwsh(&format!(
                "Copy-Item -ToSession $s -Path {} -Destination {}\nRemove-PSSession $s",
                ps_quote(&src.to_string_lossy()),
                ps_quote(dest)
            )),
            "copy",
        )
        .await
    }

    async fn copy_dir_in(&self, src: &Path, dest: &str) -> Result<(), String> {
        run_ok(
            self.pwsh(&format!(
                "Copy-Item -ToSession $s -Recurse -Path {} -Destination {}\nRemove-PSSession $s",
                ps_quote(&src.to_string_lossy()),
                ps_quote(dest)
            )),
            "copy -Recurse",
        )
        .await
    }

    /// Remote exec with live output: stderr lines of the native command
    /// become `Write-Host` records (streamed progressively by PSRemoting),
    /// stdout goes to the remote redirect file, and the remote
    /// `$LASTEXITCODE` is the scriptblock's only pipeline output.
    fn exec_command(&self, spec: &ExecSpec) -> Result<tokio::process::Command, String> {
        let redirect = spec
            .stdout_to
            .as_deref()
            .ok_or("winrm exec requires a stdout redirect (the report protocol)")?;
        let args_ps = spec
            .args
            .iter()
            .map(|a| ps_quote(a))
            .collect::<Vec<_>>()
            .join(", ");
        let script = format!(
            "$code = Invoke-Command -Session $s -ScriptBlock {{\n\
               param($exe, $arglist, $out)\n\
               & $exe @arglist 2>&1 | ForEach-Object {{\n\
                 if ($_ -is [System.Management.Automation.ErrorRecord]) {{ Write-Host \"$_\" }}\n\
                 else {{ $_ }}\n\
               }} | Out-File -Encoding utf8 $out\n\
               $LASTEXITCODE\n\
             }} -ArgumentList {exe}, @({args_ps}), {out}\n\
             if ($null -eq $code) {{ $code = 1 }}",
            exe = ps_quote(&spec.program),
            out = ps_quote(redirect),
        );
        Ok(self.pwsh(&format!("{script}\nRemove-PSSession $s\nexit [int]$code")))
    }

    async fn fetch_file(&self, path: &str) -> Result<String, String> {
        run_capture(
            self.pwsh_invoke(&format!("Get-Content -Raw {}", ps_quote(path))),
            "fetch",
        )
        .await
    }

    async fn remove_dir(&self, dir: &str) -> Result<(), String> {
        run_ok(
            self.pwsh_invoke(&format!(
                "Remove-Item -Recurse -Force -ErrorAction SilentlyContinue {}",
                ps_quote(dir)
            )),
            "cleanup",
        )
        .await
    }
}

// -------------------------------------------------------------- helpers

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|c| c.is_file())
}

/// POSIX single-quote: safe for any byte except that quotes are closed,
/// escaped, and reopened.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// PowerShell single-quoted literal: only `'` needs doubling.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The POSIX-sh line for an ExecSpec.
fn sh_exec_line(spec: &ExecSpec) -> String {
    let mut line = sh_quote(&spec.program);
    for a in &spec.args {
        line.push(' ');
        line.push_str(&sh_quote(a));
    }
    if let Some(out) = &spec.stdout_to {
        line.push_str(" > ");
        line.push_str(&sh_quote(out));
    }
    line
}

/// The PowerShell script for an ExecSpec on a Windows-over-ssh target:
/// same stream discipline as the winrm scriptblock, run locally on the
/// target.
fn ps_exec_script(spec: &ExecSpec) -> String {
    let args_ps = spec
        .args
        .iter()
        .map(|a| ps_quote(a))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "$a = @({args_ps})\n\
         & {exe} @a 2>&1 | ForEach-Object {{\n\
           if ($_ -is [System.Management.Automation.ErrorRecord]) {{ [Console]::Error.WriteLine(\"$_\") }}\n\
           else {{ $_ }}\n\
         }}",
        exe = ps_quote(&spec.program),
    );
    match &spec.stdout_to {
        Some(out) => format!("{body} | Out-File -Encoding utf8 {}\nexit $LASTEXITCODE", ps_quote(out)),
        None => format!("{body}\nexit $LASTEXITCODE"),
    }
}

/// `powershell -EncodedCommand` payload: UTF-16LE + base64, immune to
/// every ssh/cmd/powershell quoting layer in between.
fn encoded_powershell(script: &str) -> String {
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(utf16)
    )
}

/// Run to completion; non-zero exit is an error carrying stderr.
async fn run_ok(mut cmd: tokio::process::Command, what: &str) -> Result<(), String> {
    let out = cmd
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("{what}: cannot run: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{what} failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Run to completion; return stdout, error on non-zero exit.
async fn run_capture(mut cmd: tokio::process::Command, what: &str) -> Result<String, String> {
    let out = cmd
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("{what}: cannot run: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "{what} failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Spawn and stream both output pipes line-by-line into `on_line` until
/// exit or cancellation.
async fn stream_child(
    cmd: &mut tokio::process::Command,
    on_line: &mut (dyn FnMut(String) + Send),
    cancel: &tokio::sync::Notify,
) -> Result<i32, String> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("cannot spawn: {e}"))?;
    let mut out_lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
    let mut err_lines = BufReader::new(child.stderr.take().expect("stderr piped")).lines();
    let (mut out_done, mut err_done, mut cancelled) = (false, false, false);

    while !(out_done && err_done) {
        tokio::select! {
            line = out_lines.next_line(), if !out_done => match line {
                Ok(Some(l)) => on_line(l),
                _ => out_done = true,
            },
            line = err_lines.next_line(), if !err_done => match line {
                Ok(Some(l)) => on_line(l),
                _ => err_done = true,
            },
            _ = cancel.notified(), if !cancelled => {
                cancelled = true;
                let _ = child.start_kill();
                // Keep draining until the pipes close.
            }
        }
    }

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    if cancelled {
        return Err("cancelled".into());
    }
    Ok(status.code().unwrap_or(-1))
}

/// Recursive copy skipping EXCLUDED_DIRS and dotdirs, dereferencing
/// nothing exotic (symlinked files copy as their content). Also used by
/// the package repository's add-to-runbook copy.
pub(crate) fn copy_dir_filtered(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            if EXCLUDED_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            copy_dir_filtered(&path, &dest.join(&name))?;
        } else {
            std::fs::copy(&path, dest.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quoting_survives_hostile_input() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        let spec = ExecSpec {
            program: "/tmp/weave run/config-weave".into(),
            args: vec!["apply".into(), "a'b".into()],
            stdout_to: Some("/tmp/out.json".into()),
        };
        assert_eq!(
            sh_exec_line(&spec),
            r"'/tmp/weave run/config-weave' 'apply' 'a'\''b' > '/tmp/out.json'"
        );
    }

    #[test]
    fn ps_quoting_doubles_single_quotes() {
        assert_eq!(ps_quote("O'Brien"), "'O''Brien'");
    }

    #[test]
    fn encoded_powershell_is_utf16le_base64() {
        let enc = encoded_powershell("exit 0");
        let b64 = enc.rsplit(' ').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        // "e\0x\0i\0t\0 \00\0"
        assert_eq!(bytes[0], b'e');
        assert_eq!(bytes[1], 0);
        assert_eq!(bytes.len(), "exit 0".len() * 2);
    }

    #[test]
    fn stage_dirs_are_os_appropriate() {
        assert_eq!(stage_dir(TargetOs::Linux, "abc"), "/tmp/weave-run-abc");
        assert_eq!(
            stage_dir(TargetOs::Windows, "abc"),
            "C:/Windows/Temp/weave-run-abc"
        );
    }

    #[test]
    fn inline_pem_becomes_a_private_tempfile() {
        let cfg = TransportConfig {
            kind: TransportKind::Ssh,
            host: "h".into(),
            port: None,
            user: "u".into(),
            password: None,
            private_key: Some("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END-----".into()),
            use_tls: false,
        };
        let t = SshTransport::new(&cfg, TargetOs::Linux).unwrap();
        let path = t.key_path.clone().unwrap();
        assert!(path.is_file());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("BEGIN OPENSSH"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(t);
        assert!(!path.exists(), "tempfile key must vanish with the transport");
    }

    #[test]
    fn winrm_requires_a_password() {
        let cfg = TransportConfig {
            kind: TransportKind::Winrm,
            host: "h".into(),
            port: None,
            user: "u".into(),
            password: None,
            private_key: None,
            use_tls: false,
        };
        assert!(WinrmTransport::new(&cfg).is_err());
    }

    #[test]
    fn filtered_copy_skips_junk_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::create_dir_all(src.join("pkgs/core")).unwrap();
        std::fs::write(src.join("playbook.wcl"), "x").unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(src.join("pkgs/core/package.wcl"), "y").unwrap();

        let dest = tmp.path().join("dest");
        copy_dir_filtered(&src, &dest).unwrap();
        assert!(dest.join("playbook.wcl").is_file());
        assert!(dest.join("pkgs/core/package.wcl").is_file());
        assert!(!dest.join(".git").exists());
    }
}
