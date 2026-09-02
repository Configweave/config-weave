# Writing wscript scripts for config-weave

Everything needed to write a resource (`check` + `apply`), a gatherer (`gather`),
a test `verify` script or a scenario `run` script. The per-module function
tables of the host API (`fs`, `shell`, `log`, …) are in host-api.md.

## The convergence contract

The engine enforces these rules on every step. Break one and the step fails at
run time or in the testlab, never at validation.

- `check` reads the host and returns without writing. It creates nothing,
  starts nothing, and touches nothing it later reads.
- `apply` converges the host so that `check`, called again, returns
  `AlreadyConfigured`.
- Convergence is cross-process: a later run in a fresh process must also see
  `AlreadyConfigured`. Everything `check` reads survives a process restart (a
  file, a registry value, a service state, a package database), never a cache,
  an open handle or a process-local variable.
- config-weave keeps no record of what it applied. The host, as seen through
  `check`, is the only source of truth. The single exception is the built-in
  `weave.execute_once`, which records that a script has run.
- Write `check` first so it defines the desired state precisely, then make
  `apply` do only what satisfies it.

### What the engine does at each phase

| Phase | Result | Effect |
|---|---|---|
| Check | `AlreadyConfigured` | Step reports *already configured*; `apply` is not called. |
| Check | `NotConfigured` | Apply mode: the engine calls `apply`. Check mode: step reports *not configured*. |
| Check | `RebootRequired` | Apply mode: step reports *reboot required*, the play halts, exit status 3. Check mode: ordinary report status, run continues. |
| Check | `Err` or VM fault | Step reports *error*; the run halts unless `--continue-on-error` was passed. |
| Apply | `Success` | The engine runs `check` again (the re-check). |
| Apply | `RebootRequired` | Step reports *reboot required* with no re-check; the play halts with exit status 3. The next run starts from the check phase again. |
| Apply | `Err` or VM fault | Step reports *error*. |
| Re-check | `AlreadyConfigured` | Step reports *configured*. |
| Re-check | anything else | Step reports *error* with the message below. |

The exact re-check failure message, with the offending variant appended:

```
apply claimed success but the re-check disagrees (NotConfigured)
```

A step whose own `condition`, or an enclosing container's, is false skips all
three phases and reports *skipped*, and does not block its dependents. Steps
never dispatched because the run halted report *not run*, so every step has a
status after a halt. Output written through `log` or `print`/`println` appears
in the run output and the NDJSON log next to the step that produced it.

The testlab's three-run protocol runs check, apply, then apply again in a fresh
process, and requires every step to report *already configured* in the third
run. A step that reports *configured* there is not idempotent and fails the test.

## Entry points

Validation compiles every script against the host API and checks that the
exported functions exist with an accepted signature before anything runs. Each
entry point accepts two signatures: a plain one returning the result directly,
and a fallible one wrapped in `Result[…, string]` so the body can use `?`. An
`Err` becomes the step's *error* status with the string as its message. A VM
fault (index out of bounds, `unwrap()` on `None`) is reported the same way.
Only the entry file's functions are exported; a helper in `lib/` cannot supply
`check`. Generic functions cannot be entry points. This is the whole set of
entry points.

### check (resource)

```rust
fn check(params: Value) -> CheckResult
fn check(params: Value) -> Result[CheckResult, string]
```

`params` is a `Value::Map` of the step's properties keyed by parameter name,
with declared defaults applied and types validated. Called once at the start of
every step, and again after a successful `apply` as the re-check.

### apply (resource)

```rust
fn apply(params: Value) -> ApplyResult
fn apply(params: Value) -> Result[ApplyResult, string]
```

Receives the same `params` map as `check`. Called only in apply mode, and only
when `check` returned `NotConfigured`.

### gather (gatherer)

```rust
fn gather(params: Value) -> Value
fn gather(params: Value) -> Result[Value, string]
```

Returns facts, normally a `Value::Map`. A playbook `gather` block binds the
result to a variable named by its label. `params` is the gather's `params`
block with the gatherer's declared defaults applied. A key the gatherer declares
with `returns … type = "symbol"` comes back as the bare string, such as
`"systemd"`, and the engine binds it into the playbook as the symbol
`:systemd`. A failed gather aborts the run before any step executes.

### verify (test)

```rust
fn verify(facts: Value) -> bool
fn verify(facts: Value) -> Result[bool, string]
```

Runs inside the test instance after the three engine runs. `facts` is a map
from each test `gather` label to the value that gatherer returned inside the
instance. Returning `false` or `Err` fails the test; the `Err` string is the
failure message. Compiles during validation on the host but executes only inside
an instance. May import from `lib/`.

### run (scenario driver)

```rust
fn run(lab: Lab) -> bool
fn run(lab: Lab) -> Result[bool, string]
```

Runs on the host, not in a guest, against the live vmlab lab the scenario
declared. `Lab` is an opaque handle from the `testlab` module, registered only
for scenario scripts on top of the host API. `lab.machine(name)` starts a
declared VM on demand and returns its handle. Returning `false` or `Err` fails
the scenario. Scenarios run one at a time after the test groups.

## Result enums

Both are host-registered and ambient: no `use` line is needed to name them.
This is the whole set of variants.

```rust
enum CheckResult { AlreadyConfigured, NotConfigured, RebootRequired }
enum ApplyResult { Success, RebootRequired }
```

| Enum | Variant | Meaning |
|---|---|---|
| `CheckResult` | `AlreadyConfigured` | Host matches desired state. As the re-check result: step reports *configured*. |
| `CheckResult` | `NotConfigured` | Host needs `apply`. As the re-check result: *error*. |
| `CheckResult` | `RebootRequired` | Host needs a reboot before it can be judged or converged. As the re-check result: *error*. |
| `ApplyResult` | `Success` | Converged; the engine runs the re-check. |
| `ApplyResult` | `RebootRequired` | Change made but a reboot is pending; no re-check; play halts with exit 3. |

## Step statuses

The status a step reports, as printed in human output and as the stable
`status` id in `--json` output (`src/engine/status.rs`). This is the whole set.

```json
{ "step": "make-a", "status": "already_configured" }
```

| Status | JSON id | Meaning |
|---|---|---|
| already configured | `already_configured` | `check` returned `AlreadyConfigured`; nothing to do. |
| configured | `configured` | `apply` ran and the re-check returned `AlreadyConfigured`. |
| not configured | `not_configured` | Check mode only: `check` returned `NotConfigured`. |
| reboot required | `reboot_required` | `check` or `apply` returned `RebootRequired`. |
| skipped | `skipped` | The step's or an enclosing container's `condition` was false. |
| error | `error` | An `Err`, a VM fault, or a re-check that disagreed with `apply`. |
| not run | `not_run` | The run halted before the step, or a dependency errored or did not run. |

Exit code of a run: 1 if any step is *error*, else 3 in apply mode if any step
is *reboot required*, else 0.

## A complete minimal resource script

Declared in `package.wcl` as `script = "resources/file_present.ws"` with params
`path` (string, required) and `content` (string, default `""`).

```rust
use value
use fs
use path
use log

fn param_str(params: Value, key: string, fallback: string) -> string {
    if let Some(v) = params.get(key) {
        if let Some(s) = v.as_string() {
            return s
        }
    }
    fallback
}

fn check(params: Value) -> Result[CheckResult, string] {
    let p = param_str(params, "path", "")
    if p == "" {
        return Err("missing 'path' parameter")
    }
    if !fs::exists(p) {
        return Ok(CheckResult::NotConfigured)
    }
    let want = param_str(params, "content", "")
    let have = fs::read(p)?
    if have == want {
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

## A complete minimal gatherer script

```rust
use value
use sys

fn gather(params: Value) -> Value {
    Value::Map(#{
        "family": Value::String(sys::family()),
        "cores": Value::Int(sys::cpu_count()),
    })
}
```

## Imports

Import a host module with `use <module>`. Every fallible host function returns
`Result[T, string]` and composes with `?`. Every module is registered on every
platform: a Windows-only function such as `registry::read` compiles on Linux and
returns a runtime `Err` there. Guard platform-specific calls with a step
`condition` (`os.family == "windows"`) or in the script with `sys::family()`.

Registered modules (`src/hostapi/mod.rs`). This is the whole surface; a `use`
of any other name is a compile error at validation.

| Module | Origin | Role |
|---|---|---|
| `value` | wscript-std | Accessor methods on `Value` (`get`, `as_string`, …). |
| `json` | wscript-std | Parse and emit JSON to and from `Value`. |
| `toml` | wscript-std | Parse and emit TOML. |
| `xml` | wscript-std | Parse and emit XML. |
| `regex` | wscript-std | Pattern matching. Every function takes `(pattern, text)` in that order. |
| `time` | wscript-std | Clock and timestamp formatting. |
| `log` | config-weave | Structured output at a level. |
| `fs` | config-weave | Files and directories, metadata, glob, temp files, symlinks. |
| `path` | config-weave | Join, split, normalise paths. |
| `shell` | config-weave | `run` (no shell interpretation), `bash`, `powershell`. |
| `http` | config-weave | Fetch and download. |
| `hash` | config-weave | Digests of strings and files. |
| `archive` | config-weave | Extract archives. |
| `env` | config-weave | Environment variables. |
| `sys` | config-weave | Platform facts: family, OS name, CPU count. |
| `data` | config-weave | Parse and write INI. |
| `template` | config-weave | Render a Tera template against a map (autoescape off). |
| `registry` | config-weave, Windows only | Registry keys and values. |
| `service` | config-weave, Windows only | Windows services. |
| `com` | config-weave, Windows only | COM via IDispatch, WMI queries. |
| `testlab` | config-weave, scenario scripts only | `Lab` and `Machine` handles. |

Ambient registered types, no `use` needed: `Value`, `CheckResult`,
`ApplyResult`, `CmdOutput`, `HttpResponse`, `ComObject`.

### Helpers in lib/

Shared code lives in a package's own `pkgs/<pkg>/lib/` or the playbook's
`lib/`, which every package can see.

```rust
use helpers            // resolves helpers.ws by the order below
use "./shared.ws"      // relative to the importing script; extension required
```

A bare `use name` resolves in this order and the first match wins:

1. A registered host module. `use fs` is always the host module, even when
   `lib/fs.ws` exists.
2. The importing script's own directory, `<dir>/name.ws`.
3. The declaring package's `lib/`. A package can shadow a playbook-wide helper.
4. The playbook's `lib/`.

A helper's functions are called as `helpers::name`. Helper files are ordinary
`.ws` scripts and may import each other; the whole graph compiles as one unit
and only the entry file exports. `config-weave validate` compiles every
`lib/*.ws` whether imported or not, and reports an error inside a helper
against the helper's own file and line.

### Editor type-checking

`config-weave wscripti [outdir]` emits `weave.wscripti` (the full host
interface, generated from the same Rust modules the binary registers) and a
starter `wscript.toml`. With both next to the scripts, `wscript check` and the
wscript language server type-check against the exact config-weave surface.

## The Value type

`Value` is the one dynamically typed value in a script. It is the type of
`params`, of a gatherer's return, of `facts` in `verify`, and of what `json`,
`toml`, `xml` and `data` parse into. The type name is ambient; the accessor
methods below come from the `value` module, so a script that calls them starts
with `use value`.

```rust
enum Value {
    Null,
    Bool(bool),
    Int(int),
    Float(float),
    String(string),
    List(List[Value]),
    Map(Map[string, Value]),
}
```

`Value` is an ordinary enum: `match`, `if let` and `let-else` apply, and a
script constructs one with variant syntax (`Value::String(s)`,
`Value::Map(#{ "k": Value::Int(1) })`). A map key is always a `string`.

The `params` map already has declared defaults applied and every declared type
validated, so a script does not re-check a parameter's type. It still needs an
accessor, because the static type of `params.get("path")` is `Option[Value]`.

### Navigation

This is the whole set.

| Method | Signature | Behaviour |
|---|---|---|
| `get` | `(key: string) -> Option[Value]` | Map lookup. `None` for any other variant or a missing key. |
| `at` | `(idx: int) -> Option[Value]` | List index. `None` for any other variant, a negative index, or one past the end. |
| `keys` | `() -> List[string]` | A Map's keys, sorted. Empty list for any other variant. |
| `len` | `() -> int` | Element count of a List or Map. |
| `is_null` | `() -> bool` | True only for `Value::Null`. |

### Conversion

`Some` when the variant matches, `None` otherwise. Only `as_float` coerces (it
widens an `Int`). `as_string` on an `Int` is `None` and `as_int` on a `String`
is `None`: parse the string with `parse_int` yourself. This is the whole set.

| Method | Accepts | Returns |
|---|---|---|
| `as_bool` | `Bool` | `Option[bool]` |
| `as_int` | `Int` | `Option[int]` |
| `as_float` | `Float` or `Int` | `Option[float]` |
| `as_string` | `String` | `Option[string]` |
| `as_list` | `List` | `Option[List[Value]]` |
| `as_map` | `Map` | `Option[Map[string, Value]]` |

### Reading a parameter

Navigate with `get`, convert with an `as_` method, fall back when either yields
`None`. The `param_str` helper in the resource script above is the idiom the
sample and built-in resources use. The chained form for one field:

```rust
let mode = params.get("mode")
    .unwrap_or(Value::String("0644"))
    .as_string()
    .unwrap_or("0644")
```

A `match` on the `Value` itself reads better when the script branches on the
variant, for example a parameter accepted as either a string or a list of
strings.

## wscript essentials

wscript is statically typed and Rust-flavoured: Rust syntax without the borrow
checker, lifetimes or user-defined generic types. Scripts use the `.ws`
extension. Type errors, including a host function called with arguments in the
wrong order, are reported when `config-weave validate` compiles the script.

### Types, bindings, conversion

- Primitives, copied on assignment: `int` (64-bit, wrapping; `42`, `-7`,
  `0xFF`, `1_000_000`), `float` (`3.14`, `1e9`), `bool`, `char` (`'a'`,
  `'\n'`, `'\u{1F600}'`), `unit` (`()`).
- Everything else is a reference type: `string`, structs, enums, `List[T]`,
  `Map[K, V]`, function values, `weak[T]`.
- `let` binds; inference is local; annotations are optional on `let` and
  required on function signatures. There is no `let mut`: every binding is
  mutable and reassignable.
- No implicit numeric conversion: `1 + 2.0` is a type error. Use `int(x)`
  (truncates a float, code point of a char) and `float(x)`.
- No truthiness: a condition is a `bool`. Write `xs.len() > 0`, never `if xs`.

### Reference semantics

Assignment, argument passing and returns copy the reference for a reference
type, so two bindings to one struct see each other's mutations. There is no `&`
anywhere; `self` in a method is always by reference. `same(a, b)` tests
reference identity; `==` compares values and needs an `Eq` impl on a struct or
enum. Plain assignment never clones; a deep copy is `#[derive(Clone)]` plus
`.clone()`.

```rust
let alias = p          // same object
alias.hp = 70          // p.hp is now 70
```

### Strings

A `string` is immutable UTF-8; every method returns a new string. `len`,
`slice` and `find` count characters; `bytes_len` counts bytes.

```rust
let a = "hp: {99}"                       // {expr} interpolation, rendered like str()
let b = "player {p.name} at {x + 1}"     // any expression, including calls and nested strings
let c = "hp: " + str(99)                 // + concatenates; str() converts
let d = fmt("{} of {}", 3, 10)           // fmt fills {} in order
let e = fmt("{:>8} {:.2} {:x}", "hi", 3.14159, 255)
```

- Every string literal may hold `{expr}` holes. Write `{{` and `}}` for
  literal braces. A bare `{}` or `{:spec}` stays literal text, so a regex
  quantifier `[0-9]{4}` in a literal is written `[0-9]{{4}}`.
- A format spec inside a hole is rejected. Padding, precision and hex are
  `fmt` only.
- For a literal template `fmt` checks the placeholder count against the
  argument count at compile time.

### Functions and closures

- Every parameter and the return carry a type. An omitted return type is
  `unit`. A block evaluates to its last expression; a trailing semicolon
  discards it. A `unit` function must not end on a non-unit expression, so
  write `f();`.
- `return` exits early. In an entry point returning `Result[CheckResult,
  string]`, `return Err("missing 'path' parameter")` ends the step in *error*.
- Generic functions take the built-in bounds `Eq`, `Ord`, `Clone`:
  `fn max_of[T: Ord](a: T, b: T) -> T`. No generic structs, enums or traits.
- Function values have type `fn(T1, T2) -> R`. A closure is `|params| body`
  and captures by reference, so assigning to a captured variable changes the
  caller's binding.
- Semicolons are never required. A statement continues across a newline only
  inside an unclosed `(` or `[`, after a binary operator, comma or `=`, when the
  next line starts with `.`, or when the next token is `else`.

### Option, Result and `?`

`Option[T]` (`Some(v)` / `None`) and `Result[T, E]` (`Ok(v)` / `Err(e)`) are
always in scope. Both are enums, so `match`, `if let` and `let-else` apply.

- `?` early-returns the `None` or `Err` from the enclosing function, which must
  itself return an `Option` or a `Result` with a compatible error type. That is
  why every entry point has a fallible signature: `fs::read(p)?` hands the
  host's error string straight back as the step's *error* message. The plain
  signature has nowhere for `?` to send an error.
- `unwrap()` and `expect()` on `None` or `Err` raise a VM fault, which ends
  the step in *error* with a stack trace. Where absence is a normal outcome use
  `unwrap_or`, `if let`, `let-else` or `match`.
- A `let-else` block must diverge with `return`, `break` or an endless `loop`:
  `let Some(n) = s.parse_int() else { return Err("not a number") }`.

### Structs, enums, match

```rust
struct Player { name: string, hp: int }

impl Player {
    fn new(name: string) -> Player { Player { name: name, hp: 100 } }  // Player::new("x")
    fn heal(self, amount: int) { self.hp += amount }                   // self by reference
}

enum Event {
    Quit,                        // unit variant
    Key(char),                   // tuple variant
    Click { x: int, y: int },    // struct variant
}

fn handle(e: Event) -> bool {
    match e {
        Event::Quit => false,
        Event::Key(c) if c == 'q' => false,            // guard
        Event::Key('h') | Event::Key('?') => help(),   // or-pattern, no bindings
        Event::Key(_) => true,
        Event::Click { x, y } => x >= 0 && y >= 0,
    }
}
```

- `match` and `if` are expressions; both arms of an `if` must agree on a type.
- `match` is exhaustiveness-checked at compile time. A guarded arm never
  counts toward exhaustiveness, so it needs an unguarded arm after it for the
  same variant.
- Patterns: `_`, `name`, a literal (`42`, `'q'`, `"hi"`), `Enum::Variant(x)`,
  `Enum::Variant { x, y }`, `pat1 | pat2` (cannot bind names).
- Where a variant and an associated function share a name, the variant wins.
- Compound assignment (`+=` and the rest) works on a variable, a field, or a
  list or map element.

### Containers and loops

```rust
let xs = [1, 2, 3]                       // List[int]
xs.push(4)
xs[0]                                    // faults if out of bounds
xs.get(99)                               // None, never faults
let lines = out.split("\n").filter(|l| !l.trim().is_empty())

let ages = #{ "alice": 30, "bob": 25 }   // Map[string, int]
ages["carol"] = 22                       // insert or overwrite
ages["nope"]                             // faults
ages.get("nope")                         // None
ages.each(|k, v| println("{k} is {v}"))

for i in 0..10 { }        // exclusive; 0..=10 inclusive; ranges only in a for header
for x in xs { }           // elements
for k in ages { }         // keys
for c in "abc" { }        // chars
while cond { }
loop { if done { break } }
```

- Map keys are `int`, `bool`, `char` or `string`. Map closures for `each`,
  `map` and `filter` take `(key, value)`. Map `map` returns a `List`; map
  `filter` returns a `Map`.
- `break` and `continue` work in all three loop forms.

### Traits, operators, derives

- A trait is Rust syntax with Go-style satisfaction. `dyn Trait` selects a
  vtable; a concrete type coerces to `dyn Trait` implicitly at a typed boundary.
  No default method bodies, no trait inheritance.
- Operator traits: `Add`, `Sub`, `Mul`, `Div`, `Rem` (`+ - * / %`), `Neg`,
  `Eq` (`==`, `!=`), `Ord` (`<` etc., hand-written `fn cmp(self, other: Self)
  -> int` returning -1, 0, 1), `Display` (`str(x)` and interpolation), `Index`
  (`x[i]`, read-only). An `Add` impl gives `+=` for free.
- `#[derive(Eq, Ord, Display, Clone)]`: structural comparison, debug-style
  rendering, deep copy.
- No bitwise operators. Write the arithmetic out with `*`, `/`, `%`.
- `units` declares a named unit family on an `int` or `float` (`500ms`,
  `4MiB`); two families cannot be added; the value reaches a host function, a
  `List` or a `Value` as its plain backing number.

### Memory and faults

- Memory is pure reference counting with no cycle collector. A reference cycle
  leaks until the VM is dropped at the end of the step. Break a cycle with
  `weak(x)` and `w.upgrade() -> Option[T]`, typically for a child-to-parent
  back-reference.
- A fault is a VM-level error: index out of bounds, division by zero,
  `unwrap()` on `None` or `Err`, an invalid `regex` pattern, or an aliasing
  violation at the host boundary. Script code cannot catch it. It ends the step
  in *error* with the message (for example `list index 10 out of bounds (len
  3)`) and a stack trace, innermost first, in the log. A fault never panics
  config-weave.
- Each step runs in its own VM. There is no async and no shared-value
  threading; parallelism is the scheduler's job.

## Prelude

Callable in every script without a `use` line. Also always in scope: the types
`Option[T]`, `Result[T, E]`, `List[T]`, `Map[K, V]`, `weak[T]`, and every
host-registered type. This is the whole prelude.

| Function | Signature | Notes |
|---|---|---|
| `print` | `(value: any)` | Renders via `Display`. In config-weave it does not write to stdout: each line becomes a `log::info` record. |
| `println` | `(value: any)` / `()` | As `print` plus a newline; no argument emits a bare newline. Same `log::info` routing. |
| `str` | `(value: any) -> string` | Renders via `Display`; also the rendering used by `{expr}` holes. |
| `fmt` | `(template: string, args: any...) -> string` | Fills `{}` placeholders in order; `{{`/`}}` for literal braces; specs after a colon. |
| `same` | `(a: T, b: T) -> bool` | Reference identity, not value equality. |
| `weak` | `(value: T) -> weak[T]` | Argument must be a reference type. |
| `int` | `(int \| float \| char) -> int` | Float truncates toward zero; char gives its code point. |
| `float` | `(int \| float) -> float` | Float passes through. |

`fmt` spec grammar, after the colon in a placeholder:

| Spec part | Form | Meaning |
|---|---|---|
| fill and align | `[fill]<`, `[fill]^`, `[fill]>` | left, centre or right in the width, padded with the fill character |
| zero | `0` | zero-pad a number, sign-aware |
| width | an integer | minimum width in characters |
| precision | `.N` | digits after the point for a float; truncation length for a string |
| type | `x`, `X`, `b`, `o` | hex, upper hex, binary or octal; integers only |

Examples: `{:>8}` right-aligns in eight, `{:.2}` two decimals, `{:04}`
zero-pads to four, `{:x}` hex.

## Built-in methods

Known to the compiler on `string`, `List[T]`, `Map[K, V]`, `Option[T]`,
`Result[T, E]` and `weak[T]`. No `use` line; cannot be extended from a script.
`T`, `K`, `V`, `E` are the receiver's type arguments; `U`, `R` are inferred at
the call site. Each table is the whole surface for its type.

### string

| Method | Signature | Notes |
|---|---|---|
| `len` | `() -> int` | character count |
| `bytes_len` | `() -> int` | UTF-8 byte count |
| `is_empty` | `() -> bool` | |
| `split` | `(sep: string) -> List[string]` | |
| `trim` | `() -> string` | both ends |
| `trim_start` | `() -> string` | |
| `trim_end` | `() -> string` | |
| `to_upper` | `() -> string` | |
| `to_lower` | `() -> string` | |
| `starts_with` | `(prefix: string) -> bool` | |
| `ends_with` | `(suffix: string) -> bool` | |
| `contains` | `(needle: string) -> bool` | |
| `find` | `(needle: string) -> Option[int]` | character index of the first match |
| `replace` | `(from: string, to: string) -> string` | every occurrence |
| `repeat` | `(times: int) -> string` | |
| `pad_left` | `(width: int, fill: string) -> string` | |
| `pad_right` | `(width: int, fill: string) -> string` | |
| `chars` | `() -> List[char]` | |
| `slice` | `(start: int, end: int) -> string` | character indices, end exclusive |
| `parse_int` | `() -> Option[int]` | `None` on a malformed number |
| `parse_float` | `() -> Option[float]` | `None` on a malformed number |

### List[T]

`xs[i]` faults when out of bounds; `get` returns an `Option`. A constrained
method compiles only when the element type meets it. A `sort_by` comparator
returns a negative, zero or positive `int`.

| Method | Signature | Notes |
|---|---|---|
| `len` | `() -> int` | |
| `is_empty` | `() -> bool` | |
| `clear` | `() -> unit` | |
| `push` | `(x: T) -> unit` | append |
| `pop` | `() -> Option[T]` | remove and return the last element |
| `insert` | `(i: int, x: T) -> unit` | |
| `remove` | `(i: int) -> T` | faults when out of bounds |
| `get` | `(i: int) -> Option[T]` | never faults |
| `set` | `(i: int, x: T) -> unit` | |
| `first` | `() -> Option[T]` | |
| `last` | `() -> Option[T]` | |
| `contains` | `(x: T) -> bool` | `T: Eq` |
| `index_of` | `(x: T) -> Option[int]` | `T: Eq` |
| `any` | `(f: fn(T) -> bool) -> bool` | |
| `all` | `(f: fn(T) -> bool) -> bool` | |
| `count` | `(f: fn(T) -> bool) -> int` | |
| `find` | `(f: fn(T) -> bool) -> Option[T]` | first match |
| `position` | `(f: fn(T) -> bool) -> Option[int]` | index of the first match |
| `reverse` | `() -> unit` | in place |
| `sort` | `() -> unit` | in place; `T: Ord` |
| `sort_by` | `(cmp: fn(T, T) -> int) -> unit` | in place |
| `map` | `(f: fn(T) -> U) -> List[U]` | |
| `map_indexed` | `(f: fn(int, T) -> U) -> List[U]` | |
| `filter` | `(f: fn(T) -> bool) -> List[T]` | |
| `fold` | `(init: U, f: fn(U, T) -> U) -> U` | |
| `zip_with` | `(other: List[U], f: fn(T, U) -> R) -> List[R]` | stops at the shorter list |
| `join` | `(sep: string) -> string` | `List[string]` only |
| `slice` | `(start: int, end: int) -> List[T]` | end exclusive |
| `concat` | `(other: List[T]) -> List[T]` | new list |
| `sum` | `() -> T` | `int` or `float` elements |
| `min` | `() -> Option[T]` | `T: Ord`; `None` when empty |
| `max` | `() -> Option[T]` | `T: Ord`; `None` when empty |
| `clone` | `() -> List[T]` | deep copy |

### Map[K, V]

`m[k]` faults when the key is missing; `get` returns an `Option`. `m[k] = v`
inserts or overwrites.

| Method | Signature | Notes |
|---|---|---|
| `len` | `() -> int` | |
| `is_empty` | `() -> bool` | |
| `clear` | `() -> unit` | |
| `insert` | `(k: K, v: V) -> unit` | insert or overwrite |
| `remove` | `(k: K) -> Option[V]` | the removed value |
| `get` | `(k: K) -> Option[V]` | never faults |
| `contains_key` | `(k: K) -> bool` | |
| `keys` | `() -> List[K]` | |
| `values` | `() -> List[V]` | |
| `each` | `(f: fn(K, V) -> unit) -> unit` | |
| `map` | `(f: fn(K, V) -> U) -> List[U]` | one element per entry |
| `filter` | `(f: fn(K, V) -> bool) -> Map[K, V]` | new map |
| `clone` | `() -> Map[K, V]` | deep copy |

### Option[T]

| Method | Signature | Notes |
|---|---|---|
| `is_some` | `() -> bool` | |
| `is_none` | `() -> bool` | |
| `unwrap` | `() -> T` | faults on `None` |
| `unwrap_or` | `(default: T) -> T` | |
| `expect` | `(msg: string) -> T` | faults on `None` with `msg` |

### Result[T, E]

Every host function that can fail returns `Result[T, string]`.

| Method | Signature | Notes |
|---|---|---|
| `is_ok` | `() -> bool` | |
| `is_err` | `() -> bool` | |
| `unwrap` | `() -> T` | faults on `Err` |
| `unwrap_or` | `(default: T) -> T` | |
| `unwrap_err` | `() -> E` | faults on `Ok` |
| `expect` | `(msg: string) -> T` | faults on `Err` with `msg` |

### weak[T]

| Method | Signature | Notes |
|---|---|---|
| `upgrade` | `() -> Option[T]` | `Some` while a strong reference exists, `None` after |

## Not registered

The wscript standard library's `math`, `process` and `fs` modules are not
registered. `use math` or `use process` is a compile error at validation. `use
fs` resolves to the config-weave `fs` module, which is a superset of the
standard one, so nothing needs migrating.

| Module | Use instead |
|---|---|
| `math` | Plain operators `+ - * / %` and comparisons; `int()` truncates, `float()` widens. |
| `process` | `shell::run`, `shell::bash`, `shell::powershell`; `env` for environment variables. |
| `fs` (std) | The config-weave `fs` module, which `use fs` already resolves to. |

Structured data: JSON, TOML, XML are the standard-library modules registered
as they are; INI is the config-weave `data` module.

Language features that do not exist, and what to write instead:

| Absent | Instead |
|---|---|
| Borrow checker, `&`, `&mut`, lifetimes | Reference types alias on assignment; `self` is by reference. |
| Generic structs, enums, traits | Generic functions with `Eq`, `Ord`, `Clone` bounds; built-in `List[T]`, `Map[K, V]`. |
| Exceptions | `Result` and `?`; a fault ends the step. |
| Async, shared-value threads | One VM per step; parallelism is the scheduler's. |
| Implicit conversions, truthiness | `int(x)`, `float(x)`; a `bool` condition. |
| Cycle collector | `weak[T]` for back-references. |
| Bitwise operators | `*`, `/`, `%`. |
| Cross-family unit arithmetic | One `units` family per dimension. |
| Format specs in interpolation holes | `"{expr}"` only; `fmt("{:.2}", x)` for formatting. |
| Range values outside a `for` header | `0..n` only in a `for` header. |
| Bindings inside an or-pattern | Split the arms, or bind in one pattern and test with a guard. |
| Default trait method bodies, trait inheritance | Implement every method on every type. |

Allowed, despite older restrictions: `use` of a `lib/` script, `{expr}` in
every string literal, compound assignment, `Type::func(...)` associated
functions, generic functions, unit families.

## Gotchas

- `check` that writes anything, or reads process-local state, passes
  validation and fails in the testlab's third run or on the next scheduled run.
- The re-check runs in the same process; only the testlab proves cross-process
  convergence.
- `?` needs the fallible entry-point signature. With the plain signature a
  host `Result` has to be handled with `unwrap_or` or `match`.
- `unwrap()`, `xs[i]`, `m[k]` and `list.remove(i)` fault; prefer `get`,
  `unwrap_or`, `if let`, `let-else`.
- `params.get("k")` is `Option[Value]`, never the value itself; always follow
  with an `as_` conversion. `as_string` on an `Int` is `None`.
- `regex` functions take `(pattern, text)`; swapping them compiles and never
  matches. An invalid pattern is a fault, not an `Err`.
- `shell::run` splits with shell-words and executes the program directly, with
  no pipes, globs or redirects; use `shell::bash` or `shell::powershell` for
  those. `powershell` tries `powershell` then `pwsh`.
- `print`/`println` go to `log::info`, never stdout, so they cannot corrupt the
  JSON report and cannot be used to return data.
- A `{}` or `{:spec}` in a string literal is literal text; `{expr}` is code.
  A regex quantifier in a literal needs `{{4}}`.
- `==` on a struct or enum needs `Eq`; `same()` is identity.
- A Windows-only module compiles everywhere and returns an `Err` at runtime
  on Linux; gate the step with `condition` or the call with `sys::family()`.
- A gatherer key declared `type = "symbol"` returns the bare string; the
  engine makes it a symbol.
- Author a Tera template body as a raw WCL heredoc (`<<'TMPL'`) so WCL's own
  `$"...${}"` interpolation leaves Tera's `{{ }}` and `{% %}` alone.
- Generic functions cannot be entry points; config-weave calls only
  monomorphic functions.
