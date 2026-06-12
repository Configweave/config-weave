# Testing packages (`config-weave test` — the testlab)

Packages declare `test` blocks in `package.wcl`; `config-weave test` runs each in a
disposable backend instance (docker container in v1) and proves convergence with a
**three-run protocol**: `check`, `apply`, `apply` (all `--json --continue-on-error`,
`--jobs` forwarded). Run 2's internal re-check proves convergence within one process;
run 3 proves **cross-process idempotence** — a check that only passes on in-process
state re-applies on run 3, surfaces as `configured`, and fails the test.

## Test block syntax

```wcl
test "file_present_converges" {
  description = "file_present creates the file and is idempotent"
  backend = "docker"                    // default; only docker in v1
  image = "debian:12"                   // required
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
| `--backend NAME` | override every test's backend (only `docker` in v1) |
| `--image IMAGE` | run every test against this image instead of its own |
| `--keep` | leave containers running for post-mortem debugging (handle is reported) |
| `--binary PATH` | static linux config-weave binary to copy into instances |
| `--json` | schema-stable report object with `mode: "test"` |

Exit codes: **0** all passed, **1** any failed/error, **2** validation or environment
problem.

## Execution model

- Binary resolution: `--binary` → `$CONFIG_WEAVE_TEST_BINARY` → the running exe if it
  is static (no `PT_INTERP` header) → newest workspace cross-build artifact. A
  `version` smoke test turns arch mismatches into one clear diagnostic. The runner
  always copies a **linux** binary (Windows containers/hosts out of scope in v1).
- Container CLI discovery: `$CONFIG_WEAVE_CONTAINER_CMD` → `docker` → `podman`;
  keep-alive via `run -d --entrypoint sleep`. Images must contain `sleep` and `sh`
  (distroless unsupported until a vmlab backend).
- One container per test, sequential in v1. The host copies in the binary plus a
  synthesized playbook (one play `test`, properties spliced verbatim, referenced
  packages copied in).
- In-container protocol (also host-runnable, hidden subcommands):
  `config-weave __gather <dir> <pkg.gatherer> [--params-json …]` prints
  `{"ok":…,"value"|"error":…}`; `config-weave __verify <script> [--facts <json>]`
  exits 0/1/2 = pass/fail/broken.
- The `TestBackend`/`TestInstance` traits in `src/testlab/backend.rs` are the seam for
  the planned vmlab backend.

## Repo test suites

- `just test` — fast cargo suite (no docker).
- `just test-lab` — cross-builds the static musl binary
  (`CARGO_TARGET_DIR=target-cross cross build --release --target x86_64-unknown-linux-musl`)
  then runs the docker-gated integration suite:
  `CONFIG_WEAVE_TEST_BINARY=… cargo test --test testlab -- --ignored`. Needs docker (or
  podman) and `cross`.
