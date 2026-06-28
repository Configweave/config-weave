# vmlab backend

_test backend_

QEMU/KVM VM testlab backend — Linux or Windows guests, shelling out to the sibling vmlab CLI.

The `vmlab` backend runs tests in disposable QEMU/KVM VMs by shelling out to the sibling `../vmlab` CLI. It supports both Linux and Windows guests and is the only backend that can run [scenarios](../references/concept_scenarios.md).

| Field | Value |
| --- | --- |
| backend value | "vmlab" |
| image | a vmlab template ref, e.g. "x86_64/linux-modern" |
| requires | templates must ship the QEMU guest agent |
| instance | each group gets a throwaway one-VM lab |
| parallelism | --vmlab-jobs N (default 2 — VMs are heavy) |
| Windows guests | Server 2019 / Windows 10 or newer |

## Related

- [Testlab](../references/concept_testlab.md)

- [docker backend](../references/entity_docker_backend.md)

- [Scenarios](../references/concept_scenarios.md)

- [testlab](../references/entity_testlab_module.md)

- [Testlab backend requirements](../references/fact_testlab_backend_requirements.md)

[← Back to SKILL.md](../SKILL.md)
