# wscript: Option, Result and ?

_Option\[T\]/Result\[T,E\] are always available; ? early-returns and composes across the host boundary._

`Option[T]` and `Result[T, E]` are always available. `?` early-returns the `None`/`Err` and composes across the host boundary — host errors arrive as `Err`.

The method surfaces are in [Option / Result methods](../references/fact_wscript_option_result_methods.md).

## Related

- [wscript: pattern matching](../references/concept_wscript_pattern_matching.md)

- [Option / Result methods](../references/fact_wscript_option_result_methods.md)

- [Host API](../references/concept_host_api.md)

[← Back to SKILL.md](../SKILL.md)
