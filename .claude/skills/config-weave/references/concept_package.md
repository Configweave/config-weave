# Package

_A bundle of resources, gatherers and tests under pkgs/<name>/._

A package bundles **resources**, **gatherers** and **tests** under `pkgs/<name>/`
inside a playbook directory. Its name qualifies references from playbooks
(`core.file_present`). The engine appends the system import `<weave/package.wcl>`
automatically.


## Directory layout

```text
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

> [!NOTE]
> **lib/ caveat**
> wscript v1 has no script-to-script imports. `lib/` folders are compiled standalone during validation but **cannot be imported** by resource scripts yet.

## Full example

```wcl
package "core" {
  description = "Core sample package"

  gatherer "os_info" {
    description = "Report basic operating system facts"
    script = "gatherers/os_info.wscript"     // path relative to the package dir
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

  test "file_present_converges" {         // see the Testlab concept
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

The block-by-block field reference is in [Package block reference](../references/fact_package_blocks.md). Properties/params in playbooks are validated against the `param` declarations at validation time: unknown keys, missing required params and type mismatches are errors; declared defaults are applied before the script runs.

## Related

- [Playbook](../references/concept_playbook.md)

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

- [Testlab](../references/concept_testlab.md)

- [package.wcl](../references/entity_package_wcl.md)

- [Package block reference](../references/fact_package_blocks.md)

[← Back to SKILL.md](../SKILL.md)
