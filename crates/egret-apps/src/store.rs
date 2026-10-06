//! Data app storage: the identity row, its single mutable draft (optimistic
//! revisions), and append-only published versions (migration 0163).
//!
//! Scoping rule: a statement that takes an app id either also takes the
//! workspace, or its doc says "called only after the handler resolved the app
//! in the workspace" (via [`AppStore::get`]).

use chrono::{DateTime, Utc};
use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use sqlx::{FromRow, Row};
use std::sync::Arc;
use uuid::Uuid;

pub const MAX_SOURCE_BYTES: usize = 1_048_576;

#[derive(Debug, Clone)]
pub struct AppRow {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: String,
    pub folder_id: Option<Uuid>,
    pub owner_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub draft_revision: i64,
    pub latest_version_no: Option<i32>,
    pub bookmarked: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct Draft {
    pub source: String,
    /// marimo's grid-layout companion file (`layouts/<app>.<type>.json`), verbatim.
    /// `None` is the default vertical arrangement.
    pub layout_json: Option<String>,
    pub revision: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct VersionSummary {
    pub id: Uuid,
    pub version_no: i32,
    pub from_revision: Option<i64>,
    pub restored_from: Option<Uuid>,
    pub published_by: Option<Uuid>,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Version {
    pub summary: VersionSummary,
    pub source: String,
    pub layout_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved { revision: i64 },
    Stale { current_revision: i64 },
}

#[derive(Debug, Clone)]
pub enum PublishOutcome {
    Published(VersionSummary),
    Stale { current_revision: i64 },
}

#[derive(FromRow)]
struct AppRowRaw {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    description: String,
    folder_id: Option<Uuid>,
    owner_user_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
    draft_revision: i64,
    latest_version_no: Option<i32>,
    bookmarked: bool,
}

impl From<AppRowRaw> for AppRow {
    fn from(r: AppRowRaw) -> Self {
        Self {
            id: r.id,
            workspace_id: WorkspaceId(r.workspace_id),
            name: r.name,
            description: r.description,
            folder_id: r.folder_id,
            owner_user_id: r.owner_user_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
            draft_revision: r.draft_revision,
            latest_version_no: r.latest_version_no,
            bookmarked: r.bookmarked,
        }
    }
}

/// The app list/detail projection; `$2` is the viewer (bookmark flag). A macro so `concat!` keeps it a
/// `&'static str` (sqlx 0.9 `query` refuses runtime strings).
macro_rules! app_select {
    ($tail:literal) => {
        concat!(
            "SELECT a.id, a.workspace_id, a.name, a.description, a.folder_id, a.owner_user_id, \
                    a.created_at, a.updated_at, a.deleted_at, \
                    COALESCE(d.revision, 1) AS draft_revision, \
                    (SELECT max(version_no) FROM analytics.app_versions v WHERE v.app_id = a.id) AS latest_version_no, \
                    EXISTS (SELECT 1 FROM analytics.bookmarks b WHERE b.user_id = $2 \
                            AND b.entity_type = 'app' AND b.entity_id = a.id) AS bookmarked \
               FROM analytics.apps a LEFT JOIN analytics.app_drafts d ON d.app_id = a.id ",
            $tail
        )
    };
}

fn db_err(e: sqlx::Error) -> Error {
    anyhow::Error::from(e).into()
}

fn store_err(e: StorageError) -> Error {
    anyhow::Error::from(e).into()
}

fn check_len(source: &str) -> Result<()> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Error::InvalidInput("source exceeds 1 MiB".into()));
    }
    Ok(())
}

#[derive(Clone)]
pub struct AppStore {
    db: Db,
    store: Arc<dyn ObjectStore>,
    bucket: String,
    prefix: String,
}

impl AppStore {
    pub fn new(db: Db, store: Arc<dyn ObjectStore>, bucket: String, prefix: String) -> Self {
        Self {
            db,
            store,
            bucket,
            prefix,
        }
    }

    /// Mirrors `FileStore::object_uri`'s shape: `s3://{bucket}/{prefix}{workspace}/...`.
    ///
    /// Keyed by draft revision (like [`Self::version_key`] is by `version_no`), so
    /// each revision's content is its own object: a save writes the NEXT
    /// revision's key and only becomes visible once its row commits, which is
    /// what keeps a write whose transaction never committed (finding I3) from
    /// ever being read as the draft.
    fn draft_key(&self, ws: WorkspaceId, app: Uuid, revision: i64) -> String {
        format!(
            "s3://{}/{}{}/{}/draft/{}.py",
            self.bucket,
            self.prefix,
            ws.0.hyphenated(),
            app.hyphenated(),
            revision
        )
    }

    fn draft_layout_key(&self, ws: WorkspaceId, app: Uuid, revision: i64) -> String {
        format!(
            "s3://{}/{}{}/{}/draft/{}.layout.json",
            self.bucket,
            self.prefix,
            ws.0.hyphenated(),
            app.hyphenated(),
            revision
        )
    }

    fn version_key(&self, ws: WorkspaceId, app: Uuid, version_no: i32) -> String {
        format!(
            "s3://{}/{}{}/{}/versions/{}/source.py",
            self.bucket,
            self.prefix,
            ws.0.hyphenated(),
            app.hyphenated(),
            version_no
        )
    }

    fn version_layout_key(&self, ws: WorkspaceId, app: Uuid, version_no: i32) -> String {
        format!(
            "s3://{}/{}{}/{}/versions/{}/layout.json",
            self.bucket,
            self.prefix,
            ws.0.hyphenated(),
            app.hyphenated(),
            version_no
        )
    }

    /// Workspace-scoped (the workspace is the row's own). App and rev-1 draft in one tx.
    pub async fn create(
        &self,
        ws: WorkspaceId,
        owner: UserId,
        name: &str,
        description: &str,
        folder_id: Option<Uuid>,
        source: &str,
    ) -> Result<AppRow> {
        check_len(source)?;
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, description, folder_id, owner_user_id) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(ws.0)
        .bind(name)
        .bind(description)
        .bind(folder_id)
        .bind(owner.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        // The object store is the only home of draft content. A new draft row
        // takes `revision`'s schema default of 1, so its content goes to
        // revision 1's key. Written before the tx commits, same fail-fast
        // ordering as `save_draft` (nothing durable if the PUT fails). `source`
        // is left NULL: a non-NULL column means "pre-Task-10 content to
        // migrate" to `backfill_draft_locked` (migration 0172).
        self.store
            .put(&self.draft_key(ws, id, 1), source.as_bytes().to_vec())
            .await
            .map_err(store_err)?;
        sqlx::query("INSERT INTO analytics.app_drafts (app_id, updated_by) VALUES ($1, $2)")
            .bind(id)
            .bind(owner.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.get(ws, owner, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("created app {id} vanished").into())
    }

    /// Workspace-scoped. `trashed` selects the trash bin instead of the live list.
    pub async fn list(&self, ws: WorkspaceId, user: UserId, trashed: bool) -> Result<Vec<AppRow>> {
        let rows = if trashed {
            sqlx::query_as::<_, AppRowRaw>(app_select!(
                "WHERE a.workspace_id = $1 AND a.deleted_at IS NOT NULL ORDER BY a.updated_at DESC"
            ))
        } else {
            sqlx::query_as::<_, AppRowRaw>(app_select!(
                "WHERE a.workspace_id = $1 AND a.deleted_at IS NULL ORDER BY a.updated_at DESC"
            ))
        }
        .bind(ws.0)
        .bind(user.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(AppRow::from).collect())
    }

    /// Workspace-scoped; includes trashed apps. The handler's resolution point.
    pub async fn get(&self, ws: WorkspaceId, user: UserId, id: Uuid) -> Result<Option<AppRow>> {
        let row =
            sqlx::query_as::<_, AppRowRaw>(app_select!("WHERE a.workspace_id = $3 AND a.id = $1"))
                .bind(id)
                .bind(user.0)
                .bind(ws.0)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(db_err)?;
        Ok(row.map(AppRow::from))
    }

    /// Workspace-scoped. `false` = no such app in this workspace.
    pub async fn update(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE analytics.apps SET name = COALESCE($3, name), description = COALESCE($4, description), \
                    updated_at = now() \
              WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(ws.0)
        .bind(name)
        .bind(description)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    /// Workspace-scoped.
    pub async fn set_trashed(&self, ws: WorkspaceId, id: Uuid, trashed: bool) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE analytics.apps SET deleted_at = CASE WHEN $3 THEN now() ELSE NULL END, updated_at = now() \
              WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(ws.0)
        .bind(trashed)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    /// Workspace-scoped on the app. The folder is NOT checked here: the handler
    /// validates it with [`Self::folder_kind`] first.
    pub async fn set_folder(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        folder_id: Option<Uuid>,
    ) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE analytics.apps SET folder_id = $3, updated_at = now() WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(ws.0)
        .bind(folder_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    /// `(workspace, entity_type)` of a folder, so the handler can refuse a folder of
    /// another workspace or another entity type. Takes no app id.
    pub async fn folder_kind(&self, folder_id: Uuid) -> Result<Option<(Uuid, String)>> {
        let row =
            sqlx::query("SELECT workspace_id, entity_type FROM analytics.folders WHERE id = $1")
                .bind(folder_id)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(db_err)?;
        Ok(row.map(|r| (r.get("workspace_id"), r.get("entity_type"))))
    }

    /// Per-user bookmark (idempotent). Called only after the handler resolved the
    /// app in the workspace; bookmarks are per-user rows, not app state.
    pub async fn set_bookmark(&self, user: UserId, id: Uuid, on: bool) -> Result<()> {
        let q = if on {
            sqlx::query(
                "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) VALUES ($1,'app',$2) \
                 ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
            )
        } else {
            sqlx::query("DELETE FROM analytics.bookmarks WHERE user_id=$1 AND entity_type='app' AND entity_id=$2")
        };
        q.bind(user.0)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Called only after the handler resolved the app in the workspace. Reads the
    /// row's current `revision`, then that revision's objects -- object store
    /// only: every draft revision has a source object (written by
    /// `create`/`save_draft` before their commit, or by `egret
    /// backfill-apps-storage` for a draft last written before revision-keyed
    /// content), so a source `NotFound` is a real error. A missing layout object
    /// just means that revision has no layout.
    ///
    /// The row read takes no lock, so a `save_draft` can commit revision N+1 and
    /// then delete revision N's objects between this reading `revision = N` and
    /// fetching N's objects. A `NotFound` caused that way is always preceded by
    /// that commit, so on any `NotFound` this re-reads the revision: if it moved,
    /// the read is retried against the new revision instead of reporting an
    /// error (source) or, worse, silently answering "no layout" (layout).
    pub async fn draft(&self, ws: WorkspaceId, id: Uuid) -> Result<Option<Draft>> {
        const ATTEMPTS: usize = 5;
        for _ in 0..ATTEMPTS {
            let Some((revision, updated_at)) = self.draft_revision(id).await? else {
                return Ok(None);
            };
            let source = match self.store.get(&self.draft_key(ws, id, revision)).await {
                Ok(bytes) => Some(String::from_utf8(bytes).map_err(|e| {
                    Error::Internal(anyhow::anyhow!("draft {id} source is not UTF-8: {e}"))
                })?),
                Err(StorageError::NotFound(_)) => None,
                Err(e) => return Err(store_err(e)),
            };
            let layout = match self
                .store
                .get(&self.draft_layout_key(ws, id, revision))
                .await
            {
                Ok(bytes) => Some(String::from_utf8(bytes).map_err(|e| {
                    Error::Internal(anyhow::anyhow!("draft {id} layout is not UTF-8: {e}"))
                })?),
                Err(StorageError::NotFound(_)) => None,
                Err(e) => return Err(store_err(e)),
            };
            if source.is_none() || layout.is_none() {
                // A miss is only trustworthy if the revision did not move.
                let current = self.draft_revision(id).await?.map(|(r, _)| r);
                if current != Some(revision) {
                    continue;
                }
            }
            let Some(source) = source else {
                return Err(Error::Internal(anyhow::anyhow!(
                    "draft {id} revision {revision} has no source object"
                )));
            };
            return Ok(Some(Draft {
                source,
                layout_json: layout,
                revision,
                updated_at,
            }));
        }
        Err(Error::Internal(anyhow::anyhow!(
            "draft {id} kept changing while being read ({ATTEMPTS} attempts)"
        )))
    }

    async fn draft_revision(&self, id: Uuid) -> Result<Option<(i64, DateTime<Utc>)>> {
        sqlx::query_as::<_, (i64, DateTime<Utc>)>(
            "SELECT revision, updated_at FROM analytics.app_drafts WHERE app_id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Called only after the handler resolved the app in the workspace.
    /// Compare-and-swap on `revision`; over-limit source is `InvalidInput`.
    /// `layout` is the complete new state (`None` clears it) -- the pod patch always
    /// sends marimo's current layout, so there is no "leave unchanged" case to track.
    ///
    /// Locks the draft row (`FOR UPDATE`, same pattern as `publish`'s lock on this
    /// same table) across the object-store writes, rather than an unlocked
    /// check-then-write: an unlocked pre-check narrows a stale-write race but does
    /// not close it -- two ordinary concurrent saves can both pass a pre-check,
    /// both write to S3, and leave the CAS loser's content visible under the
    /// winner's revision (caught by `a_losing_concurrent_save_never_becomes_the_visible_draft`).
    /// With the row locked, a second saver blocks for the first's full duration,
    /// including its S3 round-trip, and wakes to see the new revision -- so it
    /// returns `Stale` without ever touching the object store. The trailing
    /// `UPDATE` is therefore a formality under the lock, not a second
    /// compare-and-swap.
    ///
    /// The content goes to the NEXT revision's keys (finding I3): if this
    /// transaction never commits (commit error, or the handler future dropped
    /// between the PUT and the commit), the row still points at `expected`, so
    /// the uncommitted object is never read. The row's `source`/`layout_json`
    /// columns are set to NULL (migration 0172: non-NULL means "pre-Task-10
    /// content still to migrate"). Only after the commit are `expected`'s
    /// objects deleted -- best-effort cleanup, never failing a committed save.
    pub async fn save_draft(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        by: UserId,
        expected: i64,
        source: &str,
        layout: Option<&str>,
    ) -> Result<SaveOutcome> {
        check_len(source)?;
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("draft of app {id}")))?;
        if revision != expected {
            // Dropping `tx` here rolls back (a no-op: nothing was changed) and
            // releases the row lock.
            return Ok(SaveOutcome::Stale {
                current_revision: revision,
            });
        }
        // Still holding the lock: no other saver can observe or change this row
        // until this transaction commits or rolls back.
        let next = expected + 1;
        self.store
            .put(&self.draft_key(ws, id, next), source.as_bytes().to_vec())
            .await
            .map_err(store_err)?;
        match layout {
            Some(l) => self
                .store
                .put(&self.draft_layout_key(ws, id, next), l.as_bytes().to_vec())
                .await
                .map_err(store_err)?,
            // `next`'s keys are not this save's alone: an earlier attempt at the
            // same revision whose transaction never committed may have left a
            // layout object there (same orphan reasoning as `insert_version`'s
            // `else` arm), which would otherwise be served as this revision's
            // layout. "Key already absent" is success.
            None => self
                .store
                .delete(&self.draft_layout_key(ws, id, next))
                .await
                .map_err(store_err)?,
        }
        let new_revision: i64 = sqlx::query_scalar(
            "UPDATE analytics.app_drafts SET source = NULL, layout_json = NULL, revision = revision + 1, updated_by = $2, updated_at = now() \
              WHERE app_id = $1 AND revision = $3 RETURNING revision",
        )
        .bind(id)
        .bind(by.0)
        .bind(expected)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            // Unreachable under the row lock: the revision cannot have moved
            // since the check above. An error, not a silent `Stale`, if it ever is.
            Error::Internal(anyhow::anyhow!(
                "draft {id} revision changed under its own row lock"
            ))
        })?;
        tx.commit().await.map_err(db_err)?;
        // Committed: nothing points at `expected` any more. Its objects are
        // only taking up space; a failed delete leaves an object nothing reads.
        for key in [
            self.draft_key(ws, id, expected),
            self.draft_layout_key(ws, id, expected),
        ] {
            if let Err(e) = self.store.delete(&key).await {
                tracing::warn!(error = %e, %key, "data app: could not delete a superseded draft object");
            }
        }
        Ok(SaveOutcome::Saved {
            revision: new_revision,
        })
    }

    /// Called only after the handler resolved the app in the workspace. Locking the
    /// draft row serialises publishes (and restores) of one app.
    ///
    /// Locks first, then reads the draft's *current* content via [`Self::draft`]
    /// (the locked revision's object, same as every other reader) --
    /// `draft()`'s own SELECT is a plain read, which Postgres never blocks behind
    /// this transaction's row lock, and nothing can have changed the draft since
    /// the lock was taken. The version's own object-store write then happens
    /// inside [`insert_version`], before that function's `INSERT`, and so before
    /// this transaction commits: a crash between the two leaves an orphaned S3
    /// object (harmless) rather than a published-looking Postgres row with no
    /// content in the object store.
    pub async fn publish(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        by: UserId,
        expected: Option<i64>,
    ) -> Result<PublishOutcome> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("draft of app {id}")))?;
        if expected.is_some_and(|e| e != revision) {
            return Ok(PublishOutcome::Stale {
                current_revision: revision,
            });
        }
        let draft = self
            .draft(ws, id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("draft of app {id}")))?;
        let v = self
            .insert_version(
                &mut tx,
                ws,
                id,
                &draft.source,
                draft.layout_json.as_deref(),
                Some(revision),
                None,
                by,
            )
            .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(PublishOutcome::Published(v))
    }

    /// Called only after the handler resolved the app in the workspace. Newest first.
    pub async fn versions(&self, id: Uuid) -> Result<Vec<VersionSummary>> {
        sqlx::query_as::<_, VersionSummary>(
            "SELECT id, version_no, from_revision, restored_from, published_by, published_at \
               FROM analytics.app_versions WHERE app_id = $1 ORDER BY version_no DESC",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Scoped to the app by the query itself (`app_id = $1 AND id = $2`), so it is
    /// safe even with an unresolved version id. Reads `source`/`layout_json` from
    /// the object store only -- Part S2's backfill guarantees every version has a
    /// source object, so a `StorageError::NotFound` there is a real error, not a
    /// fallback case. A missing layout object is unambiguous (a version simply has
    /// no layout) and reads back as `None`.
    pub async fn version(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        version_id: Uuid,
    ) -> Result<Option<Version>> {
        let Some(row) = sqlx::query(
            "SELECT id, version_no, from_revision, restored_from, published_by, published_at \
               FROM analytics.app_versions WHERE app_id = $1 AND id = $2",
        )
        .bind(id)
        .bind(version_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?
        else {
            return Ok(None);
        };
        let summary = VersionSummary::from_row(&row).map_err(db_err)?;

        let source = match self
            .store
            .get(&self.version_key(ws, id, summary.version_no))
            .await
        {
            Ok(bytes) => String::from_utf8(bytes).map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "version {version_id} source is not UTF-8: {e}"
                ))
            })?,
            Err(e) => return Err(store_err(e)),
        };
        let layout_json = match self
            .store
            .get(&self.version_layout_key(ws, id, summary.version_no))
            .await
        {
            Ok(bytes) => Some(String::from_utf8(bytes).map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "version {version_id} layout is not UTF-8: {e}"
                ))
            })?),
            Err(StorageError::NotFound(_)) => None,
            Err(e) => return Err(store_err(e)),
        };

        Ok(Some(Version {
            summary,
            source,
            layout_json,
        }))
    }

    /// Called only after the handler resolved the app in the workspace; the old
    /// version is read scoped to the app. Republishes its source as a NEW version
    /// (`restored_from` set, `from_revision` NULL); the draft is untouched.
    ///
    /// Takes the same draft-row lock as [`Self::publish`], and requires it: with no
    /// draft row there is nothing to serialise on, so that is `NotFound` (as in
    /// publish), never an unlocked insert racing a concurrent publish.
    ///
    /// Versions are immutable and each already has its own object-store key, so
    /// "restoring" is a pure server-side [`ObjectStore::copy`] of the old
    /// version's objects into the new version's keys -- no bytes pass through
    /// this process. Every version is guaranteed a source object-store key
    /// (Part S2's backfill), so the source `copy()`'s result is trusted
    /// directly: any error, including `NotFound`, propagates. Whether the old
    /// version has a layout is determined by attempting the layout `copy()`
    /// itself, never by a Postgres column (which is `NULL` for every version
    /// published since source/layout stopped being written there): a
    /// `StorageError::NotFound` from that copy means the old version genuinely
    /// has no layout, while any other error still propagates.
    pub async fn restore_version(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        version_id: Uuid,
        by: UserId,
    ) -> Result<Option<VersionSummary>> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query("SELECT 1 FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?
            .ok_or_else(|| Error::NotFound(format!("draft of app {id}")))?;
        let Some(old_no) = sqlx::query_scalar::<_, i32>(
            "SELECT version_no FROM analytics.app_versions WHERE app_id = $1 AND id = $2",
        )
        .bind(id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        else {
            return Ok(None);
        };

        // Still holding the draft-row lock: no concurrent publish/restore of this
        // app can have changed `app_versions` since.
        let new_no: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version_no), 0) + 1 FROM analytics.app_versions WHERE app_id = $1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        self.store
            .copy(
                &self.version_key(ws, id, old_no),
                &self.version_key(ws, id, new_no),
            )
            .await
            .map_err(store_err)?;
        match self
            .store
            .copy(
                &self.version_layout_key(ws, id, old_no),
                &self.version_layout_key(ws, id, new_no),
            )
            .await
        {
            Ok(()) => {}
            Err(StorageError::NotFound(_)) => {
                // The old version genuinely has no layout (copy's source is absent) --
                // clear any stale object that might already sit at the new key (same
                // orphan-from-a-collided-version_no reasoning as `insert_version`'s
                // `else` arm).
                self.store
                    .delete(&self.version_layout_key(ws, id, new_no))
                    .await
                    .map_err(store_err)?;
            }
            Err(e) => return Err(store_err(e)),
        }

        let v = sqlx::query_as::<_, VersionSummary>(
            "INSERT INTO analytics.app_versions (app_id, version_no, from_revision, restored_from, published_by) \
             VALUES ($1, $2, NULL, $3, $4) \
             RETURNING id, version_no, from_revision, restored_from, published_by, published_at",
        )
        .bind(id)
        .bind(new_no)
        .bind(version_id)
        .bind(by.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(Some(v))
    }

    /// Computes the next `version_no` (callers hold the draft row lock, which
    /// serialises this against every other publish/restore of the same app),
    /// writes `source`/`layout_json` to that version's object-store key(s), then
    /// inserts the Postgres row -- in that order, so a crash between the PUT and
    /// the INSERT leaves an orphaned S3 object (harmless, collected by nothing
    /// today) rather than a Postgres row whose content never made it to S3.
    #[allow(clippy::too_many_arguments)]
    async fn insert_version(
        &self,
        tx: &mut sqlx::PgConnection,
        ws: WorkspaceId,
        app: Uuid,
        source: &str,
        layout_json: Option<&str>,
        from_revision: Option<i64>,
        restored_from: Option<Uuid>,
        by: UserId,
    ) -> Result<VersionSummary> {
        let version_no: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version_no), 0) + 1 FROM analytics.app_versions WHERE app_id = $1",
        )
        .bind(app)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        self.store
            .put(
                &self.version_key(ws, app, version_no),
                source.as_bytes().to_vec(),
            )
            .await
            .map_err(store_err)?;
        if let Some(l) = layout_json {
            self.store
                .put(
                    &self.version_layout_key(ws, app, version_no),
                    l.as_bytes().to_vec(),
                )
                .await
                .map_err(store_err)?;
        } else {
            // `version_no` is a freshly computed MAX+1: if a previous publish
            // attempt wrote this same key (e.g. the handler future was dropped
            // after this PUT but before the transaction committed) and this
            // publish has no layout, an orphaned layout object would otherwise
            // survive and be served ahead of the genuinely NULL Postgres column.
            self.store
                .delete(&self.version_layout_key(ws, app, version_no))
                .await
                .map_err(store_err)?;
        }

        sqlx::query_as::<_, VersionSummary>(
            "INSERT INTO analytics.app_versions (app_id, version_no, from_revision, restored_from, published_by) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, version_no, from_revision, restored_from, published_by, published_at",
        )
        .bind(app)
        .bind(version_no)
        .bind(from_revision)
        .bind(restored_from)
        .bind(by.0)
        .fetch_one(tx)
        .await
        .map_err(db_err)
    }

    /// Backfill only: write a draft's known-good Postgres content to revision
    /// `revision`'s object-store key(s). Pure S3 writes -- no Postgres
    /// statement, no transaction/lock -- so the caller must hold the row lock
    /// and have read `revision` and the content under it (see
    /// [`Self::backfill_draft_locked`]). Mirrors [`Self::save_draft`]'s S3-write
    /// shape (`Some` layout => put, `None` => delete, so a rerun never leaves a
    /// stale layout object behind).
    pub async fn backfill_draft(
        &self,
        ws: WorkspaceId,
        app: Uuid,
        revision: i64,
        source: &str,
        layout: Option<&str>,
    ) -> Result<()> {
        self.store
            .put(
                &self.draft_key(ws, app, revision),
                source.as_bytes().to_vec(),
            )
            .await
            .map_err(store_err)?;
        match layout {
            Some(l) => self
                .store
                .put(
                    &self.draft_layout_key(ws, app, revision),
                    l.as_bytes().to_vec(),
                )
                .await
                .map_err(store_err)?,
            None => self
                .store
                .delete(&self.draft_layout_key(ws, app, revision))
                .await
                .map_err(store_err)?,
        }
        Ok(())
    }

    /// Moves one draft written before revision-keyed content (Parts S1/S2's
    /// fixed `draft.py` key, or pre-S1 Postgres-only) to its CURRENT revision's
    /// object-store key. Returns whether it wrote anything.
    ///
    /// The source of truth is the row's own `source`/`layout_json` columns, not
    /// the old fixed-key object: every pre-Task-10 write committed those columns
    /// in the same `UPDATE` that bumped `revision`, so they are exactly the
    /// current revision's committed content -- whereas the fixed key may hold a
    /// write whose transaction never committed (finding I3 itself). Code from
    /// Task 10 on sets those columns to NULL (migration 0172), so a NULL
    /// `source` means the current revision's object was written by
    /// revision-keyed code and there is nothing to migrate; a rerun, or a run
    /// after saves by the new code, never overwrites newer content with the
    /// stale column.
    ///
    /// Takes the same `FOR UPDATE` row lock `save_draft`/`publish` do and reads
    /// the row under it, so a concurrent `save_draft` either commits first (and
    /// this sees its NULL columns) or waits for this to finish. (Versions don't
    /// need this: `app_versions` rows are append-only and immutable.)
    pub async fn backfill_draft_locked(&self, ws: WorkspaceId, app: Uuid) -> Result<bool> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let (revision, source, layout_json): (i64, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT revision, source, layout_json FROM analytics.app_drafts \
                  WHERE app_id = $1 FOR UPDATE",
            )
            .bind(app)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        let Some(source) = source else {
            return Ok(false);
        };
        self.backfill_draft(ws, app, revision, &source, layout_json.as_deref())
            .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(true)
    }

    /// S2-1 backfill only: write a pre-S1 published version's known-good
    /// Postgres content to its object-store key(s). Pure S3 writes -- no
    /// Postgres statement, no transaction/lock -- because the row's Postgres
    /// state is already correct and this is only catching the object store up.
    /// Mirrors [`Self::insert_version`]'s S3-write shape (`Some` layout => put,
    /// `None` => delete, so a rerun never leaves a stale layout object behind).
    pub async fn backfill_version(
        &self,
        ws: WorkspaceId,
        app: Uuid,
        version_no: i32,
        source: &str,
        layout: Option<&str>,
    ) -> Result<()> {
        self.store
            .put(
                &self.version_key(ws, app, version_no),
                source.as_bytes().to_vec(),
            )
            .await
            .map_err(store_err)?;
        match layout {
            Some(l) => self
                .store
                .put(
                    &self.version_layout_key(ws, app, version_no),
                    l.as_bytes().to_vec(),
                )
                .await
                .map_err(store_err)?,
            None => self
                .store
                .delete(&self.version_layout_key(ws, app, version_no))
                .await
                .map_err(store_err)?,
        }
        Ok(())
    }
}
