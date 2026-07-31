# 0001 — A transport seam under the testlab's guest handling

**Status:** accepted (2026-07-31)

## Context

The testlab talks to a guest from two places. `testlab::runner` drives a
disposable instance through the three-run protocol. `hostapi::testlab` —
the `testlab` wscript host module — drives several machines on behalf of a
scenario script. Both need the same facts: where the config-weave binary
lives inside a guest, which shell runs a script there, how config-weave is
invoked, and how its output is read back.

Both had their own copy of those facts. `mkdir_guest` and the runner's
per-test mkdir were identical down to the `if not exist … md …` string;
`ensure_prepared` and `prepare_instance` differed only in error wording;
`run_in_guest` and the three-run driver parsed the same report the same
way. Three guest roots were hardcoded in three files.

The cost was not the duplication itself but where it left the test
surface. Everything guest-shaped was reachable only through the
`#[ignore]`-gated suite (`just test-lab`), which needs vmlab and KVM and
**self-skips vacuously without them**. The Windows branches were worse:
reachable only through `just test-lab-vm`, which is manual and has never
run in CI. `hostapi::testlab` — 520 lines — had no unit tests at all.

That is the same class of gap that let Windows guest detection stay
silently broken for months until 2f60bf5 found it: a platform branch
nobody executes.

## Decision

Introduce `testlab::backend::Transport` — guest OS, one exec, one copy-in
— and put the shared guest handling in `testlab::guest` above it.

A live `VmlabInstance` is one adapter. A scripted fake in the guest
module's tests is the other: it records the argv it was handed and
replays canned outputs. Everything above the seam is then ordinary logic
that `cargo test` exercises on a host that has never seen a hypervisor.

## Why this is not the backend trait removed in 2f60bf5

2f60bf5 deleted `TestBackend`/`TestInstance`/`TestLab` when the
docker/podman backend went away, and `docs/notes.md` records the outcome:
*"There is no backend trait any more."* Re-adding a trait in this
subsystem needs justifying against that, and the difference is where the
seam sits and what varies across it.

- **The removed seam was at provisioning**, and it leaked. Two of its
  eight methods were documented as unsupported on one adapter — `reboot`
  "docker returns an error", `wait_ready` "docker is always ready (a
  no-op)". Callers had to know which backend they were on, which is what
  makes a seam a liability rather than an asset.
- **This seam is at exec/copy-in/os.** Every instance vmlab hands back
  supports all three identically, whether it is a container or a full VM
  — `VmlabInstance` already branches internally on that and presents one
  behaviour. There is no capability to interrogate and no adapter that
  partially satisfies the interface.
- **What varies across it is real.** The removed seam ended up with one
  production adapter, which is what made it dead weight. This one has two
  from the day it lands: the vmlab machine and the test fake. That is the
  variation the seam exists to serve, and it is exercised on every
  `cargo test`.

The container-versus-VM distinction — the one place the testlab genuinely
has two behaviours — is deliberately **not** modelled here. It stays
inside `VmlabInstance`, which resolves it per method, because it is a
property of one adapter rather than a choice a caller makes.

## Consequences

- The path scheme, shell selection, argv framing, report parsing and
  error wording become unit-testable, including every Windows branch.
- `testlab::guest` is the only module that may name a guest path. The
  rule that a container instance exposes only `/weave/…` to the host
  stops being a runtime error and becomes unreachable by construction.
- One indirection is added where the concrete type used to be called
  directly. `VmlabInstance`'s inherent methods stay, and the trait impl
  delegates to them, so code that does not need the seam is unaffected.
- A future reviewer counting adapters will see one production
  implementation. That is expected — the second is `#[cfg(test)]`. Do not
  remove the trait on that basis without also giving up in-process
  coverage of the guest rules.

## Alternatives considered

- **Deduplicate without a seam.** Removes the duplication, honours
  2f60bf5 exactly, and leaves the testability claim unmet: the Windows
  branches stay behind a manual full-VM run.
- **Split pure from impure, no trait.** Extract argv building, the path
  scheme and exit triage as free functions and test those directly. Wins
  the argv coverage but not the sequencing coverage, and leaves the
  mkdir → stage → run ordering untested.
