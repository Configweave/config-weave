# Cross-process idempotence

_apply must converge across processes, not just within the running one — why test run 3 matters._

Idempotence in config-weave is **cross-process**: applying a converged playbook a
second time, in a \*fresh\* process, must report every step `already configured`.
A `check` that only passes against in-process state (a cache, an open handle, a
process-local variable) silently breaks the contract.


The [three-run protocol](../references/concept_three_run_protocol.md)'s third run exists precisely to catch this: it re-applies in a new process, so state that only existed in-process re-applies, surfaces as `configured`, and fails the test.

## Related

- [Convergence contract](../references/concept_convergence_contract.md)

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [Testlab](../references/concept_testlab.md)

[← Back to SKILL.md](../SKILL.md)
