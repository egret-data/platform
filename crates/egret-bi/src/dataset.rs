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
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
}

const COLS: &str = "id, workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by, updated_at";

/// Private struct for list() — sqlx tuple FromRow is version-sensitive; use flatten instead.
#[derive(sqlx::FromRow)]
struct DatasetWithCount {
    #[sqlx(flatten)]
    ds: Dataset,
    column_count: i64,
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
        let ds = match sqlx::query_as::<_, Dataset>(&format!(
            "INSERT INTO datasets (workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING {COLS}"
        ))
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
        let rows = sqlx::query_as::<_, DatasetWithCount>(&format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dataset_columns c WHERE c.dataset_id = datasets.id) AS column_count \
             FROM datasets WHERE workspace_id = $1 ORDER BY updated_at DESC"
        ))
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.ds, r.column_count)).collect())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Dataset>, BiError> {
        Ok(
            sqlx::query_as::<_, Dataset>(&format!("SELECT {COLS} FROM datasets WHERE id = $1"))
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    pub async fn columns(&self, dataset_id: Uuid) -> Result<Vec<DatasetColumn>, BiError> {
        Ok(sqlx::query_as::<_, DatasetColumn>(
            "SELECT column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description \
             FROM dataset_columns WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Update name/description, and (for virtual datasets) sql. Does not touch columns.
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        sql: Option<&str>,
    ) -> Result<Dataset, BiError> {
        let ds = sqlx::query_as::<_, Dataset>(&format!(
            "UPDATE datasets SET name=$2, description=$3, \
             sql = CASE WHEN sql IS NULL THEN sql ELSE COALESCE($4, sql) END, \
             updated_at = now() WHERE id=$1 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(sql)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| BiError::NotFound(id.to_string()))?;
        Ok(ds)
    }

    /// Reconcile the *physical* column snapshot against a fresh introspection, mirroring
    /// Superset's fetch_metadata: upsert physical columns by name (preserving user-edited
    /// verbose_name/description), delete physical columns gone from the source, and NEVER
    /// touch calculated columns (expression IS NOT NULL) or metrics. Runs in one tx.
    pub async fn reconcile_physical_columns(
        &self,
        id: Uuid,
        fresh: &[NewColumn],
    ) -> Result<(), BiError> {
        // Defensive: an empty source would make the DELETE below wipe every physical column.
        // A real source always has ≥1 column; introspect() errors (not []) on an unreachable
        // engine, so an empty slice here means something is wrong — refuse rather than destroy.
        if fresh.is_empty() {
            return Err(BiError::BadRequest(
                "source returned no columns; refusing to wipe the column snapshot".into(),
            ));
        }
        let mut tx = self.db.pool.begin().await?;

        for (i, c) in fresh.iter().enumerate() {
            sqlx::query(
                "INSERT INTO dataset_columns \
                   (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
                 VALUES ($1,$2,$3,$4,$5,$6, NULL, '', '') \
                 ON CONFLICT (dataset_id, column_name) DO UPDATE SET \
                   data_type = EXCLUDED.data_type, \
                   type_generic = EXCLUDED.type_generic, \
                   is_temporal = EXCLUDED.is_temporal, \
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
            "DELETE FROM dataset_columns \
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
            "UPDATE dataset_columns AS c SET ordinal = sub.rn + phys.n \
             FROM (SELECT id, (row_number() OVER (ORDER BY ordinal)) - 1 AS rn \
                   FROM dataset_columns WHERE dataset_id = $1 AND expression IS NOT NULL) sub, \
                  (SELECT count(*) AS n FROM dataset_columns WHERE dataset_id = $1 AND expression IS NULL) phys \
             WHERE c.id = sub.id",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE datasets SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM datasets WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar("SELECT created_by FROM datasets WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten(),
        )
    }

    pub async fn metrics(&self, dataset_id: Uuid) -> Result<Vec<Metric>, BiError> {
        Ok(sqlx::query_as::<_, Metric>(
            "SELECT id, metric_name, expression, data_type, type_generic, verbose_name, description, ordinal \
             FROM dataset_metrics WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

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
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "INSERT INTO dataset_metrics \
               (dataset_id, metric_name, expression, data_type, type_generic, verbose_name, description, ordinal) \
             VALUES ($1,$2,$3,$4,$5,$6,$7, \
               (SELECT COALESCE(MAX(ordinal),-1)+1 FROM dataset_metrics WHERE dataset_id=$1))",
        )
        .bind(dataset_id)
        .bind(name)
        .bind(expression)
        .bind(data_type)
        .bind(type_generic)
        .bind(verbose_name)
        .bind(description)
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
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "UPDATE dataset_metrics SET metric_name=$2, expression=$3, data_type=$4, \
               type_generic=$5, verbose_name=$6, description=$7 WHERE id=$1",
        )
        .bind(metric_id)
        .bind(name)
        .bind(expression)
        .bind(data_type)
        .bind(type_generic)
        .bind(verbose_name)
        .bind(description)
        .execute(&self.db.pool)
        .await;
        conflict_on_unique(r, "metric name already exists").map(|_| ())
    }

    pub async fn delete_metric(&self, metric_id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM dataset_metrics WHERE id=$1")
            .bind(metric_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn metric_dataset(&self, metric_id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar("SELECT dataset_id FROM dataset_metrics WHERE id=$1")
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
            "INSERT INTO dataset_columns \
               (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
             VALUES ($1,$2,$3,$4,$5, \
               (SELECT COALESCE(MAX(ordinal),-1)+1 FROM dataset_columns WHERE dataset_id=$1), $6,$7,$8)",
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

    pub async fn update_column(
        &self,
        dataset_id: Uuid,
        column_name: &str,
        verbose_name: Option<&str>,
        description: Option<&str>,
        expr_update: Option<(&str, &str, &str, bool)>,
    ) -> Result<(), BiError> {
        // Both updates in one tx so a metadata+expression edit applies atomically.
        let mut tx = self.db.pool.begin().await?;
        sqlx::query(
            "UPDATE dataset_columns SET \
               verbose_name = COALESCE($3, verbose_name), \
               description  = COALESCE($4, description) \
             WHERE dataset_id=$1 AND column_name=$2",
        )
        .bind(dataset_id)
        .bind(column_name)
        .bind(verbose_name)
        .bind(description)
        .execute(&mut *tx)
        .await?;

        if let Some((expr, dt, tg, temporal)) = expr_update {
            sqlx::query(
                "UPDATE dataset_columns SET expression=$3, data_type=$4, type_generic=$5, is_temporal=$6 \
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
            "DELETE FROM dataset_columns WHERE dataset_id=$1 AND column_name=$2 AND expression IS NOT NULL",
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
            "SELECT (expression IS NOT NULL) FROM dataset_columns WHERE dataset_id=$1 AND column_name=$2",
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
            "INSERT INTO dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
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
