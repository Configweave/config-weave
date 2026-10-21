# Host API: every module a wscript script can call

The host API is the set of wscript modules config-weave registers for every
script it runs: resource `check`/`apply`, gatherers, verify scripts and scenario
drivers. A script reaches the host through these modules and nothing else. This
file is the whole surface: a module, function, type or method absent here does
not exist. Names and signatures follow `weave.wscripti`, the interface
`config-weave wscripti` emits from the same Rust modules the binary registers.

## Rules that apply to every module

- **Import:** `use <module>` (`use fs`, `use shell`). A bare `use name` resolves
  to a registered host module first, so `use fs` always means the host module
  even when a `lib/fs.ws` exists.
- **Ambient types:** `Value`, `CheckResult`, `ApplyResult`, `CmdOutput`,
  `HttpResponse` and `ComObject` need no `use`. (`use value` is harmless and
  appears in examples.)
- **Error convention:** every fallible function returns `Result[T, string]`.
  The `Err` string names the path, command or key and the operating-system
  error. Propagate with `?` from an entry point that returns
  `Result[..., string]`; the step then reports Error with that message.
- **Platform rule:** every module is registered on every platform, so a
  playbook compiles and validates identically everywhere. The four
  Windows-only modules (`registry`, `acl`, `service`, `com`) exist on Linux and macOS
  and every call there returns `Err("the '<module>' module is only available on Windows…")`.
  Guard such calls with a step `condition` (`os.family == "windows"`) or in the
  script with `sys::family() == "windows"`.
- **Fixed arity:** wscript functions have no optional parameters. Where a
  function takes an options `Value`, the argument is required: pass
  `Value::Null` for defaults or a `Value::Map` with only the listed keys. An
  unknown key is an `Err`.
- **stdout:** `print` and `println` in a script are routed into `log::info`,
  so stdout stays clean for `--json`.
- **Faults:** a runtime fault (an invalid regex pattern, an `unwrap()` on
  `None`, an index out of range) is not an `Err`. It ends the step with Error
  status.

### Module index

This is the whole set of registered modules.

| Module | Purpose | Platform | Source |
|---|---|---|---|
| `log` | structured logging at a level | all | config-weave |
| `fs` | file and directory IO | all | config-weave (replaces wscript-std `fs`) |
| `path` | pure path-string manipulation | all | config-weave |
| `shell` | run external commands | all | config-weave (replaces wscript-std `process`) |
| `http` | HTTP client (rustls, no system TLS) | all | config-weave |
| `hash` | SHA-256, SHA-512, MD5 digests | all | config-weave |
| `encoding` | base64, UTF-8 and UTF-16LE bytes | all | config-weave |
| `archive` | extract zip and tar.gz | all | config-weave |
| `env` | process environment and host identity | all | config-weave |
| `sys` | OS and hardware facts | all | config-weave |
| `data` | INI parse and serialize | all | config-weave |
| `template` | Tera template rendering | all | config-weave |
| `time` | clocks, sleep, ISO formatting | all | wscript-std |
| `json` | JSON parse and serialize | all | wscript-std |
| `toml` | TOML parse and serialize | all | wscript-std |
| `xml` | XML parse and serialize | all | wscript-std |
| `regex` | regular expressions | all | wscript-std |
| `value` | the shared `Value` type (no functions) | all | wscript-std |
| `registry` | Windows registry | Windows only | config-weave |
| `acl` | owner and DACL of files, directories, registry keys | Windows only | config-weave |
| `service` | Windows services via the SCM | Windows only | config-weave |
| `com` | COM via IDispatch, WMI queries | Windows only | config-weave |
| `testlab` | scenario driver handles (`Lab`, `Machine`) | scenario scripts only | config-weave |

`testlab` is registered only in the context scenario driver scripts compile
and run against. Every other script sees the twenty-two modules above it.

### The Value type

`Value` is the dynamic type every entry point receives and every data module
returns. Its variants and methods, from `weave.wscripti`:

```rust
enum Value { Null, Bool(bool), Int(int), Float(float), String(string), List(List[Value]), Map(Map[string, Value]) }

impl Value {
    fn get(self, key: string) -> Option[Value]      // map lookup
    fn at(self, idx: int) -> Option[Value]          // list index
    fn keys(self) -> List[string]
    fn len(self) -> int
    fn is_null(self) -> bool
    fn as_bool(self) -> Option[bool]
    fn as_int(self) -> Option[int]
    fn as_float(self) -> Option[float]
    fn as_string(self) -> Option[string]
    fn as_list(self) -> Option[List[Value]]
    fn as_map(self) -> Option[Map[string, Value]]
}
```

Build a map literal as `Value::Map(#{ "key": Value::String("x") })` and a list
as `Value::List([Value::Int(1)])`. Result enums (`CheckResult`, `ApplyResult`)
and entry-point signatures: scripts.md.

## log

`use log`. Structured logging with the step's context attached. Cross-platform,
never fails. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `debug` | `log::debug(message: string)` | values inspected while deciding |
| `info` | `log::info(message: string)` | progress a reader wants; `print`/`println` land here |
| `warn` | `log::warn(message: string)` | something unexpected the step tolerated |
| `error` | `log::error(message: string)` | a failure the step is about to report |

The terminal shows messages at or above the `-v` verbosity. The NDJSON file
log has its own level from `--log-level`, so a debug line can reach the file
without reaching the terminal. `shell::run_streaming` pipes command output
through this module live.

## fs

`use fs`. File IO. Cross-platform. Path strings pass to the operating system
unchanged. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `read` | `fs::read(path: string) -> Result[string, string]` | file as text |
| `read_bytes` | `fs::read_bytes(path: string) -> Result[List[int], string]` | raw bytes as ints |
| `read_with` | `fs::read_with(path: string, opts: Value) -> Result[string, string]` | `read` with the `write_with` options `{follow_symlinks: bool}`; `false` refuses a final symlink with `Err` (Unix: `O_NOFOLLOW`, race-free; Windows: check then open) |
| `read_bytes_with` | `fs::read_bytes_with(path: string, opts: Value) -> Result[List[int], string]` | `read_bytes` with the same options as `read_with` |
| `write` | `fs::write(path: string, content: string) -> Result[unit, string]` | replaces contents |
| `write_bytes` | `fs::write_bytes(path: string, bytes: List[int]) -> Result[unit, string]` | replaces contents |
| `write_with` | `fs::write_with(path: string, content: string, opts: Value) -> Result[unit, string]` | `write` with options `{follow_symlinks: bool}` (default `true`; `Value::Null` = `write`); `false` refuses a final symlink with `Err` and leaves its target untouched (Unix: `O_NOFOLLOW`, race-free; Windows: check then open, not atomic); unknown key is `Err` |
| `write_bytes_with` | `fs::write_bytes_with(path: string, bytes: List[int], opts: Value) -> Result[unit, string]` | `write_bytes` with the same options as `write_with` |
| `append` | `fs::append(path: string, content: string) -> Result[unit, string]` | creates the file when absent; `write`, `write_bytes` and `append` follow a final symlink |
| `copy` | `fs::copy(from: string, to: string) -> Result[unit, string]` | one file |
| `move` | `fs::move(from: string, to: string) -> Result[unit, string]` | renames a file or a directory |
| `delete` | `fs::delete(path: string) -> Result[unit, string]` | a file or a symlink |
| `delete_dir` | `fs::delete_dir(path: string) -> Result[unit, string]` | a directory and everything in it |
| `mkdir` | `fs::mkdir(path: string) -> Result[unit, string]` | creates missing parents |
| `exists` | `fs::exists(path: string) -> bool` | plain bool, unreadable path answers `false`; follows symlinks, so a dangling link answers `false` |
| `lexists` | `fs::lexists(path: string) -> bool` | does not follow a final symlink; a dangling link answers `true` |
| `is_file` | `fs::is_file(path: string) -> bool` | plain bool; follows symlinks |
| `is_dir` | `fs::is_dir(path: string) -> bool` | plain bool; follows symlinks |
| `list_dir` | `fs::list_dir(path: string) -> Result[List[string], string]` | entry names, sorted |
| `metadata` | `fs::metadata(path: string) -> Result[Value, string]` | map with the keys below; does not follow a final symlink (describes the link itself) |
| `glob` | `fs::glob(pattern: string) -> Result[List[string], string]` | matching paths, sorted |
| `temp_file` | `fs::temp_file() -> Result[string, string]` | creates a fresh file, returns its path |
| `temp_dir` | `fs::temp_dir() -> Result[string, string]` | creates a fresh directory, returns its path |
| `symlink` | `fs::symlink(target: string, link: string) -> Result[unit, string]` | link at `link` pointing to `target` |
| `read_link` | `fs::read_link(path: string) -> Result[string, string]` | the target of a symlink |
| `chown` | `fs::chown(path: string, owner: Value, group: Value) -> Result[unit, string]` | Unix only (`Err` on Windows); each of `owner`/`group` is `Value::String(name)`, `Value::Int(id)` or `Value::Null` to leave it unchanged; unknown name is `Err("unknown owner '<name>'")`; does not follow a final symlink |
| `chmod` | `fs::chmod(path: string, mode: Value) -> Result[unit, string]` | Unix only (`Err` on Windows); `mode` is `Value::Int(448)` (= `0o700`) or `Value::String` of octal digits (`"700"`, `"0700"`); other values or a mode above `0o7777` are `Err`; refuses a final symlink with `Err` instead of following it |

### `fs::metadata` map keys

| Key | Type | Meaning |
|---|---|---|
| `size` | int | size in bytes |
| `modified` | int | modification time, seconds since the Unix epoch |
| `readonly` | bool | the read-only attribute |
| `is_file` | bool | regular file |
| `is_dir` | bool | directory |
| `is_symlink` | bool | symlink |
| `mode` | int | Unix permission bits (`st_mode & 0o7777`, no file-type bits: `0o644` reads as `420`), `0` on other platforms |
| `uid` | int | Unix owner user id, `Value::Null` on other platforms |
| `gid` | int | Unix owner group id, `Value::Null` on other platforms |

Read them with the `Value` accessors:
`meta.get("modified").unwrap().as_int().unwrap()`.

## path

`use path`. Pure path-string manipulation using the running platform's
separator. No IO except `absolutize`. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `join` | `path::join(a: string, b: string) -> string` | two segments only |
| `parent` | `path::parent(p: string) -> string` | empty string at the root |
| `filename` | `path::filename(p: string) -> string` | final component |
| `extension` | `path::extension(p: string) -> string` | without the dot, empty when none |
| `normalize` | `path::normalize(p: string) -> string` | resolves `.` and `..` lexically, symlinks not followed |
| `absolutize` | `path::absolutize(p: string) -> Result[string, string]` | against the current directory, then normalizes |

## shell

`use shell`. Runs external commands. Cross-platform. Every function takes a
command or script string and an options `Value`, and returns
`Result[CmdOutput, string]`. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `run` | `shell::run(cmd: string, opts: Value) -> Result[CmdOutput, string]` | splits `cmd` with shell-words and executes the program directly; globs, pipes and `$VAR` are literal text |
| `run_streaming` | `shell::run_streaming(cmd: string, opts: Value) -> Result[CmdOutput, string]` | as `run`, and streams each line through `log` as it arrives: stdout at info, stderr at warn |
| `bash` | `shell::bash(script: string, opts: Value) -> Result[CmdOutput, string]` | `bash -c`, falls back to `sh`; the way to use pipes, globs and expansion |
| `powershell` | `shell::powershell(script: string, opts: Value) -> Result[CmdOutput, string]` | `powershell`, falls back to `pwsh`, with `-NoProfile -NonInteractive`; works on Linux with PowerShell Core. The script goes through a private temp file (deleted after the run), never the command line; exit codes, output and stdin (`$input`, `[Console]::In`) behave as with `-Command`, except that a script-level `return` while `$?` is false exits 0 (see below) |

`shell::powershell` runs the script as a dot-sourced script block followed by
`if ($?) { exit 0 } else { exit 1 }`. Measured on Windows PowerShell 5.1 and
pwsh 7.4, exit codes match `-Command <script>` in every case but one: a
script-level `return` while `$?` is false exits **0** where `-Command` exited 1, because
`return` leaves the block before the trailer runs —
`try { Get-Service nope -ErrorAction Stop } catch { return }` exits 0. A script
whose last statement is an empty `catch {}` after a caught error exits 1, as
under `-Command`. End any script whose exit code matters (a guard, an
`execute` check) with an explicit `exit 0` / `exit 1`; never rely on `return`.

### Shell options map

Pass `Value::Null` for defaults. Any other key is an `Err`.

| Key | Type | Meaning |
|---|---|---|
| `cwd` | string | working directory for the child |
| `env` | map of string | environment variables added to the child; a non-string value is an `Err` |
| `timeout` | int or float | seconds before the child is killed; firing returns `Err` |
| `stdin` | string | text written to the child's standard input |

### CmdOutput

```rust
struct CmdOutput { stdout: string, stderr: string, code: int, success: bool }
```

`success` is `true` when `code` is zero. A non-zero exit is `Ok` with
`success == false`. `Err` means the command could not run: program not found,
spawn failed, or the timeout fired. `stdout` and `stderr` are decoded as
UTF-8 lossily: a byte that is not valid UTF-8 becomes U+FFFD and the rest of
the stream survives.

```rust
let out = shell::run("systemctl is-active nginx", Value::Null)?
if !out.success { return Ok(CheckResult::NotConfigured) }
```

## http

`use http`. HTTP client built on rustls. Cross-platform. Every function takes an
options `Value` as its last argument. A `get`/`post` response with an error
status is still `Ok`; inspect `status`. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `get` | `http::get(url: string, opts: Value) -> Result[HttpResponse, string]` | GET |
| `post` | `http::post(url: string, body: string, opts: Value) -> Result[HttpResponse, string]` | POST `body`; set the content type through `headers` |
| `download` | `http::download(url: string, dest: string, opts: Value) -> Result[int, string]` | writes the body to the file at `dest`, returns bytes written; atomic, see below |

### HTTP options map

Pass `Value::Null` for defaults. Any other key is an `Err`.

| Key | Type | Meaning |
|---|---|---|
| `headers` | map of string | request headers; a non-string value is an `Err` |
| `timeout` | int or float | seconds before the request is abandoned |
| `redirects` | bool | follow redirects, default `true` |
| `retries` | int | `download` only: extra attempts after the first, default `0` |
| `size` | int | `download` only: expected byte count of the file |
| `sha256` | string | `download` only: expected SHA-256 of the file, 64 hex digits, any case |

`get`/`post` reject `retries`, `size` and `sha256` as unknown keys.

### download behaviour

- Streams into `<dest>.part` in the same directory, fsyncs, and renames onto
  `dest` only after every check passes. On failure `.part` is removed and an
  existing `dest` is untouched, so `fs::exists(dest)` never sees a partial
  download. A stale `<dest>.part` from an earlier call is deleted first.
- Asks for `Accept-Encoding: identity`. The body must match `Content-Length`
  when the server sends one, then `size`, then `sha256` (over the whole file).
- Retried (with `retries`): transport errors, 5xx, 429, a body shorter than
  `Content-Length`/`size`. Not retried: other non-2xx statuses, a body longer
  than expected, a `sha256` mismatch. Backoff 1s, 2s, 4s… capped at 30s.
- After a part-way failure on a response with `Accept-Ranges: bytes`, the next
  attempt sends `Range: bytes=<n>-` (+ `If-Range` with the ETag or
  Last-Modified). Kept only on `206` whose `Content-Range` starts at n;
  otherwise it restarts from byte 0.
- `Err` text: `download <url> failed: <reason>`, or `download <url> failed
  after <k> attempts: <reason>` when `retries` is set.

With `sha256` set, the `http::download` + `hash::sha256_file` + compare
pattern collapses into the one call.

### HttpResponse

```rust
struct HttpResponse { status: int, body: string, headers: Map[string, string] }
```

## hash

`use hash`. Digests as lowercase hexadecimal strings. Cross-platform. String
variants never fail; file variants fail when the file cannot be read. This is
the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `sha256` | `hash::sha256(s: string) -> string` | |
| `sha256_file` | `hash::sha256_file(path: string) -> Result[string, string]` | |
| `sha512` | `hash::sha512(s: string) -> string` | |
| `sha512_file` | `hash::sha512_file(path: string) -> Result[string, string]` | |
| `md5` | `hash::md5(s: string) -> string` | legacy checksum interop only |
| `md5_file` | `hash::md5_file(path: string) -> Result[string, string]` | legacy checksum interop only |

## encoding

`use encoding`. Converts between strings, bytes and base64. Cross-platform.
Bytes are `List[int]`, as in `fs::read_bytes`/`fs::write_bytes`; a byte argument
with an element outside `0..=255` is an `Err`. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `base64_encode` | `encoding::base64_encode(bytes: List[int]) -> Result[string, string]` | standard alphabet, `=` padded |
| `base64_decode` | `encoding::base64_decode(s: string) -> Result[List[int], string]` | standard alphabet, padded; surrounding whitespace ignored |
| `utf8` | `encoding::utf8(s: string) -> List[int]` | a string's UTF-8 bytes; base64 a string with `base64_encode(utf8(s))` |
| `from_utf8` | `encoding::from_utf8(bytes: List[int]) -> Result[string, string]` | `Err` on invalid UTF-8 |
| `utf16le` | `encoding::utf16le(s: string) -> List[int]` | UTF-16 little-endian, no BOM |
| `from_utf16le` | `encoding::from_utf16le(bytes: List[int]) -> Result[string, string]` | strips a leading `FF FE` BOM; `Err` on odd length or an unpaired surrogate |

```rust
// Windows unattend hidden password: base64(UTF-16LE(password + element name))
let hidden = encoding::base64_encode(encoding::utf16le(password + "Password"))?
```

## archive

`use archive`. Extracts archives with no external `tar` or `unzip`.
Cross-platform. The returned int is the number of entries extracted. This is
the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `extract_zip` | `archive::extract_zip(archive: string, dest: string) -> Result[int, string]` | `.zip` |
| `extract_tar_gz` | `archive::extract_tar_gz(archive: string, dest: string) -> Result[int, string]` | gzip-compressed tar |
| `extract` | `archive::extract(archive: string, dest: string) -> Result[int, string]` | picks the format from the extension: `.zip`, `.tar.gz`, `.tgz`; any other extension is an `Err` |

## env

`use env`. Process environment and host identity. Cross-platform. `set` and
`unset` affect the config-weave process and every child spawned afterwards,
including `shell` commands. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `get` | `env::get(name: string) -> Option[string]` | `None` when unset |
| `set` | `env::set(name: string, value: string)` | this process and its children |
| `unset` | `env::unset(name: string)` | |
| `path_split` | `env::path_split(value: string) -> List[string]` | PATH-style list on the platform separator |
| `path_join` | `env::path_join(parts: List[string]) -> Result[string, string]` | `Err` when a part contains the separator |
| `hostname` | `env::hostname() -> string` | |
| `current_user` | `env::current_user() -> string` | |
| `home_dir` | `env::home_dir() -> string` | |
| `is_elevated` | `env::is_elevated() -> bool` | root on Unix, Administrator on Windows |

## sys

`use sys`. OS and hardware facts, the values gatherers publish. Cross-platform,
never fails. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `family` | `sys::family() -> string` | `linux`, `windows` or `macos` |
| `os_name` | `sys::os_name() -> string` | distribution name on Linux |
| `os_version` | `sys::os_version() -> string` | |
| `kernel_version` | `sys::kernel_version() -> string` | |
| `arch` | `sys::arch() -> string` | `x86_64`, `aarch64`, … |
| `cpu_count` | `sys::cpu_count() -> int` | logical CPUs |
| `total_memory` | `sys::total_memory() -> int` | bytes |
| `available_memory` | `sys::available_memory() -> int` | bytes, now |

## data

`use data`. INI over `Value`. Cross-platform. The parser accepts `key=value`
lines and `[section]` headers and rejects any other non-blank line with an
error naming the line number. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `ini_parse` | `data::ini_parse(text: string) -> Result[Value, string]` | map of sections, each a map of keys; keys before the first header go under `""`, dropped when empty |
| `ini_serialize` | `data::ini_serialize(sections: Value) -> Result[string, string]` | the value must be a map whose entries are maps |

## template

`use template`. Tera rendering with autoescape off. Cross-platform. Backs the
`linux_files.template` resource. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `render` | `template::render(template: string, vars: Value) -> Result[string, string]` | `vars` is a `Value::Map`; `Value::Null` renders with an empty context; any other kind is an `Err`. Fails on a syntax error or a missing variable |

Tera provides `{{ x }}`, `{% for %}`, `{% if %}` and filters. In WCL, author a
template body as a raw heredoc (`<<'TMPL'`) so WCL's `${}` interpolation leaves
Tera's `{{ }}` and `{% %}` alone, and feed data through `vars`.

## time

`use time`. Wall and monotonic clocks. Cross-platform, never fails. This is the
whole surface.

| Function | Signature | Notes |
|---|---|---|
| `now_unix` | `time::now_unix() -> float` | seconds since the epoch, sub-second precision |
| `now_millis` | `time::now_millis() -> int` | whole milliseconds; `now_millis() / 1000` keeps an int comparison in ints |
| `instant` | `time::instant() -> float` | monotonic seconds since a process-wide anchor |
| `elapsed` | `time::elapsed(start: float) -> float` | seconds since an earlier `instant`; use for durations, the wall clock can step backwards |
| `sleep` | `time::sleep(millis: int)` | blocks the thread; negative is zero |
| `format_iso` | `time::format_iso(unix_seconds: float) -> string` | ISO 8601 in UTC |

## json

`use json`. JSON over `Value`. Cross-platform. Serialization sorts map keys so
the same value always yields the same text. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `parse` | `json::parse(text: string) -> Result[Value, string]` | objects to maps, arrays to lists, `null` to `Value::Null` |
| `to_string` | `json::to_string(value: Value) -> string` | compact |
| `to_string_pretty` | `json::to_string_pretty(value: Value) -> string` | indented |

## toml

`use toml`. TOML over `Value`. Cross-platform. Serialization fails when the
top-level value is not a map or a `Value::Null` appears anywhere, because TOML
has no null. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `parse` | `toml::parse(text: string) -> Result[Value, string]` | tables to maps, arrays to lists, dates and times to strings |
| `to_string` | `toml::to_string(value: Value) -> Result[string, string]` | compact |
| `to_string_pretty` | `toml::to_string_pretty(value: Value) -> Result[string, string]` | expanded |

## xml

`use xml`. XML over `Value`. Cross-platform. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `parse` | `xml::parse(text: string) -> Result[Value, string]` | mapping below |
| `to_string` | `xml::to_string(value: Value) -> Result[string, string]` | compact |
| `to_string_pretty` | `xml::to_string_pretty(value: Value) -> Result[string, string]` | indented |

Mapping: elements nest as maps keyed by element name, attributes under
`@attrs`, text under `#text`, repeated same-name children become a list.

| XML | Value |
|---|---|
| `<cfg><name>weave</name></cfg>` | `{"cfg": {"name": "weave"}}` |
| `<a k="v">text</a>` | `{"a": {"@attrs": {"k": "v"}, "#text": "text"}}` |
| `<r><i>1</i><i>2</i></r>` | `{"r": {"i": ["1", "2"]}}` |

Mixed content loses its order: text interleaved with children is concatenated
into one `#text`. Produce documents with `template` instead.

## regex

`use regex`. Rust `regex` syntax: no backreferences, no lookaround, linear
time. Cross-platform. No function returns a `Result`; an invalid pattern is a
fault that ends the step with Error. Every function takes `(pattern, text)` in
that order; the swapped order compiles and never matches. This is the whole
surface.

| Function | Signature | Notes |
|---|---|---|
| `is_match` | `regex::is_match(pattern: string, s: string) -> bool` | anywhere in `s` |
| `find` | `regex::find(pattern: string, s: string) -> Option[string]` | first match |
| `find_all` | `regex::find_all(pattern: string, s: string) -> List[string]` | every non-overlapping match, in order |
| `replace` | `regex::replace(pattern: string, s: string, rep: string) -> string` | every match; `$1` and `$name` in `rep` expand captures |
| `captures` | `regex::captures(pattern: string, s: string) -> Option[List[string]]` | group 0 (whole match) first; a non-participating group is `""`; `None` when no match |
| `split` | `regex::split(pattern: string, s: string) -> List[string]` | around every match |

## registry (Windows only)

`use registry`. Registered everywhere; on Linux and macOS every call returns
`Err`. A key is a hive-prefixed path such as `HKLM\Software\Vendor\App`
(in a wscript string literal: `"HKLM\\Software\\Vendor\\App"`). The hive
prefix is case-insensitive and accepts the short or the long name
(`HKLM` or `HKEY_LOCAL_MACHINE`). This is the whole surface.

| Constant | Value |
|---|---|
| `registry::HKLM` | `"HKLM"` |
| `registry::HKCU` | `"HKCU"` |
| `registry::HKCR` | `"HKCR"` |
| `registry::HKU` | `"HKU"` |
| `registry::HKCC` | `"HKCC"` |

| Function | Signature | Notes |
|---|---|---|
| `read` | `registry::read(key: string, name: string) -> Result[Option[Value], string]` | `None` when the key or the value is absent; `Value` typed by kind (below); a kind outside the table is an `Err` |
| `read_typed` | `registry::read_typed(key: string, name: string, opts: Value) -> Result[Option[Value], string]` | `None` when absent, else a map `{kind, data}`: `kind` is the `write` kind name (table below), `data` is what `read` returns; tells `sz` from `expand_sz` and `dword` from `qword`. A kind outside the table is `{kind: "other", type: <REG_* int>, data: <raw bytes as List[int]>}`, not an `Err` |
| `write` | `registry::write(key: string, name: string, value: Value, kind: string) -> Result[unit, string]` | `kind` must match the shape of `value` |
| `delete_value` | `registry::delete_value(key: string, name: string) -> Result[unit, string]` | one value |
| `create_key` | `registry::create_key(key: string) -> Result[unit, string]` | creates missing parents |
| `delete_key` | `registry::delete_key(key: string) -> Result[unit, string]` | the key and its whole subtree |
| `key_exists` | `registry::key_exists(key: string) -> Result[bool, string]` | `false` only when the key is not found |
| `value_names` | `registry::value_names(key: string, opts: Value) -> Result[List[string], string]` | the key's value names, sorted case-insensitively; the default value is `""` and listed only when set; a missing key is an `Err` (not an empty list) |
| `read_with` | `registry::read_with(key: string, name: string, opts: Value) -> Result[Option[Value], string]` | `read` in the view `opts` selects |
| `write_with` | `registry::write_with(key: string, name: string, value: Value, kind: string, opts: Value) -> Result[unit, string]` | |
| `delete_value_with` | `registry::delete_value_with(key: string, name: string, opts: Value) -> Result[unit, string]` | |
| `create_key_with` | `registry::create_key_with(key: string, opts: Value) -> Result[unit, string]` | |
| `delete_key_with` | `registry::delete_key_with(key: string, opts: Value) -> Result[unit, string]` | |
| `key_exists_with` | `registry::key_exists_with(key: string, opts: Value) -> Result[bool, string]` | |

`opts` (on `read_typed`, `value_names` and every `_with` function) is `Value::Null` or a
map with one key, `view`: `"native"` (default, the process's own view),
`"wow64_32"` (the 32-bit `WOW6432Node` view) or `"wow64_64"` (the 64-bit
view). An unknown key or view is an `Err`. The functions without `_with`
always use the native view.

"Absent" (`None` from `read`/`read_typed`, `false` from `key_exists`) means
only ERROR_FILE_NOT_FOUND or ERROR_PATH_NOT_FOUND. Any other failure to
open the key or query the value, such as ERROR_ACCESS_DENIED, is an `Err`
naming the key and the Win32 error. `value_names` has no absent result: a
missing key is an `Err` too.

### Registry value kinds

| `kind` string | Registry type | `Value` shape |
|---|---|---|
| `sz` | REG_SZ | `Value::String` |
| `expand_sz` | REG_EXPAND_SZ | `Value::String` |
| `dword` | REG_DWORD | `Value::Int` |
| `qword` | REG_QWORD | `Value::Int` |
| `multi_sz` | REG_MULTI_SZ | `Value::List` of `Value::String` |
| `binary` | REG_BINARY | `Value::List` of `Value::Int`, one per byte, 0-255 (the `fs::read_bytes` shape) |
| `none` | REG_NONE | as `binary` |
| `other` | any other type (REG_LINK, REG_RESOURCE_LIST, ...) | `read_typed` only: the map also carries `type` (the REG_* int); `data` is the raw bytes as for `binary`. Not accepted by `write` |

`read` returns the same shapes: REG_SZ and REG_EXPAND_SZ as `Value::String`,
REG_DWORD and REG_QWORD as `Value::Int`, REG_MULTI_SZ as a list of strings,
REG_BINARY and REG_NONE as a list of byte ints. Any other registry type
(REG_LINK, ...) is an `Err` from `read`; `read_typed` returns it as `other`. `read` cannot tell REG_SZ from REG_EXPAND_SZ or
REG_DWORD from REG_QWORD; `read_typed` can.

## acl (Windows only)

`use acl`. Reads, compares and sets the owner and DACL of files, directories
and registry keys, replacing PowerShell `Get-Acl`/`Set-Acl`, and resolves
principals to SIDs. Registered
everywhere; on Linux and macOS every call returns `Err`. No POSIX ACLs. This
is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `read` | `acl::read(path: string, opts: Value) -> Result[Value, string]` | `{owner: {sid, name}, protected: bool, null_dacl: bool, entries: [{sid, name, kind, rights, mask, inherit, inherited}]}`, entries in stored order, inherited ones included; `null_dacl: true` means a NULL DACL (everyone full access), which also has no entries |
| `set` | `acl::set(path: string, spec: Value, opts: Value) -> Result[unit, string]` | replaces every explicit entry with `spec.entries`, deny before allow; never writes inherited entries |
| `diff` | `acl::diff(path: string, spec: Value, opts: Value) -> Result[List[string], string]` | one line per difference; `[]` = converged. Exactly the comparison `set` converges, so `set` then `diff` is `[]`. Use it in `check` |
| `lookup` | `acl::lookup(principal: string) -> Result[Value, string]` | `{sid, name}` for a SID string or account name, resolved exactly as `set` resolves a spec principal; `name` is the canonical `DOMAIN\\name` (`"BUILTIN\\Administrators"` for `"Administrators"` and `"S-1-5-32-544"`), or `Value::Null` when the SID does not resolve (a SID string for a deleted account still returns its `sid`). An unresolvable account name is the same `Err` `set` gives |

`opts` is `Value::Null` or `{target, view}`: `target` is `"file"` (default,
a file or directory) or `"registry"`; `view` (`"native"`, `"wow64_32"`,
`"wow64_64"`, as in `registry`) is valid only with `target: "registry"`. A
registry path uses the `registry` syntax (`"HKLM\\Software\\Vendor\\App"`).
An unknown key is an `Err`. A missing file or key is an `Err` naming it. A
final symlink or junction (any reparse point) is refused with an `Err`:
name its target.

Read entry keys: `sid` (`"S-1-5-32-545"`); `name` (`"BUILTIN\\Users"`,
`"Everyone"`, or `Value::Null` when the SID does not resolve); `kind`
(`"allow"`, `"deny"`, or `"other"` for object/callback ACEs, which also
carry `type` and have null `sid`/`name`); `rights` (names, below); `mask`
(int); `inherit` (subset of `object`, `container`, `no_propagate`,
`inherit_only`; `[]` = this object only); `inherited` (bool). Generic bits
(GENERIC_ALL on a `CREATOR OWNER` entry) are mapped to the target's rights
in both `rights` and `mask`.

Spec: `{owner?: principal, protected?: bool, entries: [{principal, kind, rights?, mask?, inherit?}]}`.

- `entries` is required (`[]` removes every explicit entry). Each entry
  needs `principal`, `kind` (`allow`/`deny`) and `rights` or `mask`; both
  given = their union.
- Omitted `owner` / `protected` are left unchanged and not compared.
- `protected: true` disables inheritance and drops inherited entries (it
  does not copy them in: list everything the object needs). `protected:
  false` re-enables inheritance; Windows recomputes inherited entries from
  the parent, and a directory passes its inheritable entries to children.
- Owner changes enable SeRestorePrivilege + SeTakeOwnershipPrivilege on a
  thread-local token for the call (works as SYSTEM or admin); if they
  cannot be enabled and the owner differs, `Err`.
- Inherit flags on a plain file (not a directory) are an `Err`: Windows
  strips them, so the entry could never converge.
- A principal is a SID string or an account name: `"BUILTIN\\Administrators"`,
  `"Administrators"`, `"NT AUTHORITY\\SYSTEM"`, `"DOMAIN\\user"`,
  `".\\user"` (local account). Unresolvable = `Err` naming it. Comparison
  is always by SID.

`diff` compares: owner by SID (when given), `protected` (when given), and
explicit entries as a set of (SID, kind, mask with generic bits mapped,
inherit flags); order and duplicates do not count, so a changed right shows
as one `missing entry` line plus one `unexpected entry` line. Inherited
entries are ignored unless the spec has `protected: true` and the DACL is
already protected.

### acl rights

| Target | Composite names (reported first, largest first) | Single-bit names (bit order) | Aliases (spec only) |
|---|---|---|---|
| file | `full_control` 0x1F01FF, `modify` 0x301BF, `read_and_execute` 0x200A9, `read` 0x20089, `write` 0x116 | `read_data`, `write_data`, `append_data`, `read_extended_attributes`, `write_extended_attributes`, `execute_file`, `delete_subdirectories_and_files`, `read_attributes`, `write_attributes`, `delete`, `read_permissions`, `change_permissions`, `take_ownership`, `synchronize` | `list_directory` = `read_data`, `create_files` = `write_data`, `create_directories` = `append_data`, `traverse` = `execute_file` |
| registry | `full_control` 0xF003F, `read_key` 0x20019, `write_key` 0x20006 | `query_values`, `set_value`, `create_subkey`, `enumerate_subkeys`, `notify`, `create_link`, `delete`, `read_permissions`, `change_permissions`, `take_ownership` | `execute_key` = `read_key` |

On read a mask becomes: each composite whose bits are all present and that
covers a bit no earlier name did, then the single name of each bit left.
`0x1F01FF` reads `["full_control"]`; `0x1200A9` reads
`["read_and_execute", "synchronize"]`. Unnamed bits appear only in `mask`.
An unknown right name is an `Err` listing the target's valid names. `set`
adds nothing to the mask: a file allow entry normally wants `synchronize`
too (Explorer and .NET always add it).

```rust
use acl
use value

// check: params.acl is the spec map, params.path the directory.
fn check(params: Value) -> Result[CheckResult, string] {
    let p = params.get("path").unwrap().as_string().unwrap()
    let changes = acl::diff(p, params.get("acl").unwrap(), Value::Null)?
    if changes.len() == 0 { Ok(CheckResult::AlreadyConfigured) } else { Ok(CheckResult::NotConfigured) }
}
```

## service (Windows only)

`use service`. Windows services through the Service Control Manager.
Registered everywhere; on Linux and macOS every call returns `Err`. Manage
Linux services by running `systemctl` through `shell`. This is the whole
surface.

| Function | Signature | Notes |
|---|---|---|
| `status` | `service::status(name: string) -> Result[string, string]` | one of the status strings below |
| `start` | `service::start(name: string) -> Result[unit, string]` | waits (60s) until `running`; waits out pending states first, continues a `paused` service, no-op when already running |
| `stop` | `service::stop(name: string) -> Result[unit, string]` | waits (60s) until `stopped`; waits out pending states first, no-op when already stopped |
| `restart` | `service::restart(name: string, opts: Value) -> Result[unit, string]` | `stop` then `start`, each waiting; `opts` as for `stop_with`, `timeout` applies to each phase; with `stop_dependents` the stopped dependents are started again afterwards, in reverse stop order |
| `start_with` | `service::start_with(name: string, opts: Value) -> Result[unit, string]` | as `start`; `opts` is `Value::Null` or `{timeout: secs}` |
| `stop_with` | `service::stop_with(name: string, opts: Value) -> Result[unit, string]` | as `stop`; `opts` is `Value::Null` or `{timeout: secs, stop_dependents: bool}` (default `false`; `true` first stops every dependent not already stopped, in SCM order) |
| `dependents` | `service::dependents(name: string) -> Result[List[string], string]` | names of the services that depend on `name`, directly or transitively, in any state, sorted |
| `set_startup` | `service::set_startup(name: string, mode: string) -> Result[unit, string]` | `mode` is `automatic`, `automatic_delayed`, `manual` or `disabled`; anything else is an `Err`. `automatic` clears the delayed-start flag |
| `startup` | `service::startup(name: string) -> Result[string, string]` | `automatic`, `automatic_delayed`, `manual`, `disabled`, `boot`, `system`, or `other` for any other startup type |

`stop_dependents` stops dependents in the order the SCM lists them (reverse
start order, so a dependent of a dependent stops first), each with the same
timeout, then the service. A dependent that was already stopped is left
alone. A dependent that fails ends the call with an `Err` naming it. The
service is checked for stop rights before any dependent is touched, so a
service the caller cannot stop fails without stopping anything. If the
service itself then fails to stop, the dependents already stopped are
started again and the `Err` names any that would not start.
`start_with` rejects `stop_dependents`.

A wait that runs out returns an `Err` naming the last state seen. A service
that goes pending and then drops back (for example it crashes while starting)
fails at once.

### Service status strings

`running`, `stopped`, `start_pending`, `stop_pending`, `paused`,
`pause_pending`, `continue_pending`. A state outside that list is reported as
`unknown`.

## com (Windows only)

`use com`. Late-bound COM through `IDispatch`, plus WMI. Registered everywhere;
on Linux and macOS every call returns `Err`. Worker threads are initialised as
single-threaded apartments by the engine. Arguments and results cross as
`Value`. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `create` | `com::create(progid: string) -> Result[ComObject, string]` | from a ProgID such as `WScript.Shell` |
| `get_object` | `com::get_object(name: string) -> Result[ComObject, string]` | binds a moniker (`winmgmts://./root/cimv2`) or a running object, as `GetObject` does |
| `wmi_query` | `com::wmi_query(query: string) -> Result[Value, string]` | WQL against `root\cimv2`; a `Value::List` of `Value::Map`, one per row, keyed by property name |

### ComObject

```rust
#[opaque]
struct ComObject {}
```

A result that is itself an object cannot be held in a `Value`, so fetch it with
`get_object`, `call_object` or `items`. Calling `get` or `call` on a member that
returns an object is an `Err`, and calling `get_object` or `call_object` on one
that returns plain data is an `Err`.

| Method | Signature | Notes |
|---|---|---|
| `get` | `(name: string) -> Result[Value, string]` | read a data property |
| `get_object` | `(name: string) -> Result[ComObject, string]` | read a property that is an object |
| `set` | `(name: string, value: Value) -> Result[unit, string]` | write a property |
| `call` | `(name: string, args: List[Value]) -> Result[Value, string]` | invoke a method returning data |
| `call_object` | `(name: string, args: List[Value]) -> Result[ComObject, string]` | invoke a method returning an object |
| `items` | `() -> Result[List[ComObject], string]` | enumerate a collection |

## testlab (scenario scripts only)

`use testlab`. The driver API a scenario script runs against a live vmlab lab.
A scenario runs host-side, not inside an instance, and exports
`fn run(lab: Lab) -> bool` or `fn run(lab: Lab) -> Result[bool, string]`.
The module has no free functions; it registers the opaque handles `Lab`,
`Machine` and `RunReport` and the plain structs `StepResult` and `ExecOut`.
Resource keys are `package.resource`. Playbook directories are resolved
relative to the scenario's package. The runner tears the lab down after `run`
returns. This is the whole surface.

### Lab

| Method | Signature | Notes |
|---|---|---|
| `machine` | `(name: string) -> Result[Machine, string]` | brings the declared VM up on first reference, returns its handle |
| `log` | `(message: string)` | a scenario progress line on the terminal |

### Machine

The config-weave binary is copied into a machine and smoke-tested the first
time a method needs it, so a machine used only through `exec` never pays for
that copy.

| Method | Signature | Notes |
|---|---|---|
| `name` | `() -> string` | the declared name |
| `exec` | `(cmd: string, args: List[string]) -> Result[ExecOut, string]` | run a program in the guest, argv style |
| `powershell` | `(script: string) -> Result[ExecOut, string]` | `powershell -NoProfile -NonInteractive`, script copied in as a file (off the guest's command line) and deleted afterwards, as `shell::powershell` does on a host |
| `copy_in` | `(host_path: string, dest: string) -> Result[unit, string]` | host file or directory into the guest |
| `reboot` | `() -> Result[unit, string]` | |
| `wait_ready` | `(secs: int) -> Result[unit, string]` | up to `secs` seconds for the guest to answer; negative is zero |
| `apply_resource` | `(key: string, props: Value) -> Result[StepResult, string]` | one resource; `props` is a `Value::Map`; a string for a `symbol` param is written as `:name` and must be an identifier |
| `check_resource` | `(key: string, props: Value) -> Result[StepResult, string]` | one resource |
| `gather` | `(key: string, params: Value) -> Result[Value, string]` | one gatherer's value; a refusal is an `Err` |
| `apply` | `(dir: string) -> Result[RunReport, string]` | a whole playbook directory, its only play; several or no plays is an `Err` naming them |
| `apply_with` | `(dir: string, opts: Value) -> Result[RunReport, string]` | `opts`: null or `{play: "name"}`; other keys are an `Err` |
| `check` | `(dir: string) -> Result[RunReport, string]` | as `apply` |
| `check_with` | `(dir: string, opts: Value) -> Result[RunReport, string]` | as `apply_with` |

### RunReport

| Method | Signature | Notes |
|---|---|---|
| `ok` | `() -> bool` | the run exited zero |
| `step` | `(name: string) -> Result[StepResult, string]` | one step; an unknown name is an `Err` |

### StepResult

```rust
struct StepResult { status: string, message: string, ok: bool }
```

`status` is one of `not_configured`, `configured`, `already_configured`,
`reboot_required`, `error`, `skipped`, `not_run`. `ok` is `false` only when
the step errored or was missing from the report (then `status` is `not_run`).

### ExecOut

```rust
struct ExecOut { exit_code: int, stdout: string, stderr: string }
```

```rust
use testlab
use value

fn run(lab: Lab) -> Result[bool, string] {
    let dc = lab.machine("dc")?
    let first = dc.apply_resource("windows_domain.forest", Value::Map(#{ "name": Value::String("corp.example") }))?
    if first.status != "reboot_required" { return Ok(false) }
    dc.reboot()?
    dc.wait_ready(600)?
    let second = dc.apply_resource("windows_domain.forest", Value::Map(#{ "name": Value::String("corp.example") }))?
    Ok(second.status == "already_configured")
}
```

## Not registered

`use math`, `use process` and the wscript-std `fs` are not available: `math`
and `process` fail to compile, and `use fs` is the config-weave module above.
Replacements and the language features wscript leaves out: scripts.md.
