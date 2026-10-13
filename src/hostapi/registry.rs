//! The `registry` module (PRD §7): Windows registry access, typed for
//! REG_SZ, REG_EXPAND_SZ, REG_DWORD, REG_QWORD, REG_MULTI_SZ, REG_BINARY and
//! REG_NONE. Keys are written as a hive-prefixed path:
//! `HKLM\Software\Vendor\App`. Registered on every platform; calls fail at
//! runtime off Windows.

use wscript::Module;
use wscript_std::DynValue;

#[cfg(windows)]
use codec::{View, parse_opts};

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the 'registry' module is only available on Windows";

/// The value codec and option parsing: plain Rust with no Windows calls, so
/// it compiles and is unit-tested on every platform. Only `win` uses it.
mod codec {
    #![cfg_attr(not(windows), allow(dead_code))]

    use std::collections::HashMap;

    use wscript_std::DynValue;

    // Registry value types (winnt.h). Mirrored here so the codec compiles and is
    // tested off Windows; `win` converts them to `REG_VALUE_TYPE`.
    const REG_NONE: u32 = 0;
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const REG_BINARY: u32 = 3;
    const REG_DWORD: u32 = 4;
    const REG_MULTI_SZ: u32 = 7;
    const REG_QWORD: u32 = 11;

    // Access-mask bits that select a registry view (winnt.h).
    const KEY_WOW64_64KEY: u32 = 0x0100;
    const KEY_WOW64_32KEY: u32 = 0x0200;

    const KINDS: &str = "sz, expand_sz, dword, qword, multi_sz, binary, none";

    /// The kind name `write` accepts for a registry type, or None for a type
    /// the module does not handle (REG_LINK, REG_RESOURCE_LIST, ...).
    pub(super) fn kind_name(kind: u32) -> Option<&'static str> {
        Some(match kind {
            REG_SZ => "sz",
            REG_EXPAND_SZ => "expand_sz",
            REG_DWORD => "dword",
            REG_QWORD => "qword",
            REG_MULTI_SZ => "multi_sz",
            REG_BINARY => "binary",
            REG_NONE => "none",
            _ => return None,
        })
    }

    /// Raw value bytes to a script value. Binary data uses the same shape as
    /// `fs::read_bytes`: a list of ints, one per byte.
    pub(super) fn decode(kind: u32, buf: &[u8]) -> Result<DynValue, String> {
        match kind {
            REG_SZ | REG_EXPAND_SZ => Ok(DynValue::String(utf16_z(buf))),
            REG_DWORD => {
                let mut b = [0u8; 4];
                let n = 4.min(buf.len());
                b[..n].copy_from_slice(&buf[..n]);
                Ok(DynValue::Int(u32::from_le_bytes(b) as i64))
            }
            REG_QWORD => {
                let mut b = [0u8; 8];
                let n = 8.min(buf.len());
                b[..n].copy_from_slice(&buf[..n]);
                Ok(DynValue::Int(i64::from_le_bytes(b)))
            }
            REG_MULTI_SZ => {
                let wide = utf16_units(buf);
                let mut items = Vec::new();
                for part in wide.split(|&c| c == 0) {
                    if !part.is_empty() {
                        items.push(DynValue::String(String::from_utf16_lossy(part)));
                    }
                }
                Ok(DynValue::List(items))
            }
            REG_BINARY | REG_NONE => Ok(DynValue::List(
                buf.iter().map(|b| DynValue::Int(*b as i64)).collect(),
            )),
            other => Err(format!("unsupported registry value type {other}")),
        }
    }

    /// The `{kind, data}` map `read_typed` returns.
    pub(super) fn typed(kind: u32, buf: &[u8]) -> Result<DynValue, String> {
        let name =
            kind_name(kind).ok_or_else(|| format!("unsupported registry value type {kind}"))?;
        let mut m = HashMap::new();
        m.insert("kind".to_string(), DynValue::String(name.to_string()));
        m.insert("data".to_string(), decode(kind, buf)?);
        Ok(DynValue::Map(m))
    }

    /// A script value to the registry type and raw bytes `write` stores.
    pub(super) fn encode(kind: &str, value: &DynValue) -> Result<(u32, Vec<u8>), String> {
        match (kind, value) {
            ("sz", DynValue::String(s)) => Ok((REG_SZ, encode_utf16_z(s))),
            ("expand_sz", DynValue::String(s)) => Ok((REG_EXPAND_SZ, encode_utf16_z(s))),
            ("dword", DynValue::Int(n)) => {
                let v = u32::try_from(*n).map_err(|_| format!("{n} does not fit in a DWORD"))?;
                Ok((REG_DWORD, v.to_le_bytes().to_vec()))
            }
            ("qword", DynValue::Int(n)) => Ok((REG_QWORD, n.to_le_bytes().to_vec())),
            ("multi_sz", DynValue::List(items)) => {
                let mut bytes = Vec::new();
                for item in items {
                    let DynValue::String(s) = item else {
                        return Err("multi_sz expects a list of strings".to_string());
                    };
                    bytes.extend(encode_utf16_z(s));
                }
                bytes.extend([0, 0]); // double-NUL terminator
                Ok((REG_MULTI_SZ, bytes))
            }
            ("binary" | "none", DynValue::List(items)) => {
                let mut bytes = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        DynValue::Int(n) if (0..=255).contains(n) => bytes.push(*n as u8),
                        other => {
                            return Err(format!(
                                "{kind} expects a list of byte ints (0-255), got {other:?}"
                            ));
                        }
                    }
                }
                Ok((
                    if kind == "binary" {
                        REG_BINARY
                    } else {
                        REG_NONE
                    },
                    bytes,
                ))
            }
            (k, v) => Err(format!(
                "registry kind '{k}' does not accept {v:?} (kinds: {KINDS})"
            )),
        }
    }

    pub(super) fn utf16_units(buf: &[u8]) -> Vec<u16> {
        buf.as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect()
    }

    pub(super) fn utf16_z(buf: &[u8]) -> String {
        let wide = utf16_units(buf);
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    pub(super) fn encode_utf16_z(s: &str) -> Vec<u8> {
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        wide.push(0);
        wide.iter().flat_map(|c| c.to_le_bytes()).collect()
    }

    /// Which registry view a key is opened in. `Native` is the process's own
    /// view; the others force the 32-bit (`WOW6432Node`) or 64-bit view.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub(super) enum View {
        #[default]
        Native,
        Wow64_32,
        Wow64_64,
    }

    impl View {
        /// The bits OR-ed into the access mask on open, create and delete.
        pub(super) fn sam_flag(self) -> u32 {
            match self {
                View::Native => 0,
                View::Wow64_32 => KEY_WOW64_32KEY,
                View::Wow64_64 => KEY_WOW64_64KEY,
            }
        }
    }

    /// Parse the trailing `opts` map of the `_with` functions; `Null` means
    /// all defaults.
    pub(super) fn parse_opts(opts: &DynValue) -> Result<View, String> {
        let mut view = View::default();
        match opts {
            DynValue::Null => {}
            DynValue::Map(m) => {
                for (k, v) in m {
                    match (k.as_str(), v) {
                        ("view", DynValue::String(s)) => {
                            view = match s.as_str() {
                                "native" => View::Native,
                                "wow64_32" => View::Wow64_32,
                                "wow64_64" => View::Wow64_64,
                                other => {
                                    return Err(format!(
                                        "unknown registry view '{other}' \
                                         (expected native, wow64_32, wow64_64)"
                                    ));
                                }
                            };
                        }
                        ("view", other) => {
                            return Err(format!("registry view must be a string, got {other:?}"));
                        }
                        (other, _) => {
                            return Err(format!(
                                "unknown registry option '{other}' (expected view)"
                            ));
                        }
                    }
                }
            }
            other => {
                return Err(format!(
                    "registry options must be a map or null, got {other:?}"
                ));
            }
        }
        Ok(view)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn bytes(v: &[i64]) -> DynValue {
            DynValue::List(v.iter().map(|b| DynValue::Int(*b)).collect())
        }

        fn kind_of(v: &DynValue) -> &str {
            match v.get("kind") {
                Some(DynValue::String(s)) => s,
                other => panic!("no kind: {other:?}"),
            }
        }

        #[test]
        fn binary_and_none_decode_to_byte_lists() {
            assert_eq!(
                decode(REG_BINARY, &[0, 7, 255]).unwrap(),
                bytes(&[0, 7, 255])
            );
            assert_eq!(decode(REG_NONE, &[]).unwrap(), bytes(&[]));
        }

        #[test]
        fn unhandled_types_still_error() {
            // REG_LINK
            let err = decode(6, &[]).unwrap_err();
            assert!(
                err.contains("unsupported registry value type 6"),
                "got: {err}"
            );
            assert!(typed(6, &[]).is_err());
        }

        #[test]
        fn short_dword_and_qword_buffers_do_not_panic() {
            assert_eq!(decode(REG_DWORD, &[1]).unwrap(), DynValue::Int(1));
            assert_eq!(decode(REG_QWORD, &[]).unwrap(), DynValue::Int(0));
        }

        #[test]
        fn typed_tells_sz_from_expand_sz_and_dword_from_qword() {
            let s = encode_utf16_z("%TEMP%");
            let sz = typed(REG_SZ, &s).unwrap();
            let expand = typed(REG_EXPAND_SZ, &s).unwrap();
            assert_eq!(kind_of(&sz), "sz");
            assert_eq!(kind_of(&expand), "expand_sz");
            assert_eq!(sz.get("data"), Some(&DynValue::String("%TEMP%".into())));
            assert_eq!(kind_of(&typed(REG_DWORD, &[1, 0, 0, 0]).unwrap()), "dword");
            assert_eq!(
                kind_of(&typed(REG_QWORD, &[1, 0, 0, 0, 0, 0, 0, 0]).unwrap()),
                "qword"
            );
            let bin = typed(REG_BINARY, &[9]).unwrap();
            assert_eq!(kind_of(&bin), "binary");
            assert_eq!(bin.get("data"), Some(&bytes(&[9])));
            assert_eq!(kind_of(&typed(REG_NONE, &[]).unwrap()), "none");
        }

        #[test]
        fn typed_kinds_round_trip_through_encode() {
            let cases = [
                ("sz", DynValue::String("a".into())),
                ("expand_sz", DynValue::String("%PATH%".into())),
                ("dword", DynValue::Int(42)),
                ("qword", DynValue::Int(-1)),
                (
                    "multi_sz",
                    DynValue::List(vec![
                        DynValue::String("x".into()),
                        DynValue::String("y".into()),
                    ]),
                ),
                ("binary", bytes(&[1, 2, 3])),
                ("none", bytes(&[])),
            ];
            for (kind, value) in cases {
                let (reg, raw) = encode(kind, &value).unwrap();
                let back = typed(reg, &raw).unwrap();
                assert_eq!(kind_of(&back), kind);
                assert_eq!(back.get("data"), Some(&value), "kind {kind}");
            }
        }

        #[test]
        fn binary_rejects_non_byte_items() {
            let err = encode("binary", &bytes(&[256])).unwrap_err();
            assert!(err.contains("byte ints"), "got: {err}");
            assert!(
                encode(
                    "binary",
                    &DynValue::List(vec![DynValue::String("a".into())])
                )
                .is_err()
            );
            let err = encode("blob", &bytes(&[1])).unwrap_err();
            assert!(err.contains("binary, none"), "got: {err}");
        }

        #[test]
        fn opts_select_the_view() {
            let view = |s: &str| {
                let mut m = HashMap::new();
                m.insert("view".to_string(), DynValue::String(s.into()));
                parse_opts(&DynValue::Map(m))
            };
            assert_eq!(parse_opts(&DynValue::Null).unwrap(), View::Native);
            assert_eq!(
                parse_opts(&DynValue::Map(HashMap::new())).unwrap(),
                View::Native
            );
            assert_eq!(view("native").unwrap(), View::Native);
            assert_eq!(view("wow64_32").unwrap(), View::Wow64_32);
            assert_eq!(view("wow64_64").unwrap(), View::Wow64_64);
            assert!(view("wow32").unwrap_err().contains("unknown registry view"));
        }

        #[test]
        fn opts_reject_unknown_keys_and_shapes() {
            let mut m = HashMap::new();
            m.insert("hive".to_string(), DynValue::String("x".into()));
            assert!(
                parse_opts(&DynValue::Map(m))
                    .unwrap_err()
                    .contains("unknown registry option")
            );
            assert!(parse_opts(&DynValue::Int(1)).is_err());
        }

        #[test]
        fn views_map_to_wow64_access_bits() {
            assert_eq!(View::Native.sam_flag(), 0);
            assert_eq!(View::Wow64_32.sam_flag(), 0x0200);
            assert_eq!(View::Wow64_64.sam_flag(), 0x0100);
        }

        #[cfg(windows)]
        #[test]
        fn mirrored_constants_match_the_windows_crate() {
            use windows::Win32::System::Registry as r;
            assert_eq!(REG_NONE, r::REG_NONE.0);
            assert_eq!(REG_SZ, r::REG_SZ.0);
            assert_eq!(REG_EXPAND_SZ, r::REG_EXPAND_SZ.0);
            assert_eq!(REG_BINARY, r::REG_BINARY.0);
            assert_eq!(REG_DWORD, r::REG_DWORD.0);
            assert_eq!(REG_MULTI_SZ, r::REG_MULTI_SZ.0);
            assert_eq!(REG_QWORD, r::REG_QWORD.0);
            assert_eq!(KEY_WOW64_32KEY, r::KEY_WOW64_32KEY.0);
            assert_eq!(KEY_WOW64_64KEY, r::KEY_WOW64_64KEY.0);
        }
    }
}

pub fn module() -> Module {
    let mut m = Module::new("registry");
    m.doc("Windows registry (Windows only). Keys are hive-prefixed paths like HKLM\\Software\\App");
    m.const_("HKLM", "HKLM");
    m.const_("HKCU", "HKCU");
    m.const_("HKCR", "HKCR");
    m.const_("HKU", "HKU");
    m.const_("HKCC", "HKCC");

    m.doc_next("Read a value (None when the key or value is absent)");
    #[cfg(windows)]
    m.fn_(
        "read",
        |key: &str, name: &str| -> Result<Option<DynValue>, String> {
            win::read(key, name, View::Native)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "read",
        |_: &str, _: &str| -> Result<Option<DynValue>, String> { Err(NOT_WINDOWS.to_string()) },
    );

    m.doc_next("Like read, with opts: view (native | wow64_32 | wow64_64)");
    #[cfg(windows)]
    m.fn_(
        "read_with",
        |key: &str, name: &str, opts: DynValue| -> Result<Option<DynValue>, String> {
            win::read(key, name, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "read_with",
        |_: &str, _: &str, _: DynValue| -> Result<Option<DynValue>, String> {
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next(
        "Read a value with its kind: {kind, data} (None when absent). \
         opts: view (native | wow64_32 | wow64_64)",
    );
    #[cfg(windows)]
    m.fn_(
        "read_typed",
        |key: &str, name: &str, opts: DynValue| -> Result<Option<DynValue>, String> {
            win::read_typed(key, name, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "read_typed",
        |_: &str, _: &str, _: DynValue| -> Result<Option<DynValue>, String> {
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next("Write a value. kind: sz | expand_sz | dword | qword | multi_sz | binary | none");
    #[cfg(windows)]
    m.fn_(
        "write",
        |key: String, name: String, value: DynValue, kind: String| -> Result<(), String> {
            win::write(&key, &name, &value, &kind, View::Native)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "write",
        |_: String, _: String, _: DynValue, _: String| -> Result<(), String> {
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next("Like write, with opts: view (native | wow64_32 | wow64_64)");
    #[cfg(windows)]
    m.fn_(
        "write_with",
        |key: String,
         name: String,
         value: DynValue,
         kind: String,
         opts: DynValue|
         -> Result<(), String> {
            win::write(&key, &name, &value, &kind, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "write_with",
        |_: String, _: String, _: DynValue, _: String, _: DynValue| -> Result<(), String> {
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next("Delete a value");
    #[cfg(windows)]
    m.fn_(
        "delete_value",
        |key: &str, name: &str| -> Result<(), String> {
            win::delete_value(key, name, View::Native)
        },
    );
    #[cfg(not(windows))]
    m.fn_("delete_value", |_: &str, _: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Like delete_value, with opts: view (native | wow64_32 | wow64_64)");
    #[cfg(windows)]
    m.fn_(
        "delete_value_with",
        |key: &str, name: &str, opts: DynValue| -> Result<(), String> {
            win::delete_value(key, name, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "delete_value_with",
        |_: &str, _: &str, _: DynValue| -> Result<(), String> { Err(NOT_WINDOWS.to_string()) },
    );

    m.doc_next("Create a key (and parents)");
    #[cfg(windows)]
    m.fn_("create_key", |key: &str| -> Result<(), String> {
        win::create_key(key, View::Native)
    });
    #[cfg(not(windows))]
    m.fn_("create_key", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Like create_key, with opts: view (native | wow64_32 | wow64_64)");
    #[cfg(windows)]
    m.fn_(
        "create_key_with",
        |key: &str, opts: DynValue| -> Result<(), String> {
            win::create_key(key, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "create_key_with",
        |_: &str, _: DynValue| -> Result<(), String> { Err(NOT_WINDOWS.to_string()) },
    );

    m.doc_next("Delete a key and its subtree");
    #[cfg(windows)]
    m.fn_("delete_key", |key: &str| -> Result<(), String> {
        win::delete_key(key, View::Native)
    });
    #[cfg(not(windows))]
    m.fn_("delete_key", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Like delete_key, with opts: view (native | wow64_32 | wow64_64)");
    #[cfg(windows)]
    m.fn_(
        "delete_key_with",
        |key: &str, opts: DynValue| -> Result<(), String> {
            win::delete_key(key, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "delete_key_with",
        |_: &str, _: DynValue| -> Result<(), String> { Err(NOT_WINDOWS.to_string()) },
    );

    m.doc_next("Whether a key exists");
    #[cfg(windows)]
    m.fn_("key_exists", |key: &str| -> Result<bool, String> {
        win::key_exists(key, View::Native)
    });
    #[cfg(not(windows))]
    m.fn_("key_exists", |_: &str| -> Result<bool, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Like key_exists, with opts: view (native | wow64_32 | wow64_64)");
    #[cfg(windows)]
    m.fn_(
        "key_exists_with",
        |key: &str, opts: DynValue| -> Result<bool, String> {
            win::key_exists(key, parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "key_exists_with",
        |_: &str, _: DynValue| -> Result<bool, String> { Err(NOT_WINDOWS.to_string()) },
    );

    m
}

#[cfg(windows)]
mod win {
    use super::View;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_CONFIG, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        HKEY_USERS, KEY_ALL_ACCESS, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS,
        REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegDeleteTreeW,
        RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR};
    use wscript_std::DynValue;

    fn split_hive(key: &str) -> Result<(HKEY, String), String> {
        let (hive, rest) = key.split_once('\\').unwrap_or((key, ""));
        let h = match hive.to_ascii_uppercase().as_str() {
            "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
            "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
            "HKCR" | "HKEY_CLASSES_ROOT" => HKEY_CLASSES_ROOT,
            "HKU" | "HKEY_USERS" => HKEY_USERS,
            "HKCC" | "HKEY_CURRENT_CONFIG" => HKEY_CURRENT_CONFIG,
            other => return Err(format!("unknown registry hive '{other}'")),
        };
        Ok((h, rest.to_string()))
    }

    fn sam(access: REG_SAM_FLAGS, view: View) -> REG_SAM_FLAGS {
        access | REG_SAM_FLAGS(view.sam_flag())
    }

    fn open(key: &str, access: REG_SAM_FLAGS, view: View) -> Result<HKEY, String> {
        let (hive, path) = split_hive(key)?;
        let wide = HSTRING::from(path);
        let mut out = HKEY::default();
        unsafe {
            RegOpenKeyExW(
                hive,
                PCWSTR(wide.as_ptr()),
                Some(0),
                sam(access, view),
                &mut out,
            )
            .ok()
            .map_err(|e| format!("opening '{key}': {e}"))?;
        }
        Ok(out)
    }

    struct Guard(HKEY);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    /// The raw type and bytes of a value; None when the key or value is
    /// absent.
    fn query(key: &str, name: &str, view: View) -> Result<Option<(u32, Vec<u8>)>, String> {
        let handle = match open(key, KEY_READ, view) {
            Ok(h) => Guard(h),
            Err(_) => return Ok(None),
        };
        let wide = HSTRING::from(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let probe = unsafe {
            RegQueryValueExW(
                handle.0,
                PCWSTR(wide.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if probe.is_err() {
            return Ok(None);
        }
        let mut buf = vec![0u8; size as usize];
        unsafe {
            RegQueryValueExW(
                handle.0,
                PCWSTR(wide.as_ptr()),
                None,
                Some(&mut kind),
                Some(buf.as_mut_ptr()),
                Some(&mut size),
            )
            .ok()
            .map_err(|e| format!("reading '{key}\\{name}': {e}"))?;
        }
        buf.truncate(size as usize);
        Ok(Some((kind.0, buf)))
    }

    pub fn read(key: &str, name: &str, view: View) -> Result<Option<DynValue>, String> {
        query(key, name, view)?
            .map(|(kind, buf)| super::codec::decode(kind, &buf))
            .transpose()
    }

    pub fn read_typed(key: &str, name: &str, view: View) -> Result<Option<DynValue>, String> {
        query(key, name, view)?
            .map(|(kind, buf)| super::codec::typed(kind, &buf))
            .transpose()
    }

    pub fn write(
        key: &str,
        name: &str,
        value: &DynValue,
        kind: &str,
        view: View,
    ) -> Result<(), String> {
        let (reg_kind, bytes) = super::codec::encode(kind, value)?;
        create_key(key, view)?;
        let handle = Guard(open(key, KEY_WRITE, view)?);
        let wide = HSTRING::from(name);
        unsafe {
            RegSetValueExW(
                handle.0,
                PCWSTR(wide.as_ptr()),
                None,
                REG_VALUE_TYPE(reg_kind),
                Some(&bytes),
            )
            .ok()
            .map_err(|e| format!("writing '{key}\\{name}': {e}"))
        }
    }

    pub fn delete_value(key: &str, name: &str, view: View) -> Result<(), String> {
        let handle = Guard(open(key, KEY_WRITE, view)?);
        let wide = HSTRING::from(name);
        unsafe {
            RegDeleteValueW(handle.0, PCWSTR(wide.as_ptr()))
                .ok()
                .map_err(|e| format!("deleting '{key}\\{name}': {e}"))
        }
    }

    pub fn create_key(key: &str, view: View) -> Result<(), String> {
        let (hive, path) = split_hive(key)?;
        let wide = HSTRING::from(path);
        let mut out = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                hive,
                PCWSTR(wide.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                sam(KEY_READ | KEY_WRITE, view),
                None,
                &mut out,
                None,
            )
            .ok()
            .map_err(|e| format!("creating '{key}': {e}"))?;
            let _ = RegCloseKey(out);
        }
        Ok(())
    }

    pub fn delete_key(key: &str, view: View) -> Result<(), String> {
        let (hive, path) = split_hive(key)?;
        let wide = HSTRING::from(path);
        if view == View::Native {
            return unsafe {
                RegDeleteTreeW(hive, PCWSTR(wide.as_ptr()))
                    .ok()
                    .map_err(|e| format!("deleting '{key}': {e}"))
            };
        }
        // RegDeleteTreeW takes no view flag: open the key in the requested
        // view, empty it through that handle, then delete the key itself
        // with RegDeleteKeyExW, which does.
        let handle = Guard(open(key, KEY_ALL_ACCESS, view)?);
        unsafe {
            RegDeleteTreeW(handle.0, PCWSTR::null())
                .ok()
                .map_err(|e| format!("deleting '{key}': {e}"))?;
            drop(handle);
            RegDeleteKeyExW(hive, PCWSTR(wide.as_ptr()), view.sam_flag(), None)
                .ok()
                .map_err(|e| format!("deleting '{key}': {e}"))
        }
    }

    pub fn key_exists(key: &str, view: View) -> Result<bool, String> {
        match open(key, KEY_READ, view) {
            Ok(h) => {
                drop(Guard(h));
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }
}
