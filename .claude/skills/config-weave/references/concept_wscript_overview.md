# wscript: overview

_Statically typed, Rust-flavored scripting — Rust minus borrow checker, lifetimes and generics; single-file in v1._

wscript is a statically typed, Rust-flavored scripting language (Rust minus the
borrow checker, lifetimes and generics). config-weave embeds it to implement
resources, gatherers and verify scripts. Scripts are **single files in v1** — no
script-to-script imports. Full tour: `~/dev/wscript/docs/tour.md`.


- **No implicit numeric conversion** (`1 + 2.0` is a type error — use
  `int()`/`float()`).
- **No truthiness** — conditions must be `bool`.
- Statements end at newlines; semicolons are permitted but never required.


What is deliberately absent is listed in [Excluded from wscript v1](../references/fact_wscript_excluded_v1.md).

## Related

- [wscript](../references/entity_wscript_lang.md)

- [wscript: values and types](../references/concept_wscript_values_types.md)

- [Excluded from wscript v1](../references/fact_wscript_excluded_v1.md)

- [Resource](../references/concept_resource.md)

[← Back to SKILL.md](../SKILL.md)
