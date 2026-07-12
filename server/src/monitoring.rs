//! Observability: the Prometheus recorder + HTTP metrics layer, the app
//! metric names, and the proxy endpoints behind the per-service
//! Monitoring/Logs tabs. The browser never talks to Prometheus or Loki —
//! handlers here compose PromQL/LogQL server-side from structured params
//! against `--prometheus-url` / `--loki-url` and return flattened JSON.

use axum::Extension;
use axum::extract::{Path as UrlPath, Query};
use axum::http::StatusCode;
use axum::response::Response;
use axum_prometheus::metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use axum_prometheus::{PrometheusMetricLayer, PrometheusMetricLayerBuilder};
use forge_server::{RequireClaims, err, ok};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::SharedState;

// App metric names (the HTTP ones come from axum-prometheus with the
// `weave` prefix). Labels are service/system names from the inventory —
// cardinality is bounded by its size.
pub const SYSTEM_RUNS_TOTAL: &str = "weave_system_runs_total";
pub const SYSTEM_RUN_DURATION: &str = "weave_system_run_duration_seconds";
pub const SYSTEM_RUNS_ACTIVE: &str = "weave_system_runs_active";
pub const SCHEDULE_DISPATCH_TOTAL: &str = "weave_schedule_dispatch_total";
pub const SCHEDULER_LAST_TICK: &str = "weave_scheduler_last_tick_timestamp_seconds";
pub const TEST_RUNS_TOTAL: &str = "weave_test_runs_total";
pub const TEST_RUNS_ACTIVE: &str = "weave_test_runs_active";
pub const REPO_SYNC_DISPATCH_TOTAL: &str = "weave_repo_sync_dispatch_total";

const RUN_DURATION_BUCKETS: &[f64] = &[1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0];

/// Install the Prometheus recorder and build the HTTP metrics layer.
/// Must run before any `metrics::` macro fires (earlier calls are
/// silently dropped, not an error).
pub fn setup() -> (PrometheusMetricLayer<'static>, PrometheusHandle) {
    let (layer, handle) = PrometheusMetricLayerBuilder::new()
        .with_prefix("weave")
        .with_metrics_from_fn(|| {
            PrometheusBuilder::new()
                // A custom recorder skips axum-prometheus's default bucket
                // setup, so the HTTP duration buckets are restated here.
                .set_buckets_for_metric(
                    Matcher::Full("weave_http_requests_duration_seconds".into()),
                    axum_prometheus::utils::SECONDS_DURATION_BUCKETS,
                )
                .expect("static buckets")
                .set_buckets_for_metric(
                    Matcher::Full(SYSTEM_RUN_DURATION.into()),
                    RUN_DURATION_BUCKETS,
                )
                .expect("static buckets")
                .install_recorder()
                .expect("prometheus recorder")
        })
        .build_pair();
    metrics::describe_counter!(
        SYSTEM_RUNS_TOTAL,
        "System runs settled, by service/system/action/trigger/status"
    );
    metrics::describe_histogram!(
        SYSTEM_RUN_DURATION,
        metrics::Unit::Seconds,
        "Wall-clock duration of settled system runs"
    );
    metrics::describe_gauge!(
        SYSTEM_RUNS_ACTIVE,
        "System runs currently in flight, by service/system"
    );
    metrics::describe_counter!(
        SCHEDULE_DISPATCH_TOTAL,
        "Schedule firings, by service/schedule/outcome (started|skipped)"
    );
    metrics::describe_gauge!(
        SCHEDULER_LAST_TICK,
        "Unix time of the scheduler's last completed tick"
    );
    metrics::describe_counter!(TEST_RUNS_TOTAL, "Package test runs settled, by status");
    metrics::describe_gauge!(TEST_RUNS_ACTIVE, "Package test runs currently in flight");
    metrics::describe_counter!(
        REPO_SYNC_DISPATCH_TOTAL,
        "Remote repository syncs, by repo/trigger (manual|cron|webhook)/outcome"
    );
    (layer, handle)
}

// ------------------------------------------------------- query building

/// A PromQL/LogQL double-quoted string literal (both languages share Go
/// escaping for `"` strings). Newlines are escaped too so a hostile
/// value cannot smuggle in a second expression line.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `15m` / `1h` / `24h` / `7d` — digits + one unit, the only shape we
/// splice into a PromQL range selector. Returns (canonical, seconds).
fn parse_range(range: &str) -> Result<(String, i64), String> {
    let (digits, unit) = range.split_at(range.len().saturating_sub(1));
    let per: i64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(format!("bad range '{range}': want e.g. 15m, 1h, 24h")),
    };
    let n: i64 = digits
        .parse()
        .ok()
        .filter(|n| (1..=10_000).contains(n))
        .ok_or(format!("bad range '{range}': want e.g. 15m, 1h, 24h"))?;
    let secs = n * per;
    if secs > 30 * 86400 {
        return Err(format!("range '{range}' is over the 30d cap"));
    }
    Ok((format!("{n}{unit}"), secs))
}

fn runs_by_status_query(service: &str, range: &str) -> String {
    format!(
        "sum by (status) (increase({SYSTEM_RUNS_TOTAL}{{service={}}}[{range}]))",
        quote(service)
    )
}

fn active_query(service: &str) -> String {
    format!("sum({SYSTEM_RUNS_ACTIVE}{{service={}}})", quote(service))
}

fn p95_query(service: &str, range: &str) -> String {
    format!(
        "histogram_quantile(0.95, sum by (le) (rate({SYSTEM_RUN_DURATION}_bucket{{service={}}}[{range}])))",
        quote(service)
    )
}

/// Runs-by-status over time. The increase window tracks the step so each
/// point counts its own slice (floored at 60s — below the scrape
/// interval `increase` sees nothing).
fn timeseries_query(service: &str, system: Option<&str>, step_s: u64) -> String {
    let mut matchers = format!("service={}", quote(service));
    if let Some(system) = system {
        matchers = format!("{matchers},system={}", quote(system));
    }
    let window = step_s.max(60);
    format!("sum by (status) (increase({SYSTEM_RUNS_TOTAL}{{{matchers}}}[{window}s]))")
}

/// The two Logs-tab sources:
/// - `runs`: weave-server's own stream — static `{app="weave-server"}`
///   labels, everything else is a JSON field, hence `| json` + label
///   filters.
/// - `systems`: the forward convention for managed-system agents, which
///   must attach `service`/`system` *labels* matching services.wcl names.
fn logs_query(
    source: &str,
    service: &str,
    system: Option<&str>,
    run: Option<&str>,
    level: Option<&str>,
    search: Option<&str>,
) -> String {
    let mut q = match source {
        "systems" => {
            let mut sel = format!("service={}", quote(service));
            if let Some(system) = system {
                sel = format!("{sel},system={}", quote(system));
            }
            format!("{{{sel}}}")
        }
        _ => {
            let mut sel = "app=\"weave-server\"".to_string();
            if let Some(level) = level {
                sel = format!("{sel},level={}", quote(level));
            }
            let mut q = format!("{{{sel}}} | json | service={}", quote(service));
            if let Some(system) = system {
                q = format!("{q} | system={}", quote(system));
            }
            if let Some(run) = run {
                q = format!("{q} | run_id={}", quote(run));
            }
            q
        }
    };
    if let Some(search) = search {
        q = format!("{q} |= {}", quote(search));
    }
    q
}

// ------------------------------------------------------ upstream client

/// GET a Prometheus/Loki API endpoint (both wrap results in
/// `{status, data, error?}`) and unwrap `data`.
async fn upstream_get(
    client: &reqwest::Client,
    base: &url::Url,
    path: &str,
    params: Vec<(&'static str, String)>,
    what: &str,
) -> Result<Value, (StatusCode, String)> {
    let url = format!("{}/{path}", base.as_str().trim_end_matches('/'));
    let resp = client
        .get(&url)
        .query(&params)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{what}: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{what}: {e}")))?;
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() || body["status"].as_str() != Some("success") {
        let msg = body["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                let t = text.trim();
                if t.is_empty() {
                    status.to_string()
                } else {
                    t.chars().take(300).collect()
                }
            });
        return Err((StatusCode::BAD_GATEWAY, format!("{what}: {msg}")));
    }
    Ok(body["data"].clone())
}

fn prometheus_url(state: &SharedState) -> Result<&url::Url, (StatusCode, String)> {
    state.prometheus_url.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "prometheus not configured (--prometheus-url / PROMETHEUS_URL)".into(),
    ))
}

fn loki_url(state: &SharedState) -> Result<&url::Url, (StatusCode, String)> {
    state.loki_url.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "loki not configured (--loki-url / LOKI_URL)".into(),
    ))
}

/// A PromQL instant vector → `{label_value: number}` keyed by `by_label`
/// (single-value results use the "" key).
fn vector_to_map(data: &Value, by_label: &str) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for sample in data["result"].as_array().into_iter().flatten() {
        let key = sample["metric"][by_label]
            .as_str()
            .unwrap_or("")
            .to_string();
        if let Some(v) = sample["value"][1]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
        {
            out.insert(key, json!(v));
        }
    }
    out
}

// ---------------------------------------------------------------- routes

/// GET /api/monitoring/status — the UI's capability probe: which tabs
/// have a backend at all. Reachability problems surface per-query.
pub async fn status(Extension(state): Extension<SharedState>, _claims: RequireClaims) -> Response {
    ok(json!({
        "prometheus": state.prometheus_url.is_some(),
        "loki": state.loki_url.is_some(),
    }))
}

#[derive(Deserialize)]
pub struct SummaryParams {
    range: Option<String>,
}

/// GET /api/services/{service}/monitoring/summary?range=1h
pub async fn summary(
    Extension(state): Extension<SharedState>,
    UrlPath(service): UrlPath<String>,
    _claims: RequireClaims,
    Query(params): Query<SummaryParams>,
) -> Response {
    let base = match prometheus_url(&state) {
        Ok(u) => u,
        Err((s, e)) => return err(s, e),
    };
    let (range, _) = match parse_range(params.range.as_deref().unwrap_or("1h")) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let instant = |query: String| {
        upstream_get(
            &state.http,
            base,
            "api/v1/query",
            vec![("query", query)],
            "prometheus",
        )
    };
    let (counts, active, p95) = tokio::join!(
        instant(runs_by_status_query(&service, &range)),
        instant(active_query(&service)),
        instant(p95_query(&service, &range)),
    );
    let (counts, active, p95) = match (counts, active, p95) {
        (Ok(c), Ok(a), Ok(p)) => (c, a, p),
        (Err((s, e)), _, _) | (_, Err((s, e)), _) | (_, _, Err((s, e))) => return err(s, e),
    };
    let single = |data: &Value| vector_to_map(data, "").get("").and_then(Value::as_f64);
    ok(json!({
        "range": range,
        "run_counts": vector_to_map(&counts, "status"),
        "active": single(&active).unwrap_or(0.0),
        // NaN (no samples in range) serializes as null, which is what
        // the UI wants for "no data".
        "p95_duration_s": single(&p95).filter(|v| v.is_finite()),
    }))
}

#[derive(Deserialize)]
pub struct TimeseriesParams {
    range: Option<String>,
    step: Option<u64>,
    system: Option<String>,
}

/// GET /api/services/{service}/monitoring/timeseries?range=1h&step=60&system=
pub async fn timeseries(
    Extension(state): Extension<SharedState>,
    UrlPath(service): UrlPath<String>,
    _claims: RequireClaims,
    Query(params): Query<TimeseriesParams>,
) -> Response {
    let base = match prometheus_url(&state) {
        Ok(u) => u,
        Err((s, e)) => return err(s, e),
    };
    let (_, range_s) = match parse_range(params.range.as_deref().unwrap_or("1h")) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let step = params.step.unwrap_or_else(|| (range_s as u64 / 60).max(15));
    let step = step.clamp(15, 3600);
    let end = chrono::Utc::now().timestamp();
    let query = timeseries_query(&service, params.system.as_deref(), step);
    let data = upstream_get(
        &state.http,
        base,
        "api/v1/query_range",
        vec![
            ("query", query),
            ("start", (end - range_s).to_string()),
            ("end", end.to_string()),
            ("step", step.to_string()),
        ],
        "prometheus",
    )
    .await;
    let data = match data {
        Ok(d) => d,
        Err((s, e)) => return err(s, e),
    };
    let series: Vec<Value> = data["result"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let points: Vec<Value> = s["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|pair| {
                    let ts = pair[0].as_f64()?;
                    let v = pair[1].as_str()?.parse::<f64>().ok()?;
                    Some(json!([ts, v]))
                })
                .collect();
            json!({
                "name": s["metric"]["status"].as_str().unwrap_or("runs"),
                "points": points,
            })
        })
        .collect();
    ok(json!({ "step": step, "series": series }))
}

#[derive(Deserialize)]
pub struct LogParams {
    range: Option<String>,
    limit: Option<usize>,
    system: Option<String>,
    run: Option<String>,
    level: Option<String>,
    search: Option<String>,
    source: Option<String>,
}

const LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];

/// GET /api/services/{service}/logs?range&limit&system&run&level&search&source=runs|systems
pub async fn logs(
    Extension(state): Extension<SharedState>,
    UrlPath(service): UrlPath<String>,
    _claims: RequireClaims,
    Query(params): Query<LogParams>,
) -> Response {
    let base = match loki_url(&state) {
        Ok(u) => u,
        Err((s, e)) => return err(s, e),
    };
    let (_, range_s) = match parse_range(params.range.as_deref().unwrap_or("1h")) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let source = params.source.as_deref().unwrap_or("runs");
    if !["runs", "systems"].contains(&source) {
        return err(StatusCode::BAD_REQUEST, "source must be runs|systems");
    }
    if let Some(level) = params.level.as_deref()
        && !LEVELS.contains(&level)
    {
        return err(
            StatusCode::BAD_REQUEST,
            "level must be trace|debug|info|warn|error",
        );
    }
    let limit = params.limit.unwrap_or(500).clamp(1, 1000);
    let query = logs_query(
        source,
        &service,
        params.system.as_deref(),
        params.run.as_deref(),
        params.level.as_deref(),
        params.search.as_deref().filter(|s| !s.is_empty()),
    );
    let end_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let start_ns = end_ns - range_s * 1_000_000_000;
    let data = upstream_get(
        &state.http,
        base,
        "loki/api/v1/query_range",
        vec![
            ("query", query),
            ("start", start_ns.to_string()),
            ("end", end_ns.to_string()),
            ("limit", limit.to_string()),
            ("direction", "backward".to_string()),
        ],
        "loki",
    )
    .await;
    let data = match data {
        Ok(d) => d,
        Err((s, e)) => return err(s, e),
    };
    let mut entries: Vec<(i64, Value)> = Vec::new();
    for stream in data["result"].as_array().into_iter().flatten() {
        let labels = &stream["stream"];
        for pair in stream["values"].as_array().into_iter().flatten() {
            let ts_ms = pair[0]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
                / 1_000_000;
            entries.push((
                ts_ms,
                flatten_entry(ts_ms, labels, pair[1].as_str().unwrap_or("")),
            ));
        }
    }
    entries.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
    entries.truncate(limit);
    ok(json!({ "entries": entries.into_iter().map(|(_, e)| e).collect::<Vec<_>>() }))
}

/// One Loki line → the UI's entry shape. weave-server's own lines are
/// tracing-loki JSON (fields flattened at top level, `_target` etc.);
/// anything else (future system streams) passes through as raw text.
fn flatten_entry(ts_ms: i64, labels: &Value, line: &str) -> Value {
    let label_level = |labels: &Value| labels["level"].as_str().map(str::to_string);
    let parsed: Option<Value> = serde_json::from_str(line).ok().filter(Value::is_object);
    match parsed {
        Some(obj) => {
            let field = |k: &str| obj[k].as_str().map(str::to_string);
            json!({
                "ts": ts_ms,
                "level": label_level(labels).or_else(|| field("level")),
                "message": field("message").unwrap_or_else(|| line.to_string()),
                "target": field("_target"),
                "service": field("service"),
                "system": field("system"),
                "run_id": field("run_id"),
                "playbook": field("playbook"),
                "play": field("play"),
                "action": field("action"),
            })
        }
        None => json!({
            "ts": ts_ms,
            "level": label_level(labels),
            "message": line,
            "system": labels["system"].as_str(),
            "service": labels["service"].as_str(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_hostile_values() {
        assert_eq!(quote("web"), "\"web\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote("a\nb"), "\"a\\nb\"");
        // A value trying to close the matcher and add its own stays inert.
        assert_eq!(quote("x\"} or up{y=\""), "\"x\\\"} or up{y=\\\"\"");
    }

    #[test]
    fn ranges_parse_and_reject() {
        assert_eq!(parse_range("15m").unwrap(), ("15m".into(), 900));
        assert_eq!(parse_range("1h").unwrap(), ("1h".into(), 3600));
        assert_eq!(parse_range("7d").unwrap(), ("7d".into(), 604800));
        assert!(parse_range("1w").is_err());
        assert!(parse_range("h").is_err());
        assert!(parse_range("-5m").is_err());
        assert!(parse_range("99d").is_err());
        assert!(parse_range("5m; drop").is_err());
    }

    #[test]
    fn promql_composition() {
        assert_eq!(
            runs_by_status_query("web", "1h"),
            "sum by (status) (increase(weave_system_runs_total{service=\"web\"}[1h]))"
        );
        assert_eq!(
            active_query("web"),
            "sum(weave_system_runs_active{service=\"web\"})"
        );
        assert_eq!(
            p95_query("web", "6h"),
            "histogram_quantile(0.95, sum by (le) (rate(weave_system_run_duration_seconds_bucket{service=\"web\"}[6h])))"
        );
        assert_eq!(
            timeseries_query("web", Some("edge"), 300),
            "sum by (status) (increase(weave_system_runs_total{service=\"web\",system=\"edge\"}[300s]))"
        );
        // Step below the floor widens to 60s.
        assert!(timeseries_query("web", None, 15).contains("[60s]"));
    }

    #[test]
    fn logql_composition() {
        assert_eq!(
            logs_query("runs", "web", None, None, None, None),
            "{app=\"weave-server\"} | json | service=\"web\""
        );
        assert_eq!(
            logs_query(
                "runs",
                "web",
                Some("edge"),
                Some("r1"),
                Some("warn"),
                Some("oops")
            ),
            "{app=\"weave-server\",level=\"warn\"} | json | service=\"web\" | system=\"edge\" | run_id=\"r1\" |= \"oops\""
        );
        assert_eq!(
            logs_query("systems", "web", Some("edge"), None, None, None),
            "{service=\"web\",system=\"edge\"}"
        );
    }
}
