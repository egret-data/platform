//! Normalized Iceberg view store: shred-on-write / assemble-on-read.
//!
//! The single-JSONB-blob view store has been replaced with a normalized schema
//! (migration 0023). On write we *shred* a [`ViewMetadata`] into its child rows
//! (schemas, versions, representations, version log, properties); on read we
//! *assemble* those rows back into a [`ViewMetadata`].
//!
//! The shape is ported from lakekeeper's `populate_view_metadata` /
//! `clear_view_metadata` / `load_view` (`lakekeeper-storage-postgres`), which
//! targets the same iceberg-rust fork. egret is view-format V1-only.
//!
//! Unlike lakekeeper (which resolves a view-version's `default-namespace` to a
//! surrogate `namespace_id` FK), egret stores the `NamespaceIdent` directly as
//! JSONB in `catalog_view_versions.default_namespace`, so no namespace lookup is
//! needed on either side.

use std::collections::HashMap;
use std::sync::Arc;

use iceberg::spec::{
    Schema, SchemaId, SqlViewRepresentation, ViewFormatVersion, ViewMetadata, ViewMetadataParts,
    ViewRepresentation, ViewRepresentations, ViewVersion, ViewVersionId, ViewVersionLog,
};
use iceberg::NamespaceIdent;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::CatalogError;

/// The `catalog_views` root row, holding the scalar columns needed to
/// reconstruct a [`ViewMetadata`] together with the child rows.
#[derive(sqlx::FromRow)]
pub struct ViewRootRow {
    pub id: Uuid,
    pub name: String,
    pub view_uuid: Uuid,
    pub location: String,
    pub format_version: i16,
    pub current_version_id: i32,
    pub metadata_location: String,
    pub current_version: i32,
}

/// Map a stored `format_version` smallint to the iceberg view enum (V1 only).
fn format_version_from_i16(v: i16) -> Result<ViewFormatVersion, CatalogError> {
    match v {
        1 => Ok(ViewFormatVersion::V1),
        other => Err(CatalogError::BadRequest(format!(
            "unsupported view format version: {other}"
        ))),
    }
}

/// Map the iceberg view format version to the stored smallint.
pub fn format_version_to_i16(v: ViewFormatVersion) -> i16 {
    match v {
        ViewFormatVersion::V1 => 1,
    }
}

/// Remove all view sub-metadata so a commit can repopulate it from `ViewMetadata`.
///
/// Used by the future view-commit path; clear-then-repopulate.
///
/// Migration 0023 FK chain (children → parents):
///   `catalog_view_representations` FKs `catalog_view_versions` (no CASCADE) →
///   `catalog_view_versions`       FKs `catalog_view_schemas`   (no CASCADE) →
///   `catalog_view_schemas`        FKs `catalog_views`
/// So we must delete leaves before parents: representations → version_log → versions
/// → schemas → properties. There is no cascade from schemas to versions or
/// representations, so every table must be deleted explicitly.
#[allow(dead_code)]
pub async fn clear_view_metadata(
    tx: &mut Transaction<'_, Postgres>,
    view_id: Uuid,
) -> Result<(), CatalogError> {
    // representations reference versions — delete first.
    sqlx::query("DELETE FROM catalog_view_representations WHERE view_id = $1")
        .bind(view_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    // version_log FKs only catalog_views(id), but delete before versions for clarity.
    sqlx::query("DELETE FROM catalog_view_version_log WHERE view_id = $1")
        .bind(view_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    // versions reference schemas — delete before schemas.
    sqlx::query("DELETE FROM catalog_view_versions WHERE view_id = $1")
        .bind(view_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    sqlx::query("DELETE FROM catalog_view_schemas WHERE view_id = $1")
        .bind(view_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    sqlx::query("DELETE FROM catalog_view_properties WHERE view_id = $1")
        .bind(view_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    Ok(())
}

/// Shred all child rows for `view_id` from `md`.
///
/// Order matters: schemas first (FK target for versions), then versions (FK
/// target for representations + version log), then representations, version log,
/// and properties. Mirrors lakekeeper's `populate_view_metadata`.
pub async fn shred_view(
    tx: &mut Transaction<'_, Postgres>,
    view_id: Uuid,
    md: &ViewMetadata,
) -> Result<(), CatalogError> {
    // --- schemas (FK target for catalog_view_versions) ---
    for schema in md.schemas_iter() {
        let schema_json =
            serde_json::to_value(schema).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog_view_schemas (view_id, schema_id, schema) \
             VALUES ($1, $2, $3) ON CONFLICT (view_id, schema_id) DO NOTHING",
        )
        .bind(view_id)
        .bind(schema.schema_id())
        .bind(schema_json)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- versions ---
    for version in md.versions() {
        let summary_json = serde_json::to_value(version.summary())
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        let default_ns_json = serde_json::to_value(version.default_namespace())
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog_view_versions \
               (view_id, version_id, schema_id, timestamp_ms, summary, default_catalog, default_namespace) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (view_id, version_id) DO NOTHING",
        )
        .bind(view_id)
        .bind(version.version_id())
        .bind(version.schema_id())
        .bind(version.timestamp_ms())
        .bind(summary_json)
        .bind(version.default_catalog())
        .bind(default_ns_json)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- representations (FK into catalog_view_versions) ---
    for version in md.versions() {
        for rep in version.representations().iter() {
            let ViewRepresentation::Sql(sql_rep) = rep;
            sqlx::query(
                "INSERT INTO catalog_view_representations \
                   (view_id, version_id, type, sql, dialect) \
                 VALUES ($1, $2, 'sql', $3, $4)",
            )
            .bind(view_id)
            .bind(version.version_id())
            .bind(&sql_rep.sql)
            .bind(&sql_rep.dialect)
            .execute(&mut **tx)
            .await
            .map_err(CatalogError::Db)?;
        }
    }

    // --- version log (ordered append) ---
    for entry in md.history() {
        sqlx::query(
            "INSERT INTO catalog_view_version_log (view_id, version_id, timestamp_ms) \
             VALUES ($1, $2, $3)",
        )
        .bind(view_id)
        .bind(entry.version_id())
        .bind(entry.timestamp_ms())
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- properties ---
    for (key, value) in md.properties() {
        sqlx::query(
            "INSERT INTO catalog_view_properties (view_id, key, value) VALUES ($1, $2, $3) \
             ON CONFLICT (view_id, key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(view_id)
        .bind(key)
        .bind(value)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    Ok(())
}

/// Fetch all child rows for `root.id` and rebuild a [`ViewMetadata`].
///
/// Ports lakekeeper's `load_view`, finishing with `ViewMetadata::try_from_parts`.
pub async fn assemble_view_metadata(
    pool: &sqlx::PgPool,
    root: &ViewRootRow,
) -> Result<ViewMetadata, CatalogError> {
    let view_id = root.id;

    // --- schemas ---
    let schema_rows =
        sqlx::query("SELECT schema_id, schema FROM catalog_view_schemas WHERE view_id = $1")
            .bind(view_id)
            .fetch_all(pool)
            .await
            .map_err(CatalogError::Db)?;
    let mut schemas: HashMap<SchemaId, Arc<Schema>> = HashMap::with_capacity(schema_rows.len());
    for row in schema_rows {
        let json: serde_json::Value = row.try_get("schema").map_err(CatalogError::Db)?;
        let schema: Schema =
            serde_json::from_value(json).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        schemas.insert(schema.schema_id(), Arc::new(schema));
    }

    // --- representations grouped by version_id ---
    let rep_rows = sqlx::query(
        "SELECT version_id, sql, dialect FROM catalog_view_representations WHERE view_id = $1 \
         ORDER BY version_id, id",
    )
    .bind(view_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut reps_by_version: HashMap<ViewVersionId, Vec<ViewRepresentation>> = HashMap::new();
    for row in rep_rows {
        let version_id: i32 = row.try_get("version_id").map_err(CatalogError::Db)?;
        let sql: String = row.try_get("sql").map_err(CatalogError::Db)?;
        let dialect: String = row.try_get("dialect").map_err(CatalogError::Db)?;
        reps_by_version
            .entry(version_id)
            .or_default()
            .push(ViewRepresentation::Sql(SqlViewRepresentation {
                sql,
                dialect,
            }));
    }

    // --- versions ---
    let version_rows = sqlx::query(
        "SELECT version_id, schema_id, timestamp_ms, summary, default_catalog, default_namespace \
         FROM catalog_view_versions WHERE view_id = $1",
    )
    .bind(view_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut versions: HashMap<ViewVersionId, Arc<ViewVersion>> =
        HashMap::with_capacity(version_rows.len());
    for row in version_rows {
        let version_id: i32 = row.try_get("version_id").map_err(CatalogError::Db)?;
        let schema_id: i32 = row.try_get("schema_id").map_err(CatalogError::Db)?;
        let timestamp_ms: i64 = row.try_get("timestamp_ms").map_err(CatalogError::Db)?;
        let summary_json: serde_json::Value = row.try_get("summary").map_err(CatalogError::Db)?;
        let default_catalog: Option<String> =
            row.try_get("default_catalog").map_err(CatalogError::Db)?;
        let default_ns_json: Option<serde_json::Value> =
            row.try_get("default_namespace").map_err(CatalogError::Db)?;

        let summary: HashMap<String, String> = serde_json::from_value(summary_json)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        let default_namespace: NamespaceIdent = match default_ns_json {
            Some(v) => {
                serde_json::from_value(v).map_err(|e| CatalogError::BadRequest(e.to_string()))?
            }
            None => empty_namespace_ident(),
        };

        let reps = reps_by_version.remove(&version_id).unwrap_or_default();
        let representations = ViewRepresentations::builder()
            .add_all_representations(reps)
            .build()
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;

        let version = ViewVersion::builder()
            .with_version_id(version_id)
            .with_schema_id(schema_id)
            .with_timestamp_ms(timestamp_ms)
            .with_summary(summary)
            .with_representations(representations)
            .with_default_catalog(default_catalog)
            .with_default_namespace(default_namespace)
            .build();
        versions.insert(version_id, Arc::new(version));
    }

    // --- version log (ordered by insert sequence) ---
    let log_rows = sqlx::query(
        "SELECT version_id, timestamp_ms FROM catalog_view_version_log \
         WHERE view_id = $1 ORDER BY seq ASC",
    )
    .bind(view_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut version_log: Vec<ViewVersionLog> = Vec::with_capacity(log_rows.len());
    for row in log_rows {
        let version_id: i32 = row.try_get("version_id").map_err(CatalogError::Db)?;
        let timestamp_ms: i64 = row.try_get("timestamp_ms").map_err(CatalogError::Db)?;
        version_log.push(ViewVersionLog::new(version_id, timestamp_ms));
    }

    // --- properties ---
    let prop_rows =
        sqlx::query("SELECT key, value FROM catalog_view_properties WHERE view_id = $1")
            .bind(view_id)
            .fetch_all(pool)
            .await
            .map_err(CatalogError::Db)?;
    let mut properties: HashMap<String, String> = HashMap::with_capacity(prop_rows.len());
    for row in prop_rows {
        let key: String = row.try_get("key").map_err(CatalogError::Db)?;
        let value: String = row.try_get("value").map_err(CatalogError::Db)?;
        properties.insert(key, value);
    }

    ViewMetadata::try_from_parts(ViewMetadataParts {
        format_version: format_version_from_i16(root.format_version)?,
        view_uuid: root.view_uuid,
        location: root.location.clone(),
        current_version_id: root.current_version_id,
        versions,
        version_log,
        schemas,
        properties,
    })
    .map_err(|e| CatalogError::BadRequest(e.to_string()))
}

/// `NamespaceIdent` rejects empty vecs; some engines send an empty
/// default-namespace. Reconstruct an empty ident via serde, mirroring
/// lakekeeper's `EMPTY_NAMESPACE_IDENT`.
pub(crate) fn empty_namespace_ident() -> NamespaceIdent {
    serde_json::from_value(serde_json::Value::Array(vec![]))
        .expect("empty namespace ident is constructible")
}
