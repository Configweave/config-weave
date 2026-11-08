//! The `hash` module (PRD §7): sha256/sha512/md5 over strings and files.
//! `http::download` + `hash::sha256_file` + compare is the canonical
//! verified-fetch pattern.

use md5::Md5;
use sha2::{Digest, Sha256, Sha512};
use wscript::Module;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Digest a file in chunks rather than loading it whole — these hash
/// downloaded archives. `digest` 0.11 dropped the `io::Write` impl on
/// hashers, so this reads and `update`s explicitly instead of
/// `io::copy`ing into one.
pub(crate) fn digest_file<D: Digest>(path: &str) -> Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut hasher = D::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
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

#[cfg(test)]
mod tests {
    // Published vectors: "abc" for SHA-256 and SHA-512 is the NIST FIPS 180-2
    // example (Appendix B.1 and C.1); "abc" and "" for MD5 are the RFC 1321
    // test suite (appendix A.5).
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const SHA512_ABC: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                              2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";
    const MD5_ABC: &str = "900150983cd24fb0d6963f7d28e17f72";
    const MD5_EMPTY: &str = "d41d8cd98f00b204e9800998ecf8427e";

    /// Run a script through the real hash module and return its string.
    fn run_script(src: &str) -> String {
        let ctx = crate::hostapi::context();
        let unit = ctx.compile(src).unwrap_or_else(|e| panic!("compile: {e}"));
        let mut vm = crate::hostapi::vm(&ctx);
        let out: Result<String, String> = vm.call_unit(&unit, "run", (0i64,)).unwrap();
        out.unwrap_or_else(|e| panic!("script error: {e}"))
    }

    #[test]
    fn string_digests_match_published_vectors() {
        let out = run_script(
            r#"
use hash

fn run(x: int) -> Result[string, string] {
    let a = hash::sha256("abc")
    let b = hash::sha512("abc")
    let c = hash::md5("abc")
    let d = hash::md5("")
    Ok("{a} {b} {c} {d}")
}
"#,
        );
        assert_eq!(
            out,
            format!("{SHA256_ABC} {SHA512_ABC} {MD5_ABC} {MD5_EMPTY}")
        );
    }

    #[test]
    fn file_digests_match_published_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc.txt");
        std::fs::write(&file, b"abc").unwrap();
        let path = file.display().to_string().replace('\\', "/");
        let src = r#"
use hash

fn run(x: int) -> Result[string, string] {
    let a = hash::sha256_file("PATH")?
    let b = hash::sha512_file("PATH")?
    let c = hash::md5_file("PATH")?
    Ok("{a} {b} {c}")
}
"#
        .replace("PATH", &path);
        let out = run_script(&src);
        assert_eq!(out, format!("{SHA256_ABC} {SHA512_ABC} {MD5_ABC}"));
    }

    #[test]
    fn file_digests_fail_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("missing.bin");
        let missing = file.display().to_string().replace('\\', "/");
        let src = r#"
use hash

fn run(x: int) -> Result[string, string] {
    let a = match hash::sha256_file("PATH") { Ok(_) => "ok", Err(_) => "err" }
    let b = match hash::sha512_file("PATH") { Ok(_) => "ok", Err(_) => "err" }
    let c = match hash::md5_file("PATH") { Ok(_) => "ok", Err(_) => "err" }
    Ok("{a} {b} {c}")
}
"#
        .replace("PATH", &missing);
        assert_eq!(run_script(&src), "err err err");
    }
}
