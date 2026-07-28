# xml

_library_

XML parse / serialize over the Value type — elements nest as maps, attributes under @attrs, text under #text.

`use xml` — XML over the [Value](../references/entity_value_type.md) type, for the config files (`.csproj`, `web.config`, XML-shaped app config) that json and toml do not cover.

| function | signature |
| --- | --- |
| `parse` | `(string) -> Result[Value, string]` |
| `to_string` | `(Value) -> Result[string, string]` |
| `to_string_pretty` | `(Value) -> Result[string, string]` |

The mapping:

| XML | Value |
| --- | --- |
| `<cfg><name>weave</name></cfg>` | `{"cfg": {"name": "weave"}}` |
| `<a k="v">text</a>` | `{"a": {"@attrs": {"k": "v"}, "#text": "text"}}` |
| `<r><i>1</i><i>2</i></r>` | `{"r": {"i": ["1", "2"]}}` — repeated siblings become a list |

> [!WARNING]
> **Mixed content loses its order**
> Text interleaved with child elements is concatenated into one `#text`: `<r>mixed<b>x</b>tail</r>` parses to `{"r": {"#text": "mixedtail", "b": "x"}}`, so a round trip moves the text ahead of the child. Fine for config files, wrong for documents — drive those with the [template](../references/entity_template_module.md) module instead.

## Related

- [Value](../references/entity_value_type.md)

- [json](../references/entity_json_module.md)

- [toml](../references/entity_toml_module.md)

- [data](../references/entity_data_module.md)

[← Back to SKILL.md](../SKILL.md)
