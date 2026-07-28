//! Finding every `secret(…)` call site in a WCL source.
//!
//! This is a *syntactic* pass over the `parse_for_edit` AST, deliberately
//! independent of evaluation: it is what lets `validate` and `docs` report
//! an un-encrypted secret with no password in hand, and what drives the
//! in-place rewrite performed by `secrets encrypt|decrypt|rekey`.
//!
//! Every `match` over a `wcl_lang::ast` enum here is **exhaustive with no
//! `_` arm**. A new AST variant upstream must break this build rather than
//! silently hide a secret from encryption — a missed call site would be
//! committed in the clear.

use wcl_lang::ast::{Expr, Item, Source, Span, TemplatePart, VariantArgs};

use super::crypto;

/// The builtin's name as written in a playbook.
pub const FN_NAME: &str = "secret";

/// What a call site holds.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// A `CWENC1` blob, ready to decrypt.
    Encrypted(String),
    /// A literal still in the clear, awaiting `secrets encrypt`.
    Plaintext(String),
    /// Something this feature cannot work with; the string explains why.
    Invalid(String),
}

/// One `secret(…)` call, located by the byte span of the **whole call**
/// (`Expr::Utf8` carries no span of its own, so the call is the smallest
/// re-writable unit).
#[derive(Debug, Clone)]
pub struct SecretCall {
    pub span: Span,
    pub state: State,
}

/// Scan already-parsed WCL. Results come out in source order.
pub fn scan(src: &Source) -> Vec<SecretCall> {
    let mut found = Vec::new();
    for item in &src.items {
        walk_item(item, &mut found);
    }
    found.sort_by_key(|c| c.span.start);
    found
}

/// Parse and scan a source string.
pub fn scan_source(source: &str, name: &str) -> Result<Vec<SecretCall>, wcl_lang::ParseError> {
    Ok(scan(&wcl_lang::parse_for_edit(source, name.to_string())?))
}

fn walk_item(item: &Item, out: &mut Vec<SecretCall>) {
    match item {
        Item::Field(f) => walk_expr(&f.expr, out),
        Item::Let(l) => walk_expr(&l.value, out),
        Item::Block(b) => {
            for label in &b.labels {
                walk_expr(label, out);
            }
            for it in &b.items {
                walk_item(it, out);
            }
        }
        Item::Table(t) => {
            for row in &t.rows {
                for v in &row.values {
                    walk_expr(v, out);
                }
            }
        }
        // Type-level declarations and connections hold no runtime value
        // expressions a playbook author would put a secret in. Listed
        // explicitly so an upstream `Item` addition fails the build.
        Item::TypeDecl(_)
        | Item::InterfaceDecl(_)
        | Item::UnionDecl(_)
        | Item::NamespaceDecl(_)
        | Item::UseDecl(_)
        | Item::SymbolSetDecl(_)
        | Item::Import(_)
        | Item::ConnectionDecl(_)
        | Item::Connection(_) => {}
    }
}

fn walk_expr(expr: &Expr, out: &mut Vec<SecretCall>) {
    match expr {
        Expr::Call {
            callee, args, span, ..
        } => {
            if is_secret_callee(callee) {
                out.push(SecretCall {
                    span: *span,
                    state: classify(args),
                });
                // Do not descend: the argument must be a literal, and a
                // nested `secret(secret(…))` is already reported as
                // invalid by `classify`.
                return;
            }
            walk_expr(callee, out);
            for a in args {
                walk_expr(a, out);
            }
        }

        Expr::InterpolatedString { parts, .. } => {
            for p in parts {
                match p {
                    TemplatePart::Literal(_) => {}
                    TemplatePart::Expr(e) => walk_expr(e, out),
                }
            }
        }

        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, out);
            walk_expr(rhs, out);
        }
        Expr::Unary { operand, .. } => walk_expr(operand, out),
        Expr::Paren { inner, .. } => walk_expr(inner, out),
        Expr::Member { recv, .. } => walk_expr(recv, out),

        Expr::Block { lets, tail, .. } => {
            for b in lets {
                walk_expr(&b.value, out);
            }
            walk_expr(tail, out);
        }

        Expr::ListLit { elements, .. } => {
            for e in elements {
                walk_expr(e, out);
            }
        }

        Expr::Record { fields, .. } => {
            for f in fields {
                walk_expr(&f.value, out);
            }
        }

        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            walk_expr(cond, out);
            walk_expr(then_block, out);
            walk_expr(else_block, out);
        }
        Expr::IfLet {
            scrut,
            then_block,
            else_block,
            ..
        } => {
            walk_expr(scrut, out);
            walk_expr(then_block, out);
            walk_expr(else_block, out);
        }

        Expr::Match { scrut, arms, .. } => {
            walk_expr(scrut, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(g, out);
                }
                walk_expr(&arm.body, out);
            }
        }

        Expr::Try { body, handler, .. } => {
            walk_expr(body, out);
            walk_expr(handler, out);
        }

        Expr::Variant { args, .. } => match args {
            VariantArgs::Unit => {}
            VariantArgs::Positional(e) => walk_expr(e, out),
            VariantArgs::Record { fields, .. } => {
                for f in fields {
                    walk_expr(&f.value, out);
                }
            }
        },

        Expr::Function(f) => walk_expr(&f.body, out),

        // Leaves.
        Expr::Bool(_)
        | Expr::I8(_)
        | Expr::I16(_)
        | Expr::I32(_)
        | Expr::I64(_)
        | Expr::I128(_)
        | Expr::Isize(_)
        | Expr::U8(_)
        | Expr::U16(_)
        | Expr::U32(_)
        | Expr::U64(_)
        | Expr::U128(_)
        | Expr::Usize(_)
        | Expr::F32(_)
        | Expr::F64(_)
        | Expr::UnitLiteral { .. }
        | Expr::Utf8(_)
        | Expr::Ascii(_)
        | Expr::Utf16(_)
        | Expr::Utf32(_)
        | Expr::Identifier(..)
        | Expr::Symbol(_)
        | Expr::None
        | Expr::SelfKw(_)
        | Expr::ParentKw(_) => {}
    }
}

fn is_secret_callee(callee: &Expr) -> bool {
    matches!(callee, Expr::Identifier(name, _) if name == FN_NAME)
}

fn classify(args: &[Expr]) -> State {
    match args {
        [Expr::Utf8(s)] => {
            if crypto::is_blob(s) {
                State::Encrypted(s.clone())
            } else {
                State::Plaintext(s.clone())
            }
        }
        [_] => State::Invalid(format!(
            "{FN_NAME}() takes a plain string literal — a variable, an \
             interpolated string (`$\"…\"`) or a non-utf8 literal cannot be \
             encrypted in place"
        )),
        _ => State::Invalid(format!(
            "{FN_NAME}() takes exactly one argument, got {}",
            args.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(source: &str) -> Vec<State> {
        scan_source(source, "t.wcl")
            .unwrap()
            .into_iter()
            .map(|c| c.state)
            .collect()
    }

    #[test]
    fn finds_a_plain_call_in_a_field() {
        assert_eq!(
            states(r#"playbook "p" { vars { a = secret("pw") } }"#),
            vec![State::Plaintext("pw".into())]
        );
    }

    #[test]
    fn distinguishes_encrypted_from_plaintext() {
        let s =
            states(r#"playbook "p" { vars { a = secret("CWENC1.a.b.c") b = secret("clear") } }"#);
        assert_eq!(
            s,
            vec![
                State::Encrypted("CWENC1.a.b.c".into()),
                State::Plaintext("clear".into())
            ]
        );
    }

    #[test]
    fn finds_nested_call_sites() {
        let source = r#"
playbook "p" {
  vars {
    inlist = [secret("a"), "x"]
    joined = "prefix" + secret("b")
    interp = $"v=${secret("c")}"
    cond   = if true { secret("d") } else { "" }
  }
}
"#;
        assert_eq!(
            states(source),
            vec![
                State::Plaintext("a".into()),
                State::Plaintext("b".into()),
                State::Plaintext("c".into()),
                State::Plaintext("d".into()),
            ]
        );
    }

    #[test]
    fn finds_calls_in_step_properties_and_labels() {
        let source = r#"
playbook "p" {
  play "main" {
    step "s" { properties { password = secret("pw") } }
  }
}
"#;
        assert_eq!(states(source), vec![State::Plaintext("pw".into())]);
    }

    #[test]
    fn rejects_non_literal_arguments() {
        let s = states(r#"playbook "p" { vars { a = secret(other) } }"#);
        assert!(matches!(s[0], State::Invalid(_)));
        let s = states(r#"playbook "p" { vars { a = secret($"x${y}") } }"#);
        assert!(matches!(s[0], State::Invalid(_)));
    }

    #[test]
    fn rejects_wrong_arity() {
        let s = states(r#"playbook "p" { vars { a = secret("x", "y") } }"#);
        assert!(matches!(&s[0], State::Invalid(m) if m.contains("exactly one")));
        let s = states(r#"playbook "p" { vars { a = secret() } }"#);
        assert!(matches!(&s[0], State::Invalid(m) if m.contains("exactly one")));
    }

    #[test]
    fn ignores_an_unrelated_function_named_differently() {
        assert!(states(r#"playbook "p" { vars { a = notsecret("x") } }"#).is_empty());
    }

    #[test]
    fn spans_cover_the_whole_call() {
        let source = r#"playbook "p" { vars { a = secret("pw") } }"#;
        let calls = scan_source(source, "t.wcl").unwrap();
        let c = &calls[0];
        assert_eq!(&source[c.span.start..c.span.end], r#"secret("pw")"#);
    }
}
