# Script entry-point signatures

Each entry point accepts two signatures — plain, or fallible when you want `?`:

```rust
// resources/<name>.wscript
fn check(params: Value) -> CheckResult            // or -> Result[CheckResult, string]
fn apply(params: Value) -> ApplyResult            // or -> Result[ApplyResult, string]

// gatherers/<name>.wscript
fn gather(params: Value) -> Value                 // or -> Result[Value, string]

// tests/<name>.wscript (testlab verify)
fn verify(facts: Value) -> bool                   // or -> Result[bool, string]

// scenario driver (host-side, testlab module)
fn run(lab: Lab) -> bool                          // or -> Result[bool, string]
```

An `Err` (or a VM fault) maps to the step's **Error** status. `params` is a `Value::Map` with declared defaults already applied and types validated.

## Related

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

- [CheckResult and ApplyResult](../references/fact_result_enums.md)

- [Value](../references/entity_value_type.md)

[← Back to SKILL.md](../SKILL.md)
