# Testlab backend requirements

| Backend | Requirement |
| --- | --- |
| docker | images must contain `sleep` and `sh` (distroless unsupported — use vmlab) |
| vmlab | templates must ship the QEMU guest agent; each group gets a throwaway one-VM lab |
| vmlab (Windows scenarios) | guests must be Server 2019 / Windows 10 or newer |

## Related

- [docker backend](../references/entity_docker_backend.md)

- [vmlab backend](../references/entity_vmlab_backend.md)

- [Scenarios](../references/concept_scenarios.md)

[← Back to SKILL.md](../SKILL.md)
