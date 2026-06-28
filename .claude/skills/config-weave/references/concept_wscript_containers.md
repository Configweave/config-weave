# wscript: containers and strings

_Lists and maps with indexing vs .get; immutable, char-indexed strings._

```rust
let xs = [1, 2, 3]                  // List[int]; xs[0] faults OOB, xs.get(0) is Option
let ages = #{ "alice": 30 }         // Map[string, int]; keys: int|bool|char|string
ages["bob"] = 25                    // insert or overwrite; ages["nope"] faults — use .get
xs.map(|x| x * 2).filter(|x| x > 2).fold(0, |a, x| a + x)
```

Strings are immutable; operations are methods returning new strings, indexed in **characters** (not bytes). The method lists live in [list](../references/fact_wscript_list_methods.md) / [map](../references/fact_wscript_map_methods.md) / [string](../references/fact_wscript_string_methods.md) methods.

## Related

- [wscript: values and types](../references/concept_wscript_values_types.md)

- [wscript list methods](../references/fact_wscript_list_methods.md)

- [wscript map methods](../references/fact_wscript_map_methods.md)

- [wscript string methods](../references/fact_wscript_string_methods.md)

[← Back to SKILL.md](../SKILL.md)
