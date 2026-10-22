//! Dataset repository: dataset rows + their synced column snapshot.

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct DatasetRepo {
    db: Db,
}

/// The source side of a dataset: exactly one kind.
#[derive(Debug, Clone)]
pub enum SourceSpec {
    Physical {
        catalog: Option<String>,
        schema: String,
        table: String,
    },
    Virtual {
        sql: String,
    },
}

/// A classified column ready to persist (route maps raw engine type → generic).
#[derive(Debug, Clone)]
pub struct NewColumn {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dataset {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub database_id: Uuid,
    pub name: String,
    pub description: String,
    pub catalog: Option<String>,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub sql: Option<String>,
    pub created_by: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub certified: bool,
    pub certified_by: String,
    pub certification_details: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DatasetColumn {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
    pub expression: Option<String>,
    pub verbose_name: String,
    pub description: String,
    pub groupby: bool,
    pub filterable: bool,
    pub format: String,
    pub semantic_type: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Metric {
    pub id: Uuid,
    pub metric_name: String,
    pub expression: String,
    pub data_type: String,
    pub type_generic: String,
    pub verbose_name: String,
    pub description: String,
    pub ordinal: i32,
    pub format: String,
    pub certified: bool,
    pub certified_by: String,
    pub certification_details: String,
}

/// A chart that references a dataset (Usage view, and the list a refused delete answers with).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DatasetUsage {
    pub id: Uuid,
    pub name: String,
    pub viz_type: String,
    /// The chart is in the trash. It still references the dataset, and a delete of the dataset
    /// deletes it too (`ON DELETE CASCADE`), so a refusal names it.
    pub trashed: bool,
}

/// What a Refresh changed in the physical column snapshot, by column name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshChanges {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

const COLS: &str = "id, workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by, folder_id, updated_at, deleted_at, certified, certified_by, certification_details";

/// Private struct for list() — sqlx tuple FromRow is version-sensitive; use flatten instead.
#[derive(sqlx::FromRow)]
struct DatasetWithCount {
    #[sqlx(flatten)]
    ds: Dataset,
    column_count: i64,
}

/// Private struct for list_by_workspace_for — flatten the Dataset + the two extra scalar columns.
#[derive(sqlx::FromRow)]
struct DatasetWithBm {
    #[sqlx(flatten)]
    ds: Dataset,
    column_count: i64,
    bookmarked: bool,
}

impl DatasetRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Create a dataset and its column snapshot in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        ws: Uuid,
        database_id: Uuid,
        name: &str,
        description: &str,
        source: &SourceSpec,
        columns: &[NewColumn],
        user: Option<UserId>,
    ) -> Result<Dataset, BiError> {
        let (catalog, schema_name, table_name, sql) = match source {
            SourceSpec::Physical {
                catalog,
                schema,
                table,
            } => (
                catalog.clone(),
                Some(schema.clone()),
                Some(table.clone()),
                None,
            ),
            SourceSpec::Virtual { sql } => (None, None, None, Some(sql.clone())),
        };

        let mut tx = self.db.pool.begin().await?;
        let ds = match sqlx::query_as::<_, Dataset>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO analytics.datasets (workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING {COLS}"
        )))
        .bind(ws)
        .bind(database_id)
        .bind(name)
        .bind(description)
        .bind(&catalog)
        .bind(&schema_name)
        .bind(&table_name)
        .bind(&sql)
        .bind(user.map(|u| u.0))
        .fetch_one(&mut *tx)
        .await
        {
            Ok(d) => d,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(BiError::Conflict("a dataset with this name already exists".into()));
            }
            Err(e) => return Err(BiError::Db(e)),
        };

        insert_columns(&mut tx, ds.id, columns).await?;
        tx.commit().await?;
        Ok(ds)
    }

    pub async fn list(&self, ws: Uuid) -> Result<Vec<(Dataset, i64)>, BiError> {
        let rows = sqlx::query_as::<_, DatasetWithCount>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM analytics.dataset_columns c WHERE c.dataset_id = datasets.id) AS column_count \
             FROM analytics.datasets WHERE workspace_id = $1 ORDER BY updated_at DESC"
        )))
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.ds, r.column_count)).collect())
    }

    /// Datasets in a workspace with the per-user `bookmarked` flag and the column count,
    /// newest first. `trashed` selects the trash bin (soft-deleted) instead of the live list.
    /// The column count uses the same correlated subquery as `list`, so behaviour is unchanged.
    pub async fn list_by_workspace_for(
        &self,
        ws: Uuid,
        user: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dataset, i64, bool)>, BiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM analytics.dataset_columns c WHERE c.dataset_id = datasets.id) AS column_count, \
             EXISTS (SELECT 1 FROM analytics.bookmarks b WHERE b.user_id = $2 AND b.entity_type='dataset' AND b.entity_id = datasets.id) AS bookmarked \
             FROM analytics.datasets WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DatasetWithBm>(sqlx::AssertSqlSafe(sql))
            .bind(ws)
            .bind(user)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.ds, r.column_count, r.bookmarked))
            .collect())
    }

    /// Soft-delete (trashed=true → deleted_at=now()) or restore (false → NULL).
    pub async fn set_trashed(&self, id: Uuid, trashed: bool) -> Result<(), BiError> {
        let sql = if trashed {
            "UPDATE analytics.datasets SET deleted_at = now() WHERE id = $1"
        } else {
            "UPDATE analytics.datasets SET deleted_at = NULL WHERE id = $1"
        };
        sqlx::query(sql).bind(id).execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn add_bookmark(&self, user: Uuid, entity_id: Uuid) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) VALUES ($1,'dataset',$2) \
             ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_bookmark(&self, user: Uuid, entity_id: Uuid) -> Result<(), BiError> {
        sqlx::query(
            "DELETE FROM analytics.bookmarks WHERE user_id=$1 AND entity_type='dataset' AND entity_id=$2",
        )
        .bind(user)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Dataset>, BiError> {
        Ok(sqlx::query_as::<_, Dataset>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM analytics.datasets WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    pub async fn columns(&self, dataset_id: Uuid) -> Result<Vec<DatasetColumn>, BiError> {
        Ok(sqlx::query_as::<_, DatasetColumn>(
            "SELECT column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description, groupby, filterable, format, semantic_type \
             FROM analytics.dataset_columns WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Update name/description, (for virtual datasets) sql, and optionally the
    /// certification (a None certification arg leaves that field unchanged).
    /// Does not touch columns, and DOES NOT TOUCH CERTIFICATION — see [`Self::set_certification`].
    /// An ordinary save must not be able to reach those columns, or the `dataset:certify` gate is
    /// decoration: an editor would simply certify through the update body, which is exactly what
    /// they used to do. The absence of the parameters is the enforcement.
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        sql: Option<&str>,
    ) -> Result<Dataset, BiError> {
        let ds = sqlx::query_as::<_, Dataset>(sqlx::AssertSqlSafe(format!(
            "UPDATE analytics.datasets SET name=$2, description=$3, \
             sql = CASE WHEN sql IS NULL THEN sql ELSE COALESCE($4, sql) END, \
             updated_at = now() WHERE id=$1 RETURNING {COLS}"
        )))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(sql)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| BiError::NotFound(id.to_string()))?;
        Ok(ds)
    }

    /// The only way certification reaches a dataset. `by` is the CERTIFIER'S EMAIL, taken from the
    /// session by the route — never from the request body, which is how an editor used to be able
    /// to attribute their own certification to somebody else.
    pub async fn set_certification(
        &self,
        id: Uuid,
        certified: bool,
        by: &str,
        details: &str,
    ) -> Result<Dataset, BiError> {
        // Withdrawal clears the attribution with it: "not certified, by Ana" is a worse answer
        // than saying nothing, and leaving Ana's name on a withdrawn badge misrepresents her.
        let (by, details) = if certified { (by, details) } else { ("", "") };
        sqlx::query_as::<_, Dataset>(sqlx::AssertSqlSafe(format!(
            "UPDATE analytics.datasets SET certified=$2, certified_by=$3, \
             certification_details=$4 WHERE id=$1 RETURNING {COLS}"
        )))
        .bind(id)
        .bind(certified)
        .bind(by)
        .bind(details)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| BiError::NotFound(id.to_string()))
    }

    /// As [`Self::set_certification`], for one metric. Same permission, because a metric belongs
    /// to a dataset and vouching for the data is one act.
    pub async fn set_metric_certification(
        &self,
        metric_id: Uuid,
        certified: bool,
        by: &str,
        details: &str,
    ) -> Result<(), BiError> {
        let (by, details) = if certified { (by, details) } else { ("", "") };
        sqlx::query(
            "UPDATE analytics.dataset_metrics SET certified=$2, certified_by=$3, \
             certification_details=$4 WHERE id=$1",
        )
        .bind(metric_id)
        .bind(certified)
        .bind(by)
        .bind(details)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Reconcile the *physical* column snapshot against a fresh introspection, mirroring
    /// Superset's fetch_metadata: upsert physical columns by name (preserving user-edited
    /// verbose_name/description), delete physical columns gone from the source, and NEVER
    /// touch calculated columns (expression IS NOT NULL) or metrics. Runs in one tx.
    ///
    /// The Time switch (`is_temporal`) is the user's: a column that still exists with the SAME
    /// engine type keeps it. Only a column whose type changed takes the engine's answer, since the
    /// old choice was made about a different type. The answer names the columns added and removed.
    pub async fn reconcile_physical_columns(
        &self,
        id: Uuid,
        fresh: &[NewColumn],
    ) -> Result<RefreshChanges, BiError> {
        // Defensive: an empty source would make the DELETE below wipe every physical column.
        // A real source always has ≥1 column; introspect() errors (not []) on an unreachable
        // engine, so an empty slice here means something is wrong — refuse rather than destroy.
        if fresh.is_empty() {
            return Err(BiError::BadRequest(
                "source returned no columns; refusing to wipe the column snapshot".into(),
            ));
        }
        let mut tx = self.db.pool.begin().await?;

        let before: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM analytics.dataset_columns \
             WHERE dataset_id = $1 AND expression IS NULL ORDER BY ordinal",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;

        for (i, c) in fresh.iter().enumerate() {
            sqlx::query(
                "INSERT INTO analytics.dataset_columns \
                   (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
                 VALUES ($1,$2,$3,$4,$5,$6, NULL, '', '') \
                 ON CONFLICT (dataset_id, column_name) DO UPDATE SET \
                   data_type = EXCLUDED.data_type, \
                   type_generic = EXCLUDED.type_generic, \
                   is_temporal = CASE WHEN dataset_columns.data_type = EXCLUDED.data_type \
                                      THEN dataset_columns.is_temporal ELSE EXCLUDED.is_temporal END, \
                   ordinal = EXCLUDED.ordinal \
                 WHERE dataset_columns.expression IS NULL",
            )
            .bind(id)
            .bind(&c.column_name)
            .bind(&c.data_type)
            .bind(&c.type_generic)
            .bind(c.is_temporal)
            .bind(i as i32)
            .execute(&mut *tx)
            .await?;
        }

        let fresh_names: Vec<String> = fresh.iter().map(|c| c.column_name.clone()).collect();
        sqlx::query(
            "DELETE FROM analytics.dataset_columns \
             WHERE dataset_id = $1 AND expression IS NULL AND column_name <> ALL($2)",
        )
        .bind(id)
        .bind(&fresh_names)
        .execute(&mut *tx)
        .await?;

        // Renumber calculated columns to follow the physical ones. `sub` is one row per calc
        // column (with a 0-based rank); `phys` is a single scalar count — the cross-join just
        // attaches that scalar to each `sub` row (not a Cartesian blow-up).
        sqlx::query(
            "UPDATE analytics.dataset_columns AS c SET ordinal = sub.rn + phys.n \
             FROM (SELECT id, (row_number() OVER (ORDER BY ordinal)) - 1 AS rn \
                   FROM analytics.dataset_columns WHERE dataset_id = $1 AND expression IS NOT NULL) sub, \
                  (SELECT count(*) AS n FROM analytics.dataset_columns WHERE dataset_id = $1 AND expression IS NULL) phys \
             WHERE c.id = sub.id",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE analytics.datasets SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(RefreshChanges {
            added: fresh_names
                .iter()
                .filter(|n| !before.contains(n))
                .cloned()
                .collect(),
            removed: before
                .into_iter()
                .filter(|n| !fresh_names.contains(n))
                .collect(),
        })
    }

    /// Copy a dataset of either kind under a new name: the source, every column with all its
    /// settings (calculated columns included) and every metric. Certification is NOT copied — a
    /// copy has not been vouched for — and neither are shares, tags or bookmarks; the copy belongs
    /// to `user`. One transaction.
    pub async fn duplicate(
        &self,
        id: Uuid,
        name: &str,
        user: Option<UserId>,
    ) -> Result<Dataset, BiError> {
        let mut tx = self.db.pool.begin().await?;
        let ds = match sqlx::query_as::<_, Dataset>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO analytics.datasets (workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by, folder_id) \
             SELECT workspace_id, database_id, $2, description, catalog, schema_name, table_name, sql, $3, folder_id \
             FROM analytics.datasets WHERE id = $1 RETURNING {COLS}"
        )))
        .bind(id)
        .bind(name)
        .bind(user.map(|u| u.0))
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(Some(d)) => d,
            Ok(None) => return Err(BiError::NotFound("dataset".into())),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(BiError::Conflict("a dataset with this name already exists".into()));
            }
            Err(e) => return Err(BiError::Db(e)),
        };
        sqlx::query(
            "INSERT INTO analytics.dataset_columns \
               (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, \
                verbose_name, description, groupby, filterable, format, semantic_type) \
             SELECT $2, column_name, data_type, type_generic, is_temporal, ordinal, expression, \
                verbose_name, description, groupby, filterable, format, semantic_type \
             FROM analytics.dataset_columns WHERE dataset_id = $1",
        )
        .bind(id)
        .bind(ds.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO analytics.dataset_metrics \
               (dataset_id, metric_name, expression, data_type, type_generic, verbose_name, description, format, ordinal) \
             SELECT $2, metric_name, expression, data_type, type_generic, verbose_name, description, format, ordinal \
             FROM analytics.dataset_metrics WHERE dataset_id = $1",
        )
        .bind(id)
        .bind(ds.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ds)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM analytics.datasets WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn set_folder(&self, id: Uuid, folder_id: Option<Uuid>) -> Result<(), BiError> {
        sqlx::query("UPDATE analytics.datasets SET folder_id=$2 WHERE id=$1")
            .bind(id)
            .bind(folder_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar("SELECT created_by FROM analytics.datasets WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten(),
        )
    }

    pub async fn metrics(&self, dataset_id: Uuid) -> Result<Vec<Metric>, BiError> {
        Ok(sqlx::query_as::<_, Metric>(
            "SELECT id, metric_name, expression, data_type, type_generic, verbose_name, description, ordinal, format, certified, certified_by, certification_details \
             FROM analytics.dataset_metrics WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Charts that use this dataset (Usage view): live ones only.
    pub async fn usage(&self, dataset_id: Uuid) -> Result<Vec<DatasetUsage>, BiError> {
        Ok(sqlx::query_as::<_, DatasetUsage>(
            "SELECT id, name, viz_type, false AS trashed FROM analytics.charts \
             WHERE dataset_id = $1 AND deleted_at IS NULL ORDER BY updated_at DESC",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// EVERY chart built on this dataset, the trashed ones included: deleting the dataset
    /// deletes all of them (`charts.dataset_id … ON DELETE CASCADE`), so this is the list a
    /// delete has to answer for. Live charts first.
    pub async fn charts_on(&self, dataset_id: Uuid) -> Result<Vec<DatasetUsage>, BiError> {
        Ok(sqlx::query_as::<_, DatasetUsage>(
            "SELECT id, name, viz_type, (deleted_at IS NOT NULL) AS trashed FROM analytics.charts \
             WHERE dataset_id = $1 ORDER BY (deleted_at IS NOT NULL), updated_at DESC",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Charts on this dataset (trashed ones included) whose query names the metric: in
    /// `spec.metrics`, or as an order-by field. Those are the charts that fail with
    /// `unknown metric` once it is renamed or deleted.
    pub async fn charts_using_metric(
        &self,
        dataset_id: Uuid,
        metric_name: &str,
    ) -> Result<Vec<DatasetUsage>, BiError> {
        Ok(sqlx::query_as::<_, DatasetUsage>(
            "SELECT id, name, viz_type, (deleted_at IS NOT NULL) AS trashed FROM analytics.charts \
             WHERE dataset_id = $1 AND ( \
               COALESCE(params->'spec'->'metrics', '[]'::jsonb) ? $2 \
               OR EXISTS (SELECT 1 FROM jsonb_array_elements( \
                    CASE WHEN jsonb_typeof(params->'spec'->'orderby') = 'array' \
                         THEN params->'spec'->'orderby' ELSE '[]'::jsonb END) o \
                  WHERE o->>'field' = $2)) \
             ORDER BY (deleted_at IS NOT NULL), updated_at DESC",
        )
        .bind(dataset_id)
        .bind(metric_name)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// A new metric is NEVER born certified — see [`Self::set_metric_certification`], which is
    /// the only path to those columns. Accepting them here would let an editor create a
    /// pre-vouched metric in one call and skip the gate entirely.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_metric(
        &self,
        dataset_id: Uuid,
        name: &str,
        expression: &str,
        data_type: &str,
        type_generic: &str,
        verbose_name: &str,
        description: &str,
        format: &str,
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "INSERT INTO analytics.dataset_metrics \
               (dataset_id, metric_name, expression, data_type, type_generic, verbose_name, description, format, ordinal) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8, \
               (SELECT COALESCE(MAX(ordinal),-1)+1 FROM analytics.dataset_metrics WHERE dataset_id=$1))",
        )
        .bind(dataset_id)
        .bind(name)
        .bind(expression)
        .bind(data_type)
        .bind(type_generic)
        .bind(verbose_name)
        .bind(description)
        .bind(format)
        .execute(&self.db.pool)
        .await;
        conflict_on_unique(r, "metric name already exists").map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_metric(
        &self,
        metric_id: Uuid,
        name: &str,
        expression: &str,
        data_type: &str,
        type_generic: &str,
        verbose_name: &str,
        description: &str,
        format: &str,
    ) -> Result<(), BiError> {
        // Certification is absent here for the same reason it is absent from `update`: editing a
        // metric's expression must not be a way to grant it a badge.
        let r = sqlx::query(
            "UPDATE analytics.dataset_metrics SET metric_name=$2, expression=$3, data_type=$4, \
               type_generic=$5, verbose_name=$6, description=$7, format=$8 WHERE id=$1",
        )
        .bind(metric_id)
        .bind(name)
        .bind(expression)
        .bind(data_type)
        .bind(type_generic)
        .bind(verbose_name)
        .bind(description)
        .bind(format)
        .execute(&self.db.pool)
        .await;
        conflict_on_unique(r, "metric name already exists").map(|_| ())
    }

    /// Rename a metric (and save its other fields) AND rewrite every chart on the dataset that
    /// names it, in one transaction, so no chart is ever left pointing at a name that no longer
    /// exists. The chart rows are locked before they are read so a concurrent chart save cannot
    /// be overwritten with a stale rewrite. A dashboard card's `options.vizOverride` for those
    /// charts carries the same settings keys, so it is rewritten the same way.
    #[allow(clippy::too_many_arguments)]
    pub async fn rename_metric(
        &self,
        dataset_id: Uuid,
        metric_id: Uuid,
        old_name: &str,
        name: &str,
        expression: &str,
        data_type: &str,
        type_generic: &str,
        verbose_name: &str,
        description: &str,
        format: &str,
    ) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        let r = sqlx::query(
            "UPDATE analytics.dataset_metrics SET metric_name=$2, expression=$3, data_type=$4, \
               type_generic=$5, verbose_name=$6, description=$7, format=$8 WHERE id=$1",
        )
        .bind(metric_id)
        .bind(name)
        .bind(expression)
        .bind(data_type)
        .bind(type_generic)
        .bind(verbose_name)
        .bind(description)
        .bind(format)
        .execute(&mut *tx)
        .await;
        conflict_on_unique(r, "metric name already exists")?;

        let charts: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, params FROM analytics.charts WHERE dataset_id = $1 ORDER BY id FOR UPDATE",
        )
        .bind(dataset_id)
        .fetch_all(&mut *tx)
        .await?;
        for (chart_id, mut params) in charts {
            if !crate::chartparams::rename_metric(&mut params, old_name, name) {
                continue;
            }
            sqlx::query(
                "UPDATE analytics.charts SET params = $2, updated_at = now() WHERE id = $1",
            )
            .bind(chart_id)
            .bind(&params)
            .execute(&mut *tx)
            .await?;
            let cards: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
                "SELECT id, options FROM analytics.dashboard_cards \
                 WHERE chart_id = $1 AND options ? 'vizOverride' ORDER BY id FOR UPDATE",
            )
            .bind(chart_id)
            .fetch_all(&mut *tx)
            .await?;
            for (card_id, mut options) in cards {
                let changed = options
                    .get_mut("vizOverride")
                    .map(|v| crate::chartparams::rename_in_settings(v, old_name, name))
                    .unwrap_or(false);
                if changed {
                    sqlx::query("UPDATE analytics.dashboard_cards SET options = $2 WHERE id = $1")
                        .bind(card_id)
                        .bind(&options)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_metric(&self, metric_id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM analytics.dataset_metrics WHERE id=$1")
            .bind(metric_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn metric_dataset(&self, metric_id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar("SELECT dataset_id FROM analytics.dataset_metrics WHERE id=$1")
                .bind(metric_id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_calc_column(
        &self,
        dataset_id: Uuid,
        name: &str,
        expression: &str,
        data_type: &str,
        type_generic: &str,
        is_temporal: bool,
        verbose_name: &str,
        description: &str,
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "INSERT INTO analytics.dataset_columns \
               (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
             VALUES ($1,$2,$3,$4,$5, \
               (SELECT COALESCE(MAX(ordinal),-1)+1 FROM analytics.dataset_columns WHERE dataset_id=$1), $6,$7,$8)",
        )
        .bind(dataset_id)
        .bind(name)
        .bind(data_type)
        .bind(type_generic)
        .bind(is_temporal)
        .bind(expression)
        .bind(verbose_name)
        .bind(description)
        .execute(&self.db.pool)
        .await;
        conflict_on_unique(r, "column name already exists").map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_column(
        &self,
        dataset_id: Uuid,
        column_name: &str,
        verbose_name: Option<&str>,
        description: Option<&str>,
        groupby: Option<bool>,
        filterable: Option<bool>,
        is_temporal: Option<bool>,
        format: Option<&str>,
        semantic_type: Option<&str>,
        expr_update: Option<(&str, &str, &str, bool)>,
    ) -> Result<(), BiError> {
        // Both updates in one tx so a metadata+expression edit applies atomically.
        let mut tx = self.db.pool.begin().await?;
        sqlx::query(
            "UPDATE analytics.dataset_columns SET \
               verbose_name  = COALESCE($3, verbose_name), \
               description   = COALESCE($4, description), \
               groupby       = COALESCE($5, groupby), \
               filterable    = COALESCE($6, filterable), \
               is_temporal   = COALESCE($7, is_temporal), \
               format        = COALESCE($8, format), \
               semantic_type = COALESCE($9, semantic_type) \
             WHERE dataset_id=$1 AND column_name=$2",
        )
        .bind(dataset_id)
        .bind(column_name)
        .bind(verbose_name)
        .bind(description)
        .bind(groupby)
        .bind(filterable)
        .bind(is_temporal)
        .bind(format)
        .bind(semantic_type)
        .execute(&mut *tx)
        .await?;

        if let Some((expr, dt, tg, temporal)) = expr_update {
            sqlx::query(
                "UPDATE analytics.dataset_columns SET expression=$3, data_type=$4, type_generic=$5, is_temporal=$6 \
                 WHERE dataset_id=$1 AND column_name=$2 AND expression IS NOT NULL",
            )
            .bind(dataset_id)
            .bind(column_name)
            .bind(expr)
            .bind(dt)
            .bind(tg)
            .bind(temporal)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_calc_column(
        &self,
        dataset_id: Uuid,
        column_name: &str,
    ) -> Result<(), BiError> {
        let n = sqlx::query(
            "DELETE FROM analytics.dataset_columns WHERE dataset_id=$1 AND column_name=$2 AND expression IS NOT NULL",
        )
        .bind(dataset_id)
        .bind(column_name)
        .execute(&self.db.pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(BiError::BadRequest(
                "only calculated columns can be deleted".into(),
            ));
        }
        Ok(())
    }

    pub async fn column_is_calculated(
        &self,
        dataset_id: Uuid,
        column_name: &str,
    ) -> Result<Option<bool>, BiError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT (expression IS NOT NULL) FROM analytics.dataset_columns WHERE dataset_id=$1 AND column_name=$2",
        )
        .bind(dataset_id)
        .bind(column_name)
        .fetch_optional(&self.db.pool)
        .await?)
    }
}

async fn insert_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dataset_id: Uuid,
    columns: &[NewColumn],
) -> Result<(), BiError> {
    for (i, c) in columns.iter().enumerate() {
        sqlx::query(
            "INSERT INTO analytics.dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
             VALUES ($1,$2,$3,$4,$5,$6, NULL, '', '')",
        )
        .bind(dataset_id)
        .bind(&c.column_name)
        .bind(&c.data_type)
        .bind(&c.type_generic)
        .bind(c.is_temporal)
        .bind(i as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Map a sqlx unique-violation to BiError::Conflict; pass other errors through.
fn conflict_on_unique(
    r: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
    msg: &str,
) -> Result<sqlx::postgres::PgQueryResult, BiError> {
    match r {
        Ok(v) => Ok(v),
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
            Err(BiError::Conflict(msg.into()))
        }
        Err(e) => Err(BiError::Db(e)),
    }
}
