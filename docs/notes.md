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
- **Remote repositories.** `{server root}/repos.wcl`, schema
  `<weave/repos.wcl>` (embedded from `src/vocab/repos.wcl`, same
  one-source-of-truth pattern as services): `repo "name" { url,
  subdir?, runbooks_subdir?, branch?, sync_cron?, webhook_secret? }`.
  Git repos cloned shallow (`--depth 1`) into
  `{root}/.repo-cache/<name>` — a dot-dir, invisible to the runbook
  listing and the local scan. A *missing* repos.wcl is seeded with the
  stdlib (`github.com/Configweave/config-weave-pkgs.git`, subdir
  `pkgs`); a present file — even empty — is respected, so deleting the
  stdlib sticks. The file is written 0600 (it may carry webhook
  secrets, same posture as services.wcl). Startup clones only *absent*
  caches, in the background (the server must start offline / without
  git and keep serving local packages). CRUD via `GET|POST /api/repos`,
  `GET|PUT|DELETE /api/repos/{name}`; adding clones synchronously and
  persists the entry even when the clone fails (the git stderr rides
  back in `error`, Sync retries later). The package scan merges
  sources — local packages dir first, then each repo's cache in
  repos.wcl order; first name wins (local shadows remote, collisions
  reported in the inventory's `shadowed` array) — and the wrapper
  symlinks whichever real dir won. Every inventory entry carries a
  `source` tag (`"local"` is reserved; repo names may not use it).
- **Repo runbooks, sync triggers, write-back.** A repo with
  `runbooks_subdir` (a subdir of playbook dirs; `"."` = the checkout
  root) also provides runbooks: `runbooks::scan_runbook_sources` merges
  the local root first, then each repo's runbooks root, through the
  same first-name-wins policy as packages (shared
  `packages::merge_sources`; `GET /api/playbooks` now returns
  `{ runbooks: [{name, source}], shadowed: [...] }`). `runbook_dir` is
  `resolve_runbook` under the hood, so *every* `/api/playbooks/{rb}/*`
  route (tree/file/doc/validate/inventory/zip download/test runs/system
  assignments) transparently serves repo runbooks; zip upload 409s on a
  name any repo already provides (silent shadowing refused).
  **Sync is clean-gated**: `sync_repo` skips (409 from
  `POST /api/repos/{name}/sync`, `skipped` rows in `/api/repos/sync`)
  whenever the cache has uncommitted edits (`status --porcelain`) or
  unpushed commits (`rev-list --count @{upstream}..HEAD` — our clones
  are single-branch so the upstream ref always exists; a hand-mutated
  cache degrades to ahead=0 with a warning and Discard repairs it).
  A clean cache fast-forwards via fetch with an explicit
  `+refs/heads/<b>:refs/remotes/origin/<b>` refspec (keeps the
  ahead-probe accurate) + `reset --hard FETCH_HEAD`. Three triggers,
  all funneling through the same `sync_repo` under `repo_git_lock` and
  counted in `weave_repo_sync_dispatch_total{repo,trigger,outcome}`:
  manual (the UI buttons), cron (`sync_cron`, six-field UTC cron shared
  with service schedules via `scheduler::cron_due`, checked on the same
  15s tick, spawned so slow fetches never block it), and webhook.
  `POST /api/webhooks/repos/{name}` is the only route besides /metrics
  without a JWT: it authenticates per call against the repo's
  `webhook_secret` — GitHub/Gitea HMAC `X-Hub-Signature-256` over the
  raw body, or the plain token in `X-Gitlab-Token`/`X-Weave-Token`
  (constant-time compares); unknown repo, unset secret, and bad auth
  all answer a uniform 404. A policy skip answers 200 (forges must not
  retry it). **Write-back**: repo packages *and* repo runbooks are
  editable in place (the old remote-package 403 is gone) — edits dirty
  the cache, badged in the UI (repo table + a write bar on repo-sourced
  editors polling `GET /api/repos/{name}`), and settled by
  `POST /api/repos/{name}/commit` (`add -A`, commit as `-c user.name/
  user.email` from `--git-user-name`/`--git-user-email`, defaults
  `weave-server <weave-server@localhost>`, then push to the origin
  branch — shallow clones push fine) or `POST /api/repos/{name}/discard`
  (fetch + `reset --hard FETCH_HEAD` + `clean -fd`). A rejected push
  (remote moved) is a 409 — no auto-rebase (a conflicted rebase would
  wedge a headless cache); settle via Discard or a real checkout. Push
  reuses ambient git credentials (ssh agent / credential helper);
  prompt-requiring remotes fail fast (`GIT_TERMINAL_PROMPT=0`).
- **Playbook zip transfer.** `GET /api/playbooks/{rb}/download` streams
  a self-contained zip (entries under a `<rb>/` top folder, `pkgs/`
  included, copy_dir_filtered's skip list + symlinks excluded);
  `POST /api/playbooks/upload?name=…` (raw zip body, 64 MB cap) creates
  a runbook from one. Accepted layouts: playbook.wcl at the zip root
  (needs `?name=`) or inside exactly one top-level folder (its name is
  the default; `__MACOSX`/`.DS_Store` junk ignored). Zip-slip guarded
  via `enclosed_name()` (unsafe entries refuse the whole upload),
  symlink entries skipped, extraction staged in a dot-tempdir inside
  the root and renamed into place; name conflicts 409.
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
- **Package API docs (Docs tab).** The package page grew an
  Overview/Docs tab bar (tab in the `View` union, ServicesView idiom).
  The DocJson pipeline moved out of `src/model/` into the shared
  workspace crate `docjson/` (`weave-docjson`: docjson + inspect_ast +
  emit, wcl_lang-only deps; the CLI keeps its `model::docjson` paths
  via re-exports, `just test` runs the crate's suite explicitly since
  `default-members` would skip it). weave-server consumes it directly:
  `GET /api/packages/{name}/docs` and `GET
  /api/playbooks/{rb}/packages/{name}/docs` read package.wcl and run
  `parse_for_edit` + `extract_package` **in-process** — the docs path
  never shells out (extraction fails closed ⇒ 422 with the diags, which
  the tab surfaces). The tab renders per-resource/per-gatherer sections
  (description, concurrency, script, param table) plus copyable WCL
  usage snippets generated client-side from the param schemas: a `step`
  block per resource and a `gather` block per gatherer, required params
  filled with defaults or type placeholders, optional ones
  `#`-commented, and `pkg.member` references built from the package
  *directory* name (what step `resource =` strings resolve against),
  not the manifest's declared name.
- **SSE has no replay.** EventSource connections miss events published
  while connecting, so both run views treat the server's per-run event
  buffer as authoritative: seed from the snapshot, then poll until one
  final reseed of a finished run has happened.

## weave-server: Prometheus metrics + Loki logs (post-v1)

The per-service Monitoring/Logs tabs, backed by an optional
Prometheus + Loki pair (`just stack-up` runs the compose test stack:
weave-server image + prom/prometheus + grafana/loki, configs under
`deploy/`).

- **Flags.** `--prometheus-url` (env `PROMETHEUS_URL`) and `--loki-url`
  (env `LOKI_URL`), both optional and independent. Unset ⇒ the proxy
  endpoints answer 503 and the tabs render a "not configured" empty
  state. `--loki-url` additionally turns on log shipping: a
  tracing-loki layer pushed from the server itself (no promtail).
- **Metrics** (`server/src/monitoring.rs::setup`, axum-prometheus with
  prefix `weave`; `/metrics` is on the open surface — no claims — so
  Prometheus can scrape; a future `--metrics-token` could gate it):
  - `weave_http_requests_{total,duration_seconds,pending}` —
    method/endpoint/status, endpoint = matched route template (the
    layer sits on the built router, after routing).
  - `weave_system_runs_total{service,system,action,trigger,status}`,
    `weave_system_run_duration_seconds{service,system,action}`
    (buckets 1s–30m), `weave_system_runs_active{service,system}` —
    settled in `sysruns::settle`/`start`.
  - `weave_schedule_dispatch_total{service,schedule,outcome=started|skipped}`
    and `weave_scheduler_last_tick_timestamp_seconds` (alert on a
    wedged scheduler) — `scheduler.rs` tick loop.
  - `weave_test_runs_total{status}`, `weave_test_runs_active` —
    `runs.rs`. Label cardinality is bounded by the inventory size
    (service/system/schedule names).
  - PromQL counter-birth caveat: the summary/timeseries use
    `increase()`, which can't see a label-set's first-ever increment
    (nothing→1 has no in-window delta), so the very first run of a
    given service/system/action/trigger/status combination reads 0;
    every run after that counts normally. Standard Prometheus
    behavior; not worth pre-registering ~24 zero series per system.
- **Loki binding.** Static stream labels only: `{app="weave-server"}`
  plus tracing-loki's own `level` — zero label-cardinality risk.
  Everything dynamic (`service`, `system`, `run_id`, `playbook`,
  `play`, `action`, `trigger`) rides as tracing fields, which
  tracing-loki flattens into the JSON log line ⇒ LogQL filters via
  `| json | service=\`x\``. Engine output lines are mirrored to
  tracing target `weave::runlog` in `sysruns::relay_line`/
  `deploy_phase` — that target is `off` on the console filter (the
  fmt layer keeps its old signal) and `info` on the Loki layer.
  tracing-loki drops events under backpressure; very chatty runs may
  lose lines.
- **Proxy endpoints** (browser never talks to the backends; queries
  are composed server-side from structured params — raw PromQL/LogQL
  is never accepted, label values are escaped): `GET
  /api/monitoring/status` (capability probe), `GET
  /api/services/{s}/monitoring/summary?range=`, `…/timeseries?range=
  &step=&system=`, and `GET /api/services/{s}/logs?range=&limit=
  &system=&run=&level=&search=&source=runs|systems`.
- **Forward convention for managed systems.** Agents on targets
  (node_exporter relabeling, Grafana Alloy, promtail) must attach
  `service=<name>` and `system=<name>` **labels** matching the
  services.wcl names. The Logs tab's `source=systems` selector is
  exactly `{service="X"[,system="Y"]}`, so labeled streams appear
  with no server change; future node-metric queries key off the same
  pair.

## config-weave-pipeline (the CI/CD daemon — post-v1)

A separate headless daemon (`pipeline/` crate, binary
`config-weave-pipeline`) that runs triggered `pipeline.wcl` pipelines.
Bindings fixed here:

- **Shape.** `src/vocab/pipeline.wcl` (embedded in the CLI as the one
  source of truth, registered in `src/vocab.rs`; the daemon embeds the
  same file via `include_str!("../../src/vocab/pipeline.wcl")`, the
  services.wcl idiom). A `pipeline "name"` block owns `property`
  (name/type=string|int|bool/required/default), `secret` (inline value,
  0600 on disk), `target` (os + a `transport "ssh|winrm"` reusing the
  services.wcl transport shape; `password`/`private_key` may be
  `"secret:NAME"`), `trigger` (type=manual|webhook|schedule with
  `webhook_secret`/`cron`/`enabled` + `bind` property presets), and
  ordered **steps**: `script` (a `run` body, `on = "local"` or a target
  name, `env` values literal/`prop:`/`secret:`) or `play` (`playbook` +
  `play` + `action`, `var` values literal/`prop:`/`secret:`). Each block
  kind needs its own WCL `type`, so `env`/`var` are two identical types
  (`WeaveEnvVar`/`WeavePlayVar`).
- **Step ordering.** Script and play steps must interleave in
  declaration order. The loader (`pipelines.rs`) does a **single
  `block.blocks()` pass** matching on `kind()` into one ordered
  `Vec<StepDef>` (unlike systems.rs' two order-independent filtered
  passes); `save()` iterates that Vec so items round-trip in order. The
  inventory lives in `{--dir}/pipelines.wcl`, regenerated 0600 via
  wcl_lang's AST builder + printer (services.wcl save idiom).
- **Execution** (`exec.rs`, run lifecycle `runs.rs` modeled on
  sysruns.rs). A trigger binds properties (defaults, `required`, coerce
  by type), then each step runs in order on topic `pipeline:{id}`:
  local scripts via `sh -c` / `<shell> -Command` with env vars in the
  process env (never argv); remote scripts over the shared
  `weave-remote` transport (`Transport::new`, `exec_stream`), env
  prepended as `export`/`$env:` into the script body (visible in the
  remote process list — documented tradeoff, prefer plays for secrets);
  plays shell out to `config-weave apply|check {playbooks_dir}/{playbook}
  {play} --json --events-ndjson --continue-on-error --var-file <0600
  tmp>`, streaming stderr NDJSON and collecting the stdout report.
  Failure semantics: a non-zero step fails; if `stop_on_failure`
  (default) the run stops and remaining steps are `skipped`; an infra
  error (missing secret, spawn/probe failure) is a hard `error`.
- **Transport crate.** `server/src/transport.rs` moved to a shared
  `transport/` crate (`weave-remote`) so both weave-server and the
  daemon use one copy. The only coupling removed was
  `Transport::for_system(&SystemDef)` → `Transport::new(&TransportConfig,
  TargetOs)`; `TargetOs`/`TransportKind`/`TransportConfig` moved into the
  crate and are re-exported from `server/src/systems.rs`.
- **Auth (forge-auth RS256/JWKS).** `pipeline/src/auth.rs` is a custom
  forge-server `TokenValidator` accepting forge-auth OIDC access tokens
  (user logins **and** machine/exchange tokens). It replicates ~15 lines
  of RS256 + JWKS validation with `jsonwebtoken` rather than depending on
  the forge-auth crate (which drags sqlx/ldap3/openidconnect). JWKS is
  fetched at startup and refreshed every 5 min into an `RwLock` cache, so
  the sync `validate` never blocks on the network; an unknown `kid`
  rejects (surfaces on the next refresh). `AccessClaims` → forge `Claims`:
  `preferred_username` present ⇒ user (its value is `sub`); absent ⇒
  machine (`sub` = `azp`/client id, plus a synthetic `machine` role).
  `iss` is validated; `aud` only when `--forge-audience` is set
  (forge-auth deliberately leaves aud to the resource server). Wired via
  `ForgeApp::auth_validator` (external-issuer mode: no login minting). A
  loopback bind with no `--forge-issuer` runs open (dev); a non-loopback
  one is refused unless `--no-auth`.
- **weave-server integration.** The daemon is headless; the config-weave
  site reaches it through a reverse proxy (`server/src/pipeline_proxy.rs`,
  the monitoring-proxy pattern): `--pipeline-url` + a machine token, with
  routes `GET /api/pipeline-config` (capability probe) and
  `{GET,POST,PUT,DELETE} /api/pipeline/{*rest}` → `{pipeline_url}/api/*`,
  body/status passed through verbatim (the daemon already speaks the
  forge envelope). The browser holds an HS256 weave-server token, not a
  forge-auth token, so the proxy attaches a forge-auth **machine token**:
  a static `--pipeline-token`, or one auto-refreshed via the
  `refresh_token` grant (`--pipeline-refresh-token`/`--pipeline-token-url`
  /`--pipeline-client-id`). Note: forge-auth has **no
  `client_credentials` grant**, hence the static-token / refresh-token
  bridge rather than a client-credentials fetch. The SolidJS Pipelines
  section (`web-ui/src/components/Pipeline{s,,Run}View.tsx`) lists/opens/
  triggers pipelines and **polls** `GET /api/pipeline/runs/{id}` every
  2.5s — the daemon's SSE bus is not weave-server's, so there is no event
  replay; the run snapshot's event buffer is authoritative.
- **Authorization seam.** For v1 every daemon handler requires only a
  valid token (presence), matching weave-server's posture; the machine/
  user distinction rides in `Claims.roles` for a future `require_role`
  gate on mutations/triggers. Secret **values are never returned** by any
  read endpoint (`SecretDef.value` is `skip_serializing`); `update`
  preserves a stored value when an incoming secret's value is empty, so a
  redacted round-trip never wipes a secret.
