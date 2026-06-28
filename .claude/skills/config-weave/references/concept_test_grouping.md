# Grouping tests into one instance

_Same-group tests share one disposable instance; independent groups run in parallel._

By default each test provisions its own instance. Give several tests the \*\*same
non-empty `group`** (within a package) to run them sequentially inside **one\*\*
shared instance — amortizing container start, and especially VM boot.


Grouped tests must agree on `backend` and `image`, and they \*\*share OS state with
no reset between them\*\*, so use distinct paths/state per test. Independent groups
run **in parallel**, throttled by `--docker-jobs` / `--vmlab-jobs` (see
[testlab flags](../references/fact_testlab_flags.md)).


## Related

- [Testlab](../references/concept_testlab.md)

- [Test block reference](../references/fact_test_block_fields.md)

- [config-weave test flags](../references/fact_testlab_flags.md)

[← Back to SKILL.md](../SKILL.md)
