# CLI reference

`config-weave` — single-binary configuration management.

## Subcommands

| Command | Args | Description |
|---|---|---|
| `check` | `<playbook-dir> <play>` | Report configuration status of all steps (never mutates) |
| `apply` | `<playbook-dir> <play>` | Apply all unconfigured steps in a play |
| `list` | `<playbook-dir>` | List all plays defined in the playbook |
| `validate` | `<playbook-dir>` | Full validation pipeline (WCL syntax, schema, refs, DAG, wscript compilation of every script), no execution |
| `test` | `<playbook-dir> [filter]` | Run package convergence tests in disposable instances (docker containers or vmlab VMs); filter is `pkg` or `pkg:test` (see `testing.md`) |
| `docs` | `<playbook-dir> [outdir]` | Generate wdoc documentation (default outdir `<dir>/docs/`); shares the validation pipeline |
| `wscripti` | `[outdir]` | Emit `weave.wscripti` (host API interface) plus a starter `wscript.toml` (default: cwd) |
| `init` | `<dir>` | Scaffold a skeleton playbook with example package, resource and gatherer |
| `version` | | Print version information |

Hidden internal subcommands `__gather` and `__verify` implement the in-container test
protocol (`testing.md`).

## Global flags (all commands)

| Flag | Meaning |
|---|---|
| `--var KEY=VALUE` | Override a playbook variable. Repeatable. VALUE parses as a WCL expression when possible (`--var count=3` is an int), else a plain string |
| `--var-file PATH` | Merge a WCL file's top-level `name = value` pairs into scope (expressions evaluate standalone; cannot reference other variables) |
| `--jobs N` | Worker pool size (default `min(cpu_count, 8)`); forwarded into test containers |
| `--continue-on-error` | Continue dispatching steps after an Error (check/apply) |
| `--json` | JSON output: a single schema-stable object on stdout at completion; script log output goes to file log/terminal only, never stdout |
| `--no-color` | Plain ASCII output (auto-selected when stdout is not a TTY) |
| `--log-file PATH` | Enable NDJSON file logging (independent of terminal mode) |
| `--log-level LEVEL` | File log level (independent of terminal verbosity), default `info` |
| `-v, --verbose` | Increase terminal verbosity (repeatable: `-v`, `-vv`, `-vvv`) |

Test-only flags (`--backend`, `--image`, `--keep`, `--binary`, `--binary-windows`)
are in `testing.md`.

## Output modes

1. **Rich** (default on a TTY) — ANSI colour, Unicode icons, live progress, per-step timing.
2. **Plain** (`--no-color` or non-TTY) — ASCII, line-oriented.
3. **JSON** (`--json`) — single object on stdout with playbook metadata, per-step
   status/timing/messages, exit status. Test runs emit `mode: "test"`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All steps configured (apply) / reported (check) without error; all tests passed |
| 1 | One or more steps in Error; any test failed |
| 2 | Validation failure (or test environment problem) |
| 3 | Reboot required (apply-only; halts the play) |

## Step report statuses

`already configured`, `configured` (apply changed it, re-check confirmed),
`not configured` (check mode), `skipped` (condition false), `error`,
`reboot required`, plus `not run` for steps left undispatched when a run halts.

## Repo workflow (just)

`just build` · `just test` · `just check` (clippy + fmt) · `just sample` (validate the
sample playbook) · `just test-lab` (docker-gated suite) · `just test-lab-vm` (vmlab
end-to-end smoke) · `just release` (cross-builds linux musl + windows gnu targets,
writes `dist/` + SHA256SUMS).
