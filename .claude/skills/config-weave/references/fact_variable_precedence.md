# Variable precedence and overrides

Precedence (lowest → highest): **vars declaration → gatherer result → `--var-file` → `--var`**.

- `--var KEY=VALUE` parses VALUE as a WCL expression when possible (\`--var
  count=3\` is an int), falling back to a plain string. Repeatable.
- `--var-file file.wcl` is a flat `name = value` collection; expressions evaluate
  standalone and **cannot reference other variables**.
- **Gather params evaluate before variables resolve**: they may reference `--var`
  / `--var-file` overrides, but not gatherer results or vars that depend on them.
- Gatherer invocations all run concurrently and are deduplicated by \`(gatherer,
  canonicalized params)\`; any gatherer failure aborts before step execution.
- Conditions and properties evaluate lazily at run time against the full scope.


## Related

- [Variables](../references/concept_variables.md)

- [Gatherer](../references/concept_gatherer.md)

[← Back to SKILL.md](../SKILL.md)
