---
name: config-weave
description: "Expertise skill for config-weave: authoring WCL playbooks and packages, writing wscript resource/gatherer/verify scripts, the host API surface, running and testing playbooks. Single-binary configuration management driven by WCL playbooks and wscript resource scripts, with a check → apply → re-check convergence contract and a disposable-instance testlab. Auto-activated when working with playbook.wcl, package.wcl, .wscript scripts, the config-weave CLI, or the testlab."
wskill_schema_version: 1.0.0
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

Single-binary configuration management driven by WCL playbooks and wscript resource scripts, with a check → apply → re-check convergence contract and a disposable-instance testlab.

**Upstream version:** `0.1.0`. If the real upstream has moved past this, the skill may be stale — bump `topic.version` and re-verify (see the update workflow).

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

### Foundations

_What config-weave is and the convergence model every resource obeys._

- [config-weave](references/entity_config_weave.md)

- [Convergence contract](references/concept_convergence_contract.md)

- [Step lifecycle](references/concept_step_lifecycle.md)

- [Cross-process idempotence](references/concept_idempotence.md)

- [Concurrency classes](references/concept_concurrency_classes.md)

- [CheckResult and ApplyResult](references/fact_result_enums.md)

### Authoring playbooks & packages

_The WCL building blocks: plays of steps, packages of resources and gatherers._

- [Playbook](references/concept_playbook.md)

- [Play](references/concept_play.md)

- [Step](references/concept_step.md)

- [Container](references/concept_container.md)

- [Package](references/concept_package.md)

- [Resource](references/concept_resource.md)

- [Gatherer](references/concept_gatherer.md)

- [Variables](references/concept_variables.md)

- [DAG scheduling](references/concept_dag_scheduling.md)

- [playbook.wcl](references/entity_playbook_wcl.md)

- [package.wcl](references/entity_package_wcl.md)

#### Block reference

_The playbook / package block tables and the variable rules._

- [Playbook block reference](references/fact_playbook_blocks.md)

- [Package block reference](references/fact_package_blocks.md)

- [Concurrency classes](references/fact_concurrency_class_table.md)

- [Variable precedence and overrides](references/fact_variable_precedence.md)

### The wscript language

_The statically typed, Rust-flavored language resources and gatherers are written in._

- [wscript](references/entity_wscript_lang.md)

- [wscript: overview](references/concept_wscript_overview.md)

- [wscript: values and types](references/concept_wscript_values_types.md)

- [wscript: reference semantics](references/concept_wscript_reference_semantics.md)

- [wscript: functions and closures](references/concept_wscript_functions.md)

- [wscript: structs, enums, methods](references/concept_wscript_structs_enums.md)

- [wscript: pattern matching](references/concept_wscript_pattern_matching.md)

- [wscript: Option, Result and ?](references/concept_wscript_option_result.md)

- [wscript: containers and strings](references/concept_wscript_containers.md)

- [wscript: loops](references/concept_wscript_loops.md)

- [wscript: traits and operators](references/concept_wscript_traits_operators.md)

- [wscript: memory and faults](references/concept_wscript_memory_faults.md)

#### Built-ins & standard library

_The prelude, container/string/Option/Result methods, the Value type, and json/toml._

- [wscript prelude](references/fact_wscript_prelude.md)

- [wscript string methods](references/fact_wscript_string_methods.md)

- [wscript list methods](references/fact_wscript_list_methods.md)

- [wscript map methods](references/fact_wscript_map_methods.md)

- [Option / Result methods](references/fact_wscript_option_result_methods.md)

- [Value](references/entity_value_type.md)

- [json](references/entity_json_module.md)

- [toml](references/entity_toml_module.md)

- [Not registered in config-weave scripts](references/fact_wscript_not_registered.md)

- [Excluded from wscript v1](references/fact_wscript_excluded_v1.md)

### Host API

_The wscript module surface config-weave registers for scripts._

- [Host API](references/concept_host_api.md)

- [Editor support (wscripti / LSP)](references/concept_editor_support.md)

- [weave.wscripti](references/entity_weave_wscripti.md)

- [Script entry-point signatures](references/fact_entry_point_signatures.md)

#### Cross-platform modules

_Registered on every platform._

- [log](references/entity_log_module.md)

- [fs](references/entity_fs_module.md)

- [path](references/entity_path_module.md)

- [shell](references/entity_shell_module.md)

- [http](references/entity_http_module.md)

- [hash](references/entity_hash_module.md)

- [archive](references/entity_archive_module.md)

- [env](references/entity_env_module.md)

- [sys](references/entity_sys_module.md)

- [data](references/entity_data_module.md)

- [template](references/entity_template_module.md)

#### Windows modules

_Registered everywhere; runtime-error off Windows._

- [registry](references/entity_registry_module.md)

- [service](references/entity_service_module.md)

- [com](references/entity_com_module.md)

### Testing & the testlab

_Proving package convergence in disposable docker containers or vmlab VMs._

- [Testlab](references/concept_testlab.md)

- [Three-run protocol](references/concept_three_run_protocol.md)

- [Grouping tests into one instance](references/concept_test_grouping.md)

- [Scenarios](references/concept_scenarios.md)

- [docker backend](references/entity_docker_backend.md)

- [vmlab backend](references/entity_vmlab_backend.md)

- [testlab](references/entity_testlab_module.md)

#### Test reference

_The test block, the expectation table, flags, exit codes and backend requirements._

- [Test block reference](references/fact_test_block_fields.md)

- [Step expectation table](references/fact_step_expectation_table.md)

- [config-weave test flags](references/fact_testlab_flags.md)

- [config-weave test exit codes](references/fact_testlab_exit_codes.md)

- [Testlab backend requirements](references/fact_testlab_backend_requirements.md)

### Task runbooks

_Step-by-step runbooks for authoring, testing and applying playbooks._

- [Scaffold and validate a playbook](references/process_scaffold_validate.md)

- [Add a package resource](references/process_add_resource.md)

- [Test a package for idempotence](references/process_test_package.md)

- [Check, then apply a play](references/process_check_then_apply.md)

- [CLI reference](references/cli_ref.md) — every `config-weave` subcommand, its arguments and switches

- [Glossary](references/glossary_ref.md) — config-weave vocabulary

## Views

Beyond this skill, the wskill ships these views — build them with `just render` in the wskill folder:

- **Reference book** — The comprehensive human reference — every layer of config-weave, curated into chapters. (`wdoc/book/main.wcl`)

- **Claude Code skill** — The Claude Code expertise skill (committed at .claude/skills/config-weave). (`wdoc/skill/main.wcl`)

- **Overview deck** — An introduction to config-weave as an overview deck — the model, packages, and the testlab. (`wdoc/presentation/main.wcl`)

- **Training course** — First playbook → packages → testlab: a hands-on lesson series with verifiable exercises. (`wdoc/training/main.wcl`)
