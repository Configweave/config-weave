# Concurrency classes

Declared on the resource; a step may \*tighten\* its resource's class but never loosen it.

| Class | Meaning |
| --- | --- |
| `parallel` (default) | no restriction |
| `exclusive` | at most one step using this resource type at a time (the apt/MSI lock case) |
| `global` | step runs completely alone: scheduler drains in-flight steps, runs solo, resumes |

## Related

- [Concurrency classes](../references/concept_concurrency_classes.md)

- [DAG scheduling](../references/concept_dag_scheduling.md)

[← Back to SKILL.md](../SKILL.md)
