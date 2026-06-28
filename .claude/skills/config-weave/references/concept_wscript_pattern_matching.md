# wscript: pattern matching

_match is an expression, exhaustiveness-checked at compile time; guards, or-patterns, if-let and let-else._

`match` is an expression, **exhaustiveness-checked at compile time**:

```rust
match e {
    Event::Quit => false,
    Event::Key(c) if c == 'q' => false,          // guards (don't count for exhaustiveness)
    Event::Key('h') | Event::Key('?') => help(), // or-patterns (no bindings inside, v1)
    Event::Key(_) => true,
    Event::Click { x, y } => x >= 0 && y >= 0,
}
```

`if let Some(x) = opt { … }` and `let Some(n) = expr else { return Err("…") }` work as in Rust (the let-else block must diverge).

## Related

- [wscript: structs, enums, methods](../references/concept_wscript_structs_enums.md)

- [wscript: Option, Result and ?](../references/concept_wscript_option_result.md)

[← Back to SKILL.md](../SKILL.md)
