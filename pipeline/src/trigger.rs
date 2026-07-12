//! Triggering: property binding/validation and the shared `start_run`
//! entry used by the manual trigger endpoint, webhooks, and the scheduler.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde::Deserialize;
use serde_json::json;

use crate::pipelines::PipelineDef;
use crate::runs::{PipelineRun, RunContext};
use crate::state::SharedState;

/// Bind supplied property values against a pipeline's declared properties:
/// apply defaults, enforce `required`, and coerce/validate by `type`.
pub fn bind_properties(
    def: &PipelineDef,
    supplied: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for prop in &def.properties {
        let value = supplied
            .get(&prop.name)
            .cloned()
            .or_else(|| prop.default.clone());
        let Some(value) = value else {
            if prop.required {
                return Err(format!("missing required property '{}'", prop.name));
            }
            continue;
        };
        match prop.r#type.as_str() {
            "int" if value.parse::<i64>().is_err() => {
                return Err(format!("property '{}' must be an int, got '{value}'", prop.name));
            }
            "bool" if value != "true" && value != "false" => {
                return Err(format!(
                    "property '{}' must be 'true' or 'false', got '{value}'",
                    prop.name
                ));
            }
            _ => {}
        }
        out.insert(prop.name.clone(), value);
    }
    // Unknown supplied keys are ignored (a pipeline consumes only what it
    // declares), same tolerance as the WCL loader.
    Ok(out)
}

/// Find the pipeline, bind properties, and start a run. `trigger_label`
/// records what started it (a trigger name, "manual", "webhook:<name>").
pub fn start_run(
    state: &SharedState,
    pipeline: &str,
    supplied: &HashMap<String, String>,
    trigger_label: String,
) -> Result<Arc<PipelineRun>, (StatusCode, String)> {
    let def = {
        let pipelines = state.pipelines.lock().unwrap();
        pipelines
            .iter()
            .find(|p| p.name == pipeline)
            .cloned()
            .ok_or((StatusCode::NOT_FOUND, "no such pipeline".to_string()))?
    };
    let props = bind_properties(&def, supplied).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let ctx = RunContext {
        config_weave: state.config_weave.clone(),
        playbooks_dir: state.playbooks_dir.clone(),
        events: state.events.clone(),
    };
    Ok(state.runs.start(def, props, trigger_label, ctx))
}

#[derive(Debug, Default, Deserialize)]
pub struct TriggerBody {
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// POST /api/pipelines/{name}/trigger — start a manual run.
pub async fn trigger(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    _claims: RequireClaims,
    body: Option<axum::Json<TriggerBody>>,
) -> Response {
    let supplied = body.map(|b| b.0.properties).unwrap_or_default();
    match start_run(&state, &name, &supplied, "manual".into()) {
        Ok(run) => ok(json!({ "run_id": run.id })),
        Err((status, message)) => err(status, message),
    }
}

// ------------------------------------------------------- run endpoints

/// GET /api/runs — recent runs (summaries, newest first).
pub async fn list_runs(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    ok(json!({ "runs": state.runs.list() }))
}

/// GET /api/runs/{id} — full snapshot incl. the event buffer.
pub async fn get_run(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match state.runs.get(&id) {
        Some(run) => ok(run.snapshot()),
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

/// POST /api/runs/{id}/cancel — request cancellation.
pub async fn cancel_run(
    Extension(state): Extension<SharedState>,
    UrlPath(id): UrlPath<String>,
    _claims: RequireClaims,
) -> Response {
    match state.runs.get(&id) {
        Some(run) => {
            run.cancel();
            ok(json!({ "cancelling": true }))
        }
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipelines::PropertyDef;

    fn def_with(props: Vec<PropertyDef>) -> PipelineDef {
        PipelineDef {
            name: "p".into(),
            description: None,
            properties: props,
            secrets: vec![],
            targets: vec![],
            triggers: vec![],
            steps: vec![],
        }
    }

    fn prop(name: &str, ty: &str, required: bool, default: Option<&str>) -> PropertyDef {
        PropertyDef {
            name: name.into(),
            description: None,
            r#type: ty.into(),
            required,
            default: default.map(|s| s.into()),
        }
    }

    #[test]
    fn required_missing_is_error_default_applies() {
        let def = def_with(vec![
            prop("version", "string", true, None),
            prop("dry_run", "bool", false, Some("false")),
        ]);
        assert!(bind_properties(&def, &HashMap::new()).is_err());
        let supplied: HashMap<String, String> =
            [("version".to_string(), "1.0".to_string())].into_iter().collect();
        let bound = bind_properties(&def, &supplied).unwrap();
        assert_eq!(bound["version"], "1.0");
        assert_eq!(bound["dry_run"], "false");
    }

    #[test]
    fn type_coercion_validates() {
        let def = def_with(vec![prop("n", "int", true, None)]);
        let bad: HashMap<String, String> = [("n".to_string(), "x".to_string())].into_iter().collect();
        assert!(bind_properties(&def, &bad).is_err());
        let good: HashMap<String, String> = [("n".to_string(), "42".to_string())].into_iter().collect();
        assert!(bind_properties(&def, &good).is_ok());
    }
}
