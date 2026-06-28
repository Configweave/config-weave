# docker backend

_test backend_

The default testlab backend — disposable Linux containers from an image.

The `docker` backend is the testlab's default. Each test (or test group) gets a disposable container from the test's `image`. It is Linux-only.

| Field | Value |
| --- | --- |
| backend value | "docker" (default) |
| image | a docker image ref, e.g. "debian:12" |
| requires | image must contain `sleep` and `sh` (distroless unsupported) |
| parallelism | --docker-jobs N (default min(cpu, 8)) |

## Related

- [Testlab](../references/concept_testlab.md)

- [vmlab backend](../references/entity_vmlab_backend.md)

- [Testlab backend requirements](../references/fact_testlab_backend_requirements.md)

- [config-weave test flags](../references/fact_testlab_flags.md)

[← Back to SKILL.md](../SKILL.md)
