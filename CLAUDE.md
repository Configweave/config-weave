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
NDJSON file logging, and authoring/docs (`wispi`, `init`, `docs`).
`docs/notes.md` records how the PRD's illustrative sketches were bound to
the real WCL and wisp APIs — read it before changing the vocabulary, the
variable scheme, or the host API surface.

## Layout

Binary crate per PRD §14: `model/` (WCL loading + schema validation),
`engine/` (gatherers, DAG scheduler, worker pool, lifecycle), `hostapi/`
(wisp host modules; Windows impls behind cfg), `comdispatch/` (IDispatch +
VARIANT marshalling), `docsgen`, `scaffold` (wispi/init), `vocab/` (the
embedded WCL schema served as system imports). Path deps on `../WCL` and
`../wisp`; `Cross.toml` mounts them for `cross` release builds.

## Conventions

- Trunk-based development: commit directly to `main`, no branches or PRs
  unless explicitly asked.
- **just** as command runner: `just build` / `just test` / `just check` /
  `just release` (cross-builds both PRD targets + checksums).
