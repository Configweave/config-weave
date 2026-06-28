# Gatherer

_A package fact collector exporting gather(params) -> Value, bound into a playbook variable._

A gatherer is a package-declared fact collector, implemented by a
[wscript](../references/entity_wscript_lang.md) script that exports \`gather(params: Value) ->
Value`. A playbook `gather "label" { from = "pkg.gatherer" }\` block invokes one
and binds its result to the variable named by the **label** (e.g. `os.family`).


## Example

```rust
use value
use sys

fn gather(params: Value) -> Value {
    Value::Map(#{
        "family": Value::String(sys::family()),
        "name": Value::String(sys::os_name()),
        "cpus": Value::Int(sys::cpu_count())
    })
}
```

Gatherer invocations all run concurrently and are deduplicated by `(gatherer, canonicalized params)`; any gatherer failure aborts the run before step execution. See [Variables](../references/concept_variables.md) for how results enter scope.

## Related

- [Package](../references/concept_package.md)

- [Variables](../references/concept_variables.md)

- [Host API](../references/concept_host_api.md)

- [Script entry-point signatures](../references/fact_entry_point_signatures.md)

- [Value](../references/entity_value_type.md)

[← Back to SKILL.md](../SKILL.md)
