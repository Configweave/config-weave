// `weave.execute` — the guarded escape hatch.
//
// Two scripts: a guard that answers "is the host already in the desired
// state?" by its exit status, and an action that gets it there. That split
// is what makes an imperative script convergent, and the engine enforces
// it: after the action runs, the guard runs again, and a guard that is
// still unsatisfied fails the step. A fire-and-forget command cannot pass
// itself off as converged.

use value
use lib

fn check(params: Value) -> Result[CheckResult, string] {
    let out = lib::run_script(params, lib::param_str(params, "check", ""))?
    if out.success { return Ok(CheckResult::AlreadyConfigured) }
    Ok(CheckResult::NotConfigured)
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let out = lib::run_script(params, lib::param_str(params, "run", ""))?
    // A declared reboot status is a success that has not landed yet, so it
    // is checked before the exit status is read as failure.
    if lib::is_reboot_code(params, out.code) { return Ok(ApplyResult::RebootRequired) }
    if out.success { return Ok(ApplyResult::Success) }
    Err(lib::failure("the action script", out))
}
