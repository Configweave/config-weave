//! The `registry` module (PRD §7): Windows registry access, typed for
//! REG_SZ, REG_DWORD, REG_QWORD, REG_EXPAND_SZ and REG_MULTI_SZ. Keys are
//! written as a hive-prefixed path: `HKLM\Software\Vendor\App`.
//! Registered on every platform; calls fail at runtime off Windows.

use wisp::Module;
use wisp_std::DynValue;

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the 'registry' module is only available on Windows";

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
        |key: &str, name: &str| -> Result<Option<DynValue>, String> { win::read(key, name) },
    );
    #[cfg(not(windows))]
    m.fn_(
        "read",
        |_: &str, _: &str| -> Result<Option<DynValue>, String> { Err(NOT_WINDOWS.to_string()) },
    );

    m.doc_next("Write a value. kind: sz | dword | qword | expand_sz | multi_sz");
    #[cfg(windows)]
    m.fn_(
        "write",
        |key: String, name: String, value: DynValue, kind: String| -> Result<(), String> {
            win::write(&key, &name, &value, &kind)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "write",
        |_: String, _: String, _: DynValue, _: String| -> Result<(), String> {
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next("Delete a value");
    #[cfg(windows)]
    m.fn_(
        "delete_value",
        |key: &str, name: &str| -> Result<(), String> { win::delete_value(key, name) },
    );
    #[cfg(not(windows))]
    m.fn_("delete_value", |_: &str, _: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Create a key (and parents)");
    #[cfg(windows)]
    m.fn_("create_key", |key: &str| -> Result<(), String> {
        win::create_key(key)
    });
    #[cfg(not(windows))]
    m.fn_("create_key", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Delete a key and its subtree");
    #[cfg(windows)]
    m.fn_("delete_key", |key: &str| -> Result<(), String> {
        win::delete_key(key)
    });
    #[cfg(not(windows))]
    m.fn_("delete_key", |_: &str| -> Result<(), String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next("Whether a key exists");
    #[cfg(windows)]
    m.fn_("key_exists", |key: &str| -> Result<bool, String> {
        win::key_exists(key)
    });
    #[cfg(not(windows))]
    m.fn_("key_exists", |_: &str| -> Result<bool, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_CONFIG, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        HKEY_USERS, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ,
        REG_OPTION_NON_VOLATILE, REG_QWORD, REG_SZ, REG_VALUE_TYPE, RRF_RT_ANY, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW,
        RegQueryValueExW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR};
    use wisp_std::DynValue;

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

    fn open(
        key: &str,
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<HKEY, String> {
        let (hive, path) = split_hive(key)?;
        let wide = HSTRING::from(path);
        let mut out = HKEY::default();
        unsafe {
            RegOpenKeyExW(hive, PCWSTR(wide.as_ptr()), Some(0), access, &mut out)
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

    pub fn read(key: &str, name: &str) -> Result<Option<DynValue>, String> {
        let handle = match open(key, KEY_READ) {
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
        decode(kind, &buf).map(Some)
    }

    fn decode(kind: REG_VALUE_TYPE, buf: &[u8]) -> Result<DynValue, String> {
        match kind {
            REG_SZ | REG_EXPAND_SZ => Ok(DynValue::String(utf16_z(buf))),
            REG_DWORD => {
                let mut b = [0u8; 4];
                b.copy_from_slice(&buf[..4.min(buf.len())]);
                Ok(DynValue::Int(u32::from_le_bytes(b) as i64))
            }
            REG_QWORD => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&buf[..8.min(buf.len())]);
                Ok(DynValue::Int(i64::from_le_bytes(b)))
            }
            REG_MULTI_SZ => {
                let wide: Vec<u16> = buf
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let mut items = Vec::new();
                for part in wide.split(|&c| c == 0) {
                    if !part.is_empty() {
                        items.push(DynValue::String(String::from_utf16_lossy(part)));
                    }
                }
                Ok(DynValue::List(items))
            }
            other => Err(format!("unsupported registry value type {}", other.0)),
        }
    }

    fn utf16_z(buf: &[u8]) -> String {
        let wide: Vec<u16> = buf
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    fn encode_utf16_z(s: &str) -> Vec<u8> {
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        wide.push(0);
        wide.iter().flat_map(|c| c.to_le_bytes()).collect()
    }

    pub fn write(key: &str, name: &str, value: &DynValue, kind: &str) -> Result<(), String> {
        create_key(key)?;
        let handle = Guard(open(key, KEY_WRITE)?);
        let wide = HSTRING::from(name);
        let (reg_kind, bytes): (REG_VALUE_TYPE, Vec<u8>) = match (kind, value) {
            ("sz", DynValue::String(s)) => (REG_SZ, encode_utf16_z(s)),
            ("expand_sz", DynValue::String(s)) => (REG_EXPAND_SZ, encode_utf16_z(s)),
            ("dword", DynValue::Int(n)) => {
                let v = u32::try_from(*n).map_err(|_| format!("{n} does not fit in a DWORD"))?;
                (REG_DWORD, v.to_le_bytes().to_vec())
            }
            ("qword", DynValue::Int(n)) => (REG_QWORD, n.to_le_bytes().to_vec()),
            ("multi_sz", DynValue::List(items)) => {
                let mut bytes = Vec::new();
                for item in items {
                    let DynValue::String(s) = item else {
                        return Err("multi_sz expects a list of strings".to_string());
                    };
                    bytes.extend(encode_utf16_z(s));
                }
                bytes.extend([0, 0]); // double-NUL terminator
                (REG_MULTI_SZ, bytes)
            }
            (k, v) => {
                return Err(format!(
                    "registry kind '{k}' does not accept {v:?} (kinds: sz, dword, qword, \
                     expand_sz, multi_sz)"
                ));
            }
        };
        unsafe {
            RegSetValueExW(
                handle.0,
                PCWSTR(wide.as_ptr()),
                None,
                reg_kind,
                Some(&bytes),
            )
            .ok()
            .map_err(|e| format!("writing '{key}\\{name}': {e}"))
        }
    }

    pub fn delete_value(key: &str, name: &str) -> Result<(), String> {
        let handle = Guard(open(key, KEY_WRITE)?);
        let wide = HSTRING::from(name);
        unsafe {
            RegDeleteValueW(handle.0, PCWSTR(wide.as_ptr()))
                .ok()
                .map_err(|e| format!("deleting '{key}\\{name}': {e}"))
        }
    }

    pub fn create_key(key: &str) -> Result<(), String> {
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
                KEY_READ | KEY_WRITE,
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

    pub fn delete_key(key: &str) -> Result<(), String> {
        let (hive, path) = split_hive(key)?;
        let wide = HSTRING::from(path);
        unsafe {
            RegDeleteTreeW(hive, PCWSTR(wide.as_ptr()))
                .ok()
                .map_err(|e| format!("deleting '{key}': {e}"))
        }
    }

    pub fn key_exists(key: &str) -> Result<bool, String> {
        match open(key, KEY_READ) {
            Ok(h) => {
                drop(Guard(h));
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }
}
