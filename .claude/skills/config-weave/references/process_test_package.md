# Test a package for idempotence

## Purpose

Prove a package's resources converge and stay converged using the testlab's three-run protocol in a disposable instance.

## Prerequisites

- A package with at least one test block.
- docker (or podman) available for the default backend.

## Flowchart

![diagram](../_wdoc/process_test_package-diagram-1.svg)

## Steps

### Step 1: Declare a test

```wcl
test "file_present_converges" {
  description = "file_present creates the file and is idempotent"
  image = "debian:12"
  step "create" {
    description = "Create a marker file"
    resource = "file_present"
    properties { path = "/var/tmp/weave.txt"  content = "hello" }
  }
}
```

Add a `test` block to `package.wcl` with a required `image` and one or more `step`s. All test values must be **static** — no variable references. Unqualified `resource`/`from` refs resolve to the declaring package.

### Step 2: Run the test

```console
$ config-weave test ./my-playbook core:file_present_converges
core:file_present_converges … passed
```

Run `config-weave test ./my-playbook [pkg[:test]]`. config-weave provisions a disposable instance and runs check → apply → apply. Run 3 catches state that only exists in-process (it would re-apply and surface as `configured`, failing the test).

### Step 3: Debug a failure with --keep

```console
$ config-weave test ./my-playbook core --keep
… instance kept: <handle>
```

On failure, re-run with `--keep` to leave the instance up (its handle is reported) and inspect state. Exit codes: 0 all passed, 1 any failed, 2 validation/environment problem.

> [!TIP]
> **Verification**
> `config-weave test` reports the test(s) passed and exits 0.

## Related

- [Testlab](../references/concept_testlab.md)

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [Test block reference](../references/fact_test_block_fields.md)

- [docker backend](../references/entity_docker_backend.md)

[← Back to SKILL.md](../SKILL.md)
