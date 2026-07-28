//! Registering the `secret()` builtin on a WCL evaluation environment.
//!
//! Two modes, because most commands never hold a password:
//!
//! - **locked** — `model::load` and anything else that only needs the
//!   document's *shape*. `secret(x)` evaluates to an empty `utf8`, which
//!   is enough for `check_params` to type-check a secret as the string it
//!   always is. Whether the value is actually encrypted is decided by the
//!   syntactic scan, not here, so `validate` and `docs` stay password-free.
//! - **unlocked** — `check`/`apply`/`test`, where `secret(x)` decrypts and
//!   registers the plaintext with the redactor on the way out.

use std::sync::{Arc, Mutex};

use wcl_lang::{Environment, Value, from_fn};

use super::crypto::{self, KeyCache};
use super::password::Password;
use super::redact;
use super::scan::FN_NAME;

/// The decryption context for one run: the password plus the per-salt key
/// cache, so a playbook whose secrets share a salt derives its Argon2 key
/// once no matter how many values reference it.
pub struct SecretCtx {
    password: Password,
    cache: Mutex<KeyCache>,
}

impl std::fmt::Debug for SecretCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretCtx { .. }")
    }
}

impl SecretCtx {
    pub fn new(password: Password) -> Arc<SecretCtx> {
        Arc::new(SecretCtx {
            password,
            cache: Mutex::new(KeyCache::new()),
        })
    }

    /// Decrypt one blob and register the plaintext for redaction.
    pub fn decrypt(&self, blob: &str) -> Result<String, String> {
        let salt = crypto::blob_salt(blob)?;
        let key = {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| "secret key cache poisoned".to_string())?;
            cache.key(&self.password, &salt)?
        };
        let plaintext = crypto::open(&key, blob)?;
        redact::register(&plaintext);
        Ok(plaintext)
    }

    /// Encrypt under this context's password, reusing `salt`.
    pub fn encrypt(&self, salt: &crypto::Salt, plaintext: &str) -> Result<String, String> {
        let key = {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| "secret key cache poisoned".to_string())?;
            cache.key(&self.password, salt)?
        };
        crypto::seal(&key, salt, plaintext)
    }
}

/// Add `secret()` to an existing environment. `None` installs the locked
/// placeholder.
pub fn register(env: &mut Environment, ctx: Option<Arc<SecretCtx>>) {
    match ctx {
        Some(ctx) => env.add_builtin(
            FN_NAME,
            from_fn(move |blob: String| -> Result<Value, String> {
                ctx.decrypt(&blob).map(Value::Utf8)
            }),
        ),
        None => env.add_builtin(
            FN_NAME,
            from_fn(move |_blob: String| -> Result<Value, String> {
                Ok(Value::Utf8(String::new()))
            }),
        ),
    };
}

/// An environment holding only the locked `secret()` placeholder.
pub fn locked() -> Environment {
    let mut env = Environment::new();
    register(&mut env, None);
    env
}
