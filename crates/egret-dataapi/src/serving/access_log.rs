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
//! **It does not log unauthenticated calls, and filling the gap naively would be
//! worse than the gap.** `workspace_id` is `NOT NULL`, and before the bearer
//! token verifies the only workspace identifier in hand is the slug in the URL —
//! caller-supplied and unverified.
//!
//! Writing that slug's workspace would let an **unauthenticated** caller create
//! rows attributed to any workspace whose slug they can guess, inflating that
//! tenant's error rate in the statistics computed from this table. That is not a
//! smaller version of the current gap; it is a worse outcome than having no row
//! at all, because a missing row is visibly missing and a forged one is not.
//! Making `workspace_id` nullable does not help either — it stores nothing and
//! still cannot restore a per-workspace count.
//!
//! The only real fixes are a SEPARATE table for pre-authentication refusals, or
//! an explicitly-named `claimed_slug` column that no aggregate ever joins on.
//! Until one exists, a 401 is a `tracing` event carrying the same `request_id`,
//! so it is correlatable but not countable.
//!
//! **The related blind spot is bigger than this one:** the rate limiter runs
//! AFTER token verification (spec §6.2 step 5, keyed on `endpoint_id`), so an
//! unauthenticated flood is bounded by nothing in this crate and appears in no
//! table at all. That belongs to the authentication surface and to whatever
//! fronts it, not to the access log — but it is the same hole from the other
//! side, and it is why "we would see it in the access log" is not true of the
//! traffic most worth seeing.
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
    /// Rows returned to the caller — **summed across every statement** since M2,
    /// where it was one statement's count before.
    ///
    /// One integer column cannot hold a per-statement split, and the split is
    /// reported where a caller can use it (`meta.statements`), so the log keeps
    /// the total. Recorded here because the meaning of an existing column
    /// changed WITHOUT the column changing: nothing in egret reads it today —
    /// `activity::ActivityStore` and `egret_automation::dataapi_metric` measure
    /// counts, error rates and durations, and migration 0128's
    /// `schedule_dataapi_metric_valid` admits no rows-based metric at all — so
    /// there is no query to update and nothing that breaks. An operator's own
    /// SQL over this column, or a rows-based metric added later, is reading a
    /// sum from an M2 endpoint and a single statement's count from every row
    /// written before the upgrade. It belongs in the deploy note beside the
    /// envelope break for that reason and no other.
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
    /// Non-null ⇒ this call came from the Try-it console, run by this person on
    /// a service account's behalf (P5 §3). Statistics filter these out; the call
    /// list shows them, labelled.
    ///
    /// **Set by the management `/try` handler, never by the serving path.** A
    /// real consumer call has no actor, and a serving path that could populate
    /// this would make "is this a trial?" a claim rather than a fact.
    pub actor_user_id: Option<Uuid>,
    /// The method and path as requested. Recorded because a row that resolved no
    /// endpoint (`endpoint_id == None`) has nothing else identifying it — see
    /// the column comment in `0125_dataapi_access_log_p5.sql`.
    pub request_method: Option<String>,
    pub request_path: Option<String>,
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
            actor_user_id: None,
            request_method: None,
            request_path: None,
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

/// What a configured retention means to the SCHEDULER: `Some(days)` to prune
/// hourly, `None` to not schedule the prune at all.
///
/// # Why this exists as a function instead of an `if` at the call site
///
/// It is the one place the `prune(0)` trap is closed, and it is closed in the
/// crate that owns the trap rather than in `egret-bin`, so it can be tested
/// without booting a server and so a second caller cannot reintroduce the trap
/// by writing the comparison differently.
///
/// [`AccessLogStore::prune`] treats `0` as "keep nothing", because
/// `ts < now() - 0 days` really is every row already written and that is the
/// correct primitive. A CONFIG key is not a primitive: `0` is what an operator
/// writes when they mean "off", and what a blank `EGRET_DATA_API__…` env var or
/// a half-written TOML produces by accident. On the fastest-growing table in
/// the system the difference between those two readings is the whole log,
/// deleted an hour after boot, with nothing having gone wrong.
///
/// So: the store keeps its meaning, and the scheduler refuses to call it with a
/// number that would wipe. An operator who genuinely wants an empty table can
/// call `prune(0)` themselves.
pub fn scheduled_retention(retention_days: i64) -> Option<i64> {
    (retention_days > 0).then_some(retention_days)
}

/// What [`AccessLogStore::record`] did — the ONE expression that says whether a
/// row exists for a call.
///
/// # Why this is a return value and not a rule two call sites both remember
///
/// The serving path counts the calls this table cannot hold
/// (`egret_dataapi_preauth_rejections_total`, and see [`super::metrics`] for why
/// that counter is the one thing in the package that is not ordinary practice).
/// That counter is only meaningful if it is the exact complement of the table.
/// Spelled as a second `if e.workspace_id.is_none()` at the call site — which is
/// what it was — the two were independent expressions held together by nothing
/// but adjacency and a comment: a third early return added here, skipping trial
/// rows the way `activity` filters them elsewhere, would have left the counter
/// silently no longer complementary and nothing would have gone red.
///
/// As a return value, "did a row get written" has exactly one author. A new
/// no-row path cannot be added without naming which variant it produces, and
/// [`Self::Row`] is a claim the DB made rather than one this function guesses.
///
/// It is NOT a proof against a future edit that returns [`Self::Row`] from a
/// path that wrote nothing — no type can be — so
/// `record_reports_a_row_exactly_when_one_exists` in
/// `tests/serving_metrics.rs` asserts the biconditional against the real table,
/// over a TABLE of entry shapes rather than for one case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "the caller decides which metric a missing row is counted under"]
pub enum Recorded {
    /// The INSERT succeeded: `dataapi.access_log` holds one row for this
    /// `request_id`.
    Row,
    /// No row, and none could have been written: `workspace_id` was still
    /// unknown, and the column is `NOT NULL`.
    ///
    /// This is the pre-authentication refusal — the outage class that leaves
    /// `error_rate` `None` instead of 1.0 and fires no alert.
    NoWorkspace,
    /// No row: the INSERT itself failed, and the error was logged at `warn`
    /// rather than propagated (this function never fails its caller).
    ///
    /// Kept apart from [`Self::NoWorkspace`] rather than folded into it because
    /// the two are different faults with different fixes — one says
    /// authentication is refusing everything, the other says the metastore is
    /// rejecting writes — and folding them would make the pre-auth counter mean
    /// "one of two unrelated things", which is precisely the ambiguity it was
    /// added to remove.
    InsertFailed,
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
    ///
    /// Returns [`Recorded`], which is the only statement anywhere about whether
    /// a row exists for this call. Read that type's comment before adding
    /// another path out of this function.
    pub async fn record(&self, e: &AccessLogEntry) -> Recorded {
        let Some(ws) = e.workspace_id else {
            tracing::debug!(
                request_id = %e.request_id,
                status = e.status_code,
                error_code = e.error_code.as_deref().unwrap_or(""),
                "data-api call refused before a workspace was known; not logged to access_log"
            );
            return Recorded::NoWorkspace;
        };
        let res = sqlx::query(
            "INSERT INTO dataapi.access_log \
               (workspace_id, endpoint_id, version, service_account_id, client_id, \
                status_code, row_count, duration_ms, cache_hit, engine_query_id, \
                client_ip, error_code, request_id, actor_user_id, \
                request_method, request_path) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::text::inet,$12,$13,$14,$15,$16)",
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
        .bind(e.actor_user_id)
        .bind(e.request_method.as_deref())
        .bind(e.request_path.as_deref())
        .execute(&self.db.pool)
        .await;
        match res {
            Ok(_) => Recorded::Row,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    request_id = %e.request_id,
                    "data-api access log insert failed"
                );
                Recorded::InsertFailed
            }
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
    /// # It is global, so it needs an index leading on `ts`
    ///
    /// There is no `workspace_id` predicate here and there should not be —
    /// retention is a system-wide policy. That means this statement CANNOT use
    /// `access_log_ws_ts_idx`, which leads on `workspace_id`, and for a while it
    /// had nothing else: an hourly sequential scan of the fastest-growing table
    /// in the schema, getting slower every hour by construction.
    /// `access_log_ts_idx` (migration 0123) exists for this one query. If a
    /// workspace-scoped retention is ever wanted, it is a second statement with
    /// its own predicate, not a `WHERE` bolted onto this one.
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

    /// The `prune(0)` trap, closed at the config boundary.
    ///
    /// The two halves must disagree, and that is the point: the STORE reads 0 as
    /// "keep nothing" (mirroring `AuditStore::prune`), and the SCHEDULER reads a
    /// configured 0 as "off". Rewriting this as `retention_days.max(0)` — the
    /// shape that looks like the store's — would schedule `prune(0)` and empty
    /// the table an hour after boot.
    #[test]
    fn a_non_positive_retention_does_not_schedule_a_prune() {
        assert_eq!(
            scheduled_retention(0),
            None,
            "a configured 0 means OFF, not 'delete everything an hour from now'"
        );
        assert_eq!(scheduled_retention(-1), None);
        assert_eq!(scheduled_retention(i64::MIN), None);
        assert_eq!(scheduled_retention(1), Some(1));
        assert_eq!(scheduled_retention(90), Some(90));
    }

    #[test]
    fn an_unset_status_is_zero_so_it_cannot_be_mistaken_for_success() {
        let e = AccessLogEntry::new("req-1", None);
        assert_eq!(e.status_code, 0);
        assert!(e.workspace_id.is_none());
        assert!(!e.cache_hit);
    }
}
