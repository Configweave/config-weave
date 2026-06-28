# path

_host module_

Pure path-string manipulation — no IO.

`use path` — pure path-string manipulation, no IO.

| function | signature | notes |
| --- | --- | --- |
| `join` | `(a, b) -> string` |  |
| `parent` | `(p) -> string` | empty at root |
| `filename` | `(p) -> string` |  |
| `extension` | `(p) -> string` | no dot |
| `normalize` | `(p) -> string` | lexical `.`/`..` |
| `absolutize` | `(p) -> Result[string, string]` | against cwd, then normalize |

## Related

- [Host API](../references/concept_host_api.md)

- [fs](../references/entity_fs_module.md)

[← Back to SKILL.md](../SKILL.md)
