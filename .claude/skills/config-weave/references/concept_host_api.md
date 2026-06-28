# Host API

_The wscript module surface config-weave registers for scripts — registered everywhere, foreign-platform functions error off their platform._

The host API is the wscript module surface config-weave registers for all scripts
(resources, gatherers, verify). It is **identical on every platform**:
foreign-platform functions exist everywhere and return runtime errors off their
platform, so playbooks compile and validate the same on Linux and Windows. Guard
platform-specific calls with a condition (`os.family == "windows"`) or
`sys::family()`.


Import a module with `use <module>`. All fallible functions return \`Result\[…,
string\]` and compose with `?\`. The authoritative surface is whatever
[`config-weave wscripti`](../references/entity_weave_wscripti.md) emits, generated from
`src/hostapi/*.rs`.


Cross-platform modules: [log](../references/entity_log_module.md), [fs](../references/entity_fs_module.md), [path](../references/entity_path_module.md), [shell](../references/entity_shell_module.md), [http](../references/entity_http_module.md), [hash](../references/entity_hash_module.md), [archive](../references/entity_archive_module.md), [env](../references/entity_env_module.md), [sys](../references/entity_sys_module.md), [data](../references/entity_data_module.md), [template](../references/entity_template_module.md). Windows-only modules: [registry](../references/entity_registry_module.md), [service](../references/entity_service_module.md), [com](../references/entity_com_module.md).

## Related

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

- [weave.wscripti](../references/entity_weave_wscripti.md)

- [Editor support (wscripti / LSP)](../references/concept_editor_support.md)

- [fs](../references/entity_fs_module.md)

- [registry](../references/entity_registry_module.md)

[← Back to SKILL.md](../SKILL.md)
