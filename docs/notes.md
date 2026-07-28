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
  (`import <weave/playbook.wcl>` / `<weave/package.wcl>` /
  `<weave/repo.wcl>`), exactly how wdoc ships its stdlib. The engine
  appends the import line at the *end* of user sources, so user spans
  are untouched and authors never write import lines.
- `pkgs/repo.wcl` (`<weave/repo.wcl>`, PRD §17's "config-weave fetch")
  is **tooling metadata, not playbook semantics**: `repo` blocks list
  registered git package repos, `package` blocks record installed
  packages with source repo + exact commit. The model loader never
  reads it (`load_packages` skips non-dir entries under `pkgs/`); only
  `config-weave pkg` does, which shells out to the `git` binary
  (ambient credentials → private repos work) and caches shallow clones
  under `{playbook}/.repo-cache/<repo>`. The file is regenerated from
  structs on every pkg command — hand edits to values survive, comments
  do not.
- `var x = expr` (PRD sketch) became a `vars { x = expr }` block.
- `params schema { version: string { … } }` (PRD sketch) became
  `param "version" { type = "string" … }` blocks; coarse types are
  `string|int|float|bool|list|map|symbol|duration`. §8 validation behaviour is
  engine-side and unchanged from the PRD contract. `symbol` is for
  enumerated tokens (the `ensure = :present|:absent` idiom): WCL symbols
  and strings both convert to the same script-side string, so scripts see
  `"present"` for `default = :present`. Because the two spellings are
  indistinguishable after conversion, validation enforces the `:symbol`
  form at the source: `ensure = "absent"` is an error telling you to write
  `:absent`. `is_symbol_literal` in the loader is the only place the
  distinction survives, so every path that checks a symbol property has to
  carry it — which is why `StaticPair` records `symbol_literal`. A value
  that only resolves at run time (from a variable) cannot be checked this
  way; the set membership still is.
- A symbol param may enumerate its legal values with `symbol "name" {
  description }` child blocks. Declaring any *closes* the set: the
  declared `default`, every step `properties` / gather `params` value,
  and every value that only resolves at run time (from a variable) are
  checked against it, and the generated resource/gatherer pages list the
  values under a "Symbol values" heading. Declaring none leaves the param
  open to any token, which is what every symbol param did before the
  blocks existed — so the feature is opt-in and adding it broke no
  existing package. Only `symbol` params may enumerate; the blocks are an
  error on any other coarse type. Symbols name themselves in their
  WCL-spellable form (`symbol "on_demand"`, not `"on-demand"` — the lexer
  stops a `:symbol` at `[A-Za-z0-9_]`), and a script that needs the
  hyphenated spelling translates it itself.
- Step `properties = { … }` became a `properties { … }` child block;
  gather `params = { … }` likewise a `params { … }` block.
- Gatherers document their gathered value with `returns "key" {
  description type }` child blocks (same coarse types as params). Mostly
  documentation metadata: the docs render a Returns table, and the engine
  does not check that a gathered map carries these keys, or only these
  (gathered maps may legitimately carry dynamic ones).
  A key declared `type = "symbol"` is the exception, and is *typed*: its
  value binds into the variable space as a real `Value::Symbol` (via
  `convert::dyn_to_wcl_returns`, which the gather phase uses in place of
  `dyn_to_wcl`), and its declared set is enforced against what the script
  actually returned. So `init_system` declares `returns "init" { type =
  "symbol" symbol "systemd" { … } … }`, the script keeps emitting the bare
  token `"systemd"` (the leading `:` is a WCL-source spelling, not a script
  one), and a playbook writes `condition = init.init == :systemd`.
  The asymmetry this buys is deliberate but sharp-edged: WCL's `values_eq`
  says `Symbol("systemd") != Utf8("systemd")`, so comparing a symbol fact
  against `"systemd"` is silently *false* rather than an error, and
  interpolation renders `:systemd` rather than `systemd`. Test `expect`
  blocks are checked for the `:symbol` spelling and for set membership
  (`check_expect_static`) precisely so the string form can't be copied into
  a test and quietly stop matching. Only top-level keys are typed —
  `returns` documents one level, so nested maps stay plain data.
- A `duration` param is written as a bare WCL unit literal (`max_age =
  30min`, suffixes `ns|us|ms|s|min|h|d` — note WCL spells minutes `min`,
  since `m` is metres in `std.Distance`) and reaches scripts as a plain
  `Int` of **nanoseconds**, `std.Duration`'s own base unit. The quoted
  spelling is a type error: hand-rolled `"30m"` parsing inside scripts is
  exactly what the unit type exists to delete.
  Getting there needed one addition to WCL. A unit literal resolves against
  a *declared* type, and `properties` / `params` are `@schemaless`, so
  plain `Field::value()` can only report `UnitWithoutType` for them.
  `@schemaless` turns out to suppress only WCL's *membership* check, not
  type resolution — but with no declared field there is no type to resolve
  against either, and no public API hands back the unresolved
  `Value::PendingUnit`. So WCL gained `Field::value_typed(type_fqn)`, a
  thin public wrapper over the existing `coerce_value_to_type` that
  resolves against a caller-supplied type and ignores the schema's.
  `convert::field_value_dyn` is the single choke point that uses it: it
  retries an `UnitWithoutType` failure against `std.Duration` and reports
  the *original* error if that fails too, so `max_age = 4GiB` complains
  about the unit rather than about a coarse type. Every site that reads a
  property or param field goes through it. The one exception is a param's
  own `default`, which the vocab declares `utf8?` — a duration literal
  there is coerced against `utf8` and fails before the retry could fire, so
  `load_params` consults the already-parsed coarse type and calls
  `value_typed` directly.
- Variables (gatherer results, declared vars, `--var`/`--var-file`
  overrides) bind by generating an in-memory system import
  `<weave/vars.wcl>` containing `let` declarations. Gatherer results are
  injected through an `Environment` builtin (`__weave_var`), so any value
  shape round-trips without literal serialization. Conditions and
  properties evaluate lazily against that scope at run time.
- WCL's block check flags unknown fields but not missing ones, so the
  loader enforces required fields (including the PRD's mandatory
  `description`s) from each block schema's `effective_fields()`.

## Encrypted values (`secret("…")` — post-v1 extension)

Password-encrypted values live **in** `playbook.wcl`, encrypted in place
by a CLI command. Bindings fixed here:

- **A builtin, not a block.** `secret("…")` is a WCL `Environment`
  builtin registered next to `__weave_var`, so it works anywhere an
  expression does — a `vars` entry, step `properties`, gather `params`, a
  `condition` — with **no vocabulary change**. That also means DocJson
  round-trips it for free: `inspect_ast` classifies a call as
  `Val::Expr(source)` and `emit` re-parses it, so `docjson/` needed no
  managed-kind entry. A dedicated `secret "name" { … }` block was the
  alternative and would have been simpler to validate, but it would have
  confined secrets to named variables.
- **Two resolver modes.** `model::load` registers a *locked* builtin
  returning an empty `utf8`; the run path registers an *unlocked* one that
  decrypts. The placeholder exists so `check_params` can type-check a
  secret as the string it always is without a password — which is what
  keeps `validate` and `docs` password-free. Whether a value is actually
  encrypted is never decided by evaluation: that is a separate syntactic
  scan (`secrets::scan`) over the `parse_for_edit` AST, so the
  "not encrypted yet" error needs no key. Every `match` in that walker is
  exhaustive with no `_` arm — a new `wcl_lang` AST variant must break the
  build rather than silently hide a call site, which would be committed in
  the clear.
- **Blob format.** `CWENC1.<b64url salt>.<b64url nonce>.<b64url ct||tag>`,
  Argon2id (m=19456 KiB, t=2, p=1 — OWASP's minimum, deliberately light
  because derivation also runs in testlab containers) into
  XChaCha20-Poly1305. The version tag pins both primitives *and* their
  parameters; changing either means `CWENC2`. The AAD is the constant
  `config-weave/secret/v1`, **not** the variable's name: binding a blob to
  its name would turn "you renamed a variable" into an indistinguishable
  "wrong password", and an AEAD failure carries no detail to tell them
  apart. The salt is carried per blob but *reused across a file*, so one
  playbook derives one key however many secrets it has.
- **"Same password" is the AEAD, not a stored verifier.**
  `secrets encrypt` decrypts every already-encrypted value before writing
  anything; any failure aborts and points at `secrets rekey`. So a new
  secret can only be added by someone who can already read the ones in the
  file, and no separate salt/verifier header is needed. `rekey` decrypts
  with the old password, mints a fresh salt, and re-encrypts everything —
  sweeping up any still-plaintext calls in the same pass.
- **Splice, don't reformat.** The rewrite replaces the byte span of the
  whole `Expr::Call` (`Expr::Utf8` carries no span of its own, so the call
  is the smallest re-writable unit) and re-parses before writing.
  Deliberately *not* the `format::to_source` round-trip `docjson` uses:
  `wcl_lang` has no CST, so re-printing canonicalises the file (`//` → `#`,
  indentation, one-liners) and `encrypt` would reformat a hand-authored
  playbook as a side effect of changing one string. Safety rails are
  `wcl set`'s: re-parse, then temp-file + rename.
- **Passwords: `$CONFIG_WEAVE_PASSWORD`, `--password-stdin`,
  `--password-file`** — exactly one, never a prompt. A missing password is
  always exit 2 so an automated run fails loudly instead of blocking on a
  terminal that isn't there. One trailing newline is stripped. A playbook
  with no `secret()` calls never asks. `rekey`'s new password comes from
  `--new-password-file` or `$CONFIG_WEAVE_NEW_PASSWORD`.
- **Verified up front.** A run decrypts every value before executing
  rather than leaving it to the evaluator. WCL is lazy, so a secret no step
  references is never evaluated — a wrong password would otherwise produce
  a clean, successful run and only bite later, when some other step began
  using the value. Doing it eagerly also warms the key cache and registers
  every plaintext with the redactor.
- **Playbook-only.** `secret()` in a `package.wcl` is a validation error:
  packages are shared and distributed via git, so a package cannot hold a
  value encrypted under one playbook's password. That also covers `test`
  and `scenario` blocks, which live in packages and run in disposable
  instances with no password.
- **Redaction.** Decrypted plaintexts land in a process-global registry
  and are scrubbed to `***` at four choke points: `Diag` construction
  (`diag::finish` — the pre-existing leak vector, since a spanned
  diagnostic attaches the surrounding source), `hostapi::log::emit` (the
  single path for both `log::*` and redirected `print`), `StepReport`
  construction in `engine/run.rs` (step messages come straight from
  scripts), and the docs generator, which elides a blob to `secret(…)`.
  Values under 4 bytes are not registered — masking them would corrupt
  unrelated output more than it protects.

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
- The `data` module covers INI only; JSON, TOML and XML are wscript-std's
  `json`, `toml` and `xml` modules registered as-is (the PRD's "re-export,
  don't duplicate" note). `regex` is registered for the same reason: the
  alternative is `shell::run("grep …")`, which is a subprocess and
  platform-dependent. Two sharp edges worth knowing: every `regex`
  function takes **`(pattern, text)`** — the argument types are identical,
  so swapping them compiles and then silently never matches — and an
  invalid pattern is a *fault*, not an `Err`, surfacing as the step's
  Error status. `xml` maps elements to nested maps with attributes under
  `@attrs` and text under `#text`; mixed content concatenates into one
  `#text`, so a round trip moves text ahead of its sibling elements.
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
  runtime (override the binary with `CONFIG_WEAVE_WCL`). `docs --serve`
  (used by the `serve-pkgs-docs` recipe) hands the emitted source to
  `wcl wdoc serve` after rendering — same binary resolution, blocks until
  the dev server exits; `--addr` passes through.
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
runs each in a disposable **vmlab** instance. vmlab is the only backend
(the docker/podman one was removed 2026-07-26 — see "Why vmlab only"
below). Bindings fixed here:

- **Shape.** `test "name" { description, image | template, memory?,
  group?, setup?, verify?, step…, gather… }`. Exactly one of `image` (an
  OCI ref → a vmlab **container**) or `template` (a vmlab template ref →
  a full **VM**) is required; neither and both are validation errors that
  name the fix. `memory` is a WCL byte size emitted **unquoted** into the
  lab file (`memory = 4GiB`; quoting fails vmlab's `std.ByteSize` field)
  and overrides the instance's RAM — a container defaults to 256MiB,
  which SQL Server refuses to start under, and guest memory is allocated
  on demand so raising it for one heavy image costs the light ones
  nothing. Grouped tests must agree on it. Steps mirror
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
  start / VM boot. Grouped tests must agree on their target — same kind,
  same ref (validated at load — a group provisions one instance), so a
  container member and a VM member cannot share a group,
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
  (`VmlabInstance::os()`): linux = `--binary` /
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
- **In-instance protocol.** Two hidden subcommands on the copied
  binary: `__gather <dir> <pkg.gatherer> [--params-json …]` prints
  `{"ok":…,"value"|"error":…}`; `__verify <script> [--facts <json>]`
  compiles the script against the host API and runs
  `verify(facts) -> bool` (or `Result[bool, string]`), exit 0/1/2 =
  pass/fail/broken. Verify scripts compile during stage-5 validation
  but only ever execute inside instances.
- **Instance selection.** The declared `image`/`template` is the whole
  choice — modelled as `model::TestTarget::{Container,Vm}`, which carries
  the ref and prints as `container debian:12` / `vm x86_64/ubuntu-24.04`
  in reports and events. `--image REF` / `--template REF` override the
  ref for tests of the matching kind only; neither can convert a test
  between kinds, because the two refs name entirely different things.
  `cmd_test` probes vmlab once, up front, so a broken environment is
  exit 2 before any test runs. There is no backend trait any more:
  `VmlabBackend`/`VmlabInstance`/`VmlabLab` are used concretely and
  instances report a `GuestOs` the runner derives paths/shell/binary from.
- **Concurrency.** `runner::run_groups` runs independent groups in
  parallel via scoped `std::thread` workers pulling from per-kind
  cursors — **separate caps per kind** because VMs cost far more than
  containers: `--container-jobs` (default `min(cpu, 8)`) and `--vm-jobs`
  (default 2). Total live instances ≤ container_cap + vm_cap. Within a
  group tests stay sequential (shared state). `--jobs` is unchanged — the
  in-instance engine pool, still forwarded as `--jobs` to each
  check/apply run. Provision/smoke failure errors every test in the
  group; a single test's transport trouble errors only that test and the
  rest of the group proceeds.
- **Why vmlab only.** Docker bought fast linux containers at the cost of
  a second host dependency and a permanently weaker guest: an
  unprivileged container has no `NET_ADMIN`, no live init and no kernel
  of its own, which is why whole resource families (nftables/ufw/
  firewalld, OpenRC, runit, snap, sysvinit `service_state`) were marked
  "untested by design" in the stdlib. vmlab's `container {}` block runs
  the *same* OCI images in a micro-VM — measured ~2-4s to ready with the
  image cached, full capability set, own kernel — so dropping docker cost
  no speed and lifted every one of those exclusions. Verified on the
  guest: `CapEff` is the full set, `id -u` is 0, virtiofs preserves the
  binary's executable bit, and the rootfs is writable.
- **The backend.** CLI discovery `$CONFIG_WEAVE_VMLAB_CMD` → `vmlab`
  (probed with `--version`). Each provision writes a one-machine lab into
  a tempdir whose unique name is the lab name (`cw-test-…`) and runs
  `vmlab up` there. Teardown = `vmlab destroy` + tempdir removal;
  `--keep` leaves the lab up and reports its directory so `vmlab exec` /
  `vmlab container exec` / `vmlab console` work post-mortem. A group
  provisions **one** instance and runs all its tests inside it
  sequentially (the big win — boot is paid once per group, not per test).
- **VM instances (`template`).** `vm "box" { template, nic { nat = true } }`,
  template defaults for sizing. Readiness **polls** `vmlab osinfo box`
  until the guest agent answers (up to 300s, 3s between tries); the poll
  is required because `vmlab up` only blocks on agent readiness for VMs
  something *depends on*, and this lab's single VM has no dependents, so
  a slow (Windows) boot would otherwise hit osinfo's own 30s agent wait.
  `osinfo`'s `id` picks the protocol: **`windows`** (what the vmlab agent
  reports) or `mswindows` (what the QEMU guest agent reported before
  vmlab replaced it, still accepted) → windows, anything else → linux.
  Getting this wrong is silent and nasty — it copies the linux binary
  into a windows guest, which fails as "not a valid Win32 application" —
  so `guest_os` is unit-tested. exec = `vmlab exec --timeout 3600 box --
  …` (the CLI propagates the guest exit code); copy = `vmlab cp src
  box:dest`, with `src` canonicalized to an absolute path first since
  vmlab verbs run with the lab tempdir as cwd. Windows guests need the
  guest agent in the template and `setup` written for `cmd /C`.
- **Container instances (`image`).** `container "box" { image, mode = :idle,
  user = "0:0", nic { nat = true }, volume { host = "./payload" target =
  "/weave" } }`. `mode = :idle` is what `--entrypoint sleep` used to be —
  the instance exists to be exec'd into, not to run the image's own
  process — and `user = "0:0"` forces root for images that default
  otherwise (mssql). exec = `vmlab container exec box --timeout 3600 --
  …`. **copy_in is a host-side file write**, not a guest transfer:
  everything the runner copies lives under `/weave` (see `GuestPaths`),
  so the bind-mounted `payload/` directory *is* the guest's `/weave`,
  modes included. That also sidesteps vmlab's `cp`/`osinfo` being
  VM-only verbs (`lab.vm()` rejects a container by name), so the testlab
  needs no change in vmlab. Readiness probes a trivial `exec` rather than
  `osinfo`, and the guest is linux by construction.
- **Choosing one.** `image` for anything that is really just a userland
  (file/package/config resources) — seconds to run. `template` when the
  test needs a real init system, its own kernel, a reboot, or a windows
  guest. Three measured gotchas. `dnf5` loads its repositories fine in a
  Fedora *container* but then wedges for many minutes on the transaction
  itself (the same command in a Fedora VM takes seconds). The
  `x86_64/debian-13` template in the local store has no working vmlab
  agent — use `x86_64/ubuntu-24.04` for apt-family VM tests. And the
  guest agent runs execs with **`HOME=/`**, not the target user's home
  (docker exec used to set `HOME=/root`), so a resource that defaults a
  path to `$HOME` writes somewhere unexpected — tests should pass `home`
  (or the equivalent) explicitly instead of leaning on the ambient
  environment. That one cost a green `linux_ssh` run.
- **Reporting.** Exit 0 = all passed, 1 = any failed/error, 2 =
  validation/environment. `--json` emits a schema-stable object with
  `mode: "test"`; the runner parses in-instance reports with the same
  `JsonRunReport` types that produce them.
- **Scenarios (scripted, multi-stage, over a declared vmlab lab).** The
  three-run protocol can't reboot or network multiple machines, which a
  Windows DC promotion (apply → reboot → apply) and a member join both
  need. A package declares a `scenario { lab, script }`: `lab` is a dir
  holding a `vmlab.wcl` (the full vmlab feature set — segments, static
  IPs, DC-as-DNS, depends_on), and `script` is a driver
  (`fn run(lab: Lab) -> bool`/`Result[bool,string]`) that runs
  **host-side** against the live lab via the `testlab` wscript host module
  (`src/hostapi/testlab.rs`): `Lab`/`Machine` opaque handles over
  `VmlabLab`/`VmlabInstance`. The handles hold `Rc<RefCell<LabState>>`
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
  boot). `VmlabInstance::reboot`/`wait_ready` and
  `VmlabBackend::open_lab`/`VmlabLab` carry this; the single-machine `box`
  path is unchanged (its instance owns lab teardown, lab machines don't).
  Scenario machines are always VMs — the author's own `vmlab.wcl` declares
  them, and reboots and multi-machine topologies are the point. Scenarios
  compile in stage-5 against `hostapi::scenario_context()` (host API +
  `testlab`), so `validate` catches a broken driver; at run time they
  execute sequentially after the parallel test groups, each owning its lab.
  `windows_domain:ad_matrix` is the first: forest root (DNS) → member join
  → additional DC → second forest (own segment), all over real reboots.
  The two-VM + reboot integration is smoke-verified on vmlab with Alpine.

## wscript binding (PRD §6/§7)

- **File extension: `.ws`.** Resource, gatherer, verify, scenario and
  `lib/` scripts are all `*.ws`, matching vmlab (the other wscript host).
  The PRD writes `.wscript`; that spelling is superseded. Only `lib/` scans
  the extension (`engine/scripts.rs`) — every other path is a literal
  `script =` / `verify =` string in WCL, so the extension is convention,
  not contract. The two *interface* names keep their upstream wscript-cli
  spelling and are **not** renamed: `weave.wscripti` and `wscript.toml`.
- Script entry points accept two signatures each: plain
  (`fn check(params: Value) -> CheckResult`) or fallible
  (`-> Result[CheckResult, string]`), because `?` requires a `Result`
  return. An `Err` maps to the step's Error status, per the PRD.
- **Script-to-script imports (2026-07-28).** wscript shipped them, so
  `lib/` folders became real resolution roots instead of the standalone
  compile-only lint they were — the degradation the PRD's risk table
  anticipated is retired. `ctx.compile_entry(path, src, &resolver)`
  replaces `ctx.compile(src)` everywhere; the whole import graph becomes
  one `CompiledUnit` of which only the entry file exports functions, so
  the entry-point contracts are unchanged.
  - **`WeaveResolver`, not wscript's `FsResolver`.** The upstream one
    resolves a bare `use name` to `{name}.wscript`; this repo standardised
    on `.ws`, so resolution is re-implemented over that extension.
    Everything else matches upstream: a registered host module wins over a
    file (so `use fs` still means the host API even with a `lib/fs.ws`
    present), then the importing file's own directory, then each root.
    Roots are the declaring package's `lib/` then the playbook's, per
    PRD §6.
  - **Diagnostics go through the source map.** Spans from a multi-file
    compilation are offsets into a virtual space covering every file, so
    `Diag::from_wscript` routes each through `CompileFailure::source_map`
    to pick the owning file and rebases the span local to it. Without that
    an error inside a helper renders against the *importing* file's text.
    Secondary labels landing in another file are dropped — a miette report
    carries one source.
  - **Three compile sites had to move together**, or a script would
    validate on the host and then fail to resolve its import at run time:
    stage-5 validation, scenario drivers (host-side), and `__verify`
    (inside an instance, with no playbook model — so
    `WeaveResolver::for_script` rediscovers roots by walking ancestors for
    `lib/` dirs). For the same reason the testlab now copies the
    playbook's `lib/` into every synthesized playbook; package dirs were
    already copied whole, so their helpers already rode along.
- `print`/`println` in wscript-vm write directly to stdout; routing them
  into `log::info` needs a small upstream hook in wscript-vm (planned with
  M3's stdout-redirection work).

## The weave-docjson crate (docjson/)

Structural DocJson extraction and AST-preserving round-tripping for
playbook.wcl / package.wcl live in the shared workspace crate `docjson/`
(`weave-docjson`: docjson + inspect_ast + emit, wcl_lang-only deps). The
CLI keeps its `model::docjson` paths via re-exports in `src/model/mod.rs`,
and `just test` runs the crate's suite explicitly since `default-members`
would skip it. Extraction (`extract_package`/`extract_playbook`) works on
a `parse_for_edit` AST — every leaf is `{lit}` or `{expr: "source"}` —
and **fails closed** on constructs forms can't represent; `emit` syncs a
doc back onto the current file's AST (blocks matched by `_orig`-or-name,
comments and unknown items survive) and re-parses the output before it
can reach disk. The hidden `__wcl-inspect` / `__wcl-render` /
`__templates` subcommands expose this over stdin/stdout JSON for external
tooling.

## Removed post-v1 components

The web GUI (`weave-server` + SolidJS `web-ui/`), the `config-weave-pipeline`
CI/CD daemon, and their shared `weave-remote` ssh/winrm transport crate
were removed in July 2026 to refocus the project on the CLI tools. Their
implementation notes went with them — the code and the old sections of
this file are in git history (last present at commit 775b46e).
