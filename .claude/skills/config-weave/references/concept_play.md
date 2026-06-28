# Play

_A named group of steps inside a playbook; the unit check/apply targets._

A play is a named group of steps inside a playbook. `config-weave check` and
`config-weave apply` each target **one** play by name. A play declares
`parallel = true` by default: its steps run concurrently, dispatched over a DAG as
their dependencies complete (see [DAG scheduling](../references/concept_dag_scheduling.md)). Set
`parallel = false` to force strict declaration order instead.


Steps can be grouped for organisation with a [container](../references/concept_container.md), and ordered with `requires` edges. The scheduler is also bounded by each resource's [concurrency class](../references/concept_concurrency_classes.md).

## Related

- [Playbook](../references/concept_playbook.md)

- [Step](../references/concept_step.md)

- [Container](../references/concept_container.md)

- [DAG scheduling](../references/concept_dag_scheduling.md)

- [Concurrency classes](../references/concept_concurrency_classes.md)

[← Back to SKILL.md](../SKILL.md)
