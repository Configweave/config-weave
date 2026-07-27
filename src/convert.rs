//! Conversions between WCL's `Value` (the engine's "plain data" after WCL
//! evaluation) and wscript's dynamic `DynValue` (script-side `Value`).
//! WCL and wscript never see each other; everything crosses through here.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use wcl_lang::{EvalError, Field, Value as WclValue};
use wscript_std::DynValue;

use crate::model::DURATION_TYPE;

/// Whether a WCL value was written as a `:symbol` (or a bare identifier,
/// which lexes the same way). Both spellings convert to `DynValue::String`,
/// so this is the only place the distinction survives.
pub fn is_symbol_literal(v: &WclValue) -> bool {
    matches!(v, WclValue::Symbol(_) | WclValue::Identifier(_))
}

/// One property / param field, evaluated and converted for scripts.
pub struct FieldValue {
    pub value: DynValue,
    /// Whether the source spelling was `:symbol` — see [`is_symbol_literal`].
    pub symbol_literal: bool,
}

/// Why a field's value isn't available.
pub enum FieldValueError {
    /// The value references a variable, which only binds after the gather
    /// phase. Validation defers these to run time; at run time an
    /// unresolved reference is a real failure, so the error is carried.
    Unresolved(EvalError),
    /// WCL evaluation failed.
    Eval(EvalError),
    /// The value evaluated but has no script representation.
    Convert(String),
}

/// A property / param field's value as a `DynValue`.
///
/// Config Weave's `properties` / `params` blocks are `@schemaless`, so a
/// bare unit literal (`max_age = 30min`) has no declared type to resolve
/// against and plain `value()` can only report `UnitWithoutType`. Retry
/// those against `std.Duration`, yielding base nanoseconds. A literal from
/// another unit family (`4GiB`) fails that retry, and reports the original
/// error rather than a confusing duration one.
pub fn field_value_dyn(f: &Field<'_>) -> Result<FieldValue, FieldValueError> {
    let convert = |v: &WclValue| {
        wcl_to_dyn(v)
            .map(|dv| FieldValue {
                value: dv,
                symbol_literal: is_symbol_literal(v),
            })
            .map_err(FieldValueError::Convert)
    };
    match f.value() {
        Ok(v) => convert(v),
        Err(e @ EvalError::UnresolvedReference { .. }) => {
            Err(FieldValueError::Unresolved(e.clone()))
        }
        Err(original) => {
            if matches!(original, EvalError::UnitWithoutType { .. })
                && let Ok(v) = f.value_typed(DURATION_TYPE)
            {
                return convert(&v);
            }
            Err(FieldValueError::Eval(original.clone()))
        }
    }
}

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

/// `dyn_to_wcl` for a gatherer's result, honouring its `returns`
/// declarations: a top-level key declared `type = "symbol"` binds as a real
/// `Value::Symbol`, so a playbook compares it the way it was declared
/// (`init.init == :systemd`) and interpolates it as `:systemd`.
///
/// Only top-level keys are typed — nested maps stay plain data, matching
/// the fact that `returns` documents exactly one level.
pub fn dyn_to_wcl_returns(v: &DynValue, returns: &[crate::model::ReturnDecl]) -> WclValue {
    let DynValue::Map(m) = v else {
        return dyn_to_wcl(v);
    };
    WclValue::Record {
        ty: Vec::new(),
        fields: Arc::new(
            m.iter()
                .map(|(k, val)| {
                    let symbol = returns
                        .iter()
                        .any(|r| &r.name == k && r.ty == crate::model::CoarseType::Symbol);
                    match (symbol, val) {
                        (true, DynValue::String(s)) => (k.clone(), WclValue::Symbol(s.clone())),
                        _ => (k.clone(), dyn_to_wcl(val)),
                    }
                })
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

/// Every declared symbol set a gatherer's result violates, as ready-made
/// diagnostic bodies. A key the gatherer didn't return isn't a violation —
/// `returns` doesn't require its keys to be present.
pub fn returns_symbol_violations(
    v: &DynValue,
    returns: &[crate::model::ReturnDecl],
) -> Vec<String> {
    let DynValue::Map(m) = v else {
        return Vec::new();
    };
    returns
        .iter()
        .filter(|r| r.ty == crate::model::CoarseType::Symbol)
        .filter_map(|r| {
            let got = m.get(&r.name)?;
            let why = r.symbol_violation(got)?;
            Some(format!("key '{}' is not a declared symbol: {why}", r.name))
        })
        .collect()
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
