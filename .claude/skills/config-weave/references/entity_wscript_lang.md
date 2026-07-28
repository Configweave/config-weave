# wscript

_software_

The statically typed, Rust-flavored scripting language resources, gatherers and verify scripts are written in.

wscript is the scripting language config-weave embeds for resources, gatherers and verify scripts. Compiled against the config-weave host API; scripts may import shared helpers from `lib/`. Its features are documented as the `wscript:` concepts.

| Field | Value |
| --- | --- |
| Flavour | Rust minus borrow checker, lifetimes, generics |
| Typing | Static, compile-time checked |
| Scripts | Multi-file — `use` imports helpers from `lib/` (see [Shared script helpers](../references/concept_script_imports.md)) |
| Compiled against | the config-weave host API (weave.wscripti) |
| Reference | the `wscript` wskill (~/dev/wscript/docs/wskills/wscript/) |

## Related

- [wscript: overview](../references/concept_wscript_overview.md)

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

- [Host API](../references/concept_host_api.md)

[← Back to SKILL.md](../SKILL.md)
