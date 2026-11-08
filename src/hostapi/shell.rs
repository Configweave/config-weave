//! The `shell` module (PRD §7): run commands with captured output, a
//! streaming variant that pipes through `log` live, and the `bash` /
//! `powershell` conveniences.
//!
//! `run` parses the command string with shell-words and executes the
//! program directly (no shell interpretation); use `bash`/`powershell`
//! when shell features are wanted.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use wscript::{Module, Script};
use wscript_std::DynValue;

use super::log;

/// Result of a shell invocation. A data struct, so scripts read fields
/// directly: `out.stdout`, `out.code`, `out.success`.
#[derive(Script, Debug, Clone)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i64,
    pub success: bool,
}

#[derive(Default)]
struct Opts {
    cwd: Option<String>,
    env: Vec<(String, String)>,
    timeout: Option<Duration>,
    stdin: Option<String>,
}

fn parse_opts(opts: &DynValue) -> Result<Opts, String> {
    let mut out = Opts::default();
    match opts {
        DynValue::Null => return Ok(out),
        DynValue::Map(m) => {
            for (k, v) in m {
                match (k.as_str(), v) {
                    ("cwd", DynValue::String(s)) => out.cwd = Some(s.clone()),
                    ("stdin", DynValue::String(s)) => out.stdin = Some(s.clone()),
                    ("timeout", v) => {
                        out.timeout = Some(super::timeout_secs(v).ok_or_else(|| {
                            format!(
                                "shell option 'timeout' must be a non-negative number of \
                                 seconds, got {v:?}"
                            )
                        })?);
                    }
                    ("env", DynValue::Map(vars)) => {
                        for (name, value) in vars {
                            match value {
                                DynValue::String(s) => out.env.push((name.clone(), s.clone())),
                                other => {
                                    return Err(format!(
                                        "env var '{name}' must be a string, got {other:?}"
                                    ));
                                }
                            }
                        }
                    }
                    (other, _) => {
                        return Err(format!(
                            "unknown shell option '{other}' (expected cwd, env, timeout, stdin)"
                        ));
                    }
                }
            }
        }
        other => {
            return Err(format!(
                "shell options must be a map or null, got {other:?}"
            ));
        }
    }
    Ok(out)
}

fn build_command(program: &str, args: &[String], opts: &Opts) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(cwd) = &opts.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    cmd.stdin(if opts.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd
}

/// The instant `timeout` from now. `None` when there is no timeout, or when
/// the timeout is too large to add to the clock: such a deadline is never
/// reached, so it is the same as none, and `Instant + Duration` would panic.
fn deadline_after(timeout: Option<Duration>) -> Option<Instant> {
    timeout.and_then(|t| Instant::now().checked_add(t))
}

/// Kill and reap the child after its deadline passed. Only the direct child
/// is killed: a grandchild it started keeps running.
fn time_out(child: &mut Child) -> String {
    let _ = child.kill();
    let _ = child.wait();
    "command timed out".to_string()
}

/// Wait for the child until an optional deadline, killing it on timeout.
fn wait_until(child: &mut Child, deadline: Option<Instant>) -> Result<i64, String> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code().unwrap_or(-1) as i64),
            Ok(None) => {
                if let Some(d) = deadline
                    && Instant::now() >= d
                {
                    return Err(time_out(child));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn feed_stdin(child: &mut Child, opts: &Opts) {
    if let (Some(input), Some(mut stdin)) = (opts.stdin.clone(), child.stdin.take()) {
        let _ = stdin.write_all(input.as_bytes());
        // dropped here, closing the pipe
    }
}

/// Output is decoded lossily: a child that writes a stray non-UTF-8 byte
/// (PowerShell 5.1 re-encodes native output to the OEM code page) loses
/// that character to U+FFFD, not the whole stream.
fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Read `reader` line by line, decoding each lossily, with the line ending
/// (`\n` or `\r\n`) stripped as `BufRead::lines` would.
fn lossy_lines(reader: impl Read, mut each: impl FnMut(String)) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
                each(decode(&buf));
            }
        }
    }
}

fn run_captured(program: &str, args: &[String], opts: &Opts) -> Result<CmdOutput, String> {
    let deadline = deadline_after(opts.timeout);
    let mut child = build_command(program, args, opts)
        .spawn()
        .map_err(|e| format!("cannot start '{program}': {e}"))?;
    feed_stdin(&mut child, opts);

    // Drain pipes on threads so big output can't deadlock the child.
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| format!("cannot capture stdout of '{program}'"))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| format!("cannot capture stderr of '{program}'"))?;
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let code = wait_until(&mut child, deadline)?;
    let stdout = decode(&out_thread.join().unwrap_or_default());
    let stderr = decode(&err_thread.join().unwrap_or_default());
    Ok(CmdOutput {
        stdout,
        stderr,
        code,
        success: code == 0,
    })
}

/// One output line of a streamed child.
enum Line {
    Out(String),
    Err(String),
}

/// Streaming variant: lines go through `log` live (stdout → info,
/// stderr → warn) and are captured as well.
///
/// The log sinks are thread-local, so helper threads read the pipes and
/// send the lines over a channel, and this thread logs them. It receives
/// with the deadline, so a child that stops writing still times out.
fn run_streaming_impl(program: &str, args: &[String], opts: &Opts) -> Result<CmdOutput, String> {
    let deadline = deadline_after(opts.timeout);
    let mut child = build_command(program, args, opts)
        .spawn()
        .map_err(|e| format!("cannot start '{program}': {e}"))?;
    feed_stdin(&mut child, opts);

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| format!("cannot capture stdout of '{program}'"))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| format!("cannot capture stderr of '{program}'"))?;
    let (tx, rx) = mpsc::channel();
    let err_tx = tx.clone();
    // On a timeout the readers are not joined: a grandchild can hold the
    // pipes open after the child is killed. They end at its EOF.
    std::thread::spawn(move || {
        lossy_lines(stdout_pipe, |line| {
            let _ = tx.send(Line::Out(line));
        })
    });
    std::thread::spawn(move || {
        lossy_lines(stderr_pipe, |line| {
            let _ = err_tx.send(Line::Err(line));
        })
    });

    let mut stdout = String::new();
    let mut stderr = String::new();
    loop {
        let next = match deadline {
            Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let (level, buf, line) = match next {
            Ok(Line::Out(line)) => (log::Level::Info, &mut stdout, line),
            Ok(Line::Err(line)) => (log::Level::Warn, &mut stderr, line),
            // Both pipes reached EOF.
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => return Err(time_out(&mut child)),
        };
        log::emit(level, &line);
        buf.push_str(&line);
        buf.push('\n');
    }
    let code = wait_until(&mut child, deadline)?;
    Ok(CmdOutput {
        stdout,
        stderr,
        code,
        success: code == 0,
    })
}

fn split_command(cmd: &str) -> Result<(String, Vec<String>), String> {
    let words = shell_words::split(cmd).map_err(|e| format!("cannot parse command: {e}"))?;
    let Some((program, args)) = words.split_first() else {
        return Err("empty command".to_string());
    };
    Ok((program.clone(), args.to_vec()))
}

fn bash_argv(script: &str) -> (String, Vec<String>) {
    // `bash -c`, falling back to `sh` on minimal systems.
    let shell = if which("bash") { "bash" } else { "sh" };
    (shell.to_string(), vec!["-c".into(), script.to_string()])
}

fn powershell_exe() -> Result<&'static str, String> {
    if which("powershell") {
        Ok("powershell")
    } else if which("pwsh") {
        Ok("pwsh")
    } else {
        Err("no powershell or pwsh on PATH".to_string())
    }
}

/// Appended to every script so the exit code is what `-Command <script>`
/// gives: `exit N` still wins, otherwise 0 when the last statement
/// succeeded and 1 when it failed. Without it a dot-sourced script block
/// exits 0 after a failing last statement.
pub(crate) const PS_EXIT_TRAILER: &str = "\nif ($?) { exit 0 } else { exit 1 }\n";

/// The script never goes on the command line, where the process list,
/// command-line auditing (event 4688) and EDR telemetry would see any
/// secret a package interpolated into it. It is written to a private temp
/// file, and `-Command` gets a bootstrap holding only that file's path,
/// which it reads into a script block and dot-sources. That keeps
/// `-Command`'s exit-code, output and stdin behaviour, and unlike `-File` it
/// is not subject to execution policy. `-Command -` (script on stdin) was
/// rejected: PowerShell parses stdin line by line as if typed, which breaks
/// an `else` on its own line, and it would take `opts.stdin` away.
///
/// A script block has its own `$input`, so one that reads `$input` gets the
/// process's stdin piped in. Only then: the pipe drains stdin before the
/// script starts, which would starve `[Console]::In`.
///
/// `script_path` is a string, not a `Path`, because the testlab passes a
/// path inside a Windows guest from a Linux host.
pub(crate) fn powershell_argv(exe: &str, script_path: &str, script: &str) -> (String, Vec<String>) {
    let load = format!(
        ". ([scriptblock]::Create([IO.File]::ReadAllText({})))",
        ps_quote(script_path)
    );
    let bootstrap = if reads_input(script) {
        format!("$input | {load}")
    } else {
        load
    };
    (
        exe.to_string(),
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            bootstrap,
        ],
    )
}

/// Whether `script` names the `$input` automatic variable (any case), as
/// opposed to a longer name such as `$InputObject`.
fn reads_input(script: &str) -> bool {
    let lower = script.to_ascii_lowercase();
    lower.match_indices("$input").any(|(at, m)| {
        lower[at + m.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

/// A PowerShell single-quoted string literal. PowerShell also treats the
/// typographic quotes as `'`, so those are doubled too.
fn ps_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Write `script` (plus the exit trailer) to `script.ps1` in a fresh temp
/// directory. On Unix the directory is 0700 and the file 0600; on Windows
/// the user temp directory is already ACL'd to its owner (and SYSTEM and
/// Administrators). Dropping the returned dir deletes both, so every exit
/// path of the caller, error or timeout included, cleans up.
pub(crate) fn write_ps_script(
    script: &str,
) -> Result<(tempfile::TempDir, std::path::PathBuf), String> {
    let err = |e: std::io::Error| format!("cannot write powershell script: {e}");
    let dir = tempfile::Builder::new()
        .prefix("config-weave-ps-")
        .tempdir()
        .map_err(err)?;
    // tempfile leaves the directory at the umask default; tighten it before
    // anything is written into it.
    #[cfg(unix)]
    std::fs::set_permissions(
        dir.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .map_err(err)?;
    let path = dir.path().join("script.ps1");
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
    let mut file = file.open(&path).map_err(err)?;
    file.write_all(script.as_bytes()).map_err(err)?;
    file.write_all(PS_EXIT_TRAILER.as_bytes()).map_err(err)?;
    Ok((dir, path))
}

fn run_powershell(exe: &str, script: &str, opts: &Opts) -> Result<CmdOutput, String> {
    let (_dir, path) = write_ps_script(script)?;
    let (program, args) = powershell_argv(exe, &path.to_string_lossy(), script);
    run_captured(&program, &args, opts)
}

fn which(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            if dir.join(format!("{program}.exe")).is_file() {
                return true;
            }
        }
    }
    false
}

pub fn module() -> Module {
    let mut m = Module::new("shell");
    m.doc("Run external commands (capability: process execution)");

    m.doc_next(
        "Run a command (shell-words split, no shell interpretation). \
         opts: cwd, env (map), timeout (secs), stdin",
    );
    m.fn_(
        "run",
        |cmd: &str, opts: DynValue| -> Result<CmdOutput, String> {
            let (program, args) = split_command(cmd)?;
            run_captured(&program, &args, &parse_opts(&opts)?)
        },
    );
    m.doc_next("Like run, but stream output lines through log live (long installs)");
    m.fn_(
        "run_streaming",
        |cmd: &str, opts: DynValue| -> Result<CmdOutput, String> {
            let (program, args) = split_command(cmd)?;
            run_streaming_impl(&program, &args, &parse_opts(&opts)?)
        },
    );
    m.doc_next("Run a script with `bash -c` (falls back to sh)");
    m.fn_(
        "bash",
        |script: &str, opts: DynValue| -> Result<CmdOutput, String> {
            let (program, args) = bash_argv(script);
            run_captured(&program, &args, &parse_opts(&opts)?)
        },
    );
    m.doc_next(
        "Run a script with PowerShell (-NoProfile -NonInteractive), read from a \
         private temp file so it never appears on the command line",
    );
    m.fn_(
        "powershell",
        |script: &str, opts: DynValue| -> Result<CmdOutput, String> {
            run_powershell(powershell_exe()?, script, &parse_opts(&opts)?)
        },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    // `\xff` is never valid UTF-8; the text either side of it must survive.
    #[cfg(unix)]
    const INVALID: &str = r"printf 'before \377 after\n'; printf 'err \377 tail\n' >&2";

    #[cfg(unix)]
    #[test]
    fn captured_output_survives_invalid_utf8() {
        let out = run_captured("sh", &["-c".into(), INVALID.into()], &Opts::default()).unwrap();
        assert_eq!(out.stdout, "before \u{FFFD} after\n");
        assert_eq!(out.stderr, "err \u{FFFD} tail\n");
        assert!(out.success);
    }

    #[cfg(unix)]
    #[test]
    fn streamed_output_survives_invalid_utf8() {
        let script = format!("{INVALID}; printf 'next line\\r\\n'");
        let out = run_streaming_impl("sh", &["-c".into(), script], &Opts::default()).unwrap();
        assert_eq!(out.stdout, "before \u{FFFD} after\nnext line\n");
        assert_eq!(out.stderr, "err \u{FFFD} tail\n");
    }

    fn timeout_opts(v: DynValue) -> Opts {
        parse_opts(&DynValue::Map(
            [("timeout".to_string(), v)].into_iter().collect(),
        ))
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn run_times_out_and_kills_the_child() {
        let started = Instant::now();
        let err =
            run_captured("sleep", &["30".into()], &timeout_opts(DynValue::Int(1))).unwrap_err();
        assert_eq!(err, "command timed out");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    /// The child writes, then sleeps past the timeout. The lines it wrote
    /// reach the log before the timeout fires, and the call returns soon
    /// after it, although a grandchild (`sleep`) still holds the pipes.
    #[cfg(unix)]
    #[test]
    fn run_streaming_times_out_and_logs_earlier_lines_live() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let started = Instant::now();
        let seen: Rc<RefCell<Vec<(log::Level, String, Duration)>>> = Rc::default();
        let sink = seen.clone();
        log::set_sink(Box::new(move |level, msg| {
            sink.borrow_mut()
                .push((level, msg.to_string(), started.elapsed()));
        }));
        let script = "echo first; echo oops >&2; sleep 30; echo never";
        let result = run_streaming_impl(
            "sh",
            &["-c".into(), script.into()],
            &timeout_opts(DynValue::Int(1)),
        );
        log::clear_sink();
        let took = started.elapsed();
        assert_eq!(result.unwrap_err(), "command timed out");
        assert!(took < Duration::from_secs(5), "{took:?}");
        let seen = seen.borrow();
        let line = |want: &str| seen.iter().find(|(_, m, _)| m == want).cloned();
        let (level, _, at) = line("first").expect("stdout line logged");
        assert_eq!(level, log::Level::Info);
        assert!(at < Duration::from_secs(1), "logged at {at:?}, not live");
        let (level, _, at) = line("oops").expect("stderr line logged");
        assert_eq!(level, log::Level::Warn);
        assert!(at < Duration::from_secs(1), "logged at {at:?}, not live");
        assert!(line("never").is_none());
    }

    /// A timeout too large to add to the clock means no deadline, not a
    /// panic.
    #[cfg(unix)]
    #[test]
    fn a_huge_timeout_runs_with_no_deadline() {
        for v in [DynValue::Int(i64::MAX), DynValue::Float(1e19)] {
            let opts = timeout_opts(v);
            let out = run_captured("true", &[], &opts).unwrap();
            assert!(out.success);
            let out = run_streaming_impl("true", &[], &opts).unwrap();
            assert!(out.success);
        }
    }

    fn ps(script: &str) -> Vec<String> {
        powershell_argv("pwsh", "/t/script.ps1", script).1
    }

    #[test]
    fn powershell_script_is_not_on_the_command_line() {
        let args = ps("$pw = 'MARKER-hunter2'\nWrite-Output $pw");
        assert_eq!(&args[..3], ["-NoProfile", "-NonInteractive", "-Command"]);
        assert_eq!(
            args[3],
            ". ([scriptblock]::Create([IO.File]::ReadAllText('/t/script.ps1')))"
        );
        assert!(args.iter().all(|a| !a.contains("MARKER")));
    }

    #[test]
    fn powershell_pipes_stdin_only_to_scripts_that_read_input() {
        assert!(ps("$Input | ForEach-Object { $_ }")[3].starts_with("$input | . "));
        assert!(ps("$input")[3].starts_with("$input | . "));
        assert!(ps("$InputObject = 1; [Console]::In.ReadToEnd()")[3].starts_with(". "));
    }

    #[test]
    fn ps_quote_doubles_every_single_quote_form() {
        assert_eq!(ps_quote("C:\\O'Brien\\x"), "'C:\\O''Brien\\x'");
        assert_eq!(ps_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
    }

    #[test]
    fn powershell_script_file_is_private_and_removed_on_drop() {
        let (dir, path) = write_ps_script("Write-Output 'é'").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, format!("Write-Output 'é'{PS_EXIT_TRAILER}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode();
            assert_eq!(mode(dir.path()) & 0o777, 0o700);
            assert_eq!(mode(&path) & 0o777, 0o600);
        }
        let root = dir.path().to_path_buf();
        drop(dir);
        assert!(!root.exists());
    }

    /// A stand-in interpreter that prints its argv, so the test sees the
    /// real command line `run_powershell` spawns.
    #[cfg(unix)]
    #[test]
    fn powershell_run_keeps_the_script_off_argv_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let fake = bin.path().join("pwsh");
        std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        // A fork in another test thread can inherit the fake's write fd until
        // that child execs, and our exec then fails with ETXTBSY. Each call
        // writes its own script, so a retry only repeats the spawn.
        let mut attempts = 0;
        let out = loop {
            match run_powershell(
                fake.to_str().unwrap(),
                "$pw = 'MARKER-hunter2'",
                &Opts::default(),
            ) {
                Err(e) if e.contains("Text file busy") && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => break other.unwrap(),
            }
        };
        assert!(!out.stdout.contains("MARKER"), "{}", out.stdout);
        let bootstrap = out.stdout.lines().nth(3).unwrap();
        let path = bootstrap.split('\'').nth(1).unwrap();
        assert!(path.ends_with("script.ps1"), "{bootstrap}");
        assert!(!std::path::Path::new(path).exists());
    }

    // The rest run real PowerShell, and skip where none is installed.

    fn pwsh(script: &str, opts: Opts) -> Option<CmdOutput> {
        let Ok(exe) = powershell_exe() else {
            eprintln!("skipped: no powershell or pwsh on PATH");
            return None;
        };
        Some(run_powershell(exe, script, &opts).unwrap())
    }

    #[test]
    fn powershell_runs_multi_line_constructs() {
        let script = "if ($false) {\n  'a'\n}\nelse {\n  'b'\n}\n$h = @\"\nhere é\n\"@\n$h\n& {\n  'block'\n}";
        let Some(out) = pwsh(script, Opts::default()) else {
            return;
        };
        assert_eq!(out.stdout.replace("\r\n", "\n"), "b\nhere é\nblock\n");
        assert_eq!(out.code, 0, "{}", out.stderr);
    }

    #[test]
    fn powershell_keeps_command_exit_codes() {
        let code = |script: &str| pwsh(script, Opts::default()).map(|o| o.code);
        let Some(explicit) = code("'x'\nexit 42") else {
            return;
        };
        assert_eq!(explicit, 42);
        // Like `-Command`: a failing last statement exits 1, a failure
        // followed by a success exits 0, a throw exits 1.
        assert_eq!(code("Get-Item /no/such/path/at/all"), Some(1));
        assert_eq!(code("Get-Item /no/such/path/at/all\n'ok'"), Some(0));
        assert_eq!(code("throw 'boom'"), Some(1));
        assert_eq!(code("function f { exit 7 }\nf\n'after'"), Some(7));
    }

    #[test]
    fn powershell_reads_stdin_both_ways() {
        let opts = || Opts {
            stdin: Some("a\nb\n".into()),
            ..Opts::default()
        };
        let Some(out) = pwsh("$input | ForEach-Object { \"got $_\" }", opts()) else {
            return;
        };
        assert_eq!(out.stdout.replace("\r\n", "\n"), "got a\ngot b\n");
        let out = pwsh("[Console]::In.ReadToEnd()", opts()).unwrap();
        assert_eq!(out.stdout.replace("\r\n", "\n"), "a\nb\n\n");
    }

    /// The child's own command line, read from /proc, never holds the script.
    #[cfg(target_os = "linux")]
    #[test]
    fn powershell_child_argv_has_no_script_text() {
        let script = "$m = 'MARKER-' + 'x9'\n$c = [IO.File]::ReadAllText(\"/proc/$PID/cmdline\")\n\
                      if ($c.Contains('MARKER-x9')) { 'leaked' } else { 'clean' }";
        let Some(out) = pwsh(script, Opts::default()) else {
            return;
        };
        assert_eq!(out.stdout.trim(), "clean", "{}", out.stderr);
    }
}
