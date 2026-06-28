# testlab

_host module (scenario driver)_

The host-side module a scenario driver runs against a live vmlab lab — Lab and Machine.

The `testlab` host module is available to scenario **driver scripts**, which run
**host-side** against a live vmlab lab (not inside an instance). The driver
exports `fn run(lab: Lab) -> bool` (or `Result[bool, string]`) and compiles in
stage-5 validation against this module.


| type | members |
| --- | --- |
| `Lab` | `lab.machine(name) -> Machine` |
| `Machine` | `m.exec`, `m.powershell`, `m.copy_in`, `m.reboot`, `m.apply_resource`, `m.check_resource`, `m.apply(dir)`, `m.check(dir)` |

## Related

- [Scenarios](../references/concept_scenarios.md)

- [vmlab backend](../references/entity_vmlab_backend.md)

- [Host API](../references/concept_host_api.md)

[← Back to SKILL.md](../SKILL.md)
