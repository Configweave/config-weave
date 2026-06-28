# toml

_wscript stdlib module_

TOML parse / serialize over the Value type; fails on Null or non-map top levels.

`use toml` — TOML over the [Value](../references/entity_value_type.md) type.

| function | signature |
| --- | --- |
| `parse` | `(string) -> Result[Value, string]` (datetimes become strings) |
| `to_string` | `(Value) -> Result[string, string]` |
| `to_string_pretty` | `(Value) -> Result[string, string]` |

TOML serialization fails on `Null` anywhere and on non-map top levels. INI lives in the host [data](../references/entity_data_module.md) module.

## Related

- [Value](../references/entity_value_type.md)

- [json](../references/entity_json_module.md)

- [data](../references/entity_data_module.md)

[← Back to SKILL.md](../SKILL.md)
