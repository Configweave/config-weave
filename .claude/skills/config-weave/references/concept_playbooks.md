# Playbooks

_playbook.wcl — plays of steps, gather blocks, vars, and the scheduling semantics._

A playbook is a directory containing `playbook.wcl`, an optional `lib/` of shared
wscript code, and `pkgs/<name>/` packages (see [Packages](../references/concept_packages.md)). The
engine appends the system import `<weave/playbook.wcl>` automatically — \*\*never write
import lines\*\*.


## Full example

```wcl
playbook "Sample Baseline" {
  description = "Exercises the model loader, validation and execution"
  version = "1.0.0"                      // optional, default "0.0.0"

  gather "os" {                          // label = the variable the result lands in
    description = "Operating system facts"
    from = "core.os_info"                // package.gatherer
    params {                             // optional, validated against gatherer's params
      depth = 2
    }
  }

  vars {
    work_root = "/tmp/config-weave-sample"
    is_linux = os.family == "linux"      // may reference gatherer results
    marker_a = $"${work_root}/a.txt"     // WCL string interpolation: $"...${expr}..."
  }

  play "baseline" {
    description = "Create marker files in order"
    // parallel = true is the default; false runs steps in declaration order

    step "make-a" {
      description = "Create the first marker file"
      resource = "core.file_present"     // package.resource
      condition = is_linux               // optional bool expr; false => Skipped
      properties {                       // validated against the resource's declared params
        path = marker_a
        content = "alpha"
      }
    }

    container "secondary" {              // grouping for organisation/docs; nestable
      description = "Files that depend on the first"
      // condition here would apply to all children

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

## Block reference

| Block | Fields | Notes |
| --- | --- | --- |
| `playbook "name"` | `description` (required), `version` (default `"0.0.0"`), `gather*`, `vars?`, `play*` | one per file |
| `gather "label"` | `description?`, `from` (required, `pkg.gatherer`), `params?` | label becomes the variable holding the result |
| `vars` | free-form `name = expr` | expressions may reference gatherer results and other vars |
| `play "name"` | `description` (required), `parallel` (default `true`), `step*`, `container*` | `parallel = false` = strict declaration order |
| `container "name"` | `description` (required), `condition?`, `step*`, `container*` | condition applies to all children |
| `step "name"` | `description` (required), `resource` (required), `condition?`, `requires?`, `concurrency?`, `properties?` | `concurrency` may \*tighten\* the resource's class, never loosen |
| `properties` / `params` | free-form `name = expr` | validated against the resource/gatherer `param` declarations |

`description` is mandatory wherever shown as required — the loader enforces it.

## Variables

Precedence (lowest → highest): **vars declaration → gatherer result → `--var-file` → `--var`**.

- `--var KEY=VALUE` parses VALUE as a WCL expression when possible (`--var count=3` is
  an int), falling back to a plain string. Repeatable.
- `--var-file file.wcl` is a flat `name = value` collection; expressions evaluate
  standalone and **cannot reference other variables**.
- **Gather params evaluate before variables resolve**: they may reference `--var` /
  `--var-file` overrides, but not gatherer results or vars that depend on them.
- Gatherer invocations all run concurrently and are deduplicated by
  `(gatherer, canonicalized params)`; any gatherer failure aborts before step execution.
- Conditions and properties evaluate lazily at run time against the full scope.


> [!WARNING]
> **Shadowing pitfall**
> Property/params block fields **shadow** outer variables: `url = url` inside a `properties` block is a self-reference cycle error. Use distinct names (`tool_url = ...` then `url = tool_url`).

## Scheduling semantics

- `requires` is **ordering, not a success demand**: a \*skipped\* dependency does not
  block a dependent; an \*errored\* or \*not-run\* dependency blocks dependents (`not run`)
  in apply mode.
- In a `parallel` play the DAG scheduler dispatches steps as dependencies complete,
  subject to each resource's concurrency class (`parallel` / `exclusive` / `global` —
  see [Packages](../references/concept_packages.md)).
- Steps left undispatched when a run halts report **not run**.
- The per-step check → apply → re-check lifecycle is in [Scripts](../references/concept_scripts.md); CLI
  flags and exit codes are in the CLI reference.


## Related

- [Packages](../references/concept_packages.md)

- [Resource & gatherer scripts](../references/concept_scripts.md)

- [Testing & the testlab](../references/concept_testing.md)

[← All concepts](../references/concepts_ref.md)
