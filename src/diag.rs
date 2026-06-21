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

impl Diag {
    /// A diagnostic with no source context.
    pub fn bare(message: impl Into<String>) -> Diag {
        let message = message.into();
        let rendered = format!("error: {message}");
        Diag { message, rendered }
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
        Diag { message, rendered }
    }

    /// Wrap a WCL parse error (it already carries its source).
    pub fn from_parse(err: wcl_lang::ParseError) -> Diag {
        let message = err.to_string();
        let rendered = render_report(err);
        Diag { message, rendered }
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
        Diag { message, rendered }
    }

    /// Wrap wscript compile diagnostics for one script file.
    pub fn from_wscript(diags: &[wscript::Diagnostic], file: &Path, source: &str) -> Vec<Diag> {
        diags
            .iter()
            .filter(|d| d.severity == wscript::Severity::Error)
            .map(|d| {
                let mut labels = vec![LabeledSpan::at(
                    (d.span.lo as usize)..(d.span.hi as usize),
                    d.message.clone(),
                )];
                for (span, text) in &d.labels {
                    labels.push(LabeledSpan::at(
                        (span.lo as usize)..(span.hi as usize),
                        text.clone(),
                    ));
                }
                let mut md = miette::MietteDiagnostic::new(d.message.clone())
                    .with_code(d.code)
                    .with_labels(labels);
                if let Some(help) = &d.help {
                    md = md.with_help(help.clone());
                }
                let report = miette::Report::from(md).with_source_code(NamedSource::new(
                    file.display().to_string(),
                    source.to_string(),
                ));
                Diag {
                    message: format!("[{}] {}", d.code, d.message),
                    rendered: format!("{report:?}"),
                }
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
