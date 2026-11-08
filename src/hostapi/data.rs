//! The `data` module (PRD §7). JSON and TOML are wscript-std modules
//! (`use json`, `use toml`) registered as-is — re-exported, not
//! duplicated, per the PRD's overlap note. This module adds INI, which
//! wscript-std does not cover.
//!
//! INI maps to a `Value` map of sections: keys before any `[section]`
//! header land under `""`. All INI values are strings.

use std::collections::HashMap;

use wscript::Module;
use wscript_std::DynValue;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The string value of `key` in section `section`, or None.
    fn get(v: &DynValue, section: &str, key: &str) -> Option<String> {
        let DynValue::Map(sections) = v else {
            return None;
        };
        let DynValue::Map(entries) = sections.get(section)? else {
            return None;
        };
        match entries.get(key)? {
            DynValue::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn section_names(v: &DynValue) -> Vec<String> {
        let DynValue::Map(sections) = v else {
            return Vec::new();
        };
        let mut names: Vec<String> = sections.keys().cloned().collect();
        names.sort();
        names
    }

    #[test]
    fn repeated_section_merges_keys() {
        let v = ini_parse("[a]\nx=1\n[b]\ny=2\n[a]\nz=3\n").unwrap();
        assert_eq!(get(&v, "a", "x").as_deref(), Some("1"));
        assert_eq!(get(&v, "a", "z").as_deref(), Some("3"));
        assert_eq!(get(&v, "b", "y").as_deref(), Some("2"));
        assert_eq!(section_names(&v), vec!["a", "b"]);
    }

    #[test]
    fn repeated_section_later_key_wins() {
        let v = ini_parse("[a]\nx=1\n[a]\nx=2\n").unwrap();
        assert_eq!(get(&v, "a", "x").as_deref(), Some("2"));
    }

    #[test]
    fn global_keys_go_under_empty_name() {
        let v = ini_parse("g=1\n[a]\nx=2\n").unwrap();
        assert_eq!(get(&v, "", "g").as_deref(), Some("1"));
        assert_eq!(get(&v, "a", "x").as_deref(), Some("2"));
    }

    #[test]
    fn empty_global_section_is_dropped() {
        let v = ini_parse("[a]\nx=2\n").unwrap();
        assert_eq!(section_names(&v), vec!["a"]);
    }

    #[test]
    fn comment_lines_are_skipped() {
        let v = ini_parse("; top\n# also\n[a]\n; inside\nx=1\n  # indented\n").unwrap();
        assert_eq!(section_names(&v), vec!["a"]);
        assert_eq!(get(&v, "a", "x").as_deref(), Some("1"));
    }

    #[test]
    fn unterminated_header_names_its_line() {
        let err = ini_parse("[a]\nx=1\n[b\ny=2\n").unwrap_err();
        assert_eq!(err, "line 3: unterminated section header");
    }

    #[test]
    fn line_without_equals_is_rejected_with_line() {
        let err = ini_parse("[a]\njunk\n").unwrap_err();
        assert_eq!(err, "line 2: expected key=value or [section]");
    }

    #[test]
    fn serialize_non_string_scalars() {
        let mut entries = HashMap::new();
        entries.insert("n".to_string(), DynValue::Int(42));
        entries.insert("f".to_string(), DynValue::Float(1.5));
        entries.insert("b".to_string(), DynValue::Bool(true));
        let mut sections = HashMap::new();
        sections.insert("s".to_string(), DynValue::Map(entries));
        let text = ini_serialize(&DynValue::Map(sections)).unwrap();
        assert_eq!(text, "[s]\nb=true\nf=1.5\nn=42\n");
    }

    #[test]
    fn serialize_rejects_nested_value() {
        let mut inner = HashMap::new();
        inner.insert("k".to_string(), DynValue::Map(HashMap::new()));
        let mut sections = HashMap::new();
        sections.insert("s".to_string(), DynValue::Map(inner));
        assert!(ini_serialize(&DynValue::Map(sections)).is_err());
    }

    #[test]
    fn serialize_writes_global_first_sorted_without_comments() {
        let v = ini_parse("; c\nz=9\n[b]\ny=2\nx=1\n[a]\nq=0\n").unwrap();
        assert_eq!(
            ini_serialize(&v).unwrap(),
            "z=9\n\n[a]\nq=0\n\n[b]\nx=1\ny=2\n"
        );
    }

    #[test]
    fn parse_serialize_parse_round_trip() {
        let src = "g=1\n[a]\nx=2\n[b]\ny=hello world\n[a]\nz=3\n";
        let first = ini_serialize(&ini_parse(src).unwrap()).unwrap();
        let second = ini_serialize(&ini_parse(&first).unwrap()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first, "g=1\n\n[a]\nx=2\nz=3\n\n[b]\ny=hello world\n");
    }
}
