# time

_host module_

Wall-clock and monotonic time — unix timestamps, elapsed measurement, sleeping, ISO-8601 formatting.

`use time` — clocks and sleeping. Registering this module is a capability grant: scripts can read the clock and block their thread.

| function | signature |
| --- | --- |
| `now_unix` | `() -> float` (seconds since the epoch, sub-second precision) |
| `now_millis` | `() -> int` (milliseconds since the epoch) |
| `instant` | `() -> float` (monotonic, seconds since a process-wide anchor) |
| `elapsed` | `(start) -> float` (seconds since an `instant()`) |
| `sleep` | `(ms)` (blocks the VM thread; negatives clamp to zero) |
| `format_iso` | `(ts) -> string` (UTC ISO-8601 from unix seconds) |

The common use is a resource that converges on an \*age\* rather than on a state — a package cache refreshed within the last N minutes, say. Compare `fs::metadata(stamp).modified` against now, and prefer `now_millis() / 1000` over `now_unix()` when the other side is an integer, so the comparison stays in integers.

```ws
// Refresh when the stamp file is missing or older than max_age seconds.
fn stale(stamp: string, max_age: int) -> Result[bool, string] {
    if !fs::is_file(stamp) { return Ok(true) }
    let meta = fs::metadata(stamp)?
    let modified = if let Some(m) = meta.get("modified") {
        if let Some(ts) = m.as_int() { ts } else { 0 }
    } else { 0 }
    Ok(time::now_millis() / 1000 - modified > max_age)
}
```

Use `instant`/`elapsed` for durations, never a difference of `now_unix` calls — the wall clock can step backwards (NTP, a VM resuming from a snapshot) and the monotonic clock cannot.

## Related

- [Host API](../references/concept_host_api.md)

- [fs](../references/entity_fs_module.md)

[← Back to SKILL.md](../SKILL.md)
