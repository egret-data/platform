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
    fn draft_key(&self, ws: WorkspaceId, app: Uuid) -> String {
        format!(
            "s3://{}/{}{}/{}/draft.py",
            self.bucket,
            self.prefix,
            ws.0.hyphenated(),
            app.hyphenated()
        )
    }

    fn draft_layout_key(&self, ws: WorkspaceId, app: Uuid) -> String {
        format!(
            "s3://{}/{}{}/{}/draft.layout.json",
            self.bucket,
            self.prefix,
            ws.0.hyphenated(),
            app.hyphenated()
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
        // `draft()` reads exclusively from the object store (S1-3); the rev-1
        // draft's content must land there too, or the app this just created
        // would be unreadable. Written before the tx commits, same fail-fast
        // ordering as `save_draft` (nothing durable if the PUT fails).
        self.store
            .put(&self.draft_key(ws, id), source.as_bytes().to_vec())
            .await
            .map_err(store_err)?;
        sqlx::query(
            "INSERT INTO analytics.app_drafts (app_id, source, updated_by) VALUES ($1, $2, $3)",
        )
        .bind(id)
        .bind(source)
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

    /// Called only after the handler resolved the app in the workspace. Prefers
    /// the object store for `source`/`layout_json`, falling back to the row's own
    /// columns on `StorageError::NotFound` -- a row from before this code shipped
    /// has real content in Postgres and nothing in the object store yet, and this
    /// plan is only safe to ship with that fallback in place (Task 9/S3-1 removes
    /// it once Task 8/S2's backfill is confirmed to have run everywhere). The
    /// columns are therefore still selected here, not dead weight.
    pub async fn draft(&self, ws: WorkspaceId, id: Uuid) -> Result<Option<Draft>> {
        let Some(row) = sqlx::query(
            "SELECT source, layout_json, revision, updated_at FROM analytics.app_drafts WHERE app_id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?
        else {
            return Ok(None);
        };
        let pg_source: String = row.get("source");
        let pg_layout_json: Option<String> = row.get("layout_json");
        let revision: i64 = row.get("revision");
        let updated_at: DateTime<Utc> = row.get("updated_at");

        let source = match self.store.get(&self.draft_key(ws, id)).await {
            Ok(bytes) => String::from_utf8(bytes).map_err(|e| {
                Error::Internal(anyhow::anyhow!("draft {id} source is not UTF-8: {e}"))
            })?,
            // Pre-S1 row: fall back to the column, which the dual-write keeps
            // populated for every row, old and new.
            Err(StorageError::NotFound(_)) => pg_source,
            Err(e) => return Err(store_err(e)),
        };
        // A layout-key miss is ambiguous between "no layout saved" (post-S1) and
        // "backfill hasn't run yet" (pre-S1) -- both fall back to the column,
        // which already carries the right answer either way (`None` or the
        // legacy value), so the two cases collapse correctly without having to
        // tell them apart.
        let layout_json = match self.store.get(&self.draft_layout_key(ws, id)).await {
            Ok(bytes) => Some(String::from_utf8(bytes).map_err(|e| {
                Error::Internal(anyhow::anyhow!("draft {id} layout is not UTF-8: {e}"))
            })?),
            Err(StorageError::NotFound(_)) => pg_layout_json,
            Err(e) => return Err(store_err(e)),
        };

        Ok(Some(Draft {
            source,
            layout_json,
            revision,
            updated_at,
        }))
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
    /// compare-and-swap; it still binds `source`/`layout_json` into Postgres as
    /// the rollback safety net (a revert to the pre-S1 binary reads them from
    /// Postgres again).
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
        self.store
            .put(&self.draft_key(ws, id), source.as_bytes().to_vec())
            .await
            .map_err(store_err)?;
        match layout {
            Some(l) => self
                .store
                .put(&self.draft_layout_key(ws, id), l.as_bytes().to_vec())
                .await
                .map_err(store_err)?,
            // "Key already absent" is success -- no existence check needed first.
            None => self
                .store
                .delete(&self.draft_layout_key(ws, id))
                .await
                .map_err(store_err)?,
        }
        let new_revision: i64 = sqlx::query_scalar(
            "UPDATE analytics.app_drafts SET source = $3, layout_json = $5, revision = revision + 1, updated_by = $2, updated_at = now() \
              WHERE app_id = $1 AND revision = $4 RETURNING revision",
        )
        .bind(id)
        .bind(by.0)
        .bind(source)
        .bind(expected)
        .bind(layout)
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
        Ok(SaveOutcome::Saved {
            revision: new_revision,
        })
    }

    /// Called only after the handler resolved the app in the workspace. Locking the
    /// draft row serialises publishes (and restores) of one app.
    ///
    /// Locks first, then reads the draft's *current* content via [`Self::draft`]
    /// (object-store-first, Postgres-fallback, same as every other reader) --
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
    /// safe even with an unresolved version id. Mirrors [`Self::draft`]'s fallback:
    /// prefers the object store for `source`/`layout_json`, falling back to the
    /// row's own columns on `StorageError::NotFound` -- a version published before
    /// this code shipped has real content in Postgres and nothing in the object
    /// store yet (Task 9/S3-1 removes this once Task 8/S2's backfill is confirmed
    /// to have run everywhere). The columns are therefore still selected here, not
    /// dead weight.
    pub async fn version(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        version_id: Uuid,
    ) -> Result<Option<Version>> {
        let Some(row) = sqlx::query(
            "SELECT id, version_no, from_revision, restored_from, published_by, published_at, source, layout_json \
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
        let pg_source: String = row.get("source");
        let pg_layout_json: Option<String> = row.get("layout_json");

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
            Err(StorageError::NotFound(_)) => pg_source,
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
            Err(StorageError::NotFound(_)) => pg_layout_json,
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
    /// this process. The layout is only copied if the old version actually has
    /// one (most won't). If the OLD version predates object-store-backed
    /// versions, `copy()`'s source is missing and fails with `NotFound`; that one
    /// case falls back to `put`-ing the old version's Postgres columns straight
    /// to the new key -- the one place a GET-then-PUT through this process is
    /// unavoidable, and only for the one-time migration of a version that
    /// predates this rollout.
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
        let Some((old_no, source, layout_json)) = sqlx::query_as::<_, (i32, String, Option<String>)>(
            "SELECT version_no, source, layout_json FROM analytics.app_versions WHERE app_id = $1 AND id = $2",
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

        match self
            .store
            .copy(
                &self.version_key(ws, id, old_no),
                &self.version_key(ws, id, new_no),
            )
            .await
        {
            Ok(()) => {}
            Err(StorageError::NotFound(_)) => {
                self.store
                    .put(
                        &self.version_key(ws, id, new_no),
                        source.as_bytes().to_vec(),
                    )
                    .await
                    .map_err(store_err)?;
            }
            Err(e) => return Err(store_err(e)),
        }
        if let Some(l) = &layout_json {
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
                    self.store
                        .put(
                            &self.version_layout_key(ws, id, new_no),
                            l.as_bytes().to_vec(),
                        )
                        .await
                        .map_err(store_err)?;
                }
                Err(e) => return Err(store_err(e)),
            }
        }

        let v = sqlx::query_as::<_, VersionSummary>(
            "INSERT INTO analytics.app_versions (app_id, version_no, source, layout_json, from_revision, restored_from, published_by) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6) \
             RETURNING id, version_no, from_revision, restored_from, published_by, published_at",
        )
        .bind(id)
        .bind(new_no)
        .bind(&source)
        .bind(&layout_json)
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
        }

        sqlx::query_as::<_, VersionSummary>(
            "INSERT INTO analytics.app_versions (app_id, version_no, source, layout_json, from_revision, restored_from, published_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING id, version_no, from_revision, restored_from, published_by, published_at",
        )
        .bind(app)
        .bind(version_no)
        .bind(source)
        .bind(layout_json)
        .bind(from_revision)
        .bind(restored_from)
        .bind(by.0)
        .fetch_one(tx)
        .await
        .map_err(db_err)
    }
}
