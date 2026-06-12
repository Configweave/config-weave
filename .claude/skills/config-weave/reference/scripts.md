# Writing resource & gatherer scripts (wisp)

Resource, gatherer and verify scripts are single-file wisp programs compiled against the
config-weave host API. Import host modules with `use <module>`; registered types
(`Value`, `CheckResult`, `ApplyResult`, `CmdOutput`, `HttpResponse`, `ComObject`) are
ambient — no `use` needed for type names. See `wisp-language.md` for the language,
`hostapi.md` for the modules.

## Entry-point contracts

Each entry point accepts two signatures — plain, or fallible when you want `?`:

```rust
// resources/<name>.wisp
fn check(params: Value) -> CheckResult            // or -> Result[CheckResult, string]
fn apply(params: Value) -> ApplyResult            // or -> Result[ApplyResult, string]

// gatherers/<name>.wisp
fn gather(params: Value) -> Value                 // or -> Result[Value, string]

// tests/<name>.wisp (testlab verify, see testing.md)
fn verify(facts: Value) -> bool                   // or -> Result[bool, string]
```

An `Err` (or a VM fault) maps to the step's **Error** status.

```rust
enum CheckResult { AlreadyConfigured, NotConfigured, RebootRequired }
enum ApplyResult { Success, RebootRequired }
```

## Step lifecycle (per step)

1. **Check** — `AlreadyConfigured` → report *Already Configured*, continue.
   `RebootRequired` → in apply mode report *Reboot Required* and halt the play (exit 3);
   in check mode it is an ordinary report status (check is report-only).
   `NotConfigured` → proceed to apply (check mode just reports *Not Configured*).
   Error → halt unless `--continue-on-error`.
2. **Apply** — `Success` → re-check. `RebootRequired` → report and halt.
3. **Re-check** — must return `AlreadyConfigured`, which reports *Configured*; anything
   else reports *Error* ("apply claimed success but check disagrees").

`check` must never mutate; `apply` must converge so the re-check passes — and must
converge **across processes** too (the testlab's third run catches state that only
exists in-process).

## Reading params

`params` is a `Value::Map` with declared defaults already applied and types already
validated (see `packages.md`). Typical access pattern:

```rust
use value
use fs
use path
use log

fn param_str(params: Value, key: string, fallback: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() { return s }
    }
    fallback
}

fn check(params: Value) -> Result[CheckResult, string] {
    let p = param_str(params, "path", "")
    if p == "" { return Err("missing 'path' parameter") }
    if !fs::exists(p) { return Ok(CheckResult::NotConfigured) }
    if fs::read(p)? == param_str(params, "content", "") {
        Ok(CheckResult::AlreadyConfigured)
    } else {
        Ok(CheckResult::NotConfigured)
    }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let p = param_str(params, "path", "")
    log::info("writing " + p)
    fs::mkdir(path::parent(p))?
    fs::write(p, param_str(params, "content", ""))?
    Ok(ApplyResult::Success)
}
```

## Gatherer example

Gatherers return any `Value`; the result lands in the playbook variable named by the
`gather` block's label (e.g. `os.family`).

```rust
use value
use sys

fn gather(params: Value) -> Value {
    Value::Map(#{
        "family": Value::String(sys::family()),
        "name": Value::String(sys::os_name()),
        "cpus": Value::Int(sys::cpu_count())
    })
}
```

## Logging & output

- Use `log::debug/info/warn/error` — messages carry step context into the terminal
  output and the NDJSON file log.
- Raw `print`/`println` are redirected into `log::info` (stdout stays clean for `--json`).
- `shell::run_streaming` pipes a long command's output through `log` live.

## Editor support

`config-weave wispi [outdir]` emits `weave.wispi` (the full host interface) plus a
starter `wisp.toml`. With those next to your scripts, `wisp check` and the wisp LSP
type-check scripts against the exact config-weave surface — host API misuse is a
compile-time error, also caught by `config-weave validate` (stage that compiles every
script).
