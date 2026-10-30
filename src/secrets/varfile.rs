//! `--secret-var-file`: caller-supplied variables marked secret.
//!
//! A caller such as vmlab passes some playbook variables in a file of
//! their own because they are secret. They load exactly like a
//! `--var-file`, and every value joins the redaction set that decrypted
//! `secret("…")` values use, so it is scrubbed everywhere those are:
//! diagnostics, the script log/print bridge, run reports, the event stream
//! and `--log-file`.

use std::path::Path;

use wcl_lang::Value;
use wscript_std::DynValue;

use super::redact;
use crate::diag::Diag;

/// Load a secret var file and register every value it holds for
/// redaction before any of them can reach output.
///
/// A file that fails to load reports where, never what: a parse or
/// evaluation diagnostic renders the offending source line, and that line
/// is the secret.
pub fn load_secret_var_file(path: &Path) -> Result<Vec<(String, Value)>, Vec<Diag>> {
    let vars = crate::engine::vars::load_var_file(path).map_err(|diags| {
        vec![Diag::bare(format!(
            "{}: the secret var file has {} error(s); they are not shown because they \
             would quote its values. Load a copy with placeholder values through \
             --var-file to see them",
            path.display(),
            diags.len()
        ))]
    })?;
    for (name, value) in &vars {
        let dv = crate::convert::wcl_to_dyn(value).map_err(|_| {
            vec![Diag::bare(format!(
                "{}: secret variable '{name}' holds a value that cannot be redacted; use a \
                 string, number, list or map",
                path.display()
            ))]
        })?;
        register_value(&dv);
    }
    Ok(vars)
}

/// Register every string and number inside `v`. Map keys are names, not
/// values, so they stay visible.
fn register_value(v: &DynValue) {
    match v {
        DynValue::String(s) => redact::register(s),
        DynValue::Int(n) => redact::register(&n.to_string()),
        DynValue::Float(f) => redact::register(&f.to_string()),
        DynValue::List(items) => items.iter().for_each(register_value),
        DynValue::Map(m) => m.values().for_each(register_value),
        DynValue::Bool(_) | DynValue::Null => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(src: &str) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), src).unwrap();
        f
    }

    #[test]
    fn every_value_is_registered_for_redaction() {
        let f = write(
            "db_password = \"varfile-probe-pw-1\"\n\
             pin = 918273\n\
             keys = [\"varfile-probe-key-a\", \"varfile-probe-key-b\"]\n\
             creds = { user: \"varfile-probe-user\" }\n",
        );
        let vars = load_secret_var_file(f.path()).unwrap();
        assert_eq!(vars.len(), 4);
        let out = redact::scrub(
            "pw=varfile-probe-pw-1 pin=918273 varfile-probe-key-b varfile-probe-user",
        );
        assert_eq!(out, "pw=*** pin=*** *** ***");
    }

    #[test]
    fn a_broken_file_does_not_quote_its_values() {
        let f = write("token = \"varfile-probe-unclosed\n");
        let diags = load_secret_var_file(f.path()).unwrap_err();
        assert_eq!(diags.len(), 1);
        assert!(!diags[0].message.contains("varfile-probe-unclosed"));
        assert!(!diags[0].rendered.contains("varfile-probe-unclosed"));
        assert!(diags[0].message.contains("not shown"));
    }
}
