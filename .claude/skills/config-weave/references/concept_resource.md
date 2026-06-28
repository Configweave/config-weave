# Resource

_A declared unit of desired state, implemented by a wscript check()/apply() script._

A resource is a declared unit of desired state in a package, implemented by a
[wscript](../references/entity_wscript_lang.md) script that exports `check()` and `apply()`
([signatures](../references/fact_entry_point_signatures.md)). It is referenced from steps as
`pkg.resource`. The resource declares its inputs as `param` blocks and a
[concurrency class](../references/concept_concurrency_classes.md); its script obeys the
[check → apply → re-check contract](../references/concept_convergence_contract.md).


Resource, gatherer and verify scripts are single-file wscript programs compiled against the [config-weave host API](../references/concept_host_api.md). Import host modules with `use <module>`; registered types (`Value`, `CheckResult`, `ApplyResult`, `CmdOutput`, `HttpResponse`, `ComObject`) are ambient — no `use` needed.

## Related

- [Package](../references/concept_package.md)

- [Step](../references/concept_step.md)

- [Convergence contract](../references/concept_convergence_contract.md)

- [Host API](../references/concept_host_api.md)

- [Script entry-point signatures](../references/fact_entry_point_signatures.md)

- [Concurrency classes](../references/concept_concurrency_classes.md)

[← Back to SKILL.md](../SKILL.md)
