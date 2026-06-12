# Host API — cross-platform modules

The wisp surface config-weave registers for all scripts (resources, gatherers, verify).
Identical on every platform: foreign-platform functions exist everywhere and return
runtime errors off their platform. Windows-only modules (`registry`, `service`, `com`)
are in `hostapi-windows.md`. Authoritative source: `config-weave wispi` →
`weave.wispi`, generated from `src/hostapi/*.rs`.

Import with `use <module>`. All fallible functions return `Result[…, string]` and
compose with `?`.

## `log` — structured logging (step context attached)

`debug(msg)` · `info(msg)` · `warn(msg)` · `error(msg)` — all `(string)`.
Raw `print`/`println` also route into `log::info`.

## `fs` — file IO (richer than wisp-std's; replaces it)

| function | signature | notes |
|---|---|---|
| `read` | `(path) -> Result[string, string]` | text |
| `read_bytes` | `(path) -> Result[List[int], string]` | |
| `write` / `append` | `(path, content) -> Result[unit, string]` | write replaces; append creates if absent |
| `write_bytes` | `(path, List[int]) -> Result[unit, string]` | |
| `copy` / `move` | `(from, to) -> Result[unit, string]` | move = rename, works on dirs |
| `delete` | `(path) -> Result[unit, string]` | file or symlink |
| `delete_dir` | `(path) -> Result[unit, string]` | recursive |
| `mkdir` | `(path) -> Result[unit, string]` | creates missing parents |
| `exists` / `is_file` / `is_dir` | `(path) -> bool` | |
| `list_dir` | `(path) -> Result[List[string], string]` | sorted names |
| `metadata` | `(path) -> Result[Value, string]` | map: size, modified (unix secs), readonly, is_file, is_dir, is_symlink, mode (unix bits; 0 elsewhere) |
| `glob` | `(pattern) -> Result[List[string], string]` | sorted |
| `temp_file` / `temp_dir` | `() -> Result[string, string]` | fresh path returned |
| `symlink` | `(target, link) -> Result[unit, string]` | |
| `read_link` | `(path) -> Result[string, string]` | |

## `path` — pure path-string manipulation, no IO

`join(a, b) -> string` · `parent(p) -> string` (empty at root) ·
`filename(p) -> string` · `extension(p) -> string` (no dot) ·
`normalize(p) -> string` (lexical `.`/`..`) ·
`absolutize(p) -> Result[string, string]` (against cwd, then normalize).

## `shell` — external commands

All take `(cmd_or_script: string, opts: Value)` and return `Result[CmdOutput, string]`.
**opts is required** (wisp has fixed arity): pass `Value::Null` for defaults or a
`Value::Map` with `cwd` (string), `env` (map of strings), `timeout` (int/float secs),
`stdin` (string). Timeout kills the child and returns `Err`.

| function | behaviour |
|---|---|
| `run` | splits cmd with shell-words, executes the program **directly — no shell interpretation** (no globs, pipes, `$VAR`) |
| `run_streaming` | like `run`, but streams output lines through `log` live (stdout → info, stderr → warn) — for long installs |
| `bash` | `bash -c script` (falls back to `sh`) — the shell-features escape hatch |
| `powershell` | tries `powershell` then `pwsh` with `-NoProfile -NonInteractive`; works on Linux with PowerShell Core |

```rust
struct CmdOutput { stdout: string, stderr: string, code: int, success: bool }
```

```rust
use shell
use value
let out = shell::run("systemctl is-active nginx", Value::Null)?
if !out.success { return Ok(CheckResult::NotConfigured) }
```

A non-zero exit is **not** an `Err` — inspect `out.success`/`out.code`. `Err` means the
command could not run (spawn failure, timeout).

## `http` — HTTP client (rustls; no system TLS dependency)

opts is required: `Value::Null` or a map of `headers` (map), `timeout` (secs),
`redirects` (bool, default true).

| function | signature |
|---|---|
| `get` | `(url, opts) -> Result[HttpResponse, string]` |
| `post` | `(url, body: string, opts) -> Result[HttpResponse, string]` |
| `download` | `(url, dest_path, opts) -> Result[int, string]` — returns byte count |

```rust
struct HttpResponse { status: int, body: string, headers: Map[string, string] }
```

## `hash` — digests (hex output)

`sha256(s)` / `sha512(s)` / `md5(s)` — `(string) -> string`;
`sha256_file(p)` / `sha512_file(p)` / `md5_file(p)` — `(path) -> Result[string, string]`.
MD5 is legacy-interop only.

## `archive` — extraction (no external tar/unzip needed)

`extract_zip(archive, dest)` · `extract_tar_gz(archive, dest)` ·
`extract(archive, dest)` (auto by extension: .zip, .tar.gz, .tgz) — all
`-> Result[int, string]` returning the entry count.

## `env` — process environment and host identity

`get(name) -> Option[string]` · `set(name, value)` · `unset(name)` ·
`path_split(value) -> List[string]` / `path_join(parts) -> Result[string, string]`
(platform PATH separator) · `hostname() -> string` · `current_user() -> string` ·
`home_dir() -> string` · `is_elevated() -> bool` (root/Administrator).

## `sys` — OS and hardware facts (gatherer fodder)

`family() -> string` (linux/windows/macos) · `os_name()` (distro on Linux) ·
`os_version()` · `kernel_version()` · `arch()` (x86_64, aarch64, …) ·
`cpu_count() -> int` · `total_memory() -> int` / `available_memory() -> int` (bytes).

## `data` — INI (JSON/TOML live in the `json`/`toml` modules, see `wisp-stdlib.md`)

`ini_parse(text) -> Result[Value, string]` — map of sections, global keys under `""`;
`ini_serialize(map) -> Result[string, string]`.
