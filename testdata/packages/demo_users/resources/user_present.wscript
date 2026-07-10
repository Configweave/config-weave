use value
use fs
use shell
use log

fn param_str(params: Value, key: string, fallback: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() {
            return s
        }
    }
    fallback
}

fn user_exists(name: string) -> Result[bool, string] {
    let passwd = fs::read("/etc/passwd")?
    for line in passwd.split("\n") {
        if line.starts_with(name + ":") {
            return Ok(true)
        }
    }
    Ok(false)
}

fn check(params: Value) -> Result[CheckResult, string] {
    let name = param_str(params, "name", "")
    if name == "" {
        return Err("missing 'name' parameter")
    }
    if user_exists(name)? {
        Ok(CheckResult::AlreadyConfigured)
    } else {
        Ok(CheckResult::NotConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let name = param_str(params, "name", "")
    let sh = param_str(params, "shell", "/usr/sbin/nologin")
    log::info("creating user " + name)
    let out = shell::run("useradd --system --shell " + sh + " " + name, Value::Null)?
    if !out.success {
        return Err("useradd failed: " + out.stderr)
    }
    Ok(ApplyResult::Success)
}
