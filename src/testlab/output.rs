//! Shared helpers for distilling command output into a one-line tail for
//! diagnostics. Used by both backends (`docker`, `vmlab`) and the runner.

use std::process::Output;

/// A short, process-unique lowercase-alphanumeric suffix for naming
/// throwaway labs/networks. Combines the pid with a monotonic counter so
/// concurrent provisions never collide within or across runs.
pub fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // base-36 keeps it short and DNS/identifier-safe.
    fn b36(mut v: u64) -> String {
        const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        if v == 0 {
            return "0".into();
        }
        let mut out = Vec::new();
        while v > 0 {
            out.push(D[(v % 36) as usize]);
            v /= 36;
        }
        out.reverse();
        String::from_utf8(out).unwrap()
    }
    format!("{}{}", b36(pid as u64), b36(n))
}

/// The last `max` non-empty-trimmed lines of `s`, in source order.
pub fn tail_lines(s: &str, max: usize) -> Vec<&str> {
    let rev: Vec<&str> = s.trim().lines().rev().take(max).collect();
    rev.into_iter().rev().collect()
}

/// The interesting last lines of a CLI's stderr, for diagnostics. Falls
/// back to the exit code when stderr is empty.
pub fn stderr_tail(out: &Output) -> String {
    let s = String::from_utf8_lossy(&out.stderr);
    let tail = tail_lines(&s, 3);
    if tail.is_empty() {
        format!("(no stderr, exit {})", out.status.code().unwrap_or(-1))
    } else {
        tail.join(" / ")
    }
}

/// The interesting last lines of arbitrary command output, with a caller-
/// supplied fallback when there is nothing to show.
pub fn output_tail(s: &str, fallback: &str) -> String {
    let tail = tail_lines(s, 3);
    if tail.is_empty() {
        fallback.into()
    } else {
        tail.join(" / ")
    }
}
