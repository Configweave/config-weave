use value
use fs
use log

fn param_str(params: Value, key: string, fallback: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() {
            return s
        }
    }
    fallback
}

fn check(params: Value) -> Result[CheckResult, string] {
    let p = param_str(params, "path", "")
    if p == "" {
        return Err("missing 'path' parameter")
    }
    if fs::exists(p) {
        Ok(CheckResult::NotConfigured)
    } else {
        Ok(CheckResult::AlreadyConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let p = param_str(params, "path", "")
    log::info("removing " + p)
    fs::delete(p)?
    Ok(ApplyResult::Success)
}
