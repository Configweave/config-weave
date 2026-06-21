//! The `hash` module (PRD §7): sha256/sha512/md5 over strings and files.
//! `http::download` + `hash::sha256_file` + compare is the canonical
//! verified-fetch pattern.

use md5::Md5;
use sha2::{Digest, Sha256, Sha512};
use wscript::Module;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest_file<D: Digest + std::io::Write>(path: &str) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut hasher = D::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hex(&hasher.finalize()))
}

pub fn module() -> Module {
    let mut m = Module::new("hash");
    m.doc("Cryptographic digests over strings and files (hex output)");

    m.doc_next("SHA-256 of a string");
    m.fn_("sha256", |s: &str| hex(&Sha256::digest(s.as_bytes())));
    m.doc_next("SHA-256 of a file's contents");
    m.fn_("sha256_file", |path: &str| -> Result<String, String> {
        digest_file::<Sha256>(path)
    });
    m.doc_next("SHA-512 of a string");
    m.fn_("sha512", |s: &str| hex(&Sha512::digest(s.as_bytes())));
    m.doc_next("SHA-512 of a file's contents");
    m.fn_("sha512_file", |path: &str| -> Result<String, String> {
        digest_file::<Sha512>(path)
    });
    m.doc_next("MD5 of a string (legacy interop only)");
    m.fn_("md5", |s: &str| hex(&Md5::digest(s.as_bytes())));
    m.doc_next("MD5 of a file's contents (legacy interop only)");
    m.fn_("md5_file", |path: &str| -> Result<String, String> {
        digest_file::<Md5>(path)
    });
    m
}
