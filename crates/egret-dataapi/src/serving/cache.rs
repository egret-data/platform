//! Step 6 of spec §6.2 — the response cache, and the one check that keeps a
//! `shared` one honest.
//!
//! # Why the key carries the principal
//!
//! `execute_as` runs the SQL AS the service account, and Ranger filters rows and
//! columns per principal. Two identities can therefore call the same endpoint
//! with the same parameters and legitimately receive different rows — that is
//! the design working, not an edge case. A cache key without the principal
//! serves one identity's rows to another **through the exact control the design
//! leans on**, and it does it with a 200 that looks like every other 200.
//!
//! So [`cache_key`] takes the principal, and the ONLY way it is absent is
//! [`shared_cache_allowed`] returning `true`.
//!
//! # `delegated` is not the switch — there are TWO, and this got it wrong
//!
//! Read this before touching either check. The condition that makes a shared
//! cache unsafe is "**the engine runs the query as the caller**", and TWO
//! independent, unvalidated columns on `query.databases` each cause it:
//!
//! * `delegated`, and
//! * `options.impersonate` — `delegation_for`'s non-delegated arm still passes
//!   the caller as the session user (`egret-query/src/execution/mod.rs:86`), and
//!   both connectors act on the flag on their own:
//!   `connector/trino.rs:43` sends `X-Trino-User: <caller>`, and
//!   `connector/starrocks.rs:129` issues `EXECUTE AS <caller> WITH NO REVERT`.
//!
//! `DatabaseInput` accepts `{"delegated": false, "options": {"impersonate":
//! true}}`, so a connection can be fully per-principal at the engine while
//! reading as non-delegated to anything that consults `delegated` alone.
//!
//! **Both checks used to consult `delegated` alone, and that was a live
//! cross-identity leak**, not a theoretical one: such a connection plus
//! `cache.scope: shared` passed publish-time validation, principal A's
//! Ranger-filtered rows were stored under a principal-LESS key, and principal B
//! — different policy, perfectly legitimate `:invoke` grant — was served A's
//! rows with a 200 and `meta.cached: true`. Nothing in `access_log`
//! distinguished it, and the frozen witness stayed silent because live
//! `delegated` still equalled frozen `delegated`.
//!
//! [`crate::lifecycle::runs_per_principal`] is now the single expression of the
//! condition, and both ends route through it.
//!
//! # Why `shared` is re-checked at serve time
//!
//! [`crate::lifecycle::check_cache_scope`] refuses `scope = shared` on a
//! per-principal connection at publish time, so the unsafe SHAPE never enters
//! `endpoint_versions`. That is not the whole problem: BOTH columns are mutable,
//! and switching either on after a `shared` version was published turns that
//! already-frozen version into a cross-principal leak, with no re-validation
//! running anywhere. Validate-time checking cannot close it, which is why
//! [`egret_dataapi_api::EndpointSnapshot::delegated`] and
//! [`egret_dataapi_api::EndpointSnapshot::impersonate`] freeze what the
//! connection was — a witness PAIR, and a witness nobody questions is
//! decoration. Half a witness is worse: it reads as though the question were
//! being asked.
//!
//! On disagreement in EITHER half this **refuses**
//! ([`ServingError::CacheScopeStale`], 503) rather than quietly falling back to
//! a per-principal key. A silent downgrade would keep serving a configuration
//! the operator no longer has, and hide the drift from the only person who can
//! fix it (re-publish, or put the connection back). The failure is loud on
//! purpose.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egret_core::UserId;
use egret_dataapi_api::{CacheScope, EndpointSnapshot};
use egret_metastore::Db;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::ServingError;

/// Upper bound applied to a snapshot's `cache.ttl_seconds` before it is acted
/// on. Author input frozen with no validation rule anywhere (spec §5.3 and §7),
/// exactly like `limits` — a `u32::MAX` TTL is a permanent answer to a query
/// whose data moves.
const TTL_CEILING_SECS: u64 = 3_600;

/// Total bytes the in-process cache will hold. A served response can be up to
/// `limits.max_rows` rows, so an unbounded map is a memory fault waiting for the
/// first wide endpoint. Over budget the OLDEST entries are evicted — see
/// [`ResponseCache::put_at`] for why declining instead was a cross-tenant
/// availability bug.
const CACHE_BYTE_BUDGET: usize = 64 * 1024 * 1024;

/// Largest single response that may be cached. One enormous entry must not be
/// able to evict every small one, and re-running one wide query is cheaper than
/// pinning 64 MiB for its TTL.
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;

/// Entries held at once, across every endpoint and workspace in the process.
///
/// A second bound beside the byte budget because they bind at different times:
/// 4096 one-row responses are a rounding error in bytes and still 4096 hash
/// entries plus 4096 live `Arc`s. The rate-limit buckets carry the same kind of
/// cap (`PRUNE_ABOVE`) for the same reason.
const MAX_ENTRIES: usize = 4_096;

/// What a cached response is keyed by.
///
/// `principal` is present unless the snapshot says `shared` AND the delegated
/// re-check passed — see the module comment for why that is the whole safety
/// argument.
///
/// # Why two identities cannot collide, structurally
///
/// The hashed stream is `endpoint(16) ‖ version(4) ‖ params ‖ principal(16)`,
/// and the principal is the LAST 16 bytes. Two byte streams that are equal are
/// equal in length, so `params_a ‖ A == params_b ‖ B` forces `A == B` — no
/// arrangement of parameter bytes can push one identity's suffix out of
/// position, whatever the parameters contain. That is a property of the layout
/// rather than of the delimiting, and it holds even if everything below were
/// wrong.
///
/// The `b"shared"` marker for an absent principal is therefore
/// defence-in-depth, not the load-bearing part: `shared` vs `per_principal` is
/// frozen in the snapshot, so one endpoint version can never mint keys in both
/// spaces to begin with.
///
/// # Why every field is LENGTH-PREFIXED and not delimited
///
/// The first version delimited with `0x00`, which was safe only because
/// `http::Uri` refuses that byte in a raw query string — a property held by a
/// crate this module never mentions, and one that evaporated the moment step 7
/// began hashing DECODED parameters — which it now does (P4.5). Under the old
/// scheme `?a=1%00b%002` would hash byte-identically to `?a=1&b=2`. Same
/// principal both times, so never a cross-identity leak, but once parameters
/// reach the SQL that is one filter's rows served for another's — and a
/// parameter is very often a tenant or a date.
///
/// A `u64` length before every field removes the question rather than relying on
/// an alphabet: no byte is special, so no value can be spelled to look like
/// structure. The per-key value LIST is counted the same way, which is why
/// repeated parameters are a `Vec` here and not a joined string — a joiner has
/// exactly the forgeability this construction exists to remove.
///
/// **The endpoint's identity is its `endpoint_id`, never its request path.**
/// axum percent-decodes the wildcard capture, so `reports%2Fdaily` and
/// `reports/daily` reach the same endpoint; keying on the raw capture would give
/// one endpoint two entries that never hit each other.
pub fn cache_key(
    endpoint_id: Uuid,
    version: i32,
    params: &BTreeMap<String, Vec<String>>,
    principal: Option<UserId>,
) -> String {
    let mut h = Sha256::new();
    h.update(endpoint_id.as_bytes());
    h.update(version.to_le_bytes());
    h.update((params.len() as u64).to_le_bytes());
    // A `BTreeMap` so the iteration order is the KEY's and not the request's:
    // `?a=1&b=2` and `?b=2&a=1` are the same call and must hit the same entry.
    for (k, values) in params {
        h.update((k.len() as u64).to_le_bytes());
        h.update(k.as_bytes());
        h.update((values.len() as u64).to_le_bytes());
        for v in values {
            h.update((v.len() as u64).to_le_bytes());
            h.update(v.as_bytes());
        }
    }
    match principal {
        Some(p) => h.update(p.0.as_bytes()),
        None => h.update(b"shared"),
    }
    format!("{:x}", h.finalize())
}

/// Decide whether this request may use a shared (principal-free) cache entry.
///
/// Returns `Ok(false)` for every `per_principal` snapshot without touching the
/// database — the common case costs nothing. For a `shared` one it reads the
/// LIVE `(delegated, options.impersonate)` pair and compares it against the
/// frozen witness pair.
///
/// **Deliberately not gated on `cache.enabled`.** `check_cache_scope` refuses a
/// `shared` scope at publish time whether or not the cache is currently on,
/// because the flag is one draft edit away from `true`; the serving side takes
/// the same position, so an operator sees the drift when it happens rather than
/// when someone enables the cache.
///
/// The cost of that choice, stated rather than left to be discovered: an
/// endpoint with `enabled: false` and a leftover `scope: shared` **hard-503s on
/// a delegation flip even though it caches nothing and provably cannot leak.**
/// The decision stands — the safe-looking configuration is one edit from the
/// unsafe one, and a refusal an operator can act on beats a latent one — but it
/// is a false positive and it is written down in spec §6.3 as one.
///
/// # Cost, per request
///
/// This is a metastore round-trip on EVERY call to a `shared` endpoint,
/// including calls that would have been cache hits. The placement is deliberate:
/// checking after the hit would mean serving the stale entry first and asking
/// afterwards, which is the leak. It is also the reason the check returns early
/// for `per_principal` — the common case must not pay for it.
///
/// If that round-trip ever matters, memoise
/// `(database_id → (delegated, impersonate))` with a short TTL rather than
/// moving the check: the window a memo opens is bounded and stated, while a
/// re-ordered check is unbounded. Nothing memoises today, because a single
/// indexed primary-key read next to a query execution is not yet worth the
/// staleness.
///
/// # Why this compares a PAIR rather than refusing on live `impersonate`
///
/// Kept because the rejected option is the one a reader will reach for — it is
/// two lines and obviously safe. Refusing whenever the live row has
/// `impersonate == true`, regardless of `delegated`, IS safe. But it answers
/// [`ServingError::CacheScopeStale`], whose entire meaning to an operator is
/// "the published configuration has drifted and a re-publish clears it" — and
/// for that condition a re-publish would be refused too, by
/// [`crate::lifecycle::check_cache_scope`]. The operator would be sent to do a
/// thing that cannot work, and told the same wrong thing again when it failed.
///
/// The pair makes publish refuse the combination up front, with a message naming
/// `options.impersonate` rather than a `delegated` the author can see is
/// `false` — so `cache_scope_stale` keeps meaning only what it says: something
/// changed after publish, and re-publishing against what is there now will
/// succeed.
pub async fn shared_cache_allowed(db: &Db, snap: &EndpointSnapshot) -> Result<bool, ServingError> {
    if snap.cache.scope != CacheScope::Shared {
        return Ok(false);
    }

    // `fetch_optional`, not `fetch_one`: a deleted database is not a "no rows"
    // panic-shaped error, and it is certainly not a reason to serve a shared
    // entry. It is the same refusal — the configuration the version was
    // published against no longer exists.
    //
    // BOTH columns, in ONE statement. Two reads would let the pair come from two
    // different moments, and reading only `delegated` — which is what this did —
    // was the leak: `options.impersonate` makes the engine run as the caller
    // independently of `delegated`, so a connection could be per-principal at
    // the engine and look shared-safe here forever.
    let live: Option<(bool, serde_json::Value)> =
        sqlx::query_as("SELECT delegated, options FROM query.databases WHERE id = $1")
            .bind(snap.database_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(ServingError::internal)?;

    let Some((live_delegated, live_options)) = live else {
        tracing::warn!(
            database_id = %snap.database_id,
            "a shared-cache endpoint's database row is gone; refusing rather than serving it"
        );
        return Err(ServingError::CacheScopeStale);
    };
    let live_impersonate = crate::lifecycle::impersonate_flag(&live_options);

    // Disagreement in EITHER half is a refusal, not a downgrade to
    // per-principal: the operator published a configuration that is no longer
    // safe, and silently serving it a different way hides that from them.
    //
    // The comparison is on the pair rather than on `runs_per_principal`'s
    // derived boolean, so the log names the field that actually moved. For a
    // `shared` endpoint the derived value would do — publish refuses `shared`
    // unless both halves are false, so any drift to true is caught either way —
    // but "delegated changed" and "impersonate was switched on" send an operator
    // to two different settings, and this is the one line they will read.
    if live_delegated != snap.delegated || live_impersonate != snap.impersonate {
        tracing::warn!(
            database_id = %snap.database_id,
            frozen_delegated = snap.delegated,
            live_delegated,
            frozen_impersonate = snap.impersonate,
            live_impersonate,
            "cache.scope = shared was published against a different \
             (delegated, options.impersonate) pair than the connection now has; refusing"
        );
        return Err(ServingError::CacheScopeStale);
    }
    // Belt and braces: the pair agreeing means the connection is what publish
    // validated, and publish refuses `shared` on a per-principal connection — so
    // this can only be false via a snapshot that predates the check. Returning
    // `!runs_per_principal` rather than a bare `true` means the safety property
    // is re-derived here instead of being inferred from "nothing changed".
    Ok(!crate::lifecycle::runs_per_principal(
        live_delegated,
        live_impersonate,
    ))
}

/// How long this snapshot's responses may be cached, or `None` for "do not
/// cache". The ONE place that decides whether the cache is consulted at all.
///
/// **`ttl_seconds: 0` means do not cache, even with `enabled: true`.** Nothing
/// validates it, and an earlier version clamped it into `1..=TTL_CEILING`, so an
/// author spelling "don't cache this" as a zero TTL got a one-second cache —
/// the one place where a published `0` silently became non-zero behaviour.
/// Every other clamp in the serving path widens a nonsensical value into a
/// working one; this one would have invented a caching policy the author
/// declined.
///
/// A too-LARGE TTL is still clamped rather than refused, because there the
/// author's intent ("cache for a long time") is honoured, just bounded.
pub fn cache_ttl(snap: &EndpointSnapshot) -> Option<Duration> {
    if !snap.cache.enabled || snap.cache.ttl_seconds == 0 {
        return None;
    }
    Some(Duration::from_secs(
        (snap.cache.ttl_seconds as u64).min(TTL_CEILING_SECS),
    ))
}

/// A cached response body: the SHAPED rows plus the row counts that described
/// them.
///
/// Shaped rather than raw, because shaping is where `output.shape`/`naming` and
/// the drift refusal are applied — caching the raw result would re-run all of
/// that on every hit, and (worse) let a hit and a miss disagree about whether
/// the endpoint had drifted.
///
/// # Why the counts are a LIST, and positional
///
/// `meta` reports a row count PER STATEMENT once an endpoint has several (spec
/// §4.4), and a hit runs none of them — so a single total cannot rebuild the
/// body a miss would have produced, and a hit would answer with an empty
/// `statements` map under a `200`.
///
/// Positional rather than keyed by name because the key stream already carries
/// `version` ([`cache_key`]), and a re-publish is the only thing that can change
/// the statement list. An entry therefore belongs to exactly one frozen list,
/// in the order that list was published. The caller still checks the LENGTH
/// before zipping — see `router::run` — because a silent `zip` truncation would
/// drop a statement from `meta` rather than fail.
#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub data: Arc<serde_json::Value>,
    pub row_counts: Vec<u64>,
}

struct Entry {
    data: Arc<serde_json::Value>,
    row_counts: Vec<u64>,
    expires: Instant,
    bytes: usize,
    /// Insertion order, for eviction. Not a timestamp: two entries written in
    /// the same `Instant` still have to be orderable, and `now` is supplied by
    /// the caller so it need not advance between two writes at all.
    seq: u64,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    /// Maintained INCREMENTALLY on every insert and removal. The first version
    /// re-summed every entry on each write while holding this mutex, which made
    /// each write O(n) under exactly the load that grows n.
    bytes: usize,
    seq: u64,
}

impl Inner {
    fn remove(&mut self, key: &str) -> Option<Entry> {
        let e = self.entries.remove(key)?;
        self.bytes = self.bytes.saturating_sub(e.bytes);
        Some(e)
    }

    /// Drop every entry whose TTL has passed. O(n), so it runs only when a limit
    /// is about to bind — expiry is otherwise handled on read.
    fn drop_expired(&mut self, now: Instant) {
        let mut freed = 0usize;
        self.entries.retain(|_, e| {
            let live = e.expires > now;
            if !live {
                freed += e.bytes;
            }
            live
        });
        self.bytes = self.bytes.saturating_sub(freed);
    }

    /// Evict the oldest entry by insertion order. Returns false when there is
    /// nothing left to evict.
    ///
    /// Oldest-first rather than least-recently-used: entries of one endpoint all
    /// carry the same TTL, so insertion order already approximates expiry order,
    /// and an LRU needs a second index maintained on the read path — which is
    /// the path that must stay cheap.
    fn evict_oldest(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.seq)
            .map(|(k, _)| k.clone())
        else {
            return false;
        };
        self.remove(&key);
        true
    }
}

/// In-memory, per-process response cache with an injected clock.
///
/// Per-process is correct while the serving surface is in-process: a second
/// replica simply misses and re-executes, which is slower but never wrong. The
/// key format ([`cache_key`]) is what a shared implementation would reuse.
///
/// `Instant` (monotonic) rather than a wall clock, so an NTP step cannot make a
/// live entry immortal or expire the whole cache at once.
#[derive(Clone, Default)]
pub struct ResponseCache {
    inner: Arc<Mutex<Inner>>,
}

impl ResponseCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A live entry, or `None`. Expiry is checked against `now` on read, so an
    /// entry that outlived its TTL is never served even if nothing has pruned it
    /// yet.
    pub fn get_at(&self, key: &str, now: Instant) -> Option<CachedResponse> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let e = g.entries.get(key)?;
        if e.expires <= now {
            g.remove(key);
            return None;
        }
        Some(CachedResponse {
            data: e.data.clone(),
            row_counts: e.row_counts.clone(),
        })
    }

    /// Store a response under `key` for `ttl`.
    ///
    /// Returns whether it was stored — `false` only for a body larger than
    /// [`MAX_ENTRY_BYTES`], which is a performance outcome and never a
    /// correctness one (the next call misses and re-executes).
    ///
    /// # Why a full cache EVICTS rather than declines
    ///
    /// The first version declined to store once the budget was full, which made
    /// one client's key space everyone else's problem: entries are global to the
    /// process, so a single authenticated caller minting distinct keys could
    /// fill 64 MiB and stop caching for **every endpoint in every workspace**
    /// until those entries lapsed — up to an hour. Correctness was untouched and
    /// that is exactly why it was easy to wave through; it is a cross-tenant
    /// availability effect produced by one client.
    ///
    /// Evicting the oldest entry instead makes the same pressure self-limiting:
    /// the noisy key space evicts ITSELF as fast as it is written, because its
    /// own entries are the newest and the oldest in turn. Two bounds, not one —
    /// a byte budget and an entry count — since many small entries exhaust the
    /// map long before they exhaust the bytes.
    pub fn put_at(
        &self,
        key: String,
        data: Arc<serde_json::Value>,
        row_counts: Vec<u64>,
        ttl: Duration,
        now: Instant,
    ) -> bool {
        let bytes = estimate_bytes(&data);
        if bytes > MAX_ENTRY_BYTES {
            tracing::debug!(bytes, "data-api response too large to cache");
            return false;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Replacing an existing key must release its bytes before the limits are
        // measured, or a re-publish of a large response counts twice.
        g.remove(&key);

        let over =
            |g: &Inner| g.bytes + bytes > CACHE_BYTE_BUDGET || g.entries.len() + 1 > MAX_ENTRIES;
        if over(&g) {
            // Expired entries are reclaimed FIRST, so a live one is never
            // evicted while dead ones are still held. This is the only path that
            // walks the whole map, and it runs only when a limit binds — expiry
            // is otherwise handled on read.
            g.drop_expired(now);
        }
        while over(&g) {
            if !g.evict_oldest() {
                // Unreachable while `bytes <= MAX_ENTRY_BYTES < CACHE_BYTE_BUDGET`
                // and `MAX_ENTRIES >= 1`; a `break` rather than an `unwrap` so a
                // future constant change is a missed cache, not a hang.
                break;
            }
        }

        g.seq += 1;
        let seq = g.seq;
        g.bytes += bytes;
        g.entries.insert(
            key,
            Entry {
                data,
                row_counts,
                expires: now + ttl,
                bytes,
                seq,
            },
        );
        true
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.inner.lock().unwrap().bytes
    }
}

/// Roughly how much memory a cached body occupies.
///
/// Serialising to measure would double the work already done for the wire, so
/// this walks the value instead. It is an estimate and says so — its job is to
/// keep the budget bounded, not to be exact.
fn estimate_bytes(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Null | serde_json::Value::Bool(_) => 8,
        serde_json::Value::Number(_) => 16,
        serde_json::Value::String(s) => s.len() + 16,
        serde_json::Value::Array(a) => 16 + a.iter().map(estimate_bytes).sum::<usize>(),
        serde_json::Value::Object(m) => {
            16 + m
                .iter()
                .map(|(k, val)| k.len() + 16 + estimate_bytes(val))
                .sum::<usize>()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in pairs {
            out.entry(k.to_string()).or_default().push(v.to_string());
        }
        out
    }

    #[test]
    fn the_principal_changes_the_key() {
        // The property the whole module exists for, pinned without a database:
        // same endpoint, same version, same params, two identities.
        let ep = Uuid::new_v4();
        let a = cache_key(
            ep,
            1,
            &params(&[("d", "2026-01-01")]),
            Some(UserId(Uuid::new_v4())),
        );
        let b = cache_key(
            ep,
            1,
            &params(&[("d", "2026-01-01")]),
            Some(UserId(Uuid::new_v4())),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn a_shared_key_does_not_collide_with_the_nil_principal() {
        // Defence-in-depth rather than the load-bearing part: `scope` is frozen
        // in the snapshot, so one endpoint version cannot mint keys in both
        // spaces. Pinned anyway, because "absent" must not be spellable by any
        // principal if that ever stops being true.
        let ep = Uuid::new_v4();
        assert_ne!(
            cache_key(ep, 1, &BTreeMap::new(), None),
            cache_key(ep, 1, &BTreeMap::new(), Some(UserId(Uuid::nil()))),
        );
    }

    #[test]
    fn params_are_order_independent_but_value_sensitive() {
        let ep = Uuid::new_v4();
        let p = UserId(Uuid::new_v4());
        let one = params(&[("a", "1"), ("b", "2")]);
        let other = params(&[("b", "2"), ("a", "1")]);
        assert_eq!(
            cache_key(ep, 1, &one, Some(p)),
            cache_key(ep, 1, &other, Some(p)),
            "a BTreeMap is what makes the request's order irrelevant"
        );
        assert_ne!(
            cache_key(ep, 1, &one, Some(p)),
            cache_key(ep, 1, &params(&[("a", "1"), ("b", "3")]), Some(p)),
        );
        // The field boundaries, pinned: without them `{a: "1", b: "2"}` and
        // `{ab: "12"}` hash the same bytes.
        assert_ne!(
            cache_key(ep, 1, &one, Some(p)),
            cache_key(ep, 1, &params(&[("ab", "12")]), Some(p)),
        );
    }

    #[test]
    fn a_value_containing_the_old_delimiter_bytes_cannot_forge_a_boundary() {
        // The first version delimited with `0x00` and joined repeats with
        // `\u{1}`, which was safe only because `http::Uri` refuses those bytes in
        // a raw query string — a property held by a crate this module never
        // mentions, and one that disappeared the moment step 7 began hashing
        // DECODED values — which it now does (P4.5). These are the exact
        // forgeries that would otherwise have worked:
        //
        //   ?a=1%00b%002  vs  ?a=1&b=2
        //   ?a=%011       vs  ?a=&a=1
        //
        // Same principal in both, so never a cross-identity leak — but once
        // parameters reach the SQL it is one filter's rows served for another's,
        // and a parameter is very often a tenant or a date.
        let ep = Uuid::new_v4();
        let p = Some(UserId(Uuid::new_v4()));
        assert_ne!(
            cache_key(ep, 1, &params(&[("a", "1\u{0}b\u{0}2")]), p),
            cache_key(ep, 1, &params(&[("a", "1"), ("b", "2")]), p),
            "a NUL inside a value must not be able to spell a field boundary"
        );
        assert_ne!(
            cache_key(ep, 1, &params(&[("a", "\u{1}1")]), p),
            cache_key(ep, 1, &params(&[("a", ""), ("a", "1")]), p),
            "a joiner byte inside a value must not be able to spell a repeat"
        );
        // And the general case the length prefixes buy: no split of the same
        // bytes across the key/value boundary can collide.
        assert_ne!(
            cache_key(ep, 1, &params(&[("ab", "cd")]), p),
            cache_key(ep, 1, &params(&[("abc", "d")]), p),
        );
    }

    #[test]
    fn a_repeated_parameter_is_not_the_same_as_one_joined_value() {
        // Repeats are a `Vec` rather than a joined string precisely so this
        // holds without depending on which byte the joiner would have been.
        let ep = Uuid::new_v4();
        let p = Some(UserId(Uuid::new_v4()));
        assert_ne!(
            cache_key(ep, 1, &params(&[("a", "1"), ("a", "2")]), p),
            cache_key(ep, 1, &params(&[("a", "12")]), p),
        );
        // Order WITHIN one parameter's repeats is significant — `?a=1&a=2` and
        // `?a=2&a=1` are different calls to any renderer that reads them.
        assert_ne!(
            cache_key(ep, 1, &params(&[("a", "1"), ("a", "2")]), p),
            cache_key(ep, 1, &params(&[("a", "2"), ("a", "1")]), p),
        );
    }

    #[test]
    fn the_version_is_part_of_the_key() {
        // A re-publish must not serve the previous version's rows: the snapshot
        // (SQL, output shape, columns) changed under the same endpoint id.
        let ep = Uuid::new_v4();
        let p = Some(UserId(Uuid::new_v4()));
        assert_ne!(
            cache_key(ep, 1, &BTreeMap::new(), p),
            cache_key(ep, 2, &BTreeMap::new(), p),
        );
    }

    #[test]
    fn an_entry_expires_at_its_ttl_and_not_before() {
        // Two Instants constructed explicitly and a whole second apart. Calling
        // `Instant::now()` twice could yield readings that coincide, and a test
        // whose two time points can be equal proves nothing about expiry.
        let c = ResponseCache::new();
        let t0 = Instant::now();
        let data = Arc::new(serde_json::json!([{ "n": 1 }]));
        assert!(c.put_at("k".into(), data, vec![1], Duration::from_secs(60), t0));

        assert!(c.get_at("k", t0 + Duration::from_secs(59)).is_some());
        assert!(
            c.get_at("k", t0 + Duration::from_secs(61)).is_none(),
            "an entry past its TTL must not be served"
        );
        assert_eq!(c.len(), 0, "reading an expired entry must reclaim it");
    }

    #[test]
    fn an_oversized_response_is_not_cached() {
        let c = ResponseCache::new();
        let big = Arc::new(serde_json::Value::String("x".repeat(MAX_ENTRY_BYTES + 1)));
        assert!(!c.put_at(
            "k".into(),
            big,
            vec![1],
            Duration::from_secs(60),
            Instant::now()
        ));
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn a_full_cache_evicts_the_oldest_rather_than_declining_to_store() {
        // The cross-tenant availability bug this replaced: `put_at` used to
        // DECLINE once full, so one client minting distinct keys could stop
        // caching for every endpoint in every workspace until its entries
        // lapsed — up to an hour. Evicting makes the pressure self-limiting.
        let c = ResponseCache::new();
        let t0 = Instant::now();
        let d = || Arc::new(serde_json::json!([{ "n": 1 }]));
        let ttl = Duration::from_secs(600);
        for i in 0..MAX_ENTRIES {
            assert!(c.put_at(format!("k{i}"), d(), vec![1], ttl, t0));
        }
        assert_eq!(c.len(), MAX_ENTRIES);

        // One past the cap: stored, and the map has not grown.
        assert!(
            c.put_at("newest".into(), d(), vec![1], ttl, t0),
            "a full cache must still store — declining is what starved everyone else"
        );
        assert_eq!(c.len(), MAX_ENTRIES);
        assert!(
            c.get_at("newest", t0).is_some(),
            "the new entry must be live"
        );
        assert!(
            c.get_at("k0", t0).is_none(),
            "the OLDEST entry is the one that goes"
        );
        assert!(c.get_at("k1", t0).is_some(), "…and only that one");
    }

    #[test]
    fn expired_entries_are_reclaimed_before_a_live_one_is_evicted() {
        // Ordering inside `put_at`: dead entries first, live ones only if the
        // limit still binds. Without it a full cache of expired entries would
        // evict its one live entry to make room.
        let c = ResponseCache::new();
        let t0 = Instant::now();
        let d = || Arc::new(serde_json::json!([{ "n": 1 }]));
        for i in 0..MAX_ENTRIES - 1 {
            c.put_at(format!("old{i}"), d(), vec![1], Duration::from_secs(10), t0);
        }
        // One entry with a long TTL, written last so it is also the NEWEST —
        // eviction by insertion order would keep it anyway. Written first would
        // not distinguish the two rules.
        c.put_at(
            "survivor".into(),
            d(),
            vec![1],
            Duration::from_secs(600),
            t0,
        );
        assert_eq!(c.len(), MAX_ENTRIES);

        // Half a minute later every `old*` entry is dead. The next write must
        // reclaim them wholesale rather than evict one at a time.
        let later = t0 + Duration::from_secs(30);
        c.put_at(
            "fresh".into(),
            d(),
            vec![1],
            Duration::from_secs(600),
            later,
        );
        assert_eq!(c.len(), 2, "only the two live entries remain");
        assert!(c.get_at("survivor", later).is_some());
        assert!(c.get_at("fresh", later).is_some());
    }

    #[test]
    fn replacing_a_key_does_not_double_count_its_bytes() {
        let c = ResponseCache::new();
        let t0 = Instant::now();
        let d = || Arc::new(serde_json::json!([{ "n": 1 }]));
        let ttl = Duration::from_secs(60);
        c.put_at("k".into(), d(), vec![1], ttl, t0);
        let after_first = c.bytes();
        c.put_at("k".into(), d(), vec![1], ttl, t0);
        assert_eq!(c.len(), 1);
        assert_eq!(
            c.bytes(),
            after_first,
            "an overwrite must release the old entry's bytes"
        );
    }

    #[test]
    fn a_zero_ttl_means_do_not_cache_even_when_enabled() {
        // The one place a published 0 could silently become non-zero behaviour:
        // an earlier version clamped into `1..=CEILING`, so an author spelling
        // "don't cache" as a zero TTL got a one-second cache.
        assert_eq!(cache_ttl(&snapshot_with_cache(true, 0)), None);
        assert_eq!(cache_ttl(&snapshot_with_cache(false, 60)), None);
        assert_eq!(
            cache_ttl(&snapshot_with_cache(true, 60)),
            Some(Duration::from_secs(60))
        );
        // A too-large TTL is still clamped rather than refused — there the
        // author's intent is honoured, just bounded.
        assert_eq!(
            cache_ttl(&snapshot_with_cache(true, u32::MAX)),
            Some(Duration::from_secs(TTL_CEILING_SECS))
        );
    }

    fn snapshot_with_cache(enabled: bool, ttl_seconds: u32) -> EndpointSnapshot {
        EndpointSnapshot {
            statements: vec![egret_dataapi_api::StatementSnapshot {
                name: "main".into(),
                sql: "SELECT 1".into(),
                columns: vec![],
            }],
            method: "GET".into(),
            path: "/x".into(),
            database_id: Uuid::nil(),
            params: vec![],
            output: serde_json::from_value(serde_json::json!({})).unwrap(),
            cache: egret_dataapi_api::CacheSpec {
                enabled,
                ttl_seconds,
                scope: CacheScope::PerPrincipal,
            },
            limits: serde_json::from_value(serde_json::json!({})).unwrap(),
            rate_limit: serde_json::from_value(serde_json::json!({})).unwrap(),
            delegated: false,
            impersonate: false,
            param_gate: egret_dataapi_api::PARAM_GATE,
        }
    }
}
