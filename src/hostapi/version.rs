//! The `version` module: parse and compare version strings with any number
//! of numeric parts, and match them against node-semver ranges, so every
//! package resource compares versions by the same rules.
//!
//! Hand-written rather than the `semver` crate, which only takes exactly
//! three parts: Windows versions have four (`15.0.4490.9`). Everything
//! above `module()` is pure, so the rules are unit-tested directly. The
//! N-part generalisation of `^`, `~`, x-ranges and hyphen upper bounds is
//! recorded in docs/notes.md.

use std::cmp::Ordering;
use std::collections::HashMap;

use wscript::Module;
use wscript_std::DynValue;

/// A range version with fewer numeric parts than this is a partial: under
/// any operator, and as a hyphen-range upper bound, it names the x-range of
/// its prefix (`<=1.2` is `<1.3`, `1.2.3 - 2.3` is `<2.4`), as in
/// node-semver. Three is semver's full version; longer ones are exact too.
const FULL_PARTS: usize = 3;

/// One dot-separated pre-release identifier (semver §9/§11).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Alpha(String),
}

impl Ord for Ident {
    /// Numeric identifiers compare numerically and sort before
    /// alphanumeric ones; alphanumeric ones compare in ASCII order.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            (Ident::Num(_), Ident::Alpha(_)) => Ordering::Less,
            (Ident::Alpha(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Alpha(a), Ident::Alpha(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A parsed version: numeric parts as written (a missing part counts as 0
/// when comparing), the pre-release identifiers and the build metadata.
#[derive(Debug, Clone)]
pub struct Version {
    parts: Vec<u64>,
    pre: Vec<Ident>,
    pre_text: String,
    build: String,
}

/// A version as a range writes it: numeric parts up to the first wildcard,
/// and whether a wildcard (`x`, `X`, `*`) followed them.
#[derive(Debug, Clone)]
struct Partial {
    parts: Vec<u64>,
    wild: bool,
    pre: Vec<Ident>,
    pre_text: String,
    build: String,
}

impl Partial {
    fn version(&self) -> Version {
        Version {
            parts: self.parts.clone(),
            pre: self.pre.clone(),
            pre_text: self.pre_text.clone(),
            build: self.build.clone(),
        }
    }
}

/// A parse failure: what went wrong and the byte offset it went wrong at.
type PosErr = (String, usize);

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

fn is_wildcard(part: &str) -> bool {
    matches!(part, "x" | "X" | "*")
}

/// Parse a version or a range partial. `s` holds no surrounding
/// whitespace; `base` is its offset in the caller's input, so every error
/// position is an offset into that input.
fn parse_partial(s: &str, base: usize, allow_wild: bool) -> Result<Partial, PosErr> {
    let mut start = 0;
    if s.starts_with(['v', 'V']) {
        start = 1;
    }
    if start == s.len() {
        return Err(("missing version".to_string(), base));
    }
    let body = &s[start..];
    let (body, build) = match body.split_once('+') {
        Some((b, build)) => (b, Some(build)),
        None => (body, None),
    };
    let (core, pre) = match body.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (body, None),
    };

    let mut parts = Vec::new();
    let mut wild = false;
    let mut off = base + start;
    for part in core.split('.') {
        if part.is_empty() {
            return Err(("empty part".to_string(), off));
        }
        if allow_wild && is_wildcard(part) {
            wild = true;
        } else if wild {
            return Err((format!("part \"{part}\" follows a wildcard"), off));
        } else if part.bytes().all(|b| b.is_ascii_digit()) {
            let n = part
                .parse::<u64>()
                .map_err(|_| (format!("part \"{part}\" is out of range"), off))?;
            parts.push(n);
        } else {
            return Err((format!("part \"{part}\" is not a number"), off));
        }
        off += part.len() + 1;
    }

    let pre_off = base + start + core.len() + 1;
    let mut idents = Vec::new();
    if let Some(pre) = pre {
        if wild {
            return Err(("pre-release on a wildcard version".to_string(), pre_off - 1));
        }
        let mut off = pre_off;
        for id in pre.split('.') {
            if id.is_empty() {
                return Err(("empty pre-release identifier".to_string(), off));
            }
            if !id.chars().all(is_ident_char) {
                return Err((format!("invalid pre-release identifier \"{id}\""), off));
            }
            if id.bytes().all(|b| b.is_ascii_digit()) {
                let n = id.parse::<u64>().map_err(|_| {
                    (
                        format!("pre-release identifier \"{id}\" is out of range"),
                        off,
                    )
                })?;
                idents.push(Ident::Num(n));
            } else {
                idents.push(Ident::Alpha(id.to_string()));
            }
            off += id.len() + 1;
        }
    }

    if let Some(build) = build {
        let mut off = base + start + body.len() + 1;
        for id in build.split('.') {
            if id.is_empty() {
                return Err(("empty build identifier".to_string(), off));
            }
            if !id.chars().all(is_ident_char) {
                return Err((format!("invalid build identifier \"{id}\""), off));
            }
            off += id.len() + 1;
        }
    }

    Ok(Partial {
        parts,
        wild,
        pre: idents,
        pre_text: pre.unwrap_or("").to_string(),
        build: build.unwrap_or("").to_string(),
    })
}

/// Compare numeric parts, padding the shorter with zeros.
fn cmp_parts(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| {
            let x = a.get(i).copied().unwrap_or(0);
            let y = b.get(i).copied().unwrap_or(0);
            x.cmp(&y)
        })
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

impl Version {
    pub fn parse(input: &str) -> Result<Version, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(format!("invalid version \"{input}\": empty"));
        }
        let lead = input.len() - input.trim_start().len();
        parse_partial(trimmed, lead, false)
            .map(|p| p.version())
            .map_err(|(msg, pos)| format!("invalid version \"{input}\": {msg} at {pos}"))
    }

    /// Semver precedence over any number of parts: parts as integers with
    /// missing ones 0, a pre-release before its release, identifiers per
    /// semver §11, build metadata ignored.
    pub fn compare(&self, other: &Version) -> Ordering {
        cmp_parts(&self.parts, &other.parts).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Lexicographic over identifiers; a longer set wins when
                // every preceding identifier is equal — `Vec`'s own order.
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }

    /// The `version::parse` map: `{parts, pre, build}`.
    fn to_value(&self, input: &str) -> Result<DynValue, String> {
        let parts = self
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                i64::try_from(*p).map(DynValue::Int).map_err(|_| {
                    format!(
                        "invalid version \"{input}\": part {} exceeds the script int range",
                        i + 1
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DynValue::Map(HashMap::from([
            ("parts".to_string(), DynValue::List(parts)),
            ("pre".to_string(), DynValue::String(self.pre_text.clone())),
            ("build".to_string(), DynValue::String(self.build.clone())),
        ])))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

#[derive(Debug, Clone)]
struct Comparator {
    op: Op,
    version: Version,
}

impl Comparator {
    fn satisfied_by(&self, v: &Version) -> bool {
        let ord = v.compare(&self.version);
        match self.op {
            Op::Lt => ord.is_lt(),
            Op::Le => ord.is_le(),
            Op::Gt => ord.is_gt(),
            Op::Ge => ord.is_ge(),
            Op::Eq => ord.is_eq(),
        }
    }
}

/// One `||` alternative: every comparator must hold. `never` is a set that
/// matches nothing (`<*`, `>*`); an empty set matches every release.
#[derive(Debug, Clone, Default)]
struct ComparatorSet {
    comparators: Vec<Comparator>,
    never: bool,
}

impl ComparatorSet {
    fn push(&mut self, op: Op, version: Version) {
        self.comparators.push(Comparator { op, version });
    }

    fn matches(&self, v: &Version) -> bool {
        if self.never || !self.comparators.iter().all(|c| c.satisfied_by(v)) {
            return false;
        }
        // node-semver: a pre-release matches only when a comparator in the
        // same set names a pre-release on the same numeric parts.
        v.pre.is_empty()
            || self
                .comparators
                .iter()
                .any(|c| !c.version.pre.is_empty() && cmp_parts(&c.version.parts, &v.parts).is_eq())
    }
}

/// A parsed range: a version matches when any alternative matches.
#[derive(Debug, Clone)]
pub struct Range {
    sets: Vec<ComparatorSet>,
}

/// `parts` truncated after `idx`, with `parts[idx]` incremented: the
/// exclusive upper bound of `^`, `~` and x-ranges.
fn bump(parts: &[u64], idx: usize, pos: usize) -> Result<Version, PosErr> {
    let mut out = parts[..=idx].to_vec();
    out[idx] = out[idx]
        .checked_add(1)
        .ok_or_else(|| ("part is out of range for an upper bound".to_string(), pos))?;
    Ok(Version {
        parts: out,
        pre: Vec::new(),
        pre_text: String::new(),
        build: String::new(),
    })
}

const OPERATORS: [&str; 9] = ["", "<", "<=", ">", ">=", "=", "~", "~>", "^"];

fn is_op_char(c: char) -> bool {
    matches!(c, '<' | '>' | '=' | '~' | '^')
}

/// One whitespace-separated term of a set: an operator (possibly empty)
/// and the version text after it, each with its byte offset.
struct Term<'a> {
    op: &'a str,
    op_pos: usize,
    text: &'a str,
    text_pos: usize,
}

/// Split on whitespace, keeping byte offsets.
fn words(s: &str, base: usize) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(st)) => {
                out.push((base + st, &s[st..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(st) = start {
        out.push((base + st, &s[st..]));
    }
    out
}

/// Group words into terms: an operator written apart from its version
/// (`>= 1.2`) joins the word after it.
fn terms(s: &str, base: usize) -> Result<Vec<Term<'_>>, PosErr> {
    let words = words(s, base);
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let (pos, w) = words[i];
        let op_len = w.find(|c: char| !is_op_char(c)).unwrap_or(w.len());
        let op = &w[..op_len];
        if !OPERATORS.contains(&op) {
            return Err(("unknown operator".to_string(), pos));
        }
        if op_len == w.len() {
            let Some(&(next_pos, next)) = words.get(i + 1) else {
                return Err(("missing version after operator".to_string(), pos));
            };
            if next.starts_with(is_op_char) {
                return Err(("unknown operator".to_string(), pos));
            }
            out.push(Term {
                op,
                op_pos: pos,
                text: next,
                text_pos: next_pos,
            });
            i += 2;
        } else {
            out.push(Term {
                op,
                op_pos: pos,
                text: &w[op_len..],
                text_pos: pos + op_len,
            });
            i += 1;
        }
    }
    Ok(out)
}

/// Desugar one operator term into the set.
fn add_term(set: &mut ComparatorSet, term: &Term<'_>) -> Result<(), PosErr> {
    let p = parse_partial(term.text, term.text_pos, true)?;
    let pos = term.op_pos;
    let lower = p.version();
    // A bare `*` / `x`: no numeric part written.
    let Some(last) = p.parts.len().checked_sub(1) else {
        if matches!(term.op, "<" | ">") {
            set.never = true;
        }
        return Ok(());
    };
    // node-semver: a partial (fewer than three parts, or a wildcard) names
    // the x-range of its prefix under every operator. A pre-release names
    // one specific version, so it is never a prefix.
    let prefix = p.wild || (p.parts.len() < FULL_PARTS && p.pre.is_empty());
    match term.op {
        "" | "=" if prefix => {
            set.push(Op::Ge, lower);
            set.push(Op::Lt, bump(&p.parts, last, pos)?);
        }
        "" | "=" => set.push(Op::Eq, lower),
        "^" => {
            let idx = p.parts.iter().position(|n| *n != 0).unwrap_or(last);
            set.push(Op::Ge, lower);
            set.push(Op::Lt, bump(&p.parts, idx, pos)?);
        }
        "~" | "~>" => {
            let idx = if p.parts.len() >= 2 { 1 } else { 0 };
            set.push(Op::Ge, lower);
            set.push(Op::Lt, bump(&p.parts, idx, pos)?);
        }
        ">=" => set.push(Op::Ge, lower),
        ">" if prefix => set.push(Op::Ge, bump(&p.parts, last, pos)?),
        ">" => set.push(Op::Gt, lower),
        "<" => set.push(Op::Lt, lower),
        "<=" if prefix => set.push(Op::Lt, bump(&p.parts, last, pos)?),
        "<=" => set.push(Op::Le, lower),
        _ => return Err(("unknown operator".to_string(), pos)),
    }
    Ok(())
}

/// `a - b`, both inclusive; a partial upper bound covers its whole prefix.
fn add_hyphen(set: &mut ComparatorSet, a: &Term<'_>, b: &Term<'_>) -> Result<(), PosErr> {
    for t in [a, b] {
        if !t.op.is_empty() {
            return Err(("operator in a hyphen range".to_string(), t.op_pos));
        }
    }
    let lo = parse_partial(a.text, a.text_pos, true)?;
    let hi = parse_partial(b.text, b.text_pos, true)?;
    if !lo.parts.is_empty() {
        set.push(Op::Ge, lo.version());
    }
    if let Some(last) = hi.parts.len().checked_sub(1) {
        if hi.wild || (hi.parts.len() < FULL_PARTS && hi.pre.is_empty()) {
            set.push(Op::Lt, bump(&hi.parts, last, b.text_pos)?);
        } else {
            set.push(Op::Le, hi.version());
        }
    }
    Ok(())
}

fn parse_set(s: &str, base: usize) -> Result<ComparatorSet, PosErr> {
    let terms = terms(s, base)?;
    let mut set = ComparatorSet::default();
    let hyphen = |t: &Term<'_>| t.op.is_empty() && t.text == "-";
    if let Some(i) = terms.iter().position(hyphen) {
        let at = terms[i].text_pos;
        if i == 0 {
            return Err(("hyphen range is missing its lower bound".to_string(), at));
        }
        if i + 1 == terms.len() {
            return Err(("hyphen range is missing its upper bound".to_string(), at));
        }
        if terms.len() != 3 {
            let stray = if i == 1 { &terms[3] } else { &terms[0] };
            return Err((
                "unexpected term beside a hyphen range".to_string(),
                stray.op_pos,
            ));
        }
        add_hyphen(&mut set, &terms[0], &terms[2])?;
        return Ok(set);
    }
    for term in &terms {
        add_term(&mut set, term)?;
    }
    Ok(set)
}

impl Range {
    pub fn parse(input: &str) -> Result<Range, String> {
        Self::parse_inner(input)
            .map_err(|(msg, pos)| format!("invalid version range \"{input}\": {msg} at {pos}"))
    }

    fn parse_inner(input: &str) -> Result<Range, PosErr> {
        // Each alternative with its offset, and the offsets of the `||`s.
        let bars: Vec<usize> = input.match_indices("||").map(|(i, _)| i).collect();
        if bars.is_empty() {
            return Ok(Range {
                sets: vec![parse_set(input, 0)?],
            });
        }
        let mut sets = Vec::new();
        let mut start = 0;
        for (n, end) in bars.iter().copied().chain([input.len()]).enumerate() {
            let alt = &input[start..end];
            if alt.trim().is_empty() {
                // The `||` before an empty alternative, or after the first.
                let bar = if n == 0 { bars[0] } else { bars[n - 1] };
                return Err(("dangling ||".to_string(), bar));
            }
            sets.push(parse_set(alt, start)?);
            start = end + 2;
        }
        Ok(Range { sets })
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.sets.iter().any(|s| s.matches(v))
    }
}

pub fn compare(a: &str, b: &str) -> Result<i64, String> {
    let ord = Version::parse(a)?.compare(&Version::parse(b)?);
    Ok(ord as i64)
}

pub fn matches(v: &str, range: &str) -> Result<bool, String> {
    let v = Version::parse(v)?;
    Ok(Range::parse(range)?.matches(&v))
}

/// The newest entry that matches, as written; the first of equal ones.
/// Every entry must parse, matching or not.
pub fn max_matching(vs: &[String], range: &str) -> Result<Option<String>, String> {
    let range = Range::parse(range)?;
    let mut best: Option<(Version, &String)> = None;
    for s in vs {
        let v = Version::parse(s)?;
        if !range.matches(&v) {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| v.compare(b).is_gt()) {
            best = Some((v, s));
        }
    }
    Ok(best.map(|(_, s)| s.clone()))
}

pub fn parse(v: &str) -> Result<DynValue, String> {
    Version::parse(v)?.to_value(v)
}

pub fn module() -> Module {
    let mut m = Module::new("version");
    m.doc("Parse and compare versions with any number of parts; match node-semver ranges");

    m.doc_next("Parse a version into {parts: List[int], pre: string, build: string}");
    m.fn_("parse", |v: &str| -> Result<DynValue, String> { parse(v) });
    m.doc_next("Compare two versions: -1, 0 or 1 (missing parts are 0, build ignored)");
    m.fn_("compare", |a: &str, b: &str| -> Result<i64, String> {
        compare(a, b)
    });
    m.doc_next("Whether a version satisfies a node-semver range");
    m.fn_("matches", |v: &str, range: &str| -> Result<bool, String> {
        matches(v, range)
    });
    m.doc_next("The newest version in the list that satisfies the range, or None");
    m.fn_(
        "max_matching",
        |vs: Vec<String>, range: &str| -> Result<Option<String>, String> {
            max_matching(&vs, range)
        },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(a: &str, b: &str) -> Ordering {
        Version::parse(a)
            .unwrap()
            .compare(&Version::parse(b).unwrap())
    }

    #[test]
    fn compare_table() {
        use Ordering::*;
        let cases = [
            // two parts
            ("14.9", "14.38", Less),
            ("14.38", "14.9", Greater),
            ("1.0", "1.0", Equal),
            // three parts
            ("1.2.3", "1.2.10", Less),
            ("1.10.0", "1.9.9", Greater),
            ("v7.6.7", "7.6.7", Equal),
            ("V7.6.7", "7.6.8", Less),
            // four parts
            ("15.0.4490.9", "15.0.4490.10", Less),
            ("10.0.26100.1742", "10.0.19045.5000", Greater),
            ("16.0.5467.0", "16.0.5467", Equal),
            // missing parts are 0
            ("14.38", "14.38.0.0", Equal),
            ("14.38", "14.38.0.1", Less),
            ("7", "7.0.0", Equal),
            // build metadata ignored
            ("1.2.3+abc", "1.2.3+def", Equal),
            ("1.2.3+abc", "1.2.3", Equal),
            ("1.2.3-rc.1+x", "1.2.3-rc.1", Equal),
            // pre-release before its release
            ("7.6.0-preview.3", "7.6.0", Less),
            ("7.6.0-preview.3", "7.5.9", Greater),
            ("7.6-preview.3", "7.6.0-preview.3", Equal),
            // surrounding whitespace
            (" 1.2 ", "1.2.0", Equal),
        ];
        for (a, b, want) in cases {
            assert_eq!(cmp(a, b), want, "compare({a:?}, {b:?})");
            assert_eq!(cmp(b, a), want.reverse(), "compare({b:?}, {a:?})");
        }
    }

    #[test]
    fn compare_returns_minus_one_zero_one() {
        assert_eq!(compare("1", "2").unwrap(), -1);
        assert_eq!(compare("2", "2.0").unwrap(), 0);
        assert_eq!(compare("2.1", "2").unwrap(), 1);
    }

    #[test]
    fn semver_precedence_chain() {
        // semver §11's example, plus a build-tagged release at the end.
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for (i, a) in chain.iter().enumerate() {
            for (j, b) in chain.iter().enumerate() {
                assert_eq!(cmp(a, b), i.cmp(&j), "compare({a:?}, {b:?})");
            }
        }
    }

    #[test]
    fn range_table() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            // (range, matches, does not match)
            // exact: three or more parts, under any operator
            (
                "1.2.3",
                &["1.2.3", "v1.2.3", "1.2.3.0", "1.2.3+b"],
                &["1.2.4", "1.2.3.1"],
            ),
            (
                "15.0.4490.9",
                &["15.0.4490.9"],
                &["15.0.4490.10", "15.0.4490"],
            ),
            ("=v1.2.3", &["1.2.3", "1.2.3.0"], &["1.2.4"]),
            ("=15.0.4490.9", &["15.0.4490.9"], &["15.0.4490.9.1"]),
            (">1.2.3", &["1.2.3.1", "1.2.4"], &["1.2.3", "1.2.3.0"]),
            ("<=1.2.3", &["1.2.3", "1.2.3.0"], &["1.2.3.1", "1.2.4"]),
            (">15.0.4490.9", &["15.0.4490.10"], &["15.0.4490.9"]),
            // partials (one or two parts) name their x-range under any operator
            (
                "=1.2",
                &["1.2", "1.2.0.0", "1.2.1", "1.2.99"],
                &["1.3", "1.1.9"],
            ),
            ("1.2", &["1.2", "1.2.1"], &["1.3", "1.1.9"]),
            (">1.2", &["1.3", "2"], &["1.2", "1.2.1", "1.2.99.9", "1.1"]),
            (
                ">1.2.x",
                &["1.3", "2"],
                &["1.2", "1.2.1", "1.2.99.9", "1.1"],
            ),
            (">=1.2", &["1.2", "1.2.0.0", "1.3"], &["1.1.9"]),
            ("<1.2", &["1.1.9", "0.1"], &["1.2", "1.2.0.1"]),
            ("<=1.2", &["1.2", "1.2.1", "1.2.99"], &["1.3", "1.3.0.0"]),
            ("<=1.2.x", &["1.2.99"], &["1.3"]),
            ("<=1.4", &["1.4.1", "1.4.7"], &["1.5"]),
            (">1", &["2", "2.0.1"], &["1", "1.9.9"]),
            ("<=1", &["1", "1.9.9"], &["2"]),
            ("=1", &["1.0", "1.9"], &["2", "0.9"]),
            // comparisons
            (
                ">=14.38",
                &["14.38", "14.38.33130.0", "15"],
                &["14.29.30133", "14.9"],
            ),
            ("<2", &["1.99.99", "0.1"], &["2", "2.0.1"]),
            ("<=1.4.7", &["1.4.7", "1.4.6.9"], &["1.4.7.1", "1.5"]),
            (">= 1.2 < 2", &["1.2", "1.9"], &["2", "1.1"]),
            // AND
            (">=14.38 <15", &["14.38", "14.40.1"], &["15.0", "14.37"]),
            // caret
            ("^7.5.0", &["7.5.0", "7.9.9"], &["8.0.0", "7.4.9"]),
            ("^0.2.3", &["0.2.3", "0.2.9"], &["0.3.0", "0.2.2"]),
            ("^0.0.3", &["0.0.3"], &["0.0.4", "0.0.2"]),
            (
                "^15.0.4490.9",
                &["15.0.4490.9", "15.9"],
                &["16", "15.0.4490.8"],
            ),
            ("^0.0", &["0.0.0", "0.0.9"], &["0.1"]),
            ("^0", &["0.0.0", "0.9.9"], &["1"]),
            ("^0.0.0.5", &["0.0.0.5"], &["0.0.0.6"]),
            ("^1.x", &["1.0", "1.9"], &["2", "0.9"]),
            // tilde
            ("~20.2.1", &["20.2.1", "20.2.99"], &["20.3.0", "20.2.0"]),
            (
                "~15.0.4490.9",
                &["15.0.4490.9", "15.0.9999"],
                &["15.1", "15.0.4490.8"],
            ),
            ("~1", &["1.0", "1.9.9"], &["2"]),
            ("~0.2", &["0.2.0", "0.2.9"], &["0.3"]),
            ("~>1.2", &["1.2.5"], &["1.3"]),
            // x-ranges and bare partials
            ("7", &["7", "7.0.1", "7.99"], &["8", "6.9"]),
            ("7.x", &["7.1"], &["8.0"]),
            ("7.*", &["7.1"], &["8.0"]),
            ("7.X", &["7.1"], &["8.0"]),
            ("20.2.x", &["20.2.0", "20.2.9.9"], &["20.3", "20.1.9"]),
            ("14.38", &["14.38", "14.38.33130"], &["14.39", "14.37"]),
            ("=1.x", &["1.5"], &["2"]),
            // operators on wildcards
            (">=1.x", &["1.0", "2"], &["0.9"]),
            (">1.x", &["2"], &["1.9"]),
            ("<1.x", &["0.9"], &["1.0"]),
            ("<=1.x", &["1.9"], &["2"]),
            (">=*", &["0.0.0", "99"], &[]),
            ("<*", &[], &["0.0.0", "1"]),
            (">*", &[], &["1"]),
            // OR
            ("^6 || ^7", &["6.1", "7.0"], &["5.9", "8.0"]),
            ("1.2.3||2.x", &["1.2.3", "2.5"], &["1.2.4", "3"]),
            (
                "<1 || >=3 <4 || 9",
                &["0.5", "3.5", "9.1"],
                &["1", "4", "10"],
            ),
            // hyphen ranges, inclusive; a partial upper bound is its prefix
            ("1.2.3 - 2.3.4", &["1.2.3", "2.3.4"], &["1.2.2", "2.3.5"]),
            ("1.2.3 - 2.3", &["2.3.9"], &["2.4.0"]),
            ("1.2 - 2", &["1.2.0", "2.9"], &["1.1.9", "3"]),
            ("1.2.3 - 2.3.x", &["2.3.9"], &["2.4"]),
            ("1.0.0.1 - 1.0.0.5", &["1.0.0.5"], &["1.0.0.6", "1.0.0.0"]),
            ("* - 2", &["0.0.1", "2.5"], &["3"]),
            ("1 - *", &["1", "99"], &["0.9"]),
            // any
            ("*", &["0.0.0", "15.0.4490.9"], &[]),
            ("", &["1.2.3"], &[]),
            ("   ", &["1.2.3"], &[]),
            ("x", &["1"], &[]),
        ];
        for (range, yes, no) in cases {
            for v in *yes {
                assert!(matches(v, range).unwrap(), "{v:?} should match {range:?}");
            }
            for v in *no {
                assert!(
                    !matches(v, range).unwrap(),
                    "{v:?} should not match {range:?}"
                );
            }
        }
    }

    #[test]
    fn prerelease_table() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            // A range without a pre-release never matches one.
            ("*", &[], &["1.0.0-beta"]),
            ("", &[], &["1.0.0-beta"]),
            (">=1.0.0", &[], &["1.0.1-beta", "2.0.0-rc.1"]),
            ("^7", &["7.1"], &["7.6.0-preview.3"]),
            // Named on the same numeric parts: that tuple's pre-releases match.
            (
                ">=7.6.0-preview.1",
                &["7.6.0-preview.3", "7.6.0", "7.7"],
                &["7.6.0-alpha", "7.7.0-preview.1"],
            ),
            (
                "^1.2.3-beta.2",
                &["1.2.3-beta.2", "1.2.3-beta.11", "1.2.3", "1.9"],
                &["1.2.3-beta.1", "1.2.4-beta", "2.0.0-alpha"],
            ),
            (
                "~1.2.3-beta.2",
                &["1.2.3-beta.4", "1.2.9"],
                &["1.2.4-alpha"],
            ),
            (
                "1.2.3-rc.1",
                &["1.2.3-rc.1", "1.2.3-rc.1+b"],
                &["1.2.3-rc.2", "1.2.3"],
            ),
            // Padding: the named tuple 7.6 is 7.6.0.
            (
                ">=7.6-preview",
                &["7.6.0-preview.3", "7.6.0.0-rc"],
                &["7.6.1-rc"],
            ),
            // The pre-release must be in the same || alternative.
            ("<1 || >=2.0.0-rc.1", &["2.0.0-rc.2"], &["0.9.0-beta"]),
            // A hyphen bound names a pre-release too.
            (
                "1.0.0-alpha - 1.0.0",
                &["1.0.0-beta", "1.0.0"],
                &["0.9.0-beta"],
            ),
        ];
        for (range, yes, no) in cases {
            for v in *yes {
                assert!(matches(v, range).unwrap(), "{v:?} should match {range:?}");
            }
            for v in *no {
                assert!(
                    !matches(v, range).unwrap(),
                    "{v:?} should not match {range:?}"
                );
            }
        }
    }

    #[test]
    fn version_error_table() {
        let cases = [
            ("", "invalid version \"\": empty"),
            ("   ", "invalid version \"   \": empty"),
            ("v", "invalid version \"v\": missing version at 0"),
            (
                "1:2.3-4",
                "invalid version \"1:2.3-4\": part \"1:2\" is not a number at 0",
            ),
            (
                "1.2~rc1",
                "invalid version \"1.2~rc1\": part \"2~rc1\" is not a number at 2",
            ),
            (
                "1.2a",
                "invalid version \"1.2a\": part \"2a\" is not a number at 2",
            ),
            (
                "1.x",
                "invalid version \"1.x\": part \"x\" is not a number at 2",
            ),
            ("1..2", "invalid version \"1..2\": empty part at 2"),
            ("1.2.", "invalid version \"1.2.\": empty part at 4"),
            (".1", "invalid version \".1\": empty part at 0"),
            (
                "1.18446744073709551616",
                "invalid version \"1.18446744073709551616\": part \"18446744073709551616\" is out of range at 2",
            ),
            (
                "1.0-",
                "invalid version \"1.0-\": empty pre-release identifier at 4",
            ),
            (
                "1.0-a..b",
                "invalid version \"1.0-a..b\": empty pre-release identifier at 6",
            ),
            (
                "1.0-a_b",
                "invalid version \"1.0-a_b\": invalid pre-release identifier \"a_b\" at 4",
            ),
            (
                "1.0+",
                "invalid version \"1.0+\": empty build identifier at 4",
            ),
            (
                "1.0+a!",
                "invalid version \"1.0+a!\": invalid build identifier \"a!\" at 4",
            ),
            (
                "1.2 3",
                "invalid version \"1.2 3\": part \"2 3\" is not a number at 2",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(Version::parse(input).unwrap_err(), want, "parse({input:?})");
        }
        // compare names whichever side is bad
        assert_eq!(
            compare("1.2", "1..2").unwrap_err(),
            "invalid version \"1..2\": empty part at 2"
        );
    }

    #[test]
    fn range_error_table() {
        let cases = [
            (">== 1.2", "unknown operator at 0"),
            (">=1 =<2", "unknown operator at 4"),
            ("1.2 !1", "part \"!1\" is not a number at 4"),
            (">= >= 1", "unknown operator at 0"),
            (">=", "missing version after operator at 0"),
            ("1 <", "missing version after operator at 2"),
            ("^v", "missing version at 1"),
            ("1..2", "empty part at 2"),
            (">=1.2.", "empty part at 6"),
            ("1.2a", "part \"2a\" is not a number at 2"),
            ("1.x.3", "part \"3\" follows a wildcard at 4"),
            ("1.x-beta", "pre-release on a wildcard version at 3"),
            ("1.2 ||", "dangling || at 4"),
            ("|| 1.2", "dangling || at 0"),
            ("1 || || 2", "dangling || at 2"),
            ("||", "dangling || at 0"),
            ("- 2", "hyphen range is missing its lower bound at 0"),
            ("1 -", "hyphen range is missing its upper bound at 2"),
            ("1 - 2 3", "unexpected term beside a hyphen range at 6"),
            ("0 1 - 2", "unexpected term beside a hyphen range at 0"),
            (">=1 - 2", "operator in a hyphen range at 0"),
            ("1 - ^2", "operator in a hyphen range at 4"),
            (
                "^18446744073709551615",
                "part is out of range for an upper bound at 0",
            ),
        ];
        for (range, want) in cases {
            let want = format!("invalid version range \"{range}\": {want}");
            assert_eq!(
                Range::parse(range).unwrap_err(),
                want,
                "Range::parse({range:?})"
            );
        }
        // matches reports a bad version before looking at the range
        assert_eq!(
            matches("1.2a", ">=1").unwrap_err(),
            "invalid version \"1.2a\": part \"2a\" is not a number at 2"
        );
        assert!(
            matches("1.2", ">== 1.2")
                .unwrap_err()
                .ends_with("unknown operator at 0")
        );
    }

    #[test]
    fn parse_shape() {
        let v = parse("v15.0.4490.9-rc.1+build.7").unwrap();
        let DynValue::Map(m) = v else {
            panic!("not a map: {v:?}")
        };
        assert_eq!(
            m["parts"],
            DynValue::List([15, 0, 4490, 9].map(DynValue::Int).to_vec())
        );
        assert_eq!(m["pre"], DynValue::String("rc.1".into()));
        assert_eq!(m["build"], DynValue::String("build.7".into()));

        let DynValue::Map(m) = parse("14.38").unwrap() else {
            panic!()
        };
        assert_eq!(
            m["parts"],
            DynValue::List([14, 38].map(DynValue::Int).to_vec())
        );
        assert_eq!(m["pre"], DynValue::String(String::new()));
        assert_eq!(m["build"], DynValue::String(String::new()));
    }

    #[test]
    fn parse_part_beyond_script_int_errors() {
        assert_eq!(
            parse("1.9223372036854775808").unwrap_err(),
            "invalid version \"1.9223372036854775808\": part 2 exceeds the script int range"
        );
        // compare still handles it: parts are u64.
        assert_eq!(compare("1.9223372036854775808", "1.1").unwrap(), 1);
    }

    fn list(vs: &[&str]) -> Vec<String> {
        vs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn max_matching_table() {
        let cases: &[(&[&str], &str, Option<&str>)] = &[
            (
                &["20.1.4", "20.2.1", "20.2.10", "20.3.0"],
                "~20.2",
                Some("20.2.10"),
            ),
            (&["7.4.6", "7.6.0-preview.3", "7.5.2"], "^7", Some("7.5.2")),
            (
                &["7.6.0-preview.3", "7.6.0-preview.4"],
                ">=7.6.0-preview.1",
                Some("7.6.0-preview.4"),
            ),
            (
                &["14.29.30133", "14.38.33130.0", "14.40.33810.0"],
                ">=14.38 <15",
                Some("14.40.33810.0"),
            ),
            (&["1.0", "2.0"], ">=3", None),
            (&[], "*", None),
            // equal versions: the first as written
            (&["1.2", "1.2.0", "v1.2.0.0"], "1.2", Some("1.2")),
        ];
        for (vs, range, want) in cases {
            assert_eq!(
                max_matching(&list(vs), range).unwrap().as_deref(),
                *want,
                "max_matching({vs:?}, {range:?})"
            );
        }
    }

    #[test]
    fn max_matching_errors_on_a_bad_entry() {
        // Even a non-matching bad entry is an error, not skipped.
        assert_eq!(
            max_matching(&list(&["1.0", "1:2.3-4", "2.0"]), ">=5").unwrap_err(),
            "invalid version \"1:2.3-4\": part \"1:2\" is not a number at 0"
        );
        assert_eq!(
            max_matching(&list(&["1.0"]), ">== 1").unwrap_err(),
            "invalid version range \">== 1\": unknown operator at 0"
        );
    }

    /// The module through the script engine, against the real host context.
    #[test]
    fn script_smoke() {
        let ctx = crate::hostapi::context();
        let unit = ctx
            .compile(
                r#"
use version

fn run(x: int) -> Result[string, string] {
    let c = version::compare("14.9", "14.38")?
    let m = version::matches("15.0.4490.9", "^15")?
    let best = version::max_matching(["7.4.6", "7.5.2", "8.0.0"], "^7")?
    let none = version::max_matching(["1.0"], ">=2")?
    let p = version::parse("v1.2.3-rc.1")?
    let pre = p.get("pre").unwrap().as_string().unwrap()
    let picked = match best { Some(s) => s, None => "none" }
    let missing = match none { Some(s) => s, None => "none" }
    let bad = match version::matches("1.2", ">== 1") { Ok(_) => "ok", Err(e) => e }
    Ok("{c} {m} {picked} {missing} {pre} | {bad}")
}
"#,
            )
            .unwrap_or_else(|e| panic!("compile: {e}"));
        let mut vm = crate::hostapi::vm(&ctx);
        let out: Result<String, String> = vm.call_unit(&unit, "run", (0i64,)).unwrap();
        assert_eq!(
            out.unwrap(),
            "-1 true 7.5.2 none rc.1 | invalid version range \">== 1\": unknown operator at 0"
        );
    }
}
