# config-weave test flags

```console
config-weave test <playbook-dir>              # everything
config-weave test <dir> core                  # one package
config-weave test <dir> core:file_present_converges   # one test
```

| Flag | Meaning |
| --- | --- |
| `--backend NAME` | override every test's backend (`docker` or `vmlab`) |
| `--image IMAGE` | run every test against this image instead of its own |
| `--keep` | leave instances running for post-mortem debugging |
| `--binary PATH` | static linux config-weave binary to copy into instances |
| `--binary-windows PATH` | windows config-weave binary for windows vmlab guests |
| `--docker-jobs N` | max docker groups running at once (default `min(cpu, 8)`) |
| `--vmlab-jobs N` | max vmlab groups running at once (default `2` — VMs are heavy) |

## Related

- [Testlab](../references/concept_testlab.md)

- [Grouping tests into one instance](../references/concept_test_grouping.md)

- [docker backend](../references/entity_docker_backend.md)

- [vmlab backend](../references/entity_vmlab_backend.md)

[← Back to SKILL.md](../SKILL.md)
