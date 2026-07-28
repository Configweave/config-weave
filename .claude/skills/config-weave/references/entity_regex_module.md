# regex

_library_

Regular expressions over strings; the pattern comes first in every signature, and an invalid pattern is a fault, not an Err.

`use regex` — pattern matching without shelling out to `grep`/`sed`, which is a subprocess and platform-dependent. Rust `regex` syntax: no backreferences, no lookaround, linear time.

| function | signature |
| --- | --- |
| `is_match` | `(pattern, text) -> bool` |
| `find` | `(pattern, text) -> Option[string]` — first match |
| `find_all` | `(pattern, text) -> List[string]` — every non-overlapping match, in order |
| `replace` | `(pattern, text, replacement) -> string` — every match; `$1` / `$name` expand capture groups |
| `captures` | `(pattern, text) -> Option[List[string]]` — group 0 (whole match) first; non-participating groups are empty strings |
| `split` | `(pattern, text) -> List[string]` |

> [!WARNING]
> **Pattern first**
> Every function takes `(pattern, text)`, not `(text, pattern)`. The argument types are identical, so swapping them compiles and then silently never matches.

> [!NOTE]
> **An invalid pattern faults**
> A malformed pattern is a runtime fault, not a `Result` — patterns are literals in practice, and a script has no way to recover from a bad one. It surfaces as the step's Error status.

## Related

- [Value](../references/entity_value_type.md)

- [shell](../references/entity_shell_module.md)

- [data](../references/entity_data_module.md)

[← Back to SKILL.md](../SKILL.md)
