# VM instance

_test instance_

A test that declares `template` — a full QEMU/KVM VM cloned from a vmlab template. Linux or Windows, real init, reboots.

A `test` block declaring `template` runs in a full **VM** cloned from a vmlab template. It is the only kind with a real init system, its own kernel and the ability to reboot, and the only one that can run Windows guests. [Scenarios](../references/concept_scenarios.md) always use VMs.

| Field | Value |
| --- | --- |
| declared with | template = "x86_64/ubuntu-24.04" |
| guest OS | Linux or Windows, detected from the guest agent |
| requires | the template must ship the vmlab guest agent |
| instance | each group gets a throwaway one-VM lab |
| parallelism | --vm-jobs N (default 2 — VMs are heavy) |
| Windows guests | Server 2019 / Windows 10 or newer |

Reach for a VM when the test needs systemd/OpenRC/runit to actually be running, a reboot, or Windows. A fresh Windows clone costs several minutes on its first boot, so group Windows tests together.

## Related

- [Testlab](../references/concept_testlab.md)

- [container instance](../references/entity_container_instance.md)

- [Scenarios](../references/concept_scenarios.md)

- [testlab](../references/entity_testlab_module.md)

- [Testlab instance requirements](../references/fact_testlab_backend_requirements.md)

[← Back to SKILL.md](../SKILL.md)
