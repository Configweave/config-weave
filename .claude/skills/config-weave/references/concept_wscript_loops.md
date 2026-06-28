# wscript: loops

_for over ranges/collections, while, and loop; ranges exist only in for headers._

```rust
for i in 0..10 { }       // exclusive; 0..=10 inclusive (ranges only in for headers)
for x in [1, 2, 3] { }   // list elements; maps iterate keys; strings iterate chars
while cond { }
loop { if done { break } }   // break / continue
```

Ranges are usable **only** in `for` headers, never as standalone values.

## Related

- [wscript: containers and strings](../references/concept_wscript_containers.md)

- [Excluded from wscript v1](../references/fact_wscript_excluded_v1.md)

[← Back to SKILL.md](../SKILL.md)
