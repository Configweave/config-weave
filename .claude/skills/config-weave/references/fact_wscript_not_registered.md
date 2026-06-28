# Not registered in config-weave scripts

> [!WARNING]
> **Not available**
> wscript-std's `math`, `process`, `xml`, and standalone `fs` modules are **not registered** in config-weave scripts.

config-weave provides its own richer [fs](../references/entity_fs_module.md), and [shell](../references/entity_shell_module.md) replaces `process`. There is no `math` module — use plain operators. JSON/TOML live in the [json](../references/entity_json_module.md)/[toml](../references/entity_toml_module.md) modules; INI in the host [data](../references/entity_data_module.md) module.

## Related

- [Host API](../references/concept_host_api.md)

- [shell](../references/entity_shell_module.md)

- [fs](../references/entity_fs_module.md)

[← Back to SKILL.md](../SKILL.md)
