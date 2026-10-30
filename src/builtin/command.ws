// `weave.command` — a readiness probe written as a shell script.
//
// A wait step polls it: exit status 0 means ready, anything else means not
// ready yet. A script that cannot start at all (no interpreter, a bad
// `cwd`) is an error, which the step treats as "not ready" unless it sets
// `fail_on_error`.
//
// So are the shell's own 126 (found but not executable) and 127 (not
// found). Without that, a typo in `command` reads as an upstream that is
// not up yet: `check` looks normal and `apply` waits out its whole timeout
// with nothing pointing at the cause. As an error it carries the shell's
// message into `check`, the timeout report and `fail_on_error`.

use value
use lib

fn ready(params: Value) -> Result[bool, string] {
    let out = lib::run_script(params, lib::param_str(params, "command", ""))?
    if out.code == 126 || out.code == 127 {
        return Err(lib::failure("the probe command", out))
    }
    Ok(out.success)
}
