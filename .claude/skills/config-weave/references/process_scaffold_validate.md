# Scaffold and validate a playbook

**Purpose:** Start a new playbook from the built-in skeleton and confirm it validates before running anything.

_Preconditions:_ The config-weave binary is on PATH.

### 1. Scaffold a skeleton

```console
$ config-weave init ./my-playbook
created ./my-playbook (playbook.wcl, pkgs/core/…)
```

Run `config-weave init ./my-playbook`. It writes a skeleton playbook with an example package, resource and gatherer — a working starting point.

### 2. Validate the whole pipeline

```console
$ config-weave validate ./my-playbook
ok
```

> [!NOTE]
> **What validate covers**
> WCL syntax, schema, ref resolution, the step DAG, and **wscript compilation of every script** — host-API misuse is caught here, before any execution.

Run `config-weave validate ./my-playbook`. Nothing executes; fix any reported syntax, schema, ref, DAG or script-compilation error before moving on.

### 3. List the plays

```console
$ config-weave list ./my-playbook
baseline
```

Run `config-weave list ./my-playbook` to see the plays you can `check` or `apply`.

> [!TIP]
> **Verification**
> `config-weave validate` exits 0 and `list` prints the expected play names.

## Related

- [Playbooks](../references/concept_playbooks.md)

- [Packages](../references/concept_packages.md)

[← All processes](../references/processes_ref.md)
