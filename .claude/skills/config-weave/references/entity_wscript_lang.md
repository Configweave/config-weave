# wscript

_language_

The statically typed, Rust-flavored scripting language resources, gatherers and verify scripts are written in.

wscript is the scripting language config-weave embeds for resources, gatherers and verify scripts. Single-file in v1, compiled against the config-weave host API. Its features are documented as the `wscript:` concepts.

| Field | Value |
| --- | --- |
| Flavour | Rust minus borrow checker, lifetimes, generics |
| Typing | Static, compile-time checked |
| Scripts | Single file (no script-to-script imports in v1) |
| Compiled against | the config-weave host API (weave.wscripti) |
| Tour | ~/dev/wscript/docs/tour.md |

## Related

- [wscript: overview](../references/concept_wscript_overview.md)

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

- [Host API](../references/concept_host_api.md)

[← Back to SKILL.md](../SKILL.md)
