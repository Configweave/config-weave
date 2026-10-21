//! The `http` module (PRD §7): get, post, download. rustls everywhere —
//! no OpenSSL linkage, keeping the musl-static build clean.

use std::collections::HashMap;
use std::time::Duration;

use wscript::{Module, Script};
use wscript_std::DynValue;

/// An HTTP response as scripts see it.
#[derive(Script, Debug, Clone)]
pub struct HttpResponse {
    pub status: i64,
    pub body: String,
    pub headers: HashMap<String, String>,
}

#[derive(Default)]
struct Opts {
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
    follow_redirects: bool,
    /// `download` only: extra attempts after the first.
    retries: u32,
    /// `download` only: the expected byte count of the finished file.
    size: Option<u64>,
    /// `download` only: the expected sha256 of the finished file, lowercase hex.
    sha256: Option<String>,
}

const REQUEST_KEYS: &str = "headers, timeout, redirects";
const DOWNLOAD_KEYS: &str = "headers, timeout, redirects, retries, size, sha256";

/// Parse an options map. `download` admits the download-only keys
/// (`retries`, `size`, `sha256`); `get`/`post` reject them as unknown.
fn parse_opts(opts: &DynValue, download: bool) -> Result<Opts, String> {
    let mut out = Opts {
        follow_redirects: true,
        ..Opts::default()
    };
    match opts {
        DynValue::Null => return Ok(out),
        DynValue::Map(m) => {
            for (k, v) in m {
                match (k.as_str(), v) {
                    ("headers", DynValue::Map(hs)) => {
                        for (name, value) in hs {
                            match value {
                                DynValue::String(s) => out.headers.push((name.clone(), s.clone())),
                                other => {
                                    return Err(format!(
                                        "header '{name}' must be a string, got {other:?}"
                                    ));
                                }
                            }
                        }
                    }
                    ("timeout", DynValue::Int(secs)) => {
                        out.timeout = Some(Duration::from_secs(*secs as u64));
                    }
                    ("timeout", DynValue::Float(secs)) => {
                        out.timeout = Some(Duration::from_secs_f64(*secs));
                    }
                    ("redirects", DynValue::Bool(b)) => out.follow_redirects = *b,
                    ("retries", v) if download => {
                        out.retries = match v {
                            DynValue::Int(n) => u32::try_from(*n).ok(),
                            _ => None,
                        }
                        .ok_or_else(|| format!("retries must be a non-negative int, got {v:?}"))?;
                    }
                    ("size", v) if download => {
                        out.size = Some(
                            match v {
                                DynValue::Int(n) => u64::try_from(*n).ok(),
                                _ => None,
                            }
                            .ok_or_else(|| format!("size must be a non-negative int, got {v:?}"))?,
                        );
                    }
                    ("sha256", v) if download => match v {
                        DynValue::String(s)
                            if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) =>
                        {
                            out.sha256 = Some(s.to_ascii_lowercase());
                        }
                        other => {
                            return Err(format!(
                                "sha256 must be a 64-character hex string, got {other:?}"
                            ));
                        }
                    },
                    (other, _) => {
                        let keys = if download {
                            DOWNLOAD_KEYS
                        } else {
                            REQUEST_KEYS
                        };
                        return Err(format!("unknown http option '{other}' (expected {keys})"));
                    }
                }
            }
        }
        other => return Err(format!("http options must be a map or null, got {other:?}")),
    }
    Ok(out)
}

/// The agent for `get`/`post`, or for `download`. A download agent asks
/// for the identity encoding (so `Content-Length` and byte ranges describe
/// the bytes written) and hands every status back as a response, so the
/// retry policy can tell 5xx from 4xx.
fn agent(opts: &Opts, download: bool) -> ureq::Agent {
    let mut config =
        ureq::Agent::config_builder().max_redirects(if opts.follow_redirects { 10 } else { 0 });
    if let Some(t) = opts.timeout {
        config = config.timeout_global(Some(t));
    }
    if download {
        config = config
            .http_status_as_error(false)
            .accept_encoding("identity");
    }
    config.build().into()
}

fn to_response(res: ureq::http::Response<ureq::Body>) -> Result<HttpResponse, String> {
    let status = res.status().as_u16() as i64;
    let mut headers = HashMap::new();
    for (name, value) in res.headers() {
        // Header values are ASCII in practice; a non-UTF-8 value maps to "".
        headers.insert(
            name.as_str().to_string(),
            value.to_str().unwrap_or_default().to_string(),
        );
    }
    let body = res
        .into_body()
        .read_to_string()
        .map_err(|e| format!("reading response body: {e}"))?;
    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

fn apply_headers<B>(mut req: ureq::RequestBuilder<B>, opts: &Opts) -> ureq::RequestBuilder<B> {
    for (name, value) in &opts.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    req
}

/// Delay before the first retry; doubled for each later one, capped at
/// [`RETRY_CAP`].
const RETRY_BASE: Duration = Duration::from_secs(1);
const RETRY_CAP: Duration = Duration::from_secs(30);

/// The pause before retry number `retry` (1-based): base, 2×base, 4×base…
fn backoff(base: Duration, retry: u32) -> Duration {
    base.saturating_mul(1u32 << retry.saturating_sub(1).min(16))
        .min(RETRY_CAP)
}

/// Why one download attempt failed, and whether another attempt may help.
struct Fail {
    msg: String,
    retry: bool,
}

impl Fail {
    fn retry(msg: impl Into<String>) -> Self {
        Fail {
            msg: msg.into(),
            retry: true,
        }
    }

    fn fatal(msg: impl Into<String>) -> Self {
        Fail {
            msg: msg.into(),
            retry: false,
        }
    }
}

/// What the bytes in `<dest>.part` are, carried from one attempt to the next.
#[derive(Default)]
struct Partial {
    /// Bytes in `.part` a `Range` request may continue from; 0 = start over.
    have: u64,
    /// The last full response sent `Accept-Ranges: bytes` and a
    /// `Content-Length` (so the body was not transparently decoded).
    ranges: bool,
    /// ETag, else Last-Modified, of that response — sent as `If-Range`.
    validator: Option<String>,
    /// The length the assembled file must reach, when the server said.
    expected: Option<u64>,
}

/// Classify an error from sending a request: transport trouble is
/// retried, a malformed URL or a redirect loop is not.
fn call_failure(e: ureq::Error) -> Fail {
    let retry = matches!(
        e,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Protocol(_)
            | ureq::Error::BodyStalled
    );
    Fail {
        msg: e.to_string(),
        retry,
    }
}

/// Parse `Content-Range: bytes <first>-<last>/<total|*>` into
/// `(first, total)`.
fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (first, _) = range.split_once('-')?;
    let first = first.trim().parse().ok()?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    Some((first, total))
}

/// One GET, streamed into `part`. Continues the bytes already there with
/// a `Range` request when the previous response allowed it.
fn attempt(
    agent: &ureq::Agent,
    url: &str,
    opts: &Opts,
    part: &str,
    st: &mut Partial,
) -> Result<u64, Fail> {
    use std::io::{Read, Seek, Write};

    let resume_from = if st.ranges { st.have } else { 0 };
    let mut req = apply_headers(agent.get(url), opts);
    if resume_from > 0 {
        req = req.header("Range", format!("bytes={resume_from}-"));
        if let Some(v) = &st.validator {
            req = req.header("If-Range", v.as_str());
        }
    }
    let res = req.call().map_err(call_failure)?;
    let status = res.status();
    let header = |name: &str| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
    };
    let content_length: Option<u64> = header("content-length").and_then(|v| v.parse().ok());

    let start = match status.as_u16() {
        206 if resume_from > 0 => {
            let range = header("content-range").unwrap_or_default();
            match parse_content_range(&range) {
                Some((first, total)) if first == resume_from => {
                    st.expected = total.or(content_length.map(|n| resume_from + n));
                    resume_from
                }
                _ => {
                    st.have = 0;
                    return Err(Fail::retry(format!(
                        "asked for bytes {resume_from}- and got Content-Range '{range}'"
                    )));
                }
            }
        }
        206 => {
            return Err(Fail::fatal(
                "server returned 206 Partial Content to a request without Range",
            ));
        }
        200..=299 => {
            st.expected = content_length;
            st.ranges = content_length.is_some()
                && header("accept-ranges").is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
            st.validator = header("etag").or_else(|| header("last-modified"));
            0
        }
        429 | 500..=599 => return Err(Fail::retry(format!("server returned {status}"))),
        _ => return Err(Fail::fatal(format!("server returned {status}"))),
    };

    let io_fail = |e: std::io::Error| Fail::fatal(format!("cannot write {part}: {e}"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(part)
        .map_err(|e| Fail::fatal(format!("cannot create {part}: {e}")))?;
    file.set_len(start).map_err(io_fail)?;
    file.seek(std::io::SeekFrom::Start(start))
        .map_err(io_fail)?;

    let of = |e: Option<u64>| e.map(|e| format!(" of {e}")).unwrap_or_default();
    let mut reader = res.into_body().into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = start;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                st.have = total;
                return Err(Fail::retry(format!(
                    "body ended at {total}{} bytes ({e})",
                    of(st.expected)
                )));
            }
        };
        file.write_all(&buf[..n]).map_err(io_fail)?;
        total += n as u64;
    }
    file.sync_all().map_err(io_fail)?;

    if let Some(expected) = st.expected {
        if total < expected {
            st.have = total;
            return Err(Fail::retry(format!(
                "body ended at {total} of {expected} bytes"
            )));
        }
        if total > expected {
            return Err(Fail::fatal(format!(
                "body is {total} bytes, more than the {expected} the server declared"
            )));
        }
    }
    if let Some(size) = opts.size {
        // The server considers this body complete, so there is nothing to
        // resume: a retry starts over.
        st.have = 0;
        if total < size {
            return Err(Fail::retry(format!(
                "body is {total} bytes, expected size {size}"
            )));
        }
        if total > size {
            return Err(Fail::fatal(format!(
                "body is {total} bytes, expected size {size}"
            )));
        }
    }
    Ok(total)
}

/// `http::download`: fetch into `<dest>.part`, check, then rename onto
/// `dest`. `dest` is only ever replaced by a complete, verified file.
fn download_file(url: &str, dest: &str, opts: &Opts, base_delay: Duration) -> Result<u64, String> {
    let part = format!("{dest}.part");
    // A `.part` from an earlier, interrupted call is never stitched onto.
    match std::fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "download {url} failed: cannot remove stale {part}: {e}"
            ));
        }
    }

    let agent = agent(opts, true);
    let attempts = opts.retries.saturating_add(1);
    let mut st = Partial::default();
    let mut made = 0;
    let outcome = loop {
        made += 1;
        match attempt(&agent, url, opts, &part, &mut st) {
            Ok(total) => break Ok(total),
            Err(f) if f.retry && made < attempts => {
                std::thread::sleep(backoff(base_delay, made));
            }
            Err(f) => break Err(f.msg),
        }
    };
    let outcome = outcome
        .and_then(|total| {
            if let Some(want) = &opts.sha256 {
                let got = super::hash::digest_file::<sha2::Sha256>(&part)?;
                if &got != want {
                    return Err(format!("sha256 mismatch: expected {want}, got {got}"));
                }
            }
            Ok(total)
        })
        .and_then(|total| {
            std::fs::rename(&part, dest)
                .map(|()| total)
                .map_err(|e| format!("cannot rename {part} to {dest}: {e}"))
        });

    outcome.map_err(|msg| {
        let _ = std::fs::remove_file(&part);
        if opts.retries > 0 {
            let s = if made == 1 { "" } else { "s" };
            format!("download {url} failed after {made} attempt{s}: {msg}")
        } else {
            format!("download {url} failed: {msg}")
        }
    })
}

pub fn module() -> Module {
    let mut m = Module::new("http");
    m.doc("HTTP client (capability: network access). rustls, no system TLS");

    m.doc_next("GET a URL. opts: headers (map), timeout (secs), redirects (bool)");
    m.fn_(
        "get",
        |url: &str, opts: DynValue| -> Result<HttpResponse, String> {
            let opts = parse_opts(&opts, false)?;
            let req = apply_headers(agent(&opts, false).get(url), &opts);
            let res = req.call().map_err(|e| e.to_string())?;
            to_response(res)
        },
    );
    m.doc_next("POST a body to a URL. opts as for get");
    m.fn_(
        "post",
        |url: &str, body: &str, opts: DynValue| -> Result<HttpResponse, String> {
            let opts = parse_opts(&opts, false)?;
            let req = apply_headers(agent(&opts, false).post(url), &opts);
            let res = req.send(body).map_err(|e| e.to_string())?;
            to_response(res)
        },
    );
    m.doc_next(
        "Download a URL to a file; returns the byte count. Writes <dest>.part and renames it \
         onto dest only when complete, so a failure never leaves a partial dest. Any non-2xx \
         status is an Err. opts as for get, plus (download only; get/post reject them): \
         retries (int, default 0; extra attempts after transport errors, 5xx, 429 or a short \
         body, backoff 1s doubling to 30s, resuming with Range when the server allows), \
         size (int, expected byte count), sha256 (hex, expected digest; a mismatch is not retried)",
    );
    m.fn_(
        "download",
        |url: &str, dest: &str, opts: DynValue| -> Result<i64, String> {
            let opts = parse_opts(&opts, true)?;
            download_file(url, dest, &opts, RETRY_BASE).map(|n| n as i64)
        },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Shutdown, TcpListener};
    use std::sync::{Arc, Mutex};

    /// A one-request-per-connection HTTP server on a loopback port. The
    /// handler gets the request index and head and returns the raw bytes
    /// to send; the connection is closed after them, so a response that
    /// declares more than it sends is a truncated body.
    struct Server {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn serve(handler: impl Fn(usize, &str) -> Vec<u8> + Send + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let end = line == "\r\n";
                    head.push_str(&line);
                    if end {
                        break;
                    }
                }
                let n = {
                    let mut l = log.lock().unwrap();
                    l.push(head.clone());
                    l.len() - 1
                };
                let _ = stream.write_all(&handler(n, &head));
                let _ = stream.flush();
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
        Server {
            url: format!("http://{addr}/file.bin"),
            requests,
        }
    }

    /// A response declaring `declared` bytes of body but sending `body`.
    fn raw(status: &str, headers: &[(&str, &str)], declared: usize, body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for (k, v) in headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("Content-Length: {declared}\r\n\r\n"));
        let mut out = out.into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn full(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        raw(status, headers, body.len(), body)
    }

    fn header_of(head: &str, name: &str) -> Option<String> {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }

    fn content() -> Vec<u8> {
        (0..10_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn opts() -> Opts {
        Opts {
            follow_redirects: true,
            ..Opts::default()
        }
    }

    fn fetch(server: &Server, dest: &std::path::Path, opts: &Opts) -> Result<u64, String> {
        download_file(
            &server.url,
            dest.to_str().unwrap(),
            opts,
            Duration::from_millis(1),
        )
    }

    fn part_of(dest: &std::path::Path) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("{}.part", dest.display()))
    }

    fn sha256_hex(data: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn download_success_writes_dest_and_no_part() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| full("200 OK", &[], &b));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        assert_eq!(fetch(&server, &dest, &opts()).unwrap(), body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_of(&dest).exists());
        let req = &server.requests()[0];
        assert_eq!(
            header_of(req, "accept-encoding").as_deref(),
            Some("identity")
        );
    }

    #[test]
    fn download_truncated_body_fails_without_dest_or_part() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| raw("200 OK", &[], b.len(), &b[..4000]));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let err = fetch(&server, &dest, &opts()).unwrap_err();
        assert!(err.contains(&server.url), "{err}");
        assert!(err.contains("body ended at 4000 of 10000 bytes"), "{err}");
        assert!(!dest.exists());
        assert!(!part_of(&dest).exists());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn download_failure_leaves_existing_dest_unchanged() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| raw("200 OK", &[], b.len(), &b[..4000]));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        std::fs::write(&dest, b"the previous good file").unwrap();
        assert!(fetch(&server, &dest, &opts()).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"the previous good file");
        assert!(!part_of(&dest).exists());
    }

    #[test]
    fn download_discards_stale_part() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| full("200 OK", &[("Accept-Ranges", "bytes")], &b));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        std::fs::write(part_of(&dest), b"stale bytes from an earlier call").unwrap();
        fetch(&server, &dest, &opts()).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_of(&dest).exists());
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(header_of(&reqs[0], "range"), None);
    }

    #[test]
    fn download_retries_after_truncated_body() {
        let body = content();
        let b = body.clone();
        let server = serve(move |n, _| {
            if n == 0 {
                raw("200 OK", &[], b.len(), &b[..4000])
            } else {
                full("200 OK", &[], &b)
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 2,
            ..opts()
        };
        assert_eq!(fetch(&server, &dest, &o).unwrap(), body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        // No Accept-Ranges on the first response, so no resume.
        assert_eq!(header_of(&reqs[1], "range"), None);
    }

    #[test]
    fn download_resumes_with_range_after_truncation() {
        let body = content();
        let b = body.clone();
        let server = serve(move |n, _| {
            if n == 0 {
                let h = [("Accept-Ranges", "bytes"), ("ETag", "\"v1\"")];
                raw("200 OK", &h, b.len(), &b[..4000])
            } else {
                let range = format!("bytes 4000-{}/{}", b.len() - 1, b.len());
                full(
                    "206 Partial Content",
                    &[("Content-Range", &range)],
                    &b[4000..],
                )
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 1,
            ..opts()
        };
        assert_eq!(fetch(&server, &dest, &o).unwrap(), body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_of(&dest).exists());
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(header_of(&reqs[1], "range").as_deref(), Some("bytes=4000-"));
        assert_eq!(header_of(&reqs[1], "if-range").as_deref(), Some("\"v1\""));
    }

    #[test]
    fn download_restarts_when_server_ignores_range() {
        let body = content();
        let b = body.clone();
        let server = serve(move |n, _| {
            let h = [("Accept-Ranges", "bytes")];
            if n == 0 {
                raw("200 OK", &h, b.len(), &b[..4000])
            } else {
                full("200 OK", &h, &b)
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 1,
            ..opts()
        };
        assert_eq!(fetch(&server, &dest, &o).unwrap(), body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(
            header_of(&server.requests()[1], "range").as_deref(),
            Some("bytes=4000-")
        );
    }

    #[test]
    fn download_rejects_mismatched_content_range() {
        let body = content();
        let b = body.clone();
        let server = serve(move |n, _| {
            let h = [("Accept-Ranges", "bytes")];
            match n {
                0 => raw("200 OK", &h, b.len(), &b[..4000]),
                // Wrong start offset: must not be stitched on.
                1 => full(
                    "206 Partial Content",
                    &[("Content-Range", "bytes 3000-9999/10000")],
                    &b[3000..],
                ),
                _ => full("200 OK", &h, &b),
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 2,
            ..opts()
        };
        fetch(&server, &dest, &o).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let reqs = server.requests();
        assert_eq!(reqs.len(), 3);
        assert_eq!(header_of(&reqs[2], "range"), None);
    }

    #[test]
    fn download_does_not_retry_404() {
        let server = serve(|_, _| full("404 Not Found", &[], b"nope"));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 3,
            ..opts()
        };
        let err = fetch(&server, &dest, &o).unwrap_err();
        assert!(err.contains("failed after 1 attempt: "), "{err}");
        assert!(err.contains("404"), "{err}");
        assert_eq!(server.requests().len(), 1);
        assert!(!dest.exists());
        assert!(!part_of(&dest).exists());
    }

    #[test]
    fn download_retries_503() {
        let body = content();
        let b = body.clone();
        let server = serve(move |n, _| {
            if n < 2 {
                full("503 Service Unavailable", &[], b"busy")
            } else {
                full("200 OK", &[], &b)
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 3,
            ..opts()
        };
        fetch(&server, &dest, &o).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn download_reports_attempt_count_when_retries_run_out() {
        let server = serve(|_, _| full("503 Service Unavailable", &[], b"busy"));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 2,
            ..opts()
        };
        let err = fetch(&server, &dest, &o).unwrap_err();
        assert!(
            err.starts_with(&format!(
                "download {} failed after 3 attempts: ",
                server.url
            )),
            "{err}"
        );
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn download_size_mismatch_fails() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| full("200 OK", &[], &b));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            size: Some(12_345),
            ..opts()
        };
        let err = fetch(&server, &dest, &o).unwrap_err();
        assert!(err.contains("expected size 12345"), "{err}");
        assert!(!dest.exists());
        assert!(!part_of(&dest).exists());
    }

    #[test]
    fn download_sha256_mismatch_fails_without_retry() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| full("200 OK", &[], &b));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let o = Opts {
            retries: 3,
            sha256: Some("0".repeat(64)),
            ..opts()
        };
        let err = fetch(&server, &dest, &o).unwrap_err();
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert_eq!(server.requests().len(), 1);
        assert!(!dest.exists());
        assert!(!part_of(&dest).exists());
    }

    #[test]
    fn download_sha256_and_size_match() {
        let body = content();
        let b = body.clone();
        let server = serve(move |_, _| full("200 OK", &[], &b));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        // Upper case in the option is accepted.
        let mut map = HashMap::new();
        map.insert(
            "sha256".to_string(),
            DynValue::String(sha256_hex(&body).to_ascii_uppercase()),
        );
        map.insert("size".to_string(), DynValue::Int(body.len() as i64));
        let o = parse_opts(&DynValue::Map(map), true).unwrap();
        fetch(&server, &dest, &o).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[test]
    fn unknown_option_still_errors() {
        let map = |k: &str, v: DynValue| DynValue::Map([(k.to_string(), v)].into_iter().collect());
        let err = parse_opts(&map("bogus", DynValue::Int(1)), true)
            .err()
            .unwrap();
        assert_eq!(
            err,
            "unknown http option 'bogus' (expected headers, timeout, redirects, retries, size, sha256)"
        );
        // The download-only keys are unknown to get/post.
        let err = parse_opts(&map("retries", DynValue::Int(1)), false)
            .err()
            .unwrap();
        assert_eq!(
            err,
            "unknown http option 'retries' (expected headers, timeout, redirects)"
        );
        assert!(parse_opts(&map("retries", DynValue::Int(-1)), true).is_err());
        assert!(parse_opts(&map("sha256", DynValue::String("abc".into())), true).is_err());
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let s = Duration::from_secs;
        assert_eq!(backoff(s(1), 1), s(1));
        assert_eq!(backoff(s(1), 2), s(2));
        assert_eq!(backoff(s(1), 3), s(4));
        assert_eq!(backoff(s(1), 6), s(30));
        assert_eq!(backoff(s(1), 1000), s(30));
    }
}
