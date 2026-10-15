//! Successful client-secret verifications, so a proven-good repeat skips Argon2.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Successful verifications, keyed by (client_id, sha256(secret)), storing the
/// secret hash that verified. Bounded; entries expire.
///
/// An entry is only a claim that this secret matched THAT stored hash; it says
/// nothing about whether the client may still mint. The caller must compare it
/// with the registry's current hash (`ClientVerifier::current`) on every hit,
/// which is what makes a rotation or a revoke take effect on the next request.
///
/// Only `put` after a successful verify adds entries, so an attacker cannot
/// grow the map; the capacity bound is defence in depth.
///
/// A hit skips the registry's `verify`, and with it the `last_used_at` UPDATE
/// both registries do there: that column now advances at most about once per
/// TTL per (client, secret).
#[derive(Clone)]
pub struct VerifiedCache {
    inner: Arc<Mutex<HashMap<(String, String), Entry>>>,
    capacity: usize,
    ttl: Duration,
}

struct Entry {
    secret_hash: String,
    inserted: Instant,
}

impl VerifiedCache {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            capacity,
            ttl,
        }
    }

    /// The hash that verified this (client_id, secret), if cached and unexpired.
    pub fn get(&self, client_id: &str, secret: &str) -> Option<String> {
        let key = (client_id.to_string(), secret_digest(secret));
        let mut map = self.lock();
        match map.get(&key) {
            // A hit deliberately does NOT refresh `inserted`: every client
            // re-verifies (Argon2 + `last_used_at`) once per TTL however busy.
            Some(e) if e.inserted.elapsed() < self.ttl => Some(e.secret_hash.clone()),
            Some(_) => {
                map.remove(&key);
                None
            }
            None => None,
        }
    }

    pub fn put(&self, client_id: &str, secret: &str, secret_hash: String) {
        if self.capacity == 0 {
            return;
        }
        let key = (client_id.to_string(), secret_digest(secret));
        let mut map = self.lock();
        if !map.contains_key(&key) && map.len() >= self.capacity {
            let ttl = self.ttl;
            map.retain(|_, e| e.inserted.elapsed() < ttl);
            if map.len() >= self.capacity {
                // Still full of live entries: drop the oldest. A linear scan,
                // but only on an insert into a full map of a bounded size.
                if let Some(oldest) = map
                    .iter()
                    .min_by_key(|(_, e)| e.inserted)
                    .map(|(k, _)| k.clone())
                {
                    map.remove(&oldest);
                }
            }
        }
        map.insert(
            key,
            Entry {
                secret_hash,
                inserted: Instant::now(),
            },
        );
    }

    /// A poisoned lock only means another request panicked mid-update; the map
    /// is still a valid map, and every entry is re-checked against the
    /// registry anyway.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Entry>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

impl Default for VerifiedCache {
    fn default() -> Self {
        Self::new(1024, Duration::from_secs(600))
    }
}

/// Lowercase-hex SHA-256 of the secret: the cache never holds the secret
/// itself. Unsalted is fine here — issued secrets are 256-bit random, so
/// there is no dictionary to run against the digest.
fn secret_digest(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_put_is_found_by_the_same_pair_only() {
        let c = VerifiedCache::default();
        c.put("cid", "sec", "H".into());

        assert_eq!(c.get("cid", "sec").as_deref(), Some("H"));
        assert_eq!(c.get("cid", "other"), None, "a different secret must miss");
        assert_eq!(c.get("other", "sec"), None, "a different client must miss");
    }

    #[test]
    fn an_entry_expires_after_its_ttl() {
        let c = VerifiedCache::new(16, Duration::from_millis(1));
        c.put("cid", "sec", "H".into());

        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(c.get("cid", "sec"), None);
    }

    #[test]
    fn the_cache_never_exceeds_its_capacity() {
        let c = VerifiedCache::new(4, Duration::from_secs(600));
        for i in 0..5 {
            c.put(&format!("c{i}"), "sec", "H".into());
        }

        assert!(c.len() <= 4, "len {} over capacity 4", c.len());
        assert_eq!(
            c.get("c4", "sec").as_deref(),
            Some("H"),
            "the newest entry must be the one kept"
        );
    }

    #[test]
    fn a_put_replaces_the_hash_for_the_same_pair() {
        let c = VerifiedCache::new(4, Duration::from_secs(600));
        c.put("cid", "sec", "OLD".into());
        c.put("cid", "sec", "NEW".into());

        assert_eq!(c.get("cid", "sec").as_deref(), Some("NEW"));
        assert_eq!(c.len(), 1);
    }

    /// The secret itself must never sit in memory longer than the request.
    #[test]
    fn the_key_holds_a_digest_not_the_secret() {
        assert_eq!(
            secret_digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
