//! Postgres-backed CatalogStore: namespace CRUD + table CRUD + CAS commit.

use crate::CatalogError;
use egret_metastore::Db;
use iceberg::spec::TableMetadata;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct CatalogStore {
    db: Db,
}

#[derive(Debug, sqlx::FromRow)]
pub struct NamespaceRow {
    pub id: Uuid,
    pub levels: Vec<String>,
    pub properties: serde_json::Value,
}

impl CatalogStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    pub async fn create_namespace(
        &self,
        catalog_id: Uuid,
        levels: &[String],
        props: &HashMap<String, String>,
    ) -> Result<NamespaceRow, CatalogError> {
        let props_json = serde_json::to_value(props).unwrap_or_default();
        sqlx::query_as::<_, NamespaceRow>(
            "INSERT INTO catalog_namespaces (catalog_id, levels, properties) VALUES ($1, $2, $3) \
             RETURNING id, levels, properties",
        )
        .bind(catalog_id)
        .bind(levels)
        .bind(props_json)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists(levels.join("."))
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn get_namespace(
        &self,
        catalog_id: Uuid,
        levels: &[String],
    ) -> Result<NamespaceRow, CatalogError> {
        sqlx::query_as::<_, NamespaceRow>(
            "SELECT id, levels, properties FROM catalog_namespaces \
             WHERE catalog_id = $1 AND levels = $2",
        )
        .bind(catalog_id)
        .bind(levels)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(levels.join(".")))
    }

    pub async fn list_namespaces(
        &self,
        catalog_id: Uuid,
        parent: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, CatalogError> {
        match parent {
            None => sqlx::query_scalar::<_, Vec<String>>(
                "SELECT levels FROM catalog_namespaces WHERE catalog_id = $1 ORDER BY levels",
            )
            .bind(catalog_id)
            .fetch_all(&self.db.pool)
            .await
            .map_err(CatalogError::Db),
            Some(p) => sqlx::query_scalar::<_, Vec<String>>(
                "SELECT levels FROM catalog_namespaces \
                     WHERE catalog_id = $1 \
                       AND levels[1:cardinality($2::text[])] = $2 \
                       AND cardinality(levels) = cardinality($2::text[]) + 1 \
                     ORDER BY levels",
            )
            .bind(catalog_id)
            .bind(p)
            .fetch_all(&self.db.pool)
            .await
            .map_err(CatalogError::Db),
        }
    }

    pub async fn drop_namespace(
        &self,
        catalog_id: Uuid,
        levels: &[String],
    ) -> Result<(), CatalogError> {
        let ns = self.get_namespace(catalog_id, levels).await?;
        let count: i64 = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_tables WHERE namespace_id = $1",
        )
        .bind(ns.id)
        .fetch_one(&self.db.pool)
        .await
        .map_err(CatalogError::Db)?;
        if count > 0 {
            return Err(CatalogError::NamespaceNotEmpty(levels.join(".")));
        }
        sqlx::query("DELETE FROM catalog_namespaces WHERE id = $1")
            .bind(ns.id)
            .execute(&self.db.pool)
            .await
            .map_err(CatalogError::Db)?;
        Ok(())
    }

    pub async fn update_namespace_properties(
        &self,
        catalog_id: Uuid,
        levels: &[String],
        removals: &[String],
        updates: &HashMap<String, String>,
    ) -> Result<(Vec<String>, Vec<String>), CatalogError> {
        let ns = self.get_namespace(catalog_id, levels).await?;
        let mut props: HashMap<String, String> =
            serde_json::from_value(ns.properties).unwrap_or_default();
        let mut removed = vec![];
        for k in removals {
            if props.remove(k).is_some() {
                removed.push(k.clone());
            }
        }
        let mut updated = vec![];
        for (k, v) in updates {
            props.insert(k.clone(), v.clone());
            updated.push(k.clone());
        }
        let json = serde_json::to_value(&props).unwrap_or_default();
        sqlx::query("UPDATE catalog_namespaces SET properties = $2 WHERE id = $1")
            .bind(ns.id)
            .bind(json)
            .execute(&self.db.pool)
            .await
            .map_err(CatalogError::Db)?;
        Ok((updated, removed))
    }
}

// ---------------------------------------------------------------------------
// Table rows
// ---------------------------------------------------------------------------

/// Internal FromRow struct for raw DB fetch (metadata column stays as JSON value).
#[derive(sqlx::FromRow)]
struct TableRowRaw {
    id: Uuid,
    name: String,
    table_uuid: Uuid,
    metadata: serde_json::Value,
    metadata_location: String,
    location: String,
    current_version: i32,
}

/// A fully-deserialized table row returned from the store.
pub struct TableRow {
    pub id: Uuid,
    pub name: String,
    pub table_uuid: Uuid,
    pub metadata: TableMetadata,
    pub metadata_location: String,
    pub location: String,
    pub current_version: i32,
}

impl TableRowRaw {
    fn into_table_row(self) -> Result<TableRow, CatalogError> {
        let metadata: TableMetadata = serde_json::from_value(self.metadata)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        Ok(TableRow {
            id: self.id,
            name: self.name,
            table_uuid: self.table_uuid,
            metadata,
            metadata_location: self.metadata_location,
            location: self.location,
            current_version: self.current_version,
        })
    }
}

// ---------------------------------------------------------------------------
// CatalogStore — table methods
// ---------------------------------------------------------------------------

impl CatalogStore {
    /// Insert a freshly-created table (version 0). `metadata` already has its uuid/location.
    pub async fn insert_table(
        &self,
        ns_id: Uuid,
        name: &str,
        table_uuid: Uuid,
        metadata: &TableMetadata,
        metadata_location: &str,
        location: &str,
    ) -> Result<(), CatalogError> {
        let md =
            serde_json::to_value(metadata).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        let snap: Option<i64> = metadata.current_snapshot_id();
        sqlx::query(
            "INSERT INTO catalog_tables \
               (namespace_id, name, table_uuid, metadata, metadata_location, location, \
                current_snapshot_id, current_version) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 0)",
        )
        .bind(ns_id)
        .bind(name)
        .bind(table_uuid)
        .bind(md)
        .bind(metadata_location)
        .bind(location)
        .bind(snap)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists(name.into())
            }
            _ => CatalogError::Db(e),
        })?;
        Ok(())
    }

    /// Load a table by namespace + name.
    pub async fn load_table(&self, ns_id: Uuid, name: &str) -> Result<TableRow, CatalogError> {
        sqlx::query_as::<_, TableRowRaw>(
            "SELECT id, name, table_uuid, metadata, metadata_location, location, current_version \
             FROM catalog_tables WHERE namespace_id = $1 AND name = $2",
        )
        .bind(ns_id)
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(name.into()))?
        .into_table_row()
    }

    /// List table names in a namespace, ordered alphabetically.
    pub async fn list_tables(&self, ns_id: Uuid) -> Result<Vec<String>, CatalogError> {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM catalog_tables WHERE namespace_id = $1 ORDER BY name",
        )
        .bind(ns_id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(CatalogError::Db)
    }

    /// Drop a table and return its last-known row.
    pub async fn drop_table(&self, ns_id: Uuid, name: &str) -> Result<TableRow, CatalogError> {
        let row = self.load_table(ns_id, name).await?;
        sqlx::query("DELETE FROM catalog_tables WHERE id = $1")
            .bind(row.id)
            .execute(&self.db.pool)
            .await
            .map_err(CatalogError::Db)?;
        Ok(row)
    }

    /// Rename a table, optionally moving it to a different namespace.
    pub async fn rename_table(
        &self,
        src_ns: Uuid,
        src: &str,
        dst_ns: Uuid,
        dst: &str,
    ) -> Result<(), CatalogError> {
        let row = self.load_table(src_ns, src).await?;
        sqlx::query(
            "UPDATE catalog_tables \
             SET namespace_id = $2, name = $3, updated_at = now() WHERE id = $1",
        )
        .bind(row.id)
        .bind(dst_ns)
        .bind(dst)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists(dst.into())
            }
            _ => CatalogError::Db(e),
        })?;
        Ok(())
    }

    /// Compare-and-swap commit: write new metadata only if `current_version` equals
    /// `expected_version`. Returns `Err(Conflict)` if another writer advanced the version.
    pub async fn cas_commit(
        &self,
        table_id: Uuid,
        expected_version: i32,
        metadata: &TableMetadata,
        new_metadata_location: &str,
    ) -> Result<(), CatalogError> {
        let md =
            serde_json::to_value(metadata).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        let snap: Option<i64> = metadata.current_snapshot_id();
        let mut tx = self.db.pool.begin().await.map_err(CatalogError::Db)?;
        let affected = sqlx::query(
            "UPDATE catalog_tables \
               SET metadata = $3, \
                   previous_metadata_location = metadata_location, \
                   metadata_location = $4, \
                   current_snapshot_id = $5, \
                   current_version = current_version + 1, \
                   updated_at = now() \
             WHERE id = $1 AND current_version = $2",
        )
        .bind(table_id)
        .bind(expected_version)
        .bind(md)
        .bind(new_metadata_location)
        .bind(snap)
        .execute(&mut *tx)
        .await
        .map_err(CatalogError::Db)?
        .rows_affected();

        if affected == 0 {
            tx.rollback().await.map_err(CatalogError::Db)?;
            return Err(CatalogError::Conflict("stale table version".into()));
        }

        sqlx::query(
            "INSERT INTO catalog_table_metadata_log (table_id, metadata_location) \
             VALUES ($1, $2)",
        )
        .bind(table_id)
        .bind(new_metadata_location)
        .execute(&mut *tx)
        .await
        .map_err(CatalogError::Db)?;

        tx.commit().await.map_err(CatalogError::Db)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// View rows
// ---------------------------------------------------------------------------

/// Internal FromRow struct for raw DB fetch (metadata column stays as JSON value).
#[derive(sqlx::FromRow)]
struct ViewRowRaw {
    id: Uuid,
    name: String,
    metadata: serde_json::Value,
    metadata_location: String,
    location: String,
    current_version: i32,
}

/// A view row with metadata stored as an opaque JSON blob.
pub struct ViewRow {
    pub id: Uuid,
    pub name: String,
    pub metadata: serde_json::Value,
    pub metadata_location: String,
    pub location: String,
    pub current_version: i32,
}

// ---------------------------------------------------------------------------
// CatalogStore — view methods
// ---------------------------------------------------------------------------

impl CatalogStore {
    /// Insert a new view. Returns `AlreadyExists` on unique-constraint violation.
    pub async fn insert_view(
        &self,
        ns_id: Uuid,
        name: &str,
        metadata: &serde_json::Value,
        metadata_location: &str,
        location: &str,
    ) -> Result<(), CatalogError> {
        sqlx::query(
            "INSERT INTO catalog_views \
               (namespace_id, name, metadata, metadata_location, location, current_version) \
             VALUES ($1, $2, $3, $4, $5, 0)",
        )
        .bind(ns_id)
        .bind(name)
        .bind(metadata)
        .bind(metadata_location)
        .bind(location)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists(name.into())
            }
            _ => CatalogError::Db(e),
        })?;
        Ok(())
    }

    /// Load a view by namespace + name. Returns `NotFound` if absent.
    pub async fn load_view(&self, ns_id: Uuid, name: &str) -> Result<ViewRow, CatalogError> {
        let raw = sqlx::query_as::<_, ViewRowRaw>(
            "SELECT id, name, metadata, metadata_location, location, current_version \
             FROM catalog_views WHERE namespace_id = $1 AND name = $2",
        )
        .bind(ns_id)
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(name.into()))?;
        Ok(ViewRow {
            id: raw.id,
            name: raw.name,
            metadata: raw.metadata,
            metadata_location: raw.metadata_location,
            location: raw.location,
            current_version: raw.current_version,
        })
    }

    /// List view names in a namespace, ordered alphabetically.
    pub async fn list_views(&self, ns_id: Uuid) -> Result<Vec<String>, CatalogError> {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM catalog_views WHERE namespace_id = $1 ORDER BY name",
        )
        .bind(ns_id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(CatalogError::Db)
    }

    /// Drop a view (load then delete). Returns `NotFound` if absent.
    pub async fn drop_view(&self, ns_id: Uuid, name: &str) -> Result<(), CatalogError> {
        let v = self.load_view(ns_id, name).await?;
        sqlx::query("DELETE FROM catalog_views WHERE id = $1")
            .bind(v.id)
            .execute(&self.db.pool)
            .await
            .map_err(CatalogError::Db)?;
        Ok(())
    }
}
