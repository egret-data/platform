//! Data API P5 §4 — what `dataapi.access_log` can tell an operator.

use chrono::{DateTime, Duration, Utc};
use egret_metastore::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    pub async fn summary(
        &self,
        ws: Uuid,
        since: DateTime<Utc>,
    ) -> Result<ActivitySummaryDto, sqlx::Error> {
        let rows: Vec<(Option<Uuid>, Option<String>, i64, i64, Option<f64>, Option<f64>, i64, i64, i32)> =
            sqlx::query_as(
                "SELECT al.endpoint_id, \
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
                            AND (ev.snapshot -> 'cache' ->> 'enabled')::boolean IS TRUE \
                        ) AS cacheable_calls, \
                        grouping(al.endpoint_id) AS is_total \
                 FROM dataapi.access_log al \
                 LEFT JOIN dataapi.endpoints e ON e.id = al.endpoint_id \
                 LEFT JOIN dataapi.endpoint_versions ev \
                        ON ev.endpoint_id = al.endpoint_id AND ev.version = al.version \
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
                endpoint_id: r.0,
                endpoint_name: r.1,
                calls: r.2,
                errors: r.3,
                p50_ms: r.4,
                p95_ms: r.5,
                cache_hits: r.6,
                cacheable_calls: r.7,
            };
            // `grouping()` is 1 for the empty grouping set — the grand total —
            // and 0 for the per-endpoint rows. It is the only way to tell the
            // total apart from the unmatched bucket: BOTH carry a NULL
            // endpoint_id, and reading NULL as "total" would double-count every
            // 404 into the headline number.
            if r.8 == 1 {
                total = row;
            } else {
                endpoints.push(row);
            }
        }
        endpoints.sort_by(|a, b| b.calls.cmp(&a.calls));
        Ok(ActivitySummaryDto {
            total,
            endpoints,
            retention_days: self.retention_days,
        })
    }
}
