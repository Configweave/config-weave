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

use wcl_lang::{Block, Document, Environment, EvalError, Field, Value};
use wscript_std::DynValue;

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
        self.diags
            .push(Diag::spanned(message, "here", self.file, self.source, span));
    }
}

struct PlaybookLoader<'a> {
    ctx: Ctx<'a>,
    packages: &'a BTreeMap<String, Package>,
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
                    self.ctx
                        .err(format!("step '{}' requires itself", step.name), step.span);
                } else if !names.contains(req) {
                    self.ctx.err(
                        format!("step '{}' requires unknown step '{}'", step.name, req),
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
                    let condition_src = self.condition_src(&block);
                    let mut path = containers.to_vec();
                    path.push(name.clone());
                    let mut items = Vec::new();
                    self.load_items(&block, &mut items, &path);
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

    fn load_step(&mut self, block: &Block<'_>, containers: &[String]) -> Option<Step> {
        let span = wcl_span(block.span());
        let name = label_string(block)?;
        let description = string_field(block, "description", &mut self.ctx).unwrap_or_default();
        let resource_ref = string_field(block, "resource", &mut self.ctx)?;
        let (package, resource) = split_qualified(
            &resource_ref,
            "step resource must be 'package.resource'",
            span,
            &mut self.ctx,
        )?;
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
            self.check_param_block_missing(
                &params,
                span,
                &format!("resource '{package}.{resource}'"),
            );
        }

        let condition_src = self.condition_src(block);

        Some(Step {
            name,
            description,
            package: package.to_string(),
            resource: resource.to_string(),
            requires,
            concurrency,
            container_path: containers.to_vec(),
            condition_src,
            span,
        })
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
            match f.value() {
                Ok(v) => match wcl_to_dyn(v) {
                    Ok(dv) => check_param_type(decl, &dv, what, span, &mut self.ctx),
                    Err(e) => {
                        self.ctx
                            .err(format!("parameter '{}' of {what}: {e}", f.name()), span);
                    }
                },
                // Variable references resolve at run time; checked then.
                Err(EvalError::UnresolvedReference { .. }) => {}
                Err(e) => {
                    self.ctx
                        .diags
                        .push(Diag::from_eval(e.clone(), self.ctx.file, self.ctx.source));
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
    // Test references can point at packages that load later, so their
    // resolution is a second pass once the full map exists.
    let mut pending = Vec::new();
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
    for p in &pending {
        validate_tests(&packages, p, diags);
    }
    packages
}

/// One statically evaluated params/properties/expect field:
/// (name, value, source span).
type StaticPair = (String, DynValue, (usize, usize));

/// Test reference checks deferred to the second pass: resource/gatherer
/// refs plus their statically evaluated params, with everything needed to
/// render diagnostics into the right package.wcl.
struct PendingTests {
    file: PathBuf,
    source: String,
    steps: Vec<PendingStepCheck>,
    gathers: Vec<PendingGatherCheck>,
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
    span: (usize, usize),
}

fn validate_tests(packages: &BTreeMap<String, Package>, p: &PendingTests, diags: &mut Vec<Diag>) {
    let mut ctx = Ctx {
        file: &p.file,
        source: &p.source,
        diags,
    };
    for s in &p.steps {
        let Some(pkg) = packages.get(&s.package) else {
            ctx.err(
                format!("unknown package '{}' in {}", s.package, s.what),
                s.span,
            );
            continue;
        };
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
        check_params_static(
            &mut ctx,
            g.params.as_deref(),
            &decl.params,
            &format!("gatherer '{}.{}' in {}", g.package, g.gatherer, g.what),
            g.span,
        );
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
    for (name, value, vspan) in pairs.unwrap_or_default() {
        present.insert(name.as_str());
        let Some(decl) = lookup_param(&declared, name, what, *vspan, ctx) else {
            continue;
        };
        check_param_type(decl, value, what, *vspan, ctx);
    }
    check_missing_required(decls, what, span, ctx, |n| present.contains(n));
}

fn load_package(
    pkg_dir: &Path,
    wcl_path: &Path,
    diags: &mut Vec<Diag>,
) -> Option<(Package, PendingTests)> {
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
    let mut test_blocks = Vec::new();
    let mut scenarios = Vec::new();
    let mut seen_scenarios = HashSet::new();

    for block in pkg_block.blocks() {
        match block.kind() {
            // Tests parse after resources/gatherers so own-package
            // references resolve regardless of declaration order.
            "test" => test_blocks.push(block),
            "scenario" => {
                let Some(sname) = label_string(&block) else {
                    continue;
                };
                let sdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_field(&block, pkg_dir, &mut ctx) else {
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
                    ctx.err(format!("duplicate scenario '{sname}'"), wcl_span(block.span()));
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
                let Some(script) = script_field(&block, pkg_dir, &mut ctx) else {
                    continue;
                };
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
                let Some(rname) = label_string(&block) else {
                    continue;
                };
                let rdesc = string_field(&block, "description", &mut ctx).unwrap_or_default();
                let Some(script) = script_field(&block, pkg_dir, &mut ctx) else {
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

    let mut tests = Vec::new();
    let mut pending = PendingTests {
        file: wcl_path.to_path_buf(),
        source: source.clone(),
        steps: Vec::new(),
        gathers: Vec::new(),
    };
    let mut seen_tests = HashSet::new();
    for block in &test_blocks {
        if let Some(t) = load_test(block, pkg_dir, &name, &source, &mut ctx, &mut pending) {
            if !seen_tests.insert(t.name.clone()) {
                ctx.err(format!("duplicate test '{}'", t.name), t.span);
            }
            tests.push(t);
        }
    }

    // Tests sharing a group provision one instance from one image on one
    // backend, so every member must agree on both. (Runtime --backend /
    // --image overrides make every test uniform and never trip this.)
    let mut groups: HashMap<&str, (&str, &str)> = HashMap::new();
    for t in &tests {
        let Some(g) = t.group.as_deref() else {
            continue;
        };
        match groups.get(g) {
            None => {
                groups.insert(g, (t.backend.as_str(), t.image.as_str()));
            }
            Some((backend, image)) => {
                if *backend != t.backend || *image != t.image {
                    ctx.err(
                        format!(
                            "test '{}' is in group '{g}' but its backend/image \
                             ({}/{}) differ from another member's ({backend}/{image}); \
                             grouped tests share one instance and must agree",
                            t.name, t.backend, t.image
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
            dir: pkg_dir.to_path_buf(),
            gatherers,
            resources,
            tests,
            scenarios,
        },
        pending,
    ))
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
    pending: &mut PendingTests,
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
    let backend = string_field_optional(block, "backend", ctx).unwrap_or_else(|| "docker".into());
    if backend != "docker" && backend != "vmlab" {
        ctx.err(
            format!("unknown test backend '{backend}' (supported: 'docker', 'vmlab')"),
            span,
        );
    }
    let image = string_field(block, "image", ctx)?;
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
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, v, _)| (k, v))
                    .collect();
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
                        .map(|(k, v, _)| (k, v))
                        .collect(),
                    expect,
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
        backend,
        image,
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
        match f.value() {
            Ok(v) => match wcl_to_dyn(v) {
                Ok(dv) => out.push((f.name().to_string(), dv, fspan)),
                Err(e) => ctx.err(format!("{noun} '{}' of {what}: {e}", f.name()), fspan),
            },
            Err(EvalError::UnresolvedReference { .. }) => {
                ctx.err(
                    format!(
                        "{noun} '{}' of {what} references a variable; tests run against \
                         a variable-free playbook, so values must be static",
                        f.name()
                    ),
                    fspan,
                );
            }
            Err(e) => {
                ctx.diags
                    .push(Diag::from_eval(e.clone(), ctx.file, ctx.source));
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

fn load_params(block: &Block<'_>, ctx: &mut Ctx<'_>) -> Vec<ParamDecl> {
    let mut params = Vec::new();
    let mut seen = HashSet::new();
    for b in block.blocks().filter(|b| b.kind() == "param") {
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
                     float, bool, list or map)"
                ),
                wcl_span(b.span()),
            );
            continue;
        };
        let required = bool_field(&b, "required", ctx).unwrap_or(false);
        let default = b
            .fields()
            .find(|f| f.name() == "default")
            .and_then(|f| match f.value() {
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
                        ctx.err(
                            format!("default for parameter '{name}': {e}"),
                            wcl_span(f.span()),
                        );
                        None
                    }
                },
                Err(e) => {
                    ctx.diags
                        .push(Diag::from_eval(e.clone(), ctx.file, ctx.source));
                    None
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

fn script_field(block: &Block<'_>, pkg_dir: &Path, ctx: &mut Ctx<'_>) -> Option<PathBuf> {
    let rel = string_field(block, "script", ctx)?;
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
