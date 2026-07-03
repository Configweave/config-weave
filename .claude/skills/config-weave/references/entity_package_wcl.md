# package.wcl

_file format_

The WCL document under pkgs/<name>/ declaring resources, gatherers and tests.

`package.wcl` is the WCL document under `pkgs/<name>/` inside a playbook directory. It declares the `package` block: `resource`s, `gatherer`s and `test`s. The engine appends the system import `<weave/package.wcl>` automatically.

| Field | Value |
| --- | --- |
| Location | <playbook>/pkgs/<name>/package.wcl |
| System import | <weave/package.wcl> (appended by the engine) |
| Top block | package "name" { … } |
| Reference | Package block reference (fact) |

## Related

- [Package](../references/concept_package.md)

- [playbook.wcl](../references/entity_playbook_wcl.md)

- [Package block reference](../references/fact_package_blocks.md)

[← Back to SKILL.md](../SKILL.md)
