# service

_host module (Windows)_

Windows service management (SCM); Linux services use shell::run("systemctl …").

`use service` — Windows service management (SCM). Windows-only in v1 — manage Linux services with `shell::run("systemctl …", Value::Null)`. Registered everywhere; off Windows its calls return runtime errors.

| function | signature | notes |
| --- | --- | --- |
| `status` | `(name) -> Result[string, string]` | `running \| stopped \| start_pending \| stop_pending \| paused \| …` |
| `start` / `stop` | `(name) -> Result[unit, string]` | no-op when already in the target state |
| `set_startup` | `(name, mode) -> Result[unit, string]` | mode: `automatic \| manual \| disabled` |
| `startup` | `(name) -> Result[string, string]` | returns the startup type |

## Related

- [Host API](../references/concept_host_api.md)

- [registry](../references/entity_registry_module.md)

- [shell](../references/entity_shell_module.md)

[← Back to SKILL.md](../SKILL.md)
