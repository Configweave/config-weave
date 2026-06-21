//! Integration spike: verifies the WCL/wscript embedding assumptions that the
//! config-weave architecture rests on. These tests pin behaviour we depend
//! on in the dependencies; if one breaks, the engine design needs a rethink.

use wcl_lang::{Document, Environment, Registry, Value, disk_loader};

const WEAVE_PRELUDE: &str = r#"
@block("step")
type Step {
  @inline(0) name: identifier
  description: utf8
  resource: utf8
  @schemaless condition: bool?
  @child("properties") properties: Properties?
}

@schemaless
@block("properties")
type Properties {}

@document
type Playbook {
  @children("step") steps: list<Step>
}
"#;

fn open_with_registry(source: &str, vars: Option<&str>) -> Result<Document, wcl_lang::ParseError> {
    let mut reg = Registry::new();
    reg.register("weave.wcl", WEAVE_PRELUDE.to_string());
    if let Some(v) = vars {
        reg.register("weave/vars.wcl", v.to_string());
    }
    let env = Environment::new();
    Document::open_at_with_loader(
        source,
        "playbook.wcl",
        None,
        &env,
        reg.loader(disk_loader()),
    )
}

const PLAYBOOK: &str = r#"import <weave.wcl>
step install {
  description = "Install something"
  resource = "runtime.dotnet"
  condition = os.family == "linux"
  properties {
    version = "8.0"
    install_dir = app_root
  }
}
"#;

const VARS: &str = r#"
let os = { family: "linux", version: "6.1" }
let app_root = "/opt/myapp"
"#;

/// Structure is readable and schema-valid even though `os` / `app_root`
/// are unresolvable — field evaluation must stay lazy.
#[test]
fn structure_readable_without_vars() {
    let doc = open_with_registry(PLAYBOOK, None).expect("open");
    let step = doc.block("step").expect("step block");
    assert_eq!(step.kind(), "step");
    let desc = step
        .fields()
        .find(|f| f.name() == "description")
        .expect("description field");
    assert_eq!(
        desc.value().unwrap(),
        &Value::Utf8("Install something".into())
    );

    // Schema validation must not be tripped up by the unresolved condition
    // or the schemaless properties block.
    let errors = doc.schema_errors();
    assert!(errors.is_empty(), "unexpected schema errors: {errors:#?}");

    // But forcing the condition without vars must fail.
    let cond = step.fields().find(|f| f.name() == "condition").unwrap();
    assert!(cond.value().is_err(), "condition evaluated without vars?");
}

/// Appending `import <weave/vars.wcl>` at the end of the source binds the
/// generated lets without disturbing any spans in the original text.
#[test]
fn vars_import_binds_lets() {
    let source = format!("{PLAYBOOK}\nimport <weave/vars.wcl>\n");
    let doc = open_with_registry(&source, Some(VARS)).expect("open");
    let step = doc.block("step").expect("step block");

    let cond = step.fields().find(|f| f.name() == "condition").unwrap();
    assert_eq!(cond.value().unwrap(), &Value::Bool(true));

    let props = step.blocks().find(|b| b.kind() == "properties").unwrap();
    let dir = props.fields().find(|f| f.name() == "install_dir").unwrap();
    assert_eq!(dir.value().unwrap(), &Value::Utf8("/opt/myapp".into()));
}

/// Schema violations we depend on for `validate`: unknown fields are
/// flagged by WCL itself. (Missing required fields are NOT flagged by
/// WCL's block check — config-weave's validator enforces those itself
/// from the schema's effective_fields.)
#[test]
fn schema_violations_detected() {
    let bad = r#"import <weave.wcl>
step broken {
  description = "x"
  resource = "a.b"
  bogus_field = 1
}
"#;
    let doc = open_with_registry(bad, None).expect("open");
    let errors = doc.schema_errors();
    assert!(
        errors
            .iter()
            .any(|e| format!("{e:?}").contains("bogus_field")),
        "unknown field not flagged: {errors:#?}"
    );

    // The raw material for the engine-side required check: the schema's
    // declared fields with optionality.
    let schema = doc.block_schema("step").expect("step schema");
    let resource = schema.field("resource").expect("resource field decl");
    assert!(!resource.optional());
    let condition = schema.field("condition").expect("condition field decl");
    assert!(condition.optional());
}

// ---------------------------------------------------------------- wscript

use wscript::{Context, Script, UnitExt, Vm};
use wscript_std::DynValue;

#[derive(Script, Debug, Clone, PartialEq)]
enum CheckResult {
    AlreadyConfigured,
    NotConfigured,
    RebootRequired,
}

fn weave_ctx() -> Context {
    let mut log = wscript::Module::new("log");
    log.fn_("info", |_msg: &str| {});
    Context::new()
        .module(wscript_std::value())
        .module(log)
        .register_type::<CheckResult>()
}

/// A resource script exporting `fn check(params: Value) -> CheckResult`
/// compiles, type-checks against the host context, and runs with a
/// DynValue argument built host-side.
#[test]
fn wscript_check_entrypoint_roundtrip() {
    let ctx = weave_ctx();
    let unit = ctx
        .compile(
            r#"
use value
use log

fn check(params: Value) -> CheckResult {
    log::info("checking")
    if let Some(v) = params.get("version") {
        if let Some(s) = v.as_string() {
            if s == "8.0" { return CheckResult::AlreadyConfigured }
        }
    }
    CheckResult::NotConfigured
}
"#,
        )
        .expect("compile");

    // Signature enforcement via fn_handle.
    let handle = unit
        .fn_handle::<(DynValue,), CheckResult>("check")
        .expect("signature check");

    let mut params = std::collections::HashMap::new();
    params.insert("version".to_string(), DynValue::String("8.0".into()));
    let mut vm = Vm::new(&ctx);
    let result = handle
        .call(&mut vm, (DynValue::Map(params),))
        .expect("call");
    assert_eq!(result, CheckResult::AlreadyConfigured);
}

/// The fallible contract: `fn check(params: Value) -> Result[CheckResult, string]`
/// also works, and an Err comes back to the host as Err.
#[test]
fn wscript_fallible_entrypoint() {
    let ctx = weave_ctx();
    let unit = ctx
        .compile(
            r#"
use value

fn check(params: Value) -> Result[CheckResult, string] {
    if params.is_null() {
        return Err("params were null")
    }
    Ok(CheckResult::NotConfigured)
}
"#,
        )
        .expect("compile");

    let mut vm = Vm::new(&ctx);
    let err: Result<CheckResult, String> = vm
        .call_unit(&unit, "check", (DynValue::Null,))
        .expect("call itself succeeds");
    assert_eq!(err, Err("params were null".to_string()));
}

/// A typo against the host API fails compilation with a diagnostic.
#[test]
fn wscript_typo_fails_compile() {
    let ctx = weave_ctx();
    let err = ctx
        .compile(
            r#"
use log
fn check(params: Value) -> CheckResult {
    log::inof("typo")
    CheckResult::NotConfigured
}
"#,
        )
        .expect_err("should not compile");
    let msg = format!("{err}");
    assert!(msg.contains("error"), "unexpected error rendering: {msg}");
}
