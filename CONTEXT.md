# Domain glossary

The words this codebase uses, and what they mean here. Use these exactly;
where a term has a tempting synonym, the synonym is listed as *avoid* —
consistency is the point.

This file is grown lazily, as terms actually get pinned down. It is not a
map of the whole system: `docs/PRD.md` is the source of truth for scope
and design, and `docs/notes.md` records how the PRD's sketches were bound
to the real WCL, wscript and testlab APIs.

## The testlab

**Testlab** — `config-weave test`. Runs a package's convergence tests in
disposable vmlab instances. Lives in `src/testlab/`.

**Instance** — one provisioned vmlab machine: a *container* (an OCI image
booted in a micro-VM; linux, seconds) or a *VM* (cloned from a template;
linux or windows, real init, survives reboots). A test declares `image`
or `template` and that choice is the whole of it. _Avoid_: box, node,
machine (see **Machine**, which means something narrower).

**Guest** — an instance seen from the host, bound to the operating system
running inside it. The unit everything in `testlab::guest` is expressed
against. "Guest" is always the inside of an instance; the process driving
it is the **host**.

**Transport** — the seam between the host and a guest: guest OS, one
exec, one copy-in, and nothing else. A live `VmlabInstance` is one
adapter; a scripted fake in the guest module's tests is the other.
Deliberately narrower than what an instance can do, so everything above
it is testable without a hypervisor. See
`docs/adr/0001-testlab-transport-seam.md`. _Avoid_: backend — that named
the removed docker-vs-vmlab choice and means something else here.

**Working directory** (`Workdir`) — a directory inside a guest that
exists, holding one test's or one scenario apply's playbook and facts.
Obtained from a **Guest**, which creates it in the act of handing one
back, so a working directory you hold is always one you can write to.
Every guest path is built here and nowhere else.

**Machine** — in a **scenario**, one named member of an author-declared
lab, reached from a script as `lab.machine("dc1")`. Always a VM.

**Group** — the set of tests that share one instance, because they
declared the same `group`. They run sequentially inside it; different
groups run in parallel under per-kind caps.

**Scenario** — a wscript-driven multi-machine flow
(`fn run(lab: Lab) -> …`) that provisions machines, applies config-weave
inside them, reboots, and asserts. Distinct from a **test**, which is
declarative and single-instance.

**The three-run protocol** — check → apply → apply, the sequence every
test's steps go through. The first apply proves convergence within one
process; the second proves *cross-process idempotence* and that re-apply
is a true no-op. Expectations are asserted per run against the table in
`testlab::runner::expectations`.

**Refusal** — a gatherer or verify script answering "no". Distinct from
an error, which is the transport or the protocol breaking. The two
callers disagree about whether a refusal is fatal, so the guest module
returns it as data and decides nothing.
