//! Staging objects: where a namespace file's body is uploaded before any lock
//! is taken (W9b Task 8 fix round 1, the user's ruling of 2026-10-06; spec
//! §4.2, §8.1).
//!
//! [`super::store::put`] reads its caller's body -- a request body paced by a
//! client, or an internal stream -- into a fresh staging object
//! `{prefix}{workspace}/_nsfiles_staging/{uuid}`
//! ([`FileStore::namespace_staging_uri`]) while it holds NO pool connection
//! and NO advisory lock. Only then does it open the short, server-paced
//! transaction that locks the path, picks the revision and copies the staging
//! object onto the revision's key. So a slow or stalled client holds an
//! object-store upload, never a database connection or another writer's
//! lock.
//!
//! # Bounded orphans
//!
//! Every staging object has a row in `workflow.namespace_file_staging`,
//! written BEFORE its upload starts ([`record`]) and deleted only AFTER its
//! object is ([`forget`]). `put` forgets its staging object on every way out
//! it returns from; a process that dies in between (or a caller's future
//! dropped mid-way) leaves the row, and [`sweep`] -- run hourly by
//! [`spawn_staging_sweeper`] -- deletes every staging object whose row is
//! older than [`StagingSweepConfig::older_than`] (24 hours), then its row.
//! The age is the database's clock on both sides, never the store's
//! `last_modified`.
//!
//! The sweep deletes ONLY a key it proves is a staging key of its row's own
//! workspace and id ([`is_staging_uri`]), whatever the row says.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::NsError;
use crate::storage::FileStore;

/// The key segment under a workspace that holds staging objects. It starts
/// with `_`, which no namespace's first segment may (`^[a-z0-9]`), so no
/// namespace file and no execution file is ever under it.
pub const STAGING_SEGMENT: &str = "_nsfiles_staging";

fn db_err(e: sqlx::Error) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::Error::new(e)))
}

/// One staging upload: its row's id and its object's URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub id: Uuid,
    pub workspace: Uuid,
    pub uri: String,
}

/// A fresh staging object of `workspace`, recorded BEFORE anything is
/// uploaded to it: one short statement on the pool, no connection kept.
pub async fn record(pool: &PgPool, fs: &FileStore, workspace: Uuid) -> Result<Staged, NsError> {
    let id = Uuid::new_v4();
    let uri = fs.namespace_staging_uri(workspace, id);
    sqlx::query(
        "INSERT INTO workflow.namespace_file_staging (id, workspace_id, object_uri) \
         VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(workspace)
    .bind(&uri)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(Staged { id, workspace, uri })
}

/// Deletes the staging object, then its row; best-effort. A failure is
/// logged and leaves the row, so [`sweep`] tries again once it is old.
pub async fn forget(pool: &PgPool, fs: &FileStore, staged: &Staged) {
    if let Err(e) = fs.store.delete(&staged.uri).await {
        tracing::warn!(key = %staged.uri, error = %e, "a namespace file's staging object could not be deleted; the staging sweep will");
        return;
    }
    if let Err(e) = sqlx::query("DELETE FROM workflow.namespace_file_staging WHERE id = $1")
        .bind(staged.id)
        .execute(pool)
        .await
    {
        tracing::warn!(id = %staged.id, error = %e, "a namespace file's staging row could not be deleted; the staging sweep will");
    }
}

/// Is `uri` exactly the staging key of `(workspace, id)` under `fs`'s
/// bucket and prefix? The sweep deletes nothing else.
pub fn is_staging_uri(fs: &FileStore, workspace: Uuid, id: Uuid, uri: &str) -> bool {
    uri == fs.namespace_staging_uri(workspace, id)
}

/// The staging sweep's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagingSweepConfig {
    /// Between passes.
    pub every: Duration,
    /// A staging row older than this is an orphan: no `put` takes that long
    /// between its upload and its commit (its body is idle-bounded and its
    /// transaction server-paced).
    pub older_than: Duration,
    /// Rows per query.
    pub batch: i64,
}

impl Default for StagingSweepConfig {
    /// Hourly, 24 hours, 100 rows per query.
    fn default() -> Self {
        Self {
            every: Duration::from_secs(3600),
            older_than: Duration::from_secs(24 * 3600),
            batch: 100,
        }
    }
}

/// One pass: every staging row older than `cfg.older_than` -- its object
/// deleted, then the row. Answers how many rows it removed. A store failure
/// ends the pass (logged; the rows stay for the next one); `Err` is the
/// database's.
///
/// A row whose `object_uri` is not its own staging key under `fs` (a changed
/// `workflow.storage.prefix`, or a row egret did not write) is removed
/// WITHOUT deleting anything: the sweep never deletes a key it cannot prove
/// is a staging object.
pub async fn sweep(
    pool: &PgPool,
    fs: &FileStore,
    cfg: &StagingSweepConfig,
) -> Result<u64, NsError> {
    let mut removed = 0u64;
    let mut done: Vec<Uuid> = Vec::new();
    loop {
        let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
            "SELECT id, workspace_id, object_uri FROM workflow.namespace_file_staging \
              WHERE created_at < now() - make_interval(secs => $1) AND NOT (id = ANY($2)) \
              ORDER BY created_at, id LIMIT $3",
        )
        .bind(cfg.older_than.as_secs_f64())
        .bind(&done)
        .bind(cfg.batch)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        let short = (rows.len() as i64) < cfg.batch;
        for (id, workspace, uri) in rows {
            done.push(id);
            if is_staging_uri(fs, workspace, id, &uri) {
                if let Err(e) = fs.store.delete(&uri).await {
                    tracing::warn!(key = %uri, error = %e, "the namespace file staging sweep could not delete an orphan; the next pass retries");
                    return Ok(removed);
                }
            } else {
                tracing::warn!(id = %id, key = %uri, "a namespace file staging row names no staging key of this store: its row is removed, nothing is deleted");
            }
            sqlx::query("DELETE FROM workflow.namespace_file_staging WHERE id = $1")
                .bind(id)
                .execute(pool)
                .await
                .map_err(db_err)?;
            removed += 1;
        }
        if short {
            return Ok(removed);
        }
    }
}

/// Start the hourly staging sweep. Stops on `cancel`, between passes. Every
/// replica may run one: two passes over one row delete one object twice (a
/// no-op) and one row twice (a no-op).
pub fn spawn_staging_sweeper(
    pool: PgPool,
    fs: Arc<FileStore>,
    cfg: StagingSweepConfig,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => match sweep(&pool, &fs, &cfg).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "namespace file staging orphans swept"),
                    Err(e) => tracing::warn!(error = %e, "the namespace file staging sweep failed"),
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::nsfiles::access::check_namespace;

    fn fs() -> FileStore {
        FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "b".into(),
            prefix: "workflow/".into(),
            caps: Default::default(),
            retention_days: 30,
        }
    }

    /// The staging segment can never be a namespace's first segment, so no
    /// namespace file's key and no execution file's key is under it.
    #[test]
    fn no_namespace_can_reach_the_staging_segment() {
        for ns in [STAGING_SEGMENT, "_nsfiles_staging.a", "_x"] {
            assert!(check_namespace(ns).is_err(), "{ns}");
        }
        let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(
            fs().namespace_staging_uri(ws, id),
            format!("s3://b/workflow/{ws}/_nsfiles_staging/{id}")
        );
    }

    #[test]
    fn only_a_rows_own_staging_key_is_a_staging_uri() {
        let fs = fs();
        let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(is_staging_uri(
            &fs,
            ws,
            id,
            &fs.namespace_staging_uri(ws, id)
        ));
        for other in [
            fs.namespace_staging_uri(Uuid::new_v4(), id),
            fs.namespace_staging_uri(ws, Uuid::new_v4()),
            format!("s3://b/workflow/{ws}/w9b/_files/x"),
            format!("s3://b/workflow/{ws}/_nsfiles_staging/"),
            "s3://b/workflow/".to_string(),
        ] {
            assert!(!is_staging_uri(&fs, ws, id, &other), "{other}");
        }
    }
}
