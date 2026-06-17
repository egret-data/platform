//! Postgres-backed CatalogStore: namespace CRUD.

use crate::CatalogError;
use egret_metastore::Db;
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

    pub async fn create_namespace(
        &self,
        levels: &[String],
        props: &HashMap<String, String>,
    ) -> Result<NamespaceRow, CatalogError> {
        let props_json = serde_json::to_value(props).unwrap_or_default();
        sqlx::query_as::<_, NamespaceRow>(
            "INSERT INTO catalog_namespaces (levels, properties) VALUES ($1, $2) \
             RETURNING id, levels, properties",
        )
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

    pub async fn get_namespace(&self, levels: &[String]) -> Result<NamespaceRow, CatalogError> {
        sqlx::query_as::<_, NamespaceRow>(
            "SELECT id, levels, properties FROM catalog_namespaces WHERE levels = $1",
        )
        .bind(levels)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(levels.join(".")))
    }

    pub async fn list_namespaces(
        &self,
        parent: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, CatalogError> {
        match parent {
            None => {
                sqlx::query_scalar::<_, Vec<String>>(
                    "SELECT levels FROM catalog_namespaces ORDER BY levels",
                )
                .fetch_all(&self.db.pool)
                .await
                .map_err(CatalogError::Db)
            }
            Some(p) => {
                sqlx::query_scalar::<_, Vec<String>>(
                    "SELECT levels FROM catalog_namespaces \
                     WHERE levels[1:cardinality($1::text[])] = $1 \
                       AND cardinality(levels) = cardinality($1::text[]) + 1 \
                     ORDER BY levels",
                )
                .bind(p)
                .fetch_all(&self.db.pool)
                .await
                .map_err(CatalogError::Db)
            }
        }
    }

    pub async fn drop_namespace(&self, levels: &[String]) -> Result<(), CatalogError> {
        let ns = self.get_namespace(levels).await?;
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
        levels: &[String],
        removals: &[String],
        updates: &HashMap<String, String>,
    ) -> Result<(Vec<String>, Vec<String>), CatalogError> {
        let ns = self.get_namespace(levels).await?;
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
