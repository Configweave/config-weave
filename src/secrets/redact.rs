//! A process-global registry of decrypted plaintexts, and a scrubber that
//! strips them out of anything user-visible.
//!
//! Registration happens in the `secret()` builtin, as values are handed to
//! the evaluator — so the registry only ever holds values this run
//! actually decrypted. Scrubbing is applied at the output choke points
//! (diagnostics, the script log/print bridge, run reports and the event
//! stream), because a resource script is free to echo a parameter it was
//! given and nothing downstream can tell that string from any other.

use std::sync::{OnceLock, RwLock};

/// What a redacted value is replaced with.
pub const MASK: &str = "***";

/// Values shorter than this are not registered: masking a one- or
/// two-character string would corrupt unrelated output far more than it
/// would protect (and such a "secret" is not one).
const MIN_LEN: usize = 4;

fn registry() -> &'static RwLock<Vec<String>> {
    static REG: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    REG.get_or_init(|| RwLock::new(Vec::new()))
}

/// Record a decrypted plaintext so later output can be scrubbed of it.
pub fn register(plaintext: &str) {
    if plaintext.len() < MIN_LEN {
        return;
    }
    let Ok(mut reg) = registry().write() else {
        return;
    };
    if !reg.iter().any(|s| s == plaintext) {
        reg.push(plaintext.to_string());
        // Longest first, so a secret that contains another is masked
        // whole rather than left with a `***` hole in the middle.
        reg.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }
}

/// Replace every registered plaintext in `text` with [`MASK`].
pub fn scrub(text: &str) -> String {
    let Ok(reg) = registry().read() else {
        return text.to_string();
    };
    if reg.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for secret in reg.iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), MASK);
        }
    }
    out
}

/// True when at least one secret has been decrypted this run. Lets hot
/// paths skip the allocation `scrub` would make.
pub fn active() -> bool {
    registry().read().map(|r| !r.is_empty()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The registry is process-global, so these tests share it. They use
    // distinctive values and never assert that the registry is *empty*.

    #[test]
    fn scrubs_a_registered_value() {
        register("correct-horse-battery");
        assert_eq!(
            scrub("password is correct-horse-battery!"),
            format!("password is {MASK}!")
        );
    }

    #[test]
    fn leaves_unrelated_text_alone() {
        register("zzz-registry-probe-value");
        assert_eq!(scrub("nothing to see"), "nothing to see");
    }

    #[test]
    fn ignores_values_too_short_to_mask_safely() {
        register("ab");
        assert_eq!(scrub("a b ab"), "a b ab");
    }

    #[test]
    fn masks_the_longer_of_two_overlapping_values() {
        register("outer-secret-inner-part");
        register("inner-part");
        assert_eq!(scrub("outer-secret-inner-part"), MASK);
    }

    #[test]
    fn registering_twice_keeps_one_entry() {
        register("duplicate-probe-value");
        let before = registry().read().unwrap().len();
        register("duplicate-probe-value");
        assert_eq!(registry().read().unwrap().len(), before);
    }
}
