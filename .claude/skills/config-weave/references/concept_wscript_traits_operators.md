# wscript: traits and operators

_Go-flavored interfaces with Rust syntax; operator overloading via builtin traits; Eq required for == on structs/enums._

Go-flavored interfaces, Rust syntax; static dispatch when concrete, `dyn Trait` for dynamic. No default bodies or trait inheritance in v1.

Operator overloading via builtin traits `Add Sub Mul Div Rem Neg Eq Ord Display Index` (`Index` is read-only; the custom `Ord` form is `fn cmp(self, other: Self) -> int` returning -1/0/1). `==` on structs/enums **requires** an `Eq` impl. Derives: `#[derive(Eq, Ord, Display, Clone)]`.

## Related

- [wscript: structs, enums, methods](../references/concept_wscript_structs_enums.md)

- [Excluded from wscript v1](../references/fact_wscript_excluded_v1.md)

[← Back to SKILL.md](../SKILL.md)
