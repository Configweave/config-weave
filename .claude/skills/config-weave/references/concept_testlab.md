# Testlab

_config-weave test — runs package tests in disposable vmlab containers or VMs._

The testlab (`config-weave test`, `src/testlab/`) proves package convergence in
**disposable instances**. Packages declare `test` blocks in `package.wcl`; each
test runs in a fresh instance and is proven with the
[three-run protocol](../references/concept_three_run_protocol.md).


Every instance is a vmlab machine — vmlab is the only backend. A test declares
exactly one of two fields, and that choice is the whole selection:
`image = "debian:12"` gives a [container](../references/entity_container_instance.md) (the OCI
image booted in a micro-VM: linux, ready in seconds), and
`template = "x86_64/ubuntu-24.04"` gives a [VM](../references/entity_vm_instance.md) (linux or
windows, a real init system, and the only kind that can reboot).


The `test` block's fields are in [Test block reference](../references/fact_test_block_fields.md). Several tests can share one instance via [grouping](../references/concept_test_grouping.md); convergence the protocol can't express uses [scenarios](../references/concept_scenarios.md).

## Repo test suites

`just ci::test` (fast cargo suite, host-independent; part of the `just ci::check` merge bar) · `just test-lab` (cross-builds the static musl binary, runs the vmlab-gated suite in containers) · `just test-lab-vm playbook template` (end-to-end smoke in full VMs) · `just test-ad` (the full Windows DC lifecycle scenario over real reboots — heavy).

## Related

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [Grouping tests into one instance](../references/concept_test_grouping.md)

- [Scenarios](../references/concept_scenarios.md)

- [container instance](../references/entity_container_instance.md)

- [VM instance](../references/entity_vm_instance.md)

- [Test block reference](../references/fact_test_block_fields.md)

- [Test a package for idempotence](../references/process_test_package.md)

[← Back to SKILL.md](../SKILL.md)
