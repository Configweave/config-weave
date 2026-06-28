# wscript: structs, enums, methods

_struct/enum declarations, impl blocks, and the implicit by-reference self._

```rust
struct Player { name: string, hp: int }
enum Event { Quit, Key(char), Click { x: int, y: int } }

impl Player {
    fn heal(self, amount: int) { self.hp = self.hp + amount }
}
```

Enums carry data (unit, tuple, or struct variants). Methods live in `impl` blocks; `self` is implicit and always by reference (there is no `&`). Match on enums with [pattern matching](../references/concept_wscript_pattern_matching.md).

## Related

- [wscript: pattern matching](../references/concept_wscript_pattern_matching.md)

- [wscript: traits and operators](../references/concept_wscript_traits_operators.md)

- [wscript: reference semantics](../references/concept_wscript_reference_semantics.md)

[← Back to SKILL.md](../SKILL.md)
