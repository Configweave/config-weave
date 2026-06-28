# registry

_host module (Windows)_

Windows registry — hive-prefixed keys; registered everywhere, runtime-errors off Windows.

`use registry` — Windows registry. **Registered on every platform** so playbooks compile and validate identically everywhere; off Windows its calls return runtime errors. Guard with a condition (`os.family == "windows"`) or `sys::family()`. Keys are hive-prefixed paths: `HKLM\Software\Vendor\App`. Constants: `registry::HKLM HKCU HKCR HKU HKCC`.

| function | signature | notes |
| --- | --- | --- |
| `read` | `(key, name) -> Result[Option[Value], string]` | `None` when absent; typed values (SZ, DWORD, QWORD, EXPAND_SZ, MULTI_SZ) |
| `write` | `(key, name, value: Value, kind: string) -> Result[unit, string]` | kind: `sz \| dword \| qword \| expand_sz \| multi_sz` |
| `delete_value` | `(key, name) -> Result[unit, string]` |  |
| `create_key` | `(key) -> Result[unit, string]` | creates parents |
| `delete_key` | `(key) -> Result[unit, string]` | deletes the subtree |
| `key_exists` | `(key) -> Result[bool, string]` |  |

## Related

- [Host API](../references/concept_host_api.md)

- [service](../references/entity_service_module.md)

- [com](../references/entity_com_module.md)

- [Value](../references/entity_value_type.md)

[← Back to SKILL.md](../SKILL.md)
