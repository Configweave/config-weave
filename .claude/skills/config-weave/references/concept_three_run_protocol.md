# Three-run protocol

_check, apply, apply — run 2 proves in-process convergence, run 3 proves cross-process idempotence._

The three-run protocol is how the testlab proves convergence. It runs each test
through `check`, `apply`, `apply` (all `--json --continue-on-error`):

- **Run 1 (check)** — reports initial status, mutates nothing.
- **Run 2 (apply)** — converges; its internal re-check proves convergence
  **within one process**.
- **Run 3 (apply again)** — proves **cross-process idempotence**: a check that
  only passes on in-process state re-applies here, surfaces as `configured`, and
  fails the test.


Per-step outcomes are asserted with `expect` — see the [step expectation table](../references/fact_step_expectation_table.md).

## Related

- [Testlab](../references/concept_testlab.md)

- [Cross-process idempotence](../references/concept_idempotence.md)

- [Convergence contract](../references/concept_convergence_contract.md)

- [Step expectation table](../references/fact_step_expectation_table.md)

[← Back to SKILL.md](../SKILL.md)
