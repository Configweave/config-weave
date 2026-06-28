# Convergence contract

_check never mutates; after a successful apply a re-check must return AlreadyConfigured._

The convergence contract is the core rule every resource script obeys:

- **`check` must never mutate.** It only reports status.
- **`apply` must converge** so that a re-check returns `AlreadyConfigured` — and
  it must converge **across processes** too, not just within the running one.


> [!WARNING]
> **Cross-process idempotence**
> `apply` must converge so the re-check passes even in a fresh process. The testlab's third run catches state that only exists in-process — see [Cross-process idempotence](../references/concept_idempotence.md).

How the contract plays out per step is the [step lifecycle](../references/concept_step_lifecycle.md); how the testlab proves it is the [three-run protocol](../references/concept_three_run_protocol.md).

## Related

- [Step lifecycle](../references/concept_step_lifecycle.md)

- [Cross-process idempotence](../references/concept_idempotence.md)

- [Resource](../references/concept_resource.md)

- [Three-run protocol](../references/concept_three_run_protocol.md)

[← Back to SKILL.md](../SKILL.md)
