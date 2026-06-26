//! Normalized Iceberg table store: shred-on-write / assemble-on-read.
//!
//! The single-JSONB-blob store has been replaced with a normalized schema
//! (migration 0023). On write we *shred* a [`TableMetadata`] into its child
//! rows (schemas, partition specs, sort orders, snapshots, refs, properties,
//! logs); on read we *assemble* those rows back into a [`TableMetadata`].
//!
//! The shape is ported from lakekeeper's `into_table_metadata`
//! (`lakekeeper-storage-postgres`), which targets the same iceberg-rust fork.
//! egret is Iceberg V2-only, so the V3 row-lineage / encryption-key fields are
//! intentionally skipped (`next_row_id` is reconstructed as `INITIAL_ROW_ID`).

use std::collections::HashMap;
use std::sync::Arc;

use iceberg::spec::{
    FormatVersion, MetadataLog, PartitionSpec, Schema, Snapshot, SnapshotLog, SnapshotReference,
    SnapshotRetention, SortOrder, TableMetadata,
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::CatalogError;

/// The `catalog_tables` root row, holding the scalar columns needed to
/// reconstruct a [`TableMetadata`] together with the child rows.
#[derive(sqlx::FromRow)]
pub struct TableRootRow {
    pub id: Uuid,
    pub name: String,
    pub table_uuid: Uuid,
    pub location: String,
    pub format_version: i16,
    pub last_column_id: i32,
    pub last_sequence_number: i64,
    pub last_updated_ms: i64,
    pub last_partition_id: i32,
    pub current_schema_id: i32,
    pub default_spec_id: i32,
    pub default_sort_order_id: i32,
    pub current_snapshot_id: Option<i64>,
    pub metadata_location: String,
    pub current_version: i32,
}

/// Map a stored `format_version` smallint (1 or 2) to the iceberg enum.
fn format_version_from_i16(v: i16) -> FormatVersion {
    match v {
        1 => FormatVersion::V1,
        _ => FormatVersion::V2,
    }
}

/// Map the iceberg format version to the stored smallint.
pub fn format_version_to_i16(v: FormatVersion) -> i16 {
    match v {
        FormatVersion::V1 => 1,
        _ => 2,
    }
}

/// Insert all child rows for `table_id` from `md`.
///
/// Append-only sets (schemas, partition specs, sort orders, snapshots) use
/// `ON CONFLICT DO NOTHING` keyed by their id. Mutable sets (properties, refs)
/// are re-synced: delete the table's rows then re-insert.
pub async fn shred_children(
    tx: &mut Transaction<'_, Postgres>,
    table_id: Uuid,
    md: &TableMetadata,
) -> Result<(), CatalogError> {
    // --- schemas (append-only by schema_id) ---
    for schema in md.schemas_iter() {
        let schema_json =
            serde_json::to_value(schema).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog_table_schemas (table_id, schema_id, schema) \
             VALUES ($1, $2, $3) ON CONFLICT (table_id, schema_id) DO NOTHING",
        )
        .bind(table_id)
        .bind(schema.schema_id())
        .bind(schema_json)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- partition specs (append-only by spec_id) ---
    for spec in md.partition_specs_iter() {
        let spec_json =
            serde_json::to_value(spec).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog_table_partition_specs (table_id, spec_id, spec) \
             VALUES ($1, $2, $3) ON CONFLICT (table_id, spec_id) DO NOTHING",
        )
        .bind(table_id)
        .bind(spec.spec_id())
        .bind(spec_json)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- sort orders (append-only by sort_order_id) ---
    for sort_order in md.sort_orders_iter() {
        let so_json = serde_json::to_value(sort_order)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        // The DB column is `int`; iceberg `order_id` is i64 but Iceberg V2
        // sort-order ids fit in i32.
        let so_id = i32::try_from(sort_order.order_id)
            .map_err(|e| CatalogError::BadRequest(format!("sort_order_id out of range: {e}")))?;
        sqlx::query(
            "INSERT INTO catalog_table_sort_orders (table_id, sort_order_id, sort_order) \
             VALUES ($1, $2, $3) ON CONFLICT (table_id, sort_order_id) DO NOTHING",
        )
        .bind(table_id)
        .bind(so_id)
        .bind(so_json)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- snapshots (append-only by snapshot_id) ---
    // Must precede refs: catalog_table_refs FKs into catalog_table_snapshots.
    for snap in md.snapshots() {
        let summary_json = serde_json::to_value(snap.summary())
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog_table_snapshots \
               (table_id, snapshot_id, parent_snapshot_id, sequence_number, manifest_list, \
                summary, schema_id, timestamp_ms) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (table_id, snapshot_id) DO NOTHING",
        )
        .bind(table_id)
        .bind(snap.snapshot_id())
        .bind(snap.parent_snapshot_id())
        .bind(snap.sequence_number())
        .bind(snap.manifest_list())
        .bind(summary_json)
        .bind(snap.schema_id())
        .bind(snap.timestamp_ms())
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- properties (mutable: re-sync) ---
    sqlx::query("DELETE FROM catalog_table_properties WHERE table_id = $1")
        .bind(table_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    for (key, value) in md.properties() {
        sqlx::query(
            "INSERT INTO catalog_table_properties (table_id, key, value) VALUES ($1, $2, $3)",
        )
        .bind(table_id)
        .bind(key)
        .bind(value)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    // --- refs (mutable: re-sync, after snapshots) ---
    sqlx::query("DELETE FROM catalog_table_refs WHERE table_id = $1")
        .bind(table_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    for (ref_name, snapshot_ref) in md.refs() {
        let ref_type = match snapshot_ref.retention {
            SnapshotRetention::Branch { .. } => "branch",
            SnapshotRetention::Tag { .. } => "tag",
        };
        let retention_json = serde_json::to_value(&snapshot_ref.retention)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog_table_refs \
               (table_id, ref_name, snapshot_id, type, retention) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(table_id)
        .bind(ref_name)
        .bind(snapshot_ref.snapshot_id)
        .bind(ref_type)
        .bind(retention_json)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    Ok(())
}

/// Append the newly-written metadata-file pointer to `catalog_table_metadata_log`
/// and re-sync `catalog_table_snapshot_log` from `md.history()`.
pub async fn append_logs(
    tx: &mut Transaction<'_, Postgres>,
    table_id: Uuid,
    md: &TableMetadata,
    metadata_file: &str,
) -> Result<(), CatalogError> {
    // metadata log: append a pointer to the file we just wrote.
    sqlx::query(
        "INSERT INTO catalog_table_metadata_log (table_id, timestamp_ms, metadata_file) \
         VALUES ($1, $2, $3)",
    )
    .bind(table_id)
    .bind(md.last_updated_ms())
    .bind(metadata_file)
    .execute(&mut **tx)
    .await
    .map_err(CatalogError::Db)?;

    // snapshot log: re-sync from history (delete then re-insert in order).
    sqlx::query("DELETE FROM catalog_table_snapshot_log WHERE table_id = $1")
        .bind(table_id)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    for entry in md.history() {
        sqlx::query(
            "INSERT INTO catalog_table_snapshot_log (table_id, snapshot_id, timestamp_ms) \
             VALUES ($1, $2, $3)",
        )
        .bind(table_id)
        .bind(entry.snapshot_id)
        .bind(entry.timestamp_ms)
        .execute(&mut **tx)
        .await
        .map_err(CatalogError::Db)?;
    }

    Ok(())
}

/// Fetch all child rows for `root.id` and rebuild a [`TableMetadata`].
///
/// Ports lakekeeper's `into_table_metadata`, finishing with the typed-builder
/// `build_unchecked()` + `try_normalize()`. V3-only fields are skipped.
pub async fn assemble_table_metadata(
    pool: &sqlx::PgPool,
    root: &TableRootRow,
) -> Result<TableMetadata, CatalogError> {
    let table_id = root.id;

    // --- schemas ---
    let schema_rows =
        sqlx::query("SELECT schema_id, schema FROM catalog_table_schemas WHERE table_id = $1")
            .bind(table_id)
            .fetch_all(pool)
            .await
            .map_err(CatalogError::Db)?;
    let mut schemas: HashMap<i32, Arc<Schema>> = HashMap::with_capacity(schema_rows.len());
    for row in schema_rows {
        let json: serde_json::Value = row.try_get("schema").map_err(CatalogError::Db)?;
        let schema: Schema =
            serde_json::from_value(json).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        schemas.insert(schema.schema_id(), Arc::new(schema));
    }

    // --- partition specs ---
    let spec_rows =
        sqlx::query("SELECT spec_id, spec FROM catalog_table_partition_specs WHERE table_id = $1")
            .bind(table_id)
            .fetch_all(pool)
            .await
            .map_err(CatalogError::Db)?;
    let mut partition_specs: HashMap<i32, Arc<PartitionSpec>> =
        HashMap::with_capacity(spec_rows.len());
    for row in spec_rows {
        let spec_id: i32 = row.try_get("spec_id").map_err(CatalogError::Db)?;
        let json: serde_json::Value = row.try_get("spec").map_err(CatalogError::Db)?;
        let spec: PartitionSpec =
            serde_json::from_value(json).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        partition_specs.insert(spec_id, Arc::new(spec));
    }
    let default_spec = partition_specs
        .get(&root.default_spec_id)
        .cloned()
        .ok_or_else(|| {
            CatalogError::BadRequest(format!(
                "default partition spec {} missing from loaded specs",
                root.default_spec_id
            ))
        })?;

    // --- sort orders ---
    let so_rows = sqlx::query(
        "SELECT sort_order_id, sort_order FROM catalog_table_sort_orders WHERE table_id = $1",
    )
    .bind(table_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut sort_orders: HashMap<i64, Arc<SortOrder>> = HashMap::with_capacity(so_rows.len());
    for row in so_rows {
        let sort_order_id: i32 = row.try_get("sort_order_id").map_err(CatalogError::Db)?;
        let json: serde_json::Value = row.try_get("sort_order").map_err(CatalogError::Db)?;
        let sort_order: SortOrder =
            serde_json::from_value(json).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        sort_orders.insert(i64::from(sort_order_id), Arc::new(sort_order));
    }

    // --- properties ---
    let prop_rows =
        sqlx::query("SELECT key, value FROM catalog_table_properties WHERE table_id = $1")
            .bind(table_id)
            .fetch_all(pool)
            .await
            .map_err(CatalogError::Db)?;
    let mut properties: HashMap<String, String> = HashMap::with_capacity(prop_rows.len());
    for row in prop_rows {
        let key: String = row.try_get("key").map_err(CatalogError::Db)?;
        let value: String = row.try_get("value").map_err(CatalogError::Db)?;
        properties.insert(key, value);
    }

    // --- snapshots ---
    let snap_rows = sqlx::query(
        "SELECT snapshot_id, parent_snapshot_id, sequence_number, manifest_list, summary, \
                schema_id, timestamp_ms \
         FROM catalog_table_snapshots WHERE table_id = $1",
    )
    .bind(table_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut snapshots: HashMap<i64, Arc<Snapshot>> = HashMap::with_capacity(snap_rows.len());
    for row in snap_rows {
        let snapshot_id: i64 = row.try_get("snapshot_id").map_err(CatalogError::Db)?;
        let parent_snapshot_id: Option<i64> = row
            .try_get("parent_snapshot_id")
            .map_err(CatalogError::Db)?;
        let sequence_number: i64 = row.try_get("sequence_number").map_err(CatalogError::Db)?;
        let manifest_list: String = row.try_get("manifest_list").map_err(CatalogError::Db)?;
        let summary_json: serde_json::Value = row.try_get("summary").map_err(CatalogError::Db)?;
        let schema_id: Option<i32> = row.try_get("schema_id").map_err(CatalogError::Db)?;
        let timestamp_ms: i64 = row.try_get("timestamp_ms").map_err(CatalogError::Db)?;
        let summary = serde_json::from_value(summary_json)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;

        let builder = Snapshot::builder()
            .with_snapshot_id(snapshot_id)
            .with_parent_snapshot_id(parent_snapshot_id)
            .with_sequence_number(sequence_number)
            .with_manifest_list(manifest_list)
            .with_summary(summary)
            .with_timestamp_ms(timestamp_ms);
        let snapshot = match schema_id {
            Some(sid) => builder.with_schema_id(sid).build(),
            None => builder.build(),
        };
        snapshots.insert(snapshot_id, Arc::new(snapshot));
    }

    // --- snapshot log (ordered by insert sequence) ---
    let snap_log_rows = sqlx::query(
        "SELECT snapshot_id, timestamp_ms FROM catalog_table_snapshot_log \
         WHERE table_id = $1 ORDER BY seq ASC",
    )
    .bind(table_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut snapshot_log: Vec<SnapshotLog> = Vec::with_capacity(snap_log_rows.len());
    for row in snap_log_rows {
        snapshot_log.push(SnapshotLog {
            snapshot_id: row.try_get("snapshot_id").map_err(CatalogError::Db)?,
            timestamp_ms: row.try_get("timestamp_ms").map_err(CatalogError::Db)?,
        });
    }

    // --- metadata log (ordered by insert sequence) ---
    let meta_log_rows = sqlx::query(
        "SELECT metadata_file, timestamp_ms FROM catalog_table_metadata_log \
         WHERE table_id = $1 ORDER BY seq ASC",
    )
    .bind(table_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut metadata_log: Vec<MetadataLog> = Vec::with_capacity(meta_log_rows.len());
    for row in meta_log_rows {
        metadata_log.push(MetadataLog {
            metadata_file: row.try_get("metadata_file").map_err(CatalogError::Db)?,
            timestamp_ms: row.try_get("timestamp_ms").map_err(CatalogError::Db)?,
        });
    }

    // --- refs ---
    let ref_rows = sqlx::query(
        "SELECT ref_name, snapshot_id, retention FROM catalog_table_refs WHERE table_id = $1",
    )
    .bind(table_id)
    .fetch_all(pool)
    .await
    .map_err(CatalogError::Db)?;
    let mut refs: HashMap<String, SnapshotReference> = HashMap::with_capacity(ref_rows.len());
    for row in ref_rows {
        let ref_name: String = row.try_get("ref_name").map_err(CatalogError::Db)?;
        let snapshot_id: i64 = row.try_get("snapshot_id").map_err(CatalogError::Db)?;
        let retention_json: serde_json::Value =
            row.try_get("retention").map_err(CatalogError::Db)?;
        let retention: SnapshotRetention = serde_json::from_value(retention_json)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        refs.insert(
            ref_name,
            SnapshotReference {
                snapshot_id,
                retention,
            },
        );
    }

    // current_snapshot_id is authoritative on the root row (main branch).
    let current_snapshot_id = root.current_snapshot_id;

    // default_partition_type is derived from the default spec + current schema.
    let current_schema = schemas.get(&root.current_schema_id).ok_or_else(|| {
        CatalogError::BadRequest(format!(
            "current schema {} missing from loaded schemas",
            root.current_schema_id
        ))
    })?;
    let default_partition_type = default_spec
        .partition_type(current_schema)
        .map_err(|e| CatalogError::BadRequest(e.to_string()))?;

    let mut table_metadata = TableMetadata::builder()
        .format_version(format_version_from_i16(root.format_version))
        .table_uuid(root.table_uuid)
        .location(root.location.clone())
        .last_sequence_number(root.last_sequence_number)
        .last_updated_ms(root.last_updated_ms)
        .last_column_id(root.last_column_id)
        .schemas(schemas)
        .current_schema_id(root.current_schema_id)
        .partition_specs(partition_specs)
        .default_spec(default_spec)
        .default_partition_type(default_partition_type)
        .last_partition_id(root.last_partition_id)
        .properties(properties)
        .current_snapshot_id(current_snapshot_id)
        .snapshots(snapshots)
        .snapshot_log(snapshot_log)
        .metadata_log(metadata_log)
        .sort_orders(sort_orders)
        .default_sort_order_id(i64::from(root.default_sort_order_id))
        .refs(refs)
        .statistics(HashMap::new())
        .partition_statistics(HashMap::new())
        .encryption_keys(HashMap::new())
        .next_row_id(iceberg::spec::INITIAL_ROW_ID)
        .build_unchecked();

    table_metadata
        .try_normalize()
        .map_err(|e| CatalogError::BadRequest(e.to_string()))?;

    Ok(table_metadata)
}
