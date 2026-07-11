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
test-lab-vm` for a vmlab smoke. `weave-server` (`server/` + SolidJS
`web-ui/`, on the sibling `../forge` stack) is the web GUI: runbook
browsing/editing with visual playbook/package editors (DocJson pipeline
in `src/model/{docjson,inspect_ast,emit}.rs`), a systems inventory
(`systems.wcl`, ssh/winrm deployment of direct systems, remote systems
via injected `system_*` vars), a package repository (`--packages-dir`),
live test/system runs with docker-terminal/VNC debugging, and
per-service Monitoring/Logs tabs backed by an optional Prometheus + Loki
pair (`--prometheus-url`/`--loki-url`; `just stack-up` runs the compose
test stack).
`docs/notes.md` records how the PRD's illustrative
sketches were bound to the real WCL and wscript APIs, plus the testlab's
and weave-server's bindings — read it before changing the vocabulary,
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
