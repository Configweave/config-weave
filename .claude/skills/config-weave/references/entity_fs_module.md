# fs

_host module_

File IO — richer than wscript-std's standalone fs, which it replaces.

`use fs` — file IO, richer than wscript-std's standalone `fs` (which is **not** registered; this replaces it). All fallible functions return `Result[…, string]`.

| function | signature | notes |
| --- | --- | --- |
| `read` | `(path) -> Result[string, string]` | text |
| `read_bytes` | `(path) -> Result[List[int], string]` |  |
| `write` / `append` | `(path, content) -> Result[unit, string]` | write replaces; append creates if absent |
| `write_bytes` | `(path, List[int]) -> Result[unit, string]` |  |
| `copy` / `move` | `(from, to) -> Result[unit, string]` | move = rename, works on dirs |
| `delete` | `(path) -> Result[unit, string]` | file or symlink |
| `delete_dir` | `(path) -> Result[unit, string]` | recursive |
| `mkdir` | `(path) -> Result[unit, string]` | creates missing parents |
| `exists` / `is_file` / `is_dir` | `(path) -> bool` |  |
| `list_dir` | `(path) -> Result[List[string], string]` | sorted names |
| `metadata` | `(path) -> Result[Value, string]` | map: size, modified, readonly, is_file, is_dir, is_symlink, mode |
| `glob` | `(pattern) -> Result[List[string], string]` | sorted |
| `temp_file` / `temp_dir` | `() -> Result[string, string]` | fresh path returned |
| `symlink` | `(target, link) -> Result[unit, string]` |  |
| `read_link` | `(path) -> Result[string, string]` |  |

## Related

- [Host API](../references/concept_host_api.md)

- [path](../references/entity_path_module.md)

- [hash](../references/entity_hash_module.md)

[← Back to SKILL.md](../SKILL.md)
