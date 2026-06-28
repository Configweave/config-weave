# Package block reference

| Block | Fields | Notes |
| --- | --- | --- |
| `package "name"` | `description` (required), `gatherer*`, `resource*`, `test*` | name qualifies playbook refs: `core.file_present` |
| `gatherer "name"` | `description`, `script`, `param*` | script exports `gather(params: Value) -> Value` |
| `resource "name"` | `description`, `script`, `concurrency` (default `"parallel"`), `param*` | script exports `check()` + `apply()` |
| `param "name"` | `description`, `type`, `required` (default `false`), `default?` | types: `string\|int\|float\|bool\|list\|map` |
| `test "name"` | see the Test block reference | run by `config-weave test` in disposable instances |

## Related

- [Package](../references/concept_package.md)

- [package.wcl](../references/entity_package_wcl.md)

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

[← Back to SKILL.md](../SKILL.md)
