//! Rejected Ranger plugin credentials, per engine (spec
//! `docs/en/specs/2026-10-20-ranger-surface-auth-design.md` A12), stored in
//! `policy.ranger_auth_failures` (migration 0201).
//!
//! The surface middleware calls [`note_rejected`] when an `engine-<uuid>`
//! credential is refused, and the download handlers call [`note_download`]
//! when an engine credential is served. Both are fire-and-forget: they never
//! change the response, never wait on the database, and bound what an attacker
//! can make egret do:
//!
//! * **Rows**: the upsert selects from `engine_mgmt.engines`, so a forged
//!   `engine-<random uuid>` adds no row.
//! * **Writes and log lines**: per engine, at most one upsert (and one `warn!`)
//!   per [`FAILURE_WINDOW`]; the rejections in between are carried into the
//!   next upsert. The ones still pending when a flood stops are never written,
//!   so `count` is a lower bound under a flood (documented in 0201).
//! * **Memory**: the coalescing map holds at most [`MAX_TRACKED`] engine ids;
//!   when it is full of live entries, a new id is not recorded at all.
//! * **Concurrency**: at most [`MAX_IN_FLIGHT`] writes at once, each cut off
//!   after [`WRITE_TIMEOUT`]; with none free, the write is skipped.
//!
//! Neither the password nor the `Authorization` header is ever passed here.
use crate::{PolicyStore, PolicyStoreError};
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Per engine, at most one failure upsert in this window.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(10);
/// Per engine, at most one "downloaded" mark in this window. A cleared finding
/// therefore lags a successful download by at most this long.
pub const DOWNLOAD_WINDOW: Duration = Duration::from_secs(60);
/// Engine ids the coalescing map holds at most.
const MAX_TRACKED: usize = 4096;
const MAX_IN_FLIGHT: usize = 8;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// One engine's rejected credentials.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuthFailure {
    pub engine_id: Uuid,
    /// Rejections recorded; a lower bound (see the module docs).
    pub count: i64,
    /// The latest rejection recorded.
    pub last_at: DateTime<Utc>,
    /// The engine's latest successful download recorded after a rejection.
    pub ok_at: Option<DateTime<Utc>>,
}

impl AuthFailure {
    /// The latest rejection is newer than the latest successful download.
    pub fn is_outstanding(&self) -> bool {
        self.ok_at.is_none_or(|ok| self.last_at > ok)
    }
}

impl PolicyStore {
    /// Count one rejected credential for `engine_id`. `false` (and no row) when
    /// no such engine exists.
    pub async fn record_auth_failure(&self, engine_id: Uuid) -> Result<bool, PolicyStoreError> {
        self.record_auth_failures(engine_id, 1).await
    }

    /// Count `n` rejected credentials for `engine_id` in one upsert.
    pub async fn record_auth_failures(
        &self,
        engine_id: Uuid,
        n: i64,
    ) -> Result<bool, PolicyStoreError> {
        let r = sqlx::query(
            "INSERT INTO policy.ranger_auth_failures (engine_id, count) \
             SELECT id, $2 FROM engine_mgmt.engines WHERE id = $1 \
             ON CONFLICT (engine_id) DO UPDATE SET \
                 count = policy.ranger_auth_failures.count + EXCLUDED.count, \
                 last_at = now()",
        )
        .bind(engine_id)
        .bind(n)
        .execute(self.pool())
        .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Record that `engine_id` downloaded with its credential. Writes only
    /// when a rejection is outstanding: an engine that never failed has no
    /// row and gets none.
    pub async fn record_engine_download(&self, engine_id: Uuid) -> Result<(), PolicyStoreError> {
        sqlx::query(
            "UPDATE policy.ranger_auth_failures SET ok_at = now() \
             WHERE engine_id = $1 AND (ok_at IS NULL OR ok_at < last_at)",
        )
        .bind(engine_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Every engine's recorded rejections (rows of existing engines only).
    pub async fn auth_failures(&self) -> Result<Vec<AuthFailure>, PolicyStoreError> {
        let mut c = self.pool().acquire().await?;
        Ok(auth_failures_via(&mut c).await?)
    }
}

/// [`PolicyStore::auth_failures`] on a caller's connection (the Governance
/// Overview reads it inside its snapshot). A row whose engine is gone is
/// skipped; migration 0201's trigger deletes such rows anyway.
pub async fn auth_failures_via(c: &mut PgConnection) -> Result<Vec<AuthFailure>, sqlx::Error> {
    sqlx::query_as(
        "SELECT f.engine_id, f.count, f.last_at, f.ok_at \
           FROM policy.ranger_auth_failures f \
           JOIN engine_mgmt.engines e ON e.id = f.engine_id \
          ORDER BY f.engine_id",
    )
    .fetch_all(c)
    .await
}

/// Per-key coalescing: the first event of a window is let through carrying the
/// events held back since the previous one.
struct Coalescer {
    window: Duration,
    cap: usize,
    slots: Mutex<HashMap<Uuid, Slot>>,
}

struct Slot {
    since: Instant,
    pending: u64,
}

impl Coalescer {
    fn new(window: Duration, cap: usize) -> Self {
        Self {
            window,
            cap,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(n)`: write now, `n` events (this one and those held back).
    /// `None`: held back, or not tracked because the map is full.
    fn admit(&self, id: Uuid, now: Instant) -> Option<u64> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = slots.get_mut(&id) {
            if now.duration_since(s.since) < self.window {
                s.pending += 1;
                return None;
            }
            let n = s.pending + 1;
            *s = Slot {
                since: now,
                pending: 0,
            };
            return Some(n);
        }
        if slots.len() >= self.cap {
            // Expired entries' held-back events are dropped with them.
            let window = self.window;
            slots.retain(|_, s| now.duration_since(s.since) < window);
            if slots.len() >= self.cap {
                return None;
            }
        }
        slots.insert(
            id,
            Slot {
                since: now,
                pending: 0,
            },
        );
        Some(1)
    }
}

static FAILURES: LazyLock<Coalescer> =
    LazyLock::new(|| Coalescer::new(FAILURE_WINDOW, MAX_TRACKED));
static DOWNLOADS: LazyLock<Coalescer> =
    LazyLock::new(|| Coalescer::new(DOWNLOAD_WINDOW, MAX_TRACKED));
static IN_FLIGHT: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_IN_FLIGHT)));

/// Run `write` in the background if a slot is free, cut off after
/// [`WRITE_TIMEOUT`]. Never waits.
fn spawn_bounded<F>(what: &'static str, engine_id: Uuid, write: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let Ok(permit) = IN_FLIGHT.clone().try_acquire_owned() else {
        tracing::debug!(engine_id = %engine_id, what, "ranger: too many writes in flight; skipped");
        return;
    };
    tokio::spawn(async move {
        if tokio::time::timeout(WRITE_TIMEOUT, write).await.is_err() {
            tracing::warn!(engine_id = %engine_id, what, "ranger: write timed out; skipped");
        }
        drop(permit);
    });
}

/// The surface refused a credential presented for `engine_id`.
pub(crate) fn note_rejected(store: &PolicyStore, engine_id: Uuid) {
    let Some(n) = FAILURES.admit(engine_id, Instant::now()) else {
        return;
    };
    let store = store.clone();
    spawn_bounded("auth failure", engine_id, async move {
        match store.record_auth_failures(engine_id, n as i64).await {
            Ok(true) => tracing::warn!(
                engine_id = %engine_id,
                rejected = n,
                "ranger credential rejected"
            ),
            // A username naming no engine: nothing to count it under.
            Ok(false) => {
                tracing::debug!(engine_id = %engine_id, "ranger credential rejected for no engine")
            }
            Err(e) => tracing::warn!(
                engine_id = %engine_id,
                error = %e,
                "could not record a rejected ranger credential"
            ),
        }
    });
}

/// An engine credential was served a download.
pub(crate) fn note_download(store: &PolicyStore, engine_id: Uuid) {
    if DOWNLOADS.admit(engine_id, Instant::now()).is_none() {
        return;
    }
    let store = store.clone();
    spawn_bounded("engine download", engine_id, async move {
        if let Err(e) = store.record_engine_download(engine_id).await {
            tracing::warn!(engine_id = %engine_id, error = %e, "could not record an engine download");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn the_first_event_of_a_window_carries_the_ones_held_back() {
        let c = Coalescer::new(Duration::from_secs(10), 16);
        let id = Uuid::new_v4();
        let t = Instant::now();
        assert_eq!(c.admit(id, t), Some(1));
        assert_eq!(c.admit(id, at(t, 1)), None);
        assert_eq!(c.admit(id, at(t, 9)), None);
        assert_eq!(c.admit(id, at(t, 10)), Some(3));
        assert_eq!(c.admit(id, at(t, 11)), None);
        // Another key has its own window.
        assert_eq!(c.admit(Uuid::new_v4(), at(t, 11)), Some(1));
    }

    #[test]
    fn a_full_map_drops_expired_entries_then_refuses_new_keys() {
        let c = Coalescer::new(Duration::from_secs(10), 2);
        let t = Instant::now();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(c.admit(a, t), Some(1));
        assert_eq!(c.admit(b, at(t, 5)), Some(1));
        // Full of live entries: a new key is not tracked, a known one still is.
        assert_eq!(c.admit(Uuid::new_v4(), at(t, 6)), None);
        assert_eq!(c.admit(a, at(t, 6)), None);
        // `a` expired at t+10: its slot is reused for a new key.
        let d = Uuid::new_v4();
        assert_eq!(c.admit(d, at(t, 12)), Some(1));
        assert_eq!(c.slots.lock().unwrap().len(), 2);
        assert!(!c.slots.lock().unwrap().contains_key(&a));
    }

    #[test]
    fn outstanding_means_the_rejection_is_newer_than_the_download() {
        let t = Utc::now();
        let f = |ok: Option<i64>| AuthFailure {
            engine_id: Uuid::nil(),
            count: 1,
            last_at: t,
            ok_at: ok.map(|s| t + chrono::Duration::seconds(s)),
        };
        assert!(f(None).is_outstanding());
        assert!(f(Some(-1)).is_outstanding());
        assert!(!f(Some(0)).is_outstanding());
        assert!(!f(Some(1)).is_outstanding());
    }
}
