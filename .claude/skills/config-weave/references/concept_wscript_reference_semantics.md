# wscript: reference semantics

_Reference types copy the reference, not the data; same() tests identity; deep copy is explicit via Clone._

**Reference semantics**: assignment, passing and returning a reference type copies the **reference**, never the data. `same(a, b)` tests reference identity; deep copy is explicit via `#[derive(Clone)]` + `.clone()`.

`self` (on methods) is implicit and always by reference — there is no `&` in wscript.

## Related

- [wscript: values and types](../references/concept_wscript_values_types.md)

- [wscript: memory and faults](../references/concept_wscript_memory_faults.md)

- [wscript: structs, enums, methods](../references/concept_wscript_structs_enums.md)

[← Back to SKILL.md](../SKILL.md)
