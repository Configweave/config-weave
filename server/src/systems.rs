//! The systems inventory: `{root}/systems.wcl` — machines configuration
//! is applied to. Loaded through the embedded systems vocab (the same
//! file the CLI serves as `<weave/systems.wcl>`) and regenerated from
//! structs on every GUI edit via wcl_lang's AST builder + printer, so
//! the file is always schema-valid canonical WCL. Credentials are stored
//! inline by explicit choice; the file is kept at mode 0600.

use std::path::Path;

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde::{Deserialize, Serialize};
use wcl_lang::{Document, Environment, Registry, ast, disk_loader, edit, format as wclformat};

use crate::runbooks::runbook_dir;
use crate::state::SharedState;

/// One source of truth: the vocab embedded in the CLI crate.
const SYSTEMS_VOCAB: &str = include_str!("../../src/vocab/systems.wcl");
const SYSTEMS_IMPORT: &str = "weave/systems.wcl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemKind {
    /// config-weave is copied to and runs ON the target.
    Direct,
    /// The playbook runs locally on the server; wscripts connect out
    /// themselves using the injected `system_*` vars.
    Remote,
}

impl SystemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SystemKind::Direct => "direct",
            SystemKind::Remote => "remote",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "direct" => Some(SystemKind::Direct),
            "remote" => Some(SystemKind::Remote),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetOs {
    Linux,
    Windows,
}

impl TargetOs {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetOs::Linux => "linux",
            TargetOs::Windows => "windows",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "linux" => Some(TargetOs::Linux),
            "windows" => Some(TargetOs::Windows),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    Ssh,
    Winrm,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Ssh => "ssh",
            TransportKind::Winrm => "winrm",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "ssh" => Some(TransportKind::Ssh),
            "winrm" => Some(TransportKind::Winrm),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransportConfig {
    pub kind: TransportKind,
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub user: String,
    #[serde(default)]
    pub password: Option<String>,
    /// ssh: path to a key file, or an inline PEM body.
    #[serde(default)]
    pub private_key: Option<String>,
    /// winrm: HTTPS (5986).
    #[serde(default)]
    pub use_tls: bool,
}

impl TransportConfig {
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(match self.kind {
            TransportKind::Ssh => 22,
            TransportKind::Winrm => {
                if self.use_tls {
                    5986
                } else {
                    5985
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Runbook (immediate child dir of the server root) and play to run.
    pub playbook: String,
    pub play: String,
    pub kind: SystemKind,
    pub os: TargetOs,
    pub arch: String,
    pub transport: TransportConfig,
}

impl SystemDef {
    /// Key into the deploy-binary registry, matching `dist/` naming.
    pub fn binary_key(&self) -> String {
        format!("{}-{}", self.os.as_str(), self.arch)
    }
}

// ------------------------------------------------------------------ load

fn systems_loader() -> wcl_lang::FileLoader {
    let mut reg = Registry::new();
    reg.register(SYSTEMS_IMPORT, SYSTEMS_VOCAB);
    reg.loader(disk_loader())
}

/// Read and schema-validate `systems.wcl`. A missing file is an empty
/// inventory; a malformed one is an error (the caller must not risk a
/// later save clobbering a file we could not fully read).
pub fn load(path: &Path) -> Result<Vec<SystemDef>, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };

    // Same suffix trick as the CLI: the import is appended so user spans
    // stay untouched and the file itself never carries the import line.
    let mut with_import = source.clone();
    if !with_import.ends_with('\n') {
        with_import.push('\n');
    }
    with_import.push_str(&format!("import <{SYSTEMS_IMPORT}>\n"));

    let env = Environment::new();
    let doc = Document::open_at_with_loader(
        &with_import,
        "systems.wcl",
        path.parent().map(|p| p.to_path_buf()),
        &env,
        systems_loader(),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;

    let schema_errors = doc.schema_errors();
    if !schema_errors.is_empty() {
        let msgs: Vec<String> = schema_errors.iter().map(|e| e.to_string()).collect();
        return Err(format!("{}: {}", path.display(), msgs.join("; ")));
    }

    let mut systems = Vec::new();
    for block in doc.blocks() {
        if block.kind() != "system" {
            continue;
        }
        systems.push(read_system(&block).map_err(|e| format!("{}: {e}", path.display()))?);
    }

    let mut seen = std::collections::HashSet::new();
    for s in &systems {
        if !seen.insert(s.name.as_str()) {
            return Err(format!(
                "{}: duplicate system name '{}'",
                path.display(),
                s.name
            ));
        }
    }
    Ok(systems)
}

fn label_of(block: &wcl_lang::Block<'_>) -> Result<String, String> {
    match block
        .labels()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
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

fn req_str_field(block: &wcl_lang::Block<'_>, name: &str, ctx: &str) -> Result<String, String> {
    str_field(block, name)?.ok_or_else(|| format!("{ctx}: missing field '{name}'"))
}

fn read_system(block: &wcl_lang::Block<'_>) -> Result<SystemDef, String> {
    let name = label_of(block)?;
    let ctx = format!("system '{name}'");

    let kind_s = str_field(block, "kind")?.unwrap_or_else(|| "direct".into());
    let kind = SystemKind::parse(&kind_s)
        .ok_or_else(|| format!("{ctx}: kind must be \"direct\" or \"remote\", got \"{kind_s}\""))?;
    let os_s = str_field(block, "os")?.unwrap_or_else(|| "linux".into());
    let os = TargetOs::parse(&os_s)
        .ok_or_else(|| format!("{ctx}: os must be \"linux\" or \"windows\", got \"{os_s}\""))?;

    let transport_block = block
        .blocks()
        .find(|b| b.kind() == "transport")
        .ok_or_else(|| format!("{ctx}: missing transport block"))?;
    let transport = read_transport(&transport_block, &ctx)?;

    Ok(SystemDef {
        description: str_field(block, "description")?,
        playbook: req_str_field(block, "playbook", &ctx)?,
        play: req_str_field(block, "play", &ctx)?,
        kind,
        os,
        arch: str_field(block, "arch")?.unwrap_or_else(|| "x86_64".into()),
        transport,
        name,
    })
}

fn read_transport(block: &wcl_lang::Block<'_>, ctx: &str) -> Result<TransportConfig, String> {
    let kind_s = label_of(block)?;
    let kind = TransportKind::parse(&kind_s).ok_or_else(|| {
        format!("{ctx}: transport must be \"ssh\" or \"winrm\", got \"{kind_s}\"")
    })?;

    let port = match block.fields().find(|f| f.name() == "port") {
        None => None,
        Some(f) => match f.value().map_err(|e| e.to_string())?.clone() {
            wcl_lang::Value::I64(i) => {
                Some(u16::try_from(i).map_err(|_| format!("{ctx}: port {i} out of range"))?)
            }
            other => return Err(format!("{ctx}: port must be an integer, got {other:?}")),
        },
    };
    let use_tls = match block.fields().find(|f| f.name() == "use_tls") {
        None => false,
        Some(f) => match f.value().map_err(|e| e.to_string())?.clone() {
            wcl_lang::Value::Bool(b) => b,
            other => return Err(format!("{ctx}: use_tls must be a bool, got {other:?}")),
        },
    };

    Ok(TransportConfig {
        kind,
        host: req_str_field(block, "host", ctx)?,
        port,
        user: req_str_field(block, "user", ctx)?,
        password: str_field(block, "password")?,
        private_key: str_field(block, "private_key")?,
        use_tls,
    })
}

// ------------------------------------------------------------------ save

/// Regenerate `systems.wcl` from the inventory: fresh AST through the
/// canonical printer, written atomically at mode 0600.
pub fn save(path: &Path, systems: &[SystemDef]) -> Result<(), String> {
    let mut src = ast::Source {
        items: Vec::new(),
        trailing_trivia: Vec::new(),
    };
    for sys in systems {
        edit::append_top_level_block(&mut src, system_block(sys));
    }

    let header = [
        " Config Weave systems inventory — managed by weave-server.",
        " GUI edits regenerate this file; hand edits survive a reload but",
        " not the next GUI save. Kept at mode 0600 (inline credentials).",
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

fn str_expr(s: &str) -> ast::Expr {
    edit::string_literal_expr(s)
}

fn system_block(sys: &SystemDef) -> ast::Block {
    let mut fields: Vec<(String, ast::Expr)> = Vec::new();
    if let Some(d) = &sys.description {
        fields.push(("description".into(), str_expr(d)));
    }
    fields.push(("playbook".into(), str_expr(&sys.playbook)));
    fields.push(("play".into(), str_expr(&sys.play)));
    fields.push(("kind".into(), str_expr(sys.kind.as_str())));
    fields.push(("os".into(), str_expr(sys.os.as_str())));
    fields.push(("arch".into(), str_expr(&sys.arch)));

    let mut block = edit::build_block("system", &[], vec![str_expr(&sys.name)], fields);

    let t = &sys.transport;
    let mut tfields: Vec<(String, ast::Expr)> = vec![("host".into(), str_expr(&t.host))];
    if let Some(p) = t.port {
        tfields.push(("port".into(), ast::Expr::I64(i64::from(p))));
    }
    tfields.push(("user".into(), str_expr(&t.user)));
    if let Some(pw) = &t.password {
        tfields.push(("password".into(), str_expr(pw)));
    }
    if let Some(k) = &t.private_key {
        tfields.push(("private_key".into(), str_expr(k)));
    }
    if t.use_tls {
        tfields.push(("use_tls".into(), ast::Expr::Bool(true)));
    }
    let tblock = edit::build_block("transport", &[], vec![str_expr(t.kind.as_str())], tfields);
    block.items.push(ast::Item::Block(tblock));
    block
}

// ------------------------------------------------------------- handlers

fn validate_def(state: &SharedState, def: &SystemDef) -> Result<(), String> {
    if def.name.is_empty() || def.name.len() > 128 {
        return Err("system name must be 1-128 characters".into());
    }
    if runbook_dir(state, &def.playbook).is_none() {
        return Err(format!("no such runbook '{}'", def.playbook));
    }
    if def.play.is_empty() {
        return Err("play must not be empty".into());
    }
    if def.arch.is_empty() {
        return Err("arch must not be empty".into());
    }
    if def.transport.host.is_empty() {
        return Err("transport host must not be empty".into());
    }
    if def.transport.user.is_empty() {
        return Err("transport user must not be empty".into());
    }
    Ok(())
}

/// Persist the inventory, restoring `previous` in memory on failure.
fn persist(
    state: &SharedState,
    systems: &mut Vec<SystemDef>,
    previous: Vec<SystemDef>,
) -> Option<String> {
    match save(&state.systems_path, systems) {
        Ok(()) => None,
        Err(e) => {
            *systems = previous;
            Some(e)
        }
    }
}

/// GET /api/systems
pub async fn list(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    let systems = state.systems.lock().unwrap().clone();
    ok(systems)
}

/// POST /api/systems — body `SystemDef`.
pub async fn create(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
    axum::Json(def): axum::Json<SystemDef>,
) -> Response {
    if let Err(e) = validate_def(&state, &def) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let mut systems = state.systems.lock().unwrap();
    if systems.iter().any(|s| s.name == def.name) {
        return err(
            StatusCode::CONFLICT,
            "a system with that name already exists",
        );
    }
    let previous = systems.clone();
    systems.push(def.clone());
    if let Some(e) = persist(&state, &mut systems, previous) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    ok(def)
}

/// PUT /api/systems/{name} — body `SystemDef`; rename allowed via body.
pub async fn update(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    axum::Json(def): axum::Json<SystemDef>,
) -> Response {
    if let Err(e) = validate_def(&state, &def) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let mut systems = state.systems.lock().unwrap();
    let Some(idx) = systems.iter().position(|s| s.name == name) else {
        return err(StatusCode::NOT_FOUND, "no such system");
    };
    if def.name != name && systems.iter().any(|s| s.name == def.name) {
        return err(
            StatusCode::CONFLICT,
            "a system with that name already exists",
        );
    }
    let previous = systems.clone();
    systems[idx] = def.clone();
    if let Some(e) = persist(&state, &mut systems, previous) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    ok(def)
}

/// DELETE /api/systems/{name}
pub async fn delete(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    let mut systems = state.systems.lock().unwrap();
    let Some(idx) = systems.iter().position(|s| s.name == name) else {
        return err(StatusCode::NOT_FOUND, "no such system");
    };
    let previous = systems.clone();
    let removed = systems.remove(idx);
    if let Some(e) = persist(&state, &mut systems, previous) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    ok(serde_json::json!({ "deleted": removed.name }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<SystemDef> {
        vec![
            SystemDef {
                name: "web-01".into(),
                description: Some("Primary web host".into()),
                playbook: "baseline".into(),
                play: "web".into(),
                kind: SystemKind::Direct,
                os: TargetOs::Linux,
                arch: "x86_64".into(),
                transport: TransportConfig {
                    kind: TransportKind::Ssh,
                    host: "10.0.0.10".into(),
                    port: Some(2222),
                    user: "admin".into(),
                    password: None,
                    private_key: Some("/home/wil/.ssh/id_ed25519".into()),
                    use_tls: false,
                },
            },
            SystemDef {
                name: "edge-router".into(),
                description: None,
                playbook: "network".into(),
                play: "router".into(),
                kind: SystemKind::Remote,
                os: TargetOs::Linux,
                arch: "x86_64".into(),
                transport: TransportConfig {
                    kind: TransportKind::Ssh,
                    host: "10.0.0.1".into(),
                    port: None,
                    user: "admin".into(),
                    password: Some("hunter2 with spaces \"and quotes\"".into()),
                    private_key: None,
                    use_tls: false,
                },
            },
            SystemDef {
                name: "win-svc".into(),
                description: None,
                playbook: "windows".into(),
                play: "base".into(),
                kind: SystemKind::Direct,
                os: TargetOs::Windows,
                arch: "x86_64".into(),
                transport: TransportConfig {
                    kind: TransportKind::Winrm,
                    host: "192.168.1.50".into(),
                    port: None,
                    user: "Administrator".into(),
                    password: Some("p@ss".into()),
                    private_key: None,
                    use_tls: true,
                },
            },
        ]
    }

    #[test]
    fn save_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("systems.wcl");
        let systems = sample();
        save(&path, &systems).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, systems);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn missing_file_is_empty_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(&tmp.path().join("systems.wcl")).unwrap(), Vec::new());
    }

    #[test]
    fn empty_inventory_writes_a_commented_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("systems.wcl");
        save(&path, &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("systems inventory"));
        assert_eq!(load(&path).unwrap(), Vec::new());
    }

    #[test]
    fn duplicate_names_are_rejected_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("systems.wcl");
        let mut systems = sample();
        systems[1].name = systems[0].name.clone();
        save(&path, &systems).unwrap();
        assert!(load(&path).unwrap_err().contains("duplicate system name"));
    }

    #[test]
    fn effective_ports_follow_transport_defaults() {
        let mut t = sample()[1].transport.clone();
        assert_eq!(t.effective_port(), 22);
        t.kind = TransportKind::Winrm;
        assert_eq!(t.effective_port(), 5985);
        t.use_tls = true;
        assert_eq!(t.effective_port(), 5986);
        t.port = Some(1234);
        assert_eq!(t.effective_port(), 1234);
    }
}
