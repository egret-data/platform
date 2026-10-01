//! Data app storage: the identity row, its single mutable draft (optimistic
//! revisions), and append-only published versions (migration 0163).
//!
//! Scoping rule: a statement that takes an app id either also takes the
//! workspace, or its doc says "called only after the handler resolved the app
//! in the workspace" (via [`AppStore::get`]).

use chrono::{DateTime, Utc};
use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use sqlx::{FromRow, Row};
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

fn check_len(source: &str) -> Result<()> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Error::InvalidInput("source exceeds 1 MiB".into()));
    }
    Ok(())
}

#[derive(Clone)]
pub struct AppStore {
    db: Db,
}

impl AppStore {
    pub fn new(db: Db) -> Self {
        Self { db }
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

    /// Called only after the handler resolved the app in the workspace.
    pub async fn draft(&self, id: Uuid) -> Result<Option<Draft>> {
        sqlx::query_as::<_, Draft>(
            "SELECT source, revision, updated_at FROM analytics.app_drafts WHERE app_id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Called only after the handler resolved the app in the workspace.
    /// Compare-and-swap on `revision`; over-limit source is `InvalidInput`.
    pub async fn save_draft(
        &self,
        id: Uuid,
        by: UserId,
        expected: i64,
        source: &str,
    ) -> Result<SaveOutcome> {
        check_len(source)?;
        let saved: Option<i64> = sqlx::query_scalar(
            "UPDATE analytics.app_drafts SET source = $3, revision = revision + 1, updated_by = $2, updated_at = now() \
              WHERE app_id = $1 AND revision = $4 RETURNING revision",
        )
        .bind(id)
        .bind(by.0)
        .bind(source)
        .bind(expected)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        if let Some(revision) = saved {
            return Ok(SaveOutcome::Saved { revision });
        }
        let current: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM analytics.app_drafts WHERE app_id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(db_err)?;
        match current {
            Some(current_revision) => Ok(SaveOutcome::Stale { current_revision }),
            None => Err(Error::NotFound(format!("draft of app {id}"))),
        }
    }

    /// Called only after the handler resolved the app in the workspace. Locking the
    /// draft row serialises publishes (and restores) of one app.
    pub async fn publish(
        &self,
        id: Uuid,
        by: UserId,
        expected: Option<i64>,
    ) -> Result<PublishOutcome> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let draft = sqlx::query(
            "SELECT source, revision FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("draft of app {id}")))?;
        let source: String = draft.get("source");
        let revision: i64 = draft.get("revision");
        if expected.is_some_and(|e| e != revision) {
            return Ok(PublishOutcome::Stale {
                current_revision: revision,
            });
        }
        let v = insert_version(&mut tx, id, &source, Some(revision), None, by).await?;
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
    /// safe even with an unresolved version id.
    pub async fn version(&self, id: Uuid, version_id: Uuid) -> Result<Option<Version>> {
        let row = sqlx::query(
            "SELECT id, version_no, from_revision, restored_from, published_by, published_at, source \
               FROM analytics.app_versions WHERE app_id = $1 AND id = $2",
        )
        .bind(id)
        .bind(version_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.map(|r| {
            Ok(Version {
                summary: VersionSummary::from_row(&r).map_err(db_err)?,
                source: r.get("source"),
            })
        })
        .transpose()
    }

    /// Called only after the handler resolved the app in the workspace; the old
    /// version is read scoped to the app. Republishes its source as a NEW version
    /// (`restored_from` set, `from_revision` NULL); the draft is untouched.
    pub async fn restore_version(
        &self,
        id: Uuid,
        version_id: Uuid,
        by: UserId,
    ) -> Result<Option<VersionSummary>> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query("SELECT 1 FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
        let Some(source) = sqlx::query_scalar::<_, String>(
            "SELECT source FROM analytics.app_versions WHERE app_id = $1 AND id = $2",
        )
        .bind(id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        else {
            return Ok(None);
        };
        let v = insert_version(&mut tx, id, &source, None, Some(version_id), by).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(Some(v))
    }
}

/// Next version number is `max + 1`; callers hold the draft row lock.
async fn insert_version(
    tx: &mut sqlx::PgConnection,
    app: Uuid,
    source: &str,
    from_revision: Option<i64>,
    restored_from: Option<Uuid>,
    by: UserId,
) -> Result<VersionSummary> {
    sqlx::query_as::<_, VersionSummary>("INSERT INTO analytics.app_versions (app_id, version_no, source, from_revision, restored_from, published_by) \
         SELECT $1, COALESCE(MAX(version_no), 0) + 1, $2, $3, $4, $5 \
           FROM analytics.app_versions WHERE app_id = $1 \
         RETURNING id, version_no, from_revision, restored_from, published_by, published_at",
    )
    .bind(app)
    .bind(source)
    .bind(from_revision)
    .bind(restored_from)
    .bind(by.0)
    .fetch_one(tx)
    .await
    .map_err(db_err)
}
