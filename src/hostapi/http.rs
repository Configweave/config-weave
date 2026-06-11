//! The `http` module (PRD §7): get, post, download. rustls everywhere —
//! no OpenSSL linkage, keeping the musl-static build clean.

use std::collections::HashMap;
use std::time::Duration;

use wisp::{Module, Script};
use wisp_std::DynValue;

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
}

fn parse_opts(opts: &DynValue) -> Result<Opts, String> {
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
                    (other, _) => {
                        return Err(format!(
                            "unknown http option '{other}' (expected headers, timeout, redirects)"
                        ));
                    }
                }
            }
        }
        other => return Err(format!("http options must be a map or null, got {other:?}")),
    }
    Ok(out)
}

fn agent(opts: &Opts) -> ureq::Agent {
    let mut config =
        ureq::Agent::config_builder().max_redirects(if opts.follow_redirects { 10 } else { 0 });
    if let Some(t) = opts.timeout {
        config = config.timeout_global(Some(t));
    }
    config.build().into()
}

fn to_response(res: ureq::http::Response<ureq::Body>) -> Result<HttpResponse, String> {
    let status = res.status().as_u16() as i64;
    let mut headers = HashMap::new();
    for (name, value) in res.headers() {
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

pub fn module() -> Module {
    let mut m = Module::new("http");
    m.doc("HTTP client (capability: network access). rustls, no system TLS");

    m.doc_next("GET a URL. opts: headers (map), timeout (secs), redirects (bool)");
    m.fn_(
        "get",
        |url: &str, opts: DynValue| -> Result<HttpResponse, String> {
            let opts = parse_opts(&opts)?;
            let req = apply_headers(agent(&opts).get(url), &opts);
            let res = req.call().map_err(|e| e.to_string())?;
            to_response(res)
        },
    );
    m.doc_next("POST a body to a URL. opts as for get");
    m.fn_(
        "post",
        |url: &str, body: &str, opts: DynValue| -> Result<HttpResponse, String> {
            let opts = parse_opts(&opts)?;
            let req = apply_headers(agent(&opts).post(url), &opts);
            let res = req.send(body).map_err(|e| e.to_string())?;
            to_response(res)
        },
    );
    m.doc_next("Download a URL to a file; returns the byte count");
    m.fn_(
        "download",
        |url: &str, dest: &str, opts: DynValue| -> Result<i64, String> {
            let opts = parse_opts(&opts)?;
            let req = apply_headers(agent(&opts).get(url), &opts);
            let res = req.call().map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("GET {url} returned {}", res.status()));
            }
            let mut reader = res.into_body().into_reader();
            let mut file =
                std::fs::File::create(dest).map_err(|e| format!("cannot create {dest}: {e}"))?;
            let bytes = std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
            Ok(bytes as i64)
        },
    );
    m
}
