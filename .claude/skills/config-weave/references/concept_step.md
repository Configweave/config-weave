# Step

_One unit of work in a play — names a resource and supplies its properties._

A step is one unit of work in a play. It names a `resource` (qualified as
`pkg.resource`) and supplies a `properties` block validated against that
resource's declared params. A step also carries optional modifiers:


- `condition` — a bool expression; when `false` the step reports **Skipped** and
  does not run.
- `requires` — a list of sibling step **names**: ordering edges in the DAG (see
  [DAG scheduling](../references/concept_dag_scheduling.md)).
- `concurrency` — may **tighten** the resource's [concurrency class](../references/concept_concurrency_classes.md),
  never loosen it.


At run time each step executes the [check → apply → re-check lifecycle](../references/concept_step_lifecycle.md).

## Related

- [Play](../references/concept_play.md)

- [Resource](../references/concept_resource.md)

- [Step lifecycle](../references/concept_step_lifecycle.md)

- [Concurrency classes](../references/concept_concurrency_classes.md)

- [DAG scheduling](../references/concept_dag_scheduling.md)

[← Back to SKILL.md](../SKILL.md)
