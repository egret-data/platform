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
    Physical { catalog: Option<String>, schema: String, table: String },
    Virtual { sql: String },
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
            SourceSpec::Physical { catalog, schema, table } => {
                (catalog.clone(), Some(schema.clone()), Some(table.clone()), None)
            }
            SourceSpec::Virtual { sql } => (None, None, None, Some(sql.clone())),
        };

        let mut tx = self.db.pool.begin().await?;
        let ds = sqlx::query_as::<_, Dataset>(&format!(
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
        .await?;

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
            "SELECT column_name, data_type, type_generic, is_temporal, ordinal \
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

    /// Replace the column snapshot in one transaction (used by refresh).
    pub async fn replace_columns(&self, id: Uuid, columns: &[NewColumn]) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM dataset_columns WHERE dataset_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        insert_columns(&mut tx, id, columns).await?;
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
}

async fn insert_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dataset_id: Uuid,
    columns: &[NewColumn],
) -> Result<(), BiError> {
    for (i, c) in columns.iter().enumerate() {
        sqlx::query(
            "INSERT INTO dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal) \
             VALUES ($1,$2,$3,$4,$5,$6)",
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
