//! Git-forge webhooks: POST /api/webhooks/pipelines/{name}/{trigger} is the
//! only route without a JWT — each call authenticates against the named
//! trigger's `webhook_secret` instead, either as a GitHub/Gitea HMAC
//! signature over the raw body or as a plain token header (GitLab and
//! hand-rolled callers). Unknown pipeline/trigger, unconfigured secret, and
//! failed verification all answer the same 404, so the endpoint reveals
//! nothing about which pipelines exist.

use std::collections::HashMap;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::Path as UrlPath;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use forge_server::{err, ok};
use hmac::Mac as _;
use serde_json::json;

use crate::state::SharedState;

/// Constant-time byte comparison for the plain-token headers.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Accepts GitHub/Gitea `X-Hub-Signature-256: sha256=<hex>` (HMAC over the
/// raw body) or a plain secret in `X-Gitlab-Token` / `X-Weave-Token`.
fn verify_webhook(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    if let Some(sig) = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("sha256="))
    {
        let Ok(sig) = hex_decode(sig) else {
            return false;
        };
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
            .expect("hmac accepts any key length");
        mac.update(body);
        return mac.verify_slice(&sig).is_ok();
    }
    for header in ["x-gitlab-token", "x-weave-token"] {
        if let Some(token) = headers.get(header).and_then(|v| v.to_str().ok()) {
            return ct_eq(token.as_bytes(), secret.as_bytes());
        }
    }
    false
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

/// POST /api/webhooks/pipelines/{name}/{trigger} — an authenticated push
/// starts a run of the pipeline, with the trigger's `bind` presets as
/// properties. The payload itself is ignored.
pub async fn webhook(
    Extension(state): Extension<SharedState>,
    UrlPath((name, trigger)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Look up the pipeline's named webhook trigger + its secret + bindings.
    let found = {
        let pipelines = state.pipelines.lock().unwrap();
        pipelines.iter().find(|p| p.name == name).and_then(|p| {
            p.triggers
                .iter()
                .find(|t| t.name == trigger && t.r#type == "webhook" && t.enabled)
                .map(|t| (t.webhook_secret.clone(), t.bindings.clone()))
        })
    };
    let authorized = found
        .as_ref()
        .and_then(|(secret, _)| secret.as_deref())
        .is_some_and(|s| verify_webhook(s, &headers, &body));
    if !authorized {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    let (_, bindings) = found.expect("authorized implies the trigger exists");
    let supplied: HashMap<String, String> = bindings.into_iter().collect();

    match crate::trigger::start_run(&state, &name, &supplied, format!("webhook:{trigger}")) {
        Ok(run) => ok(json!({ "run_id": run.id })),
        Err((status, message)) => err(status, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "hunter2hunter2";
    const BODY: &[u8] = br#"{"ref":"refs/heads/main"}"#;

    fn hmac_header(secret: &str, body: &[u8]) -> String {
        let mut mac =
            hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("any key");
        mac.update(body);
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("sha256={hex}")
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                axum::http::HeaderName::try_from(*k).unwrap(),
                v.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn github_hmac_signature_verifies() {
        let h = headers(&[("x-hub-signature-256", &hmac_header(SECRET, BODY))]);
        assert!(verify_webhook(SECRET, &h, BODY));
    }

    #[test]
    fn tampered_or_wrong_secret_rejected() {
        let h = headers(&[("x-hub-signature-256", &hmac_header(SECRET, BODY))]);
        assert!(!verify_webhook(SECRET, &h, b"other body"));
        assert!(!verify_webhook("a different secret", &h, BODY));
    }

    #[test]
    fn plain_token_headers_verify() {
        for header in ["x-gitlab-token", "x-weave-token"] {
            assert!(verify_webhook(SECRET, &headers(&[(header, SECRET)]), BODY));
            assert!(!verify_webhook(SECRET, &headers(&[(header, "wrong")]), BODY));
        }
    }

    #[test]
    fn missing_headers_rejected() {
        assert!(!verify_webhook(SECRET, &HeaderMap::new(), BODY));
    }
}
