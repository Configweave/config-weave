# playbook.wcl

_file format_

The WCL document at a playbook directory's root — declares plays, gathers and vars.

`playbook.wcl` is the WCL document at the root of a playbook directory. It declares the `playbook` block: `gather`s, `vars`, and `play`s of `step`s. The engine appends the system import `<weave/playbook.wcl>` automatically — never write import lines.

| Field | Value |
| --- | --- |
| Location | <playbook>/playbook.wcl |
| System import | <weave/playbook.wcl> (appended by the engine) |
| Top block | playbook "name" { … } (one per file) |
| Reference | Playbook block reference (fact) |

## Related

- [Playbook](../references/concept_playbook.md)

- [package.wcl](../references/entity_package_wcl.md)

- [Playbook block reference](../references/fact_playbook_blocks.md)

[← Back to SKILL.md](../SKILL.md)
