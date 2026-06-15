# Testing packages (`config-weave test` — the testlab)

Packages declare `test` blocks in `package.wcl`; `config-weave test` runs each in a
disposable backend instance — a docker container (linux) or a vmlab VM (linux or
windows) — and proves convergence with a **three-run protocol**: `check`, `apply`,
`apply` (all `--json --continue-on-error`, `--jobs` forwarded). Run 2's internal
re-check proves convergence within one process; run 3 proves **cross-process
idempotence** — a check that only passes on in-process state re-applies on run 3,
surfaces as `configured`, and fails the test.

## Test block syntax

```wcl
test "file_present_converges" {
  description = "file_present creates the file and is idempotent"
  backend = "docker"                    // default; or "vmlab" (QEMU/KVM VMs)
  image = "debian:12"                   // required; vmlab: template ref like "x86_64/linux-modern"
  group = "files"                       // optional; share one instance with same-group tests
  setup = "..."                         // optional
  verify = "tests/file_present_verify.wisp"   // optional custom assertions

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

Rules:

- **All test values must be static** — tests run against a synthesized variable-free
  playbook; a variable reference in test properties/conditions is a validation error.
- Unqualified `resource` / `from` refs resolve to the declaring package.
- Test steps also take `condition`, `requires` and `properties` like playbook steps.

## Grouping tests into one instance

By default each test provisions its own instance. Give several tests the **same
non-empty `group`** (within a package) to run them sequentially inside **one** shared
instance — this amortizes container start, and especially VM boot, so testing is much
faster.

```wcl
test "creates"  { image = "debian:12"  group = "files"  ... }   // runs first
test "modifies" { image = "debian:12"  group = "files"  ... }   // same container, after
```

- Grouped tests must agree on `backend` and `image` (a group provisions one instance);
  mismatches are a validation error.
- They **share the instance's OS state with no reset between them** (there is no cheap
  VM snapshot). Only group tests that don't collide — the three-run protocol still needs
  each test's own resources to start from a clean state. Use distinct paths/state per
  test, or order them deliberately with `requires`-free, idempotent steps.
- Tests run in declaration order within a group; an empty/absent `group` keeps a test
  fully isolated in its own instance.
- Independent groups run **in parallel**, throttled by separate per-backend caps (see
  `--docker-jobs` / `--vmlab-jobs` below); tests within a group stay sequential.

## Step expectation table

`expect = converge | already_configured | error | skip | reboot_required`
(— = unasserted):

| expect | run 1: check | run 2: apply | run 3: apply again |
|---|---|---|---|
| `converge` (default) | not_configured | configured | already_configured |
| `already_configured` | already_configured | already_configured | already_configured |
| `error` | — | error | — |
| `skip` | skipped | skipped | skipped |
| `reboot_required` | — | reboot_required | — |

## Verify scripts

`fn verify(facts: Value) -> bool` (or `Result[bool, string]`) runs **inside the
instance after the apply runs**; `facts` is a map of the test's gather results (keyed
by gather label). Verify scripts compile during validation but only execute in
instances.

```rust
use value
use fs

fn verify(facts: Value) -> Result[bool, string] {
    Ok(fs::read("/var/tmp/weave-sample.txt")? == "hello")
}
```

## Running tests

```sh
config-weave test <playbook-dir>              # everything
config-weave test <dir> core                  # one package
config-weave test <dir> core:file_present_converges   # one test
```

| Flag | Meaning |
|---|---|
| `--backend NAME` | override every test's backend (`docker` or `vmlab`) |
| `--image IMAGE` | run every test against this image instead of its own |
| `--keep` | leave instances running for post-mortem debugging (handle is reported) |
| `--binary PATH` | static linux config-weave binary to copy into instances |
| `--binary-windows PATH` | windows config-weave binary for windows vmlab guests |
| `--docker-jobs N` | max docker groups (containers) running at once (default `min(cpu, 8)`) |
| `--vmlab-jobs N` | max vmlab groups (VMs) running at once (default `2` — VMs are heavy) |
| `--json` | schema-stable report object with `mode: "test"` |

Exit codes: **0** all passed, **1** any failed/error, **2** validation or environment
problem.

## Execution model

- Each test's `backend` field (or `--backend`) picks its backend; every backend the
  selected tests use is discovered once, up front (broken environment = exit 2 before
  any test runs).
- Binary resolution is **lazy, per guest OS** (instances report `GuestOs`): linux =
  `--binary` → `$CONFIG_WEAVE_TEST_BINARY` → the running exe if it is static (no
  `PT_INTERP` header) → newest static workspace cross-build artifact; windows =
  `--binary-windows` → `$CONFIG_WEAVE_TEST_BINARY_WINDOWS` → newest workspace
  `x86_64-pc-windows-gnu` artifact. A `version` smoke test turns arch mismatches into
  one clear diagnostic.
- In-instance paths follow the guest OS (`/weave/…` vs `C:/weave/…`); `setup` runs via
  `sh -c` on linux and `cmd /C` on windows (cd'd into the weave dir), so windows setup
  must be cmd-compatible.
- **docker**: CLI discovery `$CONFIG_WEAVE_CONTAINER_CMD` → `docker` → `podman`;
  keep-alive via `run -d --entrypoint sleep`. Images must contain `sleep` and `sh`
  (distroless unsupported — use vmlab). Guests are always linux.
- **vmlab**: CLI discovery `$CONFIG_WEAVE_VMLAB_CMD` → `vmlab`. Each group gets a
  throwaway one-VM lab (`cw-test-…`) in a tempdir — VM boot is paid once per group, not
  per test: `vmlab up`, `vmlab osinfo box`
  (decides linux vs windows protocol), `vmlab cp` for file transfer, `vmlab exec
  --timeout 3600 box -- …`, `vmlab destroy` on teardown. Templates must ship the QEMU
  guest agent; with `--keep` the lab stays up and its directory is reported.
- **One instance per group** (an ungrouped test is its own one-test group); the group's
  tests run sequentially inside it, sharing OS state. The binary is copied and
  smoke-tested once per group; each test then gets its own working dir
  (`/weave/t/<idx>-<pkg>__<test>/`) holding a synthesized playbook (one play `test`,
  properties spliced verbatim, referenced packages copied in) plus its facts. Groups run
  in parallel under the per-backend caps above (`runner::run_groups`).
- In-instance protocol (also host-runnable, hidden subcommands):
  `config-weave __gather <dir> <pkg.gatherer> [--params-json …]` prints
  `{"ok":…,"value"|"error":…}`; `config-weave __verify <script> [--facts <json>]`
  exits 0/1/2 = pass/fail/broken.
- The `TestBackend`/`TestInstance` traits in `src/testlab/backend.rs` are the backend
  seam; `src/testlab/docker.rs` and `src/testlab/vmlab.rs` implement it.

## Repo test suites

- `just test` — fast cargo suite (no docker).
- `just test-lab` — cross-builds the static musl binary
  (`CARGO_TARGET_DIR=target-cross cross build --release --target x86_64-unknown-linux-musl`)
  then runs the docker-gated integration suite:
  `CONFIG_WEAVE_TEST_BINARY=… cargo test --test testlab -- --ignored`. Needs docker (or
  podman) and `cross`.
- `just test-lab-vm playbook template` — end-to-end vmlab smoke: runs `config-weave
  test --backend vmlab --image <template>` against a playbook dir. Needs `vmlab`, KVM,
  and a built template.
