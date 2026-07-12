//! Forge-callable sync webhooks: POST /api/webhooks/repos/{name} is the
//! only route besides /metrics that skips the JWT — each call
//! authenticates against the repo's `webhook_secret` instead, either as
//! a GitHub/Gitea HMAC signature over the raw body or as a plain token
//! header (GitLab and hand-rolled callers). Unknown repo, unconfigured
//! secret, and failed verification all answer the same 404 so the
//! endpoint reveals nothing about which repos exist.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::Path as UrlPath;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use forge_server::{err, ok};
use hmac::Mac as _;
use serde_json::json;

use crate::repos::{self, SyncOutcome};
use crate::state::SharedState;

/// Constant-time byte comparison for the plain-token headers (the HMAC
/// path gets this from `verify_slice`).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Accepts GitHub/Gitea `X-Hub-Signature-256: sha256=<hex>` (HMAC over
/// the raw body) or a plain secret in `X-Gitlab-Token` / `X-Weave-Token`.
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

/// POST /api/webhooks/repos/{name} — any authenticated push event syncs
/// the repo; the payload itself is ignored. A policy skip (local
/// changes) answers 200 so forges do not retry it.
pub async fn webhook(
    Extension(state): Extension<SharedState>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let def = {
        let repos = state.repos.lock().unwrap();
        repos.iter().find(|r| r.name == name).cloned()
    };
    let authorized = def.as_ref().is_some_and(|d| {
        d.webhook_secret
            .as_deref()
            .is_some_and(|s| verify_webhook(s, &headers, &body))
    });
    if !authorized {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    let def = def.expect("authorized implies the repo exists");
    let result = {
        let _guard = state.repo_git_lock.lock().await;
        repos::sync_repo(&def, &repos::cache_dir(&state, &def.name)).await
    };
    let outcome_label = match &result {
        Ok(SyncOutcome::Synced) => "synced",
        Ok(SyncOutcome::Skipped(_)) => "skipped",
        Err(_) => "failed",
    };
    metrics::counter!(
        crate::monitoring::REPO_SYNC_DISPATCH_TOTAL,
        "repo" => def.name.clone(),
        "trigger" => "webhook",
        "outcome" => outcome_label,
    )
    .increment(1);
    match result {
        Ok(SyncOutcome::Synced) => ok(json!({ "name": def.name, "synced": true })),
        Ok(SyncOutcome::Skipped(msg)) => {
            ok(json!({ "name": def.name, "synced": false, "skipped": msg }))
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
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
    fn tampered_body_or_wrong_secret_is_rejected() {
        let h = headers(&[("x-hub-signature-256", &hmac_header(SECRET, BODY))]);
        assert!(!verify_webhook(SECRET, &h, b"other body"));
        assert!(!verify_webhook("a different secret", &h, BODY));
        let h = headers(&[("x-hub-signature-256", "sha256=nothex")]);
        assert!(!verify_webhook(SECRET, &h, BODY));
    }

    #[test]
    fn plain_token_headers_verify() {
        for header in ["x-gitlab-token", "x-weave-token"] {
            assert!(verify_webhook(SECRET, &headers(&[(header, SECRET)]), BODY));
            assert!(!verify_webhook(
                SECRET,
                &headers(&[(header, "wrong")]),
                BODY
            ));
        }
    }

    #[test]
    fn missing_headers_are_rejected() {
        assert!(!verify_webhook(SECRET, &HeaderMap::new(), BODY));
    }

    #[test]
    fn ct_eq_truth_table() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn hex_decode_edge_cases() {
        assert_eq!(hex_decode("00ff").unwrap(), vec![0x00, 0xff]);
        assert!(hex_decode("0").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
