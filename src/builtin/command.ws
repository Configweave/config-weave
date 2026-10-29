// `weave.command` — a readiness probe written as a shell script.
//
// A wait step polls it: exit status 0 means ready, anything else means not
// ready yet. A script that cannot start at all (no interpreter, a bad
// `cwd`) is an error, which the step treats as "not ready" unless it sets
// `fail_on_error`.

use value
use lib

fn ready(params: Value) -> Result[bool, string] {
    let out = lib::run_script(params, lib::param_str(params, "command", ""))?
    Ok(out.success)
}
