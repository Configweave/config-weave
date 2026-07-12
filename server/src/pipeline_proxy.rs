//! Reverse proxy to a config-weave-pipeline daemon. The browser is
//! authenticated to weave-server (HS256 forge token), not to the daemon
//! (RS256 forge-auth), so weave-server holds a forge-auth machine token and
//! attaches it to every forwarded call — the same "the browser never talks
//! to the backend directly" posture as the Prometheus/Loki proxy.
//!
//! forge-auth has no `client_credentials` grant, so the machine token is
//! either supplied statically (`--pipeline-token`, an out-of-band service
//! token) or refreshed via the `refresh_token` grant
//! (`--pipeline-refresh-token` + token URL + client id), which forge-auth
//! does support. A refresh config takes precedence; the token is cached
//! until shortly before expiry.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::Path as UrlPath;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::Response;
use forge_server::{RequireClaims, err, ok};
use serde_json::json;

use crate::state::SharedState;

/// The daemon URL + machine-token acquisition config.
#[derive(Default)]
pub struct PipelineProxy {
    pub url: Option<url::Url>,
    /// A static forge-auth machine token forwarded as-is (no refresh).
    pub static_token: Option<String>,
    /// Auto-refresh via the forge-auth `refresh_token` grant.
    pub refresh: Option<RefreshConfig>,
    cached: Mutex<Option<CachedToken>>,
}

pub struct RefreshConfig {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub refresh_token: String,
}

struct CachedToken {
    token: String,
    expires_at: Instant,
}

impl PipelineProxy {
    pub fn new(
        url: Option<url::Url>,
        static_token: Option<String>,
        refresh: Option<RefreshConfig>,
    ) -> Self {
        Self {
            url,
            static_token,
            refresh,
            cached: Mutex::new(None),
        }
    }
}

fn base_url(state: &SharedState) -> Result<url::Url, (StatusCode, String)> {
    state.pipeline.url.clone().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "pipeline daemon not configured (--pipeline-url / PIPELINE_URL)".into(),
    ))
}

/// The bearer token to attach to a forwarded call: a live refreshed token
/// (cached), the static token, or `None` (daemon in no-auth/dev mode).
async fn machine_token(state: &SharedState) -> Result<Option<String>, (StatusCode, String)> {
    if let Some(refresh) = &state.pipeline.refresh {
        {
            let cached = state.pipeline.cached.lock().unwrap();
            if let Some(c) = &*cached
                && c.expires_at > Instant::now() + Duration::from_secs(30)
            {
                return Ok(Some(c.token.clone()));
            }
        }
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.refresh_token.as_str()),
            ("client_id", refresh.client_id.as_str()),
        ];
        if let Some(secret) = &refresh.client_secret {
            form.push(("client_secret", secret.as_str()));
        }
        let resp = state
            .http
            .post(&refresh.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("pipeline token refresh: {e}")))?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let Some(token) = body["access_token"].as_str() else {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("pipeline token refresh failed ({status})"),
            ));
        };
        let ttl = body["expires_in"].as_u64().unwrap_or(300);
        let token = token.to_string();
        *state.pipeline.cached.lock().unwrap() = Some(CachedToken {
            token: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(ttl),
        });
        return Ok(Some(token));
    }
    Ok(state.pipeline.static_token.clone())
}

/// GET /api/pipeline-config — the UI capability probe: is a daemon wired up?
pub async fn config(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    ok(json!({ "configured": state.pipeline.url.is_some() }))
}

/// Forward `/api/pipeline/{*rest}` to `{pipeline_url}/api/{rest}` with the
/// machine token attached. Body and status pass through verbatim (the
/// daemon already speaks the forge envelope).
pub async fn proxy(
    Extension(state): Extension<SharedState>,
    _claims: RequireClaims,
    method: Method,
    UrlPath(rest): UrlPath<String>,
    uri: Uri,
    body: Bytes,
) -> Response {
    let base = match base_url(&state) {
        Ok(u) => u,
        Err((s, e)) => return err(s, e),
    };
    let token = match machine_token(&state).await {
        Ok(t) => t,
        Err((s, e)) => return err(s, e),
    };

    let mut url = format!("{}/api/{rest}", base.as_str().trim_end_matches('/'));
    if let Some(q) = uri.query() {
        url.push('?');
        url.push_str(q);
    }

    let mut req = state.http.request(method, &url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if !body.is_empty() {
        req = req.header(header::CONTENT_TYPE, "application/json").body(body);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("pipeline daemon: {e}")),
    };
    let status = resp.status();
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("pipeline daemon: {e}")),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ctype)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "proxy build failed"))
}
