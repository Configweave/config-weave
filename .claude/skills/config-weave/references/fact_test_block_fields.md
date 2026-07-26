# Test block reference

```wcl
test "file_present_converges" {
  description = "file_present creates the file and is idempotent"
  image = "debian:12"                   // an OCI image → a vmlab container (linux, seconds)
                                        // …or template = "x86_64/ubuntu-24.04" → a full VM.
                                        // Exactly one of the two is required.
  memory = "4GiB"                       // optional; instance RAM (containers default to 256MiB)
  group = "files"                       // optional; share one instance with same-group tests
  setup = "..."                         // optional
  verify = "tests/file_present_verify.ws"    // optional custom assertions

  step "create" {
    description = "Create a marker file"
    resource = "file_present"           // unqualified = this package
    expect = "converge"                 // default; see the step expectation table
    properties { path = "/var/tmp/weave-sample.txt"  content = "hello" }
  }

  gather "os" {                         // gatherer invocation with assertions
    description = "OS facts inside the instance"
    from = "os_info"
    expect {                            // top-level key equality assertions
      family = "linux"
    }
  }
}
```

> [!WARNING]
> **Test values are static**
> **All test values must be static** — tests run against a synthesized variable-free playbook; a variable reference in test properties/conditions is a validation error. Unqualified `resource` / `from` refs resolve to the declaring package.

Verify scripts: `fn verify(facts: Value) -> bool` (or `Result[bool, string]`) runs **inside the instance after the apply runs**; `facts` is a map of the test's gather results (keyed by gather label). Verify scripts compile during validation but only execute in instances.

## Related

- [Testlab](../references/concept_testlab.md)

- [Grouping tests into one instance](../references/concept_test_grouping.md)

- [Three-run protocol](../references/concept_three_run_protocol.md)

- [Step expectation table](../references/fact_step_expectation_table.md)

- [container instance](../references/entity_container_instance.md)

- [VM instance](../references/entity_vm_instance.md)

[← Back to SKILL.md](../SKILL.md)
