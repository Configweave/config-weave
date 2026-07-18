//! Self-documentation (PRD §12): walk the playbook model, emit wdoc
//! source, and render it by shelling out to the `wcl` CLI's `wdoc build`.
//! config-weave does not embed WCL's renderer — it defers to the installed
//! `wcl` binary. A playbook that does not validate does not document — the
//! caller runs the validation pipeline first.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use crate::diag::Diag;
use crate::model::{CoarseType, ParamDecl, Play, PlayItem, Playbook};

/// The `wcl` binary to drive the wdoc build. Defaults to a PATH lookup;
/// `CONFIG_WEAVE_WCL` pins a specific binary (used by tests/CI).
fn wcl_bin() -> String {
    std::env::var("CONFIG_WEAVE_WCL").unwrap_or_else(|_| "wcl".into())
}

/// Generate the wdoc site for a playbook. Returns the page count.
/// `pkg_only` documents just the packages — the playbook's plays,
/// variables and gathered facts (a package repo's validation harness,
/// not part of its public surface) are skipped.
pub fn generate(pb: &Playbook, outdir: &Path, pkg_only: bool) -> Result<usize, Diag> {
    let source = emit(pb, pkg_only);
    std::fs::create_dir_all(outdir)
        .map_err(|e| Diag::bare(format!("cannot create {}: {e}", outdir.display())))?;
    // Keep the generated source next to the site for inspection — and as
    // the input the `wcl` CLI renders.
    let src_path = outdir.join("_weave_docs.wcl");
    std::fs::write(&src_path, &source)
        .map_err(|e| Diag::bare(format!("cannot write {}: {e}", src_path.display())))?;

    let bin = wcl_bin();
    let output = Command::new(&bin)
        .args(["wdoc", "build"])
        .arg(&src_path)
        .arg("--out")
        .arg(outdir)
        .output()
        .map_err(|e| {
            Diag::bare(format!(
                "cannot run `{bin} wdoc build` (is `wcl` on PATH? set CONFIG_WEAVE_WCL to override): {e}"
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Diag::bare(format!(
            "wdoc build failed:\n{}",
            stderr.trim_end()
        )));
    }

    // The CLI rendered one HTML page per `page` block we emitted; recover
    // the count from the model rather than parsing stdout.
    Ok(page_count(pb, pkg_only))
}

/// Serve a generated site with `wcl wdoc serve` (watch-rebuild dev server
/// with live reload) on the emitted source. Blocks until the server exits.
pub fn serve(outdir: &Path, addr: Option<&str>) -> Result<(), Diag> {
    let src_path = outdir.join("_weave_docs.wcl");
    let bin = wcl_bin();
    let mut cmd = Command::new(&bin);
    cmd.args(["wdoc", "serve"]).arg(&src_path);
    if let Some(addr) = addr {
        cmd.args(["--addr", addr]);
    }
    let status = cmd.status().map_err(|e| {
        Diag::bare(format!(
            "cannot run `{bin} wdoc serve` (is `wcl` on PATH? set CONFIG_WEAVE_WCL to override): {e}"
        ))
    })?;
    if !status.success() {
        return Err(Diag::bare(format!("wdoc serve exited with {status}")));
    }
    Ok(())
}

/// Number of pages `emit` produces: one index, one per play (unless
/// `pkg_only`), one per package, and one per resource and gatherer.
fn page_count(pb: &Playbook, pkg_only: bool) -> usize {
    let per_pkg: usize = pb
        .packages
        .values()
        .map(|p| 1 + p.resources.len() + p.gatherers.len())
        .sum();
    let plays = if pkg_only { 0 } else { pb.plays.len() };
    1 + plays + per_pkg
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

fn gatherer_page(pkg: &str, g: &str) -> String {
    format!("gath_{}_{}", ident(pkg), ident(g))
}

fn md_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn page_link(label: &str, page: &str) -> String {
    format!("[{}]({})", md_text(label), page)
}

fn emit(pb: &Playbook, pkg_only: bool) -> String {
    let mut w = String::new();
    let _ = writeln!(w, "import <wdoc.wcl>");
    let _ = writeln!(w);
    emit_site(&mut w, pb, pkg_only);
    let _ = writeln!(w);
    // The renderer's auto-numbered "§ N" heading markers read like PRD
    // section references; keep the generated docs free of them.
    let _ = writeln!(w, "stylesheet no_section_markers {{");
    let _ = writeln!(w, "  css = \".heading-marker{{display:none}}\"");
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);

    emit_index(&mut w, pb, pkg_only);
    if !pkg_only {
        for play in &pb.plays {
            emit_play(&mut w, pb, play);
        }
    }
    for pkg in pb.packages.values() {
        emit_package(&mut w, pkg);
        for res in pkg.resources.values() {
            emit_resource(&mut w, &pkg.name, res);
        }
        for g in pkg.gatherers.values() {
            emit_gatherer(&mut w, &pkg.name, g);
        }
    }
    w
}

/// The site block: an mdBook-style `:book` site whose sidebar `toc` mirrors
/// the page tree — overview, plays, then each package with its resources and
/// gatherers grouped under their own section headings.
fn emit_site(w: &mut String, pb: &Playbook, pkg_only: bool) {
    let _ = writeln!(w, "site main {{");
    let _ = writeln!(w, "  default_template = :book");
    let _ = writeln!(w, "  title = \"{}\"", esc(&pb.name));
    let _ = writeln!(w, "  theme_toggle = true");
    let _ = writeln!(w, "  search = true");
    let _ = writeln!(w, "  toc {{");
    let _ = writeln!(w, "    chapter \"Overview\" {{ page = index }}");
    if !pkg_only && !pb.plays.is_empty() {
        let _ = writeln!(w, "    chapter \"Plays\" {{");
        for play in &pb.plays {
            let _ = writeln!(
                w,
                "      chapter \"{}\" {{ page = {} }}",
                esc(&play.name),
                play_page(&play.name)
            );
        }
        let _ = writeln!(w, "    }}");
    }
    if !pb.packages.is_empty() {
        let _ = writeln!(w, "    chapter \"Packages\" {{");
        for pkg in pb.packages.values() {
            let _ = writeln!(w, "      chapter \"{}\" {{", esc(&pkg.name));
            let _ = writeln!(w, "        page = {}", package_page(&pkg.name));
            if !pkg.resources.is_empty() {
                let _ = writeln!(w, "        chapter \"Resources\" {{");
                for res in pkg.resources.values() {
                    let _ = writeln!(
                        w,
                        "          chapter \"{}\" {{ page = {} }}",
                        esc(&res.name),
                        resource_page(&pkg.name, &res.name)
                    );
                }
                let _ = writeln!(w, "        }}");
            }
            if !pkg.gatherers.is_empty() {
                let _ = writeln!(w, "        chapter \"Gatherers\" {{");
                for g in pkg.gatherers.values() {
                    let _ = writeln!(
                        w,
                        "          chapter \"{}\" {{ page = {} }}",
                        esc(&g.name),
                        gatherer_page(&pkg.name, &g.name)
                    );
                }
                let _ = writeln!(w, "        }}");
            }
            let _ = writeln!(w, "      }}");
        }
        let _ = writeln!(w, "    }}");
    }
    let _ = writeln!(w, "  }}");
    let _ = writeln!(w, "}}");
}

fn emit_index(w: &mut String, pb: &Playbook, pkg_only: bool) {
    let _ = writeln!(
        w,
        "page index {{ sites = [:main]  title = \"{}\"",
        esc(&pb.name)
    );
    let _ = writeln!(w, "  h1 \"{}\"", esc(&pb.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&pb.description));
    let _ = writeln!(w, "  p \"Version {}\"", esc(&pb.version));

    if !pkg_only && !pb.gathers.is_empty() {
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

    if !pkg_only && !pb.vars.is_empty() {
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

    if !pkg_only {
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
    }

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
                esc(&page_link(&g.name, &gatherer_page(&pkg.name, &g.name))),
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
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

// Resource and gatherer pages carry only their own name in the title and
// heading — the package is evident from the book tree on the left.

fn emit_resource(w: &mut String, pkg: &str, res: &crate::model::ResourceDecl) {
    let _ = writeln!(
        w,
        "page res_{}_{} {{ sites = [:main]  title = \"Resource: {}\"",
        ident(pkg),
        ident(&res.name),
        esc(&res.name)
    );
    let _ = writeln!(w, "  h1 \"Resource: {}\"", esc(&res.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&res.description));
    let _ = writeln!(w, "  p \"Concurrency class: {}\"", res.concurrency.as_str());
    emit_param_table(w, &res.params);

    // A generated `step` example: required params with type placeholders,
    // optional params commented out with their defaults.
    let mut ex = String::new();
    let _ = writeln!(ex, "step \"{}\" {{", esc(&res.name));
    let _ = writeln!(ex, "  description = \"{}\"", esc(&res.description));
    let _ = writeln!(ex, "  resource = \"{}.{}\"", esc(pkg), esc(&res.name));
    if !res.params.is_empty() {
        let _ = writeln!(ex, "  properties {{");
        for line in example_param_lines(&res.params, "    ") {
            let _ = writeln!(ex, "{line}");
        }
        let _ = writeln!(ex, "  }}");
    }
    let _ = writeln!(ex, "}}");
    emit_example(w, &ex);

    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

fn emit_gatherer(w: &mut String, pkg: &str, g: &crate::model::GathererDecl) {
    let _ = writeln!(
        w,
        "page gath_{}_{} {{ sites = [:main]  title = \"Gatherer: {}\"",
        ident(pkg),
        ident(&g.name),
        esc(&g.name)
    );
    let _ = writeln!(w, "  h1 \"Gatherer: {}\"", esc(&g.name));
    let _ = writeln!(w, "  p \"{}\"", esc(&g.description));
    emit_param_table(w, &g.params);

    // A generated `gather` example — the label is the variable the
    // gathered value lands in.
    let mut ex = String::new();
    let _ = writeln!(ex, "gather \"{}\" {{", esc(&g.name));
    let _ = writeln!(ex, "  description = \"{}\"", esc(&g.description));
    let _ = writeln!(ex, "  from = \"{}.{}\"", esc(pkg), esc(&g.name));
    if !g.params.is_empty() {
        let _ = writeln!(ex, "  params {{");
        for line in example_param_lines(&g.params, "    ") {
            let _ = writeln!(ex, "{line}");
        }
        let _ = writeln!(ex, "  }}");
    }
    let _ = writeln!(ex, "}}");
    emit_example(w, &ex);

    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
}

/// An "Example" section holding generated WCL in a raw-heredoc code block —
/// verbatim, so the body needs no wdoc-level escaping.
fn emit_example(w: &mut String, body: &str) {
    let _ = writeln!(w, "  h2 \"Example\"");
    let _ = writeln!(w, "  code wcl {{");
    let _ = writeln!(w, "    source = <<'WEAVE_EX'");
    let _ = write!(w, "{body}");
    let _ = writeln!(w, "WEAVE_EX");
    let _ = writeln!(w, "  }}");
}

/// The property lines of a generated example: required params get a
/// type-based placeholder, optional ones are commented out with their
/// default; each line carries the param's description, aligned.
fn example_param_lines(params: &[ParamDecl], indent: &str) -> Vec<String> {
    let assigns: Vec<String> = params
        .iter()
        .map(|p| {
            if p.required {
                format!("{indent}{} = {}", p.name, placeholder(p.ty))
            } else {
                let default = p
                    .default
                    .as_ref()
                    .map(crate::convert::canonicalise)
                    .unwrap_or_else(|| placeholder(p.ty).to_string());
                format!("{indent}// {} = {}", p.name, default)
            }
        })
        .collect();
    let width = assigns.iter().map(|a| a.chars().count()).max().unwrap_or(0);
    assigns
        .iter()
        .zip(params)
        .map(|(a, p)| {
            if p.description.is_empty() {
                a.clone()
            } else {
                let pad = width - a.chars().count();
                format!("{a}{:pad$}  // {}", "", p.description)
            }
        })
        .collect()
}

/// Placeholder literal for a required example parameter, by declared type.
fn placeholder(ty: CoarseType) -> &'static str {
    match ty {
        CoarseType::String => "\"...\"",
        CoarseType::Int => "0",
        CoarseType::Float => "0.0",
        CoarseType::Bool => "true",
        CoarseType::List => "[]",
        CoarseType::Map => "{}",
    }
}

/// The payoff for mandatory descriptions and declared schemas (PRD §12).
fn emit_param_table(w: &mut String, params: &[ParamDecl]) {
    let _ = writeln!(w, "  h2 \"Parameters\"");
    if params.is_empty() {
        let _ = writeln!(w, "  p \"Takes no parameters.\"");
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
