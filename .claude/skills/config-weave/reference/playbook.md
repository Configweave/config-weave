# playbook.wcl

`playbook.wcl` is the WCL document at the root of a playbook directory. It holds
exactly one `playbook` block: gathers, variables, playbook-local composites and
probes, and plays of steps. The schema is `src/vocab/playbook.wcl`.

| Item | Value |
|---|---|
| Location | `<playbook>/playbook.wcl` |
| System import | `<weave/playbook.wcl>`, appended by the engine; never write an import line |
| Top-level block | `playbook "name" { … }`, exactly one per file |
| Run target | `check` and `apply` take one play by name |

## Directory layout

```text
my-playbook/
  playbook.wcl        # the playbook document
  lib/                # optional: shared wscript helpers, visible to every package
  pkgs/
    core/             # one package per directory
      package.wcl
      resources/
      gatherers/
```

`pkgs/weave/` is rejected: `weave` is the built-in package shipped in the binary.

## Minimal complete playbook

```wcl
playbook "Sample Baseline" {
  description = "Exercises the model loader, validation and execution"
  version = "1.0.0"                      // optional, default "0.0.0"

  gather "os" {                          // label = the variable the result lands in
    description = "Operating system facts"
    from = "core.os_info"                // package.gatherer
    params { depth = 2 }                 // optional, validated against the gatherer's params
  }

  vars {
    work_root = "/tmp/config-weave-sample"
    is_linux = os.family == "linux"      // may reference gatherer results
    marker_a = $"${work_root}/a.txt"     // WCL string interpolation
  }

  play "baseline" {
    description = "Create marker files in order"

    step "make-a" {
      description = "Create the first marker file"
      resource = "core.file_present"     // package.resource
      condition = is_linux               // optional bool expr; false => Skipped
      properties {
        path = marker_a
        content = "alpha"
      }
    }

    container "secondary" {
      description = "Files that depend on the first"
      step "make-b" {
        description = "Create the second marker file"
        resource = "core.file_present"
        requires = ["make-a"]            // ordering edges by step name
        properties {
          path = $"${work_root}/b.txt"
          content = "beta"
        }
      }
    }
  }
}
```

## Blocks

This is the whole surface: `playbook`, `gather`, `vars`, `play`, `container`,
`step`, `properties`, `params`, `composite`, `arg`, `probe`, `param`, `symbol`. Every field marked
required is enforced by the loader, including each block's `description`. WCL's
own block check flags unknown fields only, so a missing required field is a
loader validation error. Child blocks may appear in any order.

### playbook

```wcl
playbook "Sample" {
  description = "What this playbook converges"
  version = "1.2.0"
  gather "os" { … }
  vars { … }
  composite "site" { … }
  probe "marker" { … }
  play "default" { … }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The playbook name, reported in every run. |
| `description` | string | yes | | One-line summary, rendered by `config-weave docs`. |
| `version` | string | no | `"0.0.0"` | Free-form version string, reported with the name. |
| `gather` | block, repeatable | no | | Gatherer invocations. |
| `vars` | block, at most one | no | | Variable declarations. |
| `composite` | block, repeatable | no | | Playbook-local composites. |
| `probe` | block, repeatable | no | | Playbook-local probes, for wait steps. |
| `play` | block, repeatable | no | | The plays. |

### gather

```wcl
gather "os" {
  description = "Operating system facts"
  from = "core.os_info"
  params { detail = :full }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The variable that receives the gathered value, read as `os.family`. |
| `description` | string | no | | One-line summary for the docs. |
| `from` | string | yes | | The gatherer as `package.gatherer`. |
| `params` | block, at most one | no | | Parameters, validated against the gatherer's `param` declarations. |

Rules:
- All gathers run concurrently before any step. Two gathers with the same
  gatherer and the same canonicalised params run once and share the result.
- A gather that fails aborts the run before any step executes.
- A gather result is visible to `vars`, conditions and properties, never to
  another gather's `params`.

### vars

```wcl
vars {
  config_dir = "/etc/sample"
  is_debian = os.family == "linux" && os.distro == "debian"
  db_password = secret("CWENC1.…")
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| any name | expression | no | | Declares a variable of that name. The block has no fixed fields. |

A value is any WCL expression and may reference gatherer results and other
variables.

### play

```wcl
play "default" {
  description = "Base configuration"
  parallel = true
  step "motd" { … }
  container "web" { … }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The play name, passed to `check` and `apply`. |
| `description` | string | yes | | One-line summary for the docs. |
| `parallel` | bool | no | `true` | `false` runs steps one at a time in declaration order, ignoring the graph. |
| `step` | block, repeatable | no | | Steps directly under the play. |
| `container` | block, repeatable | no | | Groups of steps. |

### container

```wcl
container "web" {
  description = "Everything the web tier needs"
  condition = os.family == "linux"
  step "nginx" { … }
  container "tls" { … }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The container name, one segment of a step's report path. |
| `description` | string | yes | | One-line summary for the docs. |
| `condition` | bool expression | no | | When false, every child step reports Skipped. |
| `step` | block, repeatable | no | | Child steps. |
| `container` | block, repeatable | no | | Nested containers, to any depth. |

Rules:
- A container groups steps for organisation and docs only. The play's
  dependency graph is flat across containers: `requires` names a step by its
  own name regardless of which container holds it.
- A `condition` on a container applies to every step beneath it.
- This container is unrelated to the container instances the testlab runs in.

### step

```wcl
step "nginx" {
  description = "Install nginx"
  resource = "linux_apt.package"
  condition = is_debian
  requires = ["update_cache"]
  concurrency = "exclusive"
  properties {
    name = "nginx"
    ensure = :present
  }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The step name. Unique within the play; the target of `requires`. |
| `description` | string | yes | | One-line summary, shown in reports and docs. |
| `resource` | string | exactly one of `resource` / `wait_for` | | `pkg.resource`, `pkg.composite`, or the bare name of a composite declared in this playbook. |
| `wait_for` | string | exactly one of `resource` / `wait_for` | | Makes a wait step. `pkg.probe`, or the bare name of a probe declared in this playbook. |
| `timeout` | duration | yes on a wait step | | Wait steps only. How long `apply` polls before the step is Error. At least `1s`. |
| `interval` | duration | no | `30s` | Wait steps only. Pause between probe attempts. At least `1s`. |
| `fail_on_error` | bool | no | `false` | Wait steps only. `true` fails the step at the first probe error instead of counting it as not ready. |
| `condition` | bool expression | no | | When false the step reports Skipped, no phase runs, and its dependents still run. |
| `requires` | list of strings | no | | Step names in this play that must finish first. Ordering only, not a success demand. |
| `concurrency` | string | no | resource's class | `"parallel"`, `"exclusive"` or `"global"`. Tightens the resource's declared class, never loosens it. |
| `properties` | block, at most one | no | | The resource's or probe's parameters. |

Rules:
- Resource names from packages are always qualified `pkg.resource`. Only a
  playbook-local composite is referenced bare.
- Each step walks check, apply, re-check. The script contract: scripts.md.
  Concurrency class declarations on a resource: package.md.
- Both `resource` and `wait_for`, or neither, is a validation error. A probe
  named with `resource =`, or a resource with `wait_for =`, is a validation
  error whose hint names the right field.
- `requires` naming a step that does not exist, or a set of edges that forms a
  cycle, is a load error.
- Dependency outcomes: Already Configured, Configured and Skipped release
  dependents. In apply mode an Error or Not Run dependency blocks dependents,
  which report Not Run naming the dependency.
- A run halts when a step errors without `--continue-on-error` or reports
  Reboot Required in apply mode. Every undispatched step then reports Not Run.
- Concurrency order, loosest to tightest: `parallel` (overlaps freely),
  `exclusive` (no two steps of the same resource at once), `global` (drains all
  in-flight steps, runs alone, then dispatching resumes). A step value looser
  than the resource's class is a validation error. `--jobs` sets the worker
  pool size.

#### Wait steps

```wcl
step "api_up" {
  description = "Wait for the API"
  wait_for = "net.http_ok"                      // or "weave.command", or a bare playbook probe
  interval = 30s
  timeout = 3h
  fail_on_error = false
  requires = ["start_api"]
  properties { url = "https://api.internal/health" }   // validated against the probe's params
}
```

Rules:
- `interval`, `timeout` and `fail_on_error` on a resource step are a
  validation error. A wait step with no `timeout` is a validation error.
- `interval` / `timeout` are bare duration literals: `ns`, `us`, `ms`, `s`,
  `min`, `h`, `d`. Minutes are `min`; `1m` fails (`'m' is not a unit of type
  'std.Duration'`). They may be expressions (a var, `args.patience`): a static
  value is range-checked at validate, a dynamic one at plan time.
- `concurrency` on a wait step may only be `"parallel"`.
- check runs the probe once and never waits. apply runs it, then every
  `interval`, until ready or `timeout`; the last pause is shortened so a final
  attempt lands on the deadline. Statuses and messages:

| Outcome | Mode | Status | Message |
|---|---|---|---|
| ready, first attempt | both | already configured | |
| not ready | check | not configured | `not ready; apply waits up to 3h` |
| probe error | check | not configured | `probe failed: <msg>; apply waits up to 3h` |
| ready after waiting | apply | configured | `ready after 4m12s (9 attempts)` |
| deadline passed | apply | error | `not ready after 3h (361 attempts)` + `; last probe failed: <msg>` if the last attempt errored |
| probe error with `fail_on_error = true` | both | error | `probe failed: <msg>`, at the first error |
| run halted while waiting | apply | not run | `wait cancelled after 2m because the run halted` |

- A probe error (`Err`, a VM fault, a killed `weave.command` attempt) counts as
  not ready and polling continues, unless `fail_on_error = true`.
- Scheduling: a wait step runs on its own thread, outside the `--jobs` pool, so
  any number can wait even with `--jobs 1`. It is always `parallel` and never
  holds a class or `lock`. It counts as in flight: a `global` step waits for
  running wait steps, and no wait step starts while a `global` step runs. In a
  `parallel = false` play it holds the play until it ends.
- Halt cancels waiters (an exception to "in-flight steps finish"): at the next
  pause between attempts they report Not Run. A `ready` call in progress is not
  interrupted.
- Dependents behave as for any step: in apply, a wait step that errors or is
  cancelled blocks them (Not Run).
- Reports show the probe reference (`net.http_ok`, or the bare name) where a
  resource would be.

### properties and params

```wcl
properties {
  path = "/etc/motd"
  content = $"Welcome to ${os.hostname}"
  mode = :strict
  max_age = 30min
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| any declared name | per the `param` type | per the `param` | per the `param` | One parameter value. Names not declared by the resource, probe or gatherer are rejected. |

Rules:
- A step's `properties` is validated against the `param` declarations of the
  resource or probe it names, a gather's `params` against the gatherer's: declared defaults
  are applied, unknown names rejected, required names checked, each value
  checked against its coarse type.
- A `symbol` parameter is written `:name`; the quoted spelling `"name"` is an
  error.
- A `duration` parameter is a bare WCL unit literal (`30min`, `4h`), never
  quoted.
- A `map` parameter is a map literal (`env = { KEY: "value" }`), never a
  nested block; any nested block inside `properties` / `params` is an error.
- Values are any WCL expression over the variable scope, evaluated lazily when
  the step is planned.
- Field names inside the block are in scope and shadow outer variables of the
  same name: `url = url` is a self-reference cycle error. Declare `tool_url` in
  `vars` and write `url = tool_url`.

### composite

```wcl
composite "site" {
  description = "A static site: directory plus index page"
  arg "root" {
    description = "Directory that holds the site"
    type = "string"
    required = true
  }
  arg "body" {
    description = "Contents of index.html"
    type = "string"
    default = "<h1>hello</h1>"
  }

  step "dir" {
    description = "Site directory"
    resource = "core.directory"
    properties { path = args.root }
  }
  step "index" {
    description = "Index page"
    resource = "core.file"
    requires = ["dir"]
    properties {
      path = $"${args.root}/index.html"
      content = args.body
    }
  }
}

play "sites" {
  description = "Two sites from one block"
  step "alpha" {
    description = "The alpha site"
    resource = "site"                    // bare: playbook-local composite
    properties { root = "/srv/alpha" }   // arguments are passed as properties
  }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The composite name. Shares a namespace with resources, and the bare-name namespace with playbook probes. |
| `description` | string | yes | | One-line summary for the docs. |
| `arg` | block, repeatable | no | | Declared arguments. |
| `step` | block, repeatable | no | | The body. Each step has the fields of a playbook `step`. |

Rules:
- A composite is invoked from a step's `resource` field exactly like a
  resource. Declared in a playbook it is referenced bare (`resource = "site"`);
  the identical block in a `package.wcl` is referenced qualified
  (`resource = "web.site"`).
- Composites and resources share one namespace: a package cannot declare both
  under one name.
- Expansion is static, at load time. An invocation becomes a synthetic
  container of real steps, so the DAG, planner and every report see ordinary
  steps. An expanded step's report path gains one segment per enclosing
  invocation: `container/…/invocation/inner`, for example `sites/alpha/dir`.
- A body sees only its own arguments: never gatherer results, `vars` or `--var`
  overrides. A fact the body needs is passed in as a property.
- Each argument binds twice in the body: bare (`root`) and as `args.root`. Use
  `args.`: a property field shadows the bare name, so
  `properties { path = path }` is a self-reference cycle while
  `properties { path = args.path }` always works.
- `requires` scope: inside a body a name reaches only a sibling step of the
  same invocation. From the playbook a name reaches only playbook-declared
  steps, so an inner step is not addressable from outside. Naming an
  invocation in `requires` waits for every step it expanded into.
- Nesting is capped at 8 levels. A composite that invokes itself, directly or
  indirectly, is rejected by name as a cycle.
- A body may hold wait steps. An unqualified `wait_for` in a playbook
  composite names a playbook-local probe.
- An invocation's own `concurrency` tightens every resource step of the body;
  wait steps in the body stay `parallel`. A body
  step may carry its own `concurrency`. An `expect` field (testlab steps only)
  is rejected in a composite body.
- `secret()` inside a composite in a `package.wcl` is a validation error, like
  anywhere else in a package.

### arg

```wcl
arg "ensure" {
  description = "Whether the site should exist"
  type = "symbol"
  default = :present
  symbol "present" { description = "Create or update the site" }
  symbol "absent"  { description = "Remove the site" }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The argument name. |
| `description` | string | yes | | One-line summary for the docs. |
| `type` | string | yes | | One of `string`, `int`, `float`, `bool`, `list`, `map`, `symbol`, `duration`. |
| `required` | bool | no | `false` | Whether an invocation must supply the argument. |
| `default` | value of `type` | no | | Used when the invocation omits the argument. |
| `symbol` | block, repeatable | no | | For `type = "symbol"` only: one legal value, with a required `description`. Declaring any closes the set. |

An `arg` has the shape of a resource `param`. These argument names are
rejected because they are already in scope inside the body: `arg`, `step`,
`properties`, `symbol`, `name`, `description`, `declared_args`, `steps`.

### probe

```wcl
probe "marker" {
  description = "Ready once the marker file exists"
  script = "probes/marker.ws"            // relative to the playbook directory
  param "path" {
    description = "File to look for"
    type = "string"
    required = true
  }
}
```

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| label | string | yes | | The probe name, the bare value of a step's `wait_for`. |
| `description` | string | yes | | One-line summary for the docs. |
| `script` | string | yes | | wscript file exporting `ready`, relative to the playbook directory. |
| `param` | block, repeatable | no | | Declared parameters: the package `param` shape (package.md), with `symbol` children. |

Rules:
- A playbook-local probe is referenced bare (`wait_for = "marker"`); the same
  block in a `package.wcl` is referenced qualified (package.md).
- It shares the bare-name namespace with playbook composites; a duplicate is a
  validation error.
- The script's `use` imports resolve against the playbook's `lib/`. The `ready`
  contract is in scripts.md and is checked at validate.

## Variables

The four sources, lowest precedence first. A later source overrides an earlier
one for the same name. The engine binds all of them into one flat scope.

| Source | Precedence | Notes |
|---|---|---|
| `vars` declaration | lowest | Expressions may reference gatherer results and other variables. |
| gatherer result | second | Bound under the gather label. |
| `--var-file file.wcl` | third | A flat `name = value` file. Each expression evaluates standalone and cannot reference other variables. Names must be identifiers. |
| `--var KEY=VALUE` | highest | Repeatable. KEY must be an identifier. VALUE parses as a WCL expression when it can (`count=3` is an int, `debug=true` a bool); otherwise it is a plain string. A flag without `=` is an error. |

```console
config-weave apply ./my-playbook baseline --var-file ./site.wcl --var count=3
```

Evaluation order:
1. Gather `params` evaluate first. They may reference `--var` and `--var-file`
   overrides, not gatherer results or any variable that depends on them.
2. All gathers run concurrently. Any failure aborts before the first step.
3. Conditions and properties evaluate lazily at run time against the full
   scope. A var no step references is never evaluated.

## Secrets

`secret("…")` is a WCL builtin, legal anywhere an expression is: a `vars`
entry, a step's `properties`, a gather's `params`, a `condition`. It is
playbook-only; in a `package.wcl` (including `test` and `scenario` blocks) it is
a validation error.

```wcl
vars {
  db_password = secret("hunter2")            // before: fails validation
  db_password = secret("CWENC1.a1B2....")    // after `secrets encrypt`
}
```

| Rule | Value |
|---|---|
| Encrypt in place | `config-weave secrets encrypt` rewrites only the call's own bytes; the rest of the file is untouched. |
| Blob | Starts with `CWENC1`. Argon2id key derivation, XChaCha20-Poly1305. |
| Un-encrypted call | Hard error from `check`, `apply`, `validate`, `test` and `docs`, decided by a syntactic scan, no password needed. |
| Password sources | Exactly one of `$CONFIG_WEAVE_PASSWORD`, `--password-stdin`, `--password-file PATH`. One trailing newline is stripped. |
| No prompt | A missing password on `check`, `apply` or `test` is exit status 2. A playbook with no `secret()` calls never asks. |
| `validate` and `docs` | Never need a password; the builtin returns an empty-string placeholder for type-checking. |
| Decryption timing | Every secret is decrypted up front, before any step, so a wrong password fails immediately. |
| One password per file | All secrets share one password. `secrets rekey` changes it and also encrypts any still-plaintext calls. `secrets decrypt` restores plaintext for editing. |
| Redaction | Decrypted values are masked to `***` in diagnostics, the NDJSON log, step messages, and script `log`/`print` output. Values shorter than 4 bytes are not masked. Docs show `secret(...)`, never the blob. |

## Gotchas

- Never write an import line; the engine appends `<weave/playbook.wcl>`.
- Every `playbook`, `play`, `container`, `step`, `composite`, `arg`, `probe`
  and `param` block needs a `description`. Only `gather` may omit it.
- Package resources and gatherers are always `pkg.name`. A bare `resource`
  means a composite declared in this playbook.
- `properties { url = url }` and `params { x = x }` are cycle errors: field
  names shadow variables of the same name.
- In a composite body read arguments as `args.name`, not bare.
- A composite body cannot see `vars`, gatherer results or `--var`; pass them in
  as properties.
- Symbols are `:name`, durations are bare literals like `30min`; quoting either
  is a type error.
- `requires` is "after", not "only if succeeded": a Skipped dependency releases
  its dependents.
- From the playbook, `requires` cannot reach a step inside a composite
  invocation; name the invoking step, which waits for the whole expansion.
- A `concurrency` value looser than the resource's class fails validation.
- A gather's `params` cannot use another gather's result.
- A step names exactly one of `resource` and `wait_for`. A wait step needs
  `timeout`, and its durations use `min` for minutes: `timeout = 10min`, never
  `10m`.
- Messages print durations as `4m12s`; that is output, not WCL.
- `check` never waits on a wait step, so a not-ready probe in check is Not
  Configured, not a hang.
- A `--var-file` entry cannot reference another variable; it evaluates
  standalone.
- A `secret("plaintext")` call that was never run through `secrets encrypt`
  fails validation in every command that loads the playbook.
