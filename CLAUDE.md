# CLAUDE.md

Project context for Claude Code.

## Project Purpose

**config-weave** is a configuration management tool. This is a fresh
rewrite; the product requirements live in `docs/PRD.md` — read that first;
it is the source of truth for design and scope.

Earlier attempts are archived under github.com/wiltaylor/.graveyard-private
(`config-weave`, `config-weave-old`, `configweave-zig`, `config-weave-2`).
The most recent (`config-weave-2`) was a single-binary Rust tool driven by
WCL playbooks with wscript check/apply scripts. Consult them for prior art
only — the PRD overrides anything they did.

## Status

v1 complete: all seven PRD milestones (M1–M7) implemented and tested —
validation pipeline, sequential + parallel execution with concurrency
classes, full host API (Linux + Windows modules), three output modes with
NDJSON file logging, and authoring/docs (`wscripti`, `init`, `docs`).
Post-v1: `config-weave test` (the testlab, `src/testlab/`) runs package
convergence tests in disposable vmlab instances (shelling out to the
sibling `../vmlab` CLI) — a `test` block declares either `image` (an OCI
ref, run as a vmlab container: linux, seconds) or `template` (a vmlab
template ref, run as a full VM: linux or windows, real init, reboots).
vmlab is the only backend; the docker/podman one was removed 2026-07-26.
Plus a three-run idempotence protocol, `just test-lab` for the
vmlab-gated suite, and `just test-lab-vm` for a full-VM smoke. `config-weave docs` renders a static
wdoc site from the playbook/package metadata (emits `_weave_docs.wcl`,
shells out to `wcl wdoc build`; `--serve` hands off to `wcl wdoc serve`)
— the sibling `../config-weave-pkgs` stdlib repo uses it for its package
docs. The DocJson pipeline (structural package/playbook extraction and
AST-preserving round-tripping) lives in the `docjson/` crate
(`weave-docjson`), re-exported through `src/model/mod.rs`.
A web GUI (`weave-server`) and CI/CD daemon (`config-weave-pipeline`)
were built and later removed to refocus on the CLI — see git history
before 2026-07 for that code.
`config-weave secrets` (`src/secrets/`) encrypts values in place: an
author writes `secret("plaintext")` anywhere an expression is legal,
`secrets encrypt|decrypt|rekey` rewrites the call's byte span in
`playbook.wcl` (Argon2id + XChaCha20-Poly1305, `CWENC1` blobs), and
check/apply/test decrypt with a password from `$CONFIG_WEAVE_PASSWORD` /
`--password-stdin` / `--password-file`. An un-encrypted `secret()` fails
validation; decrypted values are scrubbed from all output.
A **composite** is a named, parameterised block of steps declared in a
package or a playbook and invoked from a step like a resource. Its `arg`
declarations bind into the body bare and as `args.name`; the loader
expands an invocation statically into a synthetic container of real steps,
so the DAG, planner and every report shape see ordinary steps
(`container/…/invocation/inner`). A body sees only its own arguments.
The **built-in `weave` package** ships inside the binary (`src/builtin/`,
reserved name): `weave.execute` pairs a guard script (exit 0 = already
converged) with an action, and the re-check enforces convergence;
`weave.execute_once` runs a script once per host and records it under
`/var/lib/config-weave/once/` (Windows: `HKLM\Software\config-weave\Once`,
override with `$CONFIG_WEAVE_STATE_DIR`) — the only persistent state
config-weave owns, a deliberate PRD §17 exception scoped as a migration
aid. `docs/notes.md` records the bindings for both.
`config-weave pkg` (`src/pkgrepo/`) installs packages from git repos:
`pkgs/repo.wcl` records registered repos + installed packages with
their source commit; add/remove/update/search plus `pkg repo
add/remove/list` shell out to the `git` binary (private repos work via
ambient credentials), caching shallow clones in `.repo-cache/`.
`docs/notes.md` records how the PRD's illustrative
sketches were bound to the real WCL and wscript APIs, plus the testlab's
bindings — read it before changing the vocabulary,
the variable scheme, the host API surface, or the test protocol.

## Layout

Binary crate per PRD §14: `model/` (WCL loading + schema validation),
`engine/` (gatherers, DAG scheduler, worker pool, lifecycle), `hostapi/`
(wscript host modules; Windows impls behind cfg), `comdispatch/` (IDispatch +
VARIANT marshalling), `docsgen`, `scaffold` (wscripti/init), `vocab/` (the
embedded WCL schema served as system imports).

`wcl_lang`, `wscript` and `wscript-std` are **GitHub git dependencies**, not
sibling path deps — declared once in `[workspace.dependencies]` so the CLI and
`docjson/` cannot disagree about which `wcl_lang` they build against. The commit
is pinned by `Cargo.lock`; bump it deliberately with `cargo update -p wcl_lang`
(or `-p wscript`). No sibling checkout is needed to build, which is what lets a
ticket worktree (`.tree/<ticket-id>`, where a relative `../` path resolves to
nothing) build at all. To develop across repos, put a `[patch]` section in a
**gitignored** `.cargo/config.toml` pointing those git URLs at local paths.

## Conventions

- Ticket-branch development, driven by the aciddog kanban board: work happens
  on a branch named for the ticket id (`t-…`) in that ticket's worktree at
  `.tree/<ticket-id>`, and lands on `main` through a pull request. Never commit
  or push directly to `main` — the board's Tests and Review stages gate every
  change, and a direct push bypasses them.
- **just** as command runner: `just build` / `just test` / `just check` /
  `just release` (cross-builds both PRD targets + checksums).
- Releases are trailer-gated in CI (same scheme as WCL/vmlab): land a commit on
  `main` carrying a `pre-release: true` (→ vX.Y.Z-alpha) or `release: true`
  (→ vX.Y.Z) trailer; CI bumps from the last tag by conventional commits,
  cross-builds via `just release`, tags, and publishes a GitHub release. CI
  reads the trailer off the head commit of the push, so when merging a release
  PR the trailer has to be in the commit that actually lands — put it in the
  squash-commit message, not only in an intermediate branch commit.
