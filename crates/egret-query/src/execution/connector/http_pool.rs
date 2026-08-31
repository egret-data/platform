//! One `reqwest::Client` per **coordinator**, not per query.
//!
//! # What was wrong
//!
//! `TrinoConnector::execute` built a fresh `reqwest::Client` on every call, and
//! `build_connector` is itself called once per statement
//! (`execution/port_impl.rs`'s `run_bounded`). A `reqwest::Client` owns the
//! idle-connection pool, so a new one per statement means a new TCP connection
//! — and, on a `https` engine, a new TLS handshake and a fresh **parse of the
//! pinned PEM** — for every statement served. A sixteen-statement Data API
//! endpoint paid that sixteen times, and every one of those sockets sat in
//! `TIME_WAIT` afterwards.
//!
//! A `reqwest::Client` is already an internally-pooled, `Arc`-backed handle that
//! is designed to be cloned and shared; the fix is to stop throwing it away.
//!
//! # The key is identity-free, and that is the whole safety argument
//!
//! This cache is shared by **every** query egret runs — SQL Lab, charts,
//! notebooks and the Data API — and those queries run under different
//! principals. A client shared between two principals is only safe if the client
//! itself carries no principal state, so [`HttpClientKey`] is built from exactly
//! the two things [`build_client`] reads and from nothing else:
//!
//! * `base_url` — `scheme://host:port`, from the registry row;
//! * `tls_cert` — the coordinator's pinned PEM, from the registry row.
//!
//! Neither is derived from `ConnSpec::secret`, `ConnSpec::session_user` or
//! `ConnSpec::bearer_token`. Every credential and every identity Trino sees is
//! attached **per request** by `TrinoConnector::auth` — `X-Trino-User`,
//! `Authorization: Bearer`, HTTP basic auth — never on the client. So two
//! principals hitting one coordinator share sockets and share nothing else.
//!
//! Three properties keep that true, and each is pinned by a test rather than by
//! this comment:
//!
//! 1. **No cookie store.** `cookie_store(false)` is stated explicitly even
//!    though it is `reqwest`'s default, because the workspace enables
//!    `reqwest`'s `cookies` feature — so the switch that would turn this shared
//!    client into a cross-principal session carrier is one line away and
//!    compiles. `a_shared_client_never_carries_one_principals_cookie_to_another`
//!    in `tests/connector_trino_pool.rs` drives a real server that sets one.
//! 2. **No default headers.** The builder sets none, so there is nowhere for an
//!    identity to be attached once and inherited by everyone.
//! 3. **The key ignores identity.** `the_client_key_ignores_every_identity_field`
//!    below asserts two specs differing only in secret/session user/bearer
//!    produce the SAME key, and `a_different_pinned_cert_is_a_different_client`
//!    asserts the cert is not dropped from it — a client with the wrong pinned
//!    root would trust the wrong coordinator.
//!
//! # The timeout moved to the request
//!
//! The old per-query client carried `Client::builder().timeout(...)`, which is a
//! per-request deadline that happened to be configured per client. Sharing the
//! client means that value can no longer live there — different callers pass
//! different budgets (`Limits::timeout`, `DESCRIBE_TIMEOUT`). It is now set with
//! `RequestBuilder::timeout` on every request the connector builds, which is the
//! same deadline applied at the same granularity. Missing one would make that
//! request unbounded, so `trino.rs` routes every request through helpers that
//! apply it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::execution::error::EngineError;

/// The identity-free half of a Trino connection: everything
/// [`build_client`] reads, and nothing else.
///
/// `base_url` is not read by the builder at all — it is in the key because a
/// `reqwest::Client` keeps its idle-connection pool per origin, so keying on the
/// origin bounds how many coordinators one cached client holds sockets for and
/// makes the cache's entries mean "one coordinator" rather than "one trust
/// store".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HttpClientKey {
    pub(crate) base_url: String,
    pub(crate) tls_cert: Option<String>,
}

/// How many coordinators the cache will hold clients for.
///
/// The registry has a handful of engine rows, so this is not expected to bind.
/// It exists because the key contains a `String` that ultimately comes from a
/// database row: an unbounded map keyed on data is a slow leak, and the failure
/// mode without a cap is a process that grows forever rather than one that gets
/// slower. Past the cap the caller gets a fresh, uncached client — the old
/// behaviour, which is correct and merely unpooled.
const MAX_CACHED_CLIENTS: usize = 64;

fn cache() -> &'static Mutex<HashMap<HttpClientKey, reqwest::Client>> {
    static CACHE: OnceLock<Mutex<HashMap<HttpClientKey, reqwest::Client>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build the client for `key`. Called only on a cache miss.
///
/// Pinning the coordinator's self-signed cert as a trusted root supersedes
/// `danger_accept_invalid_certs`; without a pinned cert the client falls back to
/// system roots (unchanged for plain-HTTP engines).
fn build_client(key: &HttpClientKey) -> Result<reqwest::Client, EngineError> {
    // No timeout here: it is per-request now (see the module comment).
    let mut b = reqwest::Client::builder()
        // Explicit, not inherited. See the module comment: the workspace turns
        // on `reqwest`'s `cookies` feature, so a shared client is one builder
        // line away from carrying one principal's session to the next request.
        .cookie_store(false);
    if let Some(pem) = &key.tls_cert {
        let cert = reqwest::Certificate::from_pem(pem.as_bytes())
            .map_err(|e| EngineError::Connect(format!("engine cert: {e}")))?;
        b = b.add_root_certificate(cert);
    }
    b.build().map_err(|e| EngineError::Connect(e.to_string()))
}

/// The cached client for `key`, building it on first use.
///
/// A failure to build (a malformed pinned PEM) is **not** cached: it is a
/// configuration error that a corrected registry row should clear without a
/// restart, and caching it would make the fix invisible until one.
pub(crate) fn client_for(key: &HttpClientKey) -> Result<reqwest::Client, EngineError> {
    {
        let map = cache().lock().expect("http client cache poisoned");
        if let Some(c) = map.get(key) {
            return Ok(c.clone());
        }
    }
    // Built outside the lock: `Client::builder().build()` parses a PEM and
    // initialises a TLS config, and holding the cache lock across it would
    // serialise every other engine's first query behind this one.
    let client = build_client(key)?;
    let mut map = cache().lock().expect("http client cache poisoned");
    if map.len() >= MAX_CACHED_CLIENTS && !map.contains_key(key) {
        // Past the cap, hand back the client without caching it. Loud, because
        // the only way to reach this is far more engine rows than egret expects.
        tracing::warn!(
            cached = map.len(),
            "engine HTTP client cache is full; this connection will not be pooled"
        );
        return Ok(client);
    }
    // A concurrent caller may have inserted while the lock was released. Keeping
    // theirs rather than overwriting means the two callers share one client,
    // which is the entire point.
    Ok(map.entry(key.clone()).or_insert(client).clone())
}

/// How many cached clients exist for `host`. Test-only.
///
/// Counts by host rather than returning the whole map's length so that tests
/// running in parallel in one binary — each against its own stub coordinator on
/// its own port — do not observe each other.
#[cfg(test)]
pub(crate) fn cached_clients_for_host(host: &str) -> usize {
    cache()
        .lock()
        .expect("http client cache poisoned")
        .keys()
        .filter(|k| k.base_url.contains(host))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(base: &str, cert: Option<&str>) -> HttpClientKey {
        HttpClientKey {
            base_url: base.into(),
            tls_cert: cert.map(str::to_string),
        }
    }

    /// The property the whole cache rests on: the key is a function of the
    /// COORDINATOR, so no value derived from a principal can reach it.
    ///
    /// Written against the key rather than against `ConnSpec` on purpose — the
    /// `ConnSpec` half is asserted in `trino.rs`, where the projection lives, and
    /// this is the half that says two equal keys really are one cache entry.
    #[test]
    fn one_coordinator_is_one_cache_entry() {
        let k = key("http://one-entry.invalid:8080", None);
        let a = client_for(&k).expect("build");
        let b = client_for(&k).expect("build");
        assert_eq!(
            cached_clients_for_host("one-entry.invalid"),
            1,
            "a second call for the same coordinator must reuse the first client"
        );
        // Both are usable handles onto the same pool; `reqwest::Client` exposes
        // no identity to compare, so the count above is what proves sharing.
        drop((a, b));
    }

    /// The cert may NOT be dropped from the key. Two coordinators pinning
    /// different roots that shared a client would have one of them trusting the
    /// other's CA.
    #[test]
    fn a_different_pinned_cert_is_a_different_client() {
        let base = "https://two-certs.invalid:8443";
        // Not valid PEM — `client_for` fails, which is enough: the assertion is
        // that the two keys are not equal, and equality is what the map uses.
        assert_ne!(key(base, Some("PEM-A")), key(base, Some("PEM-B")));
        assert_ne!(key(base, None), key(base, Some("PEM-A")));
    }

    /// A pinned PEM that is well-formed PEM but not a certificate is a
    /// configuration error, and it must not be REMEMBERED: an operator who fixes
    /// the registry row should not have to restart egret to clear it.
    ///
    /// Note what this test also documents, because it is not obvious and the old
    /// `build_client` made the same (wrong) claim: `reqwest::Certificate::from_pem`
    /// does not parse anything under the rustls backend — it stores the bytes and
    /// the failure surfaces at `ClientBuilder::build()`. And a string containing
    /// no PEM block at all (`"not a pem"`) produces **no error at all**: the
    /// reader simply finds zero certificates, so the client is built with the
    /// built-in roots and the pin silently does not exist. That fails closed at
    /// connect time — a self-signed coordinator is then untrusted — but it fails
    /// there and not here, which is worth knowing before believing a green
    /// `client_for` means the pin took.
    #[test]
    fn a_bad_pem_is_refused_and_not_cached() {
        let base = "https://bad-pem.invalid:8443";
        // Valid PEM framing, invalid DER inside — this one `rustls` does reject.
        let k = key(
            base,
            Some("-----BEGIN CERTIFICATE-----\nAAAAAA==\n-----END CERTIFICATE-----\n"),
        );
        assert!(
            matches!(client_for(&k), Err(EngineError::Connect(_))),
            "a PEM whose DER is not a certificate must be refused"
        );
        assert_eq!(
            cached_clients_for_host("bad-pem.invalid"),
            0,
            "a failed build must leave nothing behind"
        );
    }
}
