//! The `data` module (PRD §7). JSON and TOML are wisp-std modules
//! (`use json`, `use toml`) registered as-is — re-exported, not
//! duplicated, per the PRD's overlap note. This module adds INI, which
//! wisp-std does not cover.
//!
//! INI maps to a `Value` map of sections: keys before any `[section]`
//! header land under `""`. All INI values are strings.

use std::collections::HashMap;

use wisp::Module;
use wisp_std::DynValue;

fn ini_parse(text: &str) -> Result<DynValue, String> {
    let mut sections: HashMap<String, DynValue> = HashMap::new();
    let mut current = String::new();
    let mut current_map: HashMap<String, DynValue> = HashMap::new();

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[') {
            let Some(name) = name.strip_suffix(']') else {
                return Err(format!("line {}: unterminated section header", lineno + 1));
            };
            sections.insert(
                std::mem::take(&mut current),
                DynValue::Map(std::mem::take(&mut current_map)),
            );
            current = name.trim().to_string();
        } else if let Some((key, value)) = line.split_once('=') {
            current_map.insert(
                key.trim().to_string(),
                DynValue::String(value.trim().to_string()),
            );
        } else {
            return Err(format!(
                "line {}: expected key=value or [section]",
                lineno + 1
            ));
        }
    }
    sections.insert(current, DynValue::Map(current_map));
    // Drop an empty global section for cleanliness.
    if let Some(DynValue::Map(m)) = sections.get("")
        && m.is_empty()
    {
        sections.remove("");
    }
    Ok(DynValue::Map(sections))
}

fn ini_serialize(value: &DynValue) -> Result<String, String> {
    let DynValue::Map(sections) = value else {
        return Err("ini data must be a map of sections".to_string());
    };
    let mut out = String::new();
    let mut names: Vec<&String> = sections.keys().collect();
    names.sort();
    // Global section first.
    names.sort_by_key(|n| !n.is_empty());
    for name in names {
        let DynValue::Map(entries) = &sections[name] else {
            return Err(format!("section '{name}' must be a map"));
        };
        if !name.is_empty() {
            out.push_str(&format!("[{name}]\n"));
        }
        let mut keys: Vec<&String> = entries.keys().collect();
        keys.sort();
        for key in keys {
            let value = match &entries[key] {
                DynValue::String(s) => s.clone(),
                DynValue::Int(n) => n.to_string(),
                DynValue::Float(f) => f.to_string(),
                DynValue::Bool(b) => b.to_string(),
                other => {
                    return Err(format!(
                        "section '{name}' key '{key}': {other:?} is not an ini value"
                    ));
                }
            };
            out.push_str(&format!("{key}={value}\n"));
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_string() + "\n")
}

pub fn module() -> Module {
    let mut m = Module::new("data");
    m.doc("INI parsing/serialization (JSON and TOML live in the json/toml modules)");

    m.doc_next("Parse INI text into a map of sections (global keys under \"\")");
    m.fn_("ini_parse", |text: &str| -> Result<DynValue, String> {
        ini_parse(text)
    });
    m.doc_next("Serialize a map of sections to INI text");
    m.fn_(
        "ini_serialize",
        |value: DynValue| -> Result<String, String> { ini_serialize(&value) },
    );
    m
}
