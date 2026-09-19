//! Boot-time backfill of the catalog registry (spec §4.8). Runs on every boot,
//! under an advisory lock, and only ever adds: a warehouse without a catalog gets
//! one; an `engine_mgmt.engine_catalogs` row without a
//! `registry.legacy_engine_catalogs` entry is migrated once. Anything an admin
//! removes later is therefore never recreated.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use serde_json::json;
use uuid::Uuid;

use crate::legacy::{classify, plaintext_secrets, ClassifyOutcome, LegacyCatalog};
use crate::name::{first_free_name, join_truncated, normalize_catalog_name};
use crate::report::ReportEntry;
use crate::store::{NewAttachment, RegistryStore, WarehouseIdentity};
use crate::RegistryError;

/// `pg_advisory_lock` key ("SP3A") serialising replicas booting together.
pub const BACKFILL_LOCK_KEY: i64 = 0x5350_3341;
const EGRET: &str = "egret";
const EXTRACTED: &str =
    "Moved out of an engine catalog's configuration by the SP3 registry migration";

#[derive(Debug)]
pub struct BackfillSummary {
    pub run_id: Uuid,
    pub report_rows: usize,
    pub failed_rows: usize,
}

#[derive(sqlx::FromRow)]
struct WarehouseRow {
    id: Uuid,
    name: String,
    workspace_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct LegacyRow {
    id: Uuid,
    engine_id: Uuid,
    engine_name: String,
    engine_warehouse_id: Uuid,
    name: String,
    kind: String,
    config: serde_json::Value,
    secret_ref: Option<Uuid>,
    is_primary: bool,
    created_at: DateTime<Utc>,
}

pub async fn run_backfill(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
) -> Result<BackfillSummary, RegistryError> {
    let mut lock = store.pool().acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(BACKFILL_LOCK_KEY)
        .execute(&mut *lock)
        .await?;
    let result = run_locked(store, secrets).await;
    if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(BACKFILL_LOCK_KEY)
        .execute(&mut *lock)
        .await
    {
        tracing::warn!(error = %e, "catalog registry backfill: advisory unlock failed");
    }
    result
}

async fn run_locked(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
) -> Result<BackfillSummary, RegistryError> {
    let run_id = Uuid::new_v4();
    let mut report: Vec<ReportEntry> = Vec::new();
    let mut failed_rows = 0usize;

    // 1. Warehouses.
    let primaries: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.warehouse_id FROM engine_mgmt.engines e \
           JOIN engine_mgmt.engine_catalogs c ON c.engine_id = e.id \
          WHERE c.is_primary AND c.kind = 'egret-warehouse'",
    )
    .fetch_all(store.pool())
    .await?;
    let egret_warehouse = match primaries.as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    let warehouses: Vec<WarehouseRow> = sqlx::query_as(
        "SELECT w.id, w.name, w.workspace_id FROM catalog.warehouses w \
          WHERE NOT EXISTS (SELECT 1 FROM registry.catalogs c WHERE c.warehouse_id = w.id) \
          ORDER BY w.created_at, w.id",
    )
    .fetch_all(store.pool())
    .await?;
    for wh in &warehouses {
        let preferred = if Some(wh.id) == egret_warehouse
            && store.get_catalog_by_name(EGRET).await?.is_none()
        {
            Some(EGRET)
        } else {
            None
        };
        let ident = WarehouseIdentity {
            id: wh.id,
            name: &wh.name,
            workspace_id: wh.workspace_id,
        };
        let (cat, created) = store.ensure_warehouse_catalog(&ident, preferred).await?;
        if !created {
            continue;
        }
        report.push(
            ReportEntry::info(
                run_id,
                "warehouse_catalog_created",
                format!("warehouse {}", wh.name),
                format!(
                    "warehouse {} is catalog {} and is assigned to its workspace",
                    wh.name, cat.name
                ),
            )
            .catalog(cat.id)
            .warehouse(wh.id)
            .detail(json!({ "warehouseName": wh.name, "catalogName": cat.name })),
        );
        if preferred.is_some() {
            report.push(
                ReportEntry::info(
                    run_id,
                    "primary_named_egret",
                    format!("warehouse {}", wh.name),
                    format!(
                        "warehouse {} is the only engine primary, so its catalog keeps the name egret that saved SQL uses",
                        wh.name
                    ),
                )
                .catalog(cat.id)
                .warehouse(wh.id),
            );
        }
    }

    // 2. Legacy engine catalogs.
    let rows: Vec<LegacyRow> = sqlx::query_as(
        "SELECT c.id, c.engine_id, e.name AS engine_name, e.warehouse_id AS engine_warehouse_id, c.name, c.kind, \
                c.config, c.secret_ref, c.is_primary, c.created_at \
           FROM engine_mgmt.engine_catalogs c JOIN engine_mgmt.engines e ON e.id = c.engine_id \
          WHERE NOT EXISTS (SELECT 1 FROM registry.legacy_engine_catalogs m WHERE m.engine_catalog_id = c.id) \
          ORDER BY c.is_primary DESC, c.created_at, c.id",
    )
    .fetch_all(store.pool())
    .await?;
    let mut shared: HashMap<String, (String, Uuid)> = HashMap::new();
    let mut renames: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in &rows {
        let result = if row.kind == "egret-warehouse" {
            migrate_warehouse_row(store, secrets, run_id, row, &mut report).await
        } else {
            migrate_connector_row(store, secrets, run_id, row, &mut shared, &mut report).await
        };
        match result {
            Ok((final_name, catalog_id)) => {
                if final_name != row.name {
                    report.push(
                        ReportEntry::warning(
                            run_id,
                            "engine_catalog_renamed",
                            format!("engine {} catalog {}", row.engine_name, row.name),
                            format!(
                                "engine {} now reaches catalog {} as {}; queries that name {} fail from its next apply",
                                row.engine_name, row.name, final_name, row.name
                            ),
                        )
                        .engine(row.engine_id)
                        .catalog(catalog_id)
                        .detail(json!({ "oldName": row.name, "newName": final_name, "primary": row.is_primary })),
                    );
                    renames
                        .entry(row.name.clone())
                        .or_default()
                        .insert(final_name);
                }
            }
            Err(e) => {
                failed_rows += 1;
                report.push(
                    ReportEntry::warning(
                        run_id,
                        "backfill_failed",
                        format!("engine {} catalog {}", row.engine_name, row.name),
                        format!(
                            "not migrated: {e}. Engine {} refuses applies until a later boot migrates it",
                            row.engine_name
                        ),
                    )
                    .engine(row.engine_id)
                    .detail(json!({ "engineCatalogId": row.id })),
                );
            }
        }
    }

    // 3. Saved SQL that still names a catalog by its old name.
    for (old, new_names) in &renames {
        for hit in crate::scan::scan_saved_sql(store.pool(), old).await? {
            report.push(
                ReportEntry::warning(
                    run_id,
                    "saved_sql_references_catalog",
                    format!("{} {}", hit.object_type, hit.name),
                    format!(
                        "{} {} names catalog {old}, which is now {}",
                        hit.object_type,
                        hit.name,
                        new_names.iter().cloned().collect::<Vec<_>>().join(" / ")
                    ),
                )
                .detail(json!({
                    "objectType": hit.object_type, "objectId": hit.object_id,
                    "workspaceId": hit.workspace_id, "oldName": old, "newNames": new_names,
                })),
            );
        }
    }

    // 4. Report.
    for e in &report {
        match e.severity {
            crate::Severity::Info => {
                tracing::info!(code = e.code, subject = %e.subject, "catalog registry backfill: {}", e.message)
            }
            crate::Severity::Warning => {
                tracing::warn!(code = e.code, subject = %e.subject, "catalog registry backfill: {}", e.message)
            }
        }
    }
    store.insert_report(&report).await?;
    Ok(BackfillSummary {
        run_id,
        report_rows: report.len(),
        failed_rows,
    })
}

async fn create_or_reuse_secret(
    secrets: &WorkspaceSecretStore,
    name: &str,
    value: &str,
) -> Result<Uuid, RegistryError> {
    match secrets.create(None, name, Some(EXTRACTED), value).await {
        Ok(id) => Ok(id),
        Err(SecretsError::AlreadyExists(_)) => secrets
            .list(None)
            .await?
            .into_iter()
            .find(|m| m.name == name)
            .map(|m| m.id)
            .ok_or_else(|| {
                RegistryError::Conflict(format!("secret {name} exists but cannot be listed"))
            }),
        Err(e) => Err(e.into()),
    }
}

fn client_row_id(config: &serde_json::Value) -> Option<Uuid> {
    config
        .get("clientRowId")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

async fn row_warehouse(store: &RegistryStore, row: &LegacyRow) -> Result<Uuid, RegistryError> {
    if row.is_primary {
        return Ok(row.engine_warehouse_id);
    }
    if let Some(cid) = client_row_id(&row.config) {
        let wh: Option<Uuid> =
            sqlx::query_scalar("SELECT warehouse_id FROM catalog.catalog_clients WHERE id = $1")
                .bind(cid)
                .fetch_optional(store.pool())
                .await?;
        if let Some(wh) = wh {
            return Ok(wh);
        }
    }
    let name = row
        .config
        .get("warehouse")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RegistryError::Conflict("the row names no warehouse".into()))?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM catalog.warehouses WHERE name = $1")
        .bind(name)
        .fetch_all(store.pool())
        .await?;
    match ids.as_slice() {
        [only] => Ok(*only),
        [] => Err(RegistryError::NotFound(format!("warehouse {name}"))),
        _ => Err(RegistryError::Conflict(format!(
            "{} warehouses are named {name}",
            ids.len()
        ))),
    }
}

/// Returns (the name the engine reaches the catalog by now, catalog id).
async fn migrate_warehouse_row(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    row: &LegacyRow,
    report: &mut Vec<ReportEntry>,
) -> Result<(String, Uuid), RegistryError> {
    let warehouse_id = row_warehouse(store, row).await?;
    let wh: WarehouseRow =
        sqlx::query_as("SELECT id, name, workspace_id FROM catalog.warehouses WHERE id = $1")
            .bind(warehouse_id)
            .fetch_optional(store.pool())
            .await?
            .ok_or_else(|| RegistryError::NotFound(format!("warehouse {warehouse_id}")))?;
    let (cat, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh.id,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            None,
        )
        .await?;
    let client = client_row_id(&row.config);
    if store.get_attachment(row.engine_id, cat.id).await?.is_some() {
        store
            .map_legacy_row(row.id, row.engine_id, Some(cat.id))
            .await?;
        report.push(
            ReportEntry::warning(
                run_id,
                "duplicate_attachment_skipped",
                format!("engine {} catalog {}", row.engine_name, row.name),
                format!(
                    "engine {} already reaches warehouse {} as catalog {}; its extra catalog {} is dropped at the engine's next apply. \
                     Its Catalog Client was left in place: revoke it on the warehouse's Clients tab afterwards",
                    row.engine_name, wh.name, cat.name, row.name
                ),
            )
            .engine(row.engine_id)
            .catalog(cat.id)
            .warehouse(wh.id)
            .detail(json!({ "catalogClientId": client })),
        );
        return Ok((cat.name, cat.id));
    }
    let credential = row
        .config
        .get("credential")
        .and_then(|v| v.as_str())
        .filter(|s| s.contains(':'))
        .ok_or_else(|| {
            RegistryError::Conflict("the row carries no catalog client credential".into())
        })?;
    let secret_name = match client {
        Some(id) => format!("catalog-client-{id}"),
        None => format!("catalog-client-legacy-{}", row.id),
    };
    let secret_id = create_or_reuse_secret(secrets, &secret_name, credential).await?;
    store
        .insert_attachment_and_map(
            &NewAttachment {
                engine_id: row.engine_id,
                catalog_id: cat.id,
                catalog_client_id: client,
                credential_secret_id: Some(secret_id),
                minted_client: row.config.get("mintedClient").and_then(|v| v.as_bool())
                    != Some(false),
                applied_now: false,
                created_at: Some(row.created_at),
            },
            row.id,
        )
        .await?;
    report.push(
        ReportEntry::info(
            run_id,
            "secret_extracted",
            format!("engine {} catalog {}", row.engine_name, row.name),
            format!("the catalog client credential moved into secret {secret_name}"),
        )
        .engine(row.engine_id)
        .catalog(cat.id),
    );
    Ok((cat.name, cat.id))
}

async fn migrate_connector_row(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    row: &LegacyRow,
    shared: &mut HashMap<String, (String, Uuid)>,
    report: &mut Vec<ReportEntry>,
) -> Result<(String, Uuid), RegistryError> {
    let legacy = LegacyCatalog {
        name: &row.name,
        kind: &row.kind,
        config: &row.config,
        secret_ref: row.secret_ref,
    };
    let key =
        json!({ "kind": row.kind, "config": row.config, "secretRef": row.secret_ref }).to_string();
    let base = normalize_catalog_name(&row.name);
    let subject = format!("engine {} catalog {}", row.engine_name, row.name);

    if let Some((seen_key, catalog_id)) = shared.get(&base) {
        if *seen_key == key {
            let catalog_id = *catalog_id;
            if store
                .get_attachment(row.engine_id, catalog_id)
                .await?
                .is_none()
            {
                store
                    .insert_attachment_and_map(
                        &NewAttachment {
                            engine_id: row.engine_id,
                            catalog_id,
                            catalog_client_id: None,
                            credential_secret_id: None,
                            minted_client: false,
                            applied_now: false,
                            created_at: Some(row.created_at),
                        },
                        row.id,
                    )
                    .await?;
            } else {
                store
                    .map_legacy_row(row.id, row.engine_id, Some(catalog_id))
                    .await?;
            }
            report.push(
                ReportEntry::info(
                    run_id,
                    "engine_catalog_shared",
                    subject,
                    format!("identical to catalog {base} on another engine; both engines now attach one catalog"),
                )
                .engine(row.engine_id)
                .catalog(catalog_id),
            );
            return Ok((base, catalog_id));
        }
    }

    let name = if store.get_catalog_by_name(&base).await?.is_none() {
        base.clone()
    } else {
        let with_engine = join_truncated(&base, &normalize_catalog_name(&row.engine_name));
        let taken = store.names_starting_with(&with_engine).await?;
        let name = first_free_name(&with_engine, |n| taken.contains(n));
        report.push(
            ReportEntry::warning(
                run_id,
                "engine_catalog_suffixed",
                subject.clone(),
                format!("another catalog is already called {base} with a different configuration; this one is {name}"),
            )
            .engine(row.engine_id),
        );
        name
    };

    let mut extracted = BTreeMap::new();
    for (property, value) in plaintext_secrets(&legacy) {
        let secret_name = format!("catalog-{name}-{property}");
        let id = create_or_reuse_secret(secrets, &secret_name, &value).await?;
        extracted.insert(property.clone(), id);
        report.push(
            ReportEntry::info(
                run_id,
                "secret_extracted",
                subject.clone(),
                format!("plaintext {property} moved into secret {secret_name}"),
            )
            .engine(row.engine_id),
        );
    }
    let classified = classify(&legacy, &extracted)?;
    let cat = store
        .create_catalog_for_legacy_row(
            &name,
            &classified.def,
            row.engine_id,
            row.id,
            row.created_at,
        )
        .await?;
    report.push(
        match &classified.outcome {
            ClassifyOutcome::Typed => ReportEntry::info(
                run_id,
                "engine_catalog_typed",
                subject,
                format!(
                    "catalog {name} is a typed {} catalog",
                    classified.def.kind().as_str()
                ),
            ),
            ClassifyOutcome::Generic { reason } => ReportEntry::info(
                run_id,
                "engine_catalog_generic",
                subject,
                format!("catalog {name} keeps its properties as a generic catalog: {reason}"),
            )
            .detail(json!({ "reason": reason })),
        }
        .engine(row.engine_id)
        .catalog(cat.id),
    );
    shared.insert(base, (key, cat.id));
    Ok((name, cat.id))
}
