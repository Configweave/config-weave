//! The playbook data model: what `playbook.wcl` and the `package.wcl`
//! files describe, in plain owned Rust. Loaded once; expression fields
//! (conditions, properties, gather params, var values) stay *deferred* —
//! they are evaluated at run time against a freshly opened document with
//! the generated variables import bound.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wscript_std::DynValue;

#[derive(Debug)]
pub struct Playbook {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Playbook directory (contains `playbook.wcl`, `pkgs/`, `lib/`).
    pub root: PathBuf,
    /// Raw `playbook.wcl` source.
    pub source: String,
    pub gathers: Vec<GatherInvocation>,
    /// Declared playbook variables, in declaration order. The expression
    /// text is spliced into the generated vars import verbatim.
    pub vars: Vec<VarDecl>,
    /// Playbook-local composites, referenced from a step by bare name.
    pub composites: BTreeMap<String, CompositeDecl>,
    pub plays: Vec<Play>,
    pub packages: BTreeMap<String, Package>,
}

impl Playbook {
    pub fn play(&self, name: &str) -> Option<&Play> {
        self.plays.iter().find(|p| p.name == name)
    }

    pub fn resource(&self, package: &str, name: &str) -> Option<&ResourceDecl> {
        self.packages.get(package)?.resources.get(name)
    }

    /// A composite by the same addressing the model uses everywhere: an
    /// empty package means the playbook-local namespace.
    pub fn composite(&self, package: &str, name: &str) -> Option<&CompositeDecl> {
        if package.is_empty() {
            self.composites.get(name)
        } else {
            self.packages.get(package)?.composites.get(name)
        }
    }

    /// Source text of the document a composite was declared in — the
    /// package's `package.wcl`, or the playbook itself. The planner reopens
    /// it with the invocation's arguments bound.
    pub fn composite_source(&self, package: &str) -> Option<(&str, &Path)> {
        if package.is_empty() {
            Some((&self.source, &self.root))
        } else {
            let pkg = self.packages.get(package)?;
            Some((&pkg.source, &pkg.dir))
        }
    }
}

#[derive(Debug, Clone)]
pub struct GatherInvocation {
    /// Variable the result lands in (the block label).
    pub name: String,
    pub package: String,
    pub gatherer: String,
}

#[derive(Debug, Clone)]
pub struct VarDecl {
    pub name: String,
    /// Raw expression source text, exactly as written in the `vars` block.
    pub expr_src: String,
}

#[derive(Debug)]
pub struct Play {
    pub name: String,
    pub description: String,
    pub parallel: bool,
    /// Steps and containers in declaration order.
    pub items: Vec<PlayItem>,
}

impl Play {
    /// All steps in declaration order, flattened through containers.
    pub fn steps(&self) -> Vec<&Step> {
        fn walk<'a>(items: &'a [PlayItem], out: &mut Vec<&'a Step>) {
            for item in items {
                match item {
                    PlayItem::Step(s) => out.push(s),
                    PlayItem::Container(c) => walk(&c.items, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }
}

#[derive(Debug)]
pub enum PlayItem {
    Step(Step),
    Container(Container),
}

#[derive(Debug)]
pub struct Container {
    pub name: String,
    pub description: String,
    /// Raw condition expression text, for documentation.
    pub condition_src: Option<String>,
    pub items: Vec<PlayItem>,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub name: String,
    pub description: String,
    pub package: String,
    pub resource: String,
    pub requires: Vec<String>,
    /// Step-level concurrency tightening, if declared.
    pub concurrency: Option<Concurrency>,
    /// Names of enclosing containers, outermost first. Used to locate the
    /// step's block at run time and to inherit container conditions. A step
    /// expanded from a composite extends this with one segment per
    /// enclosing invocation, so its report path reads
    /// `container/…/invocation/inner`.
    pub container_path: Vec<String>,
    /// Empty for a step declared directly in a playbook. Otherwise the
    /// chain of composite invocations that produced it, outermost first —
    /// the planner walks it to evaluate each invocation's arguments in the
    /// document that declared it.
    ///
    /// The last `frames.len()` segments of `container_path` are the
    /// invocation names, so every scope the resolver needs derives from
    /// these two fields together.
    pub frames: Vec<CompositeFrame>,
    /// Raw condition expression text, for documentation.
    pub condition_src: Option<String>,
    pub span: (usize, usize),
}

impl Step {
    /// The enclosing *playbook* containers, with composite invocation
    /// segments stripped. Only these carry inheritable conditions.
    pub fn playbook_path(&self) -> &[String] {
        &self.container_path[..self.container_path.len() - self.frames.len()]
    }
}

/// One composite invocation in a step's provenance chain.
#[derive(Debug, Clone)]
pub struct CompositeFrame {
    /// The invoking step's name — also the `container_path` segment it
    /// contributes, and the block label the planner looks up.
    pub step: String,
    /// Package declaring the composite; empty for a playbook-local one.
    pub package: String,
    pub composite: String,
    /// The invocation's own `requires`, resolved in the *caller's* scope
    /// rather than the expanded step's.
    pub requires: Vec<String>,
}

/// A reusable, parameterised block of steps invoked like a resource.
#[derive(Debug)]
pub struct CompositeDecl {
    pub name: String,
    pub description: String,
    pub params: Vec<ParamDecl>,
    /// Inner steps in declaration order. These are templates: their
    /// `container_path` and `frames` are empty until expansion clones them
    /// into a play.
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Concurrency {
    Parallel,
    Exclusive,
    Global,
}

impl Concurrency {
    pub fn parse(s: &str) -> Option<Concurrency> {
        match s {
            "parallel" => Some(Concurrency::Parallel),
            "exclusive" => Some(Concurrency::Exclusive),
            "global" => Some(Concurrency::Global),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Concurrency::Parallel => "parallel",
            Concurrency::Exclusive => "exclusive",
            Concurrency::Global => "global",
        }
    }
}

#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub description: String,
    /// Package directory; script paths are relative to it.
    pub dir: PathBuf,
    /// Raw `package.wcl` source. Kept so the planner can reopen the
    /// document with a composite invocation's arguments bound.
    pub source: String,
    pub gatherers: BTreeMap<String, GathererDecl>,
    pub resources: BTreeMap<String, ResourceDecl>,
    /// Composites, sharing one namespace with `resources`.
    pub composites: BTreeMap<String, CompositeDecl>,
    /// Convergence tests, in declaration order.
    pub tests: Vec<TestDecl>,
    /// Wscript-scripted scenarios, in declaration order.
    pub scenarios: Vec<ScenarioDecl>,
}

#[derive(Debug)]
pub struct ResourceDecl {
    pub name: String,
    pub description: String,
    /// Absolute path to the resource script.
    pub script: PathBuf,
    pub concurrency: Concurrency,
    pub params: Vec<ParamDecl>,
}

#[derive(Debug)]
pub struct GathererDecl {
    pub name: String,
    pub description: String,
    pub script: PathBuf,
    pub params: Vec<ParamDecl>,
    /// Documented keys of the gathered value. Mostly docs metadata — the
    /// engine does not check that a gathered map has these keys, or that
    /// only these — but a key declared `symbol` *is* typed: its value
    /// binds as a WCL symbol and is checked against any declared set.
    pub returns: Vec<ReturnDecl>,
}

/// One documented key of a gatherer's returned value.
#[derive(Debug, Clone)]
pub struct ReturnDecl {
    pub name: String,
    pub description: String,
    pub ty: CoarseType,
    /// The legal symbols for a `symbol`-typed key, in declaration order.
    /// Empty means unconstrained, matching the parameter rule.
    pub symbols: Vec<SymbolDecl>,
}

impl ReturnDecl {
    pub fn symbol_violation(&self, v: &DynValue) -> Option<String> {
        symbol_violation(&self.symbols, v)
    }
}

/// One declared legal value of a symbol-typed parameter or returns key.
#[derive(Debug, Clone)]
pub struct SymbolDecl {
    pub name: String,
    pub description: String,
}

/// A declared symbol set rendered as `:symbol` literals, comma-joined —
/// the tail of every "expected one of" diagnostic.
pub fn symbol_list(symbols: &[SymbolDecl]) -> String {
    symbols
        .iter()
        .map(|s| format!(":{}", s.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `None` when `v` is an allowed symbol, or when the set is empty (an
/// unconstrained symbol accepts any token, as it always has); otherwise the
/// diagnostic body naming what was got and what was expected.
pub fn symbol_violation(symbols: &[SymbolDecl], v: &DynValue) -> Option<String> {
    if symbols.is_empty() {
        return None;
    }
    let DynValue::String(s) = v else {
        return None; // the coarse type check already rejected this
    };
    if symbols.iter().any(|d| &d.name == s) {
        return None;
    }
    Some(format!(
        "got :{s}, expected one of: {}",
        symbol_list(symbols)
    ))
}

#[derive(Debug, Clone)]
pub struct ParamDecl {
    pub name: String,
    pub description: String,
    pub ty: CoarseType,
    pub required: bool,
    pub default: Option<DynValue>,
    /// The legal symbols, in declaration order. Empty means unconstrained —
    /// a symbol param without `symbol` blocks accepts any token, as it
    /// always has.
    pub symbols: Vec<SymbolDecl>,
}

impl ParamDecl {
    pub fn symbol_violation(&self, v: &DynValue) -> Option<String> {
        symbol_violation(&self.symbols, v)
    }
}

/// What a test is provisioned into. Both kinds are vmlab machines; the
/// distinction is what vmlab clones them from, which in turn decides
/// cost, guest OS range and how much of a real system is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestTarget {
    /// An OCI image run as a vmlab container (a micro-VM around the image).
    /// Linux only, starts in seconds — the default for most tests.
    Container(String),
    /// A vmlab template cloned into a full VM. Linux or windows, and the
    /// only kind with a real init system, its own kernel, and reboots.
    Vm(String),
}

impl TestTarget {
    /// The image or template reference, whichever this is.
    pub fn reference(&self) -> &str {
        match self {
            TestTarget::Container(r) | TestTarget::Vm(r) => r,
        }
    }
}

impl std::fmt::Display for TestTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestTarget::Container(r) => write!(f, "container {r}"),
            TestTarget::Vm(r) => write!(f, "vm {r}"),
        }
    }
}

/// An isolated convergence test declared in `package.wcl`, executed by
/// `config-weave test` inside a disposable vmlab instance.
#[derive(Debug)]
pub struct TestDecl {
    pub name: String,
    pub description: String,
    /// The container image or VM template this test provisions.
    pub target: TestTarget,
    /// Guest RAM override for the instance ("4GiB"); `None` = the vmlab
    /// default. Grouped tests must agree.
    pub memory: Option<String>,
    /// Tests sharing a non-empty group (within a package) run in one
    /// shared instance; `None`/empty means the test gets its own.
    pub group: Option<String>,
    /// Optional shell provisioning, run via `sh -c` before anything else.
    pub setup: Option<String>,
    /// Absolute path to the optional wscript verify script.
    pub verify: Option<PathBuf>,
    pub steps: Vec<TestStep>,
    pub gathers: Vec<TestGather>,
    pub span: (usize, usize),
}

/// A wscript-scripted, multi-stage test declared in `package.wcl`, executed
/// by `config-weave test` over a declared vmlab lab. The driver script
/// brings VMs up by name, applies config-weave, reboots, and asserts —
/// see `hostapi::testlab`.
#[derive(Debug)]
pub struct ScenarioDecl {
    pub name: String,
    pub description: String,
    /// Absolute path to the lab directory holding a `vmlab.wcl`.
    pub lab: PathBuf,
    /// Absolute path to the driver wscript script (`fn run(lab) -> bool`).
    pub script: PathBuf,
}

/// A resource invocation under test; mirrors a playbook step. The
/// properties/condition source survives verbatim so it can be spliced
/// into the synthesized playbook.
#[derive(Debug)]
pub struct TestStep {
    pub name: String,
    pub description: String,
    pub package: String,
    pub resource: String,
    pub expect: Expect,
    pub requires: Vec<String>,
    /// Raw condition expression text, spliced into synthesis.
    pub condition_src: Option<String>,
    /// Raw `properties { … }` block text, spliced into synthesis.
    pub properties_src: Option<String>,
    pub span: (usize, usize),
}

/// What a test step asserts across the three engine runs
/// (check, apply, apply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// not_configured → configured → already_configured (the default).
    Converge,
    AlreadyConfigured,
    Error,
    Skip,
    RebootRequired,
}

impl Expect {
    pub fn parse(s: &str) -> Option<Expect> {
        match s {
            "converge" => Some(Expect::Converge),
            "already_configured" => Some(Expect::AlreadyConfigured),
            "error" => Some(Expect::Error),
            "skip" => Some(Expect::Skip),
            "reboot_required" => Some(Expect::RebootRequired),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Expect::Converge => "converge",
            Expect::AlreadyConfigured => "already_configured",
            Expect::Error => "error",
            Expect::Skip => "skip",
            Expect::RebootRequired => "reboot_required",
        }
    }
}

/// A gatherer invocation under test. Params and expectations must
/// evaluate statically (tests run against a variable-free playbook).
#[derive(Debug)]
pub struct TestGather {
    pub name: String,
    /// Mandatory in the vocab like every description; nothing renders it
    /// since tests left the generated docs.
    #[allow(dead_code)]
    pub description: String,
    pub package: String,
    pub gatherer: String,
    pub params: Vec<(String, DynValue)>,
    /// Top-level key equality assertions over the gathered value.
    pub expect: Vec<(String, DynValue)>,
}

/// The coarse parameter types the schema system distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoarseType {
    String,
    Int,
    Float,
    Bool,
    List,
    Map,
    /// An enumerated token (e.g. `ensure = :present`). WCL symbols and
    /// strings both convert to `DynValue::String` before scripts see
    /// them, so this validates exactly like `String` — declaring it
    /// documents the symbol spelling, and docs render the values as
    /// `:symbol` literals.
    Symbol,
    /// A time span written as a WCL unit literal (`30min`, `24h`, `90s`)
    /// and resolved against WCL's `std.Duration`, whose base unit is the
    /// nanosecond — so scripts receive a plain `Int` of nanoseconds and
    /// divide to whatever resolution they work in.
    Duration,
}

/// The WCL type a `duration` param's unit literal resolves against; its
/// `@unit` decorators supply the suffixes (`ns`/`us`/`ms`/`s`/`min`/`h`/`d`).
pub const DURATION_TYPE: &str = "std.Duration";

impl CoarseType {
    pub fn parse(s: &str) -> Option<CoarseType> {
        match s {
            "string" => Some(CoarseType::String),
            "int" => Some(CoarseType::Int),
            "float" => Some(CoarseType::Float),
            "bool" => Some(CoarseType::Bool),
            "list" => Some(CoarseType::List),
            "map" => Some(CoarseType::Map),
            "symbol" => Some(CoarseType::Symbol),
            "duration" => Some(CoarseType::Duration),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CoarseType::String => "string",
            CoarseType::Int => "int",
            CoarseType::Float => "float",
            CoarseType::Bool => "bool",
            CoarseType::List => "list",
            CoarseType::Map => "map",
            CoarseType::Symbol => "symbol",
            CoarseType::Duration => "duration",
        }
    }

    /// Coarse match: ints are acceptable where floats are declared, and
    /// symbols are strings post-conversion (see `Symbol`).
    pub fn matches(&self, v: &DynValue) -> bool {
        matches!(
            (self, v),
            (CoarseType::String, DynValue::String(_))
                | (CoarseType::Int, DynValue::Int(_))
                | (CoarseType::Float, DynValue::Float(_))
                | (CoarseType::Float, DynValue::Int(_))
                | (CoarseType::Bool, DynValue::Bool(_))
                | (CoarseType::List, DynValue::List(_))
                | (CoarseType::Map, DynValue::Map(_))
                | (CoarseType::Symbol, DynValue::String(_))
                // Resolved to base nanoseconds before it ever gets here.
                | (CoarseType::Duration, DynValue::Int(_))
        )
    }

    pub fn describe(v: &DynValue) -> &'static str {
        match v {
            DynValue::Null => "null",
            DynValue::Bool(_) => "bool",
            DynValue::Int(_) => "int",
            DynValue::Float(_) => "float",
            DynValue::String(_) => "string",
            DynValue::List(_) => "list",
            DynValue::Map(_) => "map",
        }
    }
}
