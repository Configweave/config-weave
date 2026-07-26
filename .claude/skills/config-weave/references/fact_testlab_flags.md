# config-weave test flags

```console
config-weave test <playbook-dir>              # everything
config-weave test <dir> core                  # one package
config-weave test <dir> core:file_present_converges   # one test
```

| Flag | Meaning |
| --- | --- |
| `--image IMAGE` | run every \*container\* test against this OCI image instead of its own |
| `--template REF` | run every \*VM\* test against this vmlab template instead of its own |
| `--keep` | leave instances running for post-mortem debugging |
| `--binary PATH` | static linux config-weave binary to copy into instances |
| `--binary-windows PATH` | windows config-weave binary for windows guests |
| `--container-jobs N` | max container groups running at once (default `min(cpu, 8)`) |
| `--vm-jobs N` | max VM groups running at once (default `2` — VMs are heavy) |

Neither `--image` nor `--template` can convert a test between kinds: they replace the reference only for tests that already declare that field.

## Related

- [Testlab](../references/concept_testlab.md)

- [Grouping tests into one instance](../references/concept_test_grouping.md)

- [container instance](../references/entity_container_instance.md)

- [VM instance](../references/entity_vm_instance.md)

[← Back to SKILL.md](../SKILL.md)
