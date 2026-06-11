//! The `shell` module (PRD §7): run commands with captured output, a
//! streaming variant that pipes through `log` live, and the `bash` /
//! `powershell` conveniences.
//!
//! `run` parses the command string with shell-words and executes the
//! program directly (no shell interpretation); use `bash`/`powershell`
//! when shell features are wanted.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wisp::{Module, Script};
use wisp_std::DynValue;

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
                    ("timeout", DynValue::Int(secs)) => {
                        out.timeout = Some(Duration::from_secs(*secs as u64));
                    }
                    ("timeout", DynValue::Float(secs)) => {
                        out.timeout = Some(Duration::from_secs_f64(*secs));
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

/// Wait with an optional deadline, killing the child on timeout.
fn wait_with_timeout(child: &mut Child, timeout: Option<Duration>) -> Result<i64, String> {
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code().unwrap_or(-1) as i64),
            Ok(None) => {
                if let Some(d) = deadline
                    && Instant::now() >= d
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("command timed out".to_string());
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

fn run_captured(program: &str, args: &[String], opts: &Opts) -> Result<CmdOutput, String> {
    let mut child = build_command(program, args, opts)
        .spawn()
        .map_err(|e| format!("cannot start '{program}': {e}"))?;
    feed_stdin(&mut child, opts);

    // Drain pipes on threads so big output can't deadlock the child.
    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });

    let code = wait_with_timeout(&mut child, opts.timeout)?;
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(CmdOutput {
        stdout,
        stderr,
        code,
        success: code == 0,
    })
}

/// Streaming variant: lines go through `log` live (stdout → info,
/// stderr → warn) and are captured as well.
fn run_streaming_impl(program: &str, args: &[String], opts: &Opts) -> Result<CmdOutput, String> {
    let mut child = build_command(program, args, opts)
        .spawn()
        .map_err(|e| format!("cannot start '{program}': {e}"))?;
    feed_stdin(&mut child, opts);

    let stdout_pipe = child.stdout.take().unwrap();
    let stderr_pipe = child.stderr.take().unwrap();
    // log sinks are thread-local, so stream from this thread; drain
    // stderr on a helper thread into a buffer, logging after.
    let err_thread = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
            lines.push(line);
        }
        lines
    });
    let mut stdout = String::new();
    for line in BufReader::new(stdout_pipe).lines().map_while(Result::ok) {
        log::emit(log::Level::Info, &line);
        stdout.push_str(&line);
        stdout.push('\n');
    }
    let code = wait_with_timeout(&mut child, opts.timeout)?;
    let mut stderr = String::new();
    for line in err_thread.join().unwrap_or_default() {
        log::emit(log::Level::Warn, &line);
        stderr.push_str(&line);
        stderr.push('\n');
    }
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

fn powershell_argv(script: &str) -> Result<(String, Vec<String>), String> {
    let exe = if which("powershell") {
        "powershell"
    } else if which("pwsh") {
        "pwsh"
    } else {
        return Err("no powershell or pwsh on PATH".to_string());
    };
    Ok((
        exe.to_string(),
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script.to_string(),
        ],
    ))
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
    m.doc_next("Run a script with PowerShell (-NoProfile -NonInteractive)");
    m.fn_(
        "powershell",
        |script: &str, opts: DynValue| -> Result<CmdOutput, String> {
            let (program, args) = powershell_argv(script)?;
            run_captured(&program, &args, &parse_opts(&opts)?)
        },
    );
    m
}
