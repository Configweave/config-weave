# Testing packages: `config-weave test`

A `test` block in `package.wcl` declares one isolated convergence test. `config-weave test` provisions a disposable vmlab instance for it, copies in a config-weave binary and a synthesized one-play playbook, runs `check`, `apply`, `apply`, and compares every step's status after each run with the step's expectation. A `scenario` block is the escape hatch for flows the three runs cannot express (reboot-and-continue, several networked machines). Schema: `src/vocab/package.wcl`; runner: `src/testlab/`.

## The `test` block

This is the whole surface.

```wcl
test "file_present_converges" {
  description = "file_present creates the file and is idempotent"
  image = "debian:12"
  memory = "512MiB"
  group = "files"
  setup = "mkdir -p /var/tmp/weave"
  verify = "tests/file_present_verify.ws"

  step "create" {
    description = "Create a marker file"
    resource = "file_present"
    expect = "converge"
    properties {
      path = "/var/tmp/weave/sample.txt"
      content = "hello"
    }
  }

  gather "os" {
    description = "OS facts inside the instance"
    from = "os_info"
    expect {
      family = "linux"
    }
  }
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| label | string | yes | The test name, unique within the package. Selectable on the command line as `package:test`. |
| `description` | string | yes | One-line summary for the docs and the report. |
| `image` | string | one of `image`, `template` | An OCI image reference such as `debian:12`. The test runs in a vmlab container: Linux only, seconds to start. |
| `template` | string | one of `image`, `template` | A vmlab template reference such as `x86_64/ubuntu-24.04`. The test runs in a full VM: Linux or Windows, with a real init system, kernel and reboots. |
| `memory` | string | no | Guest RAM as a WCL byte size, for example `"4GiB"`. Omitted, a container gets vmlab's default of 256MiB and a VM gets its template's sizing. Tests in one group must agree. |
| `group` | string | no | Tests in the same package with the same non-empty group run sequentially inside one shared instance. They must agree on `image` or `template`, and they share OS state with no reset between them. Absent or empty, the test gets its own instance. |
| `setup` | string | no | A shell command run inside the instance before the three runs, through `sh -c` on Linux or `cmd /C` on Windows, with the test's working directory as the current directory. |
| `verify` | string | no | Path of a verify script relative to the package directory. Exports `verify(facts: Value) -> bool` (entry-point signatures: scripts.md). |
| `step` | block, repeatable | no | Resource invocations with an expectation. |
| `gather` | block, repeatable | no | Gatherer invocations with equality assertions. |

Rules:

- Exactly one of `image` and `template` is present; neither or both is a validation error.
- Every value in a test is static. Tests run against a synthesized playbook with no variables, so a variable reference in a property or condition is a validation error.
- An unqualified `resource` or `from` reference resolves to the declaring package (`file_present` in package `core` means `core.file_present`).
- A `secret()` call is a validation error anywhere in a package, so a test cannot carry one.

### `step`

A step of the synthesized playbook: the fields of a playbook step plus `expect`. Steps run under one play in dependency order with the ordinary scheduler, so `requires` orders them exactly as in a playbook.

```wcl
step "remove" {
  description = "Remove the marker file"
  resource = "file_present"
  expect = "converge"
  requires = ["create"]
  properties {
    path = "/var/tmp/weave/sample.txt"
    ensure = :absent
  }
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| label | string | yes | The step name, unique within the test. |
| `description` | string | yes | One-line summary. |
| `resource` | string | yes | The resource or composite to run. Unqualified names resolve to this package. |
| `expect` | string | no, default `"converge"` | The expected status after each run. See Step expectations. |
| `condition` | bool expression | no | A static condition. False makes the step report *skipped* in every run. |
| `requires` | list of strings | no | Step names in this test that must finish first. |
| `properties` | block, at most one | no | Static property values, validated against the resource's `param` declarations. |

Rules:

- A test step has no `concurrency` field; that field belongs to playbook and composite steps, and validation rejects it here.
- Validation rejects a step expecting `converge` or `already_configured` that requires a step expecting `error` or `reboot_required`, because the dependent could never run.

### `gather`

Runs a gatherer inside the instance and asserts on its result.

```wcl
gather "os" {
  description = "OS facts inside the instance"
  from = "os_info"
  params {
    detail = :full
  }
  expect {
    family = "linux"
    init = :systemd
  }
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| label | string | yes | The gather name. The key of this result in the verify script's `facts` map. |
| `description` | string | yes | One-line summary. |
| `from` | string | yes | The gatherer to run. Unqualified names resolve to this package. |
| `params` | block, at most one | no | Static parameters, validated against the gatherer's `param` declarations. |
| `expect` | block, at most one | no | Equality assertions over top-level keys of the gathered value. |

Rules:

- Each field of `expect` names a top-level key of the returned map and the exact value it must hold; a key that is missing or holds a different value fails the test.
- A key the gatherer declares as `type = "symbol"` is asserted with the symbol spelling (`init = :systemd`), and validation checks the value against the declared set.
- A gatherer that returns `Err` fails the test.
- Every gather's result is also passed to the verify script as `facts`, keyed by the gather label.

## Step expectations

This is the whole set of legal `expect` values.

```wcl
expect = "converge" | "already_configured" | "error" | "skip" | "reboot_required"
```

A dash means the run's status is not asserted.

| `expect` | Run 1: check | Run 2: apply | Run 3: apply again |
|---|---|---|---|
| `converge` (default) | `not_configured` | `configured` | `already_configured` |
| `already_configured` | `already_configured` | `already_configured` | `already_configured` |
| `error` | — | `error` | — |
| `skip` | `skipped` | `skipped` | `skipped` |
| `reboot_required` | — | `reboot_required` | — |

## Minimal test

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

## The three-run protocol

Inside the instance the runner executes the synthesized playbook three times as three separate processes, each with `--json` and `--continue-on-error`, and `--jobs` forwarded.

1. **Run 1, check.** Reports the initial status and mutates nothing. A step expected to converge reports `not_configured` here.
2. **Run 2, apply.** Converges. The engine's own re-check after each apply proves convergence within one process.
3. **Run 3, apply again.** Proves cross-process idempotence. Every converging step reports `already_configured` in a fresh process.

A failure on run 3 means the resource's `check` only passes on state the applying process remembered: the fresh process re-applies and the step reports `configured` instead of `already_configured`. Fix the resource's `check` so it reads the real host state.

Order of work per test: `setup` command, gathers, the three runs, verify script. A failure lists each failing step under its test line in the report.

## Verify scripts

- Exports `verify(facts: Value) -> bool` or `verify(facts: Value) -> Result[bool, string]`.
- `facts` is a map keyed by gather label holding each gather's returned value.
- Compiles during validation on the host, executes only inside the instance, against the real host API of the guest.
- The exit status of the in-guest run is the protocol: 0 passed, 1 the assertions did not hold (test fails), any other code means the script broke (test errors).

## Container and VM instances

Every instance is a vmlab machine. The `image` or `template` field is the whole selection.

| | Container (`image`) | VM (`template`) |
|---|---|---|
| Guest | The OCI image booted in a micro-VM, as root, with its own kernel and the full capability set | A full QEMU/KVM VM cloned from the vmlab template |
| OS | Linux only | Linux or Windows, detected from the vmlab guest agent |
| Start time | Seconds once the image is cached | Boot time; a fresh Windows clone takes several minutes on first boot |
| Init system, reboot | No live init, no reboot | Real init, reboots |
| Default memory | 256MiB, allocated on demand | The template's own sizing |
| `setup` shell | `sh -c` | `sh -c` on Linux, `cmd /C` on Windows |
| Requirement | An OCI image with a shell; vmlab pulls it | The template ships the vmlab guest agent, which the runner polls for readiness (up to 300s) |
| Windows extra | n/a | The agent reports `windows`; a Windows build of config-weave is required (see Binary resolution) |
| Payload | Mounted at `/weave` | Copied to `/weave` (`C:/weave` on Windows) |

Choose `image` for anything that is really just a userland: file, package and config resources. Network resources that need `NET_ADMIN` or a real kernel (nftables, ufw, firewalld) also work in a container. Choose `template` when the test needs a live init system, a reboot, or a Windows guest.

Each test gets its own working directory under `/weave/t/` in the guest (`C:/weave/t/` on Windows), holding its synthesized playbook, the referenced packages and the playbook's `lib/`. `setup` runs with that directory as its current directory.

## Grouping

- Tests in the same package with the same non-empty `group` run sequentially, in declaration order, inside one shared instance; the container start or VM boot is paid once per group.
- Grouped tests agree on their target (same kind and same reference) and on `memory`, because a group provisions exactly one machine. Validation rejects a mismatch.
- Grouped tests share OS state with no reset between them, so group only tests that target distinct paths and distinct state. Each test's own resources still start clean for its three runs.
- Ungrouped tests each get their own instance.
- Independent groups run in parallel, throttled by `--container-jobs` and `--vm-jobs`. Within a group tests stay sequential.
- A provisioning or binary smoke-test failure errors every test in the group. One test's transport trouble errors only that test, and the rest of the group proceeds.
- The final report lists results in selection order regardless of completion order.

## Binary resolution

The runner copies a config-weave binary matched to the guest OS into each instance and smoke-tests it with `version` once per group, so an architecture mismatch is one clear diagnostic. First match wins:

| Guest | Order |
|---|---|
| Linux | `--binary PATH`, then `$CONFIG_WEAVE_TEST_BINARY`, then the running executable if it is a static ELF, then the newest static cross-build in the workspace (`target-cross/x86_64-unknown-linux-musl/release/config-weave` or `dist/config-weave-linux-x86_64`) |
| Windows | `--binary-windows PATH`, then `$CONFIG_WEAVE_TEST_BINARY_WINDOWS`, then the newest Windows cross-build in the workspace (`target-cross/x86_64-pc-windows-gnu/release/config-weave.exe` or `dist/config-weave-windows-x86_64.exe`) |

A dynamically linked development build (`cargo build`) cannot run inside an arbitrary container, so a dev loop usually needs `--binary` pointing at the output of `just release`. Windows guests always need an explicit Windows binary through the flag or the variable. A path given by flag or variable that does not exist is an error.

## vmlab discovery and requirements

- The command is found at `$CONFIG_WEAVE_VMLAB_CMD` if set, else as `vmlab` on the path, and probed once with `--version` before any test runs. A missing vmlab is exit 2 before a single instance is provisioned.
- vmlab and its virtualisation support are the only host requirements; there is no container runtime to install. VM tests also need KVM.
- Each instance is a one-machine lab in a temporary directory named `cw-test-…`; teardown runs `vmlab destroy` and removes the directory.

```console
vmlab --version
CONFIG_WEAVE_VMLAB_CMD=/opt/vmlab/bin/vmlab config-weave test ./my-playbook
```

## The `scenario` block

This is the whole surface.

```wcl
scenario "ad_matrix" {
  description = "Forest, additional DC and a member join over real reboots"
  lab    = "tests/ad-lab"          // a directory holding a vmlab.wcl
  script = "tests/ad_matrix.ws"
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| label | string | yes | The scenario name. Selectable as `package:scenario`. |
| `description` | string | yes | One-line summary. |
| `lab` | string | yes | Directory, relative to the package, holding a `vmlab.wcl` with every VM the scenario will use (segments, static addresses, DNS, dependencies between machines). |
| `script` | string | yes | Path, relative to the package, of the driver script exporting `run(lab: Lab) -> bool` or `run(lab: Lab) -> Result[bool, string]`. |

Rules:

- Every VM is declared in `vmlab.wcl` up front. The vmlab daemon loads its lab configuration once and does not see machines added later.
- Scenario machines are always VMs.
- The driver runs host-side against the live lab, not inside an instance, and compiles during validation against the host API plus the `testlab` module, so a broken driver fails `config-weave validate`.
- Scenarios run one at a time after every test group has finished, each owning its own lab. The runner tears the lab down after `run` returns.
- A `run` returning `true` or `Ok(true)` passes; `false`, `Ok(false)` or `Err(msg)` fails the scenario with that message; a driver that cannot be read, compiled or crashes at run time errors it.

### Driver API: `use testlab`

The module exposes no free functions. It registers the handle types `Lab`, `Machine` and `RunReport` and the plain structs `StepResult` and `ExecOut`. Every method that touches a machine returns `Result[…, string]`. The rest of the host modules a driver may also `use` are listed in host-api.md.

`Lab` is the handle `run` receives. This is the whole surface.

| Method | Signature | Summary |
|---|---|---|
| `machine` | `(name: string) -> Result[Machine, string]` | bring the declared VM up on first reference and return its handle |
| `log` | `(message: string)` | print a scenario progress line to the terminal |

`Machine` is a provisioned VM. The config-weave binary is copied in and smoke-tested the first time a method needs it, so a machine only used through `exec` never pays for the copy. Resource keys are `package.resource`; `props` and `params` are a `Value::Map`. Playbook directories are resolved relative to the scenario's package. This is the whole surface.

| Method | Signature | Summary |
|---|---|---|
| `name` | `() -> string` | the machine's declared name |
| `exec` | `(cmd: string, args: List[string]) -> Result[ExecOut, string]` | run a program in the guest |
| `powershell` | `(script: string) -> Result[ExecOut, string]` | run a script with `powershell -NoProfile -NonInteractive -Command` |
| `copy_in` | `(host_path: string, dest: string) -> Result[unit, string]` | copy a host file or directory into the guest |
| `reboot` | `() -> Result[unit, string]` | reboot the machine and wait for the guest agent again |
| `wait_ready` | `(secs: int) -> Result[unit, string]` | wait up to `secs` seconds for the guest to answer |
| `apply_resource` | `(key: string, props: Value) -> Result[StepResult, string]` | apply one resource in the guest through a synthesized one-step playbook |
| `check_resource` | `(key: string, props: Value) -> Result[StepResult, string]` | check one resource in the guest |
| `gather` | `(key: string, params: Value) -> Result[Value, string]` | run one gatherer in the guest and return its value; a refusal is an `Err` |
| `apply` | `(dir: string) -> Result[RunReport, string]` | apply a whole playbook directory in the guest |
| `check` | `(dir: string) -> Result[RunReport, string]` | check a whole playbook directory in the guest |

`RunReport` is the report of a whole-playbook `apply` or `check`. This is the whole surface.

| Method | Signature | Summary |
|---|---|---|
| `ok` | `() -> bool` | the run exited zero |
| `step` | `(name: string) -> Result[StepResult, string]` | the result of one step; an unknown name is an `Err` |

```rust
struct StepResult {
    status: string,   // not_configured | configured | already_configured | reboot_required | error | skipped | not_run
    message: string,
    ok: bool,         // false only when the step errored or was missing from the report
}

struct ExecOut {
    exit_code: int,
    stdout: string,
    stderr: string,
}
```

### Minimal scenario

```rust
use testlab
use value

fn run(lab: Lab) -> Result[bool, string] {
    let dc = lab.machine("dc")?
    let first = dc.apply_resource("windows_domain.forest", Value::Map(#{
        "name": Value::String("corp.example")
    }))?
    if first.status != "reboot_required" { return Ok(false) }
    dc.reboot()?
    dc.wait_ready(600)?
    let second = dc.apply_resource("windows_domain.forest", Value::Map(#{
        "name": Value::String("corp.example")
    }))?
    lab.log("forest converged")
    Ok(second.status == "already_configured")
}
```

## Running: `config-weave test`

```console
config-weave test [OPTIONS] <PLAYBOOK_DIR> [FILTER]
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory. The playbook is validated first. |
| `FILTER` | Optional. `pkg` selects every test and scenario in one package. `pkg:name` selects the test or scenario with that name in that package. No filter selects everything. |

A filter that matches nothing is an error, and the message lists every available `pkg:name`. A playbook in which no package declares a test or scenario is also an error.

Global options apply: `--json` selects the JSON report, `--no-color` the plain one, and `--jobs` is forwarded to the config-weave runs inside each instance. This is the whole set of `test`-specific options.

| Option | Value | Meaning |
|---|---|---|
| `--image` | `IMAGE` | Run every container test against this OCI image instead of the one it declares. Tests that declare a template are unaffected. |
| `--template` | `REF` | Run every VM test against this vmlab template instead of the one it declares. Tests that declare an image are unaffected. |
| `--keep` | | Leave the instances running after the run for post-mortem debugging. You are responsible for removing them. |
| `--binary` | `PATH` | Static Linux config-weave binary to copy into Linux instances. Alternative to `$CONFIG_WEAVE_TEST_BINARY`. |
| `--binary-windows` | `PATH` | Windows config-weave binary for Windows guests. Alternative to `$CONFIG_WEAVE_TEST_BINARY_WINDOWS`. |
| `--container-jobs` | `N` | Maximum container test groups running at once. Default is the smaller of the CPU count and 8. |
| `--vm-jobs` | `N` | Maximum VM test groups running at once. Default 2, because VMs are heavy. |
| `--events-ndjson` | | Stream one JSON event per line to stderr: lifecycle, per-phase progress and raw instance attach information. Stdout still carries the final report. |

Neither `--image` nor `--template` converts a test between kinds; each replaces the reference only for tests that already declare that field.

With `--events-ndjson`, the first event is `run_started` with the full plan, each instance announces itself with `instance_ready` carrying its id, and the last event is `run_finished` with the exit code and the pass, fail and error counts, written before the stdout report. A supervisor that kills the process removes those instances itself, because the runner's cleanup does not get a chance to run.

```console
config-weave test ./my-playbook
config-weave test ./my-playbook core
config-weave test ./my-playbook core:file_present_converges
config-weave test ./my-playbook --image docker.io/library/debian:12 \
  --binary dist/config-weave-linux-x86_64 --keep
config-weave test ./my-playbook --vm-jobs 3 --json --events-ndjson 2> events.ndjson > report.json
```

Environment variables read by `test`. This is the whole set.

| Variable | Meaning |
|---|---|
| `CONFIG_WEAVE_VMLAB_CMD` | Path of the vmlab CLI, ahead of `vmlab` on the path. |
| `CONFIG_WEAVE_TEST_BINARY` | Static Linux binary, used when `--binary` is absent. |
| `CONFIG_WEAVE_TEST_BINARY_WINDOWS` | Windows binary, used when `--binary-windows` is absent. |

### Exit status

| Code | Meaning |
|---|---|
| 0 | Every selected test and scenario passed. |
| 1 | At least one test failed a step expectation or the verify script, or errored while running. A binary that cannot be located or does not run inside the instance errors every test in that group. |
| 2 | A problem before any test ran: the playbook did not validate, the filter matched nothing, no package declares tests, or the vmlab CLI was not found. |

### `--keep` and post-mortem

Run the failing test again with `--keep`. The instance stays up after the run and its handle (the lab directory) is reported:

```console
$ config-weave test ./my-playbook core --keep
⟳ [core:file_present_converges] kept <handle> — remove it manually when done
```

From that directory `vmlab exec`, `vmlab container exec` and `vmlab console` work against the guest; the test's playbook and facts are under `/weave/t/` in the guest. A kept instance is never torn down for you; remove it with `vmlab destroy` when done. A kept scenario lab is reported the same way.

## Gotchas

- **In-memory state fails run 3.** A resource whose `check` passes only on state the applying process remembered reports `configured` on run 3, and the test fails. `check` reads the real host.
- **`HOME=/` in instances.** The guest agent runs commands with `$HOME` set to `/`, not the target user's home. Pass `home`, or the equivalent parameter, explicitly in the test instead of relying on a resource's home-directory default.
- **First Windows boot is slow.** A fresh Windows clone takes several minutes on its first boot, so group Windows tests together and expect the readiness poll to run long.
- **Dynamically linked dev builds.** The running executable is used only if it is a static ELF; otherwise build with `just release` and pass `--binary`.
- **Windows guests need an explicit binary.** There is no fallback to the running executable for Windows.
- **`dnf5` stalls in a Fedora container.** Repositories load but the transaction wedges for many minutes; use a VM for real dnf installs.
- **`x86_64/debian-13` template has no working vmlab agent** in the local template store; use `x86_64/ubuntu-24.04` for apt-family VM tests.
- **`memory` is a WCL byte size string** such as `"512MiB"` or `"4GiB"`, and every test in a group declares the same value.
- **Grouped tests share state.** No reset happens between tests in a group; a second test that touches the first test's path sees its leftovers and fails run 1.
- **No variables, no `secret()`, no `concurrency` in a test.** Each is a validation error.
- **Wrong-kind override does nothing.** `--image` leaves template tests untouched, and `--template` leaves image tests untouched.
- **Scenario labs are fixed at declaration.** A VM missing from `vmlab.wcl` cannot be added by the driver; `lab.machine(name)` fails with an unknown name.
- **Killed runs leak instances.** Under `--events-ndjson`, a supervisor that kills `config-weave test` removes the instances named by `instance_ready` events itself.
