//! `CatalogService` — orchestration layer that writes `metadata.json` to object storage
//! and delegates DB mutations to `CatalogStore`. This is what the HTTP handlers call.

use crate::service::{commit::apply_commit, metadata::create_table_metadata, CatalogStore};
use crate::CatalogError;
use egret_iceberg_ext::catalog::{CommitTableRequest, CreateTableRequest, LoadTableResult};
use egret_storage::{table_metadata_location, ObjectStore};
use iceberg::spec::TableMetadata;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct CatalogService {
    pub store: CatalogStore,
    pub storage: Arc<dyn ObjectStore>,
}

impl CatalogService {
    /// Serialize `md` to JSON and write to `<location>/metadata/<version>-<uuid>.metadata.json`.
    /// Returns the full URI of the written file.
    async fn write_metadata_json(
        &self,
        location: &str,
        version: i32,
        md: &TableMetadata,
    ) -> Result<String, CatalogError> {
        let mloc = table_metadata_location(location, version, &Uuid::new_v4());
        let bytes = serde_json::to_vec(md).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        self.storage.put(&mloc, bytes).await?;
        Ok(mloc)
    }

    /// Create a new table: build initial metadata (v0), write `metadata.json`, insert DB row.
    pub async fn create_table(
        &self,
        ns_levels: &[String],
        req: CreateTableRequest,
    ) -> Result<LoadTableResult, CatalogError> {
        let ns = self.store.get_namespace(ns_levels).await?;
        let table_uuid = Uuid::new_v4();
        let location = req.location.clone().unwrap_or_else(|| {
            egret_storage::table_base_location(
                "s3://placeholder",
                ns_levels,
                &req.name,
                &table_uuid,
            )
        });
        let md = create_table_metadata(&req, table_uuid, location.clone())?;
        let mloc = self.write_metadata_json(&location, 0, &md).await?;
        self.store
            .insert_table(ns.id, &req.name, table_uuid, &md, &mloc, &location)
            .await?;
        Ok(LoadTableResult {
            metadata_location: Some(mloc),
            metadata: Arc::new(md),
            config: None,
        })
    }

    /// Load the current state of a table from the DB.
    pub async fn load_table(
        &self,
        ns_levels: &[String],
        name: &str,
    ) -> Result<LoadTableResult, CatalogError> {
        let ns = self.store.get_namespace(ns_levels).await?;
        let row = self.store.load_table(ns.id, name).await?;
        Ok(LoadTableResult {
            metadata_location: Some(row.metadata_location),
            metadata: Arc::new(row.metadata),
            config: None,
        })
    }

    /// Apply a commit request to a table: validate requirements, apply updates,
    /// write new `metadata.json`, then CAS-update the DB row.
    pub async fn commit_table(
        &self,
        ns_levels: &[String],
        name: &str,
        req: CommitTableRequest,
    ) -> Result<LoadTableResult, CatalogError> {
        let ns = self.store.get_namespace(ns_levels).await?;
        let row = self.store.load_table(ns.id, name).await?;
        let new_md = apply_commit(
            row.metadata.clone(),
            Some(&row.metadata_location),
            &req.requirements,
            req.updates,
        )?;
        let mloc = self
            .write_metadata_json(&row.location, row.current_version + 1, &new_md)
            .await?;
        self.store
            .cas_commit(row.id, row.current_version, &new_md, &mloc)
            .await?;
        Ok(LoadTableResult {
            metadata_location: Some(mloc),
            metadata: Arc::new(new_md),
            config: None,
        })
    }
}
