//! The `template` module (PRD §7 extension). Renders a Tera template
//! string against a map of variables, on the target host, at apply time.
//!
//! This is the engine behind `linux_files.template`: a template body and a
//! `vars` map are passed in, Tera renders them (autoescape off — these are
//! config files, not HTML), and the result is written to disk. WCL-side
//! `$"…"` interpolation still works for simple cases; this covers the
//! cases WCL's `map`/`join` handles awkwardly (`{% for %}`, `{% if %}`,
//! filters) and lets template bodies live in `vars`-driven loops.

use std::error::Error;

use wscript::Module;
use wscript_std::DynValue;

/// Convert a `DynValue` into a `serde_json::Value` so it can seed a Tera
/// context. Mirrors `wscript-std`'s `json::to_json` (which is crate-private).
fn to_json(v: &DynValue) -> serde_json::Value {
    match v {
        DynValue::Null => serde_json::Value::Null,
        DynValue::Bool(b) => serde_json::Value::Bool(*b),
        DynValue::Int(n) => serde_json::Value::Number((*n).into()),
        DynValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynValue::String(s) => serde_json::Value::String(s.clone()),
        DynValue::List(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        DynValue::Map(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), to_json(v)))
                .collect(),
        ),
    }
}

/// Flatten a `tera::Error` and its source chain into one legible message,
/// so parse errors and undefined-variable errors read clearly.
fn tera_error(err: tera::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut src = err.source();
    while let Some(e) = src {
        parts.push(e.to_string());
        src = e.source();
    }
    parts.join(": ")
}

fn render(template: &str, vars: &DynValue) -> Result<String, String> {
    let context = match vars {
        DynValue::Map(_) => tera::Context::from_serialize(to_json(vars))
            .map_err(|e| format!("invalid template vars: {}", tera_error(e)))?,
        DynValue::Null => tera::Context::new(),
        other => {
            return Err(format!(
                "template vars must be a map, got {}",
                match other {
                    DynValue::Bool(_) => "bool",
                    DynValue::Int(_) => "int",
                    DynValue::Float(_) => "float",
                    DynValue::String(_) => "string",
                    DynValue::List(_) => "list",
                    _ => "value",
                }
            ));
        }
    };
    tera::Tera::one_off(template, &context, false).map_err(tera_error)
}

pub fn module() -> Module {
    let mut m = Module::new("template");
    m.doc("Render Tera templates against a variable map (autoescape off)");

    m.doc_next("Render a Tera template string with a map of variables");
    m.fn_(
        "render",
        |template: &str, vars: DynValue| -> Result<String, String> { render(template, &vars) },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, DynValue)]) -> DynValue {
        DynValue::Map(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn substitutes_a_scalar() {
        let out = render(
            "hello {{ name }}",
            &map(&[("name", DynValue::String("world".into()))]),
        )
        .unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn iterates_a_list() {
        let hosts = DynValue::List(vec![
            DynValue::String("a".into()),
            DynValue::String("b".into()),
        ]);
        let out = render(
            "{% for h in hosts %}server {{ h }}\n{% endfor %}",
            &map(&[("hosts", hosts)]),
        )
        .unwrap();
        assert_eq!(out, "server a\nserver b\n");
    }

    #[test]
    fn join_filter_is_available() {
        let hosts = DynValue::List(vec![
            DynValue::String("a".into()),
            DynValue::String("b".into()),
        ]);
        let out = render("{{ hosts | join(sep=\",\") }}", &map(&[("hosts", hosts)])).unwrap();
        assert_eq!(out, "a,b");
    }

    #[test]
    fn null_vars_render_an_empty_context() {
        let out = render("static line", &DynValue::Null).unwrap();
        assert_eq!(out, "static line");
    }

    #[test]
    fn non_map_vars_error() {
        let err = render("x", &DynValue::Int(3)).unwrap_err();
        assert!(err.contains("must be a map"), "got: {err}");
    }

    #[test]
    fn parse_error_surfaces() {
        let err = render("{% for %}", &DynValue::Null).unwrap_err();
        assert!(!err.is_empty());
    }
}
