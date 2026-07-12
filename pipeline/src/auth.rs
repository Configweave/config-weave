//! forge-auth token validation: a forge-server [`TokenValidator`] that
//! accepts the RS256 OIDC access tokens forge-auth mints (user logins and
//! machine/exchange tokens alike), validated against the issuer's JWKS.
//!
//! We replicate ~15 lines of RS256 + JWKS validation with `jsonwebtoken`
//! rather than depend on the whole forge-auth IdP crate (which drags
//! sqlx/ldap3/openidconnect). The JWKS is fetched once at startup and
//! refreshed on a background task; `validate` is pure CPU (a key lookup +
//! signature check), so the sync trait method never blocks on the network.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use forge_server::{Claims, ForgeError, TokenValidator};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;

/// The forge-auth access-token claims (copied from forge-auth's
/// `tokens::access::AccessClaims` — plain serde, no behaviour).
#[derive(Debug, Clone, Deserialize)]
struct AccessClaims {
    iss: String,
    sub: String,
    #[allow(dead_code)]
    aud: String,
    #[serde(default)]
    azp: Option<String>,
    exp: i64,
    iat: i64,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
}

pub struct ForgeAuthValidator {
    issuer: String,
    /// When set, incoming tokens must carry this audience; when `None`, the
    /// audience is not checked (forge-auth deliberately leaves `aud` to the
    /// caller).
    audience: Option<String>,
    keys: Arc<RwLock<HashMap<String, DecodingKey>>>,
}

impl ForgeAuthValidator {
    /// Fetch the JWKS once (fail fast if the IdP is unreachable), then spawn
    /// a background refresh every 5 minutes.
    pub async fn connect(
        issuer: String,
        jwks_url: String,
        audience: Option<String>,
        http: reqwest::Client,
    ) -> Result<Self, String> {
        let initial = fetch_jwks(&http, &jwks_url).await?;
        let keys = Arc::new(RwLock::new(initial));

        let refresh_keys = keys.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(300));
            tick.tick().await; // consume the immediate tick
            loop {
                tick.tick().await;
                match fetch_jwks(&http, &jwks_url).await {
                    Ok(k) => *refresh_keys.write().unwrap() = k,
                    Err(e) => tracing::warn!("forge-auth JWKS refresh failed: {e}"),
                }
            }
        });

        Ok(Self {
            issuer,
            audience,
            keys,
        })
    }

    /// Map forge-auth access claims onto forge-server's `Claims`. A user
    /// token carries `preferred_username` (used as `sub`); a machine token
    /// (client_credentials / exchange) does not, so it gets a synthetic
    /// `machine` role and keeps the client id as `sub`.
    fn map_claims(&self, ac: AccessClaims) -> Claims {
        let mut roles = ac.roles;
        let sub = match ac.preferred_username {
            Some(user) => user,
            None => {
                if !roles.iter().any(|r| r == "machine") {
                    roles.push("machine".into());
                }
                // azp names the requesting client on an exchange; fall back
                // to the token subject (the client id) otherwise.
                ac.azp.unwrap_or(ac.sub)
            }
        };
        Claims {
            sub,
            roles,
            iat: ac.iat,
            exp: ac.exp,
            iss: Some(ac.iss),
        }
    }
}

impl TokenValidator for ForgeAuthValidator {
    fn validate(&self, token: &str) -> Result<Claims, ForgeError> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| ForgeError::Unauthorized(format!("invalid token header: {e}")))?;
        if header.alg != Algorithm::RS256 {
            return Err(ForgeError::Unauthorized("token is not RS256".into()));
        }
        let kid = header
            .kid
            .ok_or_else(|| ForgeError::Unauthorized("token has no key id".into()))?;
        let key = {
            let keys = self.keys.read().unwrap();
            keys.get(&kid).cloned()
        };
        // An unknown kid rejects (a freshly rotated key surfaces on the next
        // background refresh; no inline blocking fetch).
        let key = key.ok_or_else(|| ForgeError::Unauthorized("unknown signing key".into()))?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        match &self.audience {
            Some(aud) => validation.set_audience(&[aud]),
            None => validation.validate_aud = false,
        }
        let data = jsonwebtoken::decode::<AccessClaims>(token, &key, &validation)
            .map_err(|e| ForgeError::Unauthorized(format!("invalid token: {e}")))?;
        Ok(self.map_claims(data.claims))
    }
}

/// Fetch and parse an RS256 JWKS into kid → DecodingKey.
async fn fetch_jwks(
    http: &reqwest::Client,
    url: &str,
) -> Result<HashMap<String, DecodingKey>, String> {
    let resp: serde_json::Value = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cannot fetch JWKS from {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("JWKS {url}: {e}"))?
        .json()
        .await
        .map_err(|e| format!("JWKS {url}: bad JSON: {e}"))?;

    let keys = resp
        .get("keys")
        .and_then(|k| k.as_array())
        .ok_or_else(|| format!("JWKS {url}: no `keys` array"))?;

    let mut map = HashMap::new();
    for jwk in keys {
        let alg = jwk.get("alg").and_then(|v| v.as_str()).unwrap_or("RS256");
        if alg != "RS256" {
            continue;
        }
        let (Some(kid), Some(n), Some(e)) = (
            jwk.get("kid").and_then(|v| v.as_str()),
            jwk.get("n").and_then(|v| v.as_str()),
            jwk.get("e").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let key = DecodingKey::from_rsa_components(n, e)
            .map_err(|e| format!("JWKS {url}: bad RSA key {kid}: {e}"))?;
        map.insert(kid.to_string(), key);
    }
    if map.is_empty() {
        return Err(format!("JWKS {url}: no usable RS256 keys"));
    }
    Ok(map)
}
