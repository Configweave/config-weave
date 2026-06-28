# Editor support (wscripti / LSP)

_config-weave wscripti emits the exact host interface so the wscript LSP type-checks scripts against it._

`config-weave wscripti [outdir]` emits [`weave.wscripti`](../references/entity_weave_wscripti.md)
— the full host interface — plus a starter `wscript.toml`. With those next to
your scripts, `wscript check` and the wscript LSP type-check scripts against the
**exact** config-weave host surface: host-API misuse becomes a compile-time error,
also caught by `config-weave validate`.


## Related

- [Host API](../references/concept_host_api.md)

- [weave.wscripti](../references/entity_weave_wscripti.md)

- [Resource](../references/concept_resource.md)

[← Back to SKILL.md](../SKILL.md)
