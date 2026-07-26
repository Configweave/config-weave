# container instance

_test instance_

A test that declares `image` — the OCI image booted in a vmlab micro-VM. Linux, ready in seconds.

A `test` block declaring `image` runs in a vmlab **container**: the OCI image you name, booted inside a micro-VM. It is Linux-only and ready in seconds, which makes it the default choice for anything that is really just a userland — file, package and config resources.

Unlike an unprivileged container runtime, it holds a full capability set and its own kernel, so resources that need `NET_ADMIN` or a real kernel (nftables, ufw, firewalld) work here too.

| Field | Value |
| --- | --- |
| declared with | image = "debian:12" |
| guest OS | Linux, always |
| requires | an image with a shell; no host container runtime |
| parallelism | --container-jobs N (default min(cpu, 8)) |

Two measured gotchas: `dnf5` loads its repositories fine in a Fedora container but then wedges for minutes on the transaction itself (use a [VM](../references/entity_vm_instance.md) for real dnf installs), and a test needing a live init system, a reboot or a Windows guest must use a VM.

## Related

- [Testlab](../references/concept_testlab.md)

- [VM instance](../references/entity_vm_instance.md)

- [Testlab instance requirements](../references/fact_testlab_backend_requirements.md)

- [config-weave test flags](../references/fact_testlab_flags.md)

[← Back to SKILL.md](../SKILL.md)
