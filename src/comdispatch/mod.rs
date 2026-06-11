//! Late-bound COM dispatch (PRD §7): everything goes through
//! `IDispatch::Invoke` — the same path VBScript/JScript/PowerShell use —
//! plus VARIANT ↔ value marshalling. Windows-only; the `com` host module
//! holds the cross-platform surface.
//!
//! A VARIANT either carries plain data (→ `DynValue`) or an object
//! (VT_DISPATCH → a nested `ComObject`); [`ComValue`] keeps the two apart
//! so the host module can route them to differently-typed script methods
//! (`get` vs `get_object`, …).
#![cfg(windows)]

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance, CoGetObject, CoInitializeEx,
    CoUninitialize, COINIT_APARTMENTTHREADED, DISPATCH_FLAGS, DISPATCH_METHOD,
    DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, IDispatch, SAFEARRAY,
};
use windows::Win32::System::Ole::{
    DISPID_NEWENUM, DISPID_PROPERTYPUT, GetActiveObject, IEnumVARIANT, SafeArrayCreateVector,
    SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayPutElement,
};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VariantClear, VT_ARRAY, VT_BOOL,
    VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_I1, VT_I2, VT_I4, VT_I8, VT_INT, VT_NULL, VT_R4, VT_R8,
    VT_UI1, VT_UI2, VT_UI4, VT_UI8, VT_UINT, VT_UNKNOWN, VT_VARIANT,
};
use windows::core::{BSTR, GUID, HSTRING, IUnknown, Interface, PCWSTR};

use wisp_std::DynValue;

/// A marshalled COM result: plain data or a nested object.
pub enum ComValue {
    Data(DynValue),
    Object(IDispatch),
}

/// Initialise COM (STA) on the current thread. Returns a guard that
/// uninitialises on drop; the engine holds one per worker thread.
pub struct ComInit {
    ok: bool,
}

pub fn init_sta() -> ComInit {
    // S_FALSE (already initialised) still requires a matching
    // CoUninitialize; RPC_E_CHANGED_MODE does not.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    ComInit { ok: hr.is_ok() }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.ok {
            unsafe { CoUninitialize() };
        }
    }
}

fn hr_err(context: &str, e: windows::core::Error) -> String {
    format!("{context}: {e}")
}

/// CLSIDFromProgID + CoCreateInstance → IDispatch.
pub fn create(progid: &str) -> Result<IDispatch, String> {
    let wide = HSTRING::from(progid);
    let clsid: GUID =
        unsafe { windows::Win32::System::Com::CLSIDFromProgID(PCWSTR(wide.as_ptr())) }
            .map_err(|e| hr_err(&format!("no COM class '{progid}'"), e))?;
    unsafe {
        CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER)
            .map_err(|e| hr_err(&format!("creating '{progid}'"), e))
    }
}

/// GetObject semantics: a moniker (`winmgmts:…`) or a running object.
pub fn get_object(name: &str) -> Result<IDispatch, String> {
    let wide = HSTRING::from(name);
    // The moniker path covers winmgmts:, file monikers, …
    let by_moniker: Result<IDispatch, _> = unsafe { CoGetObject(PCWSTR(wide.as_ptr()), None) };
    if let Ok(disp) = by_moniker {
        return Ok(disp);
    }
    // Fall back to the running object table via ProgID.
    let clsid: GUID =
        unsafe { windows::Win32::System::Com::CLSIDFromProgID(PCWSTR(wide.as_ptr())) }
            .map_err(|e| hr_err(&format!("GetObject('{name}')"), e))?;
    let mut unk: Option<IUnknown> = None;
    unsafe { GetActiveObject(&clsid, None, &mut unk) }
        .map_err(|e| hr_err(&format!("GetObject('{name}')"), e))?;
    let unk = unk.ok_or_else(|| format!("GetObject('{name}'): no active object"))?;
    unk.cast::<IDispatch>()
        .map_err(|e| hr_err(&format!("GetObject('{name}')"), e))
}

fn dispid(disp: &IDispatch, name: &str) -> Result<i32, String> {
    let wide = HSTRING::from(name);
    let mut id = 0i32;
    unsafe {
        disp.GetIDsOfNames(&GUID::zeroed(), &PCWSTR(wide.as_ptr()), 1, 0, &mut id)
            .map_err(|e| hr_err(&format!("member '{name}' not found"), e))?;
    }
    Ok(id)
}

fn invoke_raw(
    disp: &IDispatch,
    id: i32,
    name: &str,
    flags: DISPATCH_FLAGS,
    args: &[DynValue],
) -> Result<VARIANT, String> {
    let mut variants: Vec<VARIANT> = args
        .iter()
        .rev()
        .map(dyn_to_variant)
        .collect::<Result<Vec<_>, _>>()?;
    let mut named_dispid = DISPID_PROPERTYPUT;
    let params = DISPPARAMS {
        rgvarg: if variants.is_empty() {
            std::ptr::null_mut()
        } else {
            variants.as_mut_ptr()
        },
        rgdispidNamedArgs: if flags == DISPATCH_PROPERTYPUT {
            &mut named_dispid
        } else {
            std::ptr::null_mut()
        },
        cArgs: variants.len() as u32,
        cNamedArgs: if flags == DISPATCH_PROPERTYPUT { 1 } else { 0 },
    };
    let mut result = VARIANT::default();
    let outcome = unsafe {
        disp.Invoke(
            id,
            &GUID::zeroed(),
            0,
            flags,
            &params,
            Some(&mut result),
            None,
            None,
        )
    };
    for v in &mut variants {
        unsafe {
            let _ = VariantClear(v);
        }
    }
    outcome.map_err(|e| hr_err(&format!("invoking '{name}'"), e))?;
    Ok(result)
}

/// Invoke a member with marshalled args, marshalling the result back.
pub fn invoke(
    disp: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    args: &[DynValue],
) -> Result<ComValue, String> {
    let id = dispid(disp, name)?;
    let mut result = invoke_raw(disp, id, name, flags, args)?;
    let out = variant_to_com(&result);
    unsafe {
        let _ = VariantClear(&mut result);
    }
    out
}

pub fn get_property(disp: &IDispatch, name: &str) -> Result<ComValue, String> {
    invoke(disp, name, DISPATCH_PROPERTYGET, &[])
}

pub fn set_property(disp: &IDispatch, name: &str, value: &DynValue) -> Result<(), String> {
    invoke(disp, name, DISPATCH_PROPERTYPUT, std::slice::from_ref(value)).map(|_| ())
}

pub fn call_method(disp: &IDispatch, name: &str, args: &[DynValue]) -> Result<ComValue, String> {
    invoke(disp, name, DISPATCH_METHOD, args)
}

/// Enumerate a COM collection via DISPID_NEWENUM / IEnumVARIANT.
pub fn enumerate(disp: &IDispatch) -> Result<Vec<ComValue>, String> {
    let mut result = invoke_raw(
        disp,
        DISPID_NEWENUM,
        "_NewEnum",
        DISPATCH_METHOD | DISPATCH_PROPERTYGET,
        &[],
    )?;
    let enumerator: Result<IEnumVARIANT, String> = unsafe {
        let vt = result.Anonymous.Anonymous.vt;
        let unk: Option<IUnknown> = if vt == VT_UNKNOWN || vt == VT_DISPATCH {
            (*result.Anonymous.Anonymous.Anonymous.punkVal).clone()
        } else {
            None
        };
        match unk {
            Some(u) => u
                .cast::<IEnumVARIANT>()
                .map_err(|e| hr_err("collection enumerator", e)),
            None => Err("collection has no enumerator".to_string()),
        }
    };
    unsafe {
        let _ = VariantClear(&mut result);
    }
    let enumerator = enumerator?;

    let mut items = Vec::new();
    loop {
        let mut chunk = [VARIANT::default()];
        let mut fetched = 0u32;
        let hr = unsafe { enumerator.Next(&mut chunk, &mut fetched) };
        if hr.is_err() || fetched == 0 {
            break;
        }
        let item = variant_to_com(&chunk[0]);
        unsafe {
            let _ = VariantClear(&mut chunk[0]);
        }
        items.push(item?);
    }
    Ok(items)
}

/// WMI sugar (PRD §7): query root\cimv2, eagerly flattening each result
/// object into a map of its properties.
pub fn wmi_query(query: &str) -> Result<DynValue, String> {
    let services = get_object("winmgmts:{impersonationLevel=impersonate}!\\\\.\\root\\cimv2")?;
    let set = match call_method(
        &services,
        "ExecQuery",
        &[DynValue::String(query.to_string())],
    )? {
        ComValue::Object(o) => o,
        ComValue::Data(other) => {
            return Err(format!("ExecQuery returned unexpected data: {other:?}"));
        }
    };
    let mut rows = Vec::new();
    for item in enumerate(&set)? {
        let ComValue::Object(obj) = item else {
            continue;
        };
        rows.push(DynValue::Map(wbem_properties(&obj)?));
    }
    Ok(DynValue::List(rows))
}

/// Read an SWbemObject's Properties_ collection into a map.
fn wbem_properties(
    obj: &IDispatch,
) -> Result<std::collections::HashMap<String, DynValue>, String> {
    let props = match get_property(obj, "Properties_")? {
        ComValue::Object(p) => p,
        ComValue::Data(_) => return Err("Properties_ is not an object".to_string()),
    };
    let mut map = std::collections::HashMap::new();
    for prop in enumerate(&props)? {
        let ComValue::Object(p) = prop else { continue };
        let name = match get_property(&p, "Name")? {
            ComValue::Data(DynValue::String(s)) => s,
            _ => continue,
        };
        let value = match get_property(&p, "Value") {
            Ok(ComValue::Data(v)) => v,
            // Nested objects / unsupported VTs render as null in rows.
            _ => DynValue::Null,
        };
        map.insert(name, value);
    }
    Ok(map)
}

// --------------------------------------------------------- marshalling

fn make_variant(vt: VARENUM, data: VARIANT_0_0_0) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: data,
            }),
        },
    }
}

/// DynValue → VARIANT. Maps have no VARIANT representation. The caller
/// owns the result and must `VariantClear` it.
pub fn dyn_to_variant(v: &DynValue) -> Result<VARIANT, String> {
    Ok(match v {
        DynValue::Null => VARIANT::default(),
        DynValue::Bool(b) => make_variant(
            VT_BOOL,
            VARIANT_0_0_0 {
                boolVal: VARIANT_BOOL(if *b { -1 } else { 0 }),
            },
        ),
        DynValue::Int(n) => {
            if let Ok(small) = i32::try_from(*n) {
                make_variant(VT_I4, VARIANT_0_0_0 { lVal: small })
            } else {
                make_variant(VT_I8, VARIANT_0_0_0 { llVal: *n })
            }
        }
        DynValue::Float(f) => make_variant(VT_R8, VARIANT_0_0_0 { dblVal: *f }),
        DynValue::String(s) => make_variant(
            VT_BSTR,
            VARIANT_0_0_0 {
                bstrVal: ManuallyDrop::new(BSTR::from(s.as_str())),
            },
        ),
        DynValue::List(items) => {
            let array = unsafe { SafeArrayCreateVector(VT_VARIANT, 0, items.len() as u32) };
            if array.is_null() {
                return Err("SafeArrayCreateVector failed".to_string());
            }
            for (i, item) in items.iter().enumerate() {
                let mut variant = dyn_to_variant(item)?;
                let idx = i as i32;
                unsafe {
                    let put = SafeArrayPutElement(
                        array,
                        &idx,
                        &variant as *const VARIANT as *const std::ffi::c_void,
                    );
                    let _ = VariantClear(&mut variant);
                    put.map_err(|e| hr_err("SafeArrayPutElement", e))?;
                }
            }
            make_variant(
                VARENUM(VT_ARRAY.0 | VT_VARIANT.0),
                VARIANT_0_0_0 { parray: array },
            )
        }
        DynValue::Map(_) => {
            return Err("maps cannot cross into COM (no VARIANT representation)".to_string());
        }
    })
}

/// VARIANT → ComValue. The supported VT set per the PRD; everything else
/// is a runtime error naming the VT. Borrows `v`; does not consume it.
pub fn variant_to_com(v: &VARIANT) -> Result<ComValue, String> {
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        let vt = inner.vt;

        if vt.0 & VT_ARRAY.0 != 0 {
            let array: *mut SAFEARRAY = inner.Anonymous.parray;
            if array.is_null() {
                return Ok(ComValue::Data(DynValue::List(Vec::new())));
            }
            let lo = SafeArrayGetLBound(array, 1).map_err(|e| hr_err("SafeArrayGetLBound", e))?;
            let hi = SafeArrayGetUBound(array, 1).map_err(|e| hr_err("SafeArrayGetUBound", e))?;
            let mut items = Vec::new();
            for i in lo..=hi {
                let mut element = VARIANT::default();
                SafeArrayGetElement(
                    array,
                    &i,
                    &mut element as *mut VARIANT as *mut std::ffi::c_void,
                )
                .map_err(|e| hr_err("SafeArrayGetElement", e))?;
                let converted = variant_to_com(&element);
                let _ = VariantClear(&mut element);
                match converted? {
                    ComValue::Data(d) => items.push(d),
                    ComValue::Object(_) => {
                        return Err(
                            "arrays of COM objects are not supported by the v1 marshaller"
                                .to_string(),
                        );
                    }
                }
            }
            return Ok(ComValue::Data(DynValue::List(items)));
        }

        let data = match vt {
            VT_EMPTY | VT_NULL => DynValue::Null,
            VT_BOOL => DynValue::Bool(inner.Anonymous.boolVal.as_bool()),
            VT_I1 => DynValue::Int(inner.Anonymous.cVal as i64),
            VT_I2 => DynValue::Int(inner.Anonymous.iVal as i64),
            VT_I4 | VT_INT => DynValue::Int(inner.Anonymous.lVal as i64),
            VT_I8 => DynValue::Int(inner.Anonymous.llVal),
            VT_UI1 => DynValue::Int(inner.Anonymous.bVal as i64),
            VT_UI2 => DynValue::Int(inner.Anonymous.uiVal as i64),
            VT_UI4 | VT_UINT => DynValue::Int(inner.Anonymous.ulVal as i64),
            VT_UI8 => {
                let n = inner.Anonymous.ullVal;
                i64::try_from(n)
                    .map(DynValue::Int)
                    .map_err(|_| format!("VT_UI8 value {n} exceeds the script integer range"))?
            }
            VT_R4 => DynValue::Float(inner.Anonymous.fltVal as f64),
            VT_R8 => DynValue::Float(inner.Anonymous.dblVal),
            VT_BSTR => DynValue::String(inner.Anonymous.bstrVal.to_string()),
            VT_DISPATCH => {
                return match (*inner.Anonymous.pdispVal).clone() {
                    Some(d) => Ok(ComValue::Object(d)),
                    None => Ok(ComValue::Data(DynValue::Null)),
                };
            }
            other => {
                return Err(format!(
                    "unsupported VARIANT type {} (the v1 marshaller covers the common VT set)",
                    other.0
                ));
            }
        };
        Ok(ComValue::Data(data))
    }
}
