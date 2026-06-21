# config-weave — CLI reference

Single-binary configuration management — validate, check, and apply WCL playbooks, test packages in disposable instances, and scaffold/emit authoring aids.

## Global switches

| Switch | Value | Description |
| --- | --- | --- |
| --var | KEY=VALUE | Override a playbook variable. Repeatable. VALUE parses as a WCL expression when possible, else a plain string. |
| --var-file | PATH | Merge a WCL file's top-level name = value pairs into scope (evaluate standalone; cannot reference other variables). |
| --jobs | N | Worker pool size (default min(cpu_count, 8)); forwarded into test instances. |
| --continue-on-error | — | Continue dispatching steps after an Error (check/apply). |
| --json | — | JSON output: a single schema-stable object on stdout at completion; script log output never goes to stdout. |
| --no-color | — | Plain ASCII output (auto-selected when stdout is not a TTY). |
| --log-file | PATH | Enable NDJSON file logging (independent of terminal mode). |
| --log-level | LEVEL | File log level (independent of terminal verbosity), default info. |
| -v, --verbose | — | Increase terminal verbosity (repeatable: -v, -vv, -vvv). |

## config-weave validate

Full validation pipeline (WCL syntax, schema, refs, DAG, wscript compilation of every script), no execution.

| Argument | Required | Description |
| --- | --- | --- |
| playbook-dir | required | The playbook directory (contains playbook.wcl). |

```console
config-weave validate ./my-playbook
```

## config-weave check

Report configuration status of all steps in a play (never mutates).

| Argument | Required | Description |
| --- | --- | --- |
| playbook-dir | required | The playbook directory. |
| play | required | The play to check. |

```console
config-weave check ./my-playbook baseline
```

## config-weave apply

Apply all unconfigured steps in a play (converge the machine), then re-check each.

| Argument | Required | Description |
| --- | --- | --- |
| playbook-dir | required | The playbook directory. |
| play | required | The play to apply. |

```console
config-weave apply ./my-playbook baseline
```

## config-weave list

List all plays defined in the playbook.

| Argument | Required | Description |
| --- | --- | --- |
| playbook-dir | required | The playbook directory. |

```console
config-weave list ./my-playbook
```

## config-weave test

Run package convergence tests in disposable instances (docker containers or vmlab VMs) using the three-run protocol.

| Argument | Required | Description |
| --- | --- | --- |
| playbook-dir | required | The playbook directory. |
| filter | optional | Limit to a package or one test: `pkg` or `pkg:test`. |

| Switch | Value | Description |
| --- | --- | --- |
| --backend | NAME | Override every test's backend (docker or vmlab). |
| --image | IMAGE | Run every test against this image instead of its own. |
| --keep | — | Leave instances running for post-mortem debugging (handle reported). |
| --binary | PATH | Static linux config-weave binary to copy into instances. |
| --binary-windows | PATH | Windows config-weave binary for windows vmlab guests. |

```console
config-weave test ./my-playbook core:file_present_converges
```

## config-weave docs

Generate wdoc documentation for the playbook (shares the validation pipeline).

| Argument | Required | Description |
| --- | --- | --- |
| playbook-dir | required | The playbook directory. |
| outdir | optional | Output directory (default <dir>/docs/). |

```console
config-weave docs ./my-playbook
```

## config-weave wscripti

Emit weave.wscripti (the host API interface) plus a starter wscript.toml so editors and the wscript LSP type-check scripts against the exact host surface.

| Argument | Required | Description |
| --- | --- | --- |
| outdir | optional | Output directory (default: cwd). |

```console
config-weave wscripti ./my-playbook/pkgs/core/resources
```

## config-weave init

Scaffold a skeleton playbook with an example package, resource and gatherer.

| Argument | Required | Description |
| --- | --- | --- |
| dir | required | Destination directory for the new playbook. |

```console
config-weave init ./my-playbook
```

## config-weave version

Print version information.

```console
config-weave version
```
