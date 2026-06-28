# DAG scheduling

_How requires edges and concurrency classes drive step dispatch; requires is ordering, not a success demand._

In a `parallel` play the DAG scheduler dispatches steps as their dependencies
complete, subject to each resource's [concurrency class](../references/concept_concurrency_classes.md).
Ordering comes from each step's `requires` (a list of sibling step names).


- `requires` is **ordering, not a success demand**: a \*skipped\* dependency does
  not block a dependent; an \*errored\* or \*not-run\* dependency blocks dependents
  (they report `not run`) in apply mode.
- Steps left undispatched when a run halts report **not run**.
- A `global`-class step runs completely alone: the scheduler drains in-flight
  steps, runs it solo, then resumes.


## Related

- [Play](../references/concept_play.md)

- [Step](../references/concept_step.md)

- [Concurrency classes](../references/concept_concurrency_classes.md)

[← Back to SKILL.md](../SKILL.md)
