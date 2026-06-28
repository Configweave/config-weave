# Step expectation table

`expect = converge | already_configured | error | skip | reboot_required` (— = unasserted):

| expect | run 1: check | run 2: apply | run 3: apply again |
| --- | --- | --- | --- |
| `converge` (default) | not_configured | configured | already_configured |
| `already_configured` | already_configured | already_configured | already_configured |
| `error` | — | error | — |
| `skip` | skipped | skipped | skipped |
| `reboot_required` | — | reboot_required | — |

## Related

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [Test block reference](../references/fact_test_block_fields.md)

[← Back to SKILL.md](../SKILL.md)
