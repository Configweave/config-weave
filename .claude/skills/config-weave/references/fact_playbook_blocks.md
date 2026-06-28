# Playbook block reference

| Block | Fields | Notes |
| --- | --- | --- |
| `playbook "name"` | `description` (required), `version` (default `"0.0.0"`), `gather*`, `vars?`, `play*` | one per file |
| `gather "label"` | `description?`, `from` (required, `pkg.gatherer`), `params?` | label becomes the variable holding the result |
| `vars` | free-form `name = expr` | expressions may reference gatherer results and other vars |
| `play "name"` | `description` (required), `parallel` (default `true`), `step*`, `container*` | `parallel = false` = strict declaration order |
| `container "name"` | `description` (required), `condition?`, `step*`, `container*` | condition applies to all children |
| `step "name"` | `description` (required), `resource` (required), `condition?`, `requires?`, `concurrency?`, `properties?` | `concurrency` may \*tighten\* the resource's class, never loosen |
| `properties` / `params` | free-form `name = expr` | validated against the resource/gatherer `param` declarations |

`description` is mandatory wherever shown as required — the loader enforces it.

## Related

- [Playbook](../references/concept_playbook.md)

- [playbook.wcl](../references/entity_playbook_wcl.md)

- [Variables](../references/concept_variables.md)

[← Back to SKILL.md](../SKILL.md)
