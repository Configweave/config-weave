# config-weave

Single-binary configuration management. Copy the binary onto a target
machine alongside a playbook folder and run it — no agent, no runtime, no
package installs. A playbook describes the desired state of a system;
config-weave can **check** whether the machine matches (a report-only dry
run) or **apply** it (converge the machine to the playbook).

Three languages divide the work:

- **WCL** encodes playbooks: plays, steps, variables, conditions,
  gatherer invocations and resource/gatherer declarations. WCL never
  executes against the system.
- **wisp** implements gatherers and resources: the scripts that inspect
  and mutate the machine, through a host API registered by config-weave.
- **config-weave** (Rust) mediates. WCL and wisp never interact directly.

See `docs/PRD.md` for the full design and `docs/notes.md` for the
decisions made while binding the PRD to the real WCL and wisp APIs.

## Quick start

```sh
config-weave init my-playbook     # scaffold a working skeleton
config-weave validate my-playbook # parse, schema-check, compile scripts
config-weave list my-playbook     # list plays
config-weave check my-playbook baseline   # dry run (never mutates)
config-weave apply my-playbook baseline   # converge
config-weave docs my-playbook     # render a wdoc site to my-playbook/docs/
config-weave wispi                # emit .wispi for LSP/wisp-check support
```

Everything validates before anything runs: WCL parses, parameters
schema-check, and every wisp script compiles and type-checks against the
host API before the first script executes. Exit codes: 0 success, 1 step
error, 2 validation failure, 3 reboot required.

## Playbook layout

```
my-playbook/
  playbook.wcl              # plays, variables, gatherer invocations
  lib/                      # shared wisp code (compiled at validate time)
  pkgs/<name>/
    package.wcl             # gatherer + resource declarations (schemas)
    resources/<r>.wisp      # exports check() and apply()
    gatherers/<g>.wisp      # exports gather()
```

## Building

`just build` for a debug build, `just test` for the suite. Release
artifacts for both targets (`x86_64-unknown-linux-musl`,
`x86_64-pc-windows-gnu`) cross-build from Linux with `just release`
(requires `cross` and a container runtime; see `Cross.toml`).

The `../WCL` and `../wisp` sibling checkouts are path dependencies.

Previous iterations are archived in the private graveyard
(`config-weave`, `config-weave-old`, `configweave-zig`, `config-weave-2`).
