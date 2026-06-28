# Step lifecycle

_Per-step check → apply → re-check, with the status transitions and halt rules._

Each step runs a check → apply → re-check lifecycle. The status values come from the [result enums](../references/fact_result_enums.md).

1. **Check** — `AlreadyConfigured` → report \*Already Configured\*, continue.
   `RebootRequired` → in apply mode report \*Reboot Required\* and halt the play
   (exit 3); in check mode it is an ordinary report status (check is report-only).
   `NotConfigured` → proceed to apply (check mode just reports \*Not Configured\*).
   Error → halt unless `--continue-on-error`.
2. **Apply** — `Success` → re-check. `RebootRequired` → report and halt.
3. **Re-check** — must return `AlreadyConfigured`, which reports \*Configured\*;
   anything else reports \*Error\* ("apply claimed success but check disagrees").


An `Err` (or a VM fault) maps to the step's **Error** status. Logging and output during a step go through the [log module](../references/entity_log_module.md).

## Related

- [Convergence contract](../references/concept_convergence_contract.md)

- [Step](../references/concept_step.md)

- [CheckResult and ApplyResult](../references/fact_result_enums.md)

- [Check, then apply a play](../references/process_check_then_apply.md)

[← Back to SKILL.md](../SKILL.md)
