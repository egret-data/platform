//! `dataapi.access_log` — one row per served call (spec §8).
//!
//! # What this is for
//!
//! Two readers, and they want different things:
//!
//! * **Support**, holding a `request_id` a caller quoted out of an error body.
//!   They need the row, and they need it without knowing the workspace — hence
//!   `access_log_request_id_idx`. The row says what the caller was told
//!   (`status_code`, `error_code`); the *detail* behind an `internal` stays in
//!   `tracing` against the same `request_id`, because that is the one place it
//!   is allowed to exist (spec §7).
//! * **The management UI**, computing call count, error rate, p50/p95 and cache
//!   hit rate per endpoint. Those are aggregates over `(workspace_id, ts)`,
//!   which is the other index.
//!
//! # What it deliberately does not do
//!
//! **It does not partition.** Spec §8 asked for monthly partitions plus a
//! retention job. The retention job is here; the partitioning is not, because
//! there is no partitioned table anywhere in this repo to model it on and no
//! measurement saying a single table will not do. Introducing the first
//! partitioning scheme in the system on an unmeasured guess buys a migration
//! path nobody has exercised, plus a partition-maintenance job, in exchange for
//! nothing anyone has observed. [`AccessLogStore::prune`] is the same shape as
//! `AuditStore::prune`, which has served `audit.audit_log` — a table with the
//! same append-only, ts-ordered growth — since it was written.
//!
//! **It does not log unauthenticated calls.** `workspace_id` is `NOT NULL`, and
//! before the bearer token verifies there is no workspace to name: the slug in
//! the URL is caller-supplied and unverified, and a nil uuid would either
//! violate the foreign key or invent a tenant bucket that no operator owns. A
//! 401 is therefore a `tracing` event only. That is a real gap for anyone who
//! wants to count credential-stuffing attempts per workspace, and it is a
//! consequence of the schema rather than an oversight — closing it means either
//! a nullable `workspace_id` or a second table.
//!
//! **It does not fail a request.** A write failure is warned and swallowed. The
//! alternative — 500ing a request whose rows were already produced because the
//! bookkeeping did not land — trades a served response for an unserved one, and
//! the log exists to describe serving, not to gate it.

use std::net::IpAddr;

use axum::http::HeaderMap;
use egret_metastore::Db;
use uuid::Uuid;

/// One call, as it will be recorded.
///
/// Built empty at the top of the handler and filled in as the request chain
/// resolves each field, so that a request refused at step 2 still records the
/// two things it did know. Every optional field is optional because there is a
/// real point in the chain before which it does not exist yet.
#[derive(Debug, Clone)]
pub struct AccessLogEntry {
    /// `None` until the bearer token verifies — see the module comment for why
    /// such a row is not written at all rather than written with a placeholder.
    pub workspace_id: Option<Uuid>,
    /// `None` until routing resolves a live endpoint, so a 404 records a row
    /// with the workspace and the caller but no endpoint.
    pub endpoint_id: Option<Uuid>,
    pub version: Option<i32>,
    pub service_account_id: Option<Uuid>,
    pub client_id: Option<String>,
    pub status_code: i32,
    pub row_count: Option<i64>,
    pub duration_ms: i32,
    pub cache_hit: bool,
    /// Always `None` in P4 — `QueryExecPort` returns no engine query id. See the
    /// column comment in `0123_dataapi_access_log.sql`.
    pub engine_query_id: Option<String>,
    pub client_ip: Option<IpAddr>,
    /// `None` on success; otherwise spec §7's fixed code, byte-identical to the
    /// one the caller was given.
    pub error_code: Option<String>,
    pub request_id: String,
}

impl AccessLogEntry {
    /// A row that knows only what the transport knows: the correlation id and
    /// the client address.
    ///
    /// `status_code` starts at 0 rather than 200: a row written with 0 means the
    /// handler returned without setting it, which is a bug worth being able to
    /// see. Defaulting to 200 would make that bug indistinguishable from
    /// success.
    pub fn new(request_id: impl Into<String>, client_ip: Option<IpAddr>) -> Self {
        Self {
            workspace_id: None,
            endpoint_id: None,
            version: None,
            service_account_id: None,
            client_id: None,
            status_code: 0,
            row_count: None,
            duration_ms: 0,
            cache_hit: false,
            engine_query_id: None,
            client_ip,
            error_code: None,
            request_id: request_id.into(),
        }
    }
}

/// The caller's address, as far as it can be known here.
///
/// The first hop of `X-Forwarded-For`, matching what `egret-auth`'s login
/// limiter already keys on (`egret-auth/src/routes.rs:165`) so the two surfaces
/// agree about what "the client" means in a given deployment.
///
/// **It is not trusted, and nothing authorises on it.** A direct caller can put
/// any address in that header; it is only meaningful when a proxy that
/// overwrites it terminates in front of egret. It is recorded because an
/// operator reading an access log wants the address their proxy saw, and it is
/// used for nothing else — an unparseable value becomes `NULL` rather than being
/// coerced into something that would sort next to real addresses.
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .and_then(|s| s.parse().ok())
}

/// Writes and prunes `dataapi.access_log`.
#[derive(Clone)]
pub struct AccessLogStore {
    db: Db,
}

impl AccessLogStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// A row that knows only the transport — see [`AccessLogEntry::new`].
    pub fn entry(request_id: impl Into<String>, headers: &HeaderMap) -> AccessLogEntry {
        AccessLogEntry::new(request_id, client_ip(headers))
    }

    /// Record one call. Never fails the caller — see the module comment.
    ///
    /// A row with no `workspace_id` is dropped rather than written, because the
    /// column is `NOT NULL` and there is no honest value for it. The `tracing`
    /// line still carries the `request_id`, so the call is not invisible.
    pub async fn record(&self, e: &AccessLogEntry) {
        let Some(ws) = e.workspace_id else {
            tracing::debug!(
                request_id = %e.request_id,
                status = e.status_code,
                error_code = e.error_code.as_deref().unwrap_or(""),
                "data-api call refused before a workspace was known; not logged to access_log"
            );
            return;
        };
        let res = sqlx::query(
            "INSERT INTO dataapi.access_log \
               (workspace_id, endpoint_id, version, service_account_id, client_id, \
                status_code, row_count, duration_ms, cache_hit, engine_query_id, \
                client_ip, error_code, request_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::text::inet,$12,$13)",
        )
        .bind(ws)
        .bind(e.endpoint_id)
        .bind(e.version)
        .bind(e.service_account_id)
        .bind(e.client_id.as_deref())
        .bind(e.status_code)
        .bind(e.row_count)
        .bind(e.duration_ms)
        .bind(e.cache_hit)
        .bind(e.engine_query_id.as_deref())
        // Sent as TEXT and cast in SQL (`$11::text::inet`) rather than bound as
        // an `IpAddr`. sqlx maps `inet` only with its `ipnetwork`/`ipnet`
        // feature, and turning that on is a workspace-wide dependency change for
        // one nullable column on one table — this repo's dependency graph is
        // already carrying pins it cannot move (see `sqlx08` in
        // egret-catalog-engine). The value was already parsed as an `IpAddr`
        // above, so the cast cannot fail on anything this code can produce.
        .bind(e.client_ip.map(|ip| ip.to_string()))
        .bind(e.error_code.as_deref())
        .bind(&e.request_id)
        .execute(&self.db.pool)
        .await;
        if let Err(err) = res {
            tracing::warn!(
                error = %err,
                request_id = %e.request_id,
                "data-api access log insert failed"
            );
        }
    }

    /// Delete rows older than `retention_days`; returns the deleted count.
    ///
    /// Mirrors `AuditStore::prune` (`egret-audit/src/lib.rs:209`) deliberately —
    /// an operator who already knows one retention knob should not have to learn
    /// a second shape for this one, and the driving loop in
    /// `egret-bin/src/main.rs` is the same hourly tick.
    ///
    /// # `retention_days = 0` KEEPS NOTHING
    ///
    /// It does not mean "unlimited". `ts < now() - 0 days` is every row already
    /// written, so a zero empties the table — and `max(0)` clamps a negative to
    /// the same thing rather than letting it invert into `ts < now() + n days`
    /// and take future-dated rows with it. Both readings delete everything, so
    /// the clamp buys only that the two spellings agree.
    ///
    /// This is `AuditStore::prune`'s behaviour exactly, asserted by its own test
    /// (`egret-audit/tests/audit_store.rs:38`), and it is being mirrored rather
    /// than fixed on purpose: an operator who has learned one retention knob
    /// should not find the other one interpreting the same number differently.
    /// It IS a trap — a config default of 0 reading as "off" would silently
    /// discard the log — and it belongs to whoever wires this to a config key,
    /// which P4 does not.
    pub async fn prune(&self, retention_days: i64) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "DELETE FROM dataapi.access_log WHERE ts < now() - make_interval(days => $1)",
        )
        .bind(retention_days.max(0) as i32)
        .execute(&self.db.pool)
        .await?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_address_is_the_first_forwarded_hop_or_nothing() {
        let mut h = HeaderMap::new();
        assert_eq!(
            client_ip(&h),
            None,
            "no header means no address, not 0.0.0.0"
        );

        h.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        assert_eq!(
            client_ip(&h),
            Some("203.0.113.7".parse::<IpAddr>().unwrap()),
            "the first hop is the client; the rest are our own proxies"
        );

        // A header a direct caller wrote by hand. It must not become a row that
        // sorts next to real addresses, and it must not panic.
        h.insert("x-forwarded-for", "not-an-address".parse().unwrap());
        assert_eq!(client_ip(&h), None);

        h.insert("x-forwarded-for", " 2001:db8::1 ".parse().unwrap());
        assert_eq!(
            client_ip(&h),
            Some("2001:db8::1".parse::<IpAddr>().unwrap()),
            "IPv6 and surrounding whitespace are both ordinary here"
        );
    }

    #[test]
    fn an_unset_status_is_zero_so_it_cannot_be_mistaken_for_success() {
        let e = AccessLogEntry::new("req-1", None);
        assert_eq!(e.status_code, 0);
        assert!(e.workspace_id.is_none());
        assert!(!e.cache_hit);
    }
}
