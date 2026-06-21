# wscript built-ins

_The prelude, container/string/Option/Result methods, the Value type, and the json/toml modules available in config-weave scripts._

> [!WARNING]
> **Not available in config-weave scripts**
> wscript-std's `math`, `process`, `xml`, and standalone `fs` modules are **not registered**. config-weave provides its own richer `fs`, and `shell` replaces `process`. There is no `math` module; use plain operators.

## Prelude (always available, no import)

| function | signature | notes |
| --- | --- | --- |
| `print` / `println` | `(any)` / `(any?)` | redirected into `log::info` |
| `str` | `(any) -> string` | uses `Display` impls |
| `fmt` | `(string, any…) -> string` | `{}` placeholders; `{{`/`}}` escape; count compile-checked |
| `same` | `(T, T) -> bool` | reference identity |
| `weak` | `(T) -> weak[T]` | reference types only |
| `int` | `(int\|float\|char) -> int` | float truncates; char gives code point |
| `float` | `(int\|float) -> float` |  |

## String methods (immutable; char-indexed)

`len bytes_len is_empty split trim trim_start trim_end to_upper to_lower starts_with ends_with contains find replace repeat pad_left pad_right chars slice parse_int parse_float`

`find -> Option[int]`; `parse_int/parse_float -> Result[…, string]`; concatenate with `+`.

## List methods

`len is_empty push pop get set insert remove clear contains index_of reverse sort join map filter fold first last slice concat clone`

`xs[i]` faults out of bounds; `xs.get(i) -> Option[T]` never does.

## Map methods (keys: int|bool|char|string)

`len is_empty insert remove get contains_key keys values clear clone` · `m[k]` faults on a missing key (reads); `m[k] = v` inserts or overwrites.

## Option / Result methods

`Option`: `is_some is_none unwrap unwrap_or expect` · `Result`: `is_ok is_err unwrap unwrap_or unwrap_err expect` · plus the `?` operator.

## The Value type (use value for the methods)

The single dynamic escape hatch — gatherer results, step params, json/toml documents:

```rust
enum Value {
    Null, Bool(bool), Int(int), Float(float), String(string),
    List(List[Value]), Map(Map[string, Value]),
}
```

Match on it like any enum, or use accessors: `get(key: string) -> Option[Value]` ·
`at(idx: int) -> Option[Value]` · `keys() -> List[string]` · `len() -> int` ·
`is_null() -> bool` · \`as_bool / as_int / as_float / as_string / as_list / as_map ->
Option\[…\]` (`as_float` also accepts `Int\`). Construct with variant syntax:
`Value::Map(#{ "family": Value::String(sys::family()) })`.


## json (use json)

| function | signature |
| --- | --- |
| `parse` | `(string) -> Result[Value, string]` |
| `to_string` | `(Value) -> string` (keys sorted — deterministic) |
| `to_string_pretty` | `(Value) -> string` |

## toml (use toml)

| function | signature |
| --- | --- |
| `parse` | `(string) -> Result[Value, string]` (datetimes become strings) |
| `to_string` | `(Value) -> Result[string, string]` |
| `to_string_pretty` | `(Value) -> Result[string, string]` |

TOML serialization fails on `Null` anywhere and on non-map top levels. INI lives in the host `data` module ([host API](../references/concept_hostapi.md)).

## Related

- [The wscript language](../references/concept_wscript_language.md)

- [Host API — cross-platform](../references/concept_hostapi.md)

[← All concepts](../references/concepts_ref.md)
