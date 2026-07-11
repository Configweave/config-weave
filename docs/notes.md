# Implementation notes

Decisions made while binding the PRD to the real WCL and wscript APIs. The
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
- The `data` module covers INI only; JSON and TOML are wscript-std's `json`
  and `toml` modules registered as-is (the PRD's "re-export, don't
  duplicate" note).
- The `template` module renders a Tera template string against a `vars`
  map on the target host: `template::render(template, vars) -> string`.
  Autoescape is **off** (config files, not HTML); a non-map `vars` (other
  than `Null`, treated as empty) errors. This is a deliberate reversal of
  the PRD §1 "no templating engine" non-goal — the host-side engine gives
  resources `{% for %}`/`{% if %}`/filters that WCL's `map`/`join` handles
  awkwardly. It backs `linux_files.template`; author template bodies as raw
  heredocs (`<<'TMPL'`) so WCL's own `$"…${}"` interpolation leaves Tera's
  `{{ }}`/`{% %}` untouched, and feed dynamic data through `vars`.
- `print`/`println` route into `log::info` via a per-thread print hook
  added upstream in wscript-vm (`set_print_hook`).
- Property/params block fields **shadow** outer variables in WCL scope:
  `url = url` is a self-reference (cycle error). Use distinct variable
  names (`tool_url`) for values fed to same-named parameters.

## Authoring & docs (PRD §12/§13)

- `com` binding details: wscript has fixed arity, so `obj.call(name, args)`
  takes a `List[Value]`; VT_DISPATCH results surface through
  `get_object`/`call_object`/`items()` because the dynamic `Value` cannot
  hold an object handle. `wmi_query` flattens each SWbemObject row into a
  property map host-side — scripts never touch enumerators.
- The step DAG renders as a wdoc `diagram { layout = :layered }` of
  flowchart `process` shapes with `:flow` connections.
- `config-weave docs` does **not** embed WCL's renderer. It emits the wdoc
  source (`<out>/_weave_docs.wcl`) and shells out to the `wcl` CLI
  (`wcl wdoc build <src> --out <dir>`) — so the binary defers to the
  installed `wcl` rather than linking `wcl_wdoc`. `wcl` must be on PATH at
  runtime (override the binary with `CONFIG_WEAVE_WCL`). The
  `serve-pkgs-docs` recipe likewise serves via `wcl wdoc serve`.
- `wscript check`/LSP against the emitted `weave.wscripti` required a wscript-cli
  fix (committed upstream): when a `wscript.toml` manifest exists, the CLI
  now type-checks against exactly the declared interfaces instead of
  overlaying them on its own stdlib, whose same-named `fs` shadowed the
  config-weave surface.

## The repo's own docs site (docs/ — wskill + landing)

- The config-weave wskill (docs/wskills/config-weave/) is on **wskill base
  schema 1.0.0** (the WCL repo renumbered; our previous "1.1.0" was the old
  scheme). Entity `kind` is a closed symbol from `schema/kinds.wcl`; the old
  free-text kinds mapped as: "host module" (incl. Windows/scenario-driver
  variants) → `:host_module`, "test backend" → `:test_backend`, "registered
  type" → `:value_type` (all three topic-owned additions in kinds.wcl);
  "file type"/"generated file" → `:file_format`, "language" → `:software`,
  "tool" → `:tool`, "wscript stdlib module" → `:library`.
- The wskill ships four views declared as `artifact` blocks: book, ai_skill
  (committed at .claude/skills/config-weave, regenerated by root
  `just skill-build`, which cleans first), an overview deck
  (data/presentation/), and a training course (data/training/ — commands
  there mirror the runbooks in data/process/; keep them in sync). The docs
  site includes deck + course under `decks/` / `training/` prefixes.
- The landing page (docs/pages/config-weave/) is built from the `lp_*`
  components ported from the WCL repo's landing parts, on the stdlib
  `:website` template — theme-variable painted, no bespoke CSS. The one
  config-weave addition is `lp_term`/`lp_terms` (terminal-transcript
  panels). When the WCL repo's landing parts move again, re-diff against
  `WCL/docs/pages/wcl/landing-parts.wcl`.

## Testlab (`config-weave test` — post-v1 extension)

Packages declare `test` blocks in `package.wcl`; `config-weave test`
runs each in a disposable backend instance. Bindings fixed here:

- **Shape.** `test "name" { description, backend = "docker" (default),
  image, group?, setup?, verify?, step…, gather… }`. Steps mirror
  playbook steps plus `expect = converge (default) | already_configured
  | error | skip | reboot_required`; gathers carry static `params` and an
  `expect` block of top-level key equality assertions. All test values
  must be **static** — tests run against a synthesized variable-free
  playbook, so a variable reference in test properties is a validation
  error. Unqualified `resource`/`from` refs resolve to the declaring
  package.
- **Grouping.** A non-empty `group` field puts a test in a shared
  instance: every test in the same package with the same group name runs
  sequentially inside **one** provisioned instance, amortizing container
  start / VM boot. Grouped tests must agree on `backend` and `image`
  (validated at load — a group provisions one instance from one image),
  and they share the instance's OS state with **no reset between them**
  (vmlab has no snapshot verb), so only group tests that target distinct
  state — the three-run protocol still needs each test's own resources to
  start clean. An empty/absent `group` means the test gets its own
  instance (unchanged from before). Groups are built in `cmd_test`
  (`src/main.rs`) keyed by `(package, group)`; each test carries its
  selection index so output stays in declaration order despite parallel
  runs.
- **Three-run protocol.** Inside the instance the runner executes
  `check`, `apply`, `apply` (all `--json --continue-on-error`, `--jobs`
  forwarded). Run 2's internal re-check proves convergence within one
  process; run 3 proves *cross-process idempotence* and that re-apply is
  a true no-op (a check that only passes on in-process state re-applies
  and surfaces as `configured`, failing the test). Expectation table
  (— = unasserted):

  | expect | check | apply | apply again |
  |---|---|---|---|
  | converge | not_configured | configured | already_configured |
  | already_configured | already_configured | already_configured | already_configured |
  | error | — | error | — |
  | skip | skipped | skipped | skipped |
  | reboot_required | — | reboot_required | — |

- **Execution model.** The host copies a config-weave binary matched to
  the instance's guest OS, resolved lazily once an instance reports it
  (`TestInstance::os()`): linux = `--binary` /
  `$CONFIG_WEAVE_TEST_BINARY` → the running exe if it has no
  `PT_INTERP` header → newest static workspace cross-build artifact;
  windows = `--binary-windows` / `$CONFIG_WEAVE_TEST_BINARY_WINDOWS` →
  newest workspace `x86_64-pc-windows-gnu` artifact (`MZ`-magic
  checked). It also copies a synthesized playbook (one play `test`,
  properties/conditions spliced verbatim, referenced packages copied
  in). A `version` smoke test turns arch mismatches into one clear
  diagnostic. The binary is copied to a shared path
  (`/weave/config-weave`, `C:/weave/config-weave.exe`) and smoke-tested
  **once per group** (`prepare_instance`); each test then gets its own
  working dir `/weave/t/<idx>-<pkg>__<test>/` (forward slashes
  throughout; `C:/weave/t/…` on windows) holding its synthesized playbook
  and facts, so grouped tests never clobber each other. `setup` runs via
  `sh -c` on linux and `cmd /C` on windows, cd'd into that per-test dir
  (created first — exec has no working directory guarantee), and
  `chmod +x` is linux-only.
- **In-container protocol.** Two hidden subcommands on the copied
  binary: `__gather <dir> <pkg.gatherer> [--params-json …]` prints
  `{"ok":…,"value"|"error":…}`; `__verify <script> [--facts <json>]`
  compiles the script against the host API and runs
  `verify(facts) -> bool` (or `Result[bool, string]`), exit 0/1/2 =
  pass/fail/broken. Verify scripts compile during stage-5 validation
  but only ever execute inside instances.
- **Backend selection.** Each test's `backend` field (or the global
  `--backend` override) picks its backend; `cmd_test` discovers every
  backend the selected tests use once, up front, so a broken
  environment is exit 2 before any test runs. The
  `TestBackend`/`TestInstance` traits live in `src/testlab/backend.rs`
  (`TestBackend: Sync`, so one backend is shared across the parallel
  group runners); instances report a `GuestOs` the runner derives
  paths/shell/binary from.
- **Concurrency.** `runner::run_groups` runs independent groups in
  parallel via scoped `std::thread` workers pulling from per-backend
  cursors — **separate caps per backend** because VMs cost far more than
  containers: `--docker-jobs` (default `min(cpu, 8)`) and `--vmlab-jobs`
  (default 2). Total live instances ≤ docker_cap + vmlab_cap. Within a
  group tests stay sequential (shared state). `--jobs` is unchanged — the
  in-instance engine pool, still forwarded as `--jobs` to each
  check/apply run. Provision/smoke failure errors every test in the
  group; a single test's transport trouble errors only that test and the
  rest of the group proceeds.
- **Docker backend.** CLI discovery `$CONFIG_WEAVE_CONTAINER_CMD` →
  `docker` → `podman`; keep-alive via `run -d --entrypoint sleep`;
  images must contain `sleep` and `sh` (distroless unsupported — the
  vmlab backend lifts this). One container per group (an ungrouped test
  is its own one-test group), its tests run sequentially sharing the
  container's filesystem; `--keep` disables teardown and reports the
  handle. Guests are always linux.
- **vmlab backend.** CLI discovery `$CONFIG_WEAVE_VMLAB_CMD` → `vmlab`
  (probed with `--version`). `image` is a vmlab template ref. Each
  provision writes a one-VM lab (`vm "box"`, `nic { nat = true }`,
  template defaults for sizing) into a tempdir whose unique name is the
  lab name (`cw-test-…`), runs `vmlab up` there, then **polls** `vmlab
  osinfo box` until the guest agent answers (up to 300s, 3s between
  tries) — `id == "mswindows"` selects the windows protocol, anything
  else linux. The poll is required because `vmlab up` only blocks on
  agent readiness for VMs something *depends on*, and this lab's single
  VM has no dependents, so a slow (Windows) boot would otherwise hit
  osinfo's own 30s agent wait. exec = `vmlab exec --timeout 3600 box --
  …` (the CLI propagates the guest exit code); copy = `vmlab cp src
  box:dest` — `src` is canonicalized to an absolute path first, since
  vmlab verbs run with the lab tempdir as cwd (creates parent
  directories); teardown = `vmlab destroy` + tempdir
  removal; `--keep` leaves the lab up and reports its directory so
  `vmlab exec`/`console` work post-mortem. A group provisions **one** VM
  and runs all its tests inside it sequentially (the big win — VM boot is
  paid once per group, not per test); `--vmlab-jobs` bounds how many VMs
  boot at once. Windows guests need the guest agent in the template
  (vmlab requires this anyway for readiness) and `setup` written for
  `cmd /C`.
- **Reporting.** Exit 0 = all passed, 1 = any failed/error, 2 =
  validation/environment. `--json` emits a schema-stable object with
  `mode: "test"`; the runner parses in-container reports with the same
  `JsonRunReport` types that produce them.
- **Scenarios (scripted, multi-stage, over a declared vmlab lab).** The
  three-run protocol can't reboot or network multiple machines, which a
  Windows DC promotion (apply → reboot → apply) and a member join both
  need. A package declares a `scenario { lab, script }`: `lab` is a dir
  holding a `vmlab.wcl` (the full vmlab feature set — segments, static
  IPs, DC-as-DNS, depends_on), and `script` is a driver
  (`fn run(lab: Lab) -> bool`/`Result[bool,string]`) that runs
  **host-side** against the live lab via the `testlab` wscript host module
  (`src/hostapi/testlab.rs`): `Lab`/`Machine` opaque handles over the
  `TestLab`/`TestInstance` traits. The handles hold `Rc<RefCell<LabState>>`
  — wscript opaque values are `Rc`-backed and single-threaded, so scenarios
  run on one thread (no `Arc` needed, unlike vmlab's own scripting which
  bridges to tokio). **Why a declared lab, not script-provisioned:** the
  vmlab lab daemon loads its config once at first `up` and never reloads
  (`labd::lab::Lab { config }`), so a VM appended to a running lab is
  invisible (`no vm "b" in lab`) — proven by smoke. Declaring every VM up
  front sidesteps this: `open_lab` copies the lab dir, rewrites the `lab
  "…"` name to a unique one (registry isolation), and `lab.machine(name)`
  does `vmlab up <name>` — the VM is already in the daemon's config, so it
  starts on demand (resource-friendly, one at a time) with no reload.
  `machine.apply_resource(key, props)` synthesizes a one-step playbook
  (`synth::synthesize_resource`, rendering `props` as a WCL `properties`
  block), copies the binary in once per machine, runs `config-weave
  {check,apply} --json`, and returns the step's status; `machine.reboot()`
  = `vmlab vm restart` + osinfo re-poll (900s, DC promotion finalizes on
  boot). New trait methods `reboot`/`wait_ready` and
  `TestBackend::open_lab`/`TestLab` carry this; the single-VM `box` path is
  unchanged (its instance owns lab teardown, lab machines don't). Scenarios
  compile in stage-5 against `hostapi::scenario_context()` (host API +
  `testlab`), so `validate` catches a broken driver; at run time they
  execute sequentially after the parallel test groups, each owning its lab.
  `windows_domain:ad_matrix` is the first: forest root (DNS) → member join
  → additional DC → second forest (own segment), all over real reboots.
  The two-VM + reboot integration is smoke-verified on vmlab with Alpine.

## wscript binding (PRD §6/§7)

- Script entry points accept two signatures each: plain
  (`fn check(params: Value) -> CheckResult`) or fallible
  (`-> Result[CheckResult, string]`), because `?` requires a `Result`
  return. An `Err` maps to the step's Error status, per the PRD.
- wscript v1 has **no script-to-script imports**: `lib/` folders are
  compiled standalone during validation but cannot be imported by
  resource scripts yet. This is the degradation the PRD's risk table
  anticipated; it lifts when wscript ships imports (its v2 roadmap).
- `print`/`println` in wscript-vm write directly to stdout; routing them
  into `log::info` needs a small upstream hook in wscript-vm (planned with
  M3's stdout-redirection work).

## weave-server: systems, package repo, graphical editors (post-v1)

The web GUI grew three pillars; the CLI stays purely local (the PRD's
remote-execution non-goal holds — all transport orchestration lives in
weave-server).

- **Live engine events.** `check`/`apply` gained `--events-ndjson`
  (mirror of the testlab flag): one JSON object per stderr line, `ts`
  epoch-ms, events `run_started` (with the planned step list, idx order
  = the scheduler's), `gather_started/finished`, `step_started`,
  `step_phase` (checking | applying | re-checking), `step_finished`
  (status id + message + duration), `step_resolved`. stdout still
  carries the one final `--json` report. `list --json` now also emits
  each package's `resources`/`gatherers` with full param schemas (the
  GUI's docs pages and schema-aware property editors feed off it).
- **Services inventory.** `{server root}/services.wcl`, schema
  `<weave/services.wcl>` (embedded in both crates from
  `src/vocab/services.wcl`): `service "name" { system "name" { kind =
  "direct"|"remote", os, arch, transport "ssh"|"winrm" { host, port?,
  user, password?, private_key?, use_tls } assignment { playbook, play } } }`.
  Systems may have multiple playbook assignments. Credentials are inline
  by explicit choice; the server keeps the file 0600 and regenerates it
  on every GUI edit via wcl_lang's AST builder + canonical printer
  (hand comments do not survive a GUI save). A malformed services.wcl
  refuses server startup rather than risk a clobbering save.
- **Service schedules.** A service can persist named, enabled schedules
  targeting one system assignment with a `check` or `apply` action and a
  six-field UTC cron expression. The server evaluates schedules every
  15 seconds; automatic and “Run now” executions share the system-run
  manager and carry schedule/trigger metadata for session history.
- **System runs.** *Remote* systems run the playbook locally on the
  server with the connection details injected as vars via a 0600
  `--var-file`: `system_name/host/port/user/password/private_key/
  transport/os` (flat identifiers — var keys cannot contain dots).
  Playbooks consume them by declaring same-named `vars` entries the
  overrides replace. *Direct* systems get the matching static build
  (registry keyed `{os}-{arch}`, `--deploy-binary` with `just release`
  artifact fallbacks) plus the playbook staged to `/tmp/weave-run-{id}`
  (`C:/Windows/Temp/…`), run there with remote stdout always redirected
  to `<stage>/report.json` and fetched afterwards — the one protocol
  that survives both ssh and PSRemoting stream semantics. Windows
  remote commands always go through `powershell -EncodedCommand`
  (UTF-16LE base64), immune to ssh/cmd quoting; winrm shells out to
  `pwsh` (PSWSMan) and is best-effort — Windows targets are fully
  served over Win32-OpenSSH. Event topic `sysrun:{id}`, deploy progress
  as server-synthesized `deploy_phase` events.
- **Package repository.** The repo defaults to `packages/` inside the
  served root (created on demand — zero flags needed); `--packages-dir`
  overrides it, e.g. pointing at a config-weave-pkgs checkout's pkgs/
  folder. `testdata/packages/` ships three demo packages (demo_files,
  demo_users, sysinfo) so `just serve` has content out of the box. It is
  a folder of package dirs. The CLI only understands playbook dirs, so the server
  synthesizes a tempdir wrapper (`playbook "package-repo"` +
  `pkgs/<name>` symlinks, fingerprint-cached on package.wcl mtimes) —
  safe because the testlab's synthesize step copies packages
  (dereferencing symlinks) before anything reaches an instance.
  Add-to-playbook *copies* (the runbook editor refuses symlink
  escapes). Debug-a-test = a kept single-test run; kept instances stay
  attachable after completion and `POST /api/runs/{id}/teardown` reuses
  the orphan cleanup to destroy them on demand.
- **Packages are the editing hub.** The runbook file tree hides `pkgs/`
  (client-side only — paths stay reachable by the file API); runbooks
  show an installed-packages card instead, and `DELETE
  /api/runbooks/{rb}/packages/{name}` is add-to-runbook's inverse
  (symlinked entries refused before `remove_dir_all`). The package
  editor page serves two sources through one `WorkspaceScope`
  abstraction: repo packages edit **in place** via
  `/api/packages/{name}/{tree,file,doc/*}` (same handler guts as the
  runbook routes, resolved against the packages dir; the wrapper cache
  self-invalidates on the manifest mtime), and runbook copies ride the
  existing runbook endpoints re-rooted at `pkgs/<name>`
  (`prefixedScope` — no extra server surface). The Packages section is
  always visible; unconfigured just means a hint instead of a list. The
  runbook's installed-packages card is the sync point: an add picker
  over repo packages not yet installed, and a "not in repository" badge
  with `POST /api/runbooks/{rb}/packages/{name}/import` (copy into the
  packages dir, 409 when present) for packages that only exist inside a
  runbook.
- **Graphical editors (DocJson).** playbook.wcl / package.wcl get a
  Visual mode: `__wcl-inspect` extracts a structural doc from the
  `parse_for_edit` AST (every leaf `{lit}` or `{expr: "source"}`;
  extraction **fails closed** on constructs forms can't represent, e.g.
  non-field items inside schemaless maps); `__wcl-render` syncs the doc
  back onto the current file's AST — blocks matched by `_orig`-or-name,
  updated in place so comments survive (canonical printer re-emits them
  `#`-prefixed), unknown items preserved — and re-parses the output
  before it can reach disk. Saves go through a content-hash conflict
  guard (409 on concurrent change). Formatting canonicalizes on first
  visual save (one-line `param` blocks expand; printer is idempotent
  after).
- **SSE has no replay.** EventSource connections miss events published
  while connecting, so both run views treat the server's per-run event
  buffer as authoritative: seed from the snapshot, then poll until one
  final reseed of a finished run has happened.
