//! The `com` module (PRD §7): late-bound COM via IDispatch. Registered on
//! every platform; calls fail at runtime off Windows.
//!
//! Binding note: the PRD sketches `obj.call("Method", args...)` — wscript
//! has fixed arity, so `call` takes a `List[Value]`. VT_DISPATCH results
//! (nested objects) surface through the `*_object` variants; `wmi_query`
//! flattens each result row into a property map host-side, covering the
//! mandatory WMI collection case without scripts touching enumerators.

use wscript::{Module, Script};
use wscript_std::DynValue;

#[cfg(windows)]
use crate::comdispatch::{self, ComValue};

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the 'com' module is only available on Windows";

/// A late-bound COM object handle. Opaque: scripts only see its methods.
#[derive(Script)]
#[script(opaque)]
pub struct ComObject {
    #[cfg(windows)]
    pub(crate) disp: windows::Win32::System::Com::IDispatch,
}

#[cfg(windows)]
fn data_only(v: ComValue, what: &str) -> Result<DynValue, String> {
    match v {
        ComValue::Data(d) => Ok(d),
        ComValue::Object(_) => Err(format!(
            "{what} returned a COM object; use the *_object variant"
        )),
    }
}

#[cfg(windows)]
fn object_only(v: ComValue, what: &str) -> Result<ComObject, String> {
    match v {
        ComValue::Object(disp) => Ok(ComObject { disp }),
        ComValue::Data(d) => Err(format!("{what} returned plain data ({d:?}), not an object")),
    }
}

pub fn module() -> Module {
    let mut m = Module::new("com");
    m.doc("Late-bound COM via IDispatch (Windows only)");

    m.doc_next("Create a COM object from a ProgID (e.g. \"WScript.Shell\")");
    #[cfg(windows)]
    m.fn_("create", |progid: &str| -> Result<ComObject, String> {
        comdispatch::create(progid).map(|disp| ComObject { disp })
    });
    #[cfg(not(windows))]
    m.fn_("create", |_progid: &str| -> Result<ComObject, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("GetObject: bind a moniker (e.g. \"winmgmts://./root/cimv2\") or running object");
    #[cfg(windows)]
    m.fn_("get_object", |name: &str| -> Result<ComObject, String> {
        comdispatch::get_object(name).map(|disp| ComObject { disp })
    });
    #[cfg(not(windows))]
    m.fn_("get_object", |_name: &str| -> Result<ComObject, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Run a WMI query against root\\cimv2; returns a list of property maps");
    #[cfg(windows)]
    m.fn_("wmi_query", |query: &str| -> Result<DynValue, String> {
        comdispatch::wmi_query(query)
    });
    #[cfg(not(windows))]
    m.fn_("wmi_query", |_query: &str| -> Result<DynValue, String> {
        Err(NOT_WINDOWS.to_string())
    });

    let ty = m.ty::<ComObject>();
    build_methods(ty);
    m
}

#[cfg(windows)]
fn build_methods(mut ty: wscript::core::module::TypeBuilder<'_, ComObject>) {
    ty.method(
        "get",
        |o: &ComObject, name: &str| -> Result<DynValue, String> {
            data_only(comdispatch::get_property(&o.disp, name)?, name)
        },
    )
    .method(
        "get_object",
        |o: &ComObject, name: &str| -> Result<ComObject, String> {
            object_only(comdispatch::get_property(&o.disp, name)?, name)
        },
    )
    .method(
        "set",
        |o: &ComObject, name: String, value: DynValue| -> Result<(), String> {
            comdispatch::set_property(&o.disp, &name, &value)
        },
    )
    .method(
        "call",
        |o: &ComObject, name: String, args: Vec<DynValue>| -> Result<DynValue, String> {
            data_only(comdispatch::call_method(&o.disp, &name, &args)?, &name)
        },
    )
    .method(
        "call_object",
        |o: &ComObject, name: String, args: Vec<DynValue>| -> Result<ComObject, String> {
            object_only(comdispatch::call_method(&o.disp, &name, &args)?, &name)
        },
    )
    .method("items", |o: &ComObject| -> Result<Vec<ComObject>, String> {
        let mut out = Vec::new();
        for item in comdispatch::enumerate(&o.disp)? {
            out.push(object_only(item, "collection item")?);
        }
        Ok(out)
    });
}

#[cfg(not(windows))]
fn build_methods(mut ty: wscript::core::module::TypeBuilder<'_, ComObject>) {
    ty.method(
        "get",
        |_o: &ComObject, _name: &str| -> Result<DynValue, String> { Err(NOT_WINDOWS.to_string()) },
    )
    .method(
        "get_object",
        |_o: &ComObject, _name: &str| -> Result<ComObject, String> { Err(NOT_WINDOWS.to_string()) },
    )
    .method(
        "set",
        |_o: &ComObject, _name: String, _value: DynValue| -> Result<(), String> {
            Err(NOT_WINDOWS.to_string())
        },
    )
    .method(
        "call",
        |_o: &ComObject, _name: String, _args: Vec<DynValue>| -> Result<DynValue, String> {
            Err(NOT_WINDOWS.to_string())
        },
    )
    .method(
        "call_object",
        |_o: &ComObject, _name: String, _args: Vec<DynValue>| -> Result<ComObject, String> {
            Err(NOT_WINDOWS.to_string())
        },
    )
    .method(
        "items",
        |_o: &ComObject| -> Result<Vec<ComObject>, String> { Err(NOT_WINDOWS.to_string()) },
    );
}
