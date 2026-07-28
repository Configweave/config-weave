//! The `CWENC1` blob format: Argon2id key derivation + XChaCha20-Poly1305.
//!
//! ```text
//! CWENC1.<b64url(salt,16B)>.<b64url(nonce,24B)>.<b64url(ciphertext||tag)>
//! ```
//!
//! base64url is unpadded and `.` is outside its alphabet, so splitting is
//! unambiguous. The version tag pins *both* primitives and their
//! parameters — changing either means a `CWENC2`, never a silent
//! reinterpretation of an existing blob.
//!
//! The AAD is a constant rather than the variable's name: binding a blob
//! to its name would turn "you renamed a variable" into an
//! indistinguishable "wrong password", and an AEAD failure carries no
//! detail to tell the two apart.

use std::collections::HashMap;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroize;

/// Blob prefix, also the version tag.
pub const PREFIX: &str = "CWENC1.";

const AAD: &[u8] = b"config-weave/secret/v1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

/// Argon2id parameters. OWASP's minimum recommendation (19 MiB, t=2,
/// p=1) — deliberately at the light end, because key derivation also runs
/// inside testlab containers.
const M_COST: u32 = 19 * 1024;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

pub type Salt = [u8; SALT_LEN];

/// A derived 32-byte key, wiped on drop.
#[derive(Clone)]
pub struct Key([u8; 32]);

impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Derived keys memoised by salt, so a file whose secrets share one salt
/// pays for Argon2 exactly once.
#[derive(Default)]
pub struct KeyCache {
    keys: HashMap<Salt, Key>,
}

impl KeyCache {
    pub fn new() -> KeyCache {
        KeyCache::default()
    }

    pub fn key(&mut self, password: &str, salt: &Salt) -> Result<Key, String> {
        if let Some(k) = self.keys.get(salt) {
            return Ok(k.clone());
        }
        let key = derive_key(password, salt)?;
        self.keys.insert(*salt, key.clone());
        Ok(key)
    }
}

fn derive_key(password: &str, salt: &Salt) -> Result<Key, String> {
    let params = Params::new(M_COST, T_COST, P_COST, Some(32))
        .map_err(|e| format!("invalid Argon2 parameters: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| format!("key derivation failed: {e}"))?;
    Ok(Key(out))
}

/// A random salt for a fresh encryption pass.
pub fn random_salt() -> Result<Salt, String> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| format!("cannot read system randomness: {e}"))?;
    Ok(salt)
}

/// True when a `secret()` argument is an encrypted blob rather than a
/// plaintext awaiting `config-weave secrets encrypt`.
pub fn is_blob(s: &str) -> bool {
    s.starts_with(PREFIX)
}

/// Encrypt `plaintext` under `key`, emitting a `CWENC1` blob carrying
/// `salt` so decryption can re-derive the key without external state.
pub fn seal(key: &Key, salt: &Salt, plaintext: &str) -> Result<String, String> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|e| format!("cannot read system randomness: {e}"))?;

    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let ct = cipher
        .encrypt(
            &XNonce::from(nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: AAD,
            },
        )
        .map_err(|_| "encryption failed".to_string())?;

    Ok(format!(
        "{PREFIX}{}.{}.{}",
        B64.encode(salt),
        B64.encode(nonce_bytes),
        B64.encode(&ct)
    ))
}

/// The salt a blob was sealed under, so callers can reuse it (one Argon2
/// pass per file) and derive the right key before calling [`open`].
pub fn blob_salt(blob: &str) -> Result<Salt, String> {
    Ok(parse(blob)?.0)
}

/// Decrypt a `CWENC1` blob. An authentication failure is reported as a
/// wrong password: with a constant AAD and a self-carried salt, that is
/// the only cause other than a tampered blob.
pub fn open(key: &Key, blob: &str) -> Result<String, String> {
    let (_salt, nonce, ct) = parse(blob)?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let pt = cipher
        .decrypt(&XNonce::from(nonce), Payload { msg: &ct, aad: AAD })
        .map_err(|_| "wrong password, or the encrypted value has been tampered with".to_string())?;
    String::from_utf8(pt).map_err(|_| "decrypted value is not valid UTF-8".to_string())
}

type Parsed = (Salt, [u8; NONCE_LEN], Vec<u8>);

fn parse(blob: &str) -> Result<Parsed, String> {
    let body = blob
        .strip_prefix(PREFIX)
        .ok_or_else(|| format!("not a {} encrypted value", PREFIX.trim_end_matches('.')))?;
    let parts: Vec<&str> = body.split('.').collect();
    if parts.len() != 3 {
        return Err("malformed encrypted value (expected salt.nonce.ciphertext)".to_string());
    }
    let salt_v = B64
        .decode(parts[0])
        .map_err(|_| "malformed encrypted value (bad salt encoding)".to_string())?;
    let nonce_v = B64
        .decode(parts[1])
        .map_err(|_| "malformed encrypted value (bad nonce encoding)".to_string())?;
    let ct = B64
        .decode(parts[2])
        .map_err(|_| "malformed encrypted value (bad ciphertext encoding)".to_string())?;

    let salt: Salt = salt_v
        .try_into()
        .map_err(|_| format!("malformed encrypted value (salt must be {SALT_LEN} bytes)"))?;
    let nonce: [u8; NONCE_LEN] = nonce_v
        .try_into()
        .map_err(|_| format!("malformed encrypted value (nonce must be {NONCE_LEN} bytes)"))?;
    Ok((salt, nonce, ct))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_for(password: &str, salt: &Salt) -> Key {
        KeyCache::new().key(password, salt).unwrap()
    }

    #[test]
    fn round_trips() {
        let salt = random_salt().unwrap();
        let key = key_for("hunter2", &salt);
        let blob = seal(&key, &salt, "s3kr1t").unwrap();
        assert!(is_blob(&blob));
        assert_eq!(open(&key, &blob).unwrap(), "s3kr1t");
    }

    #[test]
    fn round_trips_multiline_and_unicode() {
        let salt = random_salt().unwrap();
        let key = key_for("pw", &salt);
        let secret = "-----BEGIN KEY-----\nlíné two\ttab\n-----END KEY-----";
        let blob = seal(&key, &salt, secret).unwrap();
        assert!(!blob.contains('\n'));
        assert_eq!(open(&key, &blob).unwrap(), secret);
    }

    #[test]
    fn wrong_password_is_an_error_not_a_panic() {
        let salt = random_salt().unwrap();
        let blob = seal(&key_for("right", &salt), &salt, "value").unwrap();
        let err = open(&key_for("wrong", &salt), &blob).unwrap_err();
        assert!(err.contains("wrong password"), "{err}");
    }

    #[test]
    fn tampering_fails_authentication() {
        let salt = random_salt().unwrap();
        let key = key_for("pw", &salt);
        let blob = seal(&key, &salt, "value").unwrap();
        // Flip the last base64 character of the ciphertext.
        let mut chars: Vec<char> = blob.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert!(open(&key, &tampered).is_err());
    }

    #[test]
    fn nonce_is_fresh_per_seal() {
        let salt = random_salt().unwrap();
        let key = key_for("pw", &salt);
        let a = seal(&key, &salt, "same").unwrap();
        let b = seal(&key, &salt, "same").unwrap();
        assert_ne!(
            a, b,
            "identical plaintexts must not produce identical blobs"
        );
    }

    #[test]
    fn salt_survives_the_round_trip() {
        let salt = random_salt().unwrap();
        let blob = seal(&key_for("pw", &salt), &salt, "v").unwrap();
        assert_eq!(blob_salt(&blob).unwrap(), salt);
    }

    #[test]
    fn cache_returns_the_same_key_for_a_salt() {
        let salt = random_salt().unwrap();
        let mut cache = KeyCache::new();
        let a = cache.key("pw", &salt).unwrap();
        let b = cache.key("pw", &salt).unwrap();
        assert_eq!(a.0, b.0);
        assert_eq!(cache.keys.len(), 1);
    }

    #[test]
    fn malformed_blobs_report_what_is_wrong() {
        let key = key_for("pw", &random_salt().unwrap());
        assert!(open(&key, "plain").unwrap_err().contains("not a CWENC1"));
        assert!(
            open(&key, "CWENC1.aaa")
                .unwrap_err()
                .contains("expected salt.nonce.ciphertext")
        );
        assert!(
            open(&key, "CWENC1.!!.!!.!!")
                .unwrap_err()
                .contains("bad salt encoding")
        );
    }
}
