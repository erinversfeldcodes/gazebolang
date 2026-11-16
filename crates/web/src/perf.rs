#![cfg(feature = "ssr")]
//! Real-user performance monitoring: the browser beacon in `rum.js` posts
//! one anonymous report per page load to `POST /api/perf`; they're stored
//! in `perf_events` and summarised (percentiles per release and route) by
//! `GET /api/perf/summary`, which needs a signed-in session.
//!
//! Nothing identifying is kept: no user id, IP or user agent. The route is
//! stored as a pattern (UUIDs collapsed to `:id`) and capped in length, so
//! a hostile client can't bloat the table with unique strings.

// These just propagate sqlx::Error as-is, same as db.rs.
#![allow(clippy::missing_errors_doc)]

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::state::AppState;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Call once at boot so `server_uptime_s` measures from process start.
pub fn init() {
    let _ = STARTED.set(Instant::now());
}

fn uptime_secs() -> u64 {
    STARTED.get().map_or(0, |t| t.elapsed().as_secs())
}

/// Identifies the deploy that served a request: Fly's machine version
/// changes on every release. `dev` outside Fly.
#[must_use]
pub fn release() -> String {
    std::env::var("FLY_MACHINE_VERSION").unwrap_or_else(|_| "dev".to_string())
}

/// Adds a `Server-Timing` header to every response: `app` is the time in
/// milliseconds to produce the response headers (first byte only — the page
/// shell streams immediately, so it says little about slow queries; see
/// `response_end_ms` for the full page) and `up` is seconds since the
/// process booted. The browser exposes both on the navigation entry, so
/// each report carries the server's timing and whether it had just
/// cold-started.
pub async fn server_timing(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let started = Instant::now();
    let mut response = next.run(request).await;
    let value = format!(
        "app;dur={}, up;dur={}",
        started.elapsed().as_millis(),
        uptime_secs()
    );
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().insert("server-timing", value);
    }
    response
}

/// Collapses a request path to its route pattern: query/fragment dropped,
/// UUID segments become `:id`, at most 6 segments and 120 bytes kept.
#[must_use]
pub fn normalize_path(raw: &str) -> String {
    let path = raw.split(['?', '#']).next().unwrap_or("");
    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .take(6)
        .map(|s| {
            if uuid::Uuid::parse_str(s).is_ok() {
                ":id".to_string()
            } else {
                s.chars().take(40).collect()
            }
        })
        .collect();
    let mut out = format!("/{}", segments.join("/"));
    out.truncate(120);
    out
}

/// A millisecond/size value from the client, kept only if it's a sane
/// non-negative number no bigger than `max`.
fn clamp(value: Option<f64>, max: f64) -> Option<i32> {
    #[allow(clippy::cast_possible_truncation)]
    value
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= max)
        .map(|v| v.round() as i32)
}

/// Free text from the client, truncated on a char boundary.
fn clip(value: Option<String>, max_chars: usize) -> Option<String> {
    value
        .map(|s| s.chars().take(max_chars).collect::<String>())
        .filter(|s| !s.is_empty())
}

const MS_MAX: f64 = 600_000.0;
const BYTES_MAX: f64 = 1_000_000_000.0;

/// What `rum.js` sends. Every metric is optional: a browser that doesn't
/// support one just leaves it out.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PerfReport {
    pub path: String,
    pub nav_type: Option<String>,
    pub ttfb_ms: Option<f64>,
    pub response_end_ms: Option<f64>,
    pub fcp_ms: Option<f64>,
    pub lcp_ms: Option<f64>,
    pub dom_ready_ms: Option<f64>,
    pub load_ms: Option<f64>,
    pub hydrated_ms: Option<f64>,
    pub cls: Option<f64>,
    pub inp_ms: Option<f64>,
    pub server_ms: Option<f64>,
    pub server_uptime_s: Option<f64>,
    pub wasm_transfer_bytes: Option<f64>,
    pub wasm_decoded_bytes: Option<f64>,
    pub wasm_fetch_ms: Option<f64>,
    pub connection: Option<String>,
    pub viewport_w: Option<f64>,
    pub js_error: Option<String>,
}

pub async fn insert_report(pool: &PgPool, release: &str, report: PerfReport) -> sqlx::Result<()> {
    #[allow(clippy::cast_possible_truncation)]
    let cls = report
        .cls
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        .map(|v| v as f32);
    sqlx::query(
        "INSERT INTO perf_events \
         (release, path, nav_type, ttfb_ms, fcp_ms, lcp_ms, dom_ready_ms, load_ms, hydrated_ms, \
          cls, inp_ms, server_ms, server_uptime_s, wasm_transfer_bytes, wasm_decoded_bytes, \
          wasm_fetch_ms, connection, viewport_w, js_error, response_end_ms) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
    )
    .bind(release)
    .bind(normalize_path(&report.path))
    .bind(clip(report.nav_type, 20))
    .bind(clamp(report.ttfb_ms, MS_MAX))
    .bind(clamp(report.fcp_ms, MS_MAX))
    .bind(clamp(report.lcp_ms, MS_MAX))
    .bind(clamp(report.dom_ready_ms, MS_MAX))
    .bind(clamp(report.load_ms, MS_MAX))
    .bind(clamp(report.hydrated_ms, MS_MAX))
    .bind(cls)
    .bind(clamp(report.inp_ms, MS_MAX))
    .bind(clamp(report.server_ms, MS_MAX))
    .bind(clamp(report.server_uptime_s, 31_536_000.0))
    .bind(clamp(report.wasm_transfer_bytes, BYTES_MAX))
    .bind(clamp(report.wasm_decoded_bytes, BYTES_MAX))
    .bind(clamp(report.wasm_fetch_ms, MS_MAX))
    .bind(clip(report.connection, 12))
    .bind(clamp(report.viewport_w, 20_000.0))
    .bind(clip(report.js_error, 400))
    .bind(clamp(report.response_end_ms, MS_MAX))
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes reports older than `days` — run at boot so the table can't grow
/// without bound.
pub async fn prune(pool: &PgPool, days: i32) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM perf_events WHERE created_at < now() - make_interval(days => $1)")
            .bind(days)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Crude flood guard for the public beacon endpoint: at most this many
/// reports are accepted per minute, process-wide. Plenty for a handful of
/// users; stops a hostile client filling the table.
const MAX_REPORTS_PER_MINUTE: u32 = 300;

fn allow_report() -> bool {
    static WINDOW: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
    let Ok(mut window) = WINDOW.lock() else {
        return false;
    };
    let now = Instant::now();
    match window.as_mut() {
        Some((start, count)) if now.duration_since(*start) < Duration::from_secs(60) => {
            if *count >= MAX_REPORTS_PER_MINUTE {
                return false;
            }
            *count += 1;
        }
        _ => *window = Some((now, 1)),
    }
    true
}

/// `POST /api/perf` — public (a page has to be able to report even when
/// signed out), so it only ever inserts, validates and caps everything it
/// stores, and is rate-limited. Always answers 204 for a well-formed
/// request so the beacon never retries.
pub async fn receive_report(
    State(state): State<AppState>,
    Json(report): Json<PerfReport>,
) -> StatusCode {
    if !allow_report() {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    match insert_report(&state.db_pool, &release(), report).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(e) => {
            log::warn!("storing a perf report failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SummaryRow {
    pub release: String,
    pub path: String,
    pub loads: i64,
    pub first_seen: time::OffsetDateTime,
    pub last_seen: time::OffsetDateTime,
    pub ttfb_p50: Option<f64>,
    pub ttfb_p95: Option<f64>,
    /// Whole HTML response received, including streamed data-dependent
    /// sections — the metric that reflects slow queries.
    pub response_end_p50: Option<f64>,
    pub response_end_p95: Option<f64>,
    pub fcp_p50: Option<f64>,
    pub fcp_p95: Option<f64>,
    pub lcp_p50: Option<f64>,
    pub lcp_p95: Option<f64>,
    pub hydrated_p50: Option<f64>,
    pub hydrated_p95: Option<f64>,
    pub load_p50: Option<f64>,
    pub load_p95: Option<f64>,
    pub server_p50: Option<f64>,
    pub server_p95: Option<f64>,
    pub inp_p95: Option<f64>,
    pub cls_p75: Option<f64>,
    /// Loads that reported a JS error / wasm panic.
    pub errors: i64,
    /// Loads that never finished hydrating.
    pub not_hydrated: i64,
    /// Loads served right after a cold start (process up under 30s).
    pub cold_starts: i64,
    pub wasm_transfer_avg: Option<f64>,
    pub wasm_cached: i64,
    pub last_error: Option<String>,
}

pub async fn summary(pool: &PgPool, days: i32) -> sqlx::Result<Vec<SummaryRow>> {
    sqlx::query_as(
        "SELECT release, path, count(*) AS loads, \
                min(created_at) AS first_seen, max(created_at) AS last_seen, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY ttfb_ms) AS ttfb_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY ttfb_ms) AS ttfb_p95, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY response_end_ms) AS response_end_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY response_end_ms) AS response_end_p95, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY fcp_ms) AS fcp_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY fcp_ms) AS fcp_p95, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY lcp_ms) AS lcp_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY lcp_ms) AS lcp_p95, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY hydrated_ms) AS hydrated_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY hydrated_ms) AS hydrated_p95, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY load_ms) AS load_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY load_ms) AS load_p95, \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY server_ms) AS server_p50, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY server_ms) AS server_p95, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY inp_ms) AS inp_p95, \
                percentile_cont(0.75) WITHIN GROUP (ORDER BY cls) AS cls_p75, \
                count(*) FILTER (WHERE js_error IS NOT NULL) AS errors, \
                count(*) FILTER (WHERE hydrated_ms IS NULL) AS not_hydrated, \
                count(*) FILTER (WHERE server_uptime_s < 30) AS cold_starts, \
                avg(wasm_transfer_bytes)::float8 AS wasm_transfer_avg, \
                count(*) FILTER (WHERE wasm_transfer_bytes = 0) AS wasm_cached, \
                (array_agg(js_error ORDER BY created_at DESC) \
                    FILTER (WHERE js_error IS NOT NULL))[1] AS last_error \
         FROM perf_events \
         WHERE created_at > now() - make_interval(days => $1) \
         GROUP BY release, path \
         ORDER BY max(created_at) DESC, path",
    )
    .bind(days)
    .fetch_all(pool)
    .await
}

/// Minimum length of `PERF_API_TOKEN` for it to be honoured at all — a short
/// or empty secret must never act as a password.
const MIN_TOKEN_LEN: usize = 32;

/// True when `authorization` is `Bearer <token>` and the token equals the
/// configured secret. Compares in constant time so response timing can't be
/// used to guess the token a byte at a time.
fn bearer_token_matches(authorization: Option<&str>, configured: Option<&str>) -> bool {
    let (Some(authorization), Some(configured)) = (authorization, configured) else {
        return false;
    };
    let Some(presented) = authorization.strip_prefix("Bearer ") else {
        return false;
    };
    if configured.len() < MIN_TOKEN_LEN || presented.len() != configured.len() {
        return false;
    }
    presented
        .bytes()
        .zip(configured.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

#[derive(Debug, Deserialize)]
pub struct SummaryQuery {
    days: Option<i32>,
}

/// `GET /api/perf/summary?days=7` — percentiles per release and route as
/// JSON. Needs either a signed-in session (the same cookie the app uses) or
/// `Authorization: Bearer <PERF_API_TOKEN>`, so it's not a public view of
/// usage data. The token is a Fly secret, unset by default (no token access).
pub async fn summary_report(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    cookies: tower_cookies::Cookies,
    Query(query): Query<SummaryQuery>,
) -> axum::response::Response {
    let has_token = bearer_token_matches(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        std::env::var("PERF_API_TOKEN").ok().as_deref(),
    );
    if !has_token
        && crate::auth::require_user(&state.db_pool, &cookies)
            .await
            .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let days = query.days.unwrap_or(7).clamp(1, 90);
    match summary(&state.db_pool, days).await {
        Ok(rows) => {
            let mut response = Json(rows).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => {
            log::warn!("perf summary failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_collapse_and_query_strings_are_dropped() {
        assert_eq!(
            normalize_path("/cards/09e6f782-bfd0-44e3-8b8e-5ef8433739e6?x=1#top"),
            "/cards/:id"
        );
        assert_eq!(normalize_path("/dictionary"), "/dictionary");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
    }

    #[test]
    fn hostile_paths_are_bounded() {
        let long = format!("/{}", "a".repeat(5000));
        assert!(normalize_path(&long).len() <= 120);
        let deep = "/a".repeat(100);
        assert_eq!(normalize_path(&deep).matches('/').count(), 6);
    }

    #[test]
    fn insane_numbers_are_dropped() {
        assert_eq!(clamp(Some(1234.6), MS_MAX), Some(1235));
        assert_eq!(clamp(Some(-1.0), MS_MAX), None);
        assert_eq!(clamp(Some(f64::NAN), MS_MAX), None);
        assert_eq!(clamp(Some(f64::INFINITY), MS_MAX), None);
        assert_eq!(clamp(Some(MS_MAX + 1.0), MS_MAX), None);
        assert_eq!(clamp(None, MS_MAX), None);
    }

    #[test]
    fn free_text_is_truncated_and_blank_is_dropped() {
        assert_eq!(
            clip(Some("é".repeat(1000)), 400).unwrap().chars().count(),
            400
        );
        assert_eq!(clip(Some(String::new()), 10), None);
    }

    #[test]
    fn only_the_exact_long_enough_bearer_token_is_accepted() {
        let secret = "a".repeat(40);
        let header = format!("Bearer {secret}");
        assert!(bearer_token_matches(Some(&header), Some(&secret)));
        // Wrong token, wrong scheme, missing header, missing secret.
        assert!(!bearer_token_matches(
            Some(&format!("Bearer {}b", "a".repeat(39))),
            Some(&secret)
        ));
        assert!(!bearer_token_matches(Some(&secret), Some(&secret)));
        assert!(!bearer_token_matches(None, Some(&secret)));
        assert!(!bearer_token_matches(Some(&header), None));
        // A short secret is never honoured, even when presented exactly.
        assert!(!bearer_token_matches(Some("Bearer short"), Some("short")));
        assert!(!bearer_token_matches(Some("Bearer "), Some("")));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reports_are_stored_normalized_and_summarised_per_release(pool: PgPool) {
        for (release, ttfb) in [("v1", 100.0), ("v1", 300.0), ("v2", 50.0)] {
            insert_report(
                &pool,
                release,
                PerfReport {
                    path: "/cards/09e6f782-bfd0-44e3-8b8e-5ef8433739e6".into(),
                    ttfb_ms: Some(ttfb),
                    response_end_ms: Some(ttfb * 4.0),
                    hydrated_ms: Some(900.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        insert_report(
            &pool,
            "v2",
            PerfReport {
                path: "/cards/09e6f782-bfd0-44e3-8b8e-5ef8433739e6".into(),
                js_error: Some("panicked at hydration.rs:195".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let rows = summary(&pool, 7).await.unwrap();
        assert_eq!(rows.len(), 2, "one row per release and route");
        let v1 = rows.iter().find(|r| r.release == "v1").unwrap();
        assert_eq!(v1.path, "/cards/:id");
        assert_eq!(v1.loads, 2);
        assert_eq!(v1.ttfb_p50, Some(200.0));
        assert_eq!(v1.response_end_p50, Some(800.0));
        assert_eq!(v1.errors, 0);
        let v2 = rows.iter().find(|r| r.release == "v2").unwrap();
        assert_eq!(v2.errors, 1);
        assert_eq!(v2.not_hydrated, 1);
        assert_eq!(
            v2.last_error.as_deref(),
            Some("panicked at hydration.rs:195")
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn prune_removes_only_old_reports(pool: PgPool) {
        insert_report(&pool, "v1", PerfReport::default())
            .await
            .unwrap();
        sqlx::query("UPDATE perf_events SET created_at = now() - interval '100 days'")
            .execute(&pool)
            .await
            .unwrap();
        insert_report(&pool, "v1", PerfReport::default())
            .await
            .unwrap();
        assert_eq!(prune(&pool, 90).await.unwrap(), 1);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM perf_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1);
    }
}
