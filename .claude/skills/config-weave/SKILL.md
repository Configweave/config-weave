---
name: config-weave
description: >-
  Expertise skill for config-weave: authoring WCL playbooks and packages, writing wscript
  resource/gatherer/verify scripts, the host API surface, running and testing playbooks.
  Auto-activated when working with playbook.wcl, package.wcl, .wscript scripts, the
  config-weave CLI, or the testlab.
user-invocable: false
allowed-tools:
  - Read
  - Write
  - Edit
  - Glob
  - Grep
  - Bash
  - Agent
---

<overview>
config-weave is a single-binary configuration management tool. Playbooks are WCL
documents (`playbook.wcl`) whose plays run **steps**; each step invokes a **resource**
declared in a package (`pkgs/<name>/package.wcl`) and implemented as a wscript script with
a check → apply → re-check convergence contract. **Gatherers** collect facts into
playbook variables. The testlab (`config-weave test`) proves package convergence in
disposable docker containers with a three-run protocol.

This skill provides reference files for every layer. Read only the file(s) the task
needs.
</overview>

<variables>
- `${CLAUDE_SKILL_DIR}`: Path to this skill's directory (contains `reference/`).
</variables>

<reference-files>
| Task | Read |
|---|---|
| Write/edit `playbook.wcl` (plays, steps, vars, gather, conditions, requires) | `${CLAUDE_SKILL_DIR}/reference/playbooks.md` |
| Write/edit `package.wcl` (resources, gatherers, params, concurrency, layout) | `${CLAUDE_SKILL_DIR}/reference/packages.md` |
| Write resource/gatherer/verify wscript scripts (contracts, lifecycle, params) | `${CLAUDE_SKILL_DIR}/reference/scripts.md` |
| Add or run package tests (`test` blocks, three-run protocol, `just test-lab`) | `${CLAUDE_SKILL_DIR}/reference/testing.md` |
| Run the tool (subcommands, flags, output modes, exit codes) | `${CLAUDE_SKILL_DIR}/reference/cli.md` |
| wscript language syntax and semantics | `${CLAUDE_SKILL_DIR}/reference/wscript-language.md` |
| wscript built-ins: prelude, string/List/Map methods, Value, json/toml | `${CLAUDE_SKILL_DIR}/reference/wscript-stdlib.md` |
| Host API signatures: log, fs, path, shell, http, hash, archive, env, sys, data | `${CLAUDE_SKILL_DIR}/reference/hostapi.md` |
| Windows host API: registry, service, com/WMI | `${CLAUDE_SKILL_DIR}/reference/hostapi-windows.md` |
</reference-files>

<quick-facts>
- Layout: `playbook.wcl` + `pkgs/<pkg>/{package.wcl, resources/*.wscript, gatherers/*.wscript, tests/*.wscript}`.
- Core commands: `config-weave validate <dir>` · `check|apply <dir> <play>` ·
  `test <dir> [pkg[:test]]` · `init <dir>` · `wscripti` (emit editor interface).
- Contract: `check` never mutates; after a successful `apply`, a re-check must return
  `AlreadyConfigured` — including in a fresh process (the testlab's third run).
- Playbook refs are qualified `package.resource` / `package.gatherer`; inside `test`
  blocks unqualified names resolve to the declaring package and values must be static.
- WCL has `$"${var}"` interpolation; wscript does not (use `fmt()`).
- After any change: `just check` (clippy + fmt) and `just test`; docker-gated suite via
  `just test-lab`.
</quick-facts>

<boundaries>
<always>
- Read `docs/notes.md` before changing the WCL vocabulary, variable scheme, host API
  surface, or test protocol — it is the binding source of truth over the PRD's sketches.
- Trust real source over these references if they disagree: `src/vocab/*.wcl`,
  `src/hostapi/*.rs`, `src/main.rs`, `~/dev/wscript/docs/` — then update the reference file.
- Regenerate `weave.wscripti` (`config-weave wscripti`) after changing the host API, and
  update `reference/hostapi*.md` to match.
</always>
<ask>
- Before running `config-weave apply` against the local machine (it mutates system
  state) — `validate`, `check`, and `test` are safe.
- Before adding new fields to the WCL vocabulary (`src/vocab/*.wcl`) — that is a
  schema change, not playbook authoring.
</ask>
<never>
- Invent host API functions or modules not listed in the hostapi references — the
  surface is exactly what `config-weave wscripti` emits.
- Use wscript-std's `math`/`process`/`xml`/standalone-`fs` in playbook scripts — they are
  not registered (see `reference/wscript-stdlib.md`).
- Write WCL `import` lines in playbooks or packages — the engine appends system imports.
</never>
</boundaries>
