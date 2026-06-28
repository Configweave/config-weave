# shell

_host module_

Run external commands; opts is required; returns CmdOutput. Replaces wscript-std's process module.

`use shell` — external commands (replaces wscript-std's `process`, which is not
registered). All take `(cmd_or_script: string, opts: Value)` and return
`Result[CmdOutput, string]`. **opts is required** (wscript has fixed arity): pass
`Value::Null` for defaults or a `Value::Map` with `cwd` (string), `env` (map),
`timeout` (int/float secs), `stdin` (string). Timeout kills the child and returns
`Err`.


| function | behaviour |
| --- | --- |
| `run` | splits cmd with shell-words, executes the program **directly — no shell interpretation** (no globs, pipes, `$VAR`) |
| `run_streaming` | like `run`, but streams output lines through `log` live (stdout → info, stderr → warn) — for long installs |
| `bash` | `bash -c script` (falls back to `sh`) — the shell-features escape hatch |
| `powershell` | tries `powershell` then `pwsh` with `-NoProfile -NonInteractive`; works on Linux with PowerShell Core |

### CmdOutput

```rust
struct CmdOutput { stdout: string, stderr: string, code: int, success: bool }

use shell
use value
let out = shell::run("systemctl is-active nginx", Value::Null)?
if !out.success { return Ok(CheckResult::NotConfigured) }
```

> [!NOTE]
> **Non-zero exit is not an Err**
> Inspect `out.success`/`out.code`. `Err` means the command could not run (spawn failure, timeout).

## Related

- [Host API](../references/concept_host_api.md)

- [Value](../references/entity_value_type.md)

- [log](../references/entity_log_module.md)

[← Back to SKILL.md](../SKILL.md)
