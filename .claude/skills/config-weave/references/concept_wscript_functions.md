# wscript: functions and closures

_fn definitions, function values as parameters, and closures that capture by reference._

```rust
fn area(w: int, h: int) -> int { w * h }
fn apply(f: fn(int) -> int, x: int) -> int { f(x) }
let double = |x| x * 2          // closures capture by reference
```

Functions are first-class: a `fn(int) -> int` type annotates a function parameter, and closures (`|x| …`) capture their environment by reference.

## Related

- [wscript: overview](../references/concept_wscript_overview.md)

- [wscript: structs, enums, methods](../references/concept_wscript_structs_enums.md)

[← Back to SKILL.md](../SKILL.md)
