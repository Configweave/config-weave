# Add a package resource

**Purpose:** Declare a new resource in a package and implement its check/apply wscript script against the host API.

_Preconditions:_ A playbook with a pkgs/<name>/package.wcl already exists.

### 1. Declare the resource and its params

```wcl
resource "file_present" {
  description = "Ensure a file exists with the given content"
  script = "resources/file_present.wscript"
  concurrency = "parallel"
  param "path"    { description = "Absolute path"  type = "string"  required = true }
  param "content" { description = "File content"   type = "string"  default = "" }
}
```

Add a `resource` block to `package.wcl` with `script` (a path relative to the package dir) and a `param` per input. Params are validated against playbook properties at validation time.

### 2. Implement check() and apply()

```rust
use value
use fs
use path

fn check(params: Value) -> Result[CheckResult, string] {
    let p = params.get("path")?.as_string()?
    if !fs::exists(p) { return Ok(CheckResult::NotConfigured) }
    if fs::read(p)? == params.get("content")?.as_string()? {
        Ok(CheckResult::AlreadyConfigured)
    } else { Ok(CheckResult::NotConfigured) }
}

fn apply(params: Value) -> Result[ApplyResult, string] {
    let p = params.get("path")?.as_string()?
    fs::mkdir(path::parent(p))?
    fs::write(p, params.get("content")?.as_string()?)?
    Ok(ApplyResult::Success)
}
```

> [!WARNING]
> **The contract**
> `check` must never mutate; `apply` must converge so a re-check returns `AlreadyConfigured` — even in a fresh process.

Write the script under `resources/`. Export `check(params)` and `apply(params)`; import host modules with `use`. Run `config-weave wscripti` next to the scripts so the wscript LSP type-checks against the exact host surface.

### 3. Validate the script compiles

```console
$ config-weave validate ./my-playbook
ok
```

Run `config-weave validate` — its final stage compiles every wscript script against the host API, so a bad signature or unknown host function is a hard error.

> [!TIP]
> **Verification**
> `config-weave validate` exits 0 with the new resource's script compiled.

## Related

- [Packages](../references/concept_packages.md)

- [Resource & gatherer scripts](../references/concept_scripts.md)

- [Host API — cross-platform](../references/concept_hostapi.md)

[← All processes](../references/processes_ref.md)
