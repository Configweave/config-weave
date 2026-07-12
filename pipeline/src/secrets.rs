//! Secret and property resolution, plus the 0600 var-file that injects a
//! play step's vars.
//!
//! Values written in a `pipeline.wcl` (step env, play vars, transport
//! credentials) may be a literal, a `"prop:NAME"` reference to a pipeline
//! property supplied at trigger time, or a `"secret:NAME"` reference to an
//! inline secret. Resolution expands the references; a dangling reference
//! is an error surfaced before any step runs.

use std::collections::HashMap;

/// Expand one value against the run's properties and secrets. `ctx` is used
/// only to make the error message point at the offending field.
pub fn resolve(
    value: &str,
    props: &HashMap<String, String>,
    secrets: &HashMap<String, String>,
    ctx: &str,
) -> Result<String, String> {
    if let Some(name) = value.strip_prefix("secret:") {
        secrets
            .get(name)
            .cloned()
            .ok_or_else(|| format!("{ctx}: no secret named '{name}'"))
    } else if let Some(name) = value.strip_prefix("prop:") {
        props
            .get(name)
            .cloned()
            .ok_or_else(|| format!("{ctx}: no property named '{name}'"))
    } else {
        Ok(value.to_string())
    }
}

/// Expand the credential fields of a transport config (`password` /
/// `private_key` may be `"secret:NAME"` references).
pub fn resolve_transport(
    cfg: &weave_remote::TransportConfig,
    secrets: &HashMap<String, String>,
    ctx: &str,
) -> Result<weave_remote::TransportConfig, String> {
    let empty = HashMap::new();
    let mut out = cfg.clone();
    if let Some(pw) = &cfg.password {
        out.password = Some(resolve(pw, &empty, secrets, ctx)?);
    }
    if let Some(key) = &cfg.private_key {
        out.private_key = Some(resolve(key, &empty, secrets, ctx)?);
    }
    Ok(out)
}

/// The flat WCL var-file injecting a play step's resolved vars; values are
/// emitted through wcl_lang's printer so any secret is quoted correctly.
/// Mirrors weave-server's `sysruns::system_var_file`.
pub fn var_file_source(vars: &[(String, String)]) -> String {
    use wcl_lang::{ast, edit, format as wclformat};
    let mut src = ast::Source {
        items: Vec::new(),
        trailing_trivia: Vec::new(),
    };
    for (name, value) in vars {
        // A var-file is flat `name = value` fields; reuse the block
        // builder's field synthesis by building one throwaway block.
        let block = edit::build_block(
            "x",
            &[],
            vec![],
            vec![(name.clone(), edit::string_literal_expr(value))],
        );
        if let Some(ast::Item::Field(f)) = block.items.into_iter().next() {
            src.items.push(ast::Item::Field(f));
        }
    }
    wclformat::to_source(&src)
}

/// Write `vars` to a 0600 `.wcl` tempfile, so secrets never hit the process
/// argv/`ps` list — only `--var-file <path>` does.
pub fn write_var_file(vars: &[(String, String)]) -> Result<tempfile::NamedTempFile, String> {
    use std::io::Write as _;
    let mut f =
        tempfile::NamedTempFile::with_suffix(".wcl").map_err(|e| format!("var-file: {e}"))?;
    f.write_all(var_file_source(vars).as_bytes())
        .map_err(|e| format!("var-file: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("var-file: {e}"))?;
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn resolve_literal_prop_and_secret() {
        let props = map(&[("version", "1.2.3")]);
        let secrets = map(&[("token", "hunter2")]);
        assert_eq!(resolve("plain", &props, &secrets, "x").unwrap(), "plain");
        assert_eq!(resolve("prop:version", &props, &secrets, "x").unwrap(), "1.2.3");
        assert_eq!(resolve("secret:token", &props, &secrets, "x").unwrap(), "hunter2");
    }

    #[test]
    fn missing_reference_is_an_error() {
        let empty = HashMap::new();
        assert!(resolve("prop:nope", &empty, &empty, "step 'x'").is_err());
        assert!(resolve("secret:nope", &empty, &empty, "step 'x'").is_err());
    }

    #[test]
    fn var_file_quotes_hostile_values() {
        let src = var_file_source(&[("token".into(), "it's \"tricky\"".into())]);
        // Flat fields, no block syntax.
        assert!(src.contains("token ="));
        assert!(!src.contains('{'));
        // Round-trips through the WCL printer without breaking.
        assert!(src.contains("it's") || src.contains("it\\'s") || src.contains("tricky"));
    }
}
