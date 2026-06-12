//! Self-documentation (PRD §12): walk the playbook model, emit wdoc
//! source, and render it with the wcl_wdoc toolchain. A playbook that
//! does not validate does not document — the caller runs the validation
//! pipeline first.

use std::fmt::Write as _;
use std::path::Path;

use crate::diag::Diag;
use crate::model::{ParamDecl, Play, PlayItem, Playbook};

/// Generate the wdoc site for a playbook. Returns the page count.
pub fn generate(pb: &Playbook, outdir: &Path) -> Result<usize, Diag> {
    let source = emit(pb);
    std::fs::create_dir_all(outdir)
        .map_err(|e| Diag::bare(format!("cannot create {}: {e}", outdir.display())))?;
    // Keep the generated source next to the site for inspection.
    let src_path = outdir.join("_weave_docs.wcl");
    std::fs::write(&src_path, &source)
        .map_err(|e| Diag::bare(format!("cannot write {}: {e}", src_path.display())))?;
    wcl_wdoc::build(&src_path, outdir, None).map_err(|e| Diag::bare(render_build_error(e)))
}

fn render_build_error(e: wcl_wdoc::BuildError) -> String {
    use wcl_wdoc::BuildError as E;
    match e {
        E::Io(err, what) => format!("wdoc build failed: {what}: {err}"),
        E::Parse(report) | E::Eval(report) => format!("wdoc build failed:\n{report:?}"),
        E::Schema(n) => format!("wdoc build failed: {n} schema violation(s) in generated source"),
        E::BadPage(p) => format!("wdoc build failed: bad page '{p}'"),
        other => format!("wdoc build failed: {}", describe_opaque(&other)),
    }
}

fn describe_opaque(e: &wcl_wdoc::BuildError) -> String {
    // Variants without payloads we need; name them via discriminant text.
    use wcl_wdoc::BuildError as E;
    match e {
        E::DuplicateId { page, id } => format!("duplicate id '{id}' on page '{page}'"),
        _ => "unrenderable build error".to_string(),
    }
}

/// Escape a string for a WCL double-quoted literal.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Page ids must be identifiers.
fn ident(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        out.insert(0, 'p');
    }
    out
}

fn package_page(pkg: &str) -> String {
    format!("pkg_{}", ident(pkg))
}

fn play_page(play: &str) -> String {
    format!("play_{}", ident(play))
}

fn resource_page(pkg: &str, res: &str) -> String {
    format!("res_{}_{}", ident(pkg), ident(res))
}

fn test_page(pkg: &str, test: &str) -> String {
    format!("test_{}_{}", ident(pkg), ident(test))
}

fn md_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn page_link(label: &str, page: &str) -> String {
    format!("[{}]({})", md_text(label), page)
}

fn emit(pb: &Playbook) -> String {
    let mut w = String::new();
    let _ = writeln!(w, "import <wdoc.wcl>");
    let _ = writeln!(w);
    let _ = writeln!(w, "site main {{ title = \"{}\" }}", esc(&pb.name));
    let _ = writeln!(w);

    emit_index(&mut w, pb);
    for play in &pb.plays {
        emit_play(&mut w, pb, play);
    }
    for pkg in pb.packages.values() {
        emit_package(&mut w, pkg);
        for res in pkg.resources.values() {
            emit_resource(&mut w, &pkg.name, res);
        }
        for test in &pkg.tests {
            emit_test(&mut w, &pkg.name, test);
        }
    }
    w
}

fn emit_index(w: &mut String, pb: &Playbook) {
    let _ = writeln!(
        w,
        "page index {{ sites = [:main]  title = \"{}\"",
        esc(&pb.name)
    );
    let _ = writeln!(w, "  h1 \"{}\"", esc(&pb.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&pb.description));
    let _ = writeln!(w, "  p \"Version {}\"", esc(&pb.version));

    if !pb.gathers.is_empty() {
        let _ = writeln!(w, "  h2 \"Gathered facts\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(w, "      | \"Variable\" | \"Gatherer\" |");
        for g in &pb.gathers {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}.{}\" |",
                esc(&g.name),
                esc(&g.package),
                esc(&g.gatherer)
            );
        }
        let _ = writeln!(w, "  }}");
    }

    if !pb.vars.is_empty() {
        let _ = writeln!(w, "  h2 \"Variables\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(w, "      | \"Name\" | \"Value\" |");
        for v in &pb.vars {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" |",
                esc(&v.name),
                esc(&v.expr_src)
            );
        }
        let _ = writeln!(w, "  }}");
    }

    let _ = writeln!(w, "  h2 \"Plays\"");
    let _ = writeln!(w, "  table {{\n    rows:");
    let _ = writeln!(w, "      | \"Play\" | \"Steps\" | \"Description\" |");
    for play in &pb.plays {
        let _ = writeln!(
            w,
            "      | \"{}\" | \"{}\" | \"{}\" |",
            esc(&page_link(&play.name, &play_page(&play.name))),
            play.steps().len(),
            esc(&play.description)
        );
    }
    let _ = writeln!(w, "  }}");

    if !pb.packages.is_empty() {
        let _ = writeln!(w, "  h2 \"Packages\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(
            w,
            "      | \"Package\" | \"Resources\" | \"Gatherers\" | \"Description\" |"
        );
        for pkg in pb.packages.values() {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" | \"{}\" | \"{}\" |",
                esc(&page_link(&pkg.name, &package_page(&pkg.name))),
                pkg.resources.len(),
                pkg.gatherers.len(),
                esc(&pkg.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

fn emit_play(w: &mut String, pb: &Playbook, play: &Play) {
    let _ = writeln!(
        w,
        "page play_{} {{ sites = [:main]  title = \"Play: {}\"",
        ident(&play.name),
        esc(&play.name)
    );
    let _ = writeln!(w, "  h1 \"Play: {}\"", esc(&play.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&play.description));
    if !play.parallel {
        let _ = writeln!(w, "  p \"This play runs sequentially (parallel = false).\"");
    }

    // Containers with conditions get called out.
    fn containers<'a>(items: &'a [PlayItem], out: &mut Vec<&'a crate::model::Container>) {
        for item in items {
            if let PlayItem::Container(c) = item {
                out.push(c);
                containers(&c.items, out);
            }
        }
    }
    let mut cs = Vec::new();
    containers(&play.items, &mut cs);
    if !cs.is_empty() {
        let _ = writeln!(w, "  h2 \"Containers\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(
            w,
            "      | \"Container\" | \"Condition\" | \"Description\" |"
        );
        for c in cs {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" | \"{}\" |",
                esc(&c.name),
                esc(c.condition_src.as_deref().unwrap_or("—")),
                esc(&c.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }

    let steps = play.steps();
    let _ = writeln!(w, "  h2 \"Steps\"");
    let _ = writeln!(w, "  table {{\n    rows:");
    let _ = writeln!(
        w,
        "      | \"Step\" | \"Resource\" | \"Condition\" | \"Requires\" | \"Description\" |"
    );
    for s in &steps {
        let path = if s.container_path.is_empty() {
            s.name.clone()
        } else {
            format!("{}/{}", s.container_path.join("/"), s.name)
        };
        let _ = writeln!(
            w,
            "      | \"{}\" | \"{}\" | \"{}\" | \"{}\" |  \"{}\" |",
            esc(&path),
            esc(&page_link(
                &format!("{}.{}", s.package, s.resource),
                &resource_page(&s.package, &s.resource)
            )),
            esc(s.condition_src.as_deref().unwrap_or("—")),
            esc(&if s.requires.is_empty() {
                "—".to_string()
            } else {
                s.requires.join(", ")
            }),
            esc(&s.description)
        );
    }
    let _ = writeln!(w, "  }}");

    // The step DAG as a layered flowchart.
    let _ = writeln!(w, "  h2 \"Step DAG\"");
    let height = 90 * steps.len().max(1) + 40;
    let _ = writeln!(
        w,
        "  diagram {{ width = 720  height = {height}  layout = :layered  layer_gap = 30.0"
    );
    for (i, s) in steps.iter().enumerate() {
        let _ = writeln!(
            w,
            "    process \"{}\" {{ id = s{i}  width = 200.0  height = 40.0 }}",
            esc(&s.name)
        );
    }
    let index_of = |name: &str| steps.iter().position(|s| s.name == name);
    for (i, s) in steps.iter().enumerate() {
        for req in &s.requires {
            if let Some(j) = index_of(req) {
                let _ = writeln!(w, "    s{j} -> s{i} :flow");
            }
        }
    }
    let _ = writeln!(w, "  }}");
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);

    let _ = pb;
}

fn emit_package(w: &mut String, pkg: &crate::model::Package) {
    let _ = writeln!(
        w,
        "page pkg_{} {{ sites = [:main]  title = \"Package: {}\"",
        ident(&pkg.name),
        esc(&pkg.name)
    );
    let _ = writeln!(w, "  h1 \"Package: {}\"", esc(&pkg.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&pkg.description));

    if !pkg.gatherers.is_empty() {
        let _ = writeln!(w, "  h2 \"Gatherers\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(w, "      | \"Gatherer\" | \"Description\" |");
        for g in pkg.gatherers.values() {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" |",
                esc(&g.name),
                esc(&g.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }
    if !pkg.resources.is_empty() {
        let _ = writeln!(w, "  h2 \"Resources\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(
            w,
            "      | \"Resource\" | \"Concurrency\" | \"Description\" |"
        );
        for r in pkg.resources.values() {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" | \"{}\" |",
                esc(&page_link(&r.name, &resource_page(&pkg.name, &r.name))),
                r.concurrency.as_str(),
                esc(&r.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }
    if !pkg.tests.is_empty() {
        let _ = writeln!(w, "  h2 \"Tests\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(
            w,
            "      | \"Test\" | \"Backend\" | \"Image\" | \"Description\" |"
        );
        for t in &pkg.tests {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" | \"{}\" | \"{}\" |",
                esc(&page_link(&t.name, &test_page(&pkg.name, &t.name))),
                esc(&t.backend),
                esc(&t.image),
                esc(&t.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

fn emit_test(w: &mut String, pkg: &str, test: &crate::model::TestDecl) {
    let _ = writeln!(
        w,
        "page test_{}_{} {{ sites = [:main]  title = \"Test: {}:{}\"",
        ident(pkg),
        ident(&test.name),
        esc(pkg),
        esc(&test.name)
    );
    let _ = writeln!(w, "  h1 \"Test: {}:{}\"", esc(pkg), esc(&test.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&test.description));
    let _ = writeln!(
        w,
        "  p \"Runs on {} image {}\"",
        esc(&test.backend),
        esc(&test.image)
    );
    if let Some(setup) = &test.setup {
        let _ = writeln!(w, "  p \"Setup: {}\"", esc(setup));
    }
    if !test.steps.is_empty() {
        let _ = writeln!(w, "  h2 \"Steps\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(
            w,
            "      | \"Step\" | \"Resource\" | \"Expect\" | \"Requires\" | \"Description\" |"
        );
        for s in &test.steps {
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}\" | \"{}\" | \"{}\" | \"{}\" |",
                esc(&s.name),
                esc(&page_link(
                    &format!("{}.{}", s.package, s.resource),
                    &resource_page(&s.package, &s.resource)
                )),
                s.expect.as_str(),
                esc(&if s.requires.is_empty() {
                    "—".to_string()
                } else {
                    s.requires.join(", ")
                }),
                esc(&s.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }
    if !test.gathers.is_empty() {
        let _ = writeln!(w, "  h2 \"Gather checks\"");
        let _ = writeln!(w, "  table {{\n    rows:");
        let _ = writeln!(
            w,
            "      | \"Gather\" | \"Gatherer\" | \"Expectations\" | \"Description\" |"
        );
        for g in &test.gathers {
            let expects = if g.expect.is_empty() {
                "—".to_string()
            } else {
                g.expect
                    .iter()
                    .map(|(k, v)| format!("{k} = {}", crate::convert::canonicalise(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(
                w,
                "      | \"{}\" | \"{}.{}\" | \"{}\" | \"{}\" |",
                esc(&g.name),
                esc(&g.package),
                esc(&g.gatherer),
                esc(&expects),
                esc(&g.description)
            );
        }
        let _ = writeln!(w, "  }}");
    }
    if let Some(verify) = &test.verify {
        let _ = writeln!(
            w,
            "  p \"Custom assertions: {}\"",
            esc(&verify.file_name().unwrap_or_default().to_string_lossy())
        );
    }
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

fn emit_resource(w: &mut String, pkg: &str, res: &crate::model::ResourceDecl) {
    let _ = writeln!(
        w,
        "page res_{}_{} {{ sites = [:main]  title = \"Resource: {}.{}\"",
        ident(pkg),
        ident(&res.name),
        esc(pkg),
        esc(&res.name)
    );
    let _ = writeln!(w, "  h1 \"Resource: {}.{}\"", esc(pkg), esc(&res.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&res.description));
    let _ = writeln!(w, "  p \"Concurrency class: {}\"", res.concurrency.as_str());
    emit_param_table(w, &res.params);
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

/// The payoff for mandatory descriptions and declared schemas (PRD §12).
fn emit_param_table(w: &mut String, params: &[ParamDecl]) {
    let _ = writeln!(w, "  h2 \"Parameters\"");
    if params.is_empty() {
        let _ = writeln!(w, "  p \"This resource takes no parameters.\"");
        return;
    }
    let _ = writeln!(w, "  table {{\n    rows:");
    let _ = writeln!(
        w,
        "      | \"Name\" | \"Type\" | \"Required\" | \"Default\" | \"Description\" |"
    );
    for p in params {
        let default = p
            .default
            .as_ref()
            .map(crate::convert::canonicalise)
            .unwrap_or_else(|| "—".to_string());
        let _ = writeln!(
            w,
            "      | \"{}\" | \"{}\" | \"{}\" | \"{}\" | \"{}\" |",
            esc(&p.name),
            p.ty.as_str(),
            if p.required { "yes" } else { "no" },
            esc(&default),
            esc(&p.description)
        );
    }
    let _ = writeln!(w, "  }}");
}
