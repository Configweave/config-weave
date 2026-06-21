# Host API — Windows

_registry, service, and com (IDispatch/WMI) — registered everywhere, runtime-erroring off Windows._

`registry`, `service` and `com` are **registered on every platform** so playbooks compile and validate identically everywhere; off Windows their calls return runtime errors. Guard with a condition (`os.family == "windows"`) or `sys::family()`.

## registry — Windows registry

Keys are hive-prefixed paths: `HKLM\Software\Vendor\App`. Constants: `registry::HKLM HKCU HKCR HKU HKCC`.

| function | signature | notes |
| --- | --- | --- |
| `read` | `(key, name) -> Result[Option[Value], string]` | `None` when absent; typed values (SZ, DWORD, QWORD, EXPAND_SZ, MULTI_SZ) |
| `write` | `(key, name, value: Value, kind: string) -> Result[unit, string]` | kind: `sz \| dword \| qword \| expand_sz \| multi_sz` |
| `delete_value` | `(key, name) -> Result[unit, string]` |  |
| `create_key` | `(key) -> Result[unit, string]` | creates parents |
| `delete_key` | `(key) -> Result[unit, string]` | deletes the subtree |
| `key_exists` | `(key) -> Result[bool, string]` |  |

## service — Windows service management (SCM)

Windows-only in v1 — manage Linux services with `shell::run("systemctl …", Value::Null)`.

| function | signature | notes |
| --- | --- | --- |
| `status` | `(name) -> Result[string, string]` | `running \| stopped \| start_pending \| stop_pending \| paused \| …` |
| `start` / `stop` | `(name) -> Result[unit, string]` | no-op when already in the target state |
| `set_startup` | `(name, mode) -> Result[unit, string]` | mode: `automatic \| manual \| disabled` |
| `startup` | `(name) -> Result[string, string]` | returns the startup type |

## com — late-bound COM via IDispatch

| function | signature | notes |
| --- | --- | --- |
| `create` | `(progid) -> Result[ComObject, string]` | e.g. `"WScript.Shell"` |
| `get_object` | `(name) -> Result[ComObject, string]` | moniker or running object |
| `wmi_query` | `(query) -> Result[Value, string]` | runs against `root\cimv2`; returns a **list of property maps** |

`ComObject` is an opaque handle with methods `get` / `get_object` / `set` / `call(name, args: List[Value])` / `call_object` / `items()`. VT_DISPATCH results must come through \`get_object`/`call_object`/`items()` because the dynamic `Value\` cannot hold an object handle.

```rust
use com
use value

fn gather(params: Value) -> Result[Value, string] {
    com::wmi_query("SELECT Name, State FROM Win32_Service WHERE StartMode='Auto'")
}
```

Worker threads are COM (STA) initialised by the engine — scripts never deal with COM lifetime or apartment setup.

## Related

- [Host API — cross-platform](../references/concept_hostapi.md)

- [Resource & gatherer scripts](../references/concept_scripts.md)

[← All concepts](../references/concepts_ref.md)
