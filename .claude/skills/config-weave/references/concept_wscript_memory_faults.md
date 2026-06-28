# wscript: memory and faults

_Pure reference counting (cycles leak; break with weak); VM faults are trappable host errors with a stack trace._

Memory is pure reference counting — **cycles leak** (no cycle collector); break cycles with `weak(x)` / `w.upgrade() -> Option[T]`.

VM faults (index OOB, divide by zero, `unwrap()` on `None`) are **trappable errors** delivered to the host with a stack trace — in config-weave they surface as the step's **Error** status. Prefer `xs.get(i)` / `m.get(k)` where failure is expected.

## Related

- [wscript: reference semantics](../references/concept_wscript_reference_semantics.md)

- [wscript: Option, Result and ?](../references/concept_wscript_option_result.md)

- [Step lifecycle](../references/concept_step_lifecycle.md)

[← Back to SKILL.md](../SKILL.md)
