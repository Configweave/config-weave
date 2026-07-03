# weave.wscripti

_file format_

The full host interface config-weave wscripti emits — the authoritative host surface for editors and the LSP.

`weave.wscripti` is the wscript interface file emitted by `config-weave wscripti [outdir]`, generated from `src/hostapi/*.rs`. It is the **authoritative** host API surface: with it (plus a starter `wscript.toml`) next to your scripts, `wscript check` and the wscript LSP type-check against the exact config-weave surface.

| Field | Value |
| --- | --- |
| Emitted by | config-weave wscripti \[outdir\] |
| Generated from | src/hostapi/\*.rs |
| Also emits | a starter wscript.toml |
| Used by | wscript check / the wscript LSP / config-weave validate |

## Related

- [Host API](../references/concept_host_api.md)

- [Editor support (wscripti / LSP)](../references/concept_editor_support.md)

- [config-weave](../references/entity_config_weave.md)

[← Back to SKILL.md](../SKILL.md)
