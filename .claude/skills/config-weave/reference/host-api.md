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
  playbook compiles and validates identically everywhere. The three
  Windows-only modules (`registry`, `service`, `com`) exist on Linux and macOS
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
| `service` | Windows services via the SCM | Windows only | config-weave |
| `com` | COM via IDispatch, WMI queries | Windows only | config-weave |
| `testlab` | scenario driver handles (`Lab`, `Machine`) | scenario scripts only | config-weave |

`testlab` is registered only in the context scenario driver scripts compile
and run against. Every other script sees the twenty modules above it.

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
| `write` | `fs::write(path: string, content: string) -> Result[unit, string]` | replaces contents |
| `write_bytes` | `fs::write_bytes(path: string, bytes: List[int]) -> Result[unit, string]` | replaces contents |
| `append` | `fs::append(path: string, content: string) -> Result[unit, string]` | creates the file when absent |
| `copy` | `fs::copy(from: string, to: string) -> Result[unit, string]` | one file |
| `move` | `fs::move(from: string, to: string) -> Result[unit, string]` | renames a file or a directory |
| `delete` | `fs::delete(path: string) -> Result[unit, string]` | a file or a symlink |
| `delete_dir` | `fs::delete_dir(path: string) -> Result[unit, string]` | a directory and everything in it |
| `mkdir` | `fs::mkdir(path: string) -> Result[unit, string]` | creates missing parents |
| `exists` | `fs::exists(path: string) -> bool` | plain bool, unreadable path answers `false` |
| `is_file` | `fs::is_file(path: string) -> bool` | plain bool |
| `is_dir` | `fs::is_dir(path: string) -> bool` | plain bool |
| `list_dir` | `fs::list_dir(path: string) -> Result[List[string], string]` | entry names, sorted |
| `metadata` | `fs::metadata(path: string) -> Result[Value, string]` | map with the keys below |
| `glob` | `fs::glob(pattern: string) -> Result[List[string], string]` | matching paths, sorted |
| `temp_file` | `fs::temp_file() -> Result[string, string]` | creates a fresh file, returns its path |
| `temp_dir` | `fs::temp_dir() -> Result[string, string]` | creates a fresh directory, returns its path |
| `symlink` | `fs::symlink(target: string, link: string) -> Result[unit, string]` | link at `link` pointing to `target` |
| `read_link` | `fs::read_link(path: string) -> Result[string, string]` | the target of a symlink |

### `fs::metadata` map keys

| Key | Type | Meaning |
|---|---|---|
| `size` | int | size in bytes |
| `modified` | int | modification time, seconds since the Unix epoch |
| `readonly` | bool | the read-only attribute |
| `is_file` | bool | regular file |
| `is_dir` | bool | directory |
| `is_symlink` | bool | symlink |
| `mode` | int | Unix permission bits, `0` on other platforms |

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
| `powershell` | `shell::powershell(script: string, opts: Value) -> Result[CmdOutput, string]` | `powershell`, falls back to `pwsh`, with `-NoProfile -NonInteractive`; works on Linux with PowerShell Core |

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
spawn failed, or the timeout fired.

```rust
let out = shell::run("systemctl is-active nginx", Value::Null)?
if !out.success { return Ok(CheckResult::NotConfigured) }
```

## http

`use http`. HTTP client built on rustls. Cross-platform. Every function takes an
options `Value` as its last argument. A response with an error status is still
`Ok`; inspect `status`. This is the whole surface.

| Function | Signature | Notes |
|---|---|---|
| `get` | `http::get(url: string, opts: Value) -> Result[HttpResponse, string]` | GET |
| `post` | `http::post(url: string, body: string, opts: Value) -> Result[HttpResponse, string]` | POST `body`; set the content type through `headers` |
| `download` | `http::download(url: string, dest: string, opts: Value) -> Result[int, string]` | writes the body to the file at `dest`, returns bytes written |

### HTTP options map

Pass `Value::Null` for defaults. Any other key is an `Err`.

| Key | Type | Meaning |
|---|---|---|
| `headers` | map of string | request headers; a non-string value is an `Err` |
| `timeout` | int or float | seconds before the request is abandoned |
| `redirects` | bool | follow redirects, default `true` |

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
| `write` | `registry::write(key: string, name: string, value: Value, kind: string) -> Result[unit, string]` | `kind` must match the shape of `value` |
| `delete_value` | `registry::delete_value(key: string, name: string) -> Result[unit, string]` | one value |
| `create_key` | `registry::create_key(key: string) -> Result[unit, string]` | creates missing parents |
| `delete_key` | `registry::delete_key(key: string) -> Result[unit, string]` | the key and its whole subtree |
| `key_exists` | `registry::key_exists(key: string) -> Result[bool, string]` | |

### Registry value kinds

| `kind` string | Registry type | `Value` shape |
|---|---|---|
| `sz` | REG_SZ | `Value::String` |
| `expand_sz` | REG_EXPAND_SZ | `Value::String` |
| `dword` | REG_DWORD | `Value::Int` |
| `qword` | REG_QWORD | `Value::Int` |
| `multi_sz` | REG_MULTI_SZ | `Value::List` of `Value::String` |

`read` returns the same shapes: REG_SZ and REG_EXPAND_SZ as `Value::String`,
REG_DWORD and REG_QWORD as `Value::Int`, REG_MULTI_SZ as a list of strings.

## service (Windows only)

`use service`. Windows services through the Service Control Manager.
Registered everywhere; on Linux and macOS every call returns `Err`. Manage
Linux services by running `systemctl` through `shell`. This is the whole
surface.

| Function | Signature | Notes |
|---|---|---|
| `status` | `service::status(name: string) -> Result[string, string]` | one of the status strings below |
| `start` | `service::start(name: string) -> Result[unit, string]` | no-op when already running |
| `stop` | `service::stop(name: string) -> Result[unit, string]` | no-op when already stopped |
| `set_startup` | `service::set_startup(name: string, mode: string) -> Result[unit, string]` | `mode` is `automatic`, `manual` or `disabled`; anything else is an `Err` |
| `startup` | `service::startup(name: string) -> Result[string, string]` | `automatic`, `manual`, `disabled`, or `other` for any other startup type |

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
| `powershell` | `(script: string) -> Result[ExecOut, string]` | `powershell -NoProfile -NonInteractive -Command <script>` |
| `copy_in` | `(host_path: string, dest: string) -> Result[unit, string]` | host file or directory into the guest |
| `reboot` | `() -> Result[unit, string]` | |
| `wait_ready` | `(secs: int) -> Result[unit, string]` | up to `secs` seconds for the guest to answer; negative is zero |
| `apply_resource` | `(key: string, props: Value) -> Result[StepResult, string]` | one resource; `props` is a `Value::Map` |
| `check_resource` | `(key: string, props: Value) -> Result[StepResult, string]` | one resource |
| `gather` | `(key: string, params: Value) -> Result[Value, string]` | one gatherer's value; a refusal is an `Err` |
| `apply` | `(dir: string) -> Result[RunReport, string]` | a whole playbook directory |
| `check` | `(dir: string) -> Result[RunReport, string]` | a whole playbook directory |

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
