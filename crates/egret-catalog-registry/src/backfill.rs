//! Boot-time backfill of the catalog registry (spec §4.8). Runs on every boot,
//! under an advisory lock, and only ever adds: a warehouse without a catalog gets
//! one; an `engine_mgmt.engine_catalogs` row without a
//! `registry.legacy_engine_catalogs` entry is migrated once. Anything an admin
//! removes later is therefore never recreated.
//!
//! Every mutation commits together with the report rows it explains, in one
//! transaction (review fix I1): a crash mid-run can only ever lose an UNSTARTED
//! unit of work (a warehouse, a legacy row, a saved-SQL scan), never a completed
//! one's report. A warehouse or legacy row that fails is reported and skipped —
//! it blocks only itself, never the rest of the run (review fix I2). A secret
//! name collision with a DIFFERENT value never silently reuses the wrong secret;
//! it is placed under a suffixed name instead and reported (review fix I3).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use serde_json::json;
use uuid::Uuid;

use crate::legacy::{classify, plaintext_secrets, ClassifyOutcome, LegacyCatalog};
use crate::name::{first_free_name, join_truncated, normalize_catalog_name};
use crate::report::ReportEntry;
use crate::store::{CatalogRow, NewAttachment, RegistryStore, WarehouseIdentity};
use crate::{RegistryError, Severity};

/// `pg_advisory_lock` key ("SP3A") serialising replicas booting together.
pub const BACKFILL_LOCK_KEY: i64 = 0x5350_3341;
const EGRET: &str = "egret";
const EXTRACTED: &str =
    "Moved out of an engine catalog's configuration by the SP3 registry migration";
/// Bound on the secret-name suffix search (`-2`, `-3`, …) `create_or_reuse_secret`
/// does on a value collision — generous for real collision counts, but finite so
/// a pathological run fails loudly instead of looping forever. Mirrors the
/// warehouse-name suffix search's own 32-attempt cap in
/// `RegistryStore::ensure_warehouse_catalog`.
const MAX_SUFFIX_ATTEMPTS: u32 = 32;

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
    // If the unlock itself fails, the connection may still hold the advisory
    // lock. Returning it to the pool would let a LATER acquire silently inherit
    // that held lock (or block forever waiting on itself); close it instead of
    // returning it, so the pool opens a fresh, lock-free replacement.
    match sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(BACKFILL_LOCK_KEY)
        .execute(&mut *lock)
        .await
    {
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                error = %e,
                "catalog registry backfill: advisory unlock failed; closing the connection \
                 instead of returning it to the pool holding the lock"
            );
            lock.close_on_drop();
        }
    }
    result
}

fn log_entries(entries: &[ReportEntry]) {
    for e in entries {
        match e.severity {
            Severity::Info => {
                tracing::info!(code = e.code, subject = %e.subject, "catalog registry backfill: {}", e.message)
            }
            Severity::Warning => {
                tracing::warn!(code = e.code, subject = %e.subject, "catalog registry backfill: {}", e.message)
            }
        }
    }
}

async fn run_locked(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
) -> Result<BackfillSummary, RegistryError> {
    let run_id = Uuid::new_v4();

    // 1. Warehouses. The engine primary (if there is exactly one) is processed
    // FIRST, so a warehouse merely NAMED "egret" (not the primary) can never grab
    // that name before the primary gets a chance to claim it (review fix, minor).
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
    let mut warehouses: Vec<WarehouseRow> = sqlx::query_as(
        "SELECT w.id, w.name, w.workspace_id FROM catalog.warehouses w \
          WHERE NOT EXISTS (SELECT 1 FROM registry.catalogs c WHERE c.warehouse_id = w.id) \
          ORDER BY w.created_at, w.id",
    )
    .fetch_all(store.pool())
    .await?;
    if let Some(primary_id) = egret_warehouse {
        if let Some(pos) = warehouses.iter().position(|w| w.id == primary_id) {
            let primary = warehouses.remove(pos);
            warehouses.insert(0, primary);
        }
    }

    for wh in &warehouses {
        let preferred = if Some(wh.id) == egret_warehouse
            && store.get_catalog_by_name(EGRET).await?.is_none()
        {
            Some(EGRET)
        } else {
            None
        };
        let had_preferred = preferred.is_some();
        let ident = WarehouseIdentity {
            id: wh.id,
            name: &wh.name,
            workspace_id: wh.workspace_id,
        };
        let result = store
            .ensure_warehouse_catalog(&ident, preferred, |cat: &CatalogRow| {
                let mut entries = vec![warehouse_created_entry(run_id, wh, cat)];
                if had_preferred {
                    entries.push(primary_named_egret_entry(run_id, wh, cat));
                }
                log_entries(&entries);
                entries
            })
            .await;
        if let Err(e) = result {
            report_immediately(
                store,
                ReportEntry::warning(
                    run_id,
                    "backfill_failed",
                    format!("warehouse {}", wh.name),
                    format!(
                        "not migrated: {e}. Its catalog, and any engine relying on it, are \
                         unavailable until a later boot migrates it"
                    ),
                )
                .warehouse(wh.id)
                .detail(json!({ "warehouseId": wh.id })),
            )
            .await?;
        }
    }

    // 2. Legacy engine catalogs. Each row's mutation and report commit together
    // (review fix I1); a row that fails is reported and skipped, blocking only
    // its own engine (review fix I2, already the design here — unchanged).
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
    // Warehouses whose Ranger tag export this run changed — see step 2b.
    let mut exported: BTreeSet<Uuid> = BTreeSet::new();
    for row in &rows {
        let result = if row.kind == "egret-warehouse" {
            migrate_warehouse_row(store, secrets, run_id, row, egret_warehouse)
                .await
                .map(|attached| {
                    if let Some(wh_id) = attached {
                        exported.insert(wh_id);
                    }
                })
        } else {
            migrate_connector_row(store, secrets, run_id, row, &mut shared).await
        };
        if let Err(e) = result {
            report_immediately(
                store,
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
            )
            .await?;
        }
    }

    // 2b. Make the new attachments REACHABLE by a plugin that is already
    // running (review F-1). Writing an attachment changes what
    // `PolicyStore::tag_assignments_for_service` returns — a warehouse an
    // engine held only as a SECONDARY `egret-warehouse` row now exports its
    // tags under the registry catalog name, where the legacy branch (which
    // filters `is_primary`) never emitted it. But nothing outside a tag
    // assign/unassign moves `tag_version`, and `download_tags` gates its 304 on
    // exactly that, so the plugin would keep 304ing until some unrelated tag
    // edit — in precisely the deployments the union was written for.
    //
    // This is not a transitional nicety: no production path writes
    // `engine_mgmt.engine_catalogs` any more, so the export's legacy branch is
    // frozen at whatever this backfill inherited and can only ever shrink. The
    // registry branch becoming reachable by an already-running plugin is the
    // mechanism now.
    //
    // Once, at the end of the migration, per warehouse — over-bumping is free
    // (the plugin re-downloads an identical payload), under-bumping is the
    // fail-open bug. A failed bump is logged, not fatal: the attachment it
    // describes is committed and correct, and the next tag edit or restart
    // still delivers it. Attach through the API needs no equivalent: its apply
    // rewrites `trino_catalogs_config_map`, which `config_checksum`
    // (`egret-provisioner/src/trino.rs`) hashes into the pod annotation, so the
    // coordinator rolls and the plugin re-downloads from scratch. The backfill
    // writes attachments with no apply and no pod roll, which is why it has to
    // say so itself.
    for wh_id in &exported {
        if let Err(e) = crate::tag_version::bump_bound_tag_versions(store.pool(), *wh_id).await {
            tracing::warn!(
                error = %e, warehouse_id = %wh_id,
                "catalog registry backfill: could not bump the bound tag versions; \
                 Ranger tag downloads for this warehouse keep 304ing until the next tag edit"
            );
        }
    }

    // 3. Saved SQL for every pending rename — this run's (already committed, so
    // visible here) and any earlier boot's whose scan never finished. A scan
    // failure is reported and skipped (review fix I2): it leaves `scanPending`
    // set, so it is retried whole on a later boot rather than treated as done.
    let mut by_old_name: BTreeMap<String, (Vec<i64>, BTreeSet<String>)> = BTreeMap::new();
    for (id, old, new) in store.pending_saved_sql_scans().await? {
        let entry = by_old_name.entry(old).or_default();
        entry.0.push(id);
        if let Some(new) = new {
            entry.1.insert(new);
        }
    }
    for (old, (rename_ids, new_names)) in &by_old_name {
        match crate::scan::scan_saved_sql(store.pool(), old).await {
            Ok(hits) => {
                let entries: Vec<ReportEntry> = hits
                    .into_iter()
                    .map(|hit| {
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
                        }))
                    })
                    .collect();
                log_entries(&entries);
                if let Err(e) = store.complete_saved_sql_scan(rename_ids, &entries).await {
                    tracing::warn!(
                        error = %e, old_name = %old,
                        "catalog registry backfill: failed to persist saved-SQL scan results; will retry next boot"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e, old_name = %old,
                    "catalog registry backfill: saved-SQL scan failed; will retry next boot"
                );
                let entry = ReportEntry::warning(
                    run_id,
                    "saved_sql_scan_failed",
                    format!("catalog {old}"),
                    format!("scanning saved SQL for {old} failed: {e}; will retry next boot"),
                )
                .detail(json!({ "oldName": old }));
                // Best-effort: this row is informational only, and the pending
                // marker (untouched) is what actually drives the retry.
                let _ = store.insert_report(&[entry]).await;
            }
        }
    }

    // The database is the source of truth for what this run actually wrote —
    // every entry above was already committed by the time we get here, so a
    // final count can never disagree with what a reader of the report sees.
    let report_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registry.migration_report WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(store.pool())
            .await?;
    let failed_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.migration_report WHERE run_id = $1 AND code = 'backfill_failed'",
    )
    .bind(run_id)
    .fetch_one(store.pool())
    .await?;
    Ok(BackfillSummary {
        run_id,
        report_rows: report_rows as usize,
        failed_rows: failed_rows as usize,
    })
}

/// Writes one report row immediately (no associated mutation to stay atomic
/// with — the operation it describes already failed and changed nothing).
async fn report_immediately(
    store: &RegistryStore,
    entry: ReportEntry,
) -> Result<(), RegistryError> {
    log_entries(std::slice::from_ref(&entry));
    store.insert_report(&[entry]).await
}

fn warehouse_created_entry(run_id: Uuid, wh: &WarehouseRow, cat: &CatalogRow) -> ReportEntry {
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
    .detail(json!({ "warehouseName": wh.name, "catalogName": cat.name }))
}

fn primary_named_egret_entry(run_id: Uuid, wh: &WarehouseRow, cat: &CatalogRow) -> ReportEntry {
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
    .warehouse(wh.id)
}

/// `None` when `row.name` still names the final catalog; otherwise a warning
/// entry with `scanPending: true` so step 3 (on THIS boot or, if interrupted, a
/// later one) looks up saved SQL for the old name.
fn renamed_entry(
    run_id: Uuid,
    row: &LegacyRow,
    final_name: &str,
    catalog_id: Uuid,
) -> Option<ReportEntry> {
    if final_name == row.name {
        return None;
    }
    Some(
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
        .detail(json!({
            "oldName": row.name, "newName": final_name, "primary": row.is_primary,
            "scanPending": true,
        })),
    )
}

/// Extract-once, collision-safe secret creation (review fix I3). Reuses an
/// existing secret only when its VALUE matches `value`; a same-named secret
/// holding something else — an unrelated secret an admin already created under
/// that name, or two legacy rows whose names happen to compute the same target —
/// is never silently bound to the wrong credential. Instead the credential is
/// stored under a numeric-suffixed name, and the caller is handed a report entry
/// to bundle into its own atomic write (never written here, so it stays
/// consistent with review fix I1: it must land in the same transaction as the
/// row it belongs to).
///
/// Returns `(secret id, name actually used, collision report entry)`. The
/// collision entry is `Some` whenever `used name != base_name`, whether THIS
/// call created the suffixed secret or found it already there (review fix M2):
/// a crash between a prior boot creating `-2` and that row's own transaction
/// committing would otherwise make a retry silently reuse `-2` (same value) and
/// never explain why. Since a row is migrated at most once, ever, the entry
/// this returns is bundled into that one row's one-time atomic write, so it is
/// written exactly once regardless of how many boots it took to get there.
async fn create_or_reuse_secret(
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    base_name: &str,
    value: &str,
) -> Result<(Uuid, String, Option<ReportEntry>), RegistryError> {
    for attempt in 0..MAX_SUFFIX_ATTEMPTS {
        let name = if attempt == 0 {
            base_name.to_string()
        } else {
            format!("{base_name}-{}", attempt + 1)
        };
        let collision_note = |used_name: &str| {
            (used_name != base_name).then(|| {
                ReportEntry::warning(
                    run_id,
                    "secret_name_collision",
                    format!("secret {base_name}"),
                    format!(
                        "a secret named {base_name} already exists with a different value; \
                         this credential was stored as {used_name} instead so it is never bound to the wrong secret"
                    ),
                )
                .detail(json!({ "wantedName": base_name, "usedName": used_name }))
            })
        };
        match secrets.create(None, &name, Some(EXTRACTED), value).await {
            Ok(id) => return Ok((id, name.clone(), collision_note(&name))),
            Err(SecretsError::AlreadyExists(_)) => {
                let existing = secrets
                    .list(None)
                    .await?
                    .into_iter()
                    .find(|m| m.name == name)
                    .ok_or_else(|| {
                        RegistryError::Conflict(format!(
                            "secret {name} exists but cannot be listed"
                        ))
                    })?;
                if secrets.resolve_plaintext(None, existing.id).await? == value {
                    return Ok((existing.id, name.clone(), collision_note(&name)));
                }
                // A DIFFERENT value already lives under this name: never reuse it.
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(RegistryError::Conflict(format!(
        "no free secret name for {base_name}"
    )))
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

/// Migrates one `egret-warehouse` legacy row: attach + map + report, atomically.
/// Returns the warehouse whose tag export this changed (`None` when the engine
/// already reached it and nothing was attached) — see step 2b.
async fn migrate_warehouse_row(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    row: &LegacyRow,
    egret_warehouse: Option<Uuid>,
) -> Result<Option<Uuid>, RegistryError> {
    let warehouse_id = row_warehouse(store, row).await?;
    let wh: WarehouseRow =
        sqlx::query_as("SELECT id, name, workspace_id FROM catalog.warehouses WHERE id = $1")
            .bind(warehouse_id)
            .fetch_optional(store.pool())
            .await?
            .ok_or_else(|| RegistryError::NotFound(format!("warehouse {warehouse_id}")))?;
    // Step 1 already created every warehouse's catalog in the common case; this
    // only creates one itself when step 1 failed for it (review fix M5) — so it
    // must make the SAME preferred-name decision step 1 would have (the
    // single-primary `egret` exception), and owes the SAME `warehouse_catalog_
    // created`/`primary_named_egret` report step 1 would have written, since
    // step 1's own `backfill_failed` entry says nothing about how this warehouse
    // actually ended up migrated.
    let preferred =
        if Some(wh.id) == egret_warehouse && store.get_catalog_by_name(EGRET).await?.is_none() {
            Some(EGRET)
        } else {
            None
        };
    let had_preferred = preferred.is_some();
    let (cat, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh.id,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            preferred,
            |cat: &CatalogRow| {
                let mut entries = vec![warehouse_created_entry(run_id, &wh, cat)];
                if had_preferred {
                    entries.push(primary_named_egret_entry(run_id, &wh, cat));
                }
                log_entries(&entries);
                entries
            },
        )
        .await?;
    let client = client_row_id(&row.config);

    if store.get_attachment(row.engine_id, cat.id).await?.is_some() {
        let mut entries = vec![ReportEntry::warning(
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
        .detail(json!({ "catalogClientId": client }))];
        if let Some(r) = renamed_entry(run_id, row, &cat.name, cat.id) {
            entries.push(r);
        }
        log_entries(&entries);
        store
            .map_legacy_row(row.id, row.engine_id, Some(cat.id), &entries)
            .await?;
        return Ok(None);
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
    let (secret_id, used_secret_name, collision_note) =
        create_or_reuse_secret(secrets, run_id, &secret_name, credential).await?;

    let mut entries = vec![ReportEntry::info(
        run_id,
        "secret_extracted",
        format!("engine {} catalog {}", row.engine_name, row.name),
        format!("the catalog client credential moved into secret {used_secret_name}"),
    )
    .engine(row.engine_id)
    .catalog(cat.id)];
    if let Some(note) = collision_note {
        entries.push(note.engine(row.engine_id).catalog(cat.id));
    }
    if let Some(r) = renamed_entry(run_id, row, &cat.name, cat.id) {
        entries.push(r);
    }
    log_entries(&entries);

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
            &entries,
        )
        .await?;
    Ok(Some(wh.id))
}

/// Migrates one non-`egret-warehouse` legacy row: share / suffix / classify,
/// then attach + map + report, atomically.
async fn migrate_connector_row(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    row: &LegacyRow,
    shared: &mut HashMap<String, (String, Uuid)>,
) -> Result<(), RegistryError> {
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
            let mut entries = vec![ReportEntry::info(
                run_id,
                "engine_catalog_shared",
                subject,
                format!("identical to catalog {base} on another engine; both engines now attach one catalog"),
            )
            .engine(row.engine_id)
            .catalog(catalog_id)];
            if let Some(r) = renamed_entry(run_id, row, &base, catalog_id) {
                entries.push(r);
            }
            log_entries(&entries);

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
                        &entries,
                    )
                    .await?;
            } else {
                store
                    .map_legacy_row(row.id, row.engine_id, Some(catalog_id), &entries)
                    .await?;
            }
            return Ok(());
        }
    }

    let mut entries: Vec<ReportEntry> = Vec::new();
    let name = if store.get_catalog_by_name(&base).await?.is_none() {
        base.clone()
    } else {
        let with_engine = join_truncated(&base, &normalize_catalog_name(&row.engine_name));
        let taken = store.names_starting_with(&with_engine).await?;
        let name = first_free_name(&with_engine, |n| taken.contains(n));
        entries.push(
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
        let (id, used_secret_name, collision_note) =
            create_or_reuse_secret(secrets, run_id, &secret_name, &value).await?;
        extracted.insert(property.clone(), id);
        entries.push(
            ReportEntry::info(
                run_id,
                "secret_extracted",
                subject.clone(),
                format!("plaintext {property} moved into secret {used_secret_name}"),
            )
            .engine(row.engine_id),
        );
        if let Some(note) = collision_note {
            entries.push(note.engine(row.engine_id));
        }
    }
    let classified = classify(&legacy, &extracted)?;
    let outcome = classified.outcome.clone();
    let kind_str = classified.def.kind().as_str().to_string();
    let name_for_report = name.clone();
    let subject_for_report = subject;
    let row_ref = row;
    let cat = store
        .create_catalog_for_legacy_row(
            &name,
            &classified.def,
            row.engine_id,
            row.id,
            row.created_at,
            move |cat_row| {
                entries.push(
                    match &outcome {
                        ClassifyOutcome::Typed => ReportEntry::info(
                            run_id,
                            "engine_catalog_typed",
                            subject_for_report.clone(),
                            format!("catalog {name_for_report} is a typed {kind_str} catalog"),
                        ),
                        ClassifyOutcome::Generic { reason } => ReportEntry::info(
                            run_id,
                            "engine_catalog_generic",
                            subject_for_report.clone(),
                            format!(
                                "catalog {name_for_report} keeps its properties as a generic catalog: {reason}"
                            ),
                        )
                        .detail(json!({ "reason": reason })),
                    }
                    .engine(row_ref.engine_id)
                    .catalog(cat_row.id),
                );
                if let Some(r) = renamed_entry(run_id, row_ref, &name_for_report, cat_row.id) {
                    entries.push(r);
                }
                log_entries(&entries);
                entries
            },
        )
        .await?;
    shared.insert(base, (key, cat.id));
    Ok(())
}
