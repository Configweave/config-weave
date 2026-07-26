# Testlab instance requirements

`config-weave test` shells out to the sibling `vmlab` CLI for every instance, so vmlab and KVM are the only host requirements — there is no container runtime to install.

| Instance | Requirement |
| --- | --- |
| container (`image`) | an OCI image with a shell; pulled by vmlab, run in a micro-VM |
| vm (`template`) | the template must ship the vmlab guest agent; each group gets a throwaway one-VM lab |
| vm (Windows) | guests must be Server 2019 / Windows 10 or newer |

## Related

- [container instance](../references/entity_container_instance.md)

- [VM instance](../references/entity_vm_instance.md)

- [Scenarios](../references/concept_scenarios.md)

[← Back to SKILL.md](../SKILL.md)
