# The config-weave command line

`config-weave` is a single binary. Every command takes a playbook directory (the directory holding `playbook.wcl`) plus the global options. Global options are accepted before or after the subcommand. The clap definitions live in `src/main.rs`.

## Synopsis

```console
config-weave [OPTIONS] <COMMAND>

Commands:
  check     Report configuration status of all steps (never mutates)
  apply     Apply all unconfigured steps in a play
  list      List all plays defined in the playbook
  validate  Full validation pipeline, no execution
  test      Run package convergence tests in disposable instances
  docs      Generate wdoc documentation (default outdir: <dir>/docs/)
  wscripti  Emit .wscripti interface files for the host API plus a starter wscript.toml
  init      Scaffold a skeleton playbook
  pkg       Manage packages installed from git package repositories (recorded in pkgs/repo.wcl)
  secrets   Encrypt, decrypt or re-key the `secret("…")` values in playbook.wcl
  version   Print version information
```

Hidden subcommands `__gather`, `__verify`, `__wcl-inspect`, `__wcl-render` and `__templates` exist for the testlab's in-instance protocol and for external tooling (JSON on stdin/stdout); they are not for direct use and their interfaces can change without notice.

## Global options

This is the whole surface. Every option is accepted by every subcommand; only the commands that run scripts (`check`, `apply`, `test`) read the execution and password options.

| Option | Value | Meaning |
|---|---|---|
| `--var` | `KEY=VALUE` | Override a playbook variable. Repeatable. `KEY` must be an identifier. `VALUE` is parsed as a WCL expression when it is one, otherwise taken as a plain string. |
| `--var-file` | `PATH` | Merge a WCL file's top-level `name = value` fields into scope. Each field is evaluated on its own and cannot reference other variables. `--var` wins over `--var-file`. |
| `--password-stdin` | | Read the secrets password from stdin (one line). Trailing newline stripped. |
| `--password-file` | `PATH` | Read the secrets password from a file. Trailing newline stripped. |
| `--jobs` | `N` | Worker pool size for step execution. Default `min(cpu_count, 8)`. `test` forwards it into the instances. |
| `--continue-on-error` | | Keep dispatching steps after a step reports Error. Without it the scheduler stops dispatching, lets in-flight steps finish and halts; steps that required the failed step report `not run`. |
| `--json` | | JSON output mode: one object on stdout at completion and nothing else on stdout. |
| `--no-color` | | Plain ASCII output. Also selected automatically when stdout is not a terminal. |
| `--log-file` | `PATH` | Write an NDJSON log file. The parent directory is used as given (must exist). Independent of the terminal mode. |
| `--log-level` | `LEVEL` | Level for the log file: `trace`, `debug`, `info`, `warn` or `error`. Default `info`. An unknown level exits 2 before the command runs. |
| `-v`, `--verbose` | | Increase terminal verbosity. Repeatable. At `-v` and above, script `log::debug` lines reach the terminal. |
| `-h`, `--help` | | Print help. |
| `-V`, `--version` | | Print the version and exit (root command only). |

Password rule: `--password-stdin`, `--password-file` and `$CONFIG_WEAVE_PASSWORD` are three sources for one password. Give exactly one. Two is an error; none while the playbook holds encrypted values is an error; an empty password is rejected. There is no prompt, so an unattended run fails with exit 2. A playbook with no `secret()` calls never asks for a password. Every value is decrypted before any step runs, so a wrong password fails the run at once even when no step reads the secret.

## check

Reports the status of every step in a play and never changes the machine. Runs validation, the gatherers and the DAG walk, but stops after each step's first `check` call and never invokes `apply`.

```console
config-weave check [OPTIONS] <PLAYBOOK_DIR> <PLAY>
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory, containing `playbook.wcl`. |
| `PLAY` | The name of the play to check (`config-weave list` shows the names). |

| Option | Value | Meaning |
|---|---|---|
| `--events-ndjson` | | Stream one JSON event per line to stderr (run/gather lifecycle, per-step phase progress). Replaces the live progress line; stdout still carries the final report. |
| `--continue-on-error` | | Keep checking remaining steps after a step's check returns Error. |
| `--jobs` | `N` | Worker pool size. |

```console
config-weave check ./my-playbook baseline --json \
  | jq '[.steps[] | select(.status == "not_configured")] | length'
```

Exit: 0 when every step reported without error, whether or not any is `not_configured` (drift is a finding, not a failure). 1 when any step's check returned Error. 2 when validation failed or the play does not exist. Never 3: a RebootRequired result is only a status in check mode.

## apply

Converges one play. For each step: `check`; if NotConfigured, `apply`, then `check` again. A step whose re-check does not report AlreadyConfigured is an Error with the message `apply claimed success but the re-check disagrees`.

```console
config-weave apply [OPTIONS] <PLAYBOOK_DIR> <PLAY>
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory. |
| `PLAY` | The play to apply. |

| Option | Value | Meaning |
|---|---|---|
| `--events-ndjson` | | As for `check`. Combine with `--json` for a fully machine-readable run. |
| `--continue-on-error` | | Keep dispatching after an Error instead of halting once in-flight steps finish. |
| `--jobs` | `N` | Worker pool size. |

```console
config-weave apply ./my-playbook baseline --var hostname=web01 --json > report.json
```

Exit: 0 when every step is `configured`, `already_configured` or `skipped`. 1 when any step ended in Error (including a re-check disagreement). 2 when validation failed, a variable or password could not be resolved, or the play does not exist. 3 when a step returned RebootRequired and no step errored: the play halted, later steps report `not run`; reboot and run `apply` again to resume through the DAG. Error takes priority over reboot.

A second `apply` with nothing changed must report every step `already_configured`. A step reporting `configured` again has a `check` that depends on state from the previous process, or an `apply` that does not produce what `check` looks for.

## list

Prints the plays with step counts and descriptions, then any playbook-local composites. Loads the playbook and reports load errors, but does not run the full validation pipeline and never executes a script.

```console
config-weave list [OPTIONS] <PLAYBOOK_DIR>
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory. |

Only `--json` has an effect: it prints the complete inventory instead. Top-level keys: `playbook`, `version`, `description`, `plays` (`name`, `description`, `steps` count), `composites`, `packages`. Each package entry: `name`, `description`, `builtin`, `resources` (`name`, `description`, `concurrency`, `params`), `composites`, `gatherers` (`name`, `description`, `params`), `tests` (`name`, `description`, `machine_kind`, `source`, `group`), `scenarios` (`name`, `description`). A composite entry: `name`, `description`, `args`, `steps`. A param: `name`, `description`, `type`, `required`, `default`.

```console
config-weave list ./my-playbook
my-playbook v0.1.0 — A starter playbook
  baseline  (3 steps) — Base configuration for every host
composites:
  user_with_home  (2 steps) — A user and its home directory
```

Exit: 0 when the playbook loaded. 2 when it could not be parsed or failed a load-time check.

## validate

Runs the whole validation pipeline and executes nothing. The same pipeline runs implicitly at the start of `check`, `apply`, `test` and `docs`. Validation is platform independent.

```console
config-weave validate [OPTIONS] <PLAYBOOK_DIR>
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory. |

`--json` switches the summary to `{ok, playbook, version, packages, plays, steps, diags}`; on failure `{ok: false, diags: [{message, rendered}]}`. Variable and password options are accepted but not consulted.

```console
config-weave validate ./my-playbook
ok: playbook 'my-playbook' v0.1.0 — 2 package(s), 2 play(s), 8 step(s)
```

A failure lists every diagnostic on stderr and ends with `validation failed with N error(s)`.

Exit: 0 when clean. 2 when any stage reported an error.

### Validation pipeline stages

The stages run in order and all findings are reported together.

1. Parse `playbook.wcl` and every `pkgs/*/package.wcl`. A WCL parse error stops here.
2. Structural checks: referenced packages, resources and gatherers exist; every mandatory `description` is present; gatherer invocation names are unique; every script file exists; every `secret()` call is encrypted.
3. Schema validation: each step's `properties` and each gatherer invocation's `params` are checked against the declared `param` schema. Unknown key, missing required key, or coarse type mismatch is an error.
4. Build the step DAG for every play, expanding composites; reject cycles and unknown `requires` targets.
5. Compile every wscript script (resources, gatherers, verify scripts, scenarios and `lib/` files) against the full host API. The type checker enforces entry-point signatures and catches misuse of a host module.

## test

Runs the `test` and `scenario` blocks declared in the playbook's packages inside disposable vmlab instances. `image` tests run as containers from an OCI image; `template` tests run as full VMs from a vmlab template. Each test gets a copy of the playbook and a config-weave binary inside the instance, and the runner drives the three-run protocol: check, apply, apply again. vmlab is the only backend and must be installed.

```console
config-weave test [OPTIONS] <PLAYBOOK_DIR> [FILTER]
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory. Validated first. |
| `FILTER` | Optional. `pkg` selects every test and scenario in one package. `pkg:name` selects one test or scenario. No filter selects everything. A filter that matches nothing is an error listing every available `pkg:name`. A playbook with no tests or scenarios is an error. |

This is the whole surface of command-specific options. `--json` selects the JSON report, `--no-color` the plain one, and `--jobs` is forwarded into the config-weave runs inside each instance.

| Option | Value | Meaning |
|---|---|---|
| `--image` | `IMAGE` | Run every container test against this OCI image instead of the one it declares. Template tests are unaffected. |
| `--template` | `REF` | Run every VM test against this vmlab template instead of the one it declares. Image tests are unaffected. |
| `--keep` | | Leave instances running after the run for post-mortem debugging. You remove them yourself. |
| `--binary` | `PATH` | Static Linux config-weave binary to copy into Linux instances. Alternative to `$CONFIG_WEAVE_TEST_BINARY`. |
| `--binary-windows` | `PATH` | Windows config-weave binary for Windows guests. Alternative to `$CONFIG_WEAVE_TEST_BINARY_WINDOWS`. |
| `--container-jobs` | `N` | Maximum container test groups running at once. Default `min(cpu_count, 8)`. Minimum 1. |
| `--vm-jobs` | `N` | Maximum VM test groups running at once. Default 2. Minimum 1. |
| `--events-ndjson` | | Stream one JSON event per line to stderr: lifecycle, per-phase progress and raw instance attach info. Stdout still carries the final report. |

Rules:

- `--image` and `--template` never convert a test between kinds; each replaces the reference only for tests that already declare that field.
- Binary resolution for a Linux guest, in order: `--binary`, `$CONFIG_WEAVE_TEST_BINARY`, the running executable if it is a static ELF, the newest static cross-build in the workspace. A dynamically linked dev build cannot run inside an arbitrary container, so a development build usually needs `--binary` pointing at the output of `just release`. Windows guests always need `--binary-windows` or the environment variable.
- Tests with the same non-empty `group` in one package share one instance and run in declaration order inside it with no reset between them. Ungrouped tests each get their own instance. Groups run in parallel up to the container and VM limits. Group members must agree on `image`/`template` and `memory`; a container member and a VM member cannot share a group.
- Scenarios run after every test group has finished, one at a time.
- The final report lists results in selection order regardless of completion order.
- The vmlab CLI is probed once up front; a broken environment fails with exit 2 before any test runs.

```console
config-weave test ./my-playbook core:file_present_converges \
  --image docker.io/library/debian:12 --binary dist/config-weave-linux-x86_64 --keep
```

Exit: 0 when every selected test and scenario passed. 1 when at least one test failed an expectation or the verify script, or errored while running (a binary that cannot be located or does not run inside the instance errors every test in that group). 2 before any test ran: validation failed, the filter matched nothing, no package declares tests, the vmlab CLI was not found, or a password was missing.

## docs

Renders a static wdoc site from the playbook's metadata (descriptions on plays, steps, variables, packages, resources, composites, gatherers). Writes `_weave_docs.wcl` into the output directory and shells out to `wcl wdoc build`. `wcl` must be installed. A playbook that does not validate does not document.

```console
config-weave docs [OPTIONS] <PLAYBOOK_DIR> [OUTDIR]
```

| Argument | Meaning |
|---|---|
| `PLAYBOOK_DIR` | The playbook directory. |
| `OUTDIR` | Optional. Defaults to `docs/` inside the playbook directory. Created when missing. |

| Option | Value | Meaning |
|---|---|---|
| `--serve` | | After rendering, hand the output directory to `wcl wdoc serve` (dev server, rebuilds on demand). Does not return until the server stops. |
| `--addr` | `ADDR` | Listen address for `--serve`. Default `127.0.0.1:8080`. Requires `--serve`. |
| `--pkg-only` | | Document only the packages; skip the playbook's plays, variables and gathered facts. For a package repository whose playbook is only a validation harness. |

Global options are accepted but none change the output (the command does not read `--var`, `--var-file` or the password options).

```console
config-weave docs ./config-weave-pkgs ./site --pkg-only --serve --addr 0.0.0.0:9000
```

Exit: 0 when the site rendered (and, with `--serve`, once the server stopped). 2 when the playbook did not validate, the output directory could not be created, or `wcl` could not be run.

## wscripti

Writes `weave.wscripti`, the wscript interface file describing the whole host API compiled into this binary, plus a starter `wscript.toml` pointing at it. With both next to your scripts, `wscript check` and the wscript LSP type-check against the exact surface config-weave provides.

```console
config-weave wscripti [OPTIONS] [OUTDIR]
```

| Argument | Meaning |
|---|---|
| `OUTDIR` | Optional. Defaults to the current directory. Created when missing. |

No option affects the output. `weave.wscripti` is always overwritten; an existing `wscript.toml` is left untouched. Regenerate after upgrading config-weave.

```toml
# wscript.toml (as written)
interfaces = ["weave.wscripti"]
```

```console
config-weave wscripti ./my-playbook/pkgs/core/resources
wrote ./my-playbook/pkgs/core/resources/weave.wscripti and wscript.toml
```

Exit: 0 when both files were written. 2 when the directory or a file could not be written.

## init

Scaffolds a skeleton playbook that validates as written, with one package named `example` holding a resource, a gatherer, a verify script and a test.

```console
config-weave init [OPTIONS] <DIR>
```

| Argument | Meaning |
|---|---|
| `DIR` | Destination directory. Created when missing. Refused when `DIR/playbook.wcl` already exists. |

No option changes what is written.

| File | Purpose |
|---|---|
| `playbook.wcl` | One play that uses the example package; the `greeting` step plus a `greeting_pair` composite. |
| `pkgs/example/package.wcl` | Declares the `file_present` resource, the `os_info` gatherer and a test. |
| `pkgs/example/resources/file_present.ws` | Resource script with `check` and `apply`. |
| `pkgs/example/gatherers/os_info.ws` | Gatherer script with `gather`. |
| `pkgs/example/tests/greeting_verify.ws` | Verify script for the package's test. |
| `lib/README.md`, `pkgs/example/lib/README.md` | Placeholders for the playbook-level and package-level `lib/` import roots. |
| `.gitignore` | Ignores `.repo-cache/`. |

```console
config-weave init ./my-playbook
config-weave validate ./my-playbook
ok: playbook 'My Playbook' v0.1.0 — 2 package(s), 1 play(s), 3 step(s)
```

The second package is the built-in `weave` package. Exit: 0 when every file was written. 2 when `playbook.wcl` already exists or a file could not be written.

## pkg

Installs packages into `pkgs/` from git repositories. `pkgs/repo.wcl` records registered repositories and every installed package with its source repository and exact commit. Every subcommand shells out to the `git` binary (ambient credentials work for private repos). Clones are shallow and cached under `.repo-cache/<repo>` in the playbook directory. The playbook loader never reads `repo.wcl`, so a broken one breaks only `pkg`, never `check` or `apply`. The command prints a reminder to gitignore `.repo-cache/` but never edits `.gitignore`.

```console
config-weave pkg [--dir <PLAYBOOK>] <COMMAND>
```

| Option | Value | Meaning |
|---|---|---|
| `--dir` | `PLAYBOOK` | The playbook directory. Default `.`. Accepted before or after the subcommand. |

This is the whole subcommand set.

| Subcommand | Synopsis | Meaning |
|---|---|---|
| `add` | `pkg add <PACKAGE>` | Sync every registered repo, find the first (in registration order) whose packages root holds `<PACKAGE>/package.wcl`, copy it to `pkgs/<PACKAGE>`, record the repo's head commit. Other repos holding it are reported as shadowed. Copy skips `.git`, `node_modules`, `target`, `.vmlab` and every dot-directory. Fails when already recorded, when `pkgs/<PACKAGE>` exists but was not installed by `pkg add`, on an invalid name, or when no repo holds it. |
| `remove` | `pkg remove <PACKAGE>` | Delete `pkgs/<PACKAGE>` and its `repo.wcl` entry (entry removed even if the directory is gone). Refuses a package not installed by `pkg add`. Repositories untouched. |
| `update` | `pkg update [PACKAGE]` | Re-sync source repos, re-copy installed packages, record the new commit. No argument updates all. `PACKAGE` must already be recorded. `pkgs/<name>` is managed: local edits are lost when the source has moved. |
| `search` | `pkg search <TERM>` | Sync every repo and print packages whose name or description contains `TERM` (case-insensitive): repo, name, description, marked `[installed]` or `[installed from <repo>]`. |
| `repo add` | `pkg repo add [--branch <BRANCH>] [--subdir <SUBDIR>] <NAME> <URL>` | Register a repo in `repo.wcl` and sync it into `.repo-cache/<NAME>` now. `NAME` must be a valid name. `URL` is anything `git clone` accepts. `--branch`: branch to track (remote default when unset). `--subdir`: subdirectory holding the package dirs (checkout root when unset). |
| `repo remove` | `pkg repo remove <NAME>` | Unregister a repo. Packages installed from it stay. |
| `repo list` | `pkg repo list` | Table of registered repos: URL, branch, subdir, cache state (`not synced`, `dirty`, or the short head commit). Local only, no network. |

Default repository: when `pkg add` or `pkg search` runs with no repository registered (including when `repo.wcl` does not exist), the command seeds `stdlib` at `https://github.com/Configweave/config-weave-pkgs.git` with packages under the `pkgs` subdirectory, and saves it. A `repo.wcl` listing at least one repo is always respected as written.

```console
config-weave pkg --dir ./my-playbook repo add corp git@github.com:example/weave-pkgs.git --subdir packages
config-weave pkg --dir ./my-playbook add nginx
installed 'nginx' from 'corp' @ 3f9c2a1
```

Exit: 0 when the operation completed, including a `search` with no matches. 2 on any error: unknown package or repo, invalid name, `repo.wcl` failing its schema, a directory that could not be copied or removed, or a `git` command the operation needed. A repo that fails to sync during `add`, `update` or `search` is a warning when the operation can still complete.

## secrets

Encrypts, decrypts and re-keys the `secret("…")` values in `playbook.wcl` in place, rewriting only each call's byte span. Works on the raw source, so it is never blocked by the validation error an unencrypted `secret()` raises. The argument to `secret()` must be a plain string literal.

```console
config-weave secrets <COMMAND>
```

This is the whole subcommand set. `PLAYBOOK_DIR` defaults to `.` in all three.

| Subcommand | Synopsis | Meaning |
|---|---|---|
| `encrypt` | `secrets encrypt [PLAYBOOK_DIR]` | Encrypt every plaintext `secret()` in place. When encrypted values already exist, the password is first proven against them and new blobs reuse the existing salt; otherwise a fresh salt is generated. No `secret()` calls: reports that, needs no password. All already encrypted: verifies the password, changes nothing. |
| `decrypt` | `secrets decrypt [PLAYBOOK_DIR]` | Rewrite every encrypted value back to `secret("plaintext")`. Plaintext calls untouched. The file then fails validation until encrypted again. |
| `rekey` | `secrets rekey [--new-password-file <PATH>] [PLAYBOOK_DIR]` | Decrypt every value with the current password, re-encrypt under the new one with a fresh salt. Plaintext calls are encrypted in the same pass. New password from `--new-password-file` or `$CONFIG_WEAVE_NEW_PASSWORD`, exactly one; stdin is not offered for it. The old password is only required when at least one encrypted value exists. |

The current password comes from the global password options. Encrypted form: `CWENC1.<salt>.<nonce>.<ciphertext>` (URL-safe base64; 16-byte salt, 24-byte nonce, ciphertext with tag). Key derived with Argon2id, sealed with XChaCha20-Poly1305. One salt per file. A wrong password and a tampered blob are both reported as a wrong password.

```console
echo "$OLD" | config-weave secrets rekey ./my-playbook --password-stdin --new-password-file ./new-pw
./my-playbook/playbook.wcl: re-encrypted 1 secret(s) under the new password
```

Exit: 0 when the file was rewritten or there was nothing to do. 2 when the password was missing, given twice or wrong, the new password for `rekey` was missing, a blob is malformed, `playbook.wcl` could not be parsed or written, or the directory holds no `playbook.wcl`.

## version

```console
config-weave version
config-weave 0.1.0
```

Prints the binary's name and version. The root `--version` flag prints the same line. Always exits 0.

## Output modes

The terminal mode is chosen once per run from `--json`, `--no-color` and whether stdout is a terminal. This is the whole set.

| Mode | Selected by | Behaviour |
|---|---|---|
| Rich | Default when stdout is a terminal | Colour, Unicode icons, a live progress line on stderr with phase detail, per-step timing. The final report repeats only the summary. |
| Plain | `--no-color`, or stdout not a terminal | ASCII, one line per step in declaration order, then a summary line. No cursor movement. |
| JSON | `--json` | One complete JSON object on stdout at completion. Nothing else on stdout. Script log output goes to stderr or the log file. |

Plain-mode step lines look like `[     not configured] make-a (core.file_present)` followed by `summary: N already configured, N configured, N not configured, N reboot required, N skipped, N error, N not run (T.Ts)`.

### Step status values

Human label in Plain and Rich output, `status` id in JSON and events. This is the whole set.

| Label | JSON id | Meaning |
|---|---|---|
| `already configured` | `already_configured` | `check` found the state present. |
| `configured` | `configured` | `apply` ran and the re-check confirmed it (apply mode only). |
| `not configured` | `not_configured` | `check` found the state absent (check mode; a finding, not a failure). |
| `reboot required` | `reboot_required` | The script returned RebootRequired. |
| `skipped` | `skipped` | The step's `condition` was false. |
| `error` | `error` | The script returned Err, faulted, or the re-check disagreed. |
| `not run` | `not_run` | Not dispatched because the play halted or a required step failed. |

### `--json` report shape for check and apply

Schema-stable. Steps appear in declaration order whatever order they finished in.

```json
{
  "playbook": "my-playbook", "version": "0.1.0", "play": "baseline",
  "mode": "check|apply", "exit_code": 0, "duration_secs": 0.42,
  "gathered": [ { "name": "os", "gatherer": "core.os_info" } ],
  "steps": [ { "name": "make-a", "container_path": [], "resource": "core.file_present",
               "status": "not_configured", "message": null, "duration_secs": 0.01 } ]
}
```

`container_path` lists enclosing container names outermost first; composite expansions appear as `container/…/invocation/inner`.

### `--json` report shape for test

```json
{
  "playbook": "my-playbook", "mode": "test", "exit_code": 0, "duration_secs": 12.3,
  "tests": [ { "package": "core", "name": "file_present_converges",
               "machine_kind": "container|vm", "source": "docker.io/library/debian:12",
               "outcome": "passed|failed|error", "duration_secs": 9.8,
               "steps": [ { "name": "…", "expect": "…", "check": "…", "apply": "…",
                            "second_apply": "…", "failures": [] } ],
               "gathers": [ { "name": "…", "failures": [] } ],
               "verify": { "passed": true, "message": null },
               "error": null, "kept": null } ]
}
```

`outcome` is `failed` for a step expectation, gather assertion or verify failure, and `error` for environmental trouble (provisioning, setup, protocol, parse). `kept` carries the instance handle when `--keep` was given.

### `--events-ndjson`

Accepted by `check`, `apply` and `test`. One JSON object per line on stderr while the run is in progress, each with an `event` field. It replaces the live progress line and leaves stdout to the final report. This is the whole event set.

check/apply: `run_started` (`play`, `mode`, `steps` with `name`, `resource`, `container_path`), `gather_started` (`unique` count), `gather_finished`, `step_started` (`idx`, `name`), `step_phase` (`idx`, `name`, `phase` of `checking`, `applying`, `re-checking`), `step_finished` (`idx`, `name`, `container_path`, `resource`, `status`, `message`, `duration_secs`), `step_resolved` (`idx`, `name`, `status`; a step resolved without running, such as `skipped` or `not_run`).

test (each stamped with `ts`, epoch millis): `run_started` (`playbook`, `groups`, `tests` list with `package`, `test`, `group` index or null for scenarios, `machine_kind`, `source`), `group_provisioning`, `instance_ready` (`group`, `label`, `machine_kind`, `source`, `attach` with the raw vmlab id), `test_started`, `phase` (`phase` of `setup`, `gather` with `name`, `check`, `first_apply`, `second_apply`, `verify`), `log` (`context`, `stream`, `chunk` tail-truncated at 8 KiB, `truncated`), `gather_result`, `step_result`, `verify_result`, `test_finished` (`outcome`, `duration_secs`, `error`), `group_teardown` (`kept`, `handle`, `warning`), `run_finished` (`exit_code`, `passed`, `failed`, `errors`, `duration_secs`). `run_finished` is written before the stdout report. A supervisor that kills the process must remove the instances named in `instance_ready` itself.

### `--log-file` and `--log-level`

File logging is independent of the terminal mode. `--log-file PATH` installs an NDJSON subscriber at `--log-level` (`trace`, `debug`, `info`, `warn`, `error`; default `info`). Every script `log::*` call becomes one line with `step` and `resource` (or `gatherer`) fields attached, alongside the engine's own events. The file is buffered and flushed at process exit. `-v` controls the terminal only: script `log::info` and above always reach stderr as `[step] level: message`; `log::debug` needs `-v`.

## Exit codes

All commands share one table. This is the whole set.

| Code | Meaning |
|---|---|
| 0 | Success. `apply`: every step converged. `check`: every step reported without error. `test`: every test and scenario passed. `validate`: clean. |
| 1 | One or more steps ended in Error (`check`, `apply`), or one or more tests failed or errored (`test`). |
| 2 | Validation failure, a bad option, an invalid `--log-level`, a missing or duplicated or wrong password, a play or filter that matched nothing, a missing external tool (`vmlab`, `wcl`, `git`), or any other problem found before execution started. |
| 3 | Reboot required. Only `apply` produces it: a step returned RebootRequired and no step errored. Reboot and run `apply` again. |

Error beats reboot: when both occur in one run the code is 1. `version` always exits 0. The hidden `__verify` subcommand exits 1 when the verify script returns false.

## Environment variables

This is the whole set the source reads (`CONFIG_WEAVE_STATE_DIR` is read by the built-in script `src/builtin/execute_once.ws`).

| Variable | Read by | Meaning |
|---|---|---|
| `CONFIG_WEAVE_PASSWORD` | `check`, `apply`, `test`, `secrets` | The secrets password. Alternative to `--password-stdin` / `--password-file`. Empty counts as unset. |
| `CONFIG_WEAVE_NEW_PASSWORD` | `secrets rekey` | The new password. Alternative to `--new-password-file`. |
| `CONFIG_WEAVE_STATE_DIR` | `weave.execute_once` (inside scripts) | Root directory for once records, replacing `/var/lib/config-weave/once` on Linux and `HKLM\Software\config-weave\Once` on Windows. On Windows it selects the file form as well. |
| `CONFIG_WEAVE_TEST_BINARY` | `test` | Static Linux binary to copy into instances. Alternative to `--binary`. |
| `CONFIG_WEAVE_TEST_BINARY_WINDOWS` | `test` | Windows binary for Windows guests. Alternative to `--binary-windows`. |
| `CONFIG_WEAVE_VMLAB_CMD` | `test` | Path to the vmlab CLI. When unset, `vmlab` is looked up on `PATH`. |
| `CONFIG_WEAVE_WCL` | `docs` | Path to the `wcl` CLI used for `wdoc build` and `wdoc serve`. When unset, `wcl` is looked up on `PATH`. |

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `validate`/`check`/`apply`/`test` stops with a diagnostic inside a `.ws` file: unknown module, unknown function, type mismatch, unresolvable import | Stage 5 compiles every script against the closed host API before anything runs. Imports resolve as a host module first, then a `.ws` file in the importing file's directory, the package's `lib/`, then the playbook's `lib/`. | Correct the module or function name. Run `config-weave wscripti` and point the editor's wscript checker at it. For an import, confirm the `.ws` extension and that the file sits in a searched directory. |
| `script does not satisfy the 'check' contract` (or `apply`, `gather`, `verify`), or `scenario script does not satisfy the 'run(lab: Lab) -> bool' contract` | The script compiled but does not export the named function with one of the two accepted signatures. Only the entry file's functions count; a helper's do not. | Match the entry-point signature exactly, including the `Value` parameter type and the return type. Use the `Result[…, string]` form only when the body uses `?`. |
| A step or gather diagnostic: property unknown, required one missing, wrong type, symbol must use a colon, duration must be a unit literal | `properties` and `params` are validated against the `param` declarations. A `symbol` is only accepted as `:name` (and must be one of the enumerated symbols when listed). A `duration` is only accepted as a bare unit literal such as `30min`. | Look up the parameter table in the package's `package.wcl` and correct the spelling or form. |
| `url = url` (or a composite body's `properties { path = path }`) fails with a self-reference or cycle | A property or parameter field shadows an outer variable of the same name inside its block, so the field refers to itself. | Rename the variable (`tool_url`). Inside a composite body read arguments as `args.path`, which never collides. |
| `steps may only tighten` | A step declared a looser concurrency class than its resource (for example `parallel` on an `exclusive` resource). | Remove the step's `concurrency` or choose a class at least as strict as the resource's. |
| `apply claimed success but the re-check disagrees` (or `re-check failed:` plus an error); play halts | `apply` returned Success but the immediate re-check did not return AlreadyConfigured, or the second `check` itself errored. | Make `check` and `apply` agree on the same definition of converged (compare content, not existence; wait for service state to settle). Run `config-weave test` to catch it in a clean instance. |
| A test passes run 2 but fails run 3: expected `already_configured`, got `configured` | The resource converges within one process but not across processes. Run 3 is a fresh `config-weave apply`, so in-memory state is gone. | Make `check` read state from the host every time. Anything `apply` records must live on disk, in the registry, or in the configured system. |
| A step reports `error` with a wscript stack trace (index out of bounds, division by zero, `unwrap()` on `None`, invalid regex) | A VM fault. Faults in `check` or `apply` map to `error`. An invalid regex pattern is a fault, not an `Err`, so `?` cannot catch it. | Use `xs.get(i)`, `m.get(k)` and `match` instead of the faulting forms. Build regexes from tested literals. |
| A script grows without bound over a long apply | wscript memory is pure reference counting with no cycle collector; mutually referencing values are never freed. | Break the cycle with `weak(x)` on one side and `w.upgrade()` (returns an `Option`) where it is read. |
| A `registry`, `service` or `com` call validates, then errors at run time on Linux | Every host module is registered on every platform so playbooks validate identically everywhere. Calling a foreign-platform function at run time returns an error. | Guard the step with a gatherer-fed `condition = os.family == "windows"`, or branch in the script on `sys::family()`. |
| `this secret has not been encrypted yet`, hint `run config-weave secrets encrypt to encrypt it in place` | A `secret("…")` in `playbook.wcl` still holds plaintext. Validation scans for calls without a `CWENC1` blob and needs no password. | Run `config-weave secrets encrypt` with the password from `$CONFIG_WEAVE_PASSWORD`, `--password-stdin` or `--password-file`. |
| `check`/`apply`/`test` on a playbook with secrets exits 2 with no prompt: missing password, or a value could not be decrypted | config-weave never prompts. Exactly one password source is required, and every value is decrypted before any step runs. | Supply the password through one of the three routes. If it is right and decryption still fails, the file was encrypted under a different password; `secrets rekey` needs the old one. |
| `secret("…")` in a `package.wcl` (including inside a `test` or `scenario` block) is rejected | Packages are shared and cannot hold a value encrypted under one playbook's password; test instances have no password at all. | Keep the secret in the playbook's `vars` and pass it to the resource through a property. |
| `config-weave test` exits 2 with a message about `vmlab`, or a group errors on provisioning or the smoke test | The testlab probes `vmlab` once up front (on `PATH` or via `$CONFIG_WEAVE_VMLAB_CMD`). A container test needs a pullable OCI image. A VM test needs a template that ships the vmlab guest agent (readiness polled for 300 seconds). After provisioning, a binary matched to the guest OS is copied in and `version` is run inside; an architecture or OS mismatch fails there. | Confirm `vmlab --version` works. For a VM use a template with a working agent (`x86_64/ubuntu-24.04` is known to work for apt-family). Provide a static Linux binary via `--binary`/`$CONFIG_WEAVE_TEST_BINARY`, or a Windows one via `--binary-windows`/`$CONFIG_WEAVE_TEST_BINARY_WINDOWS`. Pass `--keep` and inspect with `vmlab exec` or `vmlab console`. |
| A resource defaulting a path to `$HOME` writes under `/` instead of `/root` in a container test | The vmlab guest agent runs commands with `HOME=/`. | Pass the home directory explicitly as a test property. |
| A test passes alone but fails after joining a `group`, or validation rejects the group | Grouped tests run sequentially in one instance and share OS state with no reset. Members must agree on `image`/`template` and `memory`; container and VM members cannot mix. | Only group tests that target distinct state. Split tests touching the same files or services into separate groups. |
| `apply` exits 3, one step `reboot required`, later steps `not run` | A resource returned RebootRequired from `check` or `apply`. In apply mode that halts the play. In check mode it is an ordinary status and the run exits 0. | Reboot the host and run `apply` again; converged steps report `already configured` and the run resumes from the step that asked for the reboot. |
| `config-weave docs` exits 2 with `cannot run wcl wdoc build` | The `wcl` CLI is not on `PATH`. | Install `wcl` or set `CONFIG_WEAVE_WCL` to its path. |
| `invalid --log-level '…'` before the command runs | `--log-level` accepts only `trace`, `debug`, `info`, `warn`, `error`. | Use one of those. |
| `error: nothing matches '…' (available: pkg:name, …)` | The `test` filter named a package or test that does not exist. | Use one of the listed `pkg:name` values, or just `pkg`. |
