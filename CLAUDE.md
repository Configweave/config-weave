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

Scaffold only. Language and structure will be shaped by the PRD once it
lands.

## Conventions

- Trunk-based development: commit directly to `main`, no branches or PRs
  unless explicitly asked.
- **just** as command runner once there's something to build.
