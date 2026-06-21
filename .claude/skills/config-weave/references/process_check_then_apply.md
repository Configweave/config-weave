# Check, then apply a play

**Purpose:** Preview what a play would change (report-only), then converge the machine.

_Preconditions:_ A validated playbook., Authorization to mutate the target machine before applying.

### 1. Dry-run with check

```console
$ config-weave check ./my-playbook baseline
make-a   not configured
make-b   not configured
```

> [!NOTE]
> **check never mutates**
> Each step's check() runs report-only: already configured / not configured / skipped (condition false). Nothing is written.

Run `config-weave check ./my-playbook <play>` to see the per-step status without changing anything. Use `--var KEY=VALUE` / `--var-file` to supply variables.

### 2. Apply to converge

```console
$ config-weave apply ./my-playbook baseline
make-a   configured
make-b   configured
```

Run `config-weave apply ./my-playbook <play>`. Each unconfigured step applies, then re-checks; `configured` means apply changed it and the re-check confirmed. A reboot-required step halts the play with exit 3.

### 3. Confirm idempotence

```console
$ config-weave apply ./my-playbook baseline
make-a   already configured
make-b   already configured
```

Run `apply` again: every step should report `already configured`. If a step re-applies, its `check` depends on in-process state and the convergence contract is broken.

> [!TIP]
> **Verification**
> A second `apply` reports every step `already configured` and exits 0.

## Related

- [Playbooks](../references/concept_playbooks.md)

- [Resource & gatherer scripts](../references/concept_scripts.md)

[← All processes](../references/processes_ref.md)
