# Scenarios

_Scripted, multi-stage tests over a declared vmlab lab — for convergence the three-run protocol can't express._

Some convergence can't be expressed by the [three-run protocol](../references/concept_three_run_protocol.md):
a Windows DC promotion needs **apply → reboot → apply again**, and a member server
needs a **reachable DC** (a second networked VM). For these, a package declares a
`scenario` — a **declared vmlab lab** plus a wscript **driver script** that brings
the lab's VMs up by name, applies config-weave, reboots, and asserts.


```wcl
scenario "ad_matrix" {
  description = "Forest, additional DC and a member join over real reboots"
  lab    = "tests/ad-lab"          // dir holding a vmlab.wcl (vmlab only)
  script = "tests/ad_matrix.wscript"
}
```

The driver exports `fn run(lab: Lab) -> bool` (or `Result[bool, string]`) and runs **host-side** against the live lab, using the [testlab host module](../references/entity_testlab_module.md). Scenarios run **sequentially** after the parallel test groups. **Windows guests must be Server 2019 / Windows 10 or newer.**

## Related

- [Testlab](../references/concept_testlab.md)

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [vmlab backend](../references/entity_vmlab_backend.md)

- [testlab](../references/entity_testlab_module.md)

[← Back to SKILL.md](../SKILL.md)
