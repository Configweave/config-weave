# Packages (`package.wcl`)

A package bundles resources, gatherers and tests under `pkgs/<name>/` inside a playbook
directory. The engine appends the system import `<weave/package.wcl>` automatically.

## Directory layout

```
my-playbook/
  playbook.wcl
  lib/                          # playbook-level shared wscript (see note below)
  pkgs/
    core/
      package.wcl
      lib/                      # package-level shared wscript
      resources/
        file_present.wscript       # exports check() and apply()
      gatherers/
        os_info.wscript            # exports gather()
      tests/
        file_present_verify.wscript  # optional verify() for tests
```

> **lib/ caveat:** wscript v1 has no script-to-script imports. `lib/` folders are compiled
> standalone during validation but **cannot be imported** by resource scripts yet.

## Full example

```wcl
package "core" {
  description = "Core sample package"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.wscript"     // path relative to the package dir
    // param blocks allowed here too, same shape as on resources
  }

  resource "file_present" {
    description = "Ensure a file exists with the given content"
    script = "resources/file_present.wscript"
    concurrency = "parallel"              // parallel (default) | exclusive | global

    param "path" {
      description = "Absolute path of the file"
      type = "string"                     // string | int | float | bool | list | map
      required = true                     // default false
    }
    param "content" {
      description = "File content"
      type = "string"
      default = ""                        // default value when omitted
    }
  }

  test "file_present_converges" {         // see testing.md
    description = "file_present creates the file and is idempotent"
    image = "debian:12"
    verify = "tests/file_present_verify.wscript"
    step "create" {
      description = "Create a marker file"
      resource = "file_present"
      properties { path = "/var/tmp/weave-sample.txt"  content = "hello" }
    }
  }
}
```

## Block reference

| Block | Fields | Notes |
|---|---|---|
| `package "name"` | `description` (required), `gatherer*`, `resource*`, `test*` | name is how playbooks qualify refs: `core.file_present` |
| `gatherer "name"` | `description`, `script`, `param*` | script exports `gather(params: Value) -> Value` |
| `resource "name"` | `description`, `script`, `concurrency` (default `"parallel"`), `param*` | script exports `check()` + `apply()` |
| `param "name"` | `description`, `type`, `required` (default `false`), `default?` | coarse types: `string\|int\|float\|bool\|list\|map` |
| `test "name"` | see `testing.md` | run by `config-weave test` in disposable containers |

Properties/params in playbooks are validated against these `param` declarations at
validation time: unknown keys, missing required params and type mismatches are errors;
declared defaults are applied before the script runs.

## Concurrency classes

Declared on the resource; a step may *tighten* its resource's class but never loosen it.

| Class | Meaning |
|---|---|
| `parallel` (default) | no restriction |
| `exclusive` | at most one step using this resource type at a time (the apt/MSI lock case) |
| `global` | step runs completely alone: scheduler drains in-flight steps, runs solo, resumes |

Writing the wscript scripts themselves (entry-point contracts, lifecycle, params access) is
covered in `scripts.md`.
