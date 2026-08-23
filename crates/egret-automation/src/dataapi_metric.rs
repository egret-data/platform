//! Z2: the `dataapi_alert` evaluator's arithmetic — turns a window of
//! `dataapi.access_log` into the single `Option<f64>` `validator::evaluate`
//! compares against a threshold. Nothing calls [`measure`] yet; the runner
//! dispatch that will is a later task in this decomposition.
//!
//! The three definitions below are COPIED from `egret-dataapi::activity`'s
//! `ActivityStore::summary` — the statistics panel — not re-derived. An alert
//! that reports a different number from the panel a human is looking at when
//! it fires is a support ticket, so this module must not invent its own
//! reading of "error" or "trial".
//!
//! * **What counts as an error** — `status_code >= 400`.
//! * **What p50/p95 filter to** — `status_code BETWEEN 200 AND 299`. A burst
//!   of fast 401s must not drag the percentile down at exactly the moment the
//!   system is failing.
//! * **How trials are excluded** — `actor_user_id IS NULL`. A Try-it run is
//!   not consumer traffic.
//!
//! This crate does not depend on `egret-dataapi` for this: only the field
//! names and the table's shape are shared, and duplicating the six short
//! queries here is cheaper than a cross-crate dependency for a handful of
//! `SELECT`s that must stay static text anyway (sqlx 0.9's `query()` takes a
//! `&'static str`).

use chrono::{DateTime, Duration, Utc};
use egret_metastore::Db;
use uuid::Uuid;

/// Every query here shares one shape, bound the same way every time:
/// `workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) AND ts >= $3
/// AND actor_user_id IS NULL`. `>=`, matching `activity.rs`'s own `ts >= $2` —
/// the two statements must agree at the boundary, not just on the common
/// case, or a reviewer comparing them has to work out whether a `>` here was
/// deliberate. `$2` narrows to one endpoint when given, or
/// aggregates the whole workspace when `None`; `$3` is the window's start,
/// computed from the caller's `now` rather than read from a second call to
/// `now()` inside the statement — two clock readings that can coincide is a
/// flake this repo has already recorded (see `time-dependent-test-assertions`
/// in project memory), and an evaluator's `now` must be the SAME instant the
/// caller used to compute `window_s`'s start.
///
/// # The empty-window asymmetry
///
/// `call_count`, `error_count` and `error_code_count` are `Some(0.0)` over an
/// empty window: a count of zero rows IS a measurement — zero calls happened.
/// `error_rate`, `p50_duration_ms` and `p95_duration_ms` are `None`: a rate or
/// a percentile over zero rows is not zero, it is undefined, and reporting
/// `Some(0.0)` would let a silent endpoint (no traffic at all) read as a
/// perfectly healthy one (zero errors, instant responses) to a threshold
/// alert. This reads as an inconsistency on a casual pass — it is not one.
/// Fix the individual metric, not the pattern, if a future change makes them
/// look uniform.
pub async fn measure(
    db: &Db,
    ws: Uuid,
    endpoint_id: Option<Uuid>,
    metric: &str,
    error_code: Option<&str>,
    window_s: i32,
    now: DateTime<Utc>,
) -> Result<Option<f64>, sqlx::Error> {
    let since = now - Duration::seconds(i64::from(window_s));
    match metric {
        "call_count" => call_count(db, ws, endpoint_id, since).await,
        "error_count" => error_count(db, ws, endpoint_id, since).await,
        "error_rate" => error_rate(db, ws, endpoint_id, since).await,
        "p50_duration_ms" => percentile(db, ws, endpoint_id, since, 0.5).await,
        "p95_duration_ms" => percentile(db, ws, endpoint_id, since, 0.95).await,
        "error_code_count" => error_code_count(db, ws, endpoint_id, since, error_code).await,
        // Unreachable for a persisted schedule: `schedule_dataapi_metric_valid`
        // (migration 0128) already rejects any other string at INSERT time.
        // `None` rather than a panic because this function must not be the
        // thing that turns a data problem into a crashed scheduler tick, and
        // `None` is this module's existing spelling of "no evidence" — it
        // will simply never fire.
        _ => Ok(None),
    }
}

async fn call_count(
    db: &Db,
    ws: Uuid,
    endpoint_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> Result<Option<f64>, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM dataapi.access_log \
         WHERE workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) \
           AND ts >= $3 AND actor_user_id IS NULL",
    )
    .bind(ws)
    .bind(endpoint_id)
    .bind(since)
    .fetch_one(&db.pool)
    .await?;
    Ok(Some(count as f64))
}

async fn error_count(
    db: &Db,
    ws: Uuid,
    endpoint_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> Result<Option<f64>, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FILTER (WHERE status_code >= 400) \
         FROM dataapi.access_log \
         WHERE workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) \
           AND ts >= $3 AND actor_user_id IS NULL",
    )
    .bind(ws)
    .bind(endpoint_id)
    .bind(since)
    .fetch_one(&db.pool)
    .await?;
    Ok(Some(count as f64))
}

/// `None` when the window has no calls at all — an error rate needs a
/// denominator. Computed from one query's two counts, in Rust, rather than as
/// `errors::float8 / count(*)` in SQL: Postgres would return `NULL` for `0/0`
/// too, but doing the division here keeps the "empty means undefined" rule
/// visible as code instead of relying on floating-point NULL propagation.
async fn error_rate(
    db: &Db,
    ws: Uuid,
    endpoint_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> Result<Option<f64>, sqlx::Error> {
    let (total, errors): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE status_code >= 400) \
         FROM dataapi.access_log \
         WHERE workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) \
           AND ts >= $3 AND actor_user_id IS NULL",
    )
    .bind(ws)
    .bind(endpoint_id)
    .bind(since)
    .fetch_one(&db.pool)
    .await?;
    if total == 0 {
        Ok(None)
    } else {
        Ok(Some(errors as f64 / total as f64))
    }
}

/// Shared by `p50_duration_ms` and `p95_duration_ms`; `p` is `0.5` or `0.95`
/// respectively. `percentile_cont` returns SQL `NULL` — which sqlx surfaces
/// as `None` — when the `FILTER`ed population is empty, so the empty-window
/// `None` falls out of Postgres's own aggregate semantics; nothing extra is
/// needed here the way `error_rate` needs the explicit `total == 0` check.
async fn percentile(
    db: &Db,
    ws: Uuid,
    endpoint_id: Option<Uuid>,
    since: DateTime<Utc>,
    p: f64,
) -> Result<Option<f64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT percentile_cont($4) WITHIN GROUP (ORDER BY duration_ms) \
           FILTER (WHERE status_code BETWEEN 200 AND 299) \
         FROM dataapi.access_log \
         WHERE workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) \
           AND ts >= $3 AND actor_user_id IS NULL",
    )
    .bind(ws)
    .bind(endpoint_id)
    .bind(since)
    .bind(p)
    .fetch_one(&db.pool)
    .await
}

async fn error_code_count(
    db: &Db,
    ws: Uuid,
    endpoint_id: Option<Uuid>,
    since: DateTime<Utc>,
    error_code: Option<&str>,
) -> Result<Option<f64>, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FILTER (WHERE error_code = $4) \
         FROM dataapi.access_log \
         WHERE workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) \
           AND ts >= $3 AND actor_user_id IS NULL",
    )
    .bind(ws)
    .bind(endpoint_id)
    .bind(since)
    .bind(error_code)
    .fetch_one(&db.pool)
    .await?;
    Ok(Some(count as f64))
}
