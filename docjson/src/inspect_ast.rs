//! AST → DocJson extraction for the graphical editors.
//!
//! Works on `wcl_lang::parse_for_edit` output (no evaluation, no
//! imports), so it handles unsaved buffers and files with unresolved
//! vocab. Extraction **fails closed**: any construct DocJson cannot
//! represent inside a managed region (a non-literal name, a nested
//! block inside a schemaless map, …) is a diagnostic, never a silent
//! drop — the UI then falls back to the text editor. Unknown items in
//! *structural* blocks (a `let`, an unrecognized child block) are
//! tolerated: the forms don't show them and the sync preserves them.

use wcl_lang::ast::{Block, Expr, Item, Source};
use wcl_lang::format::to_source_expr;

use crate::docjson::*;

type Diags = Vec<String>;

pub fn extract_playbook(src: &Source) -> Result<PlaybookDoc, Diags> {
    let block = find_top_block(src, "playbook")
        .ok_or_else(|| vec!["no `playbook` block found".to_string()])?;
    let mut diags = Vec::new();

    let name = label_string(block, "playbook", &mut diags);
    let description = req_lit_string(block, "description", "playbook", &mut diags);
    let version = opt_lit_string(block, "version", "playbook", &mut diags);

    let mut gathers = Vec::new();
    let mut vars = Vec::new();
    let mut plays = Vec::new();
    for item in &block.items {
        let Item::Block(b) = item else { continue };
        match b.kind.as_str() {
            "gather" => gathers.push(extract_gather(b, &mut diags)),
            "vars" => vars = extract_kvs(b, "vars", &mut diags),
            "play" => plays.push(extract_play(b, &mut diags)),
            _ => {} // preserved by the sync, not editable in forms
        }
    }

    if diags.is_empty() {
        Ok(PlaybookDoc {
            name,
            description,
            version,
            gathers,
            vars,
            plays,
        })
    } else {
        Err(diags)
    }
}

pub fn extract_package(src: &Source) -> Result<PackageDoc, Diags> {
    let block = find_top_block(src, "package")
        .ok_or_else(|| vec!["no `package` block found".to_string()])?;
    let mut diags = Vec::new();

    let name = label_string(block, "package", &mut diags);
    let description = req_lit_string(block, "description", "package", &mut diags);

    let mut gatherers = Vec::new();
    let mut resources = Vec::new();
    let mut tests = Vec::new();
    let mut scenarios = Vec::new();
    for item in &block.items {
        let Item::Block(b) = item else { continue };
        match b.kind.as_str() {
            "gatherer" => gatherers.push(extract_gatherer(b, &mut diags)),
            "resource" => resources.push(extract_resource(b, &mut diags)),
            "test" => tests.push(extract_test(b, &mut diags)),
            "scenario" => scenarios.push(extract_scenario(b, &mut diags)),
            _ => {}
        }
    }

    if diags.is_empty() {
        Ok(PackageDoc {
            name,
            description,
            gatherers,
            resources,
            tests,
            scenarios,
        })
    } else {
        Err(diags)
    }
}

// ------------------------------------------------------------ playbook

fn extract_gather(b: &Block, diags: &mut Diags) -> GatherDoc {
    let name = label_string(b, "gather", diags);
    GatherDoc {
        orig: Some(name.clone()),
        description: opt_lit_string(b, "description", "gather", diags),
        from: req_lit_string(b, "from", &format!("gather '{name}'"), diags),
        params: child_kvs(b, "params", diags),
        name,
    }
}

fn extract_play(b: &Block, diags: &mut Diags) -> PlayDoc {
    let name = label_string(b, "play", diags);
    let ctx = format!("play '{name}'");
    PlayDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        parallel: opt_lit_bool(b, "parallel", &ctx, diags),
        items: extract_play_items(b, diags),
        name,
    }
}

fn extract_play_items(parent: &Block, diags: &mut Diags) -> Vec<PlayItemDoc> {
    let mut items = Vec::new();
    for item in &parent.items {
        let Item::Block(b) = item else { continue };
        match b.kind.as_str() {
            "step" => items.push(PlayItemDoc::Step(extract_step(b, diags))),
            "container" => items.push(PlayItemDoc::Container(extract_container(b, diags))),
            _ => {}
        }
    }
    items
}

fn extract_step(b: &Block, diags: &mut Diags) -> StepDoc {
    let name = label_string(b, "step", diags);
    let ctx = format!("step '{name}'");
    StepDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        resource: req_lit_string(b, "resource", &ctx, diags),
        condition: expr_source(b, "condition"),
        requires: string_list(b, "requires", &ctx, diags),
        concurrency: opt_lit_string(b, "concurrency", &ctx, diags),
        properties: child_kvs(b, "properties", diags),
        name,
    }
}

fn extract_container(b: &Block, diags: &mut Diags) -> ContainerDoc {
    let name = label_string(b, "container", diags);
    let ctx = format!("container '{name}'");
    ContainerDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        condition: expr_source(b, "condition"),
        items: extract_play_items(b, diags),
        name,
    }
}

// ------------------------------------------------------------- package

fn extract_gatherer(b: &Block, diags: &mut Diags) -> GathererDoc {
    let name = label_string(b, "gatherer", diags);
    let ctx = format!("gatherer '{name}'");
    GathererDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        script: req_lit_string(b, "script", &ctx, diags),
        params: extract_params(b, diags),
        name,
    }
}

fn extract_resource(b: &Block, diags: &mut Diags) -> ResourceDoc {
    let name = label_string(b, "resource", diags);
    let ctx = format!("resource '{name}'");
    ResourceDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        script: req_lit_string(b, "script", &ctx, diags),
        concurrency: opt_lit_string(b, "concurrency", &ctx, diags),
        params: extract_params(b, diags),
        name,
    }
}

fn extract_params(parent: &Block, diags: &mut Diags) -> Vec<ParamDoc> {
    let mut params = Vec::new();
    for item in &parent.items {
        let Item::Block(b) = item else { continue };
        if b.kind != "param" {
            continue;
        }
        let name = label_string(b, "param", diags);
        let ctx = format!("param '{name}'");
        params.push(ParamDoc {
            orig: Some(name.clone()),
            description: req_lit_string(b, "description", &ctx, diags),
            ty: req_lit_string(b, "type", &ctx, diags),
            required: opt_lit_bool(b, "required", &ctx, diags),
            default: field_expr(b, "default").map(val_of),
            name,
        });
    }
    params
}

fn extract_test(b: &Block, diags: &mut Diags) -> TestDoc {
    let name = label_string(b, "test", diags);
    let ctx = format!("test '{name}'");
    let mut steps = Vec::new();
    let mut gathers = Vec::new();
    for item in &b.items {
        let Item::Block(c) = item else { continue };
        match c.kind.as_str() {
            "step" => steps.push(extract_test_step(c, diags)),
            "gather" => gathers.push(extract_test_gather(c, diags)),
            _ => {}
        }
    }
    TestDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        backend: opt_lit_string(b, "backend", &ctx, diags),
        image: req_lit_string(b, "image", &ctx, diags),
        group: opt_lit_string(b, "group", &ctx, diags),
        setup: opt_lit_string(b, "setup", &ctx, diags),
        verify: opt_lit_string(b, "verify", &ctx, diags),
        steps,
        gathers,
        name,
    }
}

fn extract_test_step(b: &Block, diags: &mut Diags) -> TestStepDoc {
    let name = label_string(b, "step", diags);
    let ctx = format!("test step '{name}'");
    TestStepDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        resource: req_lit_string(b, "resource", &ctx, diags),
        expect: opt_lit_string(b, "expect", &ctx, diags),
        condition: expr_source(b, "condition"),
        requires: string_list(b, "requires", &ctx, diags),
        properties: child_kvs(b, "properties", diags),
        name,
    }
}

fn extract_test_gather(b: &Block, diags: &mut Diags) -> TestGatherDoc {
    let name = label_string(b, "gather", diags);
    let ctx = format!("test gather '{name}'");
    TestGatherDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        from: req_lit_string(b, "from", &ctx, diags),
        params: child_kvs(b, "params", diags),
        expect: child_kvs(b, "expect", diags),
        name,
    }
}

fn extract_scenario(b: &Block, diags: &mut Diags) -> ScenarioDoc {
    let name = label_string(b, "scenario", diags);
    let ctx = format!("scenario '{name}'");
    ScenarioDoc {
        orig: Some(name.clone()),
        description: req_lit_string(b, "description", &ctx, diags),
        lab: req_lit_string(b, "lab", &ctx, diags),
        script: req_lit_string(b, "script", &ctx, diags),
        name,
    }
}

// -------------------------------------------------------------- shared

pub(crate) fn find_top_block<'a>(src: &'a Source, kind: &str) -> Option<&'a Block> {
    src.items.iter().find_map(|i| match i {
        Item::Block(b) if b.kind == kind => Some(b),
        _ => None,
    })
}

fn lit_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Utf8(s) | Expr::Ascii(s) => Some(s.clone()),
        _ => None,
    }
}

/// The block's name label. Non-literal labels fail closed: names are
/// the sync's matching key and appear in `requires` references.
fn label_string(b: &Block, kind: &str, diags: &mut Diags) -> String {
    match b.labels.first().and_then(lit_string) {
        Some(s) => s,
        None => {
            diags.push(format!(
                "{kind} block has a non-literal or missing name label — not editable visually"
            ));
            String::new()
        }
    }
}

fn field_expr<'a>(b: &'a Block, name: &str) -> Option<&'a Expr> {
    b.items.iter().find_map(|i| match i {
        Item::Field(f) if f.name == name => Some(&f.expr),
        _ => None,
    })
}

/// A literal (form-editable) value, or the raw expression source.
pub(crate) fn val_of(expr: &Expr) -> Val {
    match expr {
        Expr::Utf8(s) | Expr::Ascii(s) => Val::Lit(serde_json::Value::String(s.clone())),
        Expr::Bool(v) => Val::Lit(serde_json::Value::Bool(*v)),
        Expr::I8(v) => Val::Lit((*v).into()),
        Expr::I16(v) => Val::Lit((*v).into()),
        Expr::I32(v) => Val::Lit((*v).into()),
        Expr::I64(v) => Val::Lit((*v).into()),
        Expr::U8(v) => Val::Lit((*v).into()),
        Expr::U16(v) => Val::Lit((*v).into()),
        Expr::U32(v) => Val::Lit((*v).into()),
        Expr::U64(v) => Val::Lit((*v).into()),
        Expr::F32(v) => Val::Lit(
            serde_json::Number::from_f64(f64::from(*v))
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        ),
        Expr::F64(v) => Val::Lit(
            serde_json::Number::from_f64(*v)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        ),
        other => Val::Expr(to_source_expr(other)),
    }
}

fn opt_lit_string(b: &Block, name: &str, ctx: &str, diags: &mut Diags) -> Option<String> {
    let expr = field_expr(b, name)?;
    match lit_string(expr) {
        Some(s) => Some(s),
        None => {
            diags.push(format!(
                "{ctx}: field '{name}' is not a plain string — not editable visually"
            ));
            None
        }
    }
}

fn req_lit_string(b: &Block, name: &str, ctx: &str, diags: &mut Diags) -> String {
    match field_expr(b, name) {
        None => String::new(), // missing required field: empty form input
        Some(expr) => match lit_string(expr) {
            Some(s) => s,
            None => {
                diags.push(format!(
                    "{ctx}: field '{name}' is not a plain string — not editable visually"
                ));
                String::new()
            }
        },
    }
}

fn opt_lit_bool(b: &Block, name: &str, ctx: &str, diags: &mut Diags) -> Option<bool> {
    match field_expr(b, name)? {
        Expr::Bool(v) => Some(*v),
        _ => {
            diags.push(format!(
                "{ctx}: field '{name}' is not a plain bool — not editable visually"
            ));
            None
        }
    }
}

/// The raw source of an expression-valued field (condition).
fn expr_source(b: &Block, name: &str) -> Option<String> {
    field_expr(b, name).map(to_source_expr)
}

fn string_list(b: &Block, name: &str, ctx: &str, diags: &mut Diags) -> Vec<String> {
    let Some(expr) = field_expr(b, name) else {
        return Vec::new();
    };
    let Expr::ListLit { elements, .. } = expr else {
        diags.push(format!(
            "{ctx}: field '{name}' is not a plain list of strings — not editable visually"
        ));
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in elements {
        match lit_string(e) {
            Some(s) => out.push(s),
            None => {
                diags.push(format!(
                    "{ctx}: field '{name}' has a non-literal element — not editable visually"
                ));
                return Vec::new();
            }
        }
    }
    out
}

/// The kv entries of a schemaless child map block (`properties`,
/// `params`, `vars`, `expect`). Anything but plain fields inside fails
/// closed — the kv editor could not show it and the sync would drop it.
fn child_kvs(parent: &Block, kind: &str, diags: &mut Diags) -> Vec<Kv> {
    let Some(map) = parent.items.iter().find_map(|i| match i {
        Item::Block(b) if b.kind == kind => Some(b),
        _ => None,
    }) else {
        return Vec::new();
    };
    extract_kvs(map, kind, diags)
}

fn extract_kvs(map: &Block, kind: &str, diags: &mut Diags) -> Vec<Kv> {
    let mut kvs = Vec::new();
    for item in &map.items {
        match item {
            Item::Field(f) => kvs.push(Kv {
                key: f.name.clone(),
                value: val_of(&f.expr),
            }),
            other => {
                diags.push(format!(
                    "`{kind}` block contains a non-field item ({}) — not editable visually",
                    item_kind_name(other)
                ));
            }
        }
    }
    kvs
}

fn item_kind_name(item: &Item) -> &'static str {
    match item {
        Item::Field(_) => "field",
        Item::Let(_) => "let binding",
        Item::Block(_) => "nested block",
        Item::Import(_) => "import",
        _ => "declaration",
    }
}
