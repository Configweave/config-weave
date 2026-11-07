//! Shared helpers for distilling command output into a one-line tail for
//! diagnostics. Used by the vmlab backend and the runner.

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
    summarize_stderr(&s, out.status.code())
}

/// The message of a failed CLI run. A miette report (vmlab's error format)
/// keeps its headline (the line that starts with "×") and its first "help:"
/// line, because the last lines of a report are only the snippet and frame.
/// Other output keeps its last three lines.
fn summarize_stderr(s: &str, code: Option<i32>) -> String {
    if let Some(msg) = miette_summary(s) {
        return msg;
    }
    let tail = tail_lines(s, 3);
    if tail.is_empty() {
        format!("(no stderr, exit {})", code.unwrap_or(-1))
    } else {
        tail.join(" / ")
    }
}

/// The headline and the first help line of a miette report, or `None` when
/// the text has no headline.
fn miette_summary(s: &str) -> Option<String> {
    let lines: Vec<&str> = s.lines().map(str::trim).collect();
    let headline = lines.iter().position(|l| l.starts_with('×'))?;
    let mut parts = vec![lines[headline]];
    if let Some(help) = lines[headline + 1..].iter().find(|l| l.starts_with("help:")) {
        parts.push(help);
    }
    Some(parts.join(" / "))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_of_empty_input_is_empty() {
        assert!(tail_lines("", 3).is_empty());
        assert!(tail_lines("  \n\n  ", 3).is_empty());
    }

    #[test]
    fn tail_lines_keeps_source_order() {
        assert_eq!(tail_lines("a\nb\nc\nd", 3), vec!["b", "c", "d"]);
    }

    #[test]
    fn tail_lines_shorter_than_max_returns_all() {
        assert_eq!(tail_lines("a\nb", 5), vec!["a", "b"]);
        assert_eq!(tail_lines("a\nb\nc", 3), vec!["a", "b", "c"]);
    }

    #[test]
    fn tail_lines_ignores_surrounding_blank_lines() {
        assert_eq!(tail_lines("\n\na\nb\n\n", 3), vec!["a", "b"]);
    }

    #[test]
    fn stderr_tail_keeps_miette_headline_and_help() {
        let stderr = "Error: \n\
              \x20 × template x86_64/ubuntu-24.04 not in the template store\n\
              \x20  ╭─[lab.wcl:6:1]\n\
              \x20 5 │ [vm.linux]\n\
              \x20 6 │ agent_update = false\n\
              \x20   · ─────────────┬─────\n\
              \x20   ·              ╰── here\n\
              \x20   ╰────\n\
              \x20 help: run `vmlab template pull` to fetch it\n";
        let msg = summarize_stderr(stderr, Some(1));
        assert!(
            msg.contains("not in the template store"),
            "headline lost: {msg:?}"
        );
        assert!(msg.contains("help: run `vmlab template pull`"), "{msg:?}");
        assert!(!msg.contains("agent_update"), "snippet kept: {msg:?}");
    }

    #[test]
    fn stderr_tail_of_plain_output_is_unchanged() {
        let stderr = "step one\nstep two\nstep three\nstep four\n";
        assert_eq!(
            summarize_stderr(stderr, Some(2)),
            "step two / step three / step four"
        );
        assert_eq!(summarize_stderr("", Some(3)), "(no stderr, exit 3)");
        assert_eq!(summarize_stderr("", None), "(no stderr, exit -1)");
    }

    #[test]
    fn output_tail_joins_lines_and_falls_back() {
        assert_eq!(output_tail("x\ny", "(none)"), "x / y");
        assert_eq!(output_tail("\n \n", "(none)"), "(none)");
    }

    #[test]
    fn rand_suffix_is_identifier_safe_and_unique() {
        let a = rand_suffix();
        let b = rand_suffix();
        assert_ne!(a, b);
        for s in [&a, &b] {
            assert!(!s.is_empty());
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "unexpected char in {s:?}"
            );
        }
    }
}
