//! The pipeline inventory: `{root}/pipelines.wcl` — pipelines own
//! properties, secrets, targets, triggers and an ordered list of steps.
//! Loaded through the embedded vocabulary (one source of truth with the
//! CLI's `src/vocab/pipeline.wcl`) and regenerated from structs on every
//! edit via wcl_lang's AST builder + printer, so the file is always
//! schema-valid canonical WCL. Secrets are stored inline; the file is kept
//! at mode 0600.

use std::path::Path;

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wcl_lang::{Document, Environment, Registry, ast, disk_loader, edit, format as wclformat};
use weave_remote::{TargetOs, TransportConfig, TransportKind};

use crate::state::SharedState;

/// One source of truth: the vocab embedded in the CLI crate.
const PIPELINE_VOCAB: &str = include_str!("../../src/vocab/pipeline.wcl");
const PIPELINE_IMPORT: &str = "weave/pipeline.wcl";

// --------------------------------------------------------------- model

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_type")]
    pub r#type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
}

fn default_type() -> String {
    "string".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Never serialized back out (redacted in every API response); accepted
    /// on input so create/update can carry new values.
    #[serde(default, skip_serializing)]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub os: TargetOs,
    pub transport: TransportConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerDef {
    pub name: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub bindings: Vec<(String, String)>,
}

fn default_true() -> bool {
    true
}

/// A step: either a shell script (local or on a target) or a config-weave
/// play. Serialized with a `kind` tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepDef {
    Script {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default = "default_local")]
        on: String,
        run: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        #[serde(default = "default_true")]
        stop_on_failure: bool,
    },
    Play {
        name: String,
        #[serde(default)]
        description: Option<String>,
        playbook: String,
        play: String,
        #[serde(default = "default_apply")]
        action: String,
        #[serde(default)]
        vars: Vec<(String, String)>,
        #[serde(default = "default_true")]
        stop_on_failure: bool,
    },
}

fn default_local() -> String {
    "local".into()
}
fn default_apply() -> String {
    "apply".into()
}

impl StepDef {
    pub fn name(&self) -> &str {
        match self {
            StepDef::Script { name, .. } | StepDef::Play { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
    #[serde(default)]
    pub secrets: Vec<SecretDef>,
    #[serde(default)]
    pub targets: Vec<TargetDef>,
    #[serde(default)]
    pub triggers: Vec<TriggerDef>,
    #[serde(default)]
    pub steps: Vec<StepDef>,
}

// ---------------------------------------------------------------- load

fn pipeline_loader() -> wcl_lang::FileLoader {
    let mut reg = Registry::new();
    reg.register(PIPELINE_IMPORT, PIPELINE_VOCAB);
    reg.loader(disk_loader())
}

/// Read and schema-validate `pipelines.wcl`. A missing file is an empty
/// inventory; a malformed one is an error (a later save must not clobber a
/// file we could not fully read).
pub fn load(path: &Path) -> Result<Vec<PipelineDef>, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };

    let mut with_import = source.clone();
    if !with_import.ends_with('\n') {
        with_import.push('\n');
    }
    with_import.push_str(&format!("import <{PIPELINE_IMPORT}>\n"));

    let env = Environment::new();
    let doc = Document::open_at_with_loader(
        &with_import,
        "pipelines.wcl",
        path.parent().map(|p| p.to_path_buf()),
        &env,
        pipeline_loader(),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;

    let schema_errors = doc.schema_errors();
    if !schema_errors.is_empty() {
        let msgs: Vec<String> = schema_errors.iter().map(|e| e.to_string()).collect();
        return Err(format!("{}: {}", path.display(), msgs.join("; ")));
    }

    let mut pipelines = Vec::new();
    for block in doc.blocks() {
        if block.kind() != "pipeline" {
            continue;
        }
        pipelines.push(read_pipeline(&block).map_err(|e| format!("{}: {e}", path.display()))?);
    }

    let mut seen = std::collections::HashSet::new();
    for p in &pipelines {
        if !seen.insert(p.name.as_str()) {
            return Err(format!("{}: duplicate pipeline '{}'", path.display(), p.name));
        }
    }
    Ok(pipelines)
}

fn read_pipeline(block: &wcl_lang::Block<'_>) -> Result<PipelineDef, String> {
    let name = label_of(block)?;
    let ctx = format!("pipeline '{name}'");

    let properties = block
        .blocks()
        .filter(|b| b.kind() == "property")
        .map(|b| read_property(&b))
        .collect::<Result<Vec<_>, _>>()?;
    let secrets = block
        .blocks()
        .filter(|b| b.kind() == "secret")
        .map(|b| {
            let sname = label_of(&b)?;
            let sctx = format!("{ctx} secret '{sname}'");
            Ok(SecretDef {
                description: str_field(&b, "description")?,
                value: req_str_field(&b, "value", &sctx)?,
                name: sname,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let targets = block
        .blocks()
        .filter(|b| b.kind() == "target")
        .map(|b| read_target(&b))
        .collect::<Result<Vec<_>, _>>()?;
    let triggers = block
        .blocks()
        .filter(|b| b.kind() == "trigger")
        .map(|b| read_trigger(&b))
        .collect::<Result<Vec<_>, _>>()?;

    // Steps: a SINGLE ordered pass over the child blocks, matching on kind,
    // so script/play steps interleave in declaration order (unlike the
    // order-independent lists above).
    let mut steps = Vec::new();
    for b in block.blocks() {
        match b.kind() {
            "script" => steps.push(read_script_step(&b)?),
            "play" => steps.push(read_play_step(&b)?),
            _ => {}
        }
    }

    Ok(PipelineDef {
        description: str_field(block, "description")?,
        properties,
        secrets,
        targets,
        triggers,
        steps,
        name,
    })
}

fn read_property(block: &wcl_lang::Block<'_>) -> Result<PropertyDef, String> {
    let name = label_of(block)?;
    Ok(PropertyDef {
        r#type: str_field(block, "type")?.unwrap_or_else(default_type),
        required: bool_field(block, "required")?.unwrap_or(false),
        default: str_field(block, "default")?,
        description: str_field(block, "description")?,
        name,
    })
}

fn read_target(block: &wcl_lang::Block<'_>) -> Result<TargetDef, String> {
    let name = label_of(block)?;
    let ctx = format!("target '{name}'");
    let os_s = str_field(block, "os")?.unwrap_or_else(|| "linux".into());
    let os = TargetOs::parse(&os_s)
        .ok_or_else(|| format!("{ctx}: os must be \"linux\" or \"windows\", got \"{os_s}\""))?;
    let transport_block = block
        .blocks()
        .find(|b| b.kind() == "transport")
        .ok_or_else(|| format!("{ctx}: missing transport block"))?;
    Ok(TargetDef {
        transport: read_transport(&transport_block, &ctx)?,
        os,
        description: str_field(block, "description")?,
        name,
    })
}

fn read_transport(block: &wcl_lang::Block<'_>, ctx: &str) -> Result<TransportConfig, String> {
    let kind_s = label_of(block)?;
    let kind = TransportKind::parse(&kind_s)
        .ok_or_else(|| format!("{ctx}: transport must be \"ssh\" or \"winrm\", got \"{kind_s}\""))?;
    let port = match block.fields().find(|f| f.name() == "port") {
        None => None,
        Some(f) => match f.value().map_err(|e| e.to_string())?.clone() {
            wcl_lang::Value::I64(i) => {
                Some(u16::try_from(i).map_err(|_| format!("{ctx}: port {i} out of range"))?)
            }
            other => return Err(format!("{ctx}: port must be an integer, got {other:?}")),
        },
    };
    Ok(TransportConfig {
        kind,
        host: req_str_field(block, "host", ctx)?,
        port,
        user: req_str_field(block, "user", ctx)?,
        password: str_field(block, "password")?,
        private_key: str_field(block, "private_key")?,
        use_tls: bool_field(block, "use_tls")?.unwrap_or(false),
    })
}

fn read_trigger(block: &wcl_lang::Block<'_>) -> Result<TriggerDef, String> {
    let name = label_of(block)?;
    Ok(TriggerDef {
        r#type: str_field(block, "type")?.unwrap_or_else(|| "manual".into()),
        webhook_secret: str_field(block, "webhook_secret")?,
        cron: str_field(block, "cron")?,
        enabled: bool_field(block, "enabled")?.unwrap_or(true),
        bindings: block
            .blocks()
            .filter(|b| b.kind() == "bind")
            .map(|b| Ok((label_of(&b)?, req_str_field(&b, "value", "bind")?)))
            .collect::<Result<Vec<_>, String>>()?,
        name,
    })
}

fn read_key_vals(block: &wcl_lang::Block<'_>, kind: &str) -> Result<Vec<(String, String)>, String> {
    block
        .blocks()
        .filter(|b| b.kind() == kind)
        .map(|b| Ok((label_of(&b)?, req_str_field(&b, "value", kind)?)))
        .collect()
}

fn read_script_step(block: &wcl_lang::Block<'_>) -> Result<StepDef, String> {
    let name = label_of(block)?;
    let ctx = format!("script '{name}'");
    Ok(StepDef::Script {
        on: str_field(block, "on")?.unwrap_or_else(default_local),
        run: req_str_field(block, "run", &ctx)?,
        shell: str_field(block, "shell")?,
        env: read_key_vals(block, "env")?,
        stop_on_failure: bool_field(block, "stop_on_failure")?.unwrap_or(true),
        description: str_field(block, "description")?,
        name,
    })
}

fn read_play_step(block: &wcl_lang::Block<'_>) -> Result<StepDef, String> {
    let name = label_of(block)?;
    let ctx = format!("play '{name}'");
    Ok(StepDef::Play {
        playbook: req_str_field(block, "playbook", &ctx)?,
        play: req_str_field(block, "play", &ctx)?,
        action: str_field(block, "action")?.unwrap_or_else(default_apply),
        vars: read_key_vals(block, "var")?,
        stop_on_failure: bool_field(block, "stop_on_failure")?.unwrap_or(true),
        description: str_field(block, "description")?,
        name,
    })
}

// --------------------------------------------------------- field readers

fn label_of(block: &wcl_lang::Block<'_>) -> Result<String, String> {
    match block.labels().map_err(|e| e.to_string())?.into_iter().next() {
        Some(wcl_lang::Value::Utf8(s))
        | Some(wcl_lang::Value::Ascii(s))
        | Some(wcl_lang::Value::Identifier(s)) => Ok(s),
        _ => Err(format!("{} block has no name label", block.kind())),
    }
}

fn str_field(block: &wcl_lang::Block<'_>, name: &str) -> Result<Option<String>, String> {
    let Some(f) = block.fields().find(|f| f.name() == name) else {
        return Ok(None);
    };
    match f.value().map_err(|e| e.to_string())?.clone() {
        wcl_lang::Value::Utf8(s) | wcl_lang::Value::Ascii(s) | wcl_lang::Value::Identifier(s) => {
            Ok(Some(s))
        }
        other => Err(format!("field '{name}' must be a string, got {other:?}")),
    }
}

fn bool_field(block: &wcl_lang::Block<'_>, name: &str) -> Result<Option<bool>, String> {
    let Some(f) = block.fields().find(|f| f.name() == name) else {
        return Ok(None);
    };
    match f.value().map_err(|e| e.to_string())?.clone() {
        wcl_lang::Value::Bool(b) => Ok(Some(b)),
        other => Err(format!("field '{name}' must be a bool, got {other:?}")),
    }
}

fn req_str_field(block: &wcl_lang::Block<'_>, name: &str, ctx: &str) -> Result<String, String> {
    str_field(block, name)?.ok_or_else(|| format!("{ctx}: missing field '{name}'"))
}

// ---------------------------------------------------------------- save

fn str_expr(s: &str) -> ast::Expr {
    edit::string_literal_expr(s)
}

/// Regenerate `pipelines.wcl` from the inventory: fresh AST through the
/// canonical printer, written atomically at mode 0600.
pub fn save(path: &Path, pipelines: &[PipelineDef]) -> Result<(), String> {
    let mut src = ast::Source {
        items: Vec::new(),
        trailing_trivia: Vec::new(),
    };
    for p in pipelines {
        edit::append_top_level_block(&mut src, pipeline_block(p));
    }

    let header = [
        " Config Weave pipelines — managed by config-weave-pipeline.",
        " Edits regenerate this file; hand edits survive a reload but not",
        " the next edit save. Kept at mode 0600 (inline secrets).",
    ];
    match src.items.first_mut() {
        Some(ast::Item::Block(b)) => {
            let mut trivia: Vec<ast::Trivia> = header
                .iter()
                .map(|l| ast::Trivia::LineComment(l.to_string()))
                .collect();
            trivia.push(ast::Trivia::BlankLine);
            trivia.append(&mut b.leading_trivia);
            b.leading_trivia = trivia;
        }
        _ => {
            src.trailing_trivia = header
                .iter()
                .map(|l| ast::Trivia::LineComment(l.to_string()))
                .collect();
        }
    }

    let rendered = wclformat::to_source(&src);
    let tmp = path.with_extension("wcl.weave-tmp");
    std::fs::write(&tmp, &rendered).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot chmod {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

fn key_val_block(kind: &str, name: &str, value: &str) -> ast::Block {
    edit::build_block(kind, &[], vec![str_expr(name)], vec![("value".into(), str_expr(value))])
}

fn pipeline_block(p: &PipelineDef) -> ast::Block {
    let mut fields = Vec::new();
    if let Some(d) = &p.description {
        fields.push(("description".into(), str_expr(d)));
    }
    let mut block = edit::build_block("pipeline", &[], vec![str_expr(&p.name)], fields);

    for prop in &p.properties {
        let mut f: Vec<(String, ast::Expr)> = Vec::new();
        if let Some(d) = &prop.description {
            f.push(("description".into(), str_expr(d)));
        }
        f.push(("type".into(), str_expr(&prop.r#type)));
        if prop.required {
            f.push(("required".into(), ast::Expr::Bool(true)));
        }
        if let Some(def) = &prop.default {
            f.push(("default".into(), str_expr(def)));
        }
        block.items.push(ast::Item::Block(edit::build_block(
            "property",
            &[],
            vec![str_expr(&prop.name)],
            f,
        )));
    }

    for secret in &p.secrets {
        let mut f: Vec<(String, ast::Expr)> = Vec::new();
        if let Some(d) = &secret.description {
            f.push(("description".into(), str_expr(d)));
        }
        f.push(("value".into(), str_expr(&secret.value)));
        block.items.push(ast::Item::Block(edit::build_block(
            "secret",
            &[],
            vec![str_expr(&secret.name)],
            f,
        )));
    }

    for target in &p.targets {
        let mut f: Vec<(String, ast::Expr)> = Vec::new();
        if let Some(d) = &target.description {
            f.push(("description".into(), str_expr(d)));
        }
        f.push(("os".into(), str_expr(target.os.as_str())));
        let mut tblock = edit::build_block("target", &[], vec![str_expr(&target.name)], f);
        tblock.items.push(ast::Item::Block(transport_block(&target.transport)));
        block.items.push(ast::Item::Block(tblock));
    }

    for trigger in &p.triggers {
        let mut f: Vec<(String, ast::Expr)> = vec![("type".into(), str_expr(&trigger.r#type))];
        if let Some(s) = &trigger.webhook_secret {
            f.push(("webhook_secret".into(), str_expr(s)));
        }
        if let Some(c) = &trigger.cron {
            f.push(("cron".into(), str_expr(c)));
        }
        if !trigger.enabled {
            f.push(("enabled".into(), ast::Expr::Bool(false)));
        }
        let mut tblock = edit::build_block("trigger", &[], vec![str_expr(&trigger.name)], f);
        for (prop, value) in &trigger.bindings {
            tblock.items.push(ast::Item::Block(edit::build_block(
                "bind",
                &[],
                vec![str_expr(prop)],
                vec![("value".into(), str_expr(value))],
            )));
        }
        block.items.push(ast::Item::Block(tblock));
    }

    for step in &p.steps {
        block.items.push(ast::Item::Block(step_block(step)));
    }
    block
}

fn transport_block(t: &TransportConfig) -> ast::Block {
    let mut tf: Vec<(String, ast::Expr)> = vec![("host".into(), str_expr(&t.host))];
    if let Some(port) = t.port {
        tf.push(("port".into(), ast::Expr::I64(i64::from(port))));
    }
    tf.push(("user".into(), str_expr(&t.user)));
    if let Some(pw) = &t.password {
        tf.push(("password".into(), str_expr(pw)));
    }
    if let Some(k) = &t.private_key {
        tf.push(("private_key".into(), str_expr(k)));
    }
    if t.use_tls {
        tf.push(("use_tls".into(), ast::Expr::Bool(true)));
    }
    edit::build_block("transport", &[], vec![str_expr(t.kind.as_str())], tf)
}

fn step_block(step: &StepDef) -> ast::Block {
    match step {
        StepDef::Script {
            name,
            description,
            on,
            run,
            shell,
            env,
            stop_on_failure,
        } => {
            let mut f: Vec<(String, ast::Expr)> = Vec::new();
            if let Some(d) = description {
                f.push(("description".into(), str_expr(d)));
            }
            f.push(("on".into(), str_expr(on)));
            f.push(("run".into(), str_expr(run)));
            if let Some(s) = shell {
                f.push(("shell".into(), str_expr(s)));
            }
            if !stop_on_failure {
                f.push(("stop_on_failure".into(), ast::Expr::Bool(false)));
            }
            let mut b = edit::build_block("script", &[], vec![str_expr(name)], f);
            for (k, v) in env {
                b.items.push(ast::Item::Block(key_val_block("env", k, v)));
            }
            b
        }
        StepDef::Play {
            name,
            description,
            playbook,
            play,
            action,
            vars,
            stop_on_failure,
        } => {
            let mut f: Vec<(String, ast::Expr)> = Vec::new();
            if let Some(d) = description {
                f.push(("description".into(), str_expr(d)));
            }
            f.push(("playbook".into(), str_expr(playbook)));
            f.push(("play".into(), str_expr(play)));
            f.push(("action".into(), str_expr(action)));
            if !stop_on_failure {
                f.push(("stop_on_failure".into(), ast::Expr::Bool(false)));
            }
            let mut b = edit::build_block("play", &[], vec![str_expr(name)], f);
            for (k, v) in vars {
                b.items.push(ast::Item::Block(key_val_block("var", k, v)));
            }
            b
        }
    }
}

// ------------------------------------------------------------- handlers

/// A pipeline with secret values redacted (names/descriptions only). Used
/// for every read response.
fn redacted(p: &PipelineDef) -> serde_json::Value {
    // SecretDef skips `value` on serialize, so serde_json already omits it.
    serde_json::to_value(p).unwrap_or(json!({}))
}

/// Persist the in-memory inventory to disk, then swap it in.
fn persist(state: &SharedState, pipelines: Vec<PipelineDef>) -> Result<(), String> {
    save(&state.pipelines_path, &pipelines)?;
    *state.pipelines.lock().unwrap() = pipelines;
    Ok(())
}

pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    let pipelines = state.pipelines.lock().unwrap();
    let out: Vec<serde_json::Value> = pipelines
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "description": p.description,
                "steps": p.steps.len(),
                "triggers": p.triggers.iter().map(|t| json!({"name": t.name, "type": t.r#type, "enabled": t.enabled})).collect::<Vec<_>>(),
            })
        })
        .collect();
    ok(json!({ "pipelines": out }))
}

pub async fn get(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let pipelines = state.pipelines.lock().unwrap();
    match pipelines.iter().find(|p| p.name == name) {
        Some(p) => ok(redacted(p)),
        None => err(StatusCode::NOT_FOUND, "no such pipeline"),
    }
}

pub async fn create(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
    axum::Json(def): axum::Json<PipelineDef>,
) -> Response {
    let mut pipelines = state.pipelines.lock().unwrap().clone();
    if pipelines.iter().any(|p| p.name == def.name) {
        return err(StatusCode::CONFLICT, "pipeline already exists");
    }
    pipelines.push(def);
    match persist(&state, pipelines) {
        Ok(()) => ok(json!({ "created": true })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn update(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(mut def): axum::Json<PipelineDef>,
) -> Response {
    let mut pipelines = state.pipelines.lock().unwrap().clone();
    let Some(idx) = pipelines.iter().position(|p| p.name == name) else {
        return err(StatusCode::NOT_FOUND, "no such pipeline");
    };
    // Secret values are redacted in reads, so an incoming secret with an
    // empty value means "keep the stored one" — never wipe a secret on a
    // round-trip edit.
    let existing = &pipelines[idx];
    for secret in def.secrets.iter_mut() {
        if secret.value.is_empty()
            && let Some(prev) = existing.secrets.iter().find(|s| s.name == secret.name)
        {
            secret.value = prev.value.clone();
        }
    }
    def.name = name;
    pipelines[idx] = def;
    match persist(&state, pipelines) {
        Ok(()) => ok(json!({ "updated": true })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn delete(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let mut pipelines = state.pipelines.lock().unwrap().clone();
    let before = pipelines.len();
    pipelines.retain(|p| p.name != name);
    if pipelines.len() == before {
        return err(StatusCode::NOT_FOUND, "no such pipeline");
    }
    match persist(&state, pipelines) {
        Ok(()) => ok(json!({ "deleted": true })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn list_secrets(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let pipelines = state.pipelines.lock().unwrap();
    match pipelines.iter().find(|p| p.name == name) {
        Some(p) => ok(json!({
            "secrets": p.secrets.iter().map(|s| json!({"name": s.name, "description": s.description})).collect::<Vec<_>>()
        })),
        None => err(StatusCode::NOT_FOUND, "no such pipeline"),
    }
}

#[derive(Deserialize)]
pub struct SecretBody {
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn set_secret(
    Extension(state): Extension<SharedState>,
    UrlPath((name, secret)): UrlPath<(String, String)>,
    _claims: RequireClaims,
    axum::Json(body): axum::Json<SecretBody>,
) -> Response {
    let mut pipelines = state.pipelines.lock().unwrap().clone();
    let Some(p) = pipelines.iter_mut().find(|p| p.name == name) else {
        return err(StatusCode::NOT_FOUND, "no such pipeline");
    };
    match p.secrets.iter_mut().find(|s| s.name == secret) {
        Some(s) => {
            s.value = body.value;
            if body.description.is_some() {
                s.description = body.description;
            }
        }
        None => p.secrets.push(SecretDef {
            name: secret,
            description: body.description,
            value: body.value,
        }),
    }
    match persist(&state, pipelines) {
        Ok(()) => ok(json!({ "set": true })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn delete_secret(
    Extension(state): Extension<SharedState>,
    UrlPath((name, secret)): UrlPath<(String, String)>,
    _claims: RequireClaims,
) -> Response {
    let mut pipelines = state.pipelines.lock().unwrap().clone();
    let Some(p) = pipelines.iter_mut().find(|p| p.name == name) else {
        return err(StatusCode::NOT_FOUND, "no such pipeline");
    };
    let before = p.secrets.len();
    p.secrets.retain(|s| s.name != secret);
    if p.secrets.len() == before {
        return err(StatusCode::NOT_FOUND, "no such secret");
    }
    match persist(&state, pipelines) {
        Ok(()) => ok(json!({ "deleted": true })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_save_round_trips_and_preserves_step_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipelines.wcl");
        std::fs::write(
            &path,
            r#"
pipeline "demo" {
  description = "d"
  property "version" { type = "string"  required = true }
  secret "tok" { value = "s3cr3t" }
  target "web1" {
    os = "linux"
    transport "ssh" { host = "h"  user = "u"  private_key = "secret:tok" }
  }
  trigger "hook" { type = "webhook"  webhook_secret = "abc" }
  script "one" { on = "local"  run = "echo 1" }
  play  "two" { playbook = "pb"  play = "p"  action = "apply" }
  script "three" { on = "web1"  run = "echo 3" }
}
"#,
        )
        .unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        let p = &loaded[0];
        assert_eq!(p.name, "demo");
        assert_eq!(p.secrets[0].value, "s3cr3t");
        // Interleaved order preserved: script, play, script.
        let order: Vec<&str> = p.steps.iter().map(|s| s.name()).collect();
        assert_eq!(order, ["one", "two", "three"]);
        assert!(matches!(p.steps[0], StepDef::Script { .. }));
        assert!(matches!(p.steps[1], StepDef::Play { .. }));

        // Save and reload — same shape, same order.
        save(&path, &loaded).unwrap();
        let again = load(&path).unwrap();
        let order2: Vec<&str> = again[0].steps.iter().map(|s| s.name()).collect();
        assert_eq!(order2, ["one", "two", "three"]);
        assert_eq!(again[0].secrets[0].value, "s3cr3t");
    }

    #[test]
    fn redacted_output_omits_secret_values() {
        let p = PipelineDef {
            name: "x".into(),
            description: None,
            properties: vec![],
            secrets: vec![SecretDef {
                name: "tok".into(),
                description: None,
                value: "s3cr3t".into(),
            }],
            targets: vec![],
            triggers: vec![],
            steps: vec![],
        };
        let json = redacted(&p).to_string();
        assert!(json.contains("tok"));
        assert!(!json.contains("s3cr3t"));
    }
}
