//! The KV expiry sweep (W9c spec §11): every key whose LATEST revision has
//! expired is hard-deleted -- every row of the key, then the objects --
//! across every workspace, hourly, the first pass one interval after start.
//!
//! **Safe on every replica.** Each key is taken with `pg_try_advisory_lock`
//! on its session lock (the hash every writer of the key takes): a busy key
//! is skipped and the next pass retries it. Under the lock the key's rows
//! are re-read; only when the latest is STILL expired are they deleted, by
//! the ids just read, so a key rewritten between the listing and the lock
//! keeps its new revision, and two sweeps delete each row once (a delete by
//! id of a row already gone is a no-op). The lock is held until the
//! objects are deleted, so no writer reuses a freed revision number's
//! object key in between. A failed object delete is a WARN orphan (§15).
//! Age is the database's clock (`expires_at <= now()`).
//!
//! **Held:** a pooled connection for each page's query; per key, the
//! session lock and its connection across the re-read, the delete and the
//! object deletes (server-paced; the user's ruling, 2026-10-07).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::key::KvKey;
use super::store::{delete_objects, release, session_key};
use crate::storage::lock::LockedSession;
use crate::storage::nsfiles::access::{authorize, Access, Actor, Resource};
use crate::storage::nsfiles::NsError;
use crate::storage::FileStore;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvSweepConfig {
    /// Between passes, and before the first.
    pub every: Duration,
    /// Keys per page.
    pub batch: i64,
}

impl Default for KvSweepConfig {
    /// Hourly, 1000 keys per page.
    fn default() -> Self {
        Self {
            every: Duration::from_secs(3600),
            batch: 1000,
        }
    }
}

fn db_err(e: sqlx::Error) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::Error::new(e)))
}

/// A test's hook, run for each listed key between the page and its lock.
pub type SweepHook =
    Arc<dyn Fn(Uuid, String, String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// One pass over EVERY workspace: keyset pages of `batch` by row id of the
/// latest rows with `expires_at <= now()`; per key: try-lock (busy: skipped,
/// the next pass retries), re-read the key's rows under the lock, the latest
/// still expired -> DELETE every row of the key by the ids read, delete the
/// objects, unlock. Grants come from `authorize(ws, Actor::System, ns,
/// Resource::Kv, Access::Purge)`. Answers keys removed; Err is the
/// database's.
pub async fn sweep_expired(
    pool: &PgPool,
    fs: &FileStore,
    cfg: &KvSweepConfig,
) -> Result<u64, NsError> {
    sweep(pool, fs, cfg, None).await
}

/// [`sweep_expired`] with `hook` run between each key's listing and its
/// lock (tests: a rewrite in between).
#[cfg(any(test, feature = "test-util"))]
pub async fn sweep_expired_with_hook(
    pool: &PgPool,
    fs: &FileStore,
    cfg: &KvSweepConfig,
    hook: SweepHook,
) -> Result<u64, NsError> {
    sweep(pool, fs, cfg, Some(hook)).await
}

async fn sweep(
    pool: &PgPool,
    fs: &FileStore,
    cfg: &KvSweepConfig,
    hook: Option<SweepHook>,
) -> Result<u64, NsError> {
    let mut removed = 0u64;
    let mut after = 0i64;
    loop {
        let page: Vec<(i64, Uuid, String, String)> = sqlx::query_as(
            "SELECT e.id, e.workspace_id, e.namespace, e.key FROM workflow.kv_entries e \
              WHERE e.expires_at <= now() AND e.id > $1 \
                AND e.revision = (SELECT max(l.revision) FROM workflow.kv_entries l \
                                   WHERE l.workspace_id = e.workspace_id \
                                     AND l.namespace = e.namespace AND l.key = e.key) \
              ORDER BY e.id LIMIT $2",
        )
        .bind(after)
        .bind(cfg.batch)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        let short = (page.len() as i64) < cfg.batch;
        for (id, workspace, namespace, key) in page {
            after = id;
            if let Some(hook) = &hook {
                hook(workspace, namespace.clone(), key.clone()).await;
            }
            if sweep_key(pool, fs, workspace, &namespace, &key).await? {
                removed += 1;
            }
        }
        if short {
            return Ok(removed);
        }
    }
}

/// One listed key: true when its rows were removed.
async fn sweep_key(
    pool: &PgPool,
    fs: &FileStore,
    workspace: Uuid,
    namespace: &str,
    key: &str,
) -> Result<bool, NsError> {
    let grant = match authorize(
        workspace,
        Actor::System,
        namespace,
        Resource::Kv,
        Access::Purge,
    ) {
        Ok(g) => g,
        Err(why) => {
            tracing::warn!(namespace = %namespace, error = %why, "the KV expiry sweep skips a namespace egret would not have written");
            return Ok(false);
        }
    };
    let key = match KvKey::parse(key) {
        Ok(k) => k,
        Err(why) => {
            tracing::warn!(key = %key, error = %why, "the KV expiry sweep skips a key egret would not have written");
            return Ok(false);
        }
    };
    let lock = session_key(pool, &grant, &key).await?;
    let Some(mut session) = LockedSession::try_acquire(pool, lock)
        .await
        .map_err(NsError::Db)?
    else {
        // Busy: a writer, a purge or another replica's sweep holds it.
        return Ok(false);
    };
    let result = async {
        let rows: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT id, (expires_at IS NOT NULL AND expires_at <= now()) \
               FROM workflow.kv_entries \
              WHERE workspace_id = $1 AND namespace = $2 AND key = $3 \
              ORDER BY revision",
        )
        .bind(grant.workspace())
        .bind(grant.namespace())
        .bind(key.as_str())
        .fetch_all(session.conn())
        .await
        .map_err(db_err)?;
        // Judged on the rows read under the lock: a key rewritten (or purged)
        // since the listing is left as it now is.
        if !rows.last().is_some_and(|(_, expired)| *expired) {
            return Ok(false);
        }
        let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
        let gone: Vec<i32> = sqlx::query_scalar(
            "DELETE FROM workflow.kv_entries \
              WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND id = ANY($4) \
              RETURNING revision",
        )
        .bind(grant.workspace())
        .bind(grant.namespace())
        .bind(key.as_str())
        .bind(&ids)
        .fetch_all(session.conn())
        .await
        .map_err(db_err)?;
        delete_objects(fs, &grant, &key, &gone).await;
        Ok(!gone.is_empty())
    }
    .await;
    release(session, "KV expiry sweep").await;
    result
}

/// Runs `pass` every `every`, the FIRST time one `every` after the start
/// (`interval_at`, spec §11: a restart does not sweep at once), until
/// `cancel` (observed between passes).
fn spawn_every<F, Fut>(every: Duration, cancel: CancellationToken, mut pass: F) -> JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => pass().await,
            }
        }
    })
}

/// Hourly, the FIRST pass after `every` (`interval_at`, plan item 4); stops
/// on `cancel` between passes. Every replica with a store runs one.
pub fn spawn_kv_expiry_sweeper(
    pool: PgPool,
    fs: Arc<FileStore>,
    cfg: KvSweepConfig,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    spawn_every(cfg.every, cancel, move || {
        let (pool, fs) = (pool.clone(), fs.clone());
        async move {
            match sweep_expired(&pool, &fs, &cfg).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(removed = n, "expired KV keys swept"),
                Err(e) => tracing::warn!(error = %e, "the KV expiry sweep failed"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Plan item 4: the first pass comes one `every` after the start, never
    /// at once (`interval`'s first tick is immediate).
    #[tokio::test(start_paused = true)]
    async fn the_first_sweep_pass_comes_after_every() {
        let passes = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let counted = passes.clone();
        let handle = spawn_every(Duration::from_secs(3600), cancel.clone(), move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        // Let the task start and take its interval.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(passes.load(Ordering::SeqCst), 0, "a pass at start");
        tokio::time::advance(Duration::from_secs(59 * 60)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(passes.load(Ordering::SeqCst), 0, "a pass before an hour");
        tokio::time::advance(Duration::from_secs(2 * 60)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(passes.load(Ordering::SeqCst), 1, "one pass after an hour");
        tokio::time::advance(Duration::from_secs(3600)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(passes.load(Ordering::SeqCst), 2, "hourly after");
        cancel.cancel();
        handle.await.unwrap();
    }

    #[test]
    fn the_default_is_hourly_in_pages_of_1000() {
        assert_eq!(
            KvSweepConfig::default(),
            KvSweepConfig {
                every: Duration::from_secs(3600),
                batch: 1000
            }
        );
    }
}
