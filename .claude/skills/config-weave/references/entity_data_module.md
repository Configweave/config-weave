# data

_host module_

INI parse/serialize. (JSON/TOML live in the wscript json/toml modules.)

`use data` — INI. (JSON/TOML live in the wscript [json](../references/entity_json_module.md)/[toml](../references/entity_toml_module.md) modules.)

| function | signature | notes |
| --- | --- | --- |
| `ini_parse` | `(text) -> Result[Value, string]` | map of sections; global keys under `""` |
| `ini_serialize` | `(map) -> Result[string, string]` |  |

## Related

- [Host API](../references/concept_host_api.md)

- [json](../references/entity_json_module.md)

- [toml](../references/entity_toml_module.md)

- [Value](../references/entity_value_type.md)

[← Back to SKILL.md](../SKILL.md)
