# archive

_host module_

Extraction with no external tar/unzip needed — zip and tar.gz.

`use archive` — extraction (no external tar/unzip needed). All return `Result[int, string]` returning the entry count.

| function | signature |
| --- | --- |
| `extract_zip` | `(archive, dest) -> Result[int, string]` |
| `extract_tar_gz` | `(archive, dest) -> Result[int, string]` |
| `extract` | `(archive, dest) -> Result[int, string]` — auto by extension (.zip, .tar.gz, .tgz) |

## Related

- [Host API](../references/concept_host_api.md)

- [http](../references/entity_http_module.md)

- [fs](../references/entity_fs_module.md)

[← Back to SKILL.md](../SKILL.md)
