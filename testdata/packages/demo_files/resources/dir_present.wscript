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
    if fs::is_dir(p) {
        Ok(CheckResult::AlreadyConfigured)
    } else {
        Ok(CheckResult::NotConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let p = param_str(params, "path", "")
    log::info("creating directory " + p)
    fs::mkdir(p)?
    Ok(ApplyResult::Success)
}
