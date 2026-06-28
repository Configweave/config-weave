# env

_host module_

Process environment and host identity — get/set vars, hostname, current user, elevation.

`use env` — process environment and host identity.

| function | signature |
| --- | --- |
| `get` | `(name) -> Option[string]` |
| `set` | `(name, value)` |
| `unset` | `(name)` |
| `path_split` | `(value) -> List[string]` |
| `path_join` | `(parts) -> Result[string, string]` |
| `hostname` | `() -> string` |
| `current_user` | `() -> string` |
| `home_dir` | `() -> string` |
| `is_elevated` | `() -> bool` (root/Administrator) |

## Related

- [Host API](../references/concept_host_api.md)

- [sys](../references/entity_sys_module.md)

[← Back to SKILL.md](../SKILL.md)
