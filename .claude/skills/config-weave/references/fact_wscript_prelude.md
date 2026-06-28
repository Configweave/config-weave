# wscript prelude

Always available, no import:

| function | signature | notes |
| --- | --- | --- |
| `print` / `println` | `(any)` / `(any?)` | redirected into `log::info` |
| `str` | `(any) -> string` | uses `Display` impls |
| `fmt` | `(string, any…) -> string` | `{}` placeholders; `{{`/`}}` escape; count compile-checked |
| `same` | `(T, T) -> bool` | reference identity |
| `weak` | `(T) -> weak[T]` | reference types only |
| `int` | `(int\|float\|char) -> int` | float truncates; char gives code point |
| `float` | `(int\|float) -> float` |  |

## Related

- [wscript: overview](../references/concept_wscript_overview.md)

- [wscript: values and types](../references/concept_wscript_values_types.md)

[← Back to SKILL.md](../SKILL.md)
