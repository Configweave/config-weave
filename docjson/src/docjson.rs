//! DocJson: the structural document the graphical editors edit.
//!
//! A DocJson is extracted from a `parse_for_edit` AST (never from the
//! evaluated model — the model is lossy for editing) and synced back
//! onto the *current* file's AST on save, so comments and unknown
//! constructs survive. Every leaf value is a [`Val`]: a plain literal
//! (typed form widget) or raw WCL expression source (fx mode). Shapes
//! mirror `src/vocab/playbook.wcl` / `package.wcl` exactly.
//!
//! `orig` carries the block's on-disk name through a rename: extraction
//! sets it, the form edits `name` freely, and the sync matches blocks
//! by `orig` so comments stay attached. A doc entry with no `orig` (or
//! one that no longer matches) is a new block.

use serde::{Deserialize, Serialize};

/// A leaf value: `{"lit": …}` for plain scalars, `{"expr": "…"}` for
/// raw WCL expression source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Val {
    Lit(serde_json::Value),
    Expr(String),
}

/// One `key = value` entry of a schemaless map block (vars, properties,
/// params, expect).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kv {
    pub key: String,
    pub value: Val,
}

fn is_none<T>(v: &Option<T>) -> bool {
    v.is_none()
}

// ------------------------------------------------------------- playbook

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybookDoc {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub gathers: Vec<GatherDoc>,
    #[serde(default)]
    pub vars: Vec<Kv>,
    #[serde(default)]
    pub plays: Vec<PlayDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatherDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub description: Option<String>,
    pub from: String,
    #[serde(default)]
    pub params: Vec<Kv>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    /// None = schema default (true).
    #[serde(default, skip_serializing_if = "is_none")]
    pub parallel: Option<bool>,
    #[serde(default)]
    pub items: Vec<PlayItemDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayItemDoc {
    Step(StepDoc),
    Container(ContainerDoc),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    pub resource: String,
    /// Raw WCL expression source.
    #[serde(default, skip_serializing_if = "is_none")]
    pub condition: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub concurrency: Option<String>,
    #[serde(default)]
    pub properties: Vec<Kv>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub condition: Option<String>,
    #[serde(default)]
    pub items: Vec<PlayItemDoc>,
}

// -------------------------------------------------------------- package

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageDoc {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub gatherers: Vec<GathererDoc>,
    #[serde(default)]
    pub resources: Vec<ResourceDoc>,
    #[serde(default)]
    pub tests: Vec<TestDoc>,
    #[serde(default)]
    pub scenarios: Vec<ScenarioDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GathererDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    pub script: String,
    #[serde(default)]
    pub params: Vec<ParamDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    pub script: String,
    /// None = schema default ("parallel").
    #[serde(default, skip_serializing_if = "is_none")]
    pub concurrency: Option<String>,
    #[serde(default)]
    pub params: Vec<ParamDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub default: Option<Val>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    /// None = schema default ("docker").
    #[serde(default, skip_serializing_if = "is_none")]
    pub backend: Option<String>,
    pub image: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub verify: Option<String>,
    #[serde(default)]
    pub steps: Vec<TestStepDoc>,
    #[serde(default)]
    pub gathers: Vec<TestGatherDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestStepDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    pub resource: String,
    /// None = schema default ("converge").
    #[serde(default, skip_serializing_if = "is_none")]
    pub expect: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub condition: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub properties: Vec<Kv>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestGatherDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    pub from: String,
    #[serde(default)]
    pub params: Vec<Kv>,
    #[serde(default)]
    pub expect: Vec<Kv>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioDoc {
    pub name: String,
    #[serde(default, rename = "_orig", skip_serializing_if = "is_none")]
    pub orig: Option<String>,
    pub description: String,
    pub lab: String,
    pub script: String,
}
