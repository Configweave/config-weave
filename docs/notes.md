# Implementation notes

Decisions made while binding the PRD to the real WCL and wisp APIs. The
PRD (docs/PRD.md) marks several syntax sketches as illustrative; this file
records the actual bindings.

## windows-rs gnu-target verification (PRD §2, M1 spike) — PASSED

A probe crate using `windows` 0.62 with features `Win32_System_Com`,
`Win32_System_Ole`, `Win32_System_Variant`, `Win32_System_Services`,
`Win32_System_Registry` — covering IDispatch + DISPATCH_* flags, VARIANT
VTs, SAFEARRAY functions, the Service Control Manager and the registry
APIs — **compiles and links to a .exe for `x86_64-pc-windows-gnu` via
`cross`** (Docker image provides the mingw toolchain; the local box lacks
`x86_64-w64-mingw32-dlltool`, so plain `cargo build` cannot link — use
`cross` as the PRD intends). The MSVC fallback is not needed.

One API drift note: `DISPATCH_METHOD` / `DISPATCH_PROPERTYGET` /
`DISPATCH_PROPERTYPUT` live in `Win32::System::Com` (not `::Ole`) in
windows 0.6x.

## WCL binding (PRD §4/§5 sketches → real WCL)

- The vocabulary ships as WCL **system imports** embedded in the binary
  (`import <weave/playbook.wcl>` / `<weave/package.wcl>`), exactly how
  wdoc ships its stdlib. The engine appends the import line at the *end*
  of user sources, so user spans are untouched and authors never write
  import lines.
- `var x = expr` (PRD sketch) became a `vars { x = expr }` block.
- `params schema { version: string { … } }` (PRD sketch) became
  `param "version" { type = "string" … }` blocks; coarse types are
  `string|int|float|bool|list|map`. §8 validation behaviour is engine-side
  and unchanged from the PRD contract.
- Step `properties = { … }` became a `properties { … }` child block;
  gather `params = { … }` likewise a `params { … }` block.
- Variables (gatherer results, declared vars, `--var`/`--var-file`
  overrides) bind by generating an in-memory system import
  `<weave/vars.wcl>` containing `let` declarations. Gatherer results are
  injected through an `Environment` builtin (`__weave_var`), so any value
  shape round-trips without literal serialization. Conditions and
  properties evaluate lazily against that scope at run time.
- WCL's block check flags unknown fields but not missing ones, so the
  loader enforces required fields (including the PRD's mandatory
  `description`s) from each block schema's `effective_fields()`.

## Execution semantics (PRD §9 interpretations)

- Steps left undispatched when a run halts get the report status
  **not run** (the PRD's six statuses describe executed steps only; a
  halted run still reports every step deterministically).
- In **check** mode, RebootRequired is an ordinary report status and does
  not halt (check is report-only; halting would gain nothing). Error
  still halts unless `--continue-on-error`. Exit code 3 is apply-only.
- In apply mode a dependency that errored or did not run blocks its
  dependents (`not run` with a message); a *skipped* dependency does not —
  `requires` is ordering, not a success demand.
- `--var-file` files are flat `name = value` collections parsed without a
  document schema; expressions evaluate standalone (they cannot reference
  other variables).
- `--var KEY=VALUE` parses VALUE as a WCL expression when possible
  (`--var count=3` is an int), falling back to a plain string.
- Gather params must evaluate before variables resolve, so they may
  reference `--var`/`--var-file` overrides but not gatherer results or
  declared vars that depend on them.

## Host API decisions (PRD §7)

- `shell::run` splits its command with shell-words and executes the
  program **directly** (no shell interpretation); `bash`/`powershell` are
  the escape hatches when shell features are wanted. `powershell` tries
  `powershell` then `pwsh`, so it also works on Linux boxes with
  PowerShell Core.
- The `data` module covers INI only; JSON and TOML are wisp-std's `json`
  and `toml` modules registered as-is (the PRD's "re-export, don't
  duplicate" note).
- `print`/`println` route into `log::info` via a per-thread print hook
  added upstream in wisp-vm (`set_print_hook`).
- Property/params block fields **shadow** outer variables in WCL scope:
  `url = url` is a self-reference (cycle error). Use distinct variable
  names (`tool_url`) for values fed to same-named parameters.

## Authoring & docs (PRD §12/§13)

- `com` binding details: wisp has fixed arity, so `obj.call(name, args)`
  takes a `List[Value]`; VT_DISPATCH results surface through
  `get_object`/`call_object`/`items()` because the dynamic `Value` cannot
  hold an object handle. `wmi_query` flattens each SWbemObject row into a
  property map host-side — scripts never touch enumerators.
- The step DAG renders as a wdoc `diagram { layout = :layered }` of
  flowchart `process` shapes with `:flow` connections.
- `wisp check`/LSP against the emitted `weave.wispi` required a wisp-cli
  fix (committed upstream): when a `wisp.toml` manifest exists, the CLI
  now type-checks against exactly the declared interfaces instead of
  overlaying them on its own stdlib, whose same-named `fs` shadowed the
  config-weave surface.

## wisp binding (PRD §6/§7)

- Script entry points accept two signatures each: plain
  (`fn check(params: Value) -> CheckResult`) or fallible
  (`-> Result[CheckResult, string]`), because `?` requires a `Result`
  return. An `Err` maps to the step's Error status, per the PRD.
- wisp v1 has **no script-to-script imports**: `lib/` folders are
  compiled standalone during validation but cannot be imported by
  resource scripts yet. This is the degradation the PRD's risk table
  anticipated; it lifts when wisp ships imports (its v2 roadmap).
- `print`/`println` in wisp-vm write directly to stdout; routing them
  into `log::info` needs a small upstream hook in wisp-vm (planned with
  M3's stdout-redirection work).
