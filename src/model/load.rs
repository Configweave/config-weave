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

use wcl_lang::{Block, Document, Field, Value};
use wscript_std::DynValue;

use crate::convert::{FieldValueError, field_value_dyn, is_symbol_literal, wcl_to_dyn};
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
    // `secret()` must resolve to *something* for property type-checking to
    // proceed without a password; whether a call is actually encrypted is
    // decided by the syntactic scan in `engine::validate`.
    let env = crate::secrets::env::locked();
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
        composites: BTreeMap::new(),
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

/// How deep composites may nest before the loader calls it a mistake.
const MAX_COMPOSITE_DEPTH: usize = 8;

/// Names a composite argument may not take. Inside a block, WCL puts both
/// the block kinds it may contain and its schema's own field names in
/// scope, and either shadows a `let` of the same name — so an argument
/// called `step` would read the step declarations rather than its value.
/// The loader turns the name away rather than let the body read the wrong
/// thing. (`args` itself is handled separately: it holds the whole map.)
const SHADOWED_ARG_NAMES: &[&str] = &[
    "arg",
    "step",
    "properties",
    "symbol",
    "name",
    "description",
    "declared_args",
    "steps",
];

/// What a playbook `step` block turned out to name.
enum LoadedStep {
    Resource(Step),
    Composite(CompositeInvocation),
}

/// A step that invokes a composite. It never reaches the scheduler: the
/// loader expands it into a container of real steps.
struct CompositeInvocation {
    name: String,
    description: String,
    /// Empty for a playbook-local composite.
    package: String,
    composite: String,
    requires: Vec<String>,
    concurrency: Option<Concurrency>,
    span: (usize, usize),
}

/// A step as the *playbook author* wrote it — one entry per `step` block,
/// whether it names a resource or a composite. Uniqueness and `requires`
/// are checked against these, not against expanded steps.
struct DeclaredStep {
    name: String,
    requires: Vec<String>,
    span: (usize, usize),
}

/// Resolve a composite by the model's addressing: an empty package means
/// the playbook-local namespace.
fn lookup_composite<'a>(
    packages: &'a BTreeMap<String, Package>,
    playbook: &'a BTreeMap<String, CompositeDecl>,
    package: &str,
    name: &str,
) -> Option<&'a CompositeDecl> {
    if package.is_empty() {
        playbook.get(name)
    } else {
        packages.get(package)?.composites.get(name)
    }
}

/// The tighter of two optional concurrency classes.
fn max_opt(a: Option<Concurrency>, b: Option<Concurrency>) -> Option<Concurrency> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
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
        self.diags
            .push(Diag::spanned(message, "here", self.file, self.source, span));
    }
}

struct PlaybookLoader<'a> {
    ctx: Ctx<'a>,
    packages: &'a BTreeMap<String, Package>,
    /// Playbook-local composites, loaded before any play so a step can
    /// reference one by bare name.
    composites: BTreeMap<String, CompositeDecl>,
}

impl PlaybookLoader<'_> {
    fn condition_src(&self, block: &Block<'_>) -> Option<String> {
        let f = block.fields().find(|f| f.name() == "condition")?;
        field_expr_source(&f, self.ctx.source)
    }

    fn load(&mut self, dir: &Path, pb: &Block<'_>, source: &str) -> Playbook {
        let name = label_string(pb).unwrap_or_default();
        let description = string_field(pb, "description", &mut self.ctx).unwrap_or_default();
        let version = string_field(pb, "version", &mut self.ctx).unwrap_or_else(|| "0.0.0".into());

        let mut gathers = Vec::new();
        let mut vars = Vec::new();
        let mut plays = Vec::new();
        let mut seen_gather_names = HashSet::new();

        // Composites first, and in their own pass: a play references them,
        // and one composite body may reference another declared further
        // down the file.
        let mut pending = Pending::new(self.ctx.file, source);
        for block in pb.blocks().filter(|b| b.kind() == "composite") {
            let Some(c) = load_composite(&block, "", source, &mut self.ctx, &mut pending) else {
                continue;
            };
            if self.composites.insert(c.name.clone(), c).is_some() {
                self.ctx.err(
                    format!(
                        "duplicate composite '{}'",
                        label_string(&block).unwrap_or_default()
                    ),
                    wcl_span(block.span()),
                );
            }
        }
        validate_pending(self.packages, &self.composites, &pending, self.ctx.diags);

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
                // Expansion reads the composite map while diagnostics need
                // `&mut self`, so the map moves out for the duration.
                "play" => {
                    let composites = std::mem::take(&mut self.composites);
                    plays.push(self.load_play(&block, &composites));
                    self.composites = composites;
                }
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
            composites: std::mem::take(&mut self.composites),
            plays,
            packages: BTreeMap::new(), // filled by caller
        }
    }

    fn load_gather(&mut self, block: &Block<'_>) -> Option<GatherInvocation> {
        let name = label_string(block)?;
        let span = wcl_span(block.span());
        let from = string_field(block, "from", &mut self.ctx)?;
        let (package, gatherer) = split_qualified(
            &from,
            "gather 'from' must be 'package.gatherer'",
            span,
            &mut self.ctx,
        )?;
        let Some(pkg) = self.packages.get(package) else {
            self.ctx.err(
                format!("unknown package '{package}' in gather '{name}'"),
                span,
            );
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
            self.check_param_block_missing(
                &decl.params.clone(),
                span,
                &format!("gatherer '{from}'"),
            );
        }
        Some(GatherInvocation {
            name,
            package: package.to_string(),
            gatherer: gatherer.to_string(),
        })
    }

    fn load_play(
        &mut self,
        block: &Block<'_>,
        composites: &BTreeMap<String, CompositeDecl>,
    ) -> Play {
        let name = label_string(block).unwrap_or_default();
        let description = string_field(block, "description", &mut self.ctx).unwrap_or_default();
        let parallel = bool_field(block, "parallel", &mut self.ctx).unwrap_or(true);
        let mut items = Vec::new();
        let mut declared = Vec::new();
        self.load_items(block, &mut items, &[], &[], composites, &mut declared);

        let play = Play {
            name,
            description,
            parallel,
            items,
        };

        // Uniqueness and `requires` are checked over what the *playbook*
        // declares — a composite invocation counts once, under its own
        // name, not once per step it expands into. Steps inside a composite
        // are encapsulated: their `requires` were checked against their
        // siblings where the body was declared.
        let mut names = HashSet::new();
        for d in &declared {
            if !names.insert(d.name.clone()) {
                self.ctx.err(
                    format!("duplicate step name '{}' in play '{}'", d.name, play.name),
                    d.span,
                );
            }
        }
        for d in &declared {
            for req in &d.requires {
                if req == &d.name {
                    self.ctx
                        .err(format!("step '{}' requires itself", d.name), d.span);
                } else if !names.contains(req) {
                    self.ctx.err(
                        format!("step '{}' requires unknown step '{}'", d.name, req),
                        d.span,
                    );
                }
            }
        }
        play
    }

    fn load_items(
        &mut self,
        parent: &Block<'_>,
        out: &mut Vec<PlayItem>,
        containers: &[String],
        frames: &[CompositeFrame],
        composites: &BTreeMap<String, CompositeDecl>,
        declared: &mut Vec<DeclaredStep>,
    ) {
        for block in parent.blocks() {
            match block.kind() {
                "step" => match self.load_step(&block, containers, composites) {
                    Some(LoadedStep::Resource(step)) => {
                        declared.push(DeclaredStep {
                            name: step.name.clone(),
                            requires: step.requires.clone(),
                            span: step.span,
                        });
                        out.push(PlayItem::Step(step));
                    }
                    Some(LoadedStep::Composite(inv)) => {
                        declared.push(DeclaredStep {
                            name: inv.name.clone(),
                            requires: inv.requires.clone(),
                            span: inv.span,
                        });
                        let mut chain = Vec::new();
                        let container =
                            self.expand_composite(inv, containers, frames, composites, &mut chain);
                        out.push(PlayItem::Container(container));
                    }
                    None => {}
                },
                "container" => {
                    let name = label_string(&block).unwrap_or_default();
                    let description =
                        string_field(&block, "description", &mut self.ctx).unwrap_or_default();
                    let condition_src = self.condition_src(&block);
                    let mut path = containers.to_vec();
                    path.push(name.clone());
                    let mut items = Vec::new();
                    self.load_items(&block, &mut items, &path, frames, composites, declared);
                    out.push(PlayItem::Container(Container {
                        name,
                        description,
                        condition_src,
                        items,
                    }));
                }
                _ => {}
            }
        }
    }

    /// Expand one composite invocation into a synthetic container of real
    /// steps. Expansion is entirely static — the body declares its steps —
    /// so it happens here rather than at run time, and everything
    /// downstream (the DAG, the planner, reports, NDJSON) sees ordinary
    /// steps in ordinary containers.
    fn expand_composite(
        &mut self,
        inv: CompositeInvocation,
        containers: &[String],
        frames: &[CompositeFrame],
        composites: &BTreeMap<String, CompositeDecl>,
        chain: &mut Vec<String>,
    ) -> Container {
        let key = format!("{}.{}", inv.package, inv.composite);
        let mut container = Container {
            name: inv.name.clone(),
            description: inv.description.clone(),
            // The invocation's own condition is evaluated by the planner as
            // part of the frame walk, not as a container condition, because
            // it must be evaluated in the invoking document's scope.
            condition_src: None,
            items: Vec::new(),
        };
        if let Some(at) = chain.iter().position(|k| k == &key) {
            let mut cycle: Vec<&str> = chain[at..].iter().map(String::as_str).collect();
            cycle.push(&key);
            self.ctx
                .err(format!("composite cycle: {}", cycle.join(" -> ")), inv.span);
            return container;
        }
        if chain.len() >= MAX_COMPOSITE_DEPTH {
            self.ctx.err(
                format!(
                    "composite nesting deeper than {MAX_COMPOSITE_DEPTH} at '{}'",
                    inv.name
                ),
                inv.span,
            );
            return container;
        }

        let Some(decl) = lookup_composite(self.packages, composites, &inv.package, &inv.composite)
        else {
            // Already reported when the invocation was resolved.
            return container;
        };

        let mut path = containers.to_vec();
        path.push(inv.name.clone());
        let mut child_frames = frames.to_vec();
        child_frames.push(CompositeFrame {
            step: inv.name.clone(),
            package: inv.package.clone(),
            composite: inv.composite.clone(),
            requires: inv.requires.clone(),
        });

        chain.push(key);
        // Clone the templates: one declaration can be invoked many times,
        // and each instance carries its own path and provenance.
        let templates: Vec<Step> = decl.steps.clone();
        for tmpl in templates {
            let nested = lookup_composite(self.packages, composites, &tmpl.package, &tmpl.resource)
                .is_some();
            if nested {
                let inner = CompositeInvocation {
                    name: tmpl.name.clone(),
                    description: tmpl.description.clone(),
                    package: tmpl.package.clone(),
                    composite: tmpl.resource.clone(),
                    requires: tmpl.requires.clone(),
                    concurrency: tmpl.concurrency.or(inv.concurrency),
                    span: tmpl.span,
                };
                let c = self.expand_composite(inner, &path, &child_frames, composites, chain);
                container.items.push(PlayItem::Container(c));
                continue;
            }
            container.items.push(PlayItem::Step(Step {
                // The invocation may tighten every step of the body.
                concurrency: max_opt(tmpl.concurrency, inv.concurrency),
                container_path: path.clone(),
                frames: child_frames.clone(),
                ..tmpl
            }));
        }
        chain.pop();
        container
    }

    fn load_step(
        &mut self,
        block: &Block<'_>,
        containers: &[String],
        composites: &BTreeMap<String, CompositeDecl>,
    ) -> Option<LoadedStep> {
        let span = wcl_span(block.span());
        let name = label_string(block)?;
        let description = string_field(block, "description", &mut self.ctx).unwrap_or_default();
        let resource_ref = string_field(block, "resource", &mut self.ctx)?;
        let requires = string_list_field(block, "requires", &mut self.ctx).unwrap_or_default();
        let declared_concurrency = parse_concurrency_field(block, &mut self.ctx);
        let condition_src = self.condition_src(block);

        // An unqualified reference names a playbook-local composite; there
        // are no unqualified resources.
        let Some((package, target)) = resource_ref.split_once('.') else {
            if !composites.contains_key(&resource_ref) {
                self.ctx.err(
                    format!(
                        "step resource must be 'package.resource' or the name of a \
                         playbook composite, got '{resource_ref}'"
                    ),
                    span,
                );
                return None;
            }
            return Some(LoadedStep::Composite(self.composite_invocation(
                block,
                composites,
                name,
                description,
                String::new(),
                resource_ref,
                requires,
                declared_concurrency,
                span,
            )));
        };

        let Some(pkg) = self.packages.get(package) else {
            self.ctx.err(
                format!("unknown package '{package}' in step '{name}'"),
                span,
            );
            return None;
        };
        if pkg.composites.contains_key(target) {
            let (package, target) = (package.to_string(), target.to_string());
            return Some(LoadedStep::Composite(self.composite_invocation(
                block,
                composites,
                name,
                description,
                package,
                target,
                requires,
                declared_concurrency,
                span,
            )));
        }
        let Some(decl) = pkg.resources.get(target) else {
            self.ctx.err(
                format!("package '{package}' has no resource '{target}'"),
                span,
            );
            return None;
        };

        // A step may tighten but never loosen.
        let concurrency = declared_concurrency.filter(|c| {
            if *c < decl.concurrency {
                self.ctx.err(
                    format!(
                        "step '{name}' declares concurrency '{}' which is looser than \
                         resource '{package}.{target}' ('{}'); steps may only tighten",
                        c.as_str(),
                        decl.concurrency.as_str()
                    ),
                    span,
                );
                return false;
            }
            true
        });

        // Validate properties against the resource's parameter schema.
        let params = decl.params.clone();
        if let Some(props) = block.blocks().find(|b| b.kind() == "properties") {
            self.check_params(&props, &params, &format!("resource '{package}.{target}'"));
        } else {
            self.check_param_block_missing(
                &params,
                span,
                &format!("resource '{package}.{target}'"),
            );
        }

        Some(LoadedStep::Resource(Step {
            name,
            description,
            package: package.to_string(),
            resource: target.to_string(),
            requires,
            concurrency,
            container_path: containers.to_vec(),
            frames: Vec::new(),
            condition_src,
            span,
        }))
    }

    /// Validate a composite invocation's properties against the
    /// composite's params — the same check a resource step gets, so the
    /// diagnostics read identically.
    #[allow(clippy::too_many_arguments)]
    fn composite_invocation(
        &mut self,
        block: &Block<'_>,
        composites: &BTreeMap<String, CompositeDecl>,
        name: String,
        description: String,
        package: String,
        composite: String,
        requires: Vec<String>,
        concurrency: Option<Concurrency>,
        span: (usize, usize),
    ) -> CompositeInvocation {
        let what = if package.is_empty() {
            format!("composite '{composite}'")
        } else {
            format!("composite '{package}.{composite}'")
        };
        let params = lookup_composite(self.packages, composites, &package, &composite)
            .map(|d| d.params.clone())
            .unwrap_or_default();
        if let Some(props) = block.blocks().find(|b| b.kind() == "properties") {
            self.check_params(&props, &params, &what);
        } else {
            self.check_param_block_missing(&params, span, &what);
        }
        CompositeInvocation {
            name,
            description,
            package,
            composite,
            requires,
            concurrency,
            span,
        }
    }

    /// Validate a `properties` / `params` block against declared params:
    /// unknown key → error, missing required → error, coarse type mismatch
    /// → error when the value evaluates statically (variable references
    /// defer to run time).
    fn check_params(&mut self, block: &Block<'_>, decls: &[ParamDecl], what: &str) {
        let declared = declared_params(decls);
        let mut present = HashSet::new();
        for f in block.fields() {
            present.insert(f.name().to_string());
            let span = wcl_span(f.span());
            let Some(decl) = lookup_param(&declared, f.name(), what, span, &mut self.ctx) else {
                continue;
            };
            match field_value_dyn(&f) {
                Ok(fv) => {
                    check_symbol_spelling(
                        decl,
                        fv.symbol_literal,
                        &fv.value,
                        what,
                        span,
                        &mut self.ctx,
                    );
                    check_param_type(decl, &fv.value, what, span, &mut self.ctx)
                }
                Err(FieldValueError::Convert(e)) => {
                    self.ctx
                        .err(format!("parameter '{}' of {what}: {e}", f.name()), span);
                }
                // Variable references resolve at run time; checked then.
                Err(FieldValueError::Unresolved(_)) => {}
                Err(FieldValueError::Eval(e)) => {
                    self.ctx
                        .diags
                        .push(Diag::from_eval(e, self.ctx.file, self.ctx.source));
                }
            }
        }
        check_missing_required(decls, what, wcl_span(block.span()), &mut self.ctx, |n| {
            present.contains(n)
        });
    }

    fn check_param_block_missing(&mut self, decls: &[ParamDecl], span: (usize, usize), what: &str) {
        // No param block at all, so nothing is present.
        check_missing_required(decls, what, span, &mut self.ctx, |_| false);
    }
}

// ------------------------------------------------------------- packages

fn load_packages(dir: &Path, diags: &mut Vec<Diag>) -> BTreeMap<String, Package> {
    let mut packages = BTreeMap::new();
    // Test references can point at packages that load later, so their
    // resolution is a second pass once the full map exists.
    let mut pending = Vec::new();

    // The built-in package first: it is always available, with or without
    // a `pkgs/` folder.
    if let Some((pkg, pend)) = load_package_source(
        crate::builtin::PACKAGE_WCL,
        Path::new(crate::builtin::PACKAGE_PATH),
        Path::new(""),
        Scripts::Embedded,
        diags,
    ) {
        packages.insert(pkg.name.clone(), pkg);
        pending.push(pend);
    }

    let pkgs_dir = dir.join("pkgs");
    let Ok(entries) = std::fs::read_dir(&pkgs_dir) else {
        // No `pkgs/` folder is legal; the built-in package is still there.
        for p in &pending {
            validate_pending(&packages, &BTreeMap::new(), p, diags);
        }
        return packages;
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
        if folder == crate::builtin::PACKAGE {
            diags.push(Diag::bare(format!(
                "package folder '{}' uses the reserved name '{}'; that package is \
                 built into config-weave, and a local one would shadow it",
                pkg_dir.display(),
                crate::builtin::PACKAGE
            )));
            continue;
        }
        let wcl_path = pkg_dir.join("package.wcl");
        if !wcl_path.is_file() {
            diags.push(Diag::bare(format!(
                "package folder '{}' has no package.wcl",
                pkg_dir.display()
            )));
            continue;
        }
        if let Some((pkg, pend)) = load_package(&pkg_dir, &wcl_path, diags) {
            if pkg.name != folder {
                diags.push(Diag::bare(format!(
                    "package '{}' lives in folder '{}'; the folder name and package \
                     name must match",
                    pkg.name, folder
                )));
            }
            packages.insert(pkg.name.clone(), pkg);
            pending.push(pend);
        }
    }
    // A package is distributed on its own, so nothing in it may reach into
    // the playbook's composite namespace — hence the empty map here.
    for p in &pending {
        validate_pending(&packages, &BTreeMap::new(), p, diags);
    }
    packages
}

/// One statically evaluated params/properties/expect field. `symbol_literal`
/// records whether the value was *written* as `:name`: WCL symbols and
/// strings converge on the same `DynValue`, so the source form is the only
/// thing that still distinguishes them here.
#[derive(Clone)]
struct StaticPair {
    name: String,
    value: DynValue,
    span: (usize, usize),
    symbol_literal: bool,
}

/// One property field whose check is deferred to the second pass.
/// `value` is `None` when the expression references something that only
/// resolves at run time — a composite param, a playbook var — which is
/// exactly what the in-document `check_params` skips.
#[derive(Clone)]
struct DeferredPair {
    name: String,
    value: Option<DynValue>,
    span: (usize, usize),
    symbol_literal: bool,
}

/// Reference checks deferred to the second pass: resource/gatherer/composite
/// refs plus their evaluated params, with everything needed to render
/// diagnostics back into the file that declared them.
struct Pending {
    file: PathBuf,
    source: String,
    steps: Vec<PendingStepCheck>,
    gathers: Vec<PendingGatherCheck>,
    composite_steps: Vec<PendingCompositeStep>,
}

impl Pending {
    fn new(file: &Path, source: &str) -> Pending {
        Pending {
            file: file.to_path_buf(),
            source: source.to_string(),
            steps: Vec::new(),
            gathers: Vec::new(),
            composite_steps: Vec::new(),
        }
    }
}

/// One step of a composite body. Its target may be a resource *or* another
/// composite, and either may be declared in a package that loads later, so
/// resolution waits for the assembled map.
struct PendingCompositeStep {
    /// "step 'x' of composite 'pkg.y'" — diagnostic prefix.
    what: String,
    /// Package holding the target; empty means the playbook-local namespace.
    package: String,
    /// Resource or composite name.
    target: String,
    /// Evaluated properties (None: no `properties` block declared).
    props: Option<Vec<DeferredPair>>,
    /// Declared step-level concurrency, for the tighten-only check.
    concurrency: Option<Concurrency>,
    span: (usize, usize),
}

struct PendingStepCheck {
    /// "step 'x' of test 'y'" — diagnostic prefix.
    what: String,
    package: String,
    resource: String,
    /// Statically evaluated properties (None: no block declared).
    props: Option<Vec<StaticPair>>,
    span: (usize, usize),
}

struct PendingGatherCheck {
    what: String,
    package: String,
    gatherer: String,
    params: Option<Vec<StaticPair>>,
    /// Kept as pairs (not name/value) so the symbol spelling of each
    /// expectation survives to the second pass, where the gatherer's
    /// `returns` declarations are finally in reach.
    expect: Vec<StaticPair>,
    span: (usize, usize),
}

/// Second pass over everything whose target could live in a package that
/// had not loaded yet: test steps/gathers and composite bodies.
fn validate_pending(
    packages: &BTreeMap<String, Package>,
    playbook_composites: &BTreeMap<String, CompositeDecl>,
    p: &Pending,
    diags: &mut Vec<Diag>,
) {
    let mut ctx = Ctx {
        file: &p.file,
        source: &p.source,
        diags,
    };
    for s in &p.composite_steps {
        validate_composite_step(&mut ctx, packages, playbook_composites, s);
    }
    for s in &p.steps {
        let Some(pkg) = packages.get(&s.package) else {
            ctx.err(
                format!("unknown package '{}' in {}", s.package, s.what),
                s.span,
            );
            continue;
        };
        // A test asserts a status per declared step, matched by name in the
        // run report — but a composite reports one row per step it expands
        // into, under names the test never declared. Rather than let that
        // fail as "missing from the report", say so here.
        if pkg.composites.contains_key(&s.resource) {
            ctx.err(
                format!(
                    "{} targets the composite '{}.{}'; a test asserts a status per \
                     step, and a composite expands into steps of its own. Test the \
                     resources it invokes instead.",
                    s.what, s.package, s.resource
                ),
                s.span,
            );
            continue;
        }
        let Some(decl) = pkg.resources.get(&s.resource) else {
            ctx.err(
                format!(
                    "package '{}' has no resource '{}' (in {})",
                    s.package, s.resource, s.what
                ),
                s.span,
            );
            continue;
        };
        check_params_static(
            &mut ctx,
            s.props.as_deref(),
            &decl.params,
            &format!("resource '{}.{}' in {}", s.package, s.resource, s.what),
            s.span,
        );
    }
    for g in &p.gathers {
        let Some(pkg) = packages.get(&g.package) else {
            ctx.err(
                format!("unknown package '{}' in {}", g.package, g.what),
                g.span,
            );
            continue;
        };
        let Some(decl) = pkg.gatherers.get(&g.gatherer) else {
            ctx.err(
                format!(
                    "package '{}' has no gatherer '{}' (in {})",
                    g.package, g.gatherer, g.what
                ),
                g.span,
            );
            continue;
        };
        let what = format!("gatherer '{}.{}' in {}", g.package, g.gatherer, g.what);
        check_params_static(&mut ctx, g.params.as_deref(), &decl.params, &what, g.span);
        check_expect_static(&mut ctx, &g.expect, &decl.returns, &what);
    }
}

/// Resolve one composite inner step's target and check its properties.
/// A target may be a resource or another composite; a package declares
/// both in one namespace, so the lookup order is unambiguous.
fn validate_composite_step(
    ctx: &mut Ctx<'_>,
    packages: &BTreeMap<String, Package>,
    playbook_composites: &BTreeMap<String, CompositeDecl>,
    s: &PendingCompositeStep,
) {
    if s.package.is_empty() {
        let Some(decl) = playbook_composites.get(&s.target) else {
            ctx.err(
                format!(
                    "no playbook composite '{}' (in {}); a step resource without a \
                     '.' names a playbook-local composite",
                    s.target, s.what
                ),
                s.span,
            );
            return;
        };
        check_params_deferred(
            ctx,
            s.props.as_deref(),
            &decl.params,
            &format!("composite '{}' in {}", s.target, s.what),
            s.span,
        );
        return;
    }
    let Some(pkg) = packages.get(&s.package) else {
        ctx.err(
            format!("unknown package '{}' in {}", s.package, s.what),
            s.span,
        );
        return;
    };
    if let Some(decl) = pkg.composites.get(&s.target) {
        check_params_deferred(
            ctx,
            s.props.as_deref(),
            &decl.params,
            &format!("composite '{}.{}' in {}", s.package, s.target, s.what),
            s.span,
        );
        return;
    }
    let Some(decl) = pkg.resources.get(&s.target) else {
        ctx.err(
            format!(
                "package '{}' has no resource or composite '{}' (in {})",
                s.package, s.target, s.what
            ),
            s.span,
        );
        return;
    };
    let what = format!("resource '{}.{}' in {}", s.package, s.target, s.what);
    if let Some(c) = s.concurrency
        && c < decl.concurrency
    {
        ctx.err(
            format!(
                "{} declares concurrency '{}' which is looser than the resource's \
                 ('{}'); steps may only tighten",
                s.what,
                c.as_str(),
                decl.concurrency.as_str()
            ),
            s.span,
        );
    }
    check_params_deferred(ctx, s.props.as_deref(), &decl.params, &what, s.span);
}

/// `check_params` over pairs evaluated out of document scope: unknown key,
/// symbol spelling, coarse type mismatch, missing required. Pairs whose
/// value only resolves at run time contribute their presence but skip the
/// value checks — the run-time `apply_param_defaults` catches those.
fn check_params_deferred(
    ctx: &mut Ctx<'_>,
    pairs: Option<&[DeferredPair]>,
    decls: &[ParamDecl],
    what: &str,
    span: (usize, usize),
) {
    let declared = declared_params(decls);
    let mut present = HashSet::new();
    for p in pairs.unwrap_or_default() {
        present.insert(p.name.as_str());
        let Some(decl) = lookup_param(&declared, &p.name, what, p.span, ctx) else {
            continue;
        };
        let Some(value) = &p.value else { continue };
        check_symbol_spelling(decl, p.symbol_literal, value, what, p.span, ctx);
        check_param_type(decl, value, what, p.span, ctx);
    }
    check_missing_required(decls, what, span, ctx, |n| present.contains(n));
}

/// Evaluate every field of a `properties` block, tolerating values that
/// only resolve at run time (`Unresolved`) so composite bodies can refer to
/// their own params.
fn deferred_pairs(block: &Block<'_>, what: &str, ctx: &mut Ctx<'_>) -> Vec<DeferredPair> {
    let mut out = Vec::new();
    for f in block.fields() {
        let fspan = wcl_span(f.span());
        let (value, symbol_literal) = match field_value_dyn(&f) {
            Ok(fv) => (Some(fv.value), fv.symbol_literal),
            Err(FieldValueError::Unresolved(_)) => (None, false),
            Err(FieldValueError::Convert(e)) => {
                ctx.err(format!("property '{}' of {what}: {e}", f.name()), fspan);
                continue;
            }
            Err(FieldValueError::Eval(e)) => {
                ctx.diags.push(Diag::from_eval(e, ctx.file, ctx.source));
                continue;
            }
        };
        out.push(DeferredPair {
            name: f.name().to_string(),
            value,
            span: fspan,
            symbol_literal,
        });
    }
    out
}

/// Check a test `expect` block against the gatherer's `returns`
/// declarations. Only symbol-typed keys are constrained: an undeclared key
/// is fine (a gathered map may hold dynamic keys), but a key declared
/// `symbol` binds as a WCL symbol, so an expectation written `"systemd"`
/// would be comparing against a spelling the variable space never holds.
fn check_expect_static(
    ctx: &mut Ctx<'_>,
    expect: &[StaticPair],
    returns: &[ReturnDecl],
    what: &str,
) {
    for pair in expect {
        let Some(decl) = returns.iter().find(|r| r.name == pair.name) else {
            continue;
        };
        if decl.ty != CoarseType::Symbol {
            continue;
        }
        if !pair.symbol_literal
            && let DynValue::String(s) = &pair.value
        {
            ctx.err(
                format!(
                    "expectation '{}' of {what} is a symbol: write :{s}, not \"{s}\"",
                    pair.name
                ),
                pair.span,
            );
            continue;
        }
        if let Some(why) = decl.symbol_violation(&pair.value) {
            ctx.err(
                format!(
                    "expectation '{}' of {what} is not a declared symbol: {why}",
                    pair.name
                ),
                pair.span,
            );
        }
    }
}

/// Index declared params by name, for the membership and type checks
/// shared by the property/param validators.
fn declared_params(decls: &[ParamDecl]) -> HashMap<&str, &ParamDecl> {
    decls.iter().map(|p| (p.name.as_str(), p)).collect()
}

/// Resolve a supplied param name against the declarations, emitting the
/// shared "unknown parameter" diagnostic when it isn't declared.
fn lookup_param<'a>(
    declared: &HashMap<&str, &'a ParamDecl>,
    name: &str,
    what: &str,
    span: (usize, usize),
    ctx: &mut Ctx<'_>,
) -> Option<&'a ParamDecl> {
    match declared.get(name) {
        Some(decl) => Some(decl),
        None => {
            ctx.err(format!("unknown parameter '{name}' for {what}"), span);
            None
        }
    }
}

/// A symbol param must be *written* as `:name`. The string spelling reaches
/// scripts as exactly the same text, so accepting it would leave two ways
/// to say one thing and let `ensure = "absent"` drift into playbooks
/// alongside `ensure = :absent`.
fn check_symbol_spelling(
    decl: &ParamDecl,
    symbol_literal: bool,
    value: &DynValue,
    what: &str,
    span: (usize, usize),
    ctx: &mut Ctx<'_>,
) {
    if decl.ty != CoarseType::Symbol || symbol_literal {
        return;
    }
    let DynValue::String(s) = value else { return };
    ctx.err(
        format!(
            "parameter '{}' of {what} is a symbol: write :{s}, not \"{s}\"",
            decl.name
        ),
        span,
    );
}

/// Emit the shared coarse type-mismatch diagnostic when `value` doesn't fit
/// the declared type.
fn check_param_type(
    decl: &ParamDecl,
    value: &DynValue,
    what: &str,
    span: (usize, usize),
    ctx: &mut Ctx<'_>,
) {
    if !decl.ty.matches(value) {
        ctx.err(
            format!(
                "parameter '{}' of {what} expects {}, got {}",
                decl.name,
                decl.ty.as_str(),
                CoarseType::describe(value)
            ),
            span,
        );
        return;
    }
    if let Some(why) = decl.symbol_violation(value) {
        ctx.err(
            format!(
                "parameter '{}' of {what} is not a declared symbol: {why}",
                decl.name
            ),
            span,
        );
    }
}

/// Emit the shared "missing required parameter" diagnostic for every
/// required, defaultless param `is_present` reports as absent.
fn check_missing_required(
    decls: &[ParamDecl],
    what: &str,
    span: (usize, usize),
    ctx: &mut Ctx<'_>,
    is_present: impl Fn(&str) -> bool,
) {
    for p in decls {
        if p.required && p.default.is_none() && !is_present(&p.name) {
            ctx.err(
                format!("missing required parameter '{}' for {what}", p.name),
                span,
            );
        }
    }
}

/// Split a `package.member` reference, emitting `must_be` (e.g. "step
/// resource must be 'package.resource'") with the offending text when the
/// `.` separator is absent.
fn split_qualified<'a>(
    s: &'a str,
    must_be: &str,
    span: (usize, usize),
    ctx: &mut Ctx<'_>,
) -> Option<(&'a str, &'a str)> {
    match s.split_once('.') {
        Some(pair) => Some(pair),
        None => {
            ctx.err(format!("{must_be}, got '{s}'"), span);
            None
        }
    }
}

/// `check_params` over already-evaluated values: unknown key, coarse type
/// mismatch, missing required. Test params are fully static, so nothing
/// defers to run time.
fn check_params_static(
    ctx: &mut Ctx<'_>,
    pairs: Option<&[StaticPair]>,
    decls: &[ParamDecl],
    what: &str,
    span: (usize, usize),
) {
    let declared = declared_params(decls);
    let mut present = HashSet::new();
    for p in pairs.unwrap_or_default() {
        present.insert(p.name.as_str());
        let Some(decl) = lookup_param(&declared, &p.name, what, p.span, ctx) else {
            continue;
        };
        check_symbol_spelling(decl, p.symbol_literal, &p.value, what, p.span, ctx);
        check_param_type(decl, &p.value, what, p.span, ctx);
    }
    check_missing_required(decls, what, span, ctx, |n| present.contains(n));
}

fn load_package(
    pkg_dir: &Path,
    wcl_path: &Path,
    diags: &mut Vec<Diag>,
) -> Option<(Package, Pending)> {
    let source = match std::fs::read_to_string(wcl_path) {
        Ok(s) => s,
        Err(e) => {
            diags.push(Diag::bare(format!(
                "cannot read {}: {e}",
                wcl_path.display()
            )));
            return None;
        }
    };
    load_package_source(&source, wcl_path, pkg_dir, Scripts::Dir(pkg_dir), diags)
}

/// Load one package manifest. `dir` becomes [`Package::dir`] and is where
/// WCL resolves any file imports; `scripts` decides what a `script = "…"`
/// field names. The built-in package passes an empty `dir` and embedded
/// scripts, and is otherwise loaded exactly like a package on disk.
fn load_package_source(
    source: &str,
    wcl_path: &Path,
    dir: &Path,
    scripts: Scripts<'_>,
    diags: &mut Vec<Diag>,
) -> Option<(Package, Pending)> {
    let pkg_dir = dir;
    // `secret()` is playbook-only, but the builtin is registered here too
    // so a misplaced call reports the explicit rejection below rather than
    // a bare "unknown identifier".
    if let Ok(calls) = crate::secrets::scan::scan_source(source, &wcl_path.display().to_string()) {
        diags.extend(crate::secrets::reject_calls(
            source,
            wcl_path,
            &calls,
            "packages are shared and distributed via git, so a package \
             cannot hold a value encrypted under one playbook's password",
        ));
    }

    let with_import = vocab::with_import(source, vocab::PACKAGE_IMPORT, false);
    let env = crate::secrets::env::locked();
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
        diags.push(Diag::from_eval(err, wcl_path, source));
    }
    check_required_fields(&doc, wcl_path, source, diags);

    let pkg_block = doc.block("package")?;
    let mut ctx = Ctx {
        file: wcl_path,
        source,
        diags,
    };

    let name = label_string(&pkg_block)?;
    let description = string_field(&pkg_block, "description", &mut ctx).unwrap_or_default();
    let mut gatherers = BTreeMap::new();
    let mut resources = BTreeMap::new();
    let mut composites: BTreeMap<String, CompositeDecl> = BTreeMap::new();
    let mut test_blocks = Vec::new();
    let mut scenarios = Vec::new();
    let mut seen_scenarios = HashSet::new();
    let mut pending = Pending::new(wcl_path, source);

    for block in pkg_block.blocks() {
        match block.kind() {
            // Tests parse after resources/gatherers so own-package
            // references resolve regardless of declaration order.
            "test" => test_blocks.push(block),
            "composite" => {
                let Some(c) = load_composite(&block, &name, source, &mut ctx, &mut pending) else {
                    continue;
                };
                if composites.insert(c.name.clone(), c).is_some() {
                    ctx.err(
                        format!(
                            "duplicate composite '{}'",
                            label_string(&block).unwrap_or_default()
                        ),
                        wcl_span(block.span()),
                    );
                }
            }
            "scenario" => {
                let Some(sname) = label_string(&block) else {
                    continue;
                };
                let sdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_path_field(&block, pkg_dir, &mut ctx) else {
                    continue;
                };
                // `lab` is a directory holding a vmlab.wcl.
                let Some(lab_rel) = string_field(&block, "lab", &mut ctx) else {
                    continue;
                };
                let lab = pkg_dir.join(&lab_rel);
                if !lab.join("vmlab.wcl").is_file() {
                    ctx.err(
                        format!(
                            "scenario lab '{lab_rel}' must be a directory containing vmlab.wcl \
                             (looked in {})",
                            pkg_dir.display()
                        ),
                        wcl_span(block.span()),
                    );
                    continue;
                }
                if !seen_scenarios.insert(sname.clone()) {
                    ctx.err(
                        format!("duplicate scenario '{sname}'"),
                        wcl_span(block.span()),
                    );
                }
                scenarios.push(ScenarioDecl {
                    name: sname,
                    description: sdesc,
                    lab,
                    script,
                });
            }
            "gatherer" => {
                let Some(gname) = label_string(&block) else {
                    continue;
                };
                let gdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_field(&block, scripts, &mut ctx) else {
                    continue;
                };
                let params = load_params(&block, &mut ctx);
                let returns = load_returns(&block, &mut ctx);
                if gatherers
                    .insert(
                        gname.clone(),
                        GathererDecl {
                            name: gname.clone(),
                            description: gdesc,
                            script,
                            params,
                            returns,
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
                let Some(rname) = label_string(&block) else {
                    continue;
                };
                let rdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_field(&block, scripts, &mut ctx) else {
                    continue;
                };
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

    // Composites and resources share one namespace: a step's `resource`
    // field names either, so a collision would make one unreachable.
    for cname in composites.keys() {
        if let Some(r) = resources.get(cname) {
            ctx.err(
                format!(
                    "'{cname}' is declared as both a resource and a composite; they \
                     share one namespace, so a step could not name either"
                ),
                wcl_span(pkg_block.span()),
            );
            let _ = r;
        }
    }

    let mut tests = Vec::new();
    let mut seen_tests = HashSet::new();
    for block in &test_blocks {
        if let Some(t) = load_test(block, pkg_dir, &name, source, &mut ctx, &mut pending) {
            if !seen_tests.insert(t.name.clone()) {
                ctx.err(format!("duplicate test '{}'", t.name), t.span);
            }
            tests.push(t);
        }
    }

    // Tests sharing a group provision one instance, so every member must
    // agree on what that instance is. (A runtime --image / --template
    // override makes every test uniform and never trips this.)
    let mut groups: HashMap<&str, (&TestTarget, Option<&str>)> = HashMap::new();
    for t in &tests {
        let Some(g) = t.group.as_deref() else {
            continue;
        };
        let mem = t.memory.as_deref();
        match groups.get(g) {
            None => {
                groups.insert(g, (&t.target, mem));
            }
            Some((first, first_mem)) => {
                if **first != t.target {
                    ctx.err(
                        format!(
                            "test '{}' is in group '{g}' but provisions a {} while \
                             another member provisions a {first}; grouped tests share \
                             one instance and must agree",
                            t.name, t.target
                        ),
                        t.span,
                    );
                }
                if *first_mem != mem {
                    ctx.err(
                        format!(
                            "test '{}' is in group '{g}' but asks for memory {} while \
                             another member asks for {}; grouped tests share one \
                             instance and must agree",
                            t.name,
                            mem.unwrap_or("(default)"),
                            first_mem.unwrap_or("(default)")
                        ),
                        t.span,
                    );
                }
            }
        }
    }

    Some((
        Package {
            name,
            description,
            dir: dir.to_path_buf(),
            source: source.to_string(),
            gatherers,
            resources,
            composites,
            tests,
            scenarios,
        },
        pending,
    ))
}

/// One `composite` block: params, then a body of steps whose targets stay
/// unresolved until the second pass (they may name a package that loads
/// later, or a composite declared further down the same file).
///
/// `owner` is the package name, or empty for a playbook-local composite;
/// it decides what an unqualified `resource` means inside the body — the
/// declaring package's own namespace, or the playbook's composites.
fn load_composite(
    block: &Block<'_>,
    owner: &str,
    source: &str,
    ctx: &mut Ctx<'_>,
    pending: &mut Pending,
) -> Option<CompositeDecl> {
    let span = wcl_span(block.span());
    let name = label_string(block)?;
    let description = string_field(block, "description", ctx).unwrap_or_default();
    let params = load_params_of(block, "arg", ctx);
    // Arguments become `let` bindings in the body's scope, so their names
    // have to be readable there: `args` holds the whole map, and WCL puts
    // the enclosing block's kinds and schema fields in scope, either of
    // which would shadow a binding of the same name.
    for p in &params {
        if p.name == crate::engine::vars::ARGS_BINDING {
            ctx.err(
                format!(
                    "a composite argument cannot be called '{}'; that name is bound to \
                     the map of every argument the invocation supplied",
                    crate::engine::vars::ARGS_BINDING
                ),
                span,
            );
        } else if SHADOWED_ARG_NAMES.contains(&p.name.as_str()) {
            ctx.err(
                format!(
                    "a composite argument cannot be called '{}'; the name is already in \
                     scope inside the body and would shadow the argument",
                    p.name
                ),
                span,
            );
        } else if !crate::engine::vars::is_identifier(&p.name) {
            ctx.err(
                format!(
                    "composite argument '{}' is not a valid identifier; arguments bind \
                     as variables inside the body",
                    p.name
                ),
                span,
            );
        }
    }
    let label = if owner.is_empty() {
        format!("composite '{name}'")
    } else {
        format!("composite '{owner}.{name}'")
    };

    let mut steps = Vec::new();
    let mut seen = HashSet::new();
    for b in block.blocks().filter(|b| b.kind() == "step") {
        let Some(step) = load_composite_step(&b, owner, &label, source, ctx, pending) else {
            continue;
        };
        if !seen.insert(step.name.clone()) {
            ctx.err(
                format!("duplicate step name '{}' in {label}", step.name),
                step.span,
            );
            continue;
        }
        steps.push(step);
    }

    // `requires` inside a body names a sibling and nothing else: inner
    // steps are not addressable from outside the composite, and the body
    // cannot see the playbook's steps.
    let names: HashSet<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    for s in &steps {
        for req in &s.requires {
            if req == &s.name {
                ctx.err(format!("step '{}' requires itself", s.name), s.span);
            } else if !names.contains(req.as_str()) {
                ctx.err(
                    format!(
                        "step '{}' of {label} requires unknown step '{req}'; a \
                         composite step may only require a sibling step of the same \
                         composite",
                        s.name
                    ),
                    s.span,
                );
            }
        }
    }

    if steps.is_empty() {
        ctx.err(format!("{label} declares no steps"), span);
    }

    Some(CompositeDecl {
        name,
        description,
        params,
        steps,
    })
}

/// One step of a composite body. Target resolution and property checking
/// are deferred into `pending`; everything else is checked here.
fn load_composite_step(
    block: &Block<'_>,
    owner: &str,
    label: &str,
    source: &str,
    ctx: &mut Ctx<'_>,
    pending: &mut Pending,
) -> Option<Step> {
    let span = wcl_span(block.span());
    let name = label_string(block)?;
    let description = string_field(block, "description", ctx).unwrap_or_default();
    let target_ref = string_field(block, "resource", ctx)?;
    // Unqualified inside a package composite means that package's own
    // namespace (as it does in a test step); inside a playbook composite it
    // means a playbook-local composite.
    let (package, target) = match target_ref.split_once('.') {
        Some((p, r)) => (p.to_string(), r.to_string()),
        None => (owner.to_string(), target_ref.clone()),
    };
    let requires = string_list_field(block, "requires", ctx).unwrap_or_default();
    let concurrency = parse_concurrency_field(block, ctx);
    // `step` is one schema shared with `test`, so the assertion field has
    // to be turned away here rather than by the schema.
    if block.fields().any(|f| f.name() == "expect") {
        ctx.err(
            format!(
                "step '{name}' of {label} declares 'expect'; that is a test assertion, \
                 and a composite step has nothing to assert against"
            ),
            span,
        );
    }
    let what = format!("step '{name}' of {label}");
    let props = block
        .blocks()
        .find(|b| b.kind() == "properties")
        .map(|p| deferred_pairs(&p, &what, ctx));
    pending.composite_steps.push(PendingCompositeStep {
        what,
        package: package.clone(),
        target: target.clone(),
        props,
        concurrency,
        span,
    });
    let condition_src = block
        .fields()
        .find(|f| f.name() == "condition")
        .and_then(|f| field_expr_source(&f, source));

    Some(Step {
        name,
        description,
        package,
        resource: target,
        requires,
        concurrency,
        container_path: Vec::new(),
        frames: Vec::new(),
        condition_src,
        span,
    })
}

/// Parse an optional `concurrency` field, reporting an invalid class. The
/// tighten-only comparison against the resource's own class happens where
/// that declaration is in reach.
fn parse_concurrency_field(block: &Block<'_>, ctx: &mut Ctx<'_>) -> Option<Concurrency> {
    let s = string_field_optional(block, "concurrency", ctx)?;
    match Concurrency::parse(&s) {
        Some(c) => Some(c),
        None => {
            ctx.err(
                format!("invalid concurrency '{s}' (expected parallel, exclusive or global)"),
                wcl_span(block.span()),
            );
            None
        }
    }
}

/// Parse one `test` block. Reference and parameter-schema checks go into
/// `pending` for the cross-package second pass; everything test-local
/// (expect values, requires shape, uniqueness) is checked here.
fn load_test(
    block: &Block<'_>,
    pkg_dir: &Path,
    pkg_name: &str,
    source: &str,
    ctx: &mut Ctx<'_>,
    pending: &mut Pending,
) -> Option<TestDecl> {
    let span = wcl_span(block.span());
    let name = label_string(block)?;
    // Test and step names are spliced into the synthesized playbook as
    // string literals, so they must stay literal-safe.
    if name.contains('"') || name.contains('\\') {
        ctx.err(
            format!("test name '{name}' must not contain quotes or backslashes"),
            span,
        );
    }
    let description = string_field(block, "description", ctx).unwrap_or_default();
    // Exactly one of `image` (an OCI ref → a vmlab container) or
    // `template` (a vmlab template ref → a full VM). Neither is the common
    // authoring slip, both is ambiguous — reject each with the fix named.
    let image = string_field_optional(block, "image", ctx).filter(|s| !s.is_empty());
    let template = string_field_optional(block, "template", ctx).filter(|s| !s.is_empty());
    let target = match (image, template) {
        (Some(i), None) => TestTarget::Container(i),
        (None, Some(t)) => TestTarget::Vm(t),
        (None, None) => {
            ctx.err(
                "test declares neither 'image' nor 'template'; set image = \"debian:12\" \
                 to run in a container, or template = \"x86_64/ubuntu-24.04\" to run in a VM"
                    .to_string(),
                span,
            );
            return None;
        }
        (Some(_), Some(_)) => {
            ctx.err(
                "test declares both 'image' and 'template'; they are alternatives — \
                 'image' runs an OCI image as a container, 'template' clones a vmlab VM"
                    .to_string(),
                span,
            );
            return None;
        }
    };
    let memory = string_field_optional(block, "memory", ctx).filter(|m| !m.is_empty());
    // Empty `group = ""` reads as ungrouped (its own instance).
    let group = string_field_optional(block, "group", ctx).filter(|g| !g.is_empty());
    let setup = string_field_optional(block, "setup", ctx);
    let verify = string_field_optional(block, "verify", ctx).and_then(|rel| {
        let path = pkg_dir.join(&rel);
        if path.is_file() {
            Some(path)
        } else {
            ctx.err(
                format!(
                    "verify script '{rel}' does not exist in {}",
                    pkg_dir.display()
                ),
                span,
            );
            None
        }
    });

    let mut steps = Vec::new();
    let mut gathers = Vec::new();
    let mut seen_gathers = HashSet::new();

    for b in block.blocks() {
        match b.kind() {
            "step" => {
                let sspan = wcl_span(b.span());
                let Some(sname) = label_string(&b) else {
                    continue;
                };
                if sname.contains('"') || sname.contains('\\') {
                    ctx.err(
                        format!("step name '{sname}' must not contain quotes or backslashes"),
                        sspan,
                    );
                }
                let sdesc = string_field(&b, "description", ctx).unwrap_or_default();
                let Some(rref) = string_field(&b, "resource", ctx) else {
                    continue;
                };
                let (spkg, sres) = match rref.split_once('.') {
                    Some((p, r)) => (p.to_string(), r.to_string()),
                    None => (pkg_name.to_string(), rref.clone()),
                };
                let what = format!("step '{sname}' of test '{name}'");
                let expect = match string_field_optional(&b, "expect", ctx) {
                    Some(s) => match Expect::parse(&s) {
                        Some(e) => e,
                        None => {
                            ctx.err(
                                format!(
                                    "invalid expect '{s}' (expected converge, \
                                     already_configured, error, skip or reboot_required)"
                                ),
                                sspan,
                            );
                            Expect::Converge
                        }
                    },
                    None => Expect::Converge,
                };
                let requires = string_list_field(&b, "requires", ctx).unwrap_or_default();
                // Shared schema with `composite`'s steps; a test step runs
                // alone in its instance, so there is nothing to tighten.
                if b.fields().any(|f| f.name() == "concurrency") {
                    ctx.err(
                        format!(
                            "step '{sname}' of test '{name}' declares 'concurrency'; test \
                             steps run one at a time in their own instance"
                        ),
                        sspan,
                    );
                }
                let condition_src = b
                    .fields()
                    .find(|f| f.name() == "condition")
                    .and_then(|f| field_expr_source(&f, source));
                let props_block = b.blocks().find(|x| x.kind() == "properties");
                let properties_src = props_block.as_ref().and_then(|p| block_source(p, source));
                let props = props_block
                    .as_ref()
                    .map(|p| static_pairs(p, "parameter", &what, ctx));
                pending.steps.push(PendingStepCheck {
                    what,
                    package: spkg.clone(),
                    resource: sres.clone(),
                    props,
                    span: sspan,
                });
                steps.push(TestStep {
                    name: sname,
                    description: sdesc,
                    package: spkg,
                    resource: sres,
                    expect,
                    requires,
                    condition_src,
                    properties_src,
                    span: sspan,
                });
            }
            "gather" => {
                let gspan = wcl_span(b.span());
                let Some(gname) = label_string(&b) else {
                    continue;
                };
                let gdesc = string_field(&b, "description", ctx).unwrap_or_default();
                let Some(from) = string_field(&b, "from", ctx) else {
                    continue;
                };
                let (gpkg, ggath) = match from.split_once('.') {
                    Some((p, g)) => (p.to_string(), g.to_string()),
                    None => (pkg_name.to_string(), from.clone()),
                };
                let what = format!("gather '{gname}' of test '{name}'");
                let params_block = b.blocks().find(|x| x.kind() == "params");
                let params = params_block
                    .as_ref()
                    .map(|p| static_pairs(p, "parameter", &what, ctx));
                let expect = b
                    .blocks()
                    .find(|x| x.kind() == "expect")
                    .map(|p| static_pairs(&p, "expectation", &what, ctx))
                    .unwrap_or_default();
                if !seen_gathers.insert(gname.clone()) {
                    ctx.err(
                        format!("duplicate gather name '{gname}' in test '{name}'"),
                        gspan,
                    );
                }
                pending.gathers.push(PendingGatherCheck {
                    what,
                    package: gpkg.clone(),
                    gatherer: ggath.clone(),
                    params: params.clone(),
                    expect: expect.clone(),
                    span: gspan,
                });
                gathers.push(TestGather {
                    name: gname,
                    description: gdesc,
                    package: gpkg,
                    gatherer: ggath,
                    params: params
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| (p.name, p.value))
                        .collect(),
                    expect: expect.into_iter().map(|p| (p.name, p.value)).collect(),
                });
            }
            _ => {}
        }
    }

    // Step names unique; requires resolve within the test and never cross
    // from an expected-success step onto an expected-failure one (the
    // dependent would be blocked and the test could never pass).
    let mut names = HashSet::new();
    for s in &steps {
        if !names.insert(s.name.clone()) {
            ctx.err(
                format!("duplicate step name '{}' in test '{}'", s.name, name),
                s.span,
            );
        }
    }
    let expects: HashMap<&str, Expect> =
        steps.iter().map(|s| (s.name.as_str(), s.expect)).collect();
    for s in &steps {
        for req in &s.requires {
            if req == &s.name {
                ctx.err(format!("step '{}' requires itself", s.name), s.span);
                continue;
            }
            match expects.get(req.as_str()) {
                None => ctx.err(
                    format!(
                        "step '{}' requires unknown step '{}' in test '{}'",
                        s.name, req, name
                    ),
                    s.span,
                ),
                Some(dep) => {
                    let wants_success =
                        matches!(s.expect, Expect::Converge | Expect::AlreadyConfigured);
                    let dep_fails = matches!(dep, Expect::Error | Expect::RebootRequired);
                    if wants_success && dep_fails {
                        ctx.err(
                            format!(
                                "step '{}' (expect = \"{}\") requires step '{}' which \
                                 expects {}; the dependent would never run, so the test \
                                 could never pass",
                                s.name,
                                s.expect.as_str(),
                                req,
                                dep.as_str()
                            ),
                            s.span,
                        );
                    }
                }
            }
        }
    }

    if steps.is_empty() && gathers.is_empty() {
        ctx.err(
            format!("test '{name}' declares no steps and no gathers"),
            span,
        );
    }

    Some(TestDecl {
        name,
        description,
        target,
        memory,
        group,
        setup,
        verify,
        steps,
        gathers,
        span,
    })
}

/// Statically evaluate every field of a params/properties/expect block.
/// Test values must be static — the synthesized playbook has no variables
/// — so unresolved references are errors, not deferrals.
fn static_pairs(block: &Block<'_>, noun: &str, what: &str, ctx: &mut Ctx<'_>) -> Vec<StaticPair> {
    let mut out = Vec::new();
    for f in block.fields() {
        let fspan = wcl_span(f.span());
        match field_value_dyn(&f) {
            Ok(fv) => out.push(StaticPair {
                name: f.name().to_string(),
                value: fv.value,
                span: fspan,
                symbol_literal: fv.symbol_literal,
            }),
            Err(FieldValueError::Convert(e)) => {
                ctx.err(format!("{noun} '{}' of {what}: {e}", f.name()), fspan)
            }
            Err(FieldValueError::Unresolved(_)) => {
                ctx.err(
                    format!(
                        "{noun} '{}' of {what} references a variable; tests run against \
                         a variable-free playbook, so values must be static",
                        f.name()
                    ),
                    fspan,
                );
            }
            Err(FieldValueError::Eval(e)) => {
                ctx.diags.push(Diag::from_eval(e, ctx.file, ctx.source));
            }
        }
    }
    out
}

/// Raw source text of a whole block (e.g. `properties { … }`), for
/// verbatim splicing into the synthesized test playbook.
fn block_source(block: &Block<'_>, source: &str) -> Option<String> {
    let span = block.span();
    source.get(span.start..span.end).map(str::to_string)
}

/// The documented keys of a gatherer's returned value (`returns` blocks) —
/// docs metadata, so only name/description/type.
fn load_returns(block: &Block<'_>, ctx: &mut Ctx<'_>) -> Vec<ReturnDecl> {
    let mut returns = Vec::new();
    let mut seen = HashSet::new();
    for b in block.blocks().filter(|b| b.kind() == "returns") {
        let Some(name) = label_string(&b) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            ctx.err(
                format!("duplicate returns key '{name}'"),
                wcl_span(b.span()),
            );
            continue;
        }
        let description = string_field(&b, "description", ctx).unwrap_or_default();
        let ty_str = string_field(&b, "type", ctx).unwrap_or_else(|| "string".into());
        let Some(ty) = CoarseType::parse(&ty_str) else {
            ctx.err(
                format!(
                    "returns key '{name}' has invalid type '{ty_str}' (expected string, int, \
                     float, bool, list, map, symbol or duration)"
                ),
                wcl_span(b.span()),
            );
            continue;
        };
        let symbols = load_symbols(&b, &format!("returns key '{name}'"), ty, ctx);
        returns.push(ReturnDecl {
            name,
            description,
            ty,
            symbols,
        });
    }
    returns
}

/// The `symbol` child blocks of one `param` or `returns` block, in
/// declaration order. Only meaningful on a symbol-typed declaration;
/// declaring none leaves it unconstrained. `what` names the owner for
/// diagnostics ("parameter 'ensure'", "returns key 'init'").
fn load_symbols(
    block: &Block<'_>,
    what: &str,
    ty: CoarseType,
    ctx: &mut Ctx<'_>,
) -> Vec<SymbolDecl> {
    let mut symbols = Vec::new();
    let mut seen = HashSet::new();
    for b in block.blocks().filter(|b| b.kind() == "symbol") {
        if ty != CoarseType::Symbol {
            ctx.err(
                format!(
                    "{what} declares symbol values but its type is {} \
                     (only 'symbol' declarations can enumerate values)",
                    ty.as_str()
                ),
                wcl_span(b.span()),
            );
            continue;
        }
        let Some(name) = label_string(&b) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            ctx.err(
                format!("duplicate symbol ':{name}' for {what}"),
                wcl_span(b.span()),
            );
            continue;
        }
        let description = string_field(&b, "description", ctx).unwrap_or_default();
        symbols.push(SymbolDecl { name, description });
    }
    symbols
}

fn load_params(block: &Block<'_>, ctx: &mut Ctx<'_>) -> Vec<ParamDecl> {
    load_params_of(block, "param", ctx)
}

/// `load_params` over an arbitrary declaration block kind — resources and
/// gatherers declare `param`, composites declare `args`.
fn load_params_of(block: &Block<'_>, kind: &str, ctx: &mut Ctx<'_>) -> Vec<ParamDecl> {
    let mut params = Vec::new();
    let mut seen = HashSet::new();
    for b in block.blocks().filter(|b| b.kind() == kind) {
        let Some(name) = label_string(&b) else {
            continue;
        };
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
                     float, bool, list, map, symbol or duration)"
                ),
                wcl_span(b.span()),
            );
            continue;
        };
        let symbols = load_symbols(&b, &format!("parameter '{name}'"), ty, ctx);
        let required = bool_field(&b, "required", ctx).unwrap_or(false);
        let default_field = b.fields().find(|f| f.name() == "default");
        let default_span = default_field.as_ref().map(|f| wcl_span(f.span()));
        let default = default_field.and_then(|f| {
            // The vocab declares `default` as `utf8?`, so a duration
            // literal would be coerced against that and fail before the
            // unresolved-unit retry in `field_value` could fire. The
            // declared coarse type is known right here, so resolve
            // against `std.Duration` directly instead.
            let evaluated = if ty == CoarseType::Duration {
                f.value_typed(DURATION_TYPE)
            } else {
                f.value().cloned().map_err(|e| e.clone())
            };
            match evaluated {
                Ok(v) => match wcl_to_dyn(&v) {
                    Ok(dv) => {
                        if ty == CoarseType::Symbol
                            && !is_symbol_literal(&v)
                            && let DynValue::String(s) = &dv
                        {
                            ctx.err(
                                format!(
                                    "default for parameter '{name}' is a symbol: \
                                 write :{s}, not \"{s}\""
                                ),
                                wcl_span(f.span()),
                            );
                        }
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
                        ctx.err(
                            format!("default for parameter '{name}': {e}"),
                            wcl_span(f.span()),
                        );
                        None
                    }
                },
                Err(e) => {
                    ctx.diags.push(Diag::from_eval(e, ctx.file, ctx.source));
                    None
                }
            }
        });
        let decl = ParamDecl {
            name,
            description,
            ty,
            required,
            default,
            symbols,
        };
        // The declaration has to satisfy its own symbol set, or every use
        // that relies on the default would fail validation instead.
        if let Some(d) = &decl.default
            && let Some(why) = decl.symbol_violation(d)
        {
            ctx.err(
                format!(
                    "default for parameter '{}' is not a declared symbol: {why}",
                    decl.name
                ),
                default_span.unwrap_or_else(|| wcl_span(b.span())),
            );
        }
        params.push(decl);
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

pub fn label_string(block: &Block<'_>) -> Option<String> {
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

/// Where a package's `script = "…"` fields resolve to: a directory on
/// disk, or the sources compiled into the binary for the built-in package.
#[derive(Clone, Copy)]
enum Scripts<'a> {
    Dir(&'a Path),
    Embedded,
}

/// A `script = "…"` field that must name a real file: scenario drivers are
/// handed to the testlab as paths, so they have no embedded form.
fn script_path_field(block: &Block<'_>, pkg_dir: &Path, ctx: &mut Ctx<'_>) -> Option<PathBuf> {
    match script_field(block, Scripts::Dir(pkg_dir), ctx)? {
        ScriptSource::File(p) => Some(p),
        ScriptSource::Embedded(_) => None,
    }
}

fn script_field(
    block: &Block<'_>,
    scripts: Scripts<'_>,
    ctx: &mut Ctx<'_>,
) -> Option<ScriptSource> {
    let rel = string_field(block, "script", ctx)?;
    match scripts {
        Scripts::Dir(pkg_dir) => {
            let path = pkg_dir.join(&rel);
            if !path.is_file() {
                ctx.err(
                    format!(
                        "script file '{rel}' does not exist in {}",
                        pkg_dir.display()
                    ),
                    wcl_span(block.span()),
                );
                return None;
            }
            Some(ScriptSource::File(path))
        }
        Scripts::Embedded => match crate::builtin::SCRIPTS.iter().find(|(n, _)| *n == rel) {
            Some((name, _)) => Some(ScriptSource::Embedded(name)),
            None => {
                ctx.err(
                    format!("no built-in script '{rel}'"),
                    wcl_span(block.span()),
                );
                None
            }
        },
    }
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
