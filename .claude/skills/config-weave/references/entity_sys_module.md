# sys

_host module_

OS and hardware facts — gatherer fodder: family, os_name, arch, cpu_count, memory.

`use sys` — OS and hardware facts (gatherer fodder).

| function | signature | notes |
| --- | --- | --- |
| `family` | `() -> string` | linux/windows/macos |
| `os_name` | `() -> string` | distro on Linux |
| `os_version` | `() -> string` |  |
| `kernel_version` | `() -> string` |  |
| `arch` | `() -> string` |  |
| `cpu_count` | `() -> int` |  |
| `total_memory` / `available_memory` | `() -> int` | bytes |

## Related

- [Host API](../references/concept_host_api.md)

- [Gatherer](../references/concept_gatherer.md)

- [env](../references/entity_env_module.md)

[← Back to SKILL.md](../SKILL.md)
