# Value

_value type_

The single dynamic escape hatch — gatherer results, step params, json/toml documents.

`Value` is the single dynamic escape hatch in config-weave scripts — gatherer results, step params, and json/toml documents are all `Value`. It is ambient (no `use` needed); `use value` brings in its accessor methods.

```rust
enum Value {
    Null, Bool(bool), Int(int), Float(float), String(string),
    List(List[Value]), Map(Map[string, Value]),
}
```

Match on it like any enum, or use accessors: `get(key: string) -> Option[Value]` ·
`at(idx: int) -> Option[Value]` · `keys() -> List[string]` · `len() -> int` ·
`is_null() -> bool` · \`as_bool / as_int / as_float / as_string / as_list / as_map
-> Option\[…\]` (`as_float` also accepts `Int\`). Construct with variant syntax:
`Value::Map(#{ "family": Value::String(sys::family()) })`.


`params` in a resource/gatherer is a `Value::Map` with declared defaults already applied and types validated.

## Related

- [Gatherer](../references/concept_gatherer.md)

- [json](../references/entity_json_module.md)

- [toml](../references/entity_toml_module.md)

- [wscript: containers and strings](../references/concept_wscript_containers.md)

[← Back to SKILL.md](../SKILL.md)
