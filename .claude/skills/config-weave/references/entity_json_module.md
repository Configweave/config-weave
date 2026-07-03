# json

_library_

JSON parse / serialize over the Value type; deterministic (sorted keys) output.

`use json` — JSON over the [Value](../references/entity_value_type.md) type. Available in config-weave scripts (unlike wscript-std's math/process/xml).

| function | signature |
| --- | --- |
| `parse` | `(string) -> Result[Value, string]` |
| `to_string` | `(Value) -> string` (keys sorted — deterministic) |
| `to_string_pretty` | `(Value) -> string` |

## Related

- [Value](../references/entity_value_type.md)

- [toml](../references/entity_toml_module.md)

- [data](../references/entity_data_module.md)

[← Back to SKILL.md](../SKILL.md)
