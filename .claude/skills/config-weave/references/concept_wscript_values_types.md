# wscript: values and types

_Value (primitive) types vs reference types; let inference and annotations; string building with fmt._

Primitives (value types): `int` (64-bit signed, wrapping), `float`, `bool`, `char`, `unit` (`()`). Everything else is a **reference type**: `string`, structs, enums, `List[T]`, `Map[K, V]`, function values, `weak[T]`.

```rust
let x = 5                  // inferred; annotations optional on lets
let name: string = "wil"
let log = "hp: " + str(99) // + concatenates strings; str() converts
let msg = fmt("{} of {}", 3, 10)   // NO string interpolation — use fmt()
```

Value vs reference behaviour is covered in [reference semantics](../references/concept_wscript_reference_semantics.md).

## Related

- [wscript: overview](../references/concept_wscript_overview.md)

- [wscript: reference semantics](../references/concept_wscript_reference_semantics.md)

- [wscript: containers and strings](../references/concept_wscript_containers.md)

[← Back to SKILL.md](../SKILL.md)
