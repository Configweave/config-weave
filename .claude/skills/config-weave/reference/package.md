# package.wcl, the built-in `weave` package, and `config-weave pkg`

Schema source: `src/vocab/package.wcl`, served as the system import `<weave/package.wcl>`. The engine appends the import when it opens the file; a package never writes an import line.

## Directory layout

```text
my-playbook/
  playbook.wcl
  lib/                          # playbook-level helpers, visible to every package
  pkgs/
    core/
      package.wcl               # exactly one `package "core" { … }`
      lib/                      # package-level helpers; shadows the playbook's lib/
      resources/
        file_present.ws         # exports check() and apply()
      gatherers/
        os_info.ws              # exports gather()
      tests/
        file_present_verify.ws  # optional verify() for a test
      scenarios/                # optional run() drivers, plus labs/<name>/vmlab.wcl
```

| Rule | Value |
| --- | --- |
| Location | `<playbook>/pkgs/<name>/package.wcl` |
| Package name | The `package` label must equal the directory name under `pkgs/` |
| Reserved name | `weave` is the built-in package; a `pkgs/weave/` directory is rejected at load |
| Script paths | Every `script`, `verify`, `setup` and `lab` path is relative to the package directory |
| Script extension | `.ws` |
| Meaningful directories | Only `lib/` means anything to the engine; `resources/`, `gatherers/`, `tests/` are convention |
| Helpers | `use helpers` resolves a registered host module first, then `pkgs/<pkg>/lib/helpers.ws`, then `<playbook>/lib/helpers.ws`; `use "./shared.ws"` is relative to the importing script and carries its extension |
| Helper exports | Only the entry script exports entry points; a helper cannot supply `check` or `apply` |
| Validation | `config-weave validate` compiles every `lib/*.ws`, imported or not |
| Qualified names | A playbook refers to `core.file_present`, `core.os_info`, `core.site` |
| `secret()` | A validation error anywhere in a package; packages hold no playbook-specific values |

## Minimal complete package

```wcl
package "core" {
  description = "Core sample package"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.ws"
    returns "family" { description = "Kernel family: linux or windows" type = "string" }
  }

  resource "file_present" {
    description = "Ensure a file exists with the given content"
    script = "resources/file_present.ws"
    concurrency = "parallel"              // parallel (default) | exclusive | global

    param "path" {
      description = "Absolute path of the file"
      type = "string"                     // string | int | float | bool | list | map | symbol | duration
      required = true
    }
    param "content" {
      description = "File content"
      type = "string"
      default = ""
    }
  }

  test "file_present_converges" {
    description = "file_present creates the file and is idempotent"
    image = "debian:12"
    verify = "tests/file_present_verify.ws"
    step "create" {
      description = "Create a marker file"
      resource = "file_present"
      properties { path = "/var/tmp/weave-sample.txt"  content = "hello" }
    }
  }
}
```

The resource script behind it, in the `Result` form:

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
    let want = param_str(params, "content", "")
    let have = fs::read(p)?
    if have == want { Ok(CheckResult::AlreadyConfigured) } else { Ok(CheckResult::NotConfigured) }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let p = param_str(params, "path", "")
    log::info("writing " + p)
    fs::mkdir(path::parent(p))?
    fs::write(p, param_str(params, "content", ""))?
    Ok(ApplyResult::Success)
}
```

Entry-point signatures, result enums and the `Value` API: scripts.md.

## Blocks

This is the whole surface: every block and every field of `package.wcl`. Every `description` is required and enforced by the loader.

### package

The root block, exactly one per file. Resources and composites share one namespace, so a package may not declare both under the same name.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Package name; must match the directory name under `pkgs/` |
| `description` | string | yes | | One-line summary for the docs |
| `gatherer` | block, repeatable | no | none | Fact collectors |
| `resource` | block, repeatable | no | none | Units of desired state |
| `composite` | block, repeatable | no | none | Reusable blocks of steps |
| `test` | block, repeatable | no | none | Convergence tests |
| `scenario` | block, repeatable | no | none | Scripted multi-machine tests |

### gatherer

```wcl
gatherer "os_info" {
  description = "Operating system facts"
  script = "gatherers/os_info.ws"

  param "detail" {
    description = "How much to collect"
    type = "symbol"
    default = :basic
  }
  returns "init" {
    description = "The init system"
    type = "symbol"
    symbol "systemd" { description = "systemd" }
    symbol "openrc"  { description = "OpenRC" }
  }
}
```

The script exports `gather(params: Value) -> Value` or `gather(params: Value) -> Result[Value, string]`. A playbook `gather "os" { from = "core.os_info" }` runs it and binds the returned map to `os`, so `os.family` is usable in vars, conditions and properties. All gathers in a playbook run concurrently before any step; invocations with the same gatherer and canonicalised params run once; any gatherer failure aborts the run before the first step.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Gatherer name, referenced as `package.gatherer` |
| `description` | string | yes | | One-line summary for the docs |
| `script` | string | yes | | Path of the wscript file, relative to the package directory |
| `param` | block, repeatable | no | none | Declared parameters |
| `returns` | block, repeatable | no | none | Documented top-level keys of the returned map |

### returns

Documents one top-level key of the map a gatherer returns. The engine does not require the returned map to carry these keys, or only these keys. The one enforced case is `type = "symbol"`: its value binds into the playbook as a WCL symbol (compare as `init.init == :systemd`), and when `symbol` blocks are declared the returned value must be one of them. Only top-level keys are typed; nested maps stay plain data.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Key name in the returned map |
| `description` | string | yes | | What the key holds |
| `type` | string | yes | | One of the `param` types |
| `symbol` | block, repeatable | no | open set | For `type = "symbol"` only: the legal values |

### resource

The script exports `check(params: Value)` and `apply(params: Value)`, each returning the bare result enum or `Result[…, string]`. `check` reads the host and returns without writing; after a successful `apply` the engine re-runs `check` and requires `AlreadyConfigured`. A step invokes the resource as `package.resource` and supplies parameters in its `properties` block; the engine validates them against the `param` declarations, fills defaults, and passes one `Value` map.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Resource name, referenced as `package.resource` |
| `description` | string | yes | | One-line summary for the docs |
| `script` | string | yes | | Path of the wscript file, relative to the package directory |
| `concurrency` | string | no | `"parallel"` | Scheduling class: `parallel`, `exclusive` or `global` |
| `param` | block, repeatable | no | none | Declared parameters |

### param

```wcl
param "max_age" {
  description = "Refresh when the last update is older than this span"
  type = "duration"
  default = 24h
}

param "ensure" {
  description = "Whether the file should exist"
  type = "symbol"
  default = :present
  symbol "present" { description = "Create or update the file" }
  symbol "absent"  { description = "Delete the file" }
}
```

Validation checks each supplied value against the coarse type, applies `default` when the value is absent, rejects a missing `required` parameter, and rejects an undeclared property name.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Parameter name; the key in the script's `params` map |
| `description` | string | yes | | One-line summary for the docs |
| `type` | string | yes | | One of the types in the table below |
| `required` | bool | no | `false` | Whether a step or gather must supply it |
| `default` | value of `type` | no | none | Used when the parameter is omitted |
| `symbol` | block, repeatable | no | open set | For `type = "symbol"` only |

#### Param types

This is the whole set of legal `type` values, for `param`, `arg` and `returns`.

| `type` | Written in WCL as | Reaches the script as |
| --- | --- | --- |
| `string` | `"text"` | `Value::String` |
| `int` | `3` | `Value::Int` |
| `float` | `1.5` | `Value::Float` |
| `bool` | `true` | `Value::Bool` |
| `list` | `[3010, 1641]` | `Value::List` |
| `map` | `{ KEY: "v" }` | `Value::Map` |
| `symbol` | `:present` (never `"present"`) | `Value::String` holding `"present"` |
| `duration` | bare unit literal `30min`, `24h`, `5s` (never `"30m"`) | `Value::Int` of nanoseconds |

Duration suffixes: `ns`, `us`, `ms`, `s`, `min`, `h`, `d`. Minutes are `min` because `m` is metres.

### symbol

One legal value of a `symbol` parameter, argument or returns key. Declaring any closes the set: the `default`, every step property or gather param, every run-time value and a test `expect` value are checked against it. Declaring none leaves any token legal. The block is an error under any other `type`. Name it in the WCL-spellable form (`on_demand`, not `on-demand`); a script that needs another spelling translates it itself.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | The value, without the leading colon |
| `description` | string | yes | | What choosing this value means |

### composite

```wcl
composite "site" {
  description = "A directory with an index page"
  arg "root" {
    description = "Directory that holds the site"
    type = "string"
    required = true
  }

  step "dir" {
    description = "Site directory"
    resource = "directory"
    properties { path = args.root }
  }
  step "index" {
    description = "Index page"
    resource = "file_present"
    requires = ["dir"]
    properties { path = $"${args.root}/index.html" }
  }
}
```

A named, parameterised block of steps, invoked from a playbook step as `package.composite`. The loader expands each invocation into a container of ordinary steps, reported under `container/…/invocation/inner`. The body sees only its own arguments, never gatherer results, `vars` or `--var` overrides; pass a fact in as a property. Inside the body an unqualified `resource` names this package's own resource or composite. `requires` inside the body reaches only sibling steps of the same invocation. A `concurrency` on the invoking step tightens every step of the body. Nesting is capped at eight levels and a cycle of invocations is rejected by name.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Composite name; shares a namespace with this package's resources |
| `description` | string | yes | | One-line summary for the docs |
| `arg` | block, repeatable | no | none | Declared arguments |
| `step` | block, repeatable | no | none | The body |

Body step fields: `description`, `resource`, `condition`, `requires`, `concurrency`, `properties`. A body step may not carry `expect`; that field belongs to a test step.

### arg

Same shape as `param`, under its own block kind. Each argument binds twice in the body: bare (`root`) and as `args.root`. Use `args.`: a property field shadows a bare outer variable of the same name, so `properties { path = path }` is a self-reference cycle while `properties { path = args.path }` always works.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Argument name |
| `description` | string | yes | | One-line summary for the docs |
| `type` | string | yes | | One of the param types |
| `required` | bool | no | `false` | Whether an invocation must supply it |
| `default` | value of `type` | no | none | Used when the invocation omits it |
| `symbol` | block, repeatable | no | open set | For `type = "symbol"` only; declaring any closes the set |

### test

An isolated convergence test run by `config-weave test` in a disposable vmlab instance with the three-run protocol. Fields (`image` or `template`, `memory`, `group`, `setup`, `verify`, nested `step` and `gather`) and the expectation table: testlab.md.

### scenario

```wcl
scenario "ad_matrix" {
  description = "Forest root, member join and a second DC over real reboots"
  lab = "labs/ad"
  script = "scenarios/ad_matrix.ws"
}
```

A scripted multi-stage test over a vmlab lab, for a reboot mid-convergence or several machines. `lab` holds a `vmlab.wcl` declaring every VM; `script` exports `run(lab: Lab) -> bool` or `Result[bool, string]` and runs on the host through the `testlab` module. It compiles during validation and runs after the parallel test groups, one scenario at a time. Driving a lab: testlab.md.

| Field | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| label | string | yes | | Scenario name |
| `description` | string | yes | | One-line summary for the docs |
| `lab` | string | yes | | Directory holding the `vmlab.wcl`, relative to the package directory |
| `script` | string | yes | | Path of the driver script, relative to the package directory |

## Concurrency classes

This is the whole set.

| Class | Meaning |
| --- | --- |
| `parallel` (default) | No restriction. Any number of steps of this resource run alongside any other step. |
| `exclusive` | At most one step of this resource type runs at a time. Steps of other resources continue. The package-manager lock case. |
| `global` | The step runs alone. The scheduler drains every in-flight step, runs this one, then resumes. The reboot or kernel-update case. |

Tighten-only rule: a step may declare a class tighter than its resource (`parallel` < `exclusive` < `global`), and an invocation's class tightens every step of a composite body, but a step declaring a looser class than its resource fails validation. Declare the loosest correct class on the resource, because a resource defaulting to `exclusive` can never run two unrelated instances at once. A step whose script touches a shared lock declares `concurrency = "exclusive"` on the step. The scheduler honours the class for every step of the resource; a play with `parallel = false` runs in declaration order instead.

## The built-in `weave` package

Ships inside the binary (`src/builtin/`), loads through the ordinary package path, and shows in `validate`, `list` and `docs` like any other package. Both resources are escape hatches for imperative work; a real resource models desired state and belongs in a package. Both are `concurrency = "parallel"`.

### weave.execute

```wcl
step "install-tool" {
  description = "Install the tool if its binary is missing"
  resource = "weave.execute"
  properties {
    check = "test -x /usr/local/bin/tool"
    run   = "curl -fsSL https://example.com/tool -o /usr/local/bin/tool && chmod +x /usr/local/bin/tool"
    timeout = 5min
  }
}
```

The guard `check` exits 0 when the host is already converged (step reports `AlreadyConfigured`); any other status runs `run`. After `run`, the engine re-runs the guard as the re-check, and a guard still non-zero fails the step with "apply claimed success but the re-check disagrees". `run` exiting non-zero fails the step unless the status is listed in `reboot_on`, in which case the step reports `RebootRequired` and the play halts until the next run.

| Param | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| `check` | string | yes | | Guard script; exit 0 means already in the desired state |
| `run` | string | yes | | Action script, run only when the guard is non-zero; must make the guard exit 0 |
| `shell` | symbol | no | `:auto` | `:auto` (PowerShell on Windows, bash elsewhere), `:bash` (falls back to `sh`), `:powershell` (`-NoProfile -NonInteractive`) |
| `cwd` | string | no | `""` | Working directory for both scripts; empty means where config-weave was run from |
| `env` | map | no | none | Extra environment variables for both scripts, string to string |
| `timeout` | duration | no | `0s` | Kill either script after this long; zero or omitted means no limit; under one second rounds up to one |
| `reboot_on` | list | no | none | Exit statuses from `run` meaning success plus reboot needed; Windows installer convention is `[3010, 1641]` |

### weave.execute_once

Runs `run` once per host and records that it ran. `check` reports `AlreadyConfigured` when the record exists. The record is keyed by `id` alone: editing `run` does not run it again, changing `id` does. The record stores an ISO timestamp and a sha256 of the script text, for forensics only. `apply` writes the record before reporting `RebootRequired`, so a reboot cannot cause a second run. A non-zero exit not listed in `reboot_on` fails the step and writes no record.

| Platform | Record location |
| --- | --- |
| Linux and macOS | file `/var/lib/config-weave/once/<id>` |
| Windows | registry value `<id>` under `HKLM\Software\config-weave\Once` |
| `$CONFIG_WEAVE_STATE_DIR` set (any platform) | file `$CONFIG_WEAVE_STATE_DIR/once/<id>`; selects the file form on Windows too |

| Param | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| `id` | string | yes | | Name of the record; no `/`, `\`, `..`, `:`, `*`, `?` or `"` |
| `run` | string | yes | | The script to run once |
| `shell` | symbol | no | `:auto` | Same values as `weave.execute` |
| `cwd` | string | no | `""` | Working directory; empty means where config-weave was run from |
| `env` | map | no | none | Extra environment variables, string to string |
| `timeout` | duration | no | `0s` | Same rule as `weave.execute` |
| `reboot_on` | list | no | none | Exit statuses meaning success plus reboot needed; the record is written first |

Unix truncates an exit status to 0 to 255, so only Windows can report the four-digit installer codes in `reboot_on`.

## config-weave pkg

```console
config-weave pkg [--dir <PLAYBOOK>] <COMMAND>
```

`--dir` is the playbook directory, default `.`, accepted before or after the subcommand. Every subcommand shells out to the `git` binary, so ambient credentials work for private repositories. Exit status 0 on completion (including a `search` with no matches), 2 on any error; a repository that fails to sync during `add`, `update` or `search` is a warning when the operation can still complete.

| Subcommand | Does |
| --- | --- |
| `pkg add <PACKAGE>` | Syncs every registered repo, copies the first `<PACKAGE>/package.wcl` found (registration order; later matches reported as shadowed) into `pkgs/<PACKAGE>`, records the repo head commit. Fails when already recorded, when `pkgs/<PACKAGE>` exists but was not installed by `pkg add`, on an invalid name, or when no repo holds it. |
| `pkg remove <PACKAGE>` | Deletes `pkgs/<PACKAGE>` and its `repo.wcl` entry. Refuses a package not installed by `pkg add`. |
| `pkg update [PACKAGE]` | Re-syncs repos, re-copies installed packages (all, or one), records the new commit. |
| `pkg search <TERM>` | Syncs repos and prints packages whose name or description contains the term, case-insensitively, marking `[installed]` or `[installed from <repo>]`. |
| `pkg repo add [--branch <B>] [--subdir <S>] <NAME> <URL>` | Registers a repo and syncs it into `.repo-cache/<NAME>`. `--branch` defaults to the remote default; `--subdir` is the checkout subdirectory holding package directories, default the root. |
| `pkg repo remove <NAME>` | Unregisters a repo; packages installed from it stay in `pkgs/` and `repo.wcl`. |
| `pkg repo list` | Prints registered repos with URL, branch, subdir and cache state (`not synced`, `dirty`, or short head commit). Reads local state only. |

| File | Holds |
| --- | --- |
| `pkgs/repo.wcl` | Registered repositories, and every installed package with its source repo and exact commit. Tooling metadata only; the playbook loader never reads it, so a broken `repo.wcl` breaks `pkg` commands and nothing else. |
| `.repo-cache/<repo>/` | Shallow git clones of each registered repo, in the playbook directory. Add `.repo-cache/` to `.gitignore`; the command prints a reminder and never edits `.gitignore` itself. |
| `pkgs/<name>/` (installed) | A managed directory. `pkg update` re-copies it from the source, so local edits are lost. Keep local changes in a package of your own. |

Default repository: when `pkg add` or `pkg search` runs with no repository registered, the command seeds `stdlib` at `https://github.com/Configweave/config-weave-pkgs.git` with subdir `pkgs` and saves it. A `repo.wcl` listing at least one repository is respected as written. The copy skips `.git`, `node_modules`, `target`, `.vmlab` and every other dot-directory.

## Gotchas

- The `package` label and the `pkgs/<name>` directory name are the same string, and `weave` is taken.
- Every block's `description` is required, including `param`, `arg`, `symbol` and `returns`.
- A symbol value is written `:present`; `ensure = "present"` is a validation error even though the script would see the same text.
- A duration is a bare literal (`30min`); `"30m"` is a type error, and the script receives nanoseconds as `Int`.
- A `symbol` block under a non-symbol `type` is an error; declaring one closes the set for defaults, properties, run-time values and test expectations.
- A `returns` key of type `symbol` binds as a WCL symbol; comparing it against the string `"systemd"` is silently false, so compare with `:systemd`.
- Inside a composite body read arguments as `args.name`; `properties { path = path }` is a self-reference cycle.
- A composite body sees no gatherer results or vars; pass them in as arguments.
- A step may tighten `concurrency` and never loosen it; declaring `parallel` on a step of an `exclusive` resource fails validation.
- `weave.execute` fails when `run` succeeds but the guard still exits non-zero; make the action satisfy the guard.
- `weave.execute_once` ignores edits to `run`; change `id` to run again, and set `$CONFIG_WEAVE_STATE_DIR` to test without touching `/var/lib` or the registry.
- A `secret()` call in a package is a validation error; secrets belong in the playbook.
- Local edits inside an installed `pkgs/<name>/` are overwritten by `pkg update`.
