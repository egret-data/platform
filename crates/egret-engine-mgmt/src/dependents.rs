//! What still depends on an engine.
//!
//! An engine is materialized into each assigned workspace as a `query.databases`
//! row (`engine_id` set). Deleting the engine — or unassigning it — deletes those
//! rows, and several user-authored assets hang off them. One cascades quietly
//! (query history), one is nulled (saved queries), and the rest either block
//! the DELETE outright or would be destroyed with it:
//!
//! * `dataapi.endpoints.database_id` is `ON DELETE RESTRICT` (0121), and soft
//!   delete (0126) keeps trashed endpoints in the table, so they block too;
//! * `automation.schedules.database_id` is `SET NULL` (0110) but
//!   `schedule_alert_needs_query` rejects a NULL on an alert;
//! * `analytics.datasets` cascade (0012), and through them charts, dashboard
//!   cards and report schedules — silently, which is why they block here.
//!
//! [`QUERY_DATABASES_CHILD_FKS`] is the classification, pinned against
//! `pg_constraint` by `tests/dependents.rs`.

use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::EngineMgmtError;
use crate::store::EngineMgmtStore;

/// A kind of asset that blocks deleting or unassigning an engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EngineDependentKind {
    /// A Data API endpoint (draft, live or disabled) on the connection.
    DataapiEndpoint,
    /// A Data API endpoint in the trash — still a row, still blocks.
    DataapiEndpointTrashed,
    /// An alert or report schedule reading the connection.
    Schedule,
    /// A dataset on the connection (charts, cards and reports hang off it).
    Dataset,
}

impl EngineDependentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DataapiEndpoint => "dataapi_endpoint",
            Self::DataapiEndpointTrashed => "dataapi_endpoint_trashed",
            Self::Schedule => "schedule",
            Self::Dataset => "dataset",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            Self::DataapiEndpoint,
            Self::DataapiEndpointTrashed,
            Self::Schedule,
            Self::Dataset,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// One (workspace, kind) group of blocking assets, with up to five names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineDependentDto {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    /// For links: the UI routes a workspace as `/explore/{slug}/…`.
    pub workspace_slug: String,
    pub kind: EngineDependentKind,
    pub count: i64,
    /// Up to five names, alphabetical.
    pub sample: Vec<String>,
}

/// The 409 body for an engine delete/unassign that was refused. `code` is
/// `engine_has_dependents` (with `dependents`) or `constraint_violation` (with
/// `constraint`).
#[derive(Debug, Serialize, ToSchema)]
pub struct EngineConflictBody {
    pub error: String,
    pub code: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependents: Vec<EngineDependentDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
}

/// What [`EngineMgmtStore::delete_engine_unless_blocked`] did.
#[derive(Debug)]
pub enum DeleteEngineOutcome {
    Deleted,
    /// No such row (a concurrent delete won).
    NotFound,
    /// A dependent appeared after the preflight; nothing was deleted.
    Blocked(Vec<EngineDependentDto>),
}

/// How deleting a `query.databases` row treats one FK child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildOnDelete {
    Blocks(EngineDependentKind),
    Cascades,
    SetsNull,
}

/// One FK child of `query.databases`, as `pg_constraint` describes it.
pub struct DatabaseChildFk {
    /// Schema-qualified table.
    pub table: &'static str,
    pub column: &'static str,
    /// `pg_constraint.confdeltype`: `c` cascade, `n` set null, `r` restrict, `a` no action.
    pub on_delete: &'static str,
    pub not_null: bool,
    pub handling: ChildOnDelete,
}

pub const QUERY_DATABASES_CHILD_FKS: &[DatabaseChildFk] = &[
    DatabaseChildFk {
        table: "analytics.datasets",
        column: "database_id",
        on_delete: "c",
        not_null: true,
        handling: ChildOnDelete::Blocks(EngineDependentKind::Dataset),
    },
    DatabaseChildFk {
        table: "analytics.saved_queries",
        column: "database_id",
        on_delete: "n",
        not_null: false,
        handling: ChildOnDelete::SetsNull,
    },
    DatabaseChildFk {
        table: "automation.schedules",
        column: "database_id",
        on_delete: "n",
        not_null: false,
        handling: ChildOnDelete::Blocks(EngineDependentKind::Schedule),
    },
    DatabaseChildFk {
        table: "dataapi.endpoints",
        column: "database_id",
        on_delete: "r",
        not_null: true,
        handling: ChildOnDelete::Blocks(EngineDependentKind::DataapiEndpoint),
    },
    DatabaseChildFk {
        table: "query.queries",
        column: "database_id",
        on_delete: "c",
        not_null: true,
        handling: ChildOnDelete::Cascades,
    },
];

/// Blocking dependents of engine `$1`, optionally only in workspaces `$2`.
/// Keep in step with [`QUERY_DATABASES_CHILD_FKS`]; the sample cap (5) is `[1:5]`.
const DEPENDENTS_SQL: &str = "\
WITH dbs AS ( \
  SELECT d.id, d.workspace_id FROM query.databases d \
   WHERE d.engine_id = $1 AND ($2::uuid[] IS NULL OR d.workspace_id = ANY($2)) \
), items AS ( \
  SELECT dbs.workspace_id, \
         CASE WHEN e.deleted_at IS NULL THEN 'dataapi_endpoint' ELSE 'dataapi_endpoint_trashed' END AS kind, \
         e.name \
    FROM dataapi.endpoints e JOIN dbs ON dbs.id = e.database_id \
  UNION ALL \
  SELECT dbs.workspace_id, 'schedule', s.name \
    FROM automation.schedules s JOIN dbs ON dbs.id = s.database_id \
  UNION ALL \
  SELECT dbs.workspace_id, 'dataset', ds.name \
    FROM analytics.datasets ds JOIN dbs ON dbs.id = ds.database_id \
) \
SELECT i.workspace_id, w.name AS workspace_name, w.slug AS workspace_slug, i.kind, \
       count(*) AS count, (array_agg(i.name ORDER BY i.name))[1:5] AS sample \
  FROM items i JOIN workspaces w ON w.id = i.workspace_id \
 GROUP BY i.workspace_id, w.name, w.slug, i.kind \
 ORDER BY w.name, i.workspace_id, i.kind";

#[derive(sqlx::FromRow)]
struct DependentRow {
    workspace_id: Uuid,
    workspace_name: String,
    workspace_slug: String,
    kind: String,
    count: i64,
    sample: Vec<String>,
}

async fn fetch_dependents<'e, E>(
    exec: E,
    engine_id: Uuid,
    workspace_ids: Option<Vec<Uuid>>,
) -> Result<Vec<EngineDependentDto>, EngineMgmtError>
where
    E: sqlx::PgExecutor<'e>,
{
    let rows: Vec<DependentRow> = sqlx::query_as(DEPENDENTS_SQL)
        .bind(engine_id)
        .bind(workspace_ids)
        .fetch_all(exec)
        .await?;
    rows.into_iter()
        .map(|r| {
            let kind = EngineDependentKind::parse(&r.kind).ok_or_else(|| {
                EngineMgmtError::Db(sqlx::Error::Protocol(format!(
                    "unknown dependent kind {:?}",
                    r.kind
                )))
            })?;
            Ok(EngineDependentDto {
                workspace_id: r.workspace_id,
                workspace_name: r.workspace_name,
                workspace_slug: r.workspace_slug,
                kind,
                count: r.count,
                sample: r.sample,
            })
        })
        .collect()
}

impl EngineMgmtStore {
    /// Blocking dependents of `engine_id`; `workspace_ids` restricts them to
    /// those workspaces (an unassign). Empty means nothing blocks.
    pub async fn engine_dependents(
        &self,
        engine_id: Uuid,
        workspace_ids: Option<&[Uuid]>,
    ) -> Result<Vec<EngineDependentDto>, EngineMgmtError> {
        fetch_dependents(self.pool(), engine_id, workspace_ids.map(<[Uuid]>::to_vec)).await
    }

    /// Delete the engine row unless something blocks it, re-checked under locks.
    ///
    /// The engine row is locked first (`FOR UPDATE` conflicts with the
    /// `FOR KEY SHARE` a concurrent `assign` takes when it materializes a new
    /// connection), then its connections (the same conflict with a concurrent
    /// endpoint/schedule/dataset INSERT referencing one). So the dependents read
    /// here cannot go stale before the DELETE, and a race answers 409 rather
    /// than a constraint 500.
    pub async fn delete_engine_unless_blocked(
        &self,
        id: Uuid,
    ) -> Result<DeleteEngineOutcome, EngineMgmtError> {
        let mut tx = self.pool().begin().await?;
        let locked: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM engine_mgmt.engines WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if locked.is_none() {
            tx.rollback().await?;
            return Ok(DeleteEngineOutcome::NotFound);
        }
        sqlx::query("SELECT id FROM query.databases WHERE engine_id = $1 FOR UPDATE")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
        let blocking = fetch_dependents(&mut *tx, id, None).await?;
        if !blocking.is_empty() {
            tx.rollback().await?;
            return Ok(DeleteEngineOutcome::Blocked(blocking));
        }
        sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(DeleteEngineOutcome::Deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_blocking_child_is_read_by_the_preflight_sql() {
        for fk in QUERY_DATABASES_CHILD_FKS {
            if let ChildOnDelete::Blocks(kind) = fk.handling {
                assert!(
                    DEPENDENTS_SQL.contains(fk.table),
                    "{} not in DEPENDENTS_SQL",
                    fk.table
                );
                assert!(
                    DEPENDENTS_SQL.contains(kind.as_str()),
                    "{kind:?} not in DEPENDENTS_SQL"
                );
            }
        }
        assert!(DEPENDENTS_SQL.contains("dataapi_endpoint_trashed"));
    }

    #[test]
    fn kinds_round_trip_and_serialize_snake_case() {
        for k in [
            EngineDependentKind::DataapiEndpoint,
            EngineDependentKind::DataapiEndpointTrashed,
            EngineDependentKind::Schedule,
            EngineDependentKind::Dataset,
        ] {
            assert_eq!(EngineDependentKind::parse(k.as_str()), Some(k));
            assert_eq!(
                serde_json::to_value(k).unwrap(),
                serde_json::json!(k.as_str())
            );
        }
        assert_eq!(EngineDependentKind::parse("chart"), None);
    }
}
