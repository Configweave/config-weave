---
name: config-weave
description: "Read when writing or fixing a playbook.wcl, a package.wcl, a wscript resource, gatherer or verify script (.ws), a test or scenario block, or when running config-weave (validate, check, apply, test, secrets, pkg, wscripti). Carries the complete block, host-API and CLI surface, so nothing needs guessing."
allowed-tools:
  - Read
  - Write
  - Edit
  - Glob
  - Grep
  - Bash
  - Agent
---

# config-weave

<overview>
config-weave is a single static binary that reads a WCL playbook and either
checks a machine against it or applies it. A playbook's plays run steps; each
step invokes a resource declared in a package and implemented by a wscript
script. Every resource obeys one contract, **converge**: `check` reads the host
and never writes; `apply` changes the host so that a re-check, in the same run
and in a fresh process, reports already configured. The engine validates
everything before anything runs, then runs the re-check itself.
</overview>

<variables>
- `${CLAUDE_SKILL_DIR}`: this skill's directory. The reference files below are
  under `${CLAUDE_SKILL_DIR}/reference/`.
</variables>

<workflow>
<step order="1">
Pick the branch from the table below and read its reference file in full before
writing a line. The files are exhaustive on their topic: a block, field, function
or flag that is absent from them does not exist.
</step>

<step order="2">
Author against the reference. WCL blocks take exactly the fields in the tables.
Scripts call only functions listed in `host-api.md`, and each resource script
keeps `check` read-only and makes `apply` converge.
</step>

<step order="3">
Run `config-weave validate <playbook-dir>` and fix every diagnostic. Done when it
exits 0: the WCL, the schema, every reference, the step graph and every script's
compilation are all clean.
</step>

<step order="4">
Prove convergence rather than assuming it. For a play on a machine you may
change: `check`, then `apply`, then `apply` again, and the second apply reports
every step already configured. For a package: `config-weave test <playbook-dir>
<pkg>` or `<pkg>:<test>`, and every test passes all three runs. A step that
reports configured on the third run has an `apply` that does not satisfy its own
`check`.
</step>

<step order="5">
When a reference file and the code disagree, the code wins: `src/vocab/*.wcl`
for blocks, `src/hostapi/*.rs` and `config-weave wscripti` for the host API,
`src/main.rs` for the CLI. Correct the reference file in this skill in the same
change.
</step>
</workflow>

<reference>
| Task | Read |
|------|------|
| Write or fix `playbook.wcl`: plays, steps, containers, composites, variables, `secret()` | `reference/playbook.md` |
| Write or fix `package.wcl`: resources, gatherers, params, composites, the built-in `weave` package, `config-weave pkg` | `reference/package.md` |
| Write a resource, gatherer or verify script: the contract, entry points, `Value`, wscript essentials, prelude and methods | `reference/scripts.md` |
| Call the host from a script: every module, function, type and option map | `reference/host-api.md` |
| Run the CLI: every command and flag, output modes, exit codes, environment, troubleshooting | `reference/cli.md` |
| Write or run tests and scenarios: the `test` block, expectations, the three-run protocol, instances, the driver API | `reference/testlab.md` |
</reference>

<boundaries>
<always>
- Validate before check, check before apply, and read the re-check result: a
  step reported as Error after a successful apply is a contract violation in the
  resource, and the fix is in the script.
- Treat `host-api.md` as the whole host surface when writing a script. A function
  it lacks is one to implement another way, with the registered modules.
</always>

<ask>
- Before `config-weave apply` against the machine you are running on, since it
  changes system state. `validate`, `check` and `test` are safe to run freely.
- Before editing `src/vocab/*.wcl` or `src/hostapi/`: that changes the language
  every playbook and script is written in, which is engine development rather than
  authoring.
</ask>

<never>
- Write `import` lines in a playbook or package. The engine appends the system
  imports itself when it opens the file.
- Encrypt, decrypt or rekey secrets by editing the `CWENC1` blobs by hand. Use
  `config-weave secrets`, which rewrites the byte span in place.
</never>
</boundaries>
