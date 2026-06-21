//! Conversions between WCL's `Value` (the engine's "plain data" after WCL
//! evaluation) and wscript's dynamic `DynValue` (script-side `Value`).
//! WCL and wscript never see each other; everything crosses through here.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use wcl_lang::Value as WclValue;
use wscript_std::DynValue;

/// WCL → wscript. Fails on values that have no dynamic representation
/// (functions, tensors, variants, data paths).
pub fn wcl_to_dyn(v: &WclValue) -> Result<DynValue, String> {
    Ok(match v {
        WclValue::Bool(b) => DynValue::Bool(*b),
        WclValue::I8(n) => DynValue::Int(*n as i64),
        WclValue::I16(n) => DynValue::Int(*n as i64),
        WclValue::I32(n) => DynValue::Int(*n as i64),
        WclValue::I64(n) => DynValue::Int(*n),
        WclValue::Isize(n) => DynValue::Int(*n as i64),
        WclValue::U8(n) => DynValue::Int(*n as i64),
        WclValue::U16(n) => DynValue::Int(*n as i64),
        WclValue::U32(n) => DynValue::Int(*n as i64),
        WclValue::Usize(n) => DynValue::Int(
            i64::try_from(*n).map_err(|_| format!("integer {n} exceeds the script range"))?,
        ),
        WclValue::U64(n) => DynValue::Int(
            i64::try_from(*n).map_err(|_| format!("integer {n} exceeds the script range"))?,
        ),
        WclValue::I128(n) => DynValue::Int(
            i64::try_from(*n).map_err(|_| format!("integer {n} exceeds the script range"))?,
        ),
        WclValue::U128(n) => DynValue::Int(
            i64::try_from(*n).map_err(|_| format!("integer {n} exceeds the script range"))?,
        ),
        WclValue::F32(f) => DynValue::Float(*f as f64),
        WclValue::F64(f) => DynValue::Float(*f),
        WclValue::Utf8(s) | WclValue::Ascii(s) => DynValue::String(s.clone()),
        WclValue::Utf16(units) => DynValue::String(String::from_utf16_lossy(units)),
        WclValue::Utf32(chars) => DynValue::String(chars.iter().collect()),
        WclValue::Identifier(s) | WclValue::Symbol(s) => DynValue::String(s.clone()),
        WclValue::None => DynValue::Null,
        WclValue::List(items) => DynValue::List(
            items
                .iter()
                .map(wcl_to_dyn)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        WclValue::Record { fields, .. } => DynValue::Map(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), wcl_to_dyn(v)?)))
                .collect::<Result<HashMap<_, _>, String>>()?,
        ),
        other => {
            return Err(format!(
                "value has no dynamic representation for scripts: {other:?}"
            ));
        }
    })
}

/// wscript → WCL. Maps become anonymous records, so member access
/// (`os.family`) works naturally in playbook expressions.
pub fn dyn_to_wcl(v: &DynValue) -> WclValue {
    match v {
        DynValue::Null => WclValue::None,
        DynValue::Bool(b) => WclValue::Bool(*b),
        DynValue::Int(n) => WclValue::I64(*n),
        DynValue::Float(f) => WclValue::F64(*f),
        DynValue::String(s) => WclValue::Utf8(s.clone()),
        DynValue::List(items) => WclValue::List(Arc::new(items.iter().map(dyn_to_wcl).collect())),
        DynValue::Map(m) => WclValue::Record {
            ty: Vec::new(),
            fields: Arc::new(
                m.iter()
                    .map(|(k, v)| (k.clone(), dyn_to_wcl(v)))
                    .collect::<BTreeMap<_, _>>(),
            ),
        },
    }
}

/// wscript → JSON, for the in-container test protocol (`__gather` output,
/// verify facts files).
pub fn dyn_to_json(v: &DynValue) -> serde_json::Value {
    match v {
        DynValue::Null => serde_json::Value::Null,
        DynValue::Bool(b) => serde_json::Value::Bool(*b),
        DynValue::Int(n) => serde_json::Value::Number((*n).into()),
        DynValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynValue::String(s) => serde_json::Value::String(s.clone()),
        DynValue::List(items) => serde_json::Value::Array(items.iter().map(dyn_to_json).collect()),
        DynValue::Map(m) => {
            serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), dyn_to_json(v))).collect())
        }
    }
}

/// JSON → wscript. Fails on numbers outside the script range.
pub fn json_to_dyn(v: &serde_json::Value) -> Result<DynValue, String> {
    Ok(match v {
        serde_json::Value::Null => DynValue::Null,
        serde_json::Value::Bool(b) => DynValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                DynValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                DynValue::Float(f)
            } else {
                return Err(format!("number {n} exceeds the script range"));
            }
        }
        serde_json::Value::String(s) => DynValue::String(s.clone()),
        serde_json::Value::Array(items) => DynValue::List(
            items
                .iter()
                .map(json_to_dyn)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        serde_json::Value::Object(m) => DynValue::Map(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), json_to_dyn(v)?)))
                .collect::<Result<HashMap<_, _>, String>>()?,
        ),
    })
}

/// Canonical text form of a `DynValue`, used to deduplicate gatherer
/// invocations by `(gatherer, canonicalised params)`. Map keys are sorted.
pub fn canonicalise(v: &DynValue) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &DynValue, out: &mut String) {
    match v {
        DynValue::Null => out.push_str("null"),
        DynValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        DynValue::Int(n) => out.push_str(&n.to_string()),
        DynValue::Float(f) => out.push_str(&format!("{f:?}")),
        DynValue::String(s) => {
            out.push('"');
            out.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
            out.push('"');
        }
        DynValue::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        DynValue::Map(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!("{k:?}:"));
                write_canonical(&m[k], out);
            }
            out.push('}');
        }
    }
}
