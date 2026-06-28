# CheckResult and ApplyResult

The ambient result enums a resource's `check()` and `apply()` return (no `use` needed):

```rust
enum CheckResult { AlreadyConfigured, NotConfigured, RebootRequired }
enum ApplyResult { Success, RebootRequired }
```

How each variant drives the step is in the [step lifecycle](../references/concept_step_lifecycle.md).

## Related

- [Step lifecycle](../references/concept_step_lifecycle.md)

- [Script entry-point signatures](../references/fact_entry_point_signatures.md)

[← Back to SKILL.md](../SKILL.md)
