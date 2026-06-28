# Testlab

_config-weave test — runs package tests in disposable docker containers or vmlab VMs._

The testlab (`config-weave test`, `src/testlab/`) proves package convergence in
**disposable instances**. Packages declare `test` blocks in `package.wcl`; each
test runs in a fresh backend instance — a [docker container](../references/entity_docker_backend.md)
(linux) or a [vmlab VM](../references/entity_vmlab_backend.md) (linux or windows) — and is proven
with the [three-run protocol](../references/concept_three_run_protocol.md).


The `test` block's fields are in [Test block reference](../references/fact_test_block_fields.md). Several tests can share one instance via [grouping](../references/concept_test_grouping.md); convergence the protocol can't express uses [scenarios](../references/concept_scenarios.md).

## Repo test suites

`just test` (fast cargo suite, no docker) · `just test-lab` (cross-builds the static musl binary, runs the docker-gated suite) · `just test-lab-vm playbook template` (end-to-end vmlab smoke) · `just test-ad` (the full Windows DC lifecycle scenario over real reboots — heavy).

## Related

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [Grouping tests into one instance](../references/concept_test_grouping.md)

- [Scenarios](../references/concept_scenarios.md)

- [docker backend](../references/entity_docker_backend.md)

- [vmlab backend](../references/entity_vmlab_backend.md)

- [Test block reference](../references/fact_test_block_fields.md)

- [Test a package for idempotence](../references/process_test_package.md)

[← Back to SKILL.md](../SKILL.md)
