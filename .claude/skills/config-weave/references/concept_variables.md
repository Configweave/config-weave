# Variables

_Playbook vars, gatherer results and CLI overrides — scope, lazy evaluation, and the shadowing pitfall._

A playbook's `vars` block holds free-form `name = expr` bindings. Expressions may
reference gatherer results and other vars. Conditions and properties evaluate
**lazily at run time** against the full scope; gather params evaluate **before**
variables resolve. The full precedence and override rules are in
[Variable precedence](../references/fact_variable_precedence.md).


> [!WARNING]
> **Shadowing pitfall**
> Property/params block fields **shadow** outer variables: `url = url` inside a `properties` block is a self-reference cycle error. Use distinct names (`tool_url = ...` then `url = tool_url`).

## Related

- [Playbook](../references/concept_playbook.md)

- [Gatherer](../references/concept_gatherer.md)

- [Variable precedence and overrides](../references/fact_variable_precedence.md)

[← Back to SKILL.md](../SKILL.md)
