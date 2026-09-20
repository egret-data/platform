//! `registry` schema store (SP3). Runtime sqlx queries, schema-qualified.
//! Reads `engine_mgmt.engines` / `engine_mgmt.engine_catalogs`,
//! `catalog.warehouses` and `workspaces` by name for joins; writes only
//! `registry.*`.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::config::{CatalogDef, ValidationError};
use crate::error::RegistryError;
use crate::name::{first_free_name, is_valid_catalog_name, normalize_catalog_name};
use crate::report::{ReportEntry, ReportRow};

macro_rules! catalog_cols {
    () => {
        "id, name, kind, config, advanced, description, warehouse_id, storage_connection_id, created_at, updated_at"
    };
}
macro_rules! attachment_cols {
    () => {
        "engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, apply_status, apply_error, applied_at, created_at"
    };
}

pub mod apply_status {
    pub const APPLIED: &str = "applied";
    pub const FAILED: &str = "failed";
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub advanced: serde_json::Value,
    pub description: Option<String>,
    pub warehouse_id: Option<Uuid>,
    pub storage_connection_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CatalogRow {
    pub fn def(&self) -> Result<CatalogDef, ValidationError> {
        CatalogDef::parse(&self.kind, &self.config, &self.advanced)
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AttachmentRow {
    pub engine_id: Uuid,
    pub catalog_id: Uuid,
    pub catalog_client_id: Option<Uuid>,
    pub credential_secret_id: Option<Uuid>,
    pub minted_client: bool,
    pub apply_status: String,
    pub apply_error: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl AttachmentRow {
    /// An attachment not yet written (attach renders it before committing it).
    pub fn pending(
        engine_id: Uuid,
        catalog_id: Uuid,
        catalog_client_id: Option<Uuid>,
        credential_secret_id: Option<Uuid>,
    ) -> Self {
        AttachmentRow {
            engine_id,
            catalog_id,
            catalog_client_id,
            credential_secret_id,
            minted_client: catalog_client_id.is_some(),
            apply_status: apply_status::APPLIED.into(),
            apply_error: None,
            applied_at: None,
            created_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AttachedCatalog {
    pub catalog: CatalogRow,
    pub attachment: AttachmentRow,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogEngineRow {
    pub engine_id: Uuid,
    pub engine_name: String,
    pub engine_type: String,
    pub apply_status: String,
    pub apply_error: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogWorkspaceRow {
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    pub assigned_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewAttachment {
    pub engine_id: Uuid,
    pub catalog_id: Uuid,
    pub catalog_client_id: Option<Uuid>,
    pub credential_secret_id: Option<Uuid>,
    pub minted_client: bool,
    /// Set `applied_at = now()` (the engine already accepted this attachment).
    pub applied_now: bool,
    /// Keep a legacy row's creation time, so attachment order — and with it the
    /// rendered env-var names — is unchanged after the migration.
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
pub struct WarehouseIdentity<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub workspace_id: Uuid,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogFilter {
    pub workspace_id: Option<Uuid>,
    pub secret_id: Option<Uuid>,
}

fn unique_constraint(e: &sqlx::Error) -> Option<String> {
    let d = e.as_database_error()?;
    (d.code().as_deref() == Some("23505")).then(|| d.constraint().map(str::to_owned))?
}

fn is_fk_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23503")
}

fn map_insert_error(e: sqlx::Error, name: &str) -> RegistryError {
    match unique_constraint(&e).as_deref() {
        Some("catalogs_name_key") => RegistryError::AlreadyExists(format!("catalog {name}")),
        Some("catalogs_warehouse_id_key") => {
            RegistryError::AlreadyExists("this warehouse already has a catalog".into())
        }
        _ => RegistryError::Db(e),
    }
}

pub(crate) async fn insert_catalog(
    conn: &mut PgConnection,
    name: &str,
    def: &CatalogDef,
    description: Option<&str>,
) -> Result<CatalogRow, sqlx::Error> {
    let row = sqlx::query_as::<_, CatalogRow>(concat!(
        "INSERT INTO registry.catalogs (name, kind, config, advanced, description, warehouse_id, storage_connection_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING ",
        catalog_cols!()
    ))
    .bind(name)
    .bind(def.kind().as_str())
    .bind(def.config_json())
    .bind(def.advanced_json())
    .bind(description)
    .bind(def.warehouse_id())
    .bind(def.storage_connection_id())
    .fetch_one(&mut *conn)
    .await?;
    replace_secret_refs(conn, row.id, &def.secret_ids()).await?;
    Ok(row)
}

async fn replace_secret_refs(
    conn: &mut PgConnection,
    catalog_id: Uuid,
    ids: &BTreeSet<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM registry.catalog_secret_refs WHERE catalog_id = $1")
        .bind(catalog_id)
        .execute(&mut *conn)
        .await?;
    let ids: Vec<Uuid> = ids.iter().copied().collect();
    sqlx::query("INSERT INTO registry.catalog_secret_refs (catalog_id, secret_id) SELECT $1, unnest($2::uuid[])")
        .bind(catalog_id)
        .bind(ids)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Writes one report row on `conn` — the low-level primitive every atomic
/// mutation+report transaction shares (see [`RegistryStore::insert_report`] and
/// the backfill's per-row/per-warehouse/per-scan writers in `backfill.rs`).
async fn insert_report_row(conn: &mut PgConnection, e: &ReportEntry) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO registry.migration_report \
           (run_id, severity, code, catalog_id, engine_id, warehouse_id, subject, message, detail) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(e.run_id)
    .bind(e.severity.as_str())
    .bind(e.code)
    .bind(e.catalog_id)
    .bind(e.engine_id)
    .bind(e.warehouse_id)
    .bind(&e.subject)
    .bind(&e.message)
    .bind(&e.detail)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn insert_attachment_row(
    conn: &mut PgConnection,
    a: &NewAttachment,
) -> Result<AttachmentRow, sqlx::Error> {
    sqlx::query_as::<_, AttachmentRow>(concat!(
        "INSERT INTO registry.engine_catalog_attachments \
           (engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, applied_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END, COALESCE($7, now())) RETURNING ",
        attachment_cols!()
    ))
    .bind(a.engine_id)
    .bind(a.catalog_id)
    .bind(a.catalog_client_id)
    .bind(a.credential_secret_id)
    .bind(a.minted_client)
    .bind(a.applied_now)
    .bind(a.created_at)
    .fetch_one(&mut *conn)
    .await
}

/// The backfill's only caller: marks an `engine_mgmt.engine_catalogs` row
/// migrated, so it is never revisited on a later boot.
pub(crate) async fn map_legacy_row_on(
    conn: &mut PgConnection,
    engine_catalog_id: Uuid,
    engine_id: Uuid,
    catalog_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO registry.legacy_engine_catalogs (engine_catalog_id, engine_id, catalog_id) \
         VALUES ($1, $2, $3) ON CONFLICT (engine_catalog_id) DO NOTHING",
    )
    .bind(engine_catalog_id)
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(Clone)]
pub struct RegistryStore {
    db: Db,
}

impl RegistryStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub(crate) fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    /// The database clock (what `updated_at` is written with).
    pub async fn db_now(&self) -> Result<DateTime<Utc>, RegistryError> {
        Ok(sqlx::query_scalar("SELECT now()")
            .fetch_one(self.pool())
            .await?)
    }

    // catalogs -----------------------------------------------------------------

    pub async fn create_catalog(
        &self,
        name: &str,
        def: &CatalogDef,
        description: Option<&str>,
    ) -> Result<CatalogRow, RegistryError> {
        if !is_valid_catalog_name(name) {
            return Err(ValidationError::Name(name.to_string()).into());
        }
        let mut tx = self.pool().begin().await?;
        let row = insert_catalog(&mut tx, name, def, description)
            .await
            .map_err(|e| map_insert_error(e, name))?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get_catalog(&self, id: Uuid) -> Result<Option<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(concat!(
            "SELECT ",
            catalog_cols!(),
            " FROM registry.catalogs WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn get_catalog_by_name(
        &self,
        name: &str,
    ) -> Result<Option<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(concat!(
            "SELECT ",
            catalog_cols!(),
            " FROM registry.catalogs WHERE name = $1"
        ))
        .bind(name)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn get_catalog_for_warehouse(
        &self,
        warehouse_id: Uuid,
    ) -> Result<Option<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(concat!(
            "SELECT ",
            catalog_cols!(),
            " FROM registry.catalogs WHERE warehouse_id = $1"
        ))
        .bind(warehouse_id)
        .fetch_optional(self.pool())
        .await?)
    }

    /// Catalogs, optionally only those assigned to a workspace and/or referencing
    /// a secret (config reference or an attachment's minted credential).
    pub async fn list_catalogs(
        &self,
        filter: &CatalogFilter,
    ) -> Result<Vec<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(
            "SELECT c.id, c.name, c.kind, c.config, c.advanced, c.description, c.warehouse_id, \
                    c.storage_connection_id, c.created_at, c.updated_at \
               FROM registry.catalogs c \
              WHERE ($1::uuid IS NULL OR EXISTS (SELECT 1 FROM registry.catalog_workspaces w \
                                                  WHERE w.catalog_id = c.id AND w.workspace_id = $1)) \
                AND ($2::uuid IS NULL \
                     OR EXISTS (SELECT 1 FROM registry.catalog_secret_refs r WHERE r.catalog_id = c.id AND r.secret_id = $2) \
                     OR EXISTS (SELECT 1 FROM registry.engine_catalog_attachments a \
                                 WHERE a.catalog_id = c.id AND a.credential_secret_id = $2)) \
              ORDER BY c.name",
        )
        .bind(filter.workspace_id)
        .bind(filter.secret_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Replace config/advanced/description (kind and name never change).
    pub async fn update_catalog(
        &self,
        id: Uuid,
        def: &CatalogDef,
        description: Option<&str>,
    ) -> Result<Option<CatalogRow>, RegistryError> {
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query_as::<_, CatalogRow>(concat!(
            "UPDATE registry.catalogs SET config = $2, advanced = $3, description = $4, \
                    warehouse_id = $5, storage_connection_id = $6, updated_at = now() \
              WHERE id = $1 AND kind = $7 RETURNING ",
            catalog_cols!()
        ))
        .bind(id)
        .bind(def.config_json())
        .bind(def.advanced_json())
        .bind(description)
        .bind(def.warehouse_id())
        .bind(def.storage_connection_id())
        .bind(def.kind().as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(r) = &row {
            replace_secret_refs(&mut tx, r.id, &def.secret_ids()).await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    /// `Conflict` while an attachment or assignment still references it.
    pub async fn delete_catalog(&self, id: Uuid) -> Result<bool, RegistryError> {
        match sqlx::query("DELETE FROM registry.catalogs WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await
        {
            Ok(r) => Ok(r.rows_affected() == 1),
            Err(e) if is_fk_violation(&e) => Err(RegistryError::Conflict(
                "the catalog is still attached or assigned".into(),
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// Create `name`; an existing catalog of that name is replaced when it is an
    /// orphan — attached to no engine, assigned to no workspace, not a warehouse
    /// catalog (plan decision #27: what a legacy engine-catalog remove leaves
    /// behind). Anything else under that name is `AlreadyExists`.
    ///
    /// Atomic: the old row is locked `FOR UPDATE` before it is checked, and the
    /// check, the delete and the insert share one transaction. An attachment or
    /// assignment insert takes `FOR KEY SHARE` on the catalog row, which
    /// conflicts with that lock, so one racing this call either committed first
    /// (and the check sees it: `AlreadyExists`, nothing changed) or waits until
    /// this commits (and then fails its foreign key: the row it named is gone).
    /// Returns the new row and whether an orphan was replaced.
    pub async fn create_catalog_replacing_orphan(
        &self,
        name: &str,
        def: &CatalogDef,
        description: Option<&str>,
    ) -> Result<(CatalogRow, bool), RegistryError> {
        if !is_valid_catalog_name(name) {
            return Err(ValidationError::Name(name.to_string()).into());
        }
        let in_use = || {
            RegistryError::AlreadyExists(format!(
                "a registry catalog named {name} already exists and is in use"
            ))
        };
        let mut tx = self.pool().begin().await?;
        let existing: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT id, warehouse_id FROM registry.catalogs WHERE name = $1 FOR UPDATE",
        )
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        let replaced = match existing {
            None => false,
            Some((_, Some(_))) => return Err(in_use()),
            Some((id, None)) => {
                let referenced: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM registry.engine_catalog_attachments WHERE catalog_id = $1) \
                         OR EXISTS (SELECT 1 FROM registry.catalog_workspaces WHERE catalog_id = $1)",
                )
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
                if referenced {
                    return Err(in_use());
                }
                sqlx::query("DELETE FROM registry.catalogs WHERE id = $1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        if is_fk_violation(&e) {
                            in_use()
                        } else {
                            e.into()
                        }
                    })?;
                true
            }
        };
        let row = insert_catalog(&mut tx, name, def, description)
            .await
            .map_err(|e| map_insert_error(e, name))?;
        tx.commit().await?;
        Ok((row, replaced))
    }

    pub(crate) async fn names_starting_with(
        &self,
        prefix: &str,
    ) -> Result<HashSet<String>, RegistryError> {
        // left(), not LIKE: `_` is a LIKE wildcard and catalog names are full of it.
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM registry.catalogs WHERE left(name, length($1)) = $1",
        )
        .bind(prefix)
        .fetch_all(self.pool())
        .await?;
        Ok(names.into_iter().collect())
    }

    /// The catalog of a warehouse, created (normalised name, assigned to the
    /// warehouse's workspace) the first time. The assignment is written only with
    /// the catalog, so an admin's later un-assignment is never undone here.
    /// `preferred_name` is used verbatim when free (the backfill's `egret`).
    ///
    /// `report_for` is called at most once, only when a catalog is actually
    /// created, right before the transaction commits: whatever it returns is
    /// written in the SAME transaction, so the mutation and its report rows are
    /// never observed apart (a crash before commit loses neither or both).
    pub async fn ensure_warehouse_catalog(
        &self,
        wh: &WarehouseIdentity<'_>,
        preferred_name: Option<&str>,
        report_for: impl FnOnce(&CatalogRow) -> Vec<ReportEntry> + Send,
    ) -> Result<(CatalogRow, bool), RegistryError> {
        if let Some(c) = self.get_catalog_for_warehouse(wh.id).await? {
            return Ok((c, false));
        }
        let def = CatalogDef::egret_warehouse(wh.id);
        let base = preferred_name
            .map(str::to_string)
            .unwrap_or_else(|| normalize_catalog_name(wh.name));
        for _ in 0..32 {
            let taken = self.names_starting_with(&base).await?;
            let name = first_free_name(&base, |n| taken.contains(n));
            let mut tx = self.pool().begin().await?;
            match insert_catalog(&mut tx, &name, &def, None).await {
                Ok(row) => {
                    sqlx::query(
                        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2) \
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(row.id)
                    .bind(wh.workspace_id)
                    .execute(&mut *tx)
                    .await?;
                    for e in report_for(&row) {
                        insert_report_row(&mut tx, &e).await?;
                    }
                    tx.commit().await?;
                    return Ok((row, true));
                }
                Err(e) => {
                    tx.rollback().await?;
                    match unique_constraint(&e).as_deref() {
                        Some("catalogs_name_key") => continue,
                        Some("catalogs_warehouse_id_key") => {
                            let c =
                                self.get_catalog_for_warehouse(wh.id)
                                    .await?
                                    .ok_or_else(|| {
                                        RegistryError::NotFound(format!(
                                            "catalog of warehouse {}",
                                            wh.id
                                        ))
                                    })?;
                            return Ok((c, false));
                        }
                        _ => return Err(e.into()),
                    }
                }
            }
        }
        Err(RegistryError::Conflict(format!(
            "no free catalog name for warehouse {}",
            wh.name
        )))
    }

    // attachments --------------------------------------------------------------

    pub async fn insert_attachment(
        &self,
        a: &NewAttachment,
    ) -> Result<AttachmentRow, RegistryError> {
        let mut conn = self.pool().acquire().await?;
        insert_attachment_row(&mut conn, a).await.map_err(|e| {
            match unique_constraint(&e).as_deref() {
                Some("engine_catalog_attachments_pkey") => RegistryError::AlreadyExists(
                    "the catalog is already attached to this engine".into(),
                ),
                _ => RegistryError::Db(e),
            }
        })
    }

    pub async fn get_attachment(
        &self,
        engine_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<Option<AttachmentRow>, RegistryError> {
        Ok(sqlx::query_as::<_, AttachmentRow>(concat!(
            "SELECT ",
            attachment_cols!(),
            " FROM registry.engine_catalog_attachments WHERE engine_id = $1 AND catalog_id = $2"
        ))
        .bind(engine_id)
        .bind(catalog_id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_attachment(
        &self,
        engine_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<bool, RegistryError> {
        let r = sqlx::query("DELETE FROM registry.engine_catalog_attachments WHERE engine_id = $1 AND catalog_id = $2")
            .bind(engine_id)
            .bind(catalog_id)
            .execute(self.pool())
            .await?;
        Ok(r.rows_affected() == 1)
    }

    /// Whether an attachment OTHER than (`engine_id`, `catalog_id`) still names
    /// `client_id` as its Catalog Client.
    ///
    /// Two attachments can share one client: the backfill takes it verbatim
    /// from the legacy row and derives the credential secret's name from it
    /// (`catalog-client-{id}`), so `create_or_reuse_secret` deliberately
    /// collapses them onto one secret. Revoking on behalf of one of them would
    /// destroy a credential the OTHER engine's rendered Trino properties still
    /// present — the standing "never destroy a credential an engine may still
    /// hold" rule, broken through the revoke door instead of the delete door
    /// (review F-5). The pair is excluded rather than assumed gone so the
    /// answer is the same whether the caller deleted its row first or not.
    pub async fn client_used_by_another_attachment(
        &self,
        client_id: Uuid,
        engine_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<bool, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM registry.engine_catalog_attachments \
                             WHERE catalog_client_id = $1 \
                               AND NOT (engine_id = $2 AND catalog_id = $3))",
        )
        .bind(client_id)
        .bind(engine_id)
        .bind(catalog_id)
        .fetch_one(self.pool())
        .await?)
    }

    /// An engine's catalogs in render order: its primary (the catalog of
    /// `primary_warehouse`) first, then by attachment time, then name — the
    /// order `engine_catalogs` rendered in, so env-var names stay put.
    pub async fn list_engine_attachments(
        &self,
        engine_id: Uuid,
        primary_warehouse: Option<Uuid>,
    ) -> Result<Vec<AttachedCatalog>, RegistryError> {
        let attachments = sqlx::query_as::<_, AttachmentRow>(
            "SELECT a.engine_id, a.catalog_id, a.catalog_client_id, a.credential_secret_id, a.minted_client, \
                    a.apply_status, a.apply_error, a.applied_at, a.created_at \
               FROM registry.engine_catalog_attachments a JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = $1 \
              ORDER BY (c.warehouse_id IS NOT DISTINCT FROM $2) DESC, a.created_at, c.name",
        )
        .bind(engine_id)
        .bind(primary_warehouse)
        .fetch_all(self.pool())
        .await?;
        let ids: Vec<Uuid> = attachments.iter().map(|a| a.catalog_id).collect();
        let catalogs = sqlx::query_as::<_, CatalogRow>(concat!(
            "SELECT ",
            catalog_cols!(),
            " FROM registry.catalogs WHERE id = ANY($1)"
        ))
        .bind(ids)
        .fetch_all(self.pool())
        .await?;
        attachments
            .into_iter()
            .map(|attachment| {
                let catalog = catalogs
                    .iter()
                    .find(|c| c.id == attachment.catalog_id)
                    .cloned()
                    .ok_or_else(|| {
                        RegistryError::NotFound(format!("catalog {}", attachment.catalog_id))
                    })?;
                Ok(AttachedCatalog {
                    catalog,
                    attachment,
                })
            })
            .collect()
    }

    /// The engines a catalog is attached to, with their apply status.
    pub async fn list_catalog_attachments(
        &self,
        catalog_id: Uuid,
    ) -> Result<Vec<CatalogEngineRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogEngineRow>(
            "SELECT a.engine_id, e.name AS engine_name, e.type AS engine_type, a.apply_status, a.apply_error, a.applied_at \
               FROM registry.engine_catalog_attachments a JOIN engine_mgmt.engines e ON e.id = a.engine_id \
              WHERE a.catalog_id = $1 ORDER BY e.name",
        )
        .bind(catalog_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn attached_catalog_names(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<String>, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT c.name FROM registry.engine_catalog_attachments a JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = $1 ORDER BY c.name",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// After a successful full apply: every attachment whose catalog the apply
    /// rendered (unchanged since `read_at`) is on the engine now.
    pub async fn mark_engine_applied(
        &self,
        engine_id: Uuid,
        read_at: DateTime<Utc>,
    ) -> Result<(), RegistryError> {
        sqlx::query(
            "UPDATE registry.engine_catalog_attachments a \
                SET apply_status = 'applied', apply_error = NULL, applied_at = now() \
               FROM registry.catalogs c \
              WHERE c.id = a.catalog_id AND a.engine_id = $1 AND c.updated_at <= $2",
        )
        .bind(engine_id)
        .bind(read_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn set_apply_failed(
        &self,
        engine_id: Uuid,
        catalog_id: Uuid,
        error: &str,
    ) -> Result<(), RegistryError> {
        sqlx::query(
            "UPDATE registry.engine_catalog_attachments SET apply_status = 'failed', apply_error = $3 \
              WHERE engine_id = $1 AND catalog_id = $2",
        )
        .bind(engine_id)
        .bind(catalog_id)
        .bind(error)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    // workspaces ---------------------------------------------------------------

    pub async fn list_catalog_workspaces(
        &self,
        catalog_id: Uuid,
    ) -> Result<Vec<CatalogWorkspaceRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogWorkspaceRow>(
            "SELECT cw.workspace_id, w.name, w.slug, cw.assigned_at \
               FROM registry.catalog_workspaces cw JOIN workspaces w ON w.id = cw.workspace_id \
              WHERE cw.catalog_id = $1 ORDER BY w.name",
        )
        .bind(catalog_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Replace the full assignment set.
    pub async fn set_catalog_workspaces(
        &self,
        catalog_id: Uuid,
        workspace_ids: &[Uuid],
    ) -> Result<(), RegistryError> {
        let ids = workspace_ids.to_vec();
        let mut tx = self.pool().begin().await?;
        sqlx::query("DELETE FROM registry.catalog_workspaces WHERE catalog_id = $1 AND NOT (workspace_id = ANY($2))")
            .bind(catalog_id)
            .bind(ids.clone())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) \
             SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
        )
        .bind(catalog_id)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The §4.6 "assigned OR owned-and-not-yet-assigned" visibility rule, as
    /// one executable predicate: a workspace sees a warehouse when its registry
    /// catalog is assigned there — or, for a warehouse the backfill has not
    /// reached yet (no registry catalog at all), when the warehouse is that
    /// workspace's own.
    ///
    /// NO PRODUCTION CALLER TODAY, deliberately (review F-10). It is kept for
    /// two reasons, and a third that is NOT one of them:
    ///   - `egret-query`'s `list_engine_catalogs` re-states the same two
    ///     branches in SQL, per-row, because it iterates catalogs that are
    ///     already known to exist; this function is the statement that SQL
    ///     mirrors, and the DB tests here and in `egret-catalog-engine` are
    ///     what keep the two honest (including the fail-closed direction:
    ///     reassigning a catalog away from its owner revokes the owner).
    ///   - SP4 — the per-workspace notebook role + namespace — is the consumer
    ///     that will call it, and is the reason it was written.
    ///   - it is NOT an authorization gate, and notebook warehouse selection
    ///     does NOT use it: that check is deliberately owner-only (user
    ///     decision, 2026-09-20), because notebook selection provisions ONE
    ///     shared role per warehouse. Calling this there before SP4 gives an
    ///     assigned workspace read/write on the OWNER's private namespace.
    ///
    /// Delete it with SP4 if SP4 ends up needing a different predicate.
    pub async fn warehouse_visible_to_workspace(
        &self,
        warehouse_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<bool, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM registry.catalogs c JOIN registry.catalog_workspaces cw ON cw.catalog_id = c.id \
                             WHERE c.warehouse_id = $1 AND cw.workspace_id = $2) \
                 OR (NOT EXISTS (SELECT 1 FROM registry.catalogs c WHERE c.warehouse_id = $1) \
                     AND EXISTS (SELECT 1 FROM catalog.warehouses w WHERE w.id = $1 AND w.workspace_id = $2))",
        )
        .bind(warehouse_id)
        .bind(workspace_id)
        .fetch_one(self.pool())
        .await?)
    }

    // legacy map + report ------------------------------------------------------

    /// `engine_mgmt.engine_catalogs` rows of this engine the backfill has not
    /// migrated. Non-zero means rendering from the registry would drop catalogs
    /// the engine runs today, so applies refuse (engines.rs).
    pub async fn unmigrated_legacy_catalog_count(
        &self,
        engine_id: Uuid,
    ) -> Result<i64, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engine_catalogs c \
              WHERE c.engine_id = $1 \
                AND NOT EXISTS (SELECT 1 FROM registry.legacy_engine_catalogs m WHERE m.engine_catalog_id = c.id)",
        )
        .bind(engine_id)
        .fetch_one(self.pool())
        .await?)
    }

    /// Backfill: attach, mark the legacy row migrated, write its report rows,
    /// and bump the tag versions this attachment changes — atomically. A crash
    /// before commit loses all of them together (the row stays unmapped and is
    /// retried whole next boot); a crash after commit loses none (review fix
    /// I1, extended by review F-14).
    ///
    /// `bump_tags_for_warehouse` is the warehouse whose Ranger tag export this
    /// attachment changes (`None` for a catalog with no warehouse, whose tags
    /// the export does not carry). It MUST be part of this transaction, not a
    /// pass afterwards: step 2 only ever selects legacy rows that are not yet
    /// in `registry.legacy_engine_catalogs`, so once this commits the row is
    /// never revisited. A bump left for later and lost to a kill — OOM, a
    /// liveness probe, a failed deploy — is lost PERMANENTLY, and the plugin
    /// 304s forever on a catalog the export has started emitting. Inside the
    /// transaction, a bump that fails takes the attachment down with it and the
    /// whole row is retried on the next boot.
    pub(crate) async fn insert_attachment_and_map(
        &self,
        a: &NewAttachment,
        engine_catalog_id: Uuid,
        report: &[ReportEntry],
        bump_tags_for_warehouse: Option<Uuid>,
    ) -> Result<(), RegistryError> {
        let mut tx = self.pool().begin().await?;
        insert_attachment_row(&mut tx, a).await?;
        map_legacy_row_on(&mut tx, engine_catalog_id, a.engine_id, Some(a.catalog_id)).await?;
        for e in report {
            insert_report_row(&mut tx, e).await?;
        }
        if let Some(warehouse_id) = bump_tags_for_warehouse {
            crate::tag_version::bump_bound_tag_versions(&mut *tx, warehouse_id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Backfill: mark a legacy row migrated (without writing an attachment) and
    /// its report rows, atomically (review fix I1).
    pub(crate) async fn map_legacy_row(
        &self,
        engine_catalog_id: Uuid,
        engine_id: Uuid,
        catalog_id: Option<Uuid>,
        report: &[ReportEntry],
    ) -> Result<(), RegistryError> {
        let mut tx = self.pool().begin().await?;
        map_legacy_row_on(&mut tx, engine_catalog_id, engine_id, catalog_id).await?;
        for e in report {
            insert_report_row(&mut tx, e).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Backfill: catalog + attachment + map, atomically. `report_for` is called
    /// once, after the catalog row exists (so its id is known) but before commit,
    /// same atomicity guarantee as [`Self::ensure_warehouse_catalog`] (review fix I1).
    pub(crate) async fn create_catalog_for_legacy_row(
        &self,
        name: &str,
        def: &CatalogDef,
        engine_id: Uuid,
        engine_catalog_id: Uuid,
        created_at: DateTime<Utc>,
        report_for: impl FnOnce(&CatalogRow) -> Vec<ReportEntry> + Send,
    ) -> Result<CatalogRow, RegistryError> {
        let mut tx = self.pool().begin().await?;
        let row = insert_catalog(&mut tx, name, def, None)
            .await
            .map_err(|e| map_insert_error(e, name))?;
        insert_attachment_row(
            &mut tx,
            &NewAttachment {
                engine_id,
                catalog_id: row.id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: false,
                created_at: Some(created_at),
            },
        )
        .await?;
        map_legacy_row_on(&mut tx, engine_catalog_id, engine_id, Some(row.id)).await?;
        for e in report_for(&row) {
            insert_report_row(&mut tx, &e).await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    /// Backfill step 3: every `engine_catalog_renamed` report row whose saved-SQL
    /// scan has not completed yet — from this run's rows (already committed by the
    /// time step 3 runs) and any earlier boot's (a crash, or a caught scan error,
    /// left `scanPending` set). `scanPending` defaults to true when the key is
    /// absent, so a row written before this marker existed is still picked up.
    /// `(report id, old name, new name)`.
    pub(crate) async fn pending_saved_sql_scans(
        &self,
    ) -> Result<Vec<(i64, String, Option<String>)>, RegistryError> {
        Ok(sqlx::query_as(
            "SELECT id, detail->>'oldName', detail->>'newName' FROM registry.migration_report \
              WHERE code = 'engine_catalog_renamed' \
                AND COALESCE((detail->>'scanPending')::boolean, true) \
              ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?)
    }

    /// Backfill step 3: persist scan-hit report rows and clear `scanPending` on
    /// every rename row that triggered them, atomically — a crash (or DB error)
    /// before this commits leaves the markers untouched, so the whole scan (not
    /// just the missing hits) is retried on the next boot rather than silently
    /// treated as done (review fix I1).
    pub(crate) async fn complete_saved_sql_scan(
        &self,
        rename_report_ids: &[i64],
        entries: &[ReportEntry],
    ) -> Result<(), RegistryError> {
        let mut tx = self.pool().begin().await?;
        for e in entries {
            insert_report_row(&mut tx, e).await?;
        }
        sqlx::query(
            "UPDATE registry.migration_report SET detail = jsonb_set(detail, '{scanPending}', 'false'::jsonb) \
              WHERE id = ANY($1)",
        )
        .bind(rename_report_ids)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn insert_report(&self, entries: &[ReportEntry]) -> Result<(), RegistryError> {
        let mut tx = self.pool().begin().await?;
        for e in entries {
            insert_report_row(&mut tx, e).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_report(
        &self,
        include_acknowledged: bool,
    ) -> Result<Vec<ReportRow>, RegistryError> {
        Ok(sqlx::query_as::<_, ReportRow>(
            "SELECT id, run_id, created_at, severity, code, catalog_id, engine_id, warehouse_id, subject, message, \
                    detail, acknowledged_at \
               FROM registry.migration_report \
              WHERE $1 OR acknowledged_at IS NULL ORDER BY id",
        )
        .bind(include_acknowledged)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn acknowledge_report(&self) -> Result<u64, RegistryError> {
        Ok(sqlx::query("UPDATE registry.migration_report SET acknowledged_at = now() WHERE acknowledged_at IS NULL")
            .execute(self.pool())
            .await?
            .rows_affected())
    }
}
