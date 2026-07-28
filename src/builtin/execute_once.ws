// `weave.execute_once` — a migration crutch, and labelled as one.
//
// One script, run once per host, with a record kept so it never runs
// again. That record is the only persistent state config-weave owns
// (PRD §17 says the design is otherwise deliberately stateless), and it
// exists for exactly one job: letting a playbook adopt a pile of existing
// shell scripts without rewriting them all first. Each one converted to a
// real resource — or to `weave.execute` with a genuine guard — is one less
// step that depends on this.
//
// The record is keyed by `id` alone, never by the script's text: editing
// `run` afterwards does *not* run it again. That is the point of the
// resource, not an oversight, so the record also stores a hash of what
// actually ran, for the day someone has to work out what a host got.
//
//   Linux/macOS   /var/lib/config-weave/once/<id>
//   Windows       HKLM\Software\config-weave\Once, value <id>
//
// $CONFIG_WEAVE_STATE_DIR overrides the root on either platform, and
// selects the file form on Windows too — which is what makes the resource
// testable without touching the real machine.

use value
use fs
use path
use hash
use json
use time
use env
use sys
use registry
use lib

// Where the Windows record lives. A function, because wscript `const`
// items belong to interface files only.
fn registry_key() -> string { "HKLM\\Software\\config-weave\\Once" }

// The id names a file and a registry value, so it has to be free of path
// separators, traversal and the characters neither will take.
fn checked_id(params: Value) -> Result[string, string] {
    let id = lib::param_str(params, "id", "")
    if id == "" { return Err("'id' must not be empty") }
    if id.contains("/") || id.contains("\\") || id.contains("..") {
        return Err("'id' must not contain '/', '\\' or '..': it names a file and a registry value")
    }
    if id.contains(":") || id.contains("*") || id.contains("?") || id.contains("\"") {
        return Err("'id' must not contain ':', '*', '?' or '\"'")
    }
    Ok(id)
}

// The file form is used everywhere except Windows, and on Windows too once
// the state root has been overridden.
fn state_root() -> Option[string] {
    if let Some(dir) = env::get("CONFIG_WEAVE_STATE_DIR") {
        if dir != "" { return Some(dir) }
    }
    if sys::family() == "windows" { return None }
    Some("/var/lib/config-weave")
}

fn stamp_path(root: string, id: string) -> string {
    path::join(path::join(root, "once"), id)
}

fn already_ran(params: Value) -> Result[bool, string] {
    let id = checked_id(params)?
    if let Some(root) = state_root() {
        return Ok(fs::exists(stamp_path(root, id)))
    }
    if let Some(_v) = registry::read(registry_key(), id)? { return Ok(true) }
    Ok(false)
}

// Recorded after the script succeeds: when it ran, and the digest of what
// ran, so a later "why does this host look like that?" has something to go
// on even though the digest never gates anything.
fn record(params: Value) -> Result[unit, string] {
    let id = checked_id(params)?
    let digest = hash::sha256(lib::param_str(params, "run", ""))
    let stamp = time::format_iso(time::now_unix()) + " sha256=" + digest + "\n"
    if let Some(root) = state_root() {
        let dir = path::join(root, "once")
        fs::mkdir(dir)?
        return fs::write(stamp_path(root, id), stamp)
    }
    registry::create_key(registry_key())?
    registry::write(registry_key(), id, Value::String(stamp.trim()), "sz")
}

fn check(params: Value) -> Result[CheckResult, string] {
    if already_ran(params)? { return Ok(CheckResult::AlreadyConfigured) }
    Ok(CheckResult::NotConfigured)
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let out = lib::run_script(params, lib::param_str(params, "run", ""))?
    let reboot = lib::is_reboot_code(params, out.code)
    if !out.success && !reboot {
        return Err(lib::failure("the script", out))
    }
    // Recorded before the reboot is reported, so the reboot itself cannot
    // cause the script to run a second time.
    record(params)?
    if reboot { return Ok(ApplyResult::RebootRequired) }
    Ok(ApplyResult::Success)
}
