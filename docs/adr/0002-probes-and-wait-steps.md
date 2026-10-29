# 0002 — Probes and wait steps

**Status:** accepted (2026-10-30)

## Context

A playbook sometimes has to wait for a system it does not own before it
can continue. A migration needs the database to accept connections. A
registration needs an API to answer. A join needs a file another host
writes. config-weave had no way to say this.

The workaround was a `weave.execute` step whose guard or action looped
with `sleep`. That works badly in four ways:

- The loop's budget is invisible to the engine. It cannot report progress,
  so a three-hour wait looks like a hang.
- The loop holds a pool worker for its whole life. With `--jobs 4`, four
  such steps stop every other step until one of them gives up.
- A halt cannot stop it. PRD §9 lets in-flight steps finish, so a run that
  has already failed stays open until the loop runs out.
- `check` either runs the loop, which is no longer a fast dry run, or the
  author hand-writes a separate guard.

## Decision

Add a **probe**: a declared wscript readiness check with the single entry
point `fn ready(params: Value) -> bool` (or `Result[bool, string]`). Add a
**wait step**: a step that names a probe with `wait_for` instead of
`resource`, with a required `timeout`, an optional `interval` (default
30s) and an optional `fail_on_error` (default false). The built-in
`weave` package ships one probe, `weave.command`, which is ready when a
shell script exits 0.

Five rules shape how a wait step runs:

1. **A probe is a declared script.** It lives in a package or a playbook,
   declares `param` blocks like a resource, and is compiled and
   contract-checked at `validate`. A wait step's `properties` are validated
   against those params, so one probe serves any number of waits.
2. **`check` never waits.** It runs the probe once. Not ready is
   `not configured` with the message `not ready; apply waits up to 3h`.
3. **Wait steps run outside the worker pool.** Each waiter runs on its own
   thread. It still counts as in flight, so a `global` step drains it and a
   sequential play waits for it.
4. **A wait step is always `parallel`.** It never holds an `exclusive`
   class or a `lock`. Any other `concurrency` is a validation error.
5. **A halt cancels waiters.** When the run halts, every wait step stops at
   its next pause between probe attempts and reports `not run`. This is a
   deliberate exception to PRD §9.

## Why each rule

- **Declared, not inline.** A probe that only parsed at run time would
  surface a typo hours into an apply, after the steps before it had
  already changed the host. A declared script fails `validate` instead,
  like every resource, gatherer and verify script. Declaring params also
  makes a probe reusable, and `list --json` reports it.
- **Check never waits.** `check` is the fast, safe dry run that operators
  run against production. A dry run that could block for three hours stops
  being one. The probe still runs once, so `check` reports whether `apply`
  would wait.
- **Off the pool.** A waiter spends nearly all its time asleep. If it held
  a worker, N waiters in a pool of N workers would block every resource
  step until one of them timed out, which is a deadlock with a timer on
  it. Threads are cheap next to that, and the count of waiters is bounded
  by the number of steps in the play.
- **Always parallel.** A class or a lock exists to stop two steps
  touching one thing at once. A wait step touches nothing. Letting it hold
  an `exclusive` class or a lock for hours would stall every step that
  needed it, for no safety gain.
- **Cancel on halt.** PRD §9 forbids mid-flight cancellation because
  killing a half-finished installer can leave the host broken. Cancelling a
  waiter between attempts changes nothing on the host, so that reason does
  not apply. Honouring §9 here would keep a failed run open for the rest
  of a three-hour timeout. A `ready` call already running is not
  interrupted; the cancel lands at the next pause, so the exception never
  cuts a script short.

## Consequences

- The DAG, the planner's report shapes and every output mode see a wait
  step as an ordinary step. Its `resource` field in reports is the probe
  reference. No new status was added: `already_configured`, `configured`,
  `not_configured`, `error` and `not_run` cover every outcome.
- The scheduler gains a second dispatch path, a cancel flag, and a
  `sequential` input, because a sequential play can no longer rely on its
  single worker to serialise steps once some steps run off the pool.
- A `global` step that becomes ready while a long wait runs sits behind
  that wait. This follows from waiters counting as in flight, and is the
  price of keeping `global` meaning "runs alone".
- PRD §9's no-cancellation rule now has one documented exception.
  `docs/notes.md` records it under "Probes and wait steps". Extending
  cancellation to anything that can mutate the host needs its own
  decision.
- Test steps cannot wait. A test asserts convergence of resources, and the
  test's own `ready` command already waits for the instance.

## Alternatives considered

- **A `ready` or `retry` attribute on every resource step.** Retry the
  step's `check` until it passes, or until a budget runs out. Rejected: it
  bleeds into the meaning of every resource. `check` would answer two
  different questions, "is the host converged?" and "has something else
  caught up?", and the convergence contract, the re-check and the
  testlab's three-run protocol would all have to learn which one a given
  step meant.
- **An inline wscript expression on the step** (`ready = "…"`). Shorter
  for one-off checks, but it compiles at run time, has no declared params
  and cannot be shared. `weave.command` covers the one-off case with a
  shell line, and a declared probe covers the rest.
- **Run waiters in the pool.** No new dispatch path, but `--jobs` would
  then bound how many things can wait at once, and enough waits would
  starve or deadlock the resource steps. Raising `--jobs` to compensate
  would also raise the overlap of real work.
- **Let waits run out on halt**, following PRD §9 to the letter. Rejected
  for the cost above. The safety reason behind §9 does not apply to a
  step that mutates nothing.
- **Let a wait step take a class or lock.** No use case needed it, and it
  invites the stall described above. A step that must not overlap with
  others while it waits can be placed in a sequential play.
- **Make `check` wait too.** This would make `check` predict `apply`
  exactly, at the cost of making the dry run slow and blocking. Rejected;
  the single-attempt report says what `apply` would do.
