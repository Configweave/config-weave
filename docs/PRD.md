# PRD: Config Weave — single-binary configuration management

**Status:** Draft v1 for implementation
**Depends on:** WCL (configuration language), wscript (embedded scripting language — assumed complete per its own PRD before this project begins), wdoc (documentation generation)
**Targets:** `x86_64-unknown-linux-musl`, `x86_64-pc-windows-gnu` — both cross-built from Linux

---

## 1. Summary

Config Weave is a configuration management tool that compiles to a single static binary. The binary is copied onto a target machine alongside a playbook folder and run with no other dependencies — no agent, no runtime, no package installs. A playbook describes the desired state of a system; Config Weave can **check** whether the machine matches that state (a report-only dry run) or **apply** it (converge the machine to the playbook).

Division of labour between the three languages:

- **WCL** is the *encoding format* for playbooks: plays, steps, variables, conditions, gatherer invocations, and resource/gatherer declarations. WCL never executes against the system.
- **wscript** is the *implementation language* for gatherers and resources: the scripts that actually inspect and mutate the machine, via a host API registered by Config Weave.
- **Config Weave (Rust)** is the mediator. WCL and wscript never interact directly. The engine evaluates WCL to plain data, marshals it into wscript's dynamic `Value` type, executes scripts, and routes results back.

### Key properties

1. **Everything validates before anything runs.** WCL parses, all step/gatherer parameters schema-check, and every wscript script in the playbook compiles and type-checks against the host API *before* the first script executes. A typo in step 40's apply script fails the run at second zero.
2. **Check is the dry run.** The check/apply split is the entire safety model. `check` never mutates; `apply` is check → apply → re-check per step.
3. **DAG-parallel execution** with resource-declared concurrency classes (parallel / exclusive / global).
4. **First-class authoring experience.** The binary emits `.wscripti` interface files for its host API, so playbook authors get wscript LSP diagnostics, hover, and completions against the real Config Weave API.
5. **Self-documenting.** `config-weave docs` renders the playbook to a wdoc site, including per-resource parameter tables and the play DAG.

### Non-goals (v1)

- Remote execution of any kind. Purely local. (Run it over SSH/WinRM yourself.)
- Drift-detection infrastructure. "Drift detection" is `check` on a cron job; no backend.
- A templating engine. WCL handles templating natively.
- Package repositories / fetching. Packages are folders. A `config-weave fetch` subcommand is a future consideration.
- Early-bound COM (vtable calls against compiled interfaces). Late binding via IDispatch only.
- A cross-platform service-management abstraction. `service` is Windows-only in v1; Linux uses `shell::run("systemctl …")`.
- Architectures beyond x86_64. aarch64 for both platforms is a future addition; nothing in the design precludes it.

---

## 2. Build & distribution

- **Linux:** `x86_64-unknown-linux-musl`, fully static.
- **Windows:** `x86_64-pc-windows-gnu`, static against the mingw runtime. Chosen specifically so both targets build from a Linux box using `cross` — the MSVC toolchain cannot be driven by `cross`.
- **⚠ Verification task (do this in milestone 1):** confirm the `windows` crate (windows-rs) covers every API surface we need on the *gnu* target — IDispatch/IDispatchEx, VARIANT/SAFEARRAY, the Service Control Manager, and the registry APIs. windows-rs ships import libs for gnu targets and this is expected to work, but it has not been verified. **Fallback if anything is missing:** switch the Windows target to `x86_64-pc-windows-msvc` and accept a native Windows CI build step. This is a build-pipeline change only; nothing else in this PRD changes.
- Release artifacts: two binaries, plus a checksums file. No installers.

---

## 3. Playbook layout

```
my-playbook/
  playbook.wcl              # Plays, variables, gatherer invocations
  lib/                      # Playbook-level shared wscript code (importable)
    util.wscript
  pkgs/
    <package-name>/
      package.wcl           # Gatherer + Resource declarations (schemas, concurrency)
      lib/                  # Package-level shared wscript code
        helpers.wscript
      resources/
        <resource-name>.wscript    # exports check() and apply()
      gatherers/
        <gatherer-name>.wscript    # exports gather()
```

- All paths in a `package.wcl` are relative to the package folder.
- `playbook.wcl` references packages by folder name under `pkgs/` and resources/gatherers as `package.name`.
- One `.wscript` file per resource containing **both** `check` and `apply` (replacing the old two-file convention). One file per gatherer.
- `description` fields are **mandatory** on every meaningful schema element — playbook, play, step, container, resource, gatherer, and every declared parameter. Enforced by `validate` and consumed by `docs`.

---

## 4. `playbook.wcl`

Defines the playbook metadata, variables, gatherer invocations, and plays.

```wcl
playbook "App Server Baseline" {
    version = "1.0.0"
    description = "Baseline configuration for application servers"

    # --- Gatherer invocations -------------------------------------------
    # The block name is the variable the result lands in. The same gatherer
    # may be invoked multiple times with different params into different
    # variables.
    gather "os" {
        from = "core.os_info"
    }
    gather "data_disk" {
        from = "core.disk_info"
        params = { mount = "/var" }
    }
    gather "root_disk" {
        from = "core.disk_info"
        params = { mount = "/" }
    }

    # --- Variables -------------------------------------------------------
    # Ordinary WCL variables; may reference gatherer results.
    var app_root = "/opt/myapp"
    var is_windows = os.family == "windows"

    play "baseline" {
        description = "Core OS configuration"
        parallel = true            # default true; false forces sequential

        step "install-runtime" {
            description = "Install the application runtime"
            resource = "runtime.dotnet"
            condition = !is_windows
            requires = []
            properties = {
                version = "8.0"
                install_dir = app_root
            }
        }

        container "hardening" {
            description = "Security hardening steps"
            # Containers group steps for organisation and docs.
            # They carry no execution semantics beyond grouping; conditions
            # on a container apply to all children.
            step "disable-telnet" { ... }
        }
    }
}
```

### Semantics carried over from the prior spec (unchanged)

- **Step statuses:** Already Configured, Configured, Not Configured, Reboot Required, Skipped, Error.
- **Conditions** are WCL expressions; false → step Skipped.
- **`requires`** names other steps; the engine builds a DAG (petgraph) and rejects cycles at validate time.
- **Variable override precedence** (lowest → highest): WCL declaration → gatherer result → `--var-file` → `--var`.
- **Containers** are organisational only.

---

## 5. `package.wcl`

Declares the package's gatherers and resources, including parameter schemas and concurrency class.

```wcl
package "runtime" {
    description = "Application runtime management"

    gatherer "installed_versions" {
        description = "Enumerate installed runtime versions"
        script = "gatherers/installed_versions.wscript"
        params schema {
            # WCL schema declaring this gatherer's parameters.
            channel: string { description = "Release channel to enumerate" }
        }
    }

    resource "dotnet" {
        description = "Manage a .NET runtime installation"
        script = "resources/dotnet.wscript"
        concurrency = "exclusive"     # parallel | exclusive | global
        params schema {
            version: string {
                description = "Runtime version to install"
                required = true
            }
            install_dir: string {
                description = "Installation root"
                default = "/usr/local/dotnet"
            }
        }
    }
}
```

> **⚠ Binding note — WCL schema syntax.** The blocks above sketch the *intent*: parameter declarations carry name, coarse type (string/int/bool/list/map), required/optional, default, and description, expressed using **WCL's native schema system**. The exact syntax must follow the WCL spec, not this sketch. The implementer should consult the WCL schema documentation and adjust the surface accordingly; the validation behaviour in §8 is the contract, the syntax here is illustrative.

### Concurrency classes (declared on the resource)

| Class | Meaning | Implementation |
|---|---|---|
| `parallel` | Default. No restriction. | — |
| `exclusive` | At most one step using this resource type runs at a time (apt/MSI lock case). | Per-resource mutex |
| `global` | Step runs completely alone. Scheduler drains all in-flight steps, runs this solo, resumes. | Scheduler barrier |

A step may *tighten* its resource's class (e.g. force `global` on one step of a `parallel` resource) but never loosen it.

---

## 6. The WCL ↔ engine ↔ wscript boundary

WCL and wscript never see each other. The engine:

1. Evaluates WCL expressions (conditions, properties, gatherer params) to plain data.
2. Marshals plain data into wscript's dynamic `Value` type — the one dynamically-typed escape hatch wscript provides, designed for exactly this host-data case.
3. Calls the script's exported entry point.
4. Marshals the return back to plain data (gatherers) or maps the typed result enum to a step status (resources).

### Script contracts

```
# resources/<name>.wscript
fn check(params: Value) -> CheckResult     # AlreadyConfigured | NotConfigured | RebootRequired
fn apply(params: Value) -> ApplyResult     # Success | RebootRequired

# gatherers/<name>.wscript
fn gather(params: Value) -> Value
```

- `CheckResult` and `ApplyResult` are **host-registered enums**. The wscript type checker enforces the contract: a resource script that doesn't export a correctly-typed `check` and `apply` fails compilation at validate time.
- **Errors are not enum variants.** Scripts use wscript's normal `Result`/`?` machinery; an error propagated out of (or panicking) a script maps to the step's **Error** status with the message attached. The enums describe *state*, errors describe *failure*.
- A gatherer's returned `Value` is converted to WCL data and bound to the invocation's variable name in the playbook scope.

### Shared code

- `lib/` at the **package** level and at the **playbook** level hold importable wscript files for shared helpers.
- Config Weave configures wscript's module resolution so these folders are importable from resource and gatherer scripts (package `lib/` visible to that package; playbook `lib/` visible to all packages).
- **⚠ Binding note — import semantics.** The path mapping (e.g. `import lib::helpers`) must match whatever import mechanism wscript actually shipped. This PRD specifies the *resolution roots* Config Weave provides; the import syntax binds to the wscript spec.

---

## 7. Host API

All modules registered by Config Weave into the wscript `Context`. Conventions: snake_case `module::function`; fallible operations return `Result` so `?` works everywhere; paths are plain strings; everything is sync (wscript v1 is sync).

**Platform availability rule:** *every* module is registered on *every* platform, so compilation, validation, and `.wscripti` emission are identical everywhere — a Linux box can validate a playbook containing Windows resources. Calling a foreign-platform function at **runtime** returns an error. In practice it never happens, because steps carry WCL platform conditions fed by gatherers.

### Cross-platform modules

| Module | Surface |
|---|---|
| `log` | `debug`, `info`, `warn`, `error`. Routes into the tracing pipeline with step context attached — appears in NDJSON logs and the rich terminal view. Raw `print` is redirected into `log::info` (scripts must not write stdout directly; it would corrupt the progress display and `--json` output). |
| `fs` | read/write/append text and bytes, copy, move, delete, mkdir (recursive), exists, metadata (size, mtime, permissions), glob, temp files/dirs, symlink create/read. |
| `path` | join, parent, filename, extension, normalize, absolutize. Pure string manipulation, platform-aware separators, no IO. |
| `shell` | `run(cmd, opts) -> CmdOutput` (captured stdout/stderr/exit code; opts: cwd, env, timeout, stdin). `run_streaming` variant that pipes output through `log` live for long-running installs. Conveniences: `powershell(script, opts)` (invokes with `-NoProfile -NonInteractive`) and `bash(script, opts)` (`bash -c`, falling back to `sh` on minimal systems). Both return `CmdOutput`. |
| `http` | `get`, `post`, `download(url, dest)`; options for headers, redirects, timeout. |
| `hash` | `sha256`, `sha512`, `md5` over strings and files. `download` + `hash::sha256_file` + compare is the canonical verified-fetch pattern. |
| `archive` | Extract zip and tar.gz to a directory. Bootstrapping must not depend on `tar`/`unzip` existing. |
| `env` | get/set process env vars, PATH-style list helpers, hostname, current user, home dir, `is_elevated()` (root/admin). |
| `sys` | OS name/version/family, architecture, CPU count, total/available memory. Gatherer fodder; gatherers are ordinary wscript scripts with no special powers. |
| `data` | Parse/serialize JSON, TOML, INI ↔ `Value`. **Note:** check overlap with wscript-std before implementing — re-export rather than duplicate where wscript-std already covers it. |

### Windows-only modules

| Module | Surface |
|---|---|
| `registry` | read/write/delete keys and values; typed for REG_SZ, REG_DWORD, REG_QWORD, REG_EXPAND_SZ, REG_MULTI_SZ; hive constants (HKLM, HKCU, …). |
| `service` | query status, start, stop, set startup type — via the Service Control Manager. |
| `com` | Late-bound COM. See below. |

### COM design (late binding only)

The constraint: wscript is statically typed; COM interfaces aren't known at script compile time. Resolution: everything goes through `IDispatch::Invoke` — the same path VBScript/JScript/PowerShell use.

```
com::create("WScript.Shell")                       -> ComObject
com::get_object("winmgmts://./root/cimv2")         -> ComObject   # GetObject / monikers
obj.get("PropertyName")                            -> Value
obj.set("PropertyName", value)
obj.call("MethodName", args...)                    -> Value
com::wmi_query("SELECT * FROM Win32_Service")      -> Value       # sugar over the SWbemLocator dance
```

- `ComObject` is a registered host type whose methods are statically typed *as taking and returning `Value`*. The wscript compiler is satisfied; dispatch stays dynamic. Type errors against a specific COM interface are **runtime** errors — inherent to late binding, accepted.
- Marshalling `Value` ↔ VARIANT: strings, ints, doubles, bools, null, arrays (SAFEARRAY), and nested `ComObject` for VT_DISPATCH returns (WMI queries return collections of objects — this case is mandatory).
- Implementation via the `windows` crate.
- The engine calls `CoInitializeEx` (STA) on **each worker thread** before any script runs on it, and uninitializes at teardown. Scripts never think about apartments.
- `wmi_query` exists because WMI is the dominant COM use in gatherers; it collapses ~8 lines of moniker-and-iterate boilerplate into one call.

---

## 8. Validation pipeline

`config-weave validate <playbook-dir>` — and the implicit validation phase at the start of every `check`/`apply` run — performs, in order, **before any script executes**:

1. Parse `playbook.wcl` and every `package.wcl`. Any WCL parse error → exit 2.
2. Structural checks: referenced packages/resources/gatherers exist; mandatory `description` fields present; gatherer invocation names unique; script files exist.
3. **Schema validation:** every step's `properties` and every gatherer invocation's `params` validate against the declared WCL schema — unknown key → error, missing required → error, coarse type mismatch → error.
4. Build the step DAG per play; reject cycles.
5. **Compile every wscript script** in the playbook (resources, gatherers, lib files) against the full host `Context`. Wscript's type checker enforces entry-point signatures and catches any misuse of the host API. Any compile/type error → exit 2 with wscript's diagnostics.

Validation is platform-independent (per the full-registration rule in §7): a playbook validates identically on Linux and Windows.

---

## 9. Execution model

### Run sequence (`apply`, single play)

1. Validate (§8, all of it).
2. **Gatherer phase.** Collect the play's gatherer invocations. All invocations are independent by definition (no gatherer sees another's output), so run them **concurrently**. Deduplicate by `(gatherer, canonicalised params)` — identical invocations into different variables share one execution; results are cached for the life of the run. Any gatherer failure aborts before step execution (downstream conditions and properties can't be trusted with holes in the scope).
3. Resolve the WCL variable scope with gatherer results bound.
4. Evaluate the DAG and dispatch steps to the worker pool (size `--jobs N`; default `min(cpu_count, 8)`), honouring concurrency classes:
   - Condition false → **Skipped**.
   - Run `check`:
     - AlreadyConfigured → **Already Configured**, continue.
     - RebootRequired → **Reboot Required**, halt (see below).
     - Error → **Error**, halt (or continue with `--continue-on-error`).
     - NotConfigured → run `apply`:
       - RebootRequired → **Reboot Required**, halt.
       - Error → **Error**, halt (or continue).
       - Success → re-run `check`:
         - AlreadyConfigured → **Configured**, continue.
         - Anything else → **Error** ("apply claimed success but check disagrees"), halt (or continue).
5. Print the report; write logs; exit code per table below.

`check` runs the same sequence but stops after each step's first check call and never invokes `apply`.

### Halting under parallelism

On Error (without `--continue-on-error`) or RebootRequired: the scheduler **stops dispatching** new steps, lets in-flight steps run to completion, then halts. No mid-flight cancellation — killing a half-finished installer is worse than waiting.

### Reboot resume (stateless)

A RebootRequired halts the play. On re-run after reboot, already-configured steps return AlreadyConfigured from their check and execution resumes naturally via the DAG. No state file.

### Determinism

The report orders steps in declaration/topological order regardless of actual completion order, so output is diffable across runs.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | All steps configured (apply) or reported (check) without error |
| 1 | One or more steps in Error |
| 2 | Validation failure |
| 3 | Reboot required — play halted |

---

## 10. CLI

```
config-weave [OPTIONS] <COMMAND>

Commands:
  check     <playbook-dir> <play>    Report configuration status of all steps
  apply     <playbook-dir> <play>    Apply all unconfigured steps in a play
  list      <playbook-dir>           List all plays defined in the playbook
  validate  <playbook-dir>           Full validation pipeline (§8), no execution
  docs      <playbook-dir> [outdir]  Generate wdoc documentation (default: <dir>/docs/)
  wscripti     [outdir]                 Emit .wscripti interface files for the host API
                                     plus a starter wscript.toml (default: cwd)
  init      <dir>                    Scaffold a skeleton playbook: playbook.wcl,
                                     pkgs/ with an example package + resource +
                                     gatherer, lib/, .wscripti files, wscript.toml
  version                            Print version information

Options:
  --var <KEY=VALUE>        Override a playbook variable. Repeatable.
  --var-file <path.wcl>    Merge a WCL file's top-level variables into scope.
  --jobs <N>               Worker pool size (default: min(cpu_count, 8))
  --continue-on-error      Continue dispatching steps after an Error
  --json                   JSON output mode (single object on stdout at completion)
  --no-color               Plain ASCII output (also auto-selected when not a TTY)
  --log-file <path>        Enable NDJSON file logging
  --log-level <level>      File log level (independent of terminal verbosity)
  -v, --verbose            Increase terminal verbosity (repeatable)
  -h, --help               Print help
```

Variable precedence (lowest → highest): WCL declaration → gatherer result → `--var-file` → `--var`.

---

## 11. Output & logging (carried over from prior spec, unchanged)

Three mutually exclusive **terminal** modes:

1. **Rich** (default on TTY): ANSI colour, Unicode status icons, live progress with phase detail (gathering / checking / applying / re-checking), per-step timing.
2. **Plain** (`--no-color` or non-TTY auto-detect): ASCII, line-oriented, no cursor movement.
3. **JSON** (`--json`): a single complete JSON object on stdout at run completion — playbook metadata, per-step status/timing/messages, gatherer results summary, exit status. Nothing else on stdout. (Script `print`/`log` output goes to the file log and, in rich/plain modes, the terminal — never stdout in JSON mode.)

**File logging** is independent of terminal mode: `tracing-subscriber` + `tracing-appender` non-blocking writer emitting NDJSON, enabled by `--log-file`/`--log-level`. Script `log::*` calls carry step/play/package context fields. **Implementation note:** the `WorkerGuard` must be held for the full process lifetime or logs are silently lost on exit.

---

## 12. Self-documentation (`config-weave docs`)

Walks the playbook model, emits wdoc source, invokes the wdoc toolchain to render.

- Playbook index page: metadata, play list.
- Per-play page: description, variables, gatherer invocations, step table with conditions and `requires`, and the step DAG rendered as a wdoc `@diagram` block.
- Per-package page: gatherers and resources.
- Per-resource page: description, concurrency class, and a **parameter table** generated from the WCL schema (name, type, required, default, description) — this is the payoff for mandatory descriptions and declared schemas.

`docs` shares the validation pipeline: a playbook that doesn't validate doesn't document.

---

## 13. Authoring experience

- `config-weave wscripti` dumps the complete host API (all modules, both platforms, `CheckResult`/`ApplyResult`, `ComObject`, `CmdOutput`, …) as `.wscripti` interface files via wscript's `Context::write_interface`, plus a starter `wscript.toml` referencing them. The wscript LSP and `wscript check` then give playbook authors diagnostics, hover, and completions against the real API.
- `config-weave init` scaffolds a working skeleton (one example package with a resource and a gatherer, lib folders, `.wscripti` files, `wscript.toml`) so the new-playbook path is `init` → edit → `validate` → `check`.

---

## 14. Architecture sketch

```
config-weave/            # binary crate: CLI, output/reporting, orchestration
  ├─ model/              # playbook/package data model, WCL loading + schema validation
  ├─ engine/             # gatherer phase, DAG scheduler, worker pool, step lifecycle
  ├─ hostapi/            # all wscript host modules; per-platform impls behind cfg,
  │                      #   stub-with-runtime-error for foreign platform
  ├─ comdispatch/        # (windows) IDispatch invoke, VARIANT<->Value marshalling
  ├─ docsgen/            # wdoc emission
  └─ wscripti/              # interface dump + init scaffolding
```

Key crates: `wcl`, `wscript` (+ `wscript-std` where re-exported), `wdoc`, `petgraph` (DAG), `clap` (CLI), `tracing`/`tracing-subscriber`/`tracing-appender` (logging), `windows` (COM/registry/SCM), plus HTTP, hashing, and archive crates chosen for musl-static compatibility (pure-Rust TLS — rustls — to keep the static build clean).

Threading model: worker pool of OS threads; **one wscript VM per worker** (wscript is one-VM-per-thread by design); `CoInitializeEx` per worker on Windows. The scheduler thread owns the DAG and dispatches ready steps to workers subject to concurrency classes.

---

## 15. Milestones

Each lands with tests and an example playbook exercising the new surface.

1. **M1 — Skeleton & validation.** WCL model loading, schema validation, DAG construction, wscript compilation of all scripts against a minimal host context (`log`, `fs`, `path`). `validate` and `list` work end to end. **Includes the windows-rs gnu-target verification spike (§2).** *Gate: a sample playbook validates; an introduced typo in a script or property fails validation with a good diagnostic.*
2. **M2 — Sequential execution.** Gatherer phase (concurrent, deduplicated), variable resolution, sequential step execution with the full check/apply/re-check lifecycle and all six statuses, plain-mode output, exit codes. *Gate: a real playbook converges a Linux test VM.*
3. **M3 — Full host API, Linux.** `shell` (+ `bash`), `http`, `hash`, `archive`, `env`, `sys`, `data`; stdout redirection into `log`. *Gate: a bootstrap playbook downloads, verifies, extracts, and installs something real.*
4. **M4 — Windows.** `registry`, `service`, `com` (+ `wmi_query`), `shell::powershell`, per-worker COM init, gnu-target static build via cross. *Gate: a playbook configures a Windows test VM using registry + WMI + an MSI install.*
5. **M5 — Parallel scheduler.** Worker pool, concurrency classes, `--jobs`, play-level `parallel = false`, drain-on-halt, deterministic reporting. *Gate: a playbook with declared exclusive/global resources runs correctly under `--jobs 8` with stable output.*
6. **M6 — Output & logging.** Rich TTY mode, `--json`, NDJSON file logging with step context. *Gate: JSON output is schema-stable and consumed by a test harness.*
7. **M7 — Authoring & docs.** `wscripti`, `init`, `docs` with DAG diagrams and parameter tables. *Gate: `init` → edit in an LSP-enabled editor with live completions → `validate` → `docs` produces a browsable site.*

---

## 16. Risks & open items

| Item | Risk | Mitigation |
|---|---|---|
| windows-rs coverage on `x86_64-pc-windows-gnu` | COM/SCM/registry APIs unavailable on gnu target | M1 spike; fallback to MSVC target + native Windows CI build (§2) |
| WCL schema syntax in this PRD is illustrative | Spec drift between PRD sketch and real WCL schema feature | Implementer binds to the WCL spec; §8 behaviour is the contract |
| wscript import semantics for `lib/` folders | PRD assumes script-to-script imports exist in shipped wscript | Bind to wscript's actual module mechanism; if absent in wscript v1, lib sharing degrades to a documented limitation tied to wscript's roadmap |
| `data` module overlap with wscript-std | Duplicate JSON/TOML handling | Check wscript-std first; re-export, don't reimplement |
| VARIANT marshalling edge cases (currency, dates, byref) | Obscure COM servers misbehave | v1 supports the common VT set listed in §7; document unsupported VTs as runtime errors |
| Static musl + TLS | OpenSSL linkage pain | rustls everywhere |

---

## 17. Future considerations (explicitly out of v1)

- `config-weave fetch` — pull packages from git repos.
- aarch64 targets (Linux musl, Windows).
- Cross-platform `service` abstraction (systemd backend).
- Parallel-safe cancellation of in-flight applies.
- Mid-play checkpointing/state files (current design is deliberately stateless).
