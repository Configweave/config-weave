//! AST-mutation helpers for `wcl_lang`'s edit path.
//!
//! `wcl_lang::edit` shipped these until the `wcl editor` was stripped
//! upstream; the editor was their only in-tree caller, so they went with
//! it. Both of this repo's writers still need them — `emit` (DocJson →
//! `playbook.wcl`) and the CLI's `pkgs/repo.wcl` store — so they live
//! here, next to the bigger of the two. They operate on the owned,
//! fully-public AST returned by [`wcl_lang::parse_for_edit`]. Synthesised
//! nodes carry a zero [`Span`]; the printer re-lays them out, so the
//! result round-trips through [`wcl_lang::format::to_source`].

use wcl_lang::ast::{self, Expr, Field, Item, Span, Trivia};

/// Set an existing field's value, or append a new `name = expr` field item to
/// `block` if no field with that name exists yet.
pub fn set_or_insert_field(block: &mut ast::Block, name: &str, expr: Expr) {
    for item in &mut block.items {
        if let Item::Field(f) = item
            && f.name == name
        {
            f.expr = expr;
            return;
        }
    }
    block.items.push(Item::Field(synth_field(name, expr)));
}

/// Set the block's inline-label slot `slot` (the positional value matched by a
/// schema field's `@inline(slot)`). Returns `false` when `slot` is past the end
/// of the existing labels and not the immediate next slot — the caller should
/// build labels contiguously rather than leave gaps.
pub fn set_label(block: &mut ast::Block, slot: usize, expr: Expr) -> bool {
    if slot < block.labels.len() {
        block.labels[slot] = expr;
        true
    } else if slot == block.labels.len() {
        block.labels.push(expr);
        true
    } else {
        false
    }
}

/// A WCL string-literal expression for `text`. The source printer chooses the
/// concrete rendering: a single-line escaped string, or a heredoc when the body
/// is multi-line and round-trips — so newlines and quotes in `text` survive.
pub fn string_literal_expr(text: &str) -> Expr {
    Expr::Utf8(text.to_string())
}

/// Build a fresh [`ast::Block`] from its kind, namespace qualifier, inline
/// label values (in slot order), and named `name = value` fields.
pub fn build_block(
    kind: &str,
    kind_ns: &[String],
    labels: Vec<Expr>,
    fields: Vec<(String, Expr)>,
) -> ast::Block {
    let items = fields
        .into_iter()
        .map(|(name, expr)| Item::Field(synth_field(&name, expr)))
        .collect();
    ast::Block {
        kind: kind.to_string(),
        kind_ns: kind_ns.to_vec(),
        conditional: false,
        slot_decl: None,
        labels,
        items,
        decorators: Vec::new(),
        span: Span::new(0, 0),
        leading_trivia: Vec::new(),
        trailing_comment: None,
        trailing_trivia: Vec::new(),
    }
}

/// Append `block` as a new top-level item of `src`, separated from the
/// preceding item by a blank line (when the source isn't empty).
pub fn append_top_level_block(src: &mut ast::Source, mut block: ast::Block) {
    if !src.items.is_empty() {
        block.leading_trivia.insert(0, Trivia::BlankLine);
    }
    src.items.push(Item::Block(block));
}

fn synth_field(name: &str, expr: Expr) -> Field {
    Field {
        name: name.to_string(),
        expr,
        decorators: Vec::new(),
        span: Span::new(0, 0),
        leading_trivia: Vec::new(),
        trailing_comment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcl_lang::{format, parse_for_edit};

    #[test]
    fn set_or_insert_updates_then_inserts() {
        let mut ast = parse_for_edit("card {\n  title = \"old\"\n}\n", "t").unwrap();
        let Item::Block(block) = &mut ast.items[0] else {
            panic!("expected a block")
        };
        set_or_insert_field(block, "title", string_literal_expr("new"));
        set_or_insert_field(block, "body", string_literal_expr("added"));
        let out = format::to_source(&ast);
        assert!(out.contains("title = \"new\""), "{out}");
        assert!(out.contains("body = \"added\""), "{out}");
        parse_for_edit(&out, "t2").unwrap();
    }

    #[test]
    fn build_and_append_block_with_labels() {
        let mut ast = parse_for_edit("page {\n}\n", "t").unwrap();
        let mut block = build_block(
            "card",
            &[],
            vec![],
            vec![("body".to_string(), string_literal_expr("Hi"))],
        );
        assert!(set_label(&mut block, 0, string_literal_expr("Title")));
        assert!(!set_label(&mut block, 3, string_literal_expr("gap")));
        append_top_level_block(&mut ast, block);
        let out = format::to_source(&ast);
        assert!(out.contains("card \"Title\""), "{out}");
        assert!(out.contains("body = \"Hi\""), "{out}");
        parse_for_edit(&out, "t2").unwrap();
    }
}
