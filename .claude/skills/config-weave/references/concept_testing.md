# Testing & the testlab

_config-weave test — test blocks, the three-run protocol, docker/vmlab backends, grouping, verify scripts, and scenarios._

Packages declare `test` blocks in `package.wcl`; `config-weave test` runs each in a
disposable backend instance — a docker container (linux) or a vmlab VM (linux or
windows) — and proves convergence with a **three-run protocol**: `check`, `apply`,
`apply` (all `--json --continue-on-error`). Run 2's internal re-check proves
convergence within one process; run 3 proves **cross-process idempotence** — a check
that only passes on in-process state re-applies on run 3, surfaces as `configured`,
and fails the test.


## Test block syntax

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
    expect = "converge"                 // default; see table below
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

## Step expectation table

`expect = converge | already_configured | error | skip | reboot_required` (— = unasserted):

| expect | run 1: check | run 2: apply | run 3: apply again |
| --- | --- | --- | --- |
| `converge` (default) | not_configured | configured | already_configured |
| `already_configured` | already_configured | already_configured | already_configured |
| `error` | — | error | — |
| `skip` | skipped | skipped | skipped |
| `reboot_required` | — | reboot_required | — |

## Grouping tests into one instance

By default each test provisions its own instance. Give several tests the **same non-empty `group`** (within a package) to run them sequentially inside **one** shared instance — amortizing container start, and especially VM boot. Grouped tests must agree on `backend` and `image`; they **share OS state with no reset between them**, so use distinct paths/state per test. Independent groups run **in parallel**, throttled by `--docker-jobs` / `--vmlab-jobs`.

## Verify scripts

`fn verify(facts: Value) -> bool` (or `Result[bool, string]`) runs **inside the instance after the apply runs**; `facts` is a map of the test's gather results (keyed by gather label). Verify scripts compile during validation but only execute in instances.

```rust
use value
use fs

fn verify(facts: Value) -> Result[bool, string] {
    Ok(fs::read("/var/tmp/weave-sample.txt")? == "hello")
}
```

## Running tests

```console
config-weave test <playbook-dir>              # everything
config-weave test <dir> core                  # one package
config-weave test <dir> core:file_present_converges   # one test
```

| Flag | Meaning |
| --- | --- |
| `--backend NAME` | override every test's backend (`docker` or `vmlab`) |
| `--image IMAGE` | run every test against this image instead of its own |
| `--keep` | leave instances running for post-mortem debugging |
| `--binary PATH` | static linux config-weave binary to copy into instances |
| `--binary-windows PATH` | windows config-weave binary for windows vmlab guests |
| `--docker-jobs N` | max docker groups running at once (default `min(cpu, 8)`) |
| `--vmlab-jobs N` | max vmlab groups running at once (default `2` — VMs are heavy) |

Exit codes: **0** all passed, **1** any failed/error, **2** validation or environment problem. **docker** images must contain `sleep` and `sh` (distroless unsupported — use vmlab). **vmlab** templates must ship the QEMU guest agent; each group gets a throwaway one-VM lab.

## Scenarios (scripted, multi-stage, over a declared vmlab lab)

Some convergence can't be expressed by the three-run protocol: a Windows DC promotion
needs **apply → reboot → apply again**, and a member server needs a **reachable DC** (a
second networked VM). For these, a package declares a `scenario` — a \*\*declared vmlab
lab** plus a wscript **driver script\*\* that brings the lab's VMs up by name, applies
config-weave, reboots, and asserts.


```wcl
scenario "ad_matrix" {
  description = "Forest, additional DC and a member join over real reboots"
  lab    = "tests/ad-lab"          // dir holding a vmlab.wcl (vmlab only)
  script = "tests/ad_matrix.wscript"
}
```

The script exports `fn run(lab: Lab) -> bool` (or `Result[bool, string]`) and runs **host-side** against the live lab. It compiles in stage-5 validation against the `testlab` host module (types `Lab` / `Machine`: `lab.machine(name)`, `m.exec`, `m.powershell`, `m.copy_in`, `m.reboot`, `m.apply_resource`, `m.check_resource`, `m.apply(dir)`, `m.check(dir)`). Scenarios run **sequentially** after the parallel test groups. **Windows guests must be Server 2019 / Windows 10 or newer.**

## Repo test suites

`just test` (fast cargo suite, no docker) · `just test-lab` (cross-builds the static musl binary, runs the docker-gated suite) · `just test-lab-vm playbook template` (end-to-end vmlab smoke) · `just test-ad` (the full Windows DC lifecycle scenario over real reboots — heavy).

## Related

- [Packages](../references/concept_packages.md)

- [Resource & gatherer scripts](../references/concept_scripts.md)

[← All concepts](../references/concepts_ref.md)
