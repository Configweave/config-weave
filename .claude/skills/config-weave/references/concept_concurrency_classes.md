# Concurrency classes

_A resource's scheduling restriction: parallel, exclusive, or global; a step may tighten but never loosen it._

A concurrency class is a scheduling restriction declared on a resource and
honoured by the DAG scheduler. The three classes — `parallel`, `exclusive`,
`global` — are defined in the [concurrency class table](../references/fact_concurrency_class_table.md).


A step may **tighten** its resource's class (e.g. mark a normally-parallel resource `exclusive` for one step) but never **loosen** it. Tightening lets a playbook serialise a step that would otherwise race, without changing the resource definition.

## Related

- [Resource](../references/concept_resource.md)

- [Step](../references/concept_step.md)

- [DAG scheduling](../references/concept_dag_scheduling.md)

- [Concurrency classes](../references/fact_concurrency_class_table.md)

[← Back to SKILL.md](../SKILL.md)
