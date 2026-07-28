//! Diagnostics: everything `validate` (and the implicit validation phase of
//! every run) can report. Diagnostics render through miette so WCL parse
//! errors, schema violations, engine-side structural errors and wscript
//! compile errors all look the same on the terminal.

use std::path::Path;

use miette::{LabeledSpan, NamedSource};

/// One rendered diagnostic. The message is kept separately so `--json`
/// output and tests can consume it without ANSI noise.
#[derive(Debug, Clone)]
pub struct Diag {
    /// Bare message without ANSI/source rendering (machine consumers).
    #[allow(dead_code)]
    pub message: String,
    pub rendered: String,
}

/// Every `Diag` is built here, so scrubbing decrypted secrets out of both
/// strings in one place covers the lot. `rendered` matters most: it
/// attaches the surrounding source, and a WCL evaluation error can quote
/// the value that caused it.
fn finish(message: String, rendered: String) -> Diag {
    if crate::secrets::redact::active() {
        return Diag {
            message: crate::secrets::redact::scrub(&message),
            rendered: crate::secrets::redact::scrub(&rendered),
        };
    }
    Diag { message, rendered }
}

impl Diag {
    /// A diagnostic with no source context.
    pub fn bare(message: impl Into<String>) -> Diag {
        let message = message.into();
        let rendered = format!("error: {message}");
        finish(message, rendered)
    }

    /// A diagnostic pointing at a span in a named source.
    pub fn spanned(
        message: impl Into<String>,
        label: impl Into<String>,
        file: &Path,
        source: &str,
        span: (usize, usize),
    ) -> Diag {
        let message = message.into();
        let md = miette::MietteDiagnostic::new(message.clone())
            .with_labels(vec![LabeledSpan::at(span.0..span.1, label.into())]);
        let report = miette::Report::from(md).with_source_code(NamedSource::new(
            file.display().to_string(),
            source.to_string(),
        ));
        let rendered = format!("{report:?}");
        finish(message, rendered)
    }

    /// Wrap a WCL parse error (it already carries its source).
    pub fn from_parse(err: wcl_lang::ParseError) -> Diag {
        let message = err.to_string();
        let rendered = render_report(err);
        finish(message, rendered)
    }

    /// Wrap a WCL evaluation/schema error, attaching the source it points
    /// into.
    pub fn from_eval(err: wcl_lang::EvalError, file: &Path, source: &str) -> Diag {
        let message = err.to_string();
        let report = miette::Report::new(err).with_source_code(NamedSource::new(
            file.display().to_string(),
            source.to_string(),
        ));
        let rendered = format!("{report:?}");
        finish(message, rendered)
    }

    /// Wrap wscript compile diagnostics.
    ///
    /// Spans are global offsets into a virtual address space covering
    /// every file of the import graph, so each is routed through the
    /// `SourceMap` to find the file it belongs to and rebased local to it.
    /// Without this, an error inside an imported helper renders against
    /// the importing file's text — wrong path, wrong line, wrong snippet.
    pub fn from_wscript(
        diags: &[wscript::Diagnostic],
        sources: &[(String, String)],
        map: &wscript::SourceMap,
    ) -> Vec<Diag> {
        diags
            .iter()
            .filter(|d| d.severity == wscript::Severity::Error)
            .map(|d| {
                // Whichever file the primary span lands in is the source
                // the report renders against.
                let (path, source, base) = match map.local(d.span.lo) {
                    Some((info, _)) => {
                        let text = sources
                            .iter()
                            .find(|(p, _)| *p == info.path)
                            .map(|(_, s)| s.as_str())
                            .unwrap_or("");
                        (info.path.clone(), text, info.base)
                    }
                    None => match sources.first() {
                        Some((p, s)) => (p.clone(), s.as_str(), 0),
                        None => (String::from("<script>"), "", 0),
                    },
                };
                let rebase = |s: &wscript::Span| {
                    (s.lo.saturating_sub(base) as usize)..(s.hi.saturating_sub(base) as usize)
                };

                let mut labels = vec![LabeledSpan::at(rebase(&d.span), d.message.clone())];
                for (span, text) in &d.labels {
                    // A secondary label in another file has no home in a
                    // single-source report; drop it rather than point at a
                    // meaningless offset in this one.
                    if map.local(span.lo).is_some_and(|(i, _)| i.path == path) {
                        labels.push(LabeledSpan::at(rebase(span), text.clone()));
                    }
                }
                let mut md = miette::MietteDiagnostic::new(d.message.clone())
                    .with_code(d.code)
                    .with_labels(labels);
                if let Some(help) = &d.help {
                    md = md.with_help(help.clone());
                }
                let report = miette::Report::from(md)
                    .with_source_code(NamedSource::new(path, source.to_string()));
                finish(format!("[{}] {}", d.code, d.message), format!("{report:?}"))
            })
            .collect()
    }
}

fn render_report(err: impl miette::Diagnostic + Send + Sync + 'static) -> String {
    let report = miette::Report::new(err);
    format!("{report:?}")
}

/// Convert a `wcl_lang` AST span to a `(start, end)` byte range.
pub fn wcl_span(span: wcl_lang::ast::Span) -> (usize, usize) {
    (span.start, span.end)
}
