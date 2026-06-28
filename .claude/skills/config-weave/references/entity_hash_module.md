# hash

_host module_

Digests with hex output — sha256 / sha512 / md5, for strings and files.

`use hash` — digests (hex output). MD5 is legacy-interop only.

| function | signature |
| --- | --- |
| `sha256` / `sha512` / `md5` | `(string) -> string` |
| `sha256_file` / `sha512_file` / `md5_file` | `(path) -> Result[string, string]` |

## Related

- [Host API](../references/concept_host_api.md)

- [fs](../references/entity_fs_module.md)

[← Back to SKILL.md](../SKILL.md)
