//! Loading `playbook.wcl` + `pkgs/*/package.wcl` into the model.
//!
//! Loading performs validation stages 1–3 of the pipeline (§8 of the PRD):
//! parse, structural checks (references resolve, mandatory descriptions,
//! unique gather names, script files exist), and schema validation of step
//! properties / gather params against declared parameter schemas. Property
//! values that reference variables (unavailable until the gather phase)
//! are deferred to run time; everything statically evaluable is checked.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use wcl_lang::{Block, Document, EvalError, Environment, Field, Value};

use crate::convert::wcl_to_dyn;
use crate::diag::{Diag, wcl_span};
use crate::vocab;

use super::types::*;

/// Result of loading: a best-effort model plus every diagnostic found.
pub struct Loaded {
    pub playbook: Option<Playbook>,
    pub diags: Vec<Diag>,
}

pub fn load(dir: &Path) -> Loaded {
    let mut diags = Vec::new();

    let playbook_path = dir.join("playbook.wcl");
    let source = match std::fs::read_to_string(&playbook_path) {
        Ok(s) => s,
        Err(e) => {
            diags.push(Diag::bare(format!(
                "cannot read {}: {e}",
                playbook_path.display()
            )));
            return Loaded {
                playbook: None,
                diags,
            };
        }
    };

    // Packages first: step properties validate against resource schemas.
    let packages = load_packages(dir, &mut diags);

    let with_import = vocab::with_import(&source, vocab::PLAYBOOK_IMPORT, false);
    let env = Environment::new();
    let doc = match Document::open_at_with_loader(
        &with_import,
        "playbook.wcl",
        Some(dir.to_path_buf()),
        &env,
        vocab::loader(None),
    ) {
        Ok(d) => d,
        Err(e) => {
            diags.push(Diag::from_parse(e));
            return Loaded {
                playbook: None,
                diags,
            };
        }
    };

    for err in doc.schema_errors() {
        diags.push(Diag::from_eval(err, &playbook_path, &source));
    }
    check_required_fields(&doc, &playbook_path, &source, &mut diags);

    let Some(pb_block) = doc.block("playbook") else {
        diags.push(Diag::bare(format!(
            "{}: no `playbook` block found",
            playbook_path.display()
        )));
        return Loaded {
            playbook: None,
            diags,
        };
    };

    let ctx = Ctx {
        file: &playbook_path,
        source: &source,
        diags: &mut diags,
    };
    let mut loader = PlaybookLoader {
        ctx,
        packages: &packages,
    };
    let playbook = loader.load(dir, &pb_block, &source);

    Loaded {
        playbook: Some(Playbook {
            packages,
            ..playbook
        }),
        diags,
    }
}

/// Shared diagnostic context for one source file.
struct Ctx<'a> {
    file: &'a Path,
    source: &'a str,
    diags: &'a mut Vec<Diag>,
}

impl Ctx<'_> {
    fn err(&mut self, message: impl Into<String>, span: (usize, usize)) {
        self.diags.push(Diag::spanned(
            message,
            "here",
            self.file,
            self.source,
            span,
        ));
    }
}

struct PlaybookLoader<'a> {
    ctx: Ctx<'a>,
    packages: &'a BTreeMap<String, Package>,
}

impl PlaybookLoader<'_> {
    fn load(&mut self, dir: &Path, pb: &Block<'_>, source: &str) -> Playbook {
        let name = label_string(pb).unwrap_or_default();
        let description = string_field(pb, "description", &mut self.ctx).unwrap_or_default();
        let version = string_field(pb, "version", &mut self.ctx).unwrap_or_else(|| "0.0.0".into());

        let mut gathers = Vec::new();
        let mut vars = Vec::new();
        let mut plays = Vec::new();
        let mut seen_gather_names = HashSet::new();

        for block in pb.blocks() {
            match block.kind() {
                "gather" => {
                    if let Some(g) = self.load_gather(&block) {
                        if !seen_gather_names.insert(g.name.clone()) {
                            self.ctx.err(
                                format!("duplicate gather invocation name '{}'", g.name),
                                wcl_span(block.span()),
                            );
                        }
                        gathers.push(g);
                    }
                }
                "vars" => {
                    for f in block.fields() {
                        if let Some(expr_src) = field_expr_source(&f, source) {
                            vars.push(VarDecl {
                                name: f.name().to_string(),
                                expr_src,
                            });
                        }
                    }
                }
                "play" => plays.push(self.load_play(&block)),
                _ => {}
            }
        }

        // Variable names must be unique across vars and gathers (gatherer
        // results override declared vars by precedence, which would make a
        // same-named var unreachable — flag it).
        let mut var_names = HashSet::new();
        for v in &vars {
            if !var_names.insert(v.name.clone()) {
                self.ctx.diags.push(Diag::bare(format!(
                    "duplicate variable declaration '{}'",
                    v.name
                )));
            }
        }

        Playbook {
            name,
            version,
            description,
            root: dir.to_path_buf(),
            source: source.to_string(),
            gathers,
            vars,
            plays,
            packages: BTreeMap::new(), // filled by caller
        }
    }

    fn load_gather(&mut self, block: &Block<'_>) -> Option<GatherInvocation> {
        let name = label_string(block)?;
        let span = wcl_span(block.span());
        let from = string_field(block, "from", &mut self.ctx)?;
        let Some((package, gatherer)) = from.split_once('.') else {
            self.ctx.err(
                format!("gather 'from' must be 'package.gatherer', got '{from}'"),
                span,
            );
            return None;
        };
        let Some(pkg) = self.packages.get(package) else {
            self.ctx
                .err(format!("unknown package '{package}' in gather '{name}'"), span);
            return None;
        };
        let Some(decl) = pkg.gatherers.get(gatherer) else {
            self.ctx.err(
                format!("package '{package}' has no gatherer '{gatherer}'"),
                span,
            );
            return None;
        };
        // Validate static params against the gatherer's schema.
        if let Some(params) = block.blocks().find(|b| b.kind() == "params") {
            self.check_params(&params, &decl.params.clone(), &format!("gatherer '{from}'"));
        } else {
            self.check_param_block_missing(&decl.params.clone(), span, &format!("gatherer '{from}'"));
        }
        Some(GatherInvocation {
            name,
            package: package.to_string(),
            gatherer: gatherer.to_string(),
            span,
        })
    }

    fn load_play(&mut self, block: &Block<'_>) -> Play {
        let name = label_string(block).unwrap_or_default();
        let description = string_field(block, "description", &mut self.ctx).unwrap_or_default();
        let parallel = bool_field(block, "parallel", &mut self.ctx).unwrap_or(true);
        let mut items = Vec::new();
        self.load_items(block, &mut items, &[]);

        // Step names must be unique within the play; `requires` must
        // reference existing steps.
        let play = Play {
            name,
            description,
            parallel,
            items,
        };
        let mut names = HashSet::new();
        for step in play.steps() {
            if !names.insert(step.name.clone()) {
                self.ctx.err(
                    format!(
                        "duplicate step name '{}' in play '{}'",
                        step.name, play.name
                    ),
                    step.span,
                );
            }
        }
        for step in play.steps() {
            for req in &step.requires {
                if req == &step.name {
                    self.ctx.err(
                        format!("step '{}' requires itself", step.name),
                        step.span,
                    );
                } else if !names.contains(req) {
                    self.ctx.err(
                        format!(
                            "step '{}' requires unknown step '{}'",
                            step.name, req
                        ),
                        step.span,
                    );
                }
            }
        }
        play
    }

    fn load_items(&mut self, parent: &Block<'_>, out: &mut Vec<PlayItem>, containers: &[String]) {
        for block in parent.blocks() {
            match block.kind() {
                "step" => {
                    if let Some(step) = self.load_step(&block, containers) {
                        out.push(PlayItem::Step(step));
                    }
                }
                "container" => {
                    let name = label_string(&block).unwrap_or_default();
                    let description =
                        string_field(&block, "description", &mut self.ctx).unwrap_or_default();
                    let has_condition = block.fields().any(|f| f.name() == "condition");
                    let mut path = containers.to_vec();
                    path.push(name.clone());
                    let mut items = Vec::new();
                    self.load_items(&block, &mut items, &path);
                    out.push(PlayItem::Container(Container {
                        name,
                        description,
                        has_condition,
                        items,
                    }));
                }
                _ => {}
            }
        }
    }

    fn load_step(&mut self, block: &Block<'_>, containers: &[String]) -> Option<Step> {
        let span = wcl_span(block.span());
        let name = label_string(block)?;
        let description = string_field(block, "description", &mut self.ctx).unwrap_or_default();
        let resource_ref = string_field(block, "resource", &mut self.ctx)?;
        let Some((package, resource)) = resource_ref.split_once('.') else {
            self.ctx.err(
                format!("step resource must be 'package.resource', got '{resource_ref}'"),
                span,
            );
            return None;
        };
        let Some(pkg) = self.packages.get(package) else {
            self.ctx.err(
                format!("unknown package '{package}' in step '{name}'"),
                span,
            );
            return None;
        };
        let Some(decl) = pkg.resources.get(resource) else {
            self.ctx.err(
                format!("package '{package}' has no resource '{resource}'"),
                span,
            );
            return None;
        };

        let requires = string_list_field(block, "requires", &mut self.ctx).unwrap_or_default();

        let concurrency = match string_field_optional(block, "concurrency", &mut self.ctx) {
            Some(s) => match Concurrency::parse(&s) {
                Some(c) => {
                    // A step may tighten but never loosen.
                    if c < decl.concurrency {
                        self.ctx.err(
                            format!(
                                "step '{name}' declares concurrency '{}' which is looser than \
                                 resource '{package}.{resource}' ('{}'); steps may only tighten",
                                c.as_str(),
                                decl.concurrency.as_str()
                            ),
                            span,
                        );
                        None
                    } else {
                        Some(c)
                    }
                }
                None => {
                    self.ctx.err(
                        format!(
                            "invalid concurrency '{s}' (expected parallel, exclusive or global)"
                        ),
                        span,
                    );
                    None
                }
            },
            None => None,
        };

        // Validate properties against the resource's parameter schema.
        let params = decl.params.clone();
        if let Some(props) = block.blocks().find(|b| b.kind() == "properties") {
            self.check_params(&props, &params, &format!("resource '{package}.{resource}'"));
        } else {
            self.check_param_block_missing(&params, span, &format!("resource '{package}.{resource}'"));
        }

        let has_condition = block.fields().any(|f| f.name() == "condition");

        Some(Step {
            name,
            description,
            package: package.to_string(),
            resource: resource.to_string(),
            requires,
            concurrency,
            container_path: containers.to_vec(),
            has_condition,
            span,
        })
    }

    /// Validate a `properties` / `params` block against declared params:
    /// unknown key → error, missing required → error, coarse type mismatch
    /// → error when the value evaluates statically (variable references
    /// defer to run time).
    fn check_params(&mut self, block: &Block<'_>, decls: &[ParamDecl], what: &str) {
        let declared: HashMap<&str, &ParamDecl> =
            decls.iter().map(|p| (p.name.as_str(), p)).collect();
        let mut present = HashSet::new();
        for f in block.fields() {
            present.insert(f.name().to_string());
            let Some(decl) = declared.get(f.name()) else {
                self.ctx.err(
                    format!("unknown parameter '{}' for {what}", f.name()),
                    wcl_span(f.span()),
                );
                continue;
            };
            match f.value() {
                Ok(v) => match wcl_to_dyn(v) {
                    Ok(dv) => {
                        if !decl.ty.matches(&dv) {
                            self.ctx.err(
                                format!(
                                    "parameter '{}' of {what} expects {}, got {}",
                                    f.name(),
                                    decl.ty.as_str(),
                                    CoarseType::describe(&dv)
                                ),
                                wcl_span(f.span()),
                            );
                        }
                    }
                    Err(e) => {
                        self.ctx.err(
                            format!("parameter '{}' of {what}: {e}", f.name()),
                            wcl_span(f.span()),
                        );
                    }
                },
                // Variable references resolve at run time; checked then.
                Err(EvalError::UnresolvedReference { .. }) => {}
                Err(e) => {
                    self.ctx.diags.push(Diag::from_eval(
                        e.clone(),
                        self.ctx.file,
                        self.ctx.source,
                    ));
                }
            }
        }
        for p in decls {
            if p.required && p.default.is_none() && !present.contains(&p.name) {
                self.ctx.err(
                    format!("missing required parameter '{}' for {what}", p.name),
                    wcl_span(block.span()),
                );
            }
        }
    }

    fn check_param_block_missing(&mut self, decls: &[ParamDecl], span: (usize, usize), what: &str) {
        for p in decls {
            if p.required && p.default.is_none() {
                self.ctx.err(
                    format!("missing required parameter '{}' for {what}", p.name),
                    span,
                );
            }
        }
    }
}

// ------------------------------------------------------------- packages

fn load_packages(dir: &Path, diags: &mut Vec<Diag>) -> BTreeMap<String, Package> {
    let mut packages = BTreeMap::new();
    let pkgs_dir = dir.join("pkgs");
    let Ok(entries) = std::fs::read_dir(&pkgs_dir) else {
        return packages; // no pkgs/ folder is legal (steps then can't resolve)
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    paths.sort();
    for pkg_dir in paths {
        let folder = pkg_dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let wcl_path = pkg_dir.join("package.wcl");
        if !wcl_path.is_file() {
            diags.push(Diag::bare(format!(
                "package folder '{}' has no package.wcl",
                pkg_dir.display()
            )));
            continue;
        }
        match load_package(&pkg_dir, &wcl_path, diags) {
            Some(pkg) => {
                if pkg.name != folder {
                    diags.push(Diag::bare(format!(
                        "package '{}' lives in folder '{}'; the folder name and package \
                         name must match",
                        pkg.name, folder
                    )));
                }
                packages.insert(pkg.name.clone(), pkg);
            }
            None => {}
        }
    }
    packages
}

fn load_package(pkg_dir: &Path, wcl_path: &Path, diags: &mut Vec<Diag>) -> Option<Package> {
    let source = match std::fs::read_to_string(wcl_path) {
        Ok(s) => s,
        Err(e) => {
            diags.push(Diag::bare(format!("cannot read {}: {e}", wcl_path.display())));
            return None;
        }
    };
    let with_import = vocab::with_import(&source, vocab::PACKAGE_IMPORT, false);
    let env = Environment::new();
    let doc = match Document::open_at_with_loader(
        &with_import,
        &wcl_path.display().to_string(),
        Some(pkg_dir.to_path_buf()),
        &env,
        vocab::loader(None),
    ) {
        Ok(d) => d,
        Err(e) => {
            diags.push(Diag::from_parse(e));
            return None;
        }
    };
    for err in doc.schema_errors() {
        diags.push(Diag::from_eval(err, wcl_path, &source));
    }
    check_required_fields(&doc, wcl_path, &source, diags);

    let pkg_block = doc.block("package")?;
    let mut ctx = Ctx {
        file: wcl_path,
        source: &source,
        diags,
    };

    let name = label_string(&pkg_block)?;
    let description = string_field(&pkg_block, "description", &mut ctx).unwrap_or_default();
    let mut gatherers = BTreeMap::new();
    let mut resources = BTreeMap::new();

    for block in pkg_block.blocks() {
        match block.kind() {
            "gatherer" => {
                let Some(gname) = label_string(&block) else { continue };
                let gdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_field(&block, pkg_dir, &mut ctx) else { continue };
                let params = load_params(&block, &mut ctx);
                if gatherers
                    .insert(
                        gname.clone(),
                        GathererDecl {
                            name: gname.clone(),
                            description: gdesc,
                            script,
                            params,
                        },
                    )
                    .is_some()
                {
                    ctx.err(
                        format!("duplicate gatherer '{gname}'"),
                        wcl_span(block.span()),
                    );
                }
            }
            "resource" => {
                let Some(rname) = label_string(&block) else { continue };
                let rdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_field(&block, pkg_dir, &mut ctx) else { continue };
                let concurrency = match string_field_optional(&block, "concurrency", &mut ctx) {
                    Some(s) => match Concurrency::parse(&s) {
                        Some(c) => c,
                        None => {
                            ctx.err(
                                format!(
                                    "invalid concurrency '{s}' (expected parallel, exclusive \
                                     or global)"
                                ),
                                wcl_span(block.span()),
                            );
                            Concurrency::Parallel
                        }
                    },
                    None => Concurrency::Parallel,
                };
                let params = load_params(&block, &mut ctx);
                if resources
                    .insert(
                        rname.clone(),
                        ResourceDecl {
                            name: rname.clone(),
                            description: rdesc,
                            script,
                            concurrency,
                            params,
                        },
                    )
                    .is_some()
                {
                    ctx.err(
                        format!("duplicate resource '{rname}'"),
                        wcl_span(block.span()),
                    );
                }
            }
            _ => {}
        }
    }

    Some(Package {
        name,
        description,
        dir: pkg_dir.to_path_buf(),
        source: source.clone(),
        gatherers,
        resources,
    })
}

fn load_params(block: &Block<'_>, ctx: &mut Ctx<'_>) -> Vec<ParamDecl> {
    let mut params = Vec::new();
    let mut seen = HashSet::new();
    for b in block.blocks().filter(|b| b.kind() == "param") {
        let Some(name) = label_string(&b) else { continue };
        if !seen.insert(name.clone()) {
            ctx.err(format!("duplicate parameter '{name}'"), wcl_span(b.span()));
            continue;
        }
        let description = string_field(&b, "description", ctx).unwrap_or_default();
        let ty_str = string_field(&b, "type", ctx).unwrap_or_else(|| "string".into());
        let Some(ty) = CoarseType::parse(&ty_str) else {
            ctx.err(
                format!(
                    "parameter '{name}' has invalid type '{ty_str}' (expected string, int, \
                     float, bool, list or map)"
                ),
                wcl_span(b.span()),
            );
            continue;
        };
        let required = bool_field(&b, "required", ctx).unwrap_or(false);
        let default = b.fields().find(|f| f.name() == "default").and_then(|f| {
            match f.value() {
                Ok(v) => match wcl_to_dyn(v) {
                    Ok(dv) => {
                        if !ty.matches(&dv) {
                            ctx.err(
                                format!(
                                    "default for parameter '{name}' does not match its \
                                     declared type {}",
                                    ty.as_str()
                                ),
                                wcl_span(f.span()),
                            );
                            None
                        } else {
                            Some(dv)
                        }
                    }
                    Err(e) => {
                        ctx.err(format!("default for parameter '{name}': {e}"), wcl_span(f.span()));
                        None
                    }
                },
                Err(e) => {
                    ctx.diags
                        .push(Diag::from_eval(e.clone(), ctx.file, ctx.source));
                    None
                }
            }
        });
        params.push(ParamDecl {
            name,
            description,
            ty,
            required,
            default,
        });
    }
    params
}

// ------------------------------------------------------------- helpers

/// Engine-side required-field enforcement: WCL's block check flags unknown
/// fields but not missing ones, so walk every block and demand each
/// schema field that is non-optional, has no default, and is not bound
/// from the label or children.
fn check_required_fields(doc: &Document, file: &Path, source: &str, diags: &mut Vec<Diag>) {
    fn walk(block: &Block<'_>, file: &Path, source: &str, diags: &mut Vec<Diag>) {
        if let Some(schema) = block.schema() {
            for f in schema.effective_fields() {
                if f.optional()
                    || f.inline_slot().is_some()
                    || f.child_block_kind().is_some()
                    || f.children_block_kind().is_some()
                    || f.default_value().is_some()
                {
                    continue;
                }
                if !block.fields().any(|bf| bf.name() == f.name()) {
                    diags.push(Diag::spanned(
                        format!(
                            "'{}' block is missing required field '{}'",
                            block.kind(),
                            f.name()
                        ),
                        format!("declare '{}' here", f.name()),
                        file,
                        source,
                        wcl_span(block.span()),
                    ));
                }
            }
        }
        for b in block.blocks() {
            walk(&b, file, source, diags);
        }
    }
    for b in doc.blocks() {
        walk(&b, file, source, diags);
    }
}

pub(crate) fn label_string(block: &Block<'_>) -> Option<String> {
    match block.labels().ok()?.into_iter().next()? {
        Value::Utf8(s) | Value::Ascii(s) | Value::Identifier(s) => Some(s),
        _ => None,
    }
}

fn field_value(block: &Block<'_>, name: &str, ctx: &mut Ctx<'_>) -> Option<Value> {
    let f = block.fields().find(|f| f.name() == name)?;
    match f.value() {
        Ok(v) => Some(v.clone()),
        Err(e) => {
            ctx.diags
                .push(Diag::from_eval(e.clone(), ctx.file, ctx.source));
            None
        }
    }
}

fn string_field(block: &Block<'_>, name: &str, ctx: &mut Ctx<'_>) -> Option<String> {
    match field_value(block, name, ctx)? {
        Value::Utf8(s) | Value::Ascii(s) | Value::Identifier(s) => Some(s),
        other => {
            ctx.err(
                format!("field '{name}' must be a string, got {other:?}"),
                wcl_span(block.span()),
            );
            None
        }
    }
}

/// Like `string_field` but absent fields are simply `None` (no diag).
fn string_field_optional(block: &Block<'_>, name: &str, ctx: &mut Ctx<'_>) -> Option<String> {
    block.fields().find(|f| f.name() == name)?;
    string_field(block, name, ctx)
}

fn bool_field(block: &Block<'_>, name: &str, ctx: &mut Ctx<'_>) -> Option<bool> {
    match field_value(block, name, ctx)? {
        Value::Bool(b) => Some(b),
        other => {
            ctx.err(
                format!("field '{name}' must be a bool, got {other:?}"),
                wcl_span(block.span()),
            );
            None
        }
    }
}

fn string_list_field(block: &Block<'_>, name: &str, ctx: &mut Ctx<'_>) -> Option<Vec<String>> {
    match field_value(block, name, ctx)? {
        Value::List(items) => {
            let mut out = Vec::new();
            for item in items.iter() {
                match item {
                    Value::Utf8(s) | Value::Ascii(s) | Value::Identifier(s) => {
                        out.push(s.clone());
                    }
                    other => {
                        ctx.err(
                            format!("field '{name}' must be a list of strings, got {other:?}"),
                            wcl_span(block.span()),
                        );
                        return None;
                    }
                }
            }
            Some(out)
        }
        other => {
            ctx.err(
                format!("field '{name}' must be a list, got {other:?}"),
                wcl_span(block.span()),
            );
            None
        }
    }
}

fn script_field(block: &Block<'_>, pkg_dir: &Path, ctx: &mut Ctx<'_>) -> Option<PathBuf> {
    let rel = string_field(block, "script", ctx)?;
    let path = pkg_dir.join(&rel);
    if !path.is_file() {
        ctx.err(
            format!("script file '{rel}' does not exist in {}", pkg_dir.display()),
            wcl_span(block.span()),
        );
        return None;
    }
    Some(path)
}

/// Extract the raw expression text of `name = expr` from the source the
/// field was declared in.
fn field_expr_source(f: &Field<'_>, playbook_source: &str) -> Option<String> {
    let span = f.span();
    // Fields declared in imported files are not supported as vars; the
    // vars block lives in playbook.wcl itself.
    let slice = playbook_source.get(span.start..span.end)?;
    let (_, expr) = slice.split_once('=')?;
    Some(expr.trim().to_string())
}
