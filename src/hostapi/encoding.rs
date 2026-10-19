//! The `encoding` module: base64 and text-to-bytes conversions. Bytes are
//! `List[int]`, the representation `fs::read_bytes`/`write_bytes` use, so
//! the two compose. The motivating case is the Windows unattend hidden
//! password, `base64(UTF-16LE(password + suffix))`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use wscript::Module;

/// Narrow script ints to bytes, rejecting anything outside 0..=255 rather
/// than truncating it into a different byte.
fn to_bytes(values: &[i64]) -> Result<Vec<u8>, String> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| u8::try_from(*v).map_err(|_| format!("byte {i} is {v}, outside 0..=255")))
        .collect()
}

fn from_bytes(bytes: &[u8]) -> Vec<i64> {
    bytes.iter().map(|b| *b as i64).collect()
}

fn base64_encode(values: &[i64]) -> Result<String, String> {
    Ok(B64.encode(to_bytes(values)?))
}

fn base64_decode(s: &str) -> Result<Vec<i64>, String> {
    B64.decode(s.trim())
        .map(|b| from_bytes(&b))
        .map_err(|e| format!("invalid base64: {e}"))
}

fn utf16le(s: &str) -> Vec<i64> {
    s.encode_utf16()
        .flat_map(u16::to_le_bytes)
        .map(|b| b as i64)
        .collect()
}

fn from_utf8(values: &[i64]) -> Result<String, String> {
    String::from_utf8(to_bytes(values)?).map_err(|e| format!("invalid UTF-8: {e}"))
}

/// Strict like `from_utf8`: an unpaired surrogate is an error, not U+FFFD.
fn from_utf16le(values: &[i64]) -> Result<String, String> {
    let bytes = to_bytes(values)?;
    let bytes = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(&bytes);
    if bytes.len() % 2 != 0 {
        return Err(format!("invalid UTF-16LE: odd byte length {}", bytes.len()));
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16(&units).map_err(|e| format!("invalid UTF-16LE: {e}"))
}

pub fn module() -> Module {
    let mut m = Module::new("encoding");
    m.doc("Base64 and text encodings. Bytes are List[int], as in fs::read_bytes");

    m.doc_next("Base64-encode bytes (standard alphabet, padded)");
    m.fn_(
        "base64_encode",
        |bytes: Vec<i64>| -> Result<String, String> { base64_encode(&bytes) },
    );
    m.doc_next("Decode a base64 string (standard alphabet, padded) to bytes");
    m.fn_("base64_decode", |s: &str| -> Result<Vec<i64>, String> {
        base64_decode(s)
    });
    m.doc_next("A string's UTF-8 bytes");
    m.fn_("utf8", |s: &str| -> Vec<i64> { from_bytes(s.as_bytes()) });
    m.doc_next("Decode UTF-8 bytes to a string");
    m.fn_("from_utf8", |bytes: Vec<i64>| -> Result<String, String> {
        from_utf8(&bytes)
    });
    m.doc_next("A string's UTF-16LE bytes, no BOM");
    m.fn_("utf16le", |s: &str| -> Vec<i64> { utf16le(s) });
    m.doc_next("Decode UTF-16LE bytes to a string, stripping a leading BOM");
    m.fn_(
        "from_utf16le",
        |bytes: Vec<i64>| -> Result<String, String> { from_utf16le(&bytes) },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unattend_hidden_password() {
        // Windows unattend: base64(UTF-16LE(password + "Password")).
        // Expected value from Python:
        // base64.b64encode("Passw0rdPassword".encode("utf-16-le"))
        let password = "Passw0rd";
        let encoded = base64_encode(&utf16le(&format!("{password}Password"))).unwrap();
        assert_eq!(encoded, "UABhAHMAcwB3ADAAcgBkAFAAYQBzAHMAdwBvAHIAZAA=");
    }

    #[test]
    fn round_trips_bytes_and_text() {
        let bytes = from_bytes("héllo".as_bytes());
        let decoded = base64_decode(&base64_encode(&bytes).unwrap()).unwrap();
        assert_eq!(decoded, bytes);
        assert_eq!(from_utf8(&decoded).unwrap(), "héllo");
    }

    #[test]
    fn utf16le_encodes_surrogate_pairs() {
        assert_eq!(utf16le("A\u{1F600}"), vec![0x41, 0, 0x3D, 0xD8, 0x00, 0xDE]);
    }

    #[test]
    fn from_utf16le_round_trips_surrogate_pairs() {
        let text = "héllo \u{1F600}";
        assert_eq!(from_utf16le(&utf16le(text)).unwrap(), text);
    }

    #[test]
    fn from_utf16le_strips_a_bom() {
        assert_eq!(from_utf16le(&[0xFF, 0xFE, 0x41, 0]).unwrap(), "A");
    }

    #[test]
    fn from_utf16le_odd_length_errors() {
        let err = from_utf16le(&[0x41, 0, 0x42]).unwrap_err();
        assert_eq!(err, "invalid UTF-16LE: odd byte length 3");
    }

    #[test]
    fn from_utf16le_unpaired_surrogate_errors() {
        // A lone high surrogate (U+D83D) followed by 'A'.
        let err = from_utf16le(&[0x3D, 0xD8, 0x41, 0]).unwrap_err();
        assert!(err.starts_with("invalid UTF-16LE"), "got: {err}");
    }

    #[test]
    fn from_utf16le_out_of_range_byte_errors() {
        let err = from_utf16le(&[0x41, 300]).unwrap_err();
        assert_eq!(err, "byte 1 is 300, outside 0..=255");
    }

    #[test]
    fn out_of_range_byte_errors() {
        let err = base64_encode(&[1, 256]).unwrap_err();
        assert_eq!(err, "byte 1 is 256, outside 0..=255");
    }

    #[test]
    fn invalid_base64_errors() {
        let err = base64_decode("not base64!").unwrap_err();
        assert!(err.starts_with("invalid base64"), "got: {err}");
    }
}
