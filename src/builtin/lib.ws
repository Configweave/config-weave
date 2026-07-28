// Shared by the two built-in `execute` resources: reading their common
// parameters and handing a script to the right interpreter.

use value
use json
use shell
use sys

fn param_str(params: Value, key: string, fallback: string) -> string {
    if let Some(v) = params.get(key) { if let Some(s) = v.as_string() { return s } }
    fallback
}

fn param_int(params: Value, key: string, fallback: int) -> int {
    if let Some(v) = params.get(key) { if let Some(i) = v.as_int() { return i } }
    fallback
}

// `auto` follows the host. `bash` is what `shell::bash` gives us, which
// falls back to sh where bash is absent.
fn interpreter(params: Value) -> Result[string, string] {
    let want = param_str(params, "shell", "auto")
    if want == "auto" {
        if sys::family() == "windows" { return Ok("powershell") }
        return Ok("bash")
    }
    if want == "bash" { return Ok("bash") }
    if want == "powershell" { return Ok("powershell") }
    Err("invalid 'shell' value '" + want + "' (expected :auto, :bash or :powershell)")
}

// A `duration` param reaches scripts as nanoseconds; the shell module
// takes whole seconds. A timeout worth setting is worth at least one.
fn timeout_secs(params: Value) -> int {
    let ns = param_int(params, "timeout", 0)
    if ns <= 0 { return 0 }
    let secs = ns / 1000000000
    if secs < 1 { 1 } else { secs }
}

// `shell::*` reads its options off a map, rejects any key it does not
// recognise, and cannot make sense of a null — so an unset option has to
// be left out entirely rather than passed as empty. That is why this is
// four spelled-out maps instead of one built up key by key.
//
// `env` is always present because an empty map is harmless, and `stdin` is
// always present so a script inherits an empty stdin rather than whatever
// the runner had.
fn options(params: Value) -> Value {
    let env = Value::Map(#{})
    if let Some(e) = params.get("env") {
        if !e.is_null() { env = e }
    }
    let cwd = param_str(params, "cwd", "")
    let secs = timeout_secs(params)
    if cwd != "" && secs > 0 {
        return Value::Map(#{
            "cwd": Value::String(cwd),
            "timeout": Value::Int(secs),
            "env": env,
            "stdin": Value::String(""),
        })
    }
    if cwd != "" {
        return Value::Map(#{
            "cwd": Value::String(cwd),
            "env": env,
            "stdin": Value::String(""),
        })
    }
    if secs > 0 {
        return Value::Map(#{
            "timeout": Value::Int(secs),
            "env": env,
            "stdin": Value::String(""),
        })
    }
    Value::Map(#{ "env": env, "stdin": Value::String("") })
}

fn run_script(params: Value, script: string) -> Result[CmdOutput, string] {
    let opts = options(params)
    let kind = interpreter(params)?
    if kind == "powershell" { return shell::powershell(script, opts) }
    shell::bash(script, opts)
}

// Whether an exit status was declared as "succeeded, but reboot first".
fn is_reboot_code(params: Value, code: int) -> bool {
    if let Some(v) = params.get("reboot_on") {
        if let Some(items) = v.as_list() {
            for item in items {
                if let Some(i) = item.as_int() {
                    if i == code { return true }
                }
            }
        }
    }
    false
}

// What went wrong, with enough of the output to act on. stderr is where a
// failing script usually says it, but plenty write to stdout instead.
fn failure(what: string, out: CmdOutput) -> string {
    let detail = out.stderr.trim()
    if detail == "" { detail = out.stdout.trim() }
    let head = what + " exited " + json::to_string(Value::Int(out.code))
    if detail == "" { head } else { head + ": " + detail }
}
