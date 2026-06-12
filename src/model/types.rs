//! The playbook data model: what `playbook.wcl` and the `package.wcl`
//! files describe, in plain owned Rust. Loaded once; expression fields
//! (conditions, properties, gather params, var values) stay *deferred* —
//! they are evaluated at run time against a freshly opened document with
//! the generated variables import bound.

use std::collections::BTreeMap;
use std::path::PathBuf;

use wisp_std::DynValue;

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

#[derive(Debug)]
pub struct Step {
    pub name: String,
    pub description: String,
    pub package: String,
    pub resource: String,
    pub requires: Vec<String>,
    /// Step-level concurrency tightening, if declared.
    pub concurrency: Option<Concurrency>,
    /// Names of enclosing containers, outermost first. Used to locate the
    /// step's block at run time and to inherit container conditions.
    pub container_path: Vec<String>,
    /// Raw condition expression text, for documentation.
    pub condition_src: Option<String>,
    pub span: (usize, usize),
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
    pub gatherers: BTreeMap<String, GathererDecl>,
    pub resources: BTreeMap<String, ResourceDecl>,
    /// Convergence tests, in declaration order.
    pub tests: Vec<TestDecl>,
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
}

#[derive(Debug, Clone)]
pub struct ParamDecl {
    pub name: String,
    pub description: String,
    pub ty: CoarseType,
    pub required: bool,
    pub default: Option<DynValue>,
}

/// An isolated convergence test declared in `package.wcl`, executed by
/// `config-weave test` inside a disposable backend instance.
#[derive(Debug)]
pub struct TestDecl {
    pub name: String,
    pub description: String,
    /// Backend selector; only "docker" exists in v1 ("vmlab" is planned).
    pub backend: String,
    /// Backend-specific image reference (docker image ref in v1).
    pub image: String,
    /// Optional shell provisioning, run via `sh -c` before anything else.
    pub setup: Option<String>,
    /// Absolute path to the optional wisp verify script.
    pub verify: Option<PathBuf>,
    pub steps: Vec<TestStep>,
    pub gathers: Vec<TestGather>,
    pub span: (usize, usize),
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
}

impl CoarseType {
    pub fn parse(s: &str) -> Option<CoarseType> {
        match s {
            "string" => Some(CoarseType::String),
            "int" => Some(CoarseType::Int),
            "float" => Some(CoarseType::Float),
            "bool" => Some(CoarseType::Bool),
            "list" => Some(CoarseType::List),
            "map" => Some(CoarseType::Map),
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
        }
    }

    /// Coarse match: ints are acceptable where floats are declared.
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
