# log

_host module_

Structured logging with step context attached — debug/info/warn/error.

`use log` — structured logging; messages carry step context into the terminal output and the NDJSON file log. Raw `print`/`println` are redirected into `log::info` (stdout stays clean for `--json`); `shell::run_streaming` pipes a long command's output through `log` live.

| function | signature |
| --- | --- |
| `debug` | `(string)` |
| `info` | `(string)` |
| `warn` | `(string)` |
| `error` | `(string)` |

## Related

- [Host API](../references/concept_host_api.md)

- [Step lifecycle](../references/concept_step_lifecycle.md)

[← Back to SKILL.md](../SKILL.md)
