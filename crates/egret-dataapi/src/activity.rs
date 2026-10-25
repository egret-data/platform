//! Data API P5 §4 — what `dataapi.access_log` can tell an operator.

use chrono::{DateTime, Duration, Utc};
use egret_metastore::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The largest page [`ActivityStore::calls`] will return, and the value a larger
/// `limit` is clamped to. Documented on `routes::ActivityCallsQuery::limit`, which
/// is where a caller looks — a clamp nobody can see is the indefensible kind.
pub const MAX_CALL_PAGE: i64 = 200;

/// The four ranges the UI offers, and the only ones it may ask for.
///
/// A fixed set rather than a date picker: every query here narrows on
/// `access_log_ws_ts_idx (workspace_id, ts DESC)`, and the presets are what keep
/// it inside that index's reach. An input that accepts "one year" on the
/// fastest-growing table in the schema has nothing stopping the person who types
/// it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
pub enum ActivityRange {
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "24h")]
    OneDay,
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "30d")]
    ThirtyDays,
}

impl ActivityRange {
    /// The cutoff, computed in Rust and bound as a `timestamptz`.
    ///
    /// Never interpolated into SQL: sqlx 0.9's `query()` takes a `&'static str`,
    /// so a range that reached the statement text would have to be built, and a
    /// built statement is the shape this codebase spent P1 removing.
    pub fn since(self, now: DateTime<Utc>) -> DateTime<Utc> {
        now - match self {
            Self::OneHour => Duration::hours(1),
            Self::OneDay => Duration::hours(24),
            Self::SevenDays => Duration::days(7),
            Self::ThirtyDays => Duration::days(30),
        }
    }
}

/// One line of the statistics table, or — with `endpoint_id: None` — either the
/// grand total or the unmatched-request bucket, depending on which grouping set
/// produced it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivityRow {
    pub endpoint_id: Option<Uuid>,
    pub endpoint_name: Option<String>,
    pub calls: i64,
    pub errors: i64,
    /// `None` when the population is empty — no successful call in range. Zero
    /// would be a measurement; this is the absence of one.
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub cache_hits: i64,
    /// The DENOMINATOR for cache-hit rate: successful calls on a version whose
    /// snapshot had caching enabled. Sent raw so the caller can render `—`
    /// rather than `0%` when it is zero — zero of zero is not zero.
    pub cacheable_calls: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivitySummaryDto {
    pub total: ActivityRow,
    pub endpoints: Vec<ActivityRow>,
    /// `DataApiConfig::access_log_retention_days`, passed through so the UI can
    /// disable ranges that reach past it (spec §4.7).
    ///
    /// **The UI cannot compute this and must not assume 30 days exist.**
    /// Retention may be shorter, and `0` means keep nothing — the same reading
    /// `AuditConfig::retention_days` has. Without this field someone selects
    /// `30d`, watches the data thin out at the old end, and concludes traffic
    /// fell; the screen would be actively supporting a wrong conclusion.
    ///
    /// `None` ⇒ no prune is scheduled (`scheduled_retention` returned `None`),
    /// so nothing is being deleted and every range is honest.
    pub retention_days: Option<i64>,
}

/// One raw row of [`ActivityStore::summary`]'s GROUPING SETS query.
///
/// **Named fields, not a nine-element tuple.** Read positionally, `calls` and
/// `errors` — same type, adjacent, and both plausible in either slot — swap
/// silently under any reordering of the SELECT list, and every existing test
/// still passes on a population where the two happen to be equal. `FromRow`
/// binds by COLUMN NAME, so the aliases in the statement are what the fields
/// track; that is why every column above is aliased even where Postgres would
/// have named it anyway.
#[derive(Debug, sqlx::FromRow)]
struct SummaryRow {
    endpoint_id: Option<Uuid>,
    endpoint_name: Option<String>,
    calls: i64,
    errors: i64,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
    cache_hits: i64,
    cacheable_calls: i64,
    /// `grouping(al.endpoint_id)` — 1 for the grand total, 0 per endpoint.
    is_total: i32,
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ActivityCallDto {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub endpoint_id: Option<Uuid>,
    pub endpoint_name: Option<String>,
    /// What the caller asked for. The ONLY identification a row carries when
    /// `endpoint_id` is null — see migration 0125.
    pub request_method: Option<String>,
    pub request_path: Option<String>,
    pub status_code: i32,
    pub error_code: Option<String>,
    pub duration_ms: i32,
    pub cache_hit: bool,
    pub client_id: Option<String>,
    pub request_id: String,
    /// Non-null ⇒ a Try-it run by this person, not consumer traffic.
    pub actor_user_id: Option<Uuid>,
    /// The actor's profile name, else email; null for consumer traffic or a deleted user.
    /// Resolved here because the user list is admin-only, so an editor's browser could not
    /// (KI-296).
    pub actor_name: Option<String>,
}

#[derive(Clone)]
pub struct ActivityStore {
    db: Db,
    /// Held rather than re-read per request: it comes from config, which does
    /// not change while the process runs.
    retention_days: Option<i64>,
}

impl ActivityStore {
    /// `retention_days` is `crate::serving::scheduled_retention(cfg.dataapi
    /// .access_log_retention_days)` — the SAME guard the prune scheduler uses,
    /// so the UI is told what is actually happening rather than what was
    /// configured. Passing the raw config value here would report a retention
    /// of 0 as "keeps nothing" even where no prune is scheduled at all.
    pub fn new(db: Db, retention_days: Option<i64>) -> Self {
        Self { db, retention_days }
    }

    /// Tiles and table, from ONE statement.
    ///
    /// `GROUPING SETS ((), (endpoint_id))` rather than two queries: the table is
    /// written continuously, and two statements moments apart produce a total
    /// that does not equal the sum of its own rows. A reader who notices that
    /// concludes they misread the screen.
    ///
    /// # The three denominators
    ///
    /// * `calls` / `errors` — every row.
    /// * `p50`/`p95` — `FILTER (WHERE status_code BETWEEN 200 AND 299)`. A burst
    ///   of 401s returning in 2ms drags p50 down and makes latency look better
    ///   at exactly the moment the system is failing.
    /// * `cacheable_calls` — successful calls whose SERVING VERSION had caching
    ///   on, read from that version's own snapshot. Joining the version the row
    ///   records (rather than the endpoint's current config) is what makes the
    ///   ratio describe the calls it counts: an endpoint whose caching was turned
    ///   on yesterday must not retroactively make last week's calls look like
    ///   misses.
    ///
    /// Trials (`actor_user_id IS NOT NULL`) are excluded here and shown only in
    /// the call list.
    ///
    /// # `cache_enabled` is projected in a CTE, not read per access-log row
    ///
    /// `endpoint_versions.snapshot` holds the WHOLE frozen definition — SQL,
    /// params, resolved columns — so it is TOASTed out of line for any endpoint
    /// worth publishing. Written the obvious way, the
    /// `(snapshot -> 'cache' ->> 'enabled')` extraction sits in the outer
    /// aggregate over the join and is therefore evaluated once per joined
    /// `access_log` row: on the `30d` preset, one detoast per call in the window,
    /// on the fastest-growing table in the schema.
    ///
    /// `version_cache` pulls that extraction back to where it belongs — once per
    /// (endpoint, version) in the workspace — and the outer query joins a
    /// two-column boolean. `MATERIALIZED` is load-bearing: Postgres 12+ inlines a
    /// non-recursive CTE referenced once, which would push the expression back
    /// into the join and restore exactly the behaviour this replaces.
    ///
    /// The RESULT is unchanged, and
    /// `tests/activity.rs`'s `cacheable_calls_reads_the_versions_own_snapshot_not_the_live_config`
    /// is what holds that: it must still pass, and must still fail if the
    /// `version_cache.version = al.version` half of the join is dropped.
    pub async fn summary(
        &self,
        ws: Uuid,
        since: DateTime<Utc>,
    ) -> Result<ActivitySummaryDto, sqlx::Error> {
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "WITH version_cache AS MATERIALIZED ( \
               SELECT ev.endpoint_id, \
                      ev.version, \
                      (ev.snapshot -> 'cache' ->> 'enabled')::boolean IS TRUE AS cache_enabled \
                 FROM dataapi.endpoint_versions ev \
                 JOIN dataapi.endpoints ep ON ep.id = ev.endpoint_id \
                WHERE ep.workspace_id = $1 \
             ) \
             SELECT al.endpoint_id AS endpoint_id, \
                    max(e.name) AS endpoint_name, \
                    count(*) AS calls, \
                    count(*) FILTER (WHERE al.status_code >= 400) AS errors, \
                    percentile_cont(0.5) WITHIN GROUP (ORDER BY al.duration_ms) \
                      FILTER (WHERE al.status_code BETWEEN 200 AND 299) AS p50_ms, \
                    percentile_cont(0.95) WITHIN GROUP (ORDER BY al.duration_ms) \
                      FILTER (WHERE al.status_code BETWEEN 200 AND 299) AS p95_ms, \
                    count(*) FILTER (WHERE al.cache_hit) AS cache_hits, \
                    count(*) FILTER ( \
                      WHERE al.status_code BETWEEN 200 AND 299 \
                        AND vc.cache_enabled \
                    ) AS cacheable_calls, \
                    grouping(al.endpoint_id) AS is_total \
             FROM dataapi.access_log al \
             LEFT JOIN dataapi.endpoints e ON e.id = al.endpoint_id \
             LEFT JOIN version_cache vc \
                    ON vc.endpoint_id = al.endpoint_id AND vc.version = al.version \
             WHERE al.workspace_id = $1 AND al.ts >= $2 AND al.actor_user_id IS NULL \
             GROUP BY GROUPING SETS ((), (al.endpoint_id))",
        )
        .bind(ws)
        .bind(since)
        .fetch_all(&self.db.pool)
        .await?;

        let mut total = ActivityRow {
            endpoint_id: None,
            endpoint_name: None,
            calls: 0,
            errors: 0,
            p50_ms: None,
            p95_ms: None,
            cache_hits: 0,
            cacheable_calls: 0,
        };
        let mut endpoints = Vec::new();
        for r in rows {
            let row = ActivityRow {
                endpoint_id: r.endpoint_id,
                endpoint_name: r.endpoint_name,
                calls: r.calls,
                errors: r.errors,
                p50_ms: r.p50_ms,
                p95_ms: r.p95_ms,
                cache_hits: r.cache_hits,
                cacheable_calls: r.cacheable_calls,
            };
            // `grouping()` is 1 for the empty grouping set — the grand total —
            // and 0 for the per-endpoint rows. It is the only way to tell the
            // total apart from the unmatched bucket: BOTH carry a NULL
            // endpoint_id, and reading NULL as "total" would double-count every
            // 404 into the headline number.
            if r.is_total == 1 {
                total = row;
            } else {
                endpoints.push(row);
            }
        }
        // Busiest first. `sort_by_key` with `Reverse` rather than a hand-written
        // reversed comparator (`clippy::unnecessary_sort_by`) — and `sort_by_key`
        // rather than `sort_unstable_by_key` deliberately: both it and the
        // comparator form are STABLE, so endpoints with equal call counts keep
        // the order the statement returned them in. Switching to the unstable
        // variant would let ties reorder between requests, which is how a list
        // that looks sorted starts shuffling under a reader's eyes.
        endpoints.sort_by_key(|e| std::cmp::Reverse(e.calls));
        Ok(ActivitySummaryDto {
            total,
            endpoints,
            retention_days: self.retention_days,
        })
    }

    /// The call list. One fixed statement; every filter is an optional
    /// predicate, because sqlx 0.9's `query_as` takes a `&'static str` and a
    /// built statement is what P1 spent its whole scope removing.
    ///
    /// **`workspace_id = $1` is not negotiable, including on the `request_id`
    /// lookup.** `access_log_request_id_idx` is deliberately global so support
    /// can find a row without knowing the workspace; a handler that leans on the
    /// index and drops the predicate lets a guessable id read across a tenancy
    /// boundary. The index's shape is not the query's authorization.
    ///
    /// Keyset pagination on `ts` — `OFFSET` on the fastest-growing table in the
    /// schema gets slower every page and can skip rows written mid-scroll.
    ///
    /// `limit` is CLAMPED to `1..=`[`MAX_CALL_PAGE`], never refused; the reason
    /// that differs from `range`'s hard 400 is on
    /// `routes::ActivityCallsQuery::limit`, which is also where a caller reads
    /// it.
    #[allow(clippy::too_many_arguments)]
    pub async fn calls(
        &self,
        ws: Uuid,
        since: DateTime<Utc>,
        endpoint_id: Option<Uuid>,
        status_code: Option<i32>,
        error_code: Option<&str>,
        request_id: Option<&str>,
        include_trials: bool,
        before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<ActivityCallDto>, sqlx::Error> {
        sqlx::query_as(
            "SELECT al.id, al.ts, al.endpoint_id, e.name AS endpoint_name, \
                    al.request_method, al.request_path, al.status_code, al.error_code, \
                    al.duration_ms, al.cache_hit, al.client_id, al.request_id, \
                    al.actor_user_id, \
                    COALESCE(NULLIF(u.name, ''), u.email::text) AS actor_name \
             FROM dataapi.access_log al \
             LEFT JOIN dataapi.endpoints e ON e.id = al.endpoint_id \
             LEFT JOIN users u ON u.id = al.actor_user_id \
             WHERE al.workspace_id = $1 \
               AND al.ts >= $2 \
               AND ($3::uuid IS NULL OR al.endpoint_id = $3) \
               AND ($4::int IS NULL OR al.status_code = $4) \
               AND ($5::text IS NULL OR al.error_code = $5) \
               AND ($6::text IS NULL OR al.request_id = $6) \
               AND ($7::boolean OR al.actor_user_id IS NULL) \
               AND ($8::timestamptz IS NULL OR al.ts < $8) \
             ORDER BY al.ts DESC \
             LIMIT $9",
        )
        .bind(ws)
        .bind(since)
        .bind(endpoint_id)
        .bind(status_code)
        .bind(error_code)
        .bind(request_id)
        .bind(include_trials)
        .bind(before)
        .bind(limit.clamp(1, MAX_CALL_PAGE))
        .fetch_all(&self.db.pool)
        .await
    }
}
