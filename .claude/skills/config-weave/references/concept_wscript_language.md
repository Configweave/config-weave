# The wscript language

_Statically typed, Rust-flavored scripting — values, functions, structs/enums, match, Option/Result, containers, traits, memory._

Statically typed, Rust-flavored scripting language (Rust minus borrow checker,
lifetimes and generics). Scripts are **single files in v1** — no script-to-script
imports. Full tour: `~/dev/wscript/docs/tour.md`.


## Values and types

Primitives (value types): `int` (64-bit signed, wrapping), `float`, `bool`, `char`, `unit` (`()`). Everything else is a **reference type**: `string`, structs, enums, `List[T]`, `Map[K, V]`, function values, `weak[T]`.

```rust
let x = 5                  // inferred; annotations optional on lets
let name: string = "wil"
let log = "hp: " + str(99) // + concatenates strings; str() converts
let msg = fmt("{} of {}", 3, 10)   // NO string interpolation — use fmt()
```

- **No implicit numeric conversion** (`1 + 2.0` is a type error — use `int()`/`float()`).
- **No truthiness** — conditions must be `bool`.
- Statements end at newlines; semicolons permitted, never required.


## Functions and closures

```rust
fn area(w: int, h: int) -> int { w * h }
fn apply(f: fn(int) -> int, x: int) -> int { f(x) }
let double = |x| x * 2          // closures capture by reference
```

## Structs, enums, methods

```rust
struct Player { name: string, hp: int }
enum Event { Quit, Key(char), Click { x: int, y: int } }

impl Player {
    fn heal(self, amount: int) { self.hp = self.hp + amount }
}
```

`self` is implicit and always by reference (there is no `&` in wscript). **Reference semantics**: assignment/passing/returning reference types copies the reference, never the data. `same(a, b)` tests reference identity; deep copy is explicit via `#[derive(Clone)]` + `.clone()`.

## Pattern matching

`match` is an expression, **exhaustiveness-checked at compile time**:

```rust
match e {
    Event::Quit => false,
    Event::Key(c) if c == 'q' => false,          // guards (don't count for exhaustiveness)
    Event::Key('h') | Event::Key('?') => help(), // or-patterns (no bindings inside, v1)
    Event::Key(_) => true,
    Event::Click { x, y } => x >= 0 && y >= 0,
}
```

`if let Some(x) = opt { … }` and `let Some(n) = expr else { return Err("…") }` work as in Rust (let-else block must diverge).

## Option, Result and ?

`Option[T]` and `Result[T, E]` are always available. `?` early-returns the `None`/`Err` and composes across the host boundary — host errors arrive as `Err`. Methods: `is_some is_none unwrap unwrap_or expect` / `is_ok is_err unwrap unwrap_or unwrap_err expect`.

## Containers

```rust
let xs = [1, 2, 3]                  // List[int]; xs[0] faults OOB, xs.get(0) is Option
let ages = #{ "alice": 30 }         // Map[string, int]; keys: int|bool|char|string
ages["bob"] = 25                    // insert or overwrite; ages["nope"] faults — use .get
xs.map(|x| x * 2).filter(|x| x > 2).fold(0, |a, x| a + x)
```

Strings are immutable; operations are methods returning new strings, indexed in characters (not bytes). Method lists: [wscript built-ins](../references/concept_wscript_stdlib.md).

## Loops

```rust
for i in 0..10 { }       // exclusive; 0..=10 inclusive (ranges only in for headers)
for x in [1, 2, 3] { }   // list elements; maps iterate keys; strings iterate chars
while cond { }
loop { if done { break } }   // break / continue
```

## Traits and operators

Go-flavored interfaces, Rust syntax; static dispatch when concrete, `dyn Trait` for dynamic. No default bodies or trait inheritance in v1. Operator overloading via builtin traits `Add Sub Mul Div Rem Neg Eq Ord Display Index` (`Index` read-only; `Ord` custom form is `fn cmp(self, other: Self) -> int` returning -1/0/1). `==` on structs/enums **requires** an `Eq` impl. Derives: `#[derive(Eq, Ord, Display, Clone)]`.

## Memory and faults

Pure reference counting — **cycles leak** (no cycle collector); break cycles with `weak(x)` / `w.upgrade() -> Option[T]`. VM faults (index OOB, div by zero, `unwrap()` on `None`) are trappable errors delivered to the host with a stack trace — in config-weave they surface as the step's Error status. Prefer `xs.get(i)` / `m.get(k)` where failure is expected.

## Not in wscript (v1)

Borrow checker, `&`/`&mut`, lifetimes, user-defined generics, exceptions, async, threads, implicit conversions, truthiness, string interpolation (use `fmt`), compound assignment (`+=`), bitwise operators, range values outside `for` headers, script imports.

## Related

- [Resource & gatherer scripts](../references/concept_scripts.md)

- [wscript built-ins](../references/concept_wscript_stdlib.md)

[← All concepts](../references/concepts_ref.md)
