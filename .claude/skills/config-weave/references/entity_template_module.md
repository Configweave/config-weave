# template

_host module_

Tera template rendering (backs linux_files.template); autoescape off for config files.

`use template` — Tera rendering (backs `linux_files.template`).
`render(template, vars) -> Result[string, string]` renders a Tera template string
against a `vars` map; autoescape is **off** (config files, not HTML). A non-map
`vars` errors (`Null` is an empty context). Gives `{{ x }}`, `{% for %}`,
`{% if %}`, and filters.


> [!NOTE]
> **Authoring template bodies**
> Author template bodies as raw WCL heredocs (`<<'TMPL'`) so WCL's `$"…${}"` interpolation leaves Tera's `{{ }}`/`{% %}` alone; pass data via `vars`.

## Related

- [Host API](../references/concept_host_api.md)

- [Value](../references/entity_value_type.md)

[← Back to SKILL.md](../SKILL.md)
