//! Rewriting `secret(…)` calls in place.
//!
//! This splices bytes into the original source rather than round-tripping
//! through `wcl_lang::format::to_source` the way `docjson` does. `wcl_lang`
//! has no lossless syntax tree: re-printing canonicalises the whole file
//! (`//` comments become `#`, indentation and one-liners are normalised),
//! so an `encrypt` built on it would reformat a hand-authored playbook as
//! a side effect of changing one string. Splicing by descending offset
//! touches only the call text.
//!
//! Safety rails follow `wcl set` (`WCL/crates/wcl/src/main.rs`): the
//! result is re-parsed before it can reach disk, and the write is a
//! temp-file + rename.

use std::path::Path;

use wcl_lang::ast::Span;

use crate::diag::Diag;

/// Apply `(span, replacement)` edits to `source`. Spans must not overlap;
/// they are applied back-to-front so earlier offsets stay valid.
///
/// The result is re-parsed and an unparseable one is an error, not a file
/// on disk.
pub fn splice(source: &str, edits: &[(Span, String)], name: &str) -> Result<String, Diag> {
    let mut edits: Vec<&(Span, String)> = edits.iter().collect();
    edits.sort_by_key(|(s, _)| std::cmp::Reverse(s.start));

    let mut out = source.to_string();
    let mut prev_start = usize::MAX;
    for (span, text) in edits {
        if span.end > prev_start {
            return Err(Diag::bare(
                "internal error: overlapping secret() edits".to_string(),
            ));
        }
        if span.end > out.len() || span.start > span.end {
            return Err(Diag::bare(
                "internal error: secret() span outside the source".to_string(),
            ));
        }
        out.replace_range(span.start..span.end, text);
        prev_start = span.start;
    }

    wcl_lang::parse_for_edit(&out, name.to_string()).map_err(|e| {
        let d = Diag::from_parse(e);
        Diag::bare(format!(
            "rewriting {name} produced source that does not parse (nothing was \
             written): {}",
            d.message
        ))
    })?;
    Ok(out)
}

/// Write `contents` over `path` via temp file + rename.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), Diag> {
    let tmp = path.with_extension("wcl.weave-tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| Diag::bare(format!("cannot write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Diag::bare(format!("cannot write {}: {e}", path.display()))
    })
}

/// Render a `secret("…")` call. The argument is always a quoted literal:
/// a heredoc is illegal inside a call's parentheses, and WCL's quoted
/// form escapes everything that could break out of it.
pub fn render_call(value: &str) -> String {
    format!("{}({})", super::scan::FN_NAME, quote(value))
}

/// Quote a string as a WCL `utf8` literal. WCL's escape table is exactly
/// `"`, `\`, `\n`, `\t`, `\r` (`WCL/crates/wcl_lang/src/lexer/strings.rs`);
/// there is no `\u{…}` or `\x`, so anything else goes in as raw UTF-8.
/// `${` needs no escaping because a non-`$`-prefixed literal does not
/// interpolate.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::scan;

    fn span_of(source: &str, needle: &str) -> Span {
        let start = source.find(needle).unwrap();
        Span {
            start,
            end: start + needle.len(),
        }
    }

    #[test]
    fn splices_one_call_and_leaves_everything_else_byte_identical() {
        let source = "playbook \"p\" {\n  // keep me\n  vars { a = secret(\"pw\") }\n}\n";
        let edits = vec![(
            span_of(source, "secret(\"pw\")"),
            render_call("CWENC1.x.y.z"),
        )];
        let out = splice(source, &edits, "playbook.wcl").unwrap();
        assert_eq!(
            out,
            "playbook \"p\" {\n  // keep me\n  vars { a = secret(\"CWENC1.x.y.z\") }\n}\n"
        );
    }

    #[test]
    fn applies_multiple_edits_back_to_front() {
        let source = "playbook \"p\" { vars { a = secret(\"one\") b = secret(\"two\") } }";
        let calls = scan::scan_source(source, "t.wcl").unwrap();
        let edits: Vec<(Span, String)> = calls
            .iter()
            .enumerate()
            .map(|(i, c)| (c.span, render_call(&format!("E{i}"))))
            .collect();
        let out = splice(source, &edits, "t.wcl").unwrap();
        assert_eq!(
            out,
            "playbook \"p\" { vars { a = secret(\"E0\") b = secret(\"E1\") } }"
        );
    }

    #[test]
    fn round_trips_a_multiline_value_through_the_quoted_form() {
        let secret = "line one\nline\ttwo\\end \"quoted\"";
        let source = r#"playbook "p" { vars { a = secret("x") } }"#;
        let edits = vec![(span_of(source, "secret(\"x\")"), render_call(secret))];
        let out = splice(source, &edits, "t.wcl").unwrap();
        let calls = scan::scan_source(&out, "t.wcl").unwrap();
        assert_eq!(calls[0].state, scan::State::Plaintext(secret.to_string()));
    }

    #[test]
    fn a_rewrite_that_would_not_parse_is_refused() {
        let source = "playbook \"p\" { vars { a = secret(\"pw\") } }";
        let edits = vec![(span_of(source, "secret(\"pw\")"), "secret(".to_string())];
        let err = splice(source, &edits, "playbook.wcl").unwrap_err();
        assert!(
            err.message.contains("nothing was written"),
            "{}",
            err.message
        );
    }

    #[test]
    fn write_atomic_replaces_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("playbook.wcl");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(!path.with_extension("wcl.weave-tmp").exists());
    }
}
