# Test block reference

```wcl
test "file_present_converges" {
  description = "file_present creates the file and is idempotent"
  backend = "docker"                    // default; or "vmlab" (QEMU/KVM VMs)
  image = "debian:12"                   // required; vmlab: template ref like "x86_64/linux-modern"
  group = "files"                       // optional; share one instance with same-group tests
  setup = "..."                         // optional
  verify = "tests/file_present_verify.wscript"   // optional custom assertions

  step "create" {
    description = "Create a marker file"
    resource = "file_present"           // unqualified = this package
    expect = "converge"                 // default; see the step expectation table
    properties { path = "/var/tmp/weave-sample.txt"  content = "hello" }
  }

  gather "os" {                         // gatherer invocation with assertions
    description = "OS facts inside the container"
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

[← Back to SKILL.md](../SKILL.md)
