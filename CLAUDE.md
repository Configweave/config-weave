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
convergence tests in disposable instances — docker containers (linux) or
vmlab VMs (linux + windows guests, shelling out to the sibling `../vmlab`
CLI) — with `test` blocks in package.wcl, a three-run idempotence
protocol, `just test-lab` for the docker-gated suite, and `just
test-lab-vm` for a vmlab smoke. `config-weave docs` renders a static
wdoc site from the playbook/package metadata (emits `_weave_docs.wcl`,
shells out to `wcl wdoc build`; `--serve` hands off to `wcl wdoc serve`)
— the sibling `../config-weave-pkgs` stdlib repo uses it for its package
docs. The DocJson pipeline (structural package/playbook extraction and
AST-preserving round-tripping) lives in the `docjson/` crate
(`weave-docjson`), re-exported through `src/model/mod.rs`.
A web GUI (`weave-server`) and CI/CD daemon (`config-weave-pipeline`)
were built and later removed to refocus on the CLI — see git history
before 2026-07 for that code.
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
embedded WCL schema served as system imports). Path deps on `../WCL` and
`../wscript`; `Cross.toml` mounts them for `cross` release builds.

## Conventions

- Trunk-based development: commit directly to `main`, no branches or PRs
  unless explicitly asked.
- **just** as command runner: `just build` / `just test` / `just check` /
  `just release` (cross-builds both PRD targets + checksums).
- Releases are trailer-gated in CI (same scheme as WCL/vmlab): push a
  commit to `main` with a `pre-release: true` (→ vX.Y.Z-alpha) or
  `release: true` (→ vX.Y.Z) trailer; CI bumps from the last tag by
  conventional commits, cross-builds via `just release`, tags, and
  publishes a GitHub release.
