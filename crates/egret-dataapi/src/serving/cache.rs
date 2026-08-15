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
//! # Why `shared` is re-checked at serve time
//!
//! [`crate::lifecycle::check_cache_scope`] refuses `scope = shared` on a
//! delegated connection at publish time, so the unsafe SHAPE never enters
//! `endpoint_versions`. That is not the whole problem: `query.databases.delegated`
//! is a mutable column, and flipping it to `true` after a `shared` version was
//! published turns that already-frozen version into a cross-principal leak, with
//! no re-validation running anywhere. Validate-time checking cannot close it,
//! which is why [`egret_dataapi_api::EndpointSnapshot::delegated`] freezes what
//! the connection was — a witness, and a witness nobody questions is decoration.
//!
//! On disagreement this **refuses** ([`ServingError::CacheScopeStale`], 503)
//! rather than quietly falling back to a per-principal key. A silent downgrade
//! would keep serving a configuration the operator no longer has, and hide the
//! drift from the only person who can fix it (re-publish, or flip the column
//! back). The failure is loud on purpose.

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
/// first wide endpoint; over budget, nothing new is cached (correctness is
/// unaffected — a miss re-executes).
const CACHE_BYTE_BUDGET: usize = 64 * 1024 * 1024;

/// Largest single response that may be cached. One enormous entry must not be
/// able to evict every small one, and re-running one wide query is cheaper than
/// pinning 64 MiB for its TTL.
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;

/// What a cached response is keyed by.
///
/// `principal` is present unless the snapshot says `shared` AND the delegated
/// re-check passed — see the module comment for why that is the whole safety
/// argument.
///
/// Hashed rather than concatenated so the key is a fixed length regardless of
/// how many parameters an endpoint declares, and so no separator question
/// arises from a parameter value containing one.
///
/// **The endpoint's identity is its `endpoint_id`, never its request path.**
/// axum percent-decodes the wildcard capture, so `reports%2Fdaily` and
/// `reports/daily` reach the same endpoint; keying on the raw capture would give
/// one endpoint two entries that never hit each other.
pub fn cache_key(
    endpoint_id: Uuid,
    version: i32,
    params: &BTreeMap<String, String>,
    principal: Option<UserId>,
) -> String {
    let mut h = Sha256::new();
    h.update(endpoint_id.as_bytes());
    h.update(version.to_le_bytes());
    // A `BTreeMap` so the iteration order is the KEY's and not the request's:
    // `?a=1&b=2` and `?b=2&a=1` are the same call and must hit the same entry.
    for (k, v) in params {
        h.update(k.as_bytes());
        h.update([0u8]);
        h.update(v.as_bytes());
        h.update([0u8]);
    }
    match principal {
        Some(p) => h.update(p.0.as_bytes()),
        // A literal marker rather than nothing: without it a shared entry and a
        // per-principal entry for the nil UUID would collide.
        None => h.update(b"shared"),
    }
    format!("{:x}", h.finalize())
}

/// Decide whether this request may use a shared (principal-free) cache entry.
///
/// Returns `Ok(false)` for every `per_principal` snapshot without touching the
/// database — the common case costs nothing. For a `shared` one it reads the
/// LIVE `query.databases.delegated` and compares it against the frozen witness.
///
/// **Deliberately not gated on `cache.enabled`.** `check_cache_scope` refuses a
/// `shared` scope at publish time whether or not the cache is currently on,
/// because the flag is one draft edit away from `true`; the serving side takes
/// the same position, so an operator sees the drift when it happens rather than
/// when someone enables the cache.
pub async fn shared_cache_allowed(db: &Db, snap: &EndpointSnapshot) -> Result<bool, ServingError> {
    if snap.cache.scope != CacheScope::Shared {
        return Ok(false);
    }

    // `fetch_optional`, not `fetch_one`: a deleted database is not a "no rows"
    // panic-shaped error, and it is certainly not a reason to serve a shared
    // entry. It is the same refusal — the configuration the version was
    // published against no longer exists.
    let live: Option<bool> =
        sqlx::query_scalar("SELECT delegated FROM query.databases WHERE id = $1")
            .bind(snap.database_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(ServingError::internal)?;

    let Some(live) = live else {
        tracing::warn!(
            database_id = %snap.database_id,
            "a shared-cache endpoint's database row is gone; refusing rather than serving it"
        );
        return Err(ServingError::CacheScopeStale);
    };

    // Disagreement is a refusal, not a downgrade to per-principal: the operator
    // published a configuration that is no longer safe, and silently serving it
    // a different way hides that from them.
    if live != snap.delegated {
        tracing::warn!(
            database_id = %snap.database_id,
            frozen = snap.delegated,
            live,
            "cache.scope = shared was published against a different `delegated` \
             than the connection now has; refusing"
        );
        return Err(ServingError::CacheScopeStale);
    }
    Ok(!live)
}

/// The snapshot's TTL, clamped. See [`TTL_CEILING_SECS`].
pub fn cache_ttl(snap: &EndpointSnapshot) -> Duration {
    Duration::from_secs((snap.cache.ttl_seconds as u64).clamp(1, TTL_CEILING_SECS))
}

/// A cached response body: the SHAPED rows plus the row count that described
/// them.
///
/// Shaped rather than raw, because shaping is where `output.shape`/`naming` and
/// the drift refusal are applied — caching the raw result would re-run all of
/// that on every hit, and (worse) let a hit and a miss disagree about whether
/// the endpoint had drifted.
#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub data: Arc<serde_json::Value>,
    pub row_count: u64,
}

struct Entry {
    data: Arc<serde_json::Value>,
    row_count: u64,
    expires: Instant,
    bytes: usize,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    bytes: usize,
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
            let bytes = e.bytes;
            g.entries.remove(key);
            g.bytes = g.bytes.saturating_sub(bytes);
            return None;
        }
        Some(CachedResponse {
            data: e.data.clone(),
            row_count: e.row_count,
        })
    }

    /// Store a response under `key` for `ttl`.
    ///
    /// Returns whether it was stored. A refusal (too large, or the budget is
    /// full of live entries) is a performance outcome and never a correctness
    /// one — the next call misses and re-executes.
    pub fn put_at(
        &self,
        key: String,
        data: Arc<serde_json::Value>,
        row_count: u64,
        ttl: Duration,
        now: Instant,
    ) -> bool {
        let bytes = estimate_bytes(&data);
        if bytes > MAX_ENTRY_BYTES {
            tracing::debug!(bytes, "data-api response too large to cache");
            return false;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Prune on write rather than on a timer: without it the map keeps every
        // key ever seen, and expiry-on-read only reclaims keys someone asks for
        // again — which is exactly the set that does NOT need reclaiming.
        g.entries.retain(|_, e| e.expires > now);
        g.bytes = g.entries.values().map(|e| e.bytes).sum();
        if let Some(old) = g.entries.remove(&key) {
            g.bytes = g.bytes.saturating_sub(old.bytes);
        }
        if g.bytes + bytes > CACHE_BYTE_BUDGET {
            tracing::warn!(
                held = g.bytes,
                incoming = bytes,
                "data-api response cache is at its byte budget; not caching"
            );
            return false;
        }
        g.bytes += bytes;
        g.entries.insert(
            key,
            Entry {
                data,
                row_count,
                expires: now + ttl,
                bytes,
            },
        );
        true
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
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

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
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
        // Why `None` hashes a literal marker instead of hashing nothing: the nil
        // UUID is a real `UserId` value, and "absent" must not be spellable by
        // any principal.
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
        let one: BTreeMap<_, _> = params(&[("a", "1"), ("b", "2")]);
        let other: BTreeMap<_, _> = params(&[("b", "2"), ("a", "1")]);
        assert_eq!(
            cache_key(ep, 1, &one, Some(p)),
            cache_key(ep, 1, &other, Some(p)),
            "a BTreeMap is what makes the request's order irrelevant"
        );
        assert_ne!(
            cache_key(ep, 1, &one, Some(p)),
            cache_key(ep, 1, &params(&[("a", "1"), ("b", "3")]), Some(p)),
        );
        // The delimiters, pinned: without them `{a: "1", b: "2"}` and
        // `{ab: "12"}` hash the same bytes.
        assert_ne!(
            cache_key(ep, 1, &one, Some(p)),
            cache_key(ep, 1, &params(&[("ab", "12")]), Some(p)),
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
        assert!(c.put_at("k".into(), data, 1, Duration::from_secs(60), t0));

        assert!(c.get_at("k", t0 + Duration::from_secs(59)).is_some());
        assert!(
            c.get_at("k", t0 + Duration::from_secs(61)).is_none(),
            "an entry past its TTL must not be served"
        );
        assert_eq!(c.len(), 0, "reading an expired entry must reclaim it");
    }

    #[test]
    fn a_write_prunes_entries_that_expired_without_being_read_again() {
        // Expiry-on-read only reclaims keys someone asks for again — precisely
        // the set that does not need reclaiming.
        let c = ResponseCache::new();
        let t0 = Instant::now();
        let d = || Arc::new(serde_json::json!([{ "n": 1 }]));
        c.put_at("a".into(), d(), 1, Duration::from_secs(10), t0);
        c.put_at("b".into(), d(), 1, Duration::from_secs(10), t0);
        assert_eq!(c.len(), 2);

        c.put_at(
            "c".into(),
            d(),
            1,
            Duration::from_secs(10),
            t0 + Duration::from_secs(30),
        );
        assert_eq!(c.len(), 1, "the two stale entries must be gone");
    }

    #[test]
    fn an_oversized_response_is_not_cached() {
        let c = ResponseCache::new();
        let big = Arc::new(serde_json::Value::String("x".repeat(MAX_ENTRY_BYTES + 1)));
        assert!(!c.put_at("k".into(), big, 1, Duration::from_secs(60), Instant::now()));
        assert_eq!(c.len(), 0);
    }
}
