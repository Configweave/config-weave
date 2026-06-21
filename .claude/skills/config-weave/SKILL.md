---
name: config-weave
description: "Expertise skill for config-weave: authoring WCL playbooks and packages, writing wscript resource/gatherer/verify scripts, the host API surface, running and testing playbooks. Single-binary configuration management driven by WCL playbooks and wscript resource scripts, with a check → apply → re-check convergence contract and a disposable-instance testlab. Auto-activated when working with playbook.wcl, package.wcl, .wscript scripts, the config-weave CLI, or the testlab."
wskill_schema_version: 1.1.0
allowed-tools:
  - Read
  - Write
  - Edit
  - Glob
  - Grep
  - Bash
  - Agent
disallowed-tools: []
disable-model-invocation: false
---

# config-weave

config-weave is single-binary configuration management. Playbooks are WCL documents whose plays run steps; each step invokes a resource (declared in a package, implemented as a wscript script) with a check → apply → re-check convergence contract. This skill captures every layer as data — playbooks, packages, scripts, the host API, the CLI, and the testlab — projected from one model.

## Parameters

Values to pass when invoking this skill — reference them as `$ARGUMENTS`, `$1`, `$2`, … in the prompt.

| Parameter | Description | How to determine the value |
| --- | --- | --- |
| $ARGUMENTS | The config-weave topic to look up — a playbook/package block, a wscript contract, a host-API module, a CLI subcommand, or a testlab feature. | Take it from the user's request. If empty, summarise the reference and ask which layer they need. |

<Boundary>

**Always:**

- Read docs/notes.md before changing the WCL vocabulary, variable scheme, host API surface, or test protocol — it is the binding source of truth over the PRD's sketches.

- Trust real source over these references if they disagree: src/vocab/\*.wcl, src/hostapi/\*.rs, src/main.rs, ~/dev/wscript/docs — then update the reference.

- Regenerate weave.wscripti (config-weave wscripti) after changing the host API, and update the host-API reference to match.

**Ask first:**

- Before running config-weave apply against the local machine (it mutates system state) — validate, check, and test are safe.

- Before adding new fields to the WCL vocabulary (src/vocab/\*.wcl) — that is a schema change, not playbook authoring.

**Never:**

- Invent host API functions or modules not listed in the host-API reference — the surface is exactly what config-weave wscripti emits.

- Use wscript-std's math / process / xml / standalone-fs in playbook scripts — they are not registered.

- Write WCL import lines in playbooks or packages — the engine appends system imports.

</Boundary>

## Reference

- [Concepts](references/concepts_ref.md) — playbooks, packages, scripts, the wscript language, the host API, and the testlab.

- [CLI reference](references/cli_ref.md) — the `config-weave` CLI: every subcommand, its arguments and switches.

- [Processes](references/processes_ref.md) — runbooks for scaffolding, adding resources, testing, and check/apply.

- [Glossary](references/glossary_ref.md) — config-weave vocabulary.
