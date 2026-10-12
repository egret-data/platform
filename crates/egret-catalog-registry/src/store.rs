//! `registry` schema store (SP3). Runtime sqlx queries, schema-qualified.
//! Reads `engine_mgmt.engines`, `catalog.warehouses` and `workspaces` by name
//! for joins; writes only `registry.*`.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::config::{CatalogDef, ValidationError};
use crate::error::RegistryError;
use crate::name::{first_free_name, is_valid_catalog_name, normalize_catalog_name};

macro_rules! catalog_cols {
    () => {
        "id, name, kind, config, advanced, description, warehouse_id, storage_connection_id, created_at, updated_at"
    };
}
macro_rules! attachment_cols {
    () => {
        "engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, apply_status, apply_error, applied_at, is_primary, created_at"
    };
}

pub mod apply_status {
    pub const APPLIED: &str = "applied";
    pub const FAILED: &str = "failed";
    /// Recorded by egret, not yet accepted by the engine (SP3b D1). Only a
    /// StarRocks attachment is ever written this way: a Trino apply happens
    /// before the row exists at all.
    pub const PENDING: &str = "pending";
    /// An attachment egret records and never applies: the engine is external
    /// (`engines.origin = 'external'`), so there is no cluster to converge. It
    /// carries no client and no credential (`attachments_declared_holds_nothing`,
    /// migration 0184). `egret_engine_mgmt::external` is its one writer.
    pub const DECLARED: &str = "declared";
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
    /// The engine's PRIMARY catalog (SP3c D2). At most one attachment per
    /// engine carries it — `engine_catalog_attachments_primary_key`, migration
    /// 0150 — and it is what `list_engine_attachments` orders first, so Trino's
    /// catalog env-var numbering depends on it.
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

impl AttachmentRow {
    /// An attachment not yet written (attach renders it before committing it).
    ///
    /// Never primary: the row an attach renders is an ADDITIONAL catalog on an
    /// engine that already has its primary, and a pending row claiming primacy
    /// would put itself first in the render order — renumbering the live
    /// cluster's catalog env vars for the duration of the apply.
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
            is_primary: false,
            created_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AttachedCatalog {
    pub catalog: CatalogRow,
    pub attachment: AttachmentRow,
}

impl AttachedCatalog {
    /// Whether this is the engine's primary catalog (SP3c D2). Read from the
    /// attachment rather than stored a second time here: primacy is one fact,
    /// and a copy on the pair could disagree with the row it was built from.
    /// Until SP3c-1 this was recomputed at three call sites as
    /// `catalog.warehouse_id == Some(engine.warehouse_id)`; two of those
    /// sites were deleted with the legacy routes in SP3c-2 Task 4, and
    /// `catalog_apply::is_primary` is the one that remains.
    pub fn is_primary(&self) -> bool {
        self.attachment.is_primary
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogEngineRow {
    pub engine_id: Uuid,
    pub engine_name: String,
    pub engine_type: String,
    pub apply_status: String,
    pub apply_error: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    /// This catalog is that engine's primary (SP3c D2).
    pub is_primary: bool,
    /// When the attachment row was written. `apply_status` is only ever set to
    /// `pending` by `insert_attachment_row`, so this is also the moment a
    /// `pending` attachment started waiting — the clock a "still pending"
    /// warning has to be measured against.
    pub created_at: DateTime<Utc>,
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
    /// The engine's PRIMARY catalog (SP3c D2). A second `true` for the same
    /// engine is refused by `engine_catalog_attachments_primary_key` as an
    /// `AlreadyExists` — see [`RegistryStore::insert_attachment`].
    pub is_primary: bool,
    /// Keep a legacy row's creation time, so attachment order — and with it the
    /// rendered env-var names — is unchanged after the migration.
    pub created_at: Option<DateTime<Utc>>,
    /// One of `apply_status::{APPLIED, FAILED, PENDING, DECLARED}`. Every existing call
    /// site passes `APPLIED` (SP3b Task 3 keeps this step behaviour-neutral);
    /// Task 5 is the one that starts writing `PENDING` for a StarRocks attach.
    pub apply_status: &'static str,
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

pub(crate) async fn insert_attachment_row(
    conn: &mut PgConnection,
    a: &NewAttachment,
) -> Result<AttachmentRow, sqlx::Error> {
    sqlx::query_as::<_, AttachmentRow>(concat!(
        "INSERT INTO registry.engine_catalog_attachments \
           (engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, apply_status, applied_at, is_primary, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7 THEN now() END, $9, COALESCE($8, now())) RETURNING ",
        attachment_cols!()
    ))
    .bind(a.engine_id)
    .bind(a.catalog_id)
    .bind(a.catalog_client_id)
    .bind(a.credential_secret_id)
    .bind(a.minted_client)
    .bind(a.apply_status)
    .bind(a.applied_now)
    .bind(a.created_at)
    .bind(a.is_primary)
    .fetch_one(&mut *conn)
    .await
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
        self.delete_catalog_with_compiler(id, None).await
    }

    /// [`Self::delete_catalog`], plus sweeping any compiled policy this
    /// catalog's (about to be cascade-deleted) grants left behind (SP4a Task
    /// 16 re-enumeration: found alongside D3 defect 3, same shape).
    ///
    /// **This is the SAME defect as `delete_warehouse_releasing_catalog`'s,
    /// on a path Task 15 did not enumerate.** `perm.grants.catalog_id`
    /// `ON DELETE CASCADE`s from `registry.catalogs`, so any grant scoped to
    /// this catalog — including an EXPLICIT one, created directly through the
    /// `perm/grants` API rather than through a workspace assignment — goes
    /// with it, silently, with no recompile. The production caller
    /// (`egret-engine-mgmt::catalogs::delete_registry_catalog`) already
    /// refuses this call while any engine attachment or workspace assignment
    /// exists, exactly like `delete_warehouse_releasing_catalog`'s own guard —
    /// so "services this catalog reaches now" is always empty here too, and
    /// the same `egret_perm::compiled_service_ids_for_catalog_via` historical
    /// read is what can still find a profile that reached this catalog in the
    /// past and was detached without a successful recompile.
    pub async fn delete_catalog_with_compiler(
        &self,
        id: Uuid,
        compiler: Option<&dyn crate::CatalogAssignmentCompiler>,
    ) -> Result<bool, RegistryError> {
        let mut tx = self.pool().begin().await?;
        // Captured before the delete: `perm.grants`/`perm.compiled` for this
        // catalog are about to cascade away.
        let mut affected_services =
            egret_perm::compiled_service_ids_for_catalog_via(&mut tx, id).await?;
        affected_services.sort_unstable();
        affected_services.dedup();
        let result = sqlx::query("DELETE FROM registry.catalogs WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await;
        match result {
            Ok(r) => {
                let deleted = r.rows_affected() == 1;
                if deleted {
                    if let (Some(c), false) = (compiler, affected_services.is_empty()) {
                        c.recompile_services_in_tx(&mut tx, &affected_services)
                            .await?;
                    }
                }
                tx.commit().await?;
                Ok(deleted)
            }
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
    /// `preferred_name` is used verbatim when free (only a `#[cfg(test)]`
    /// helper passes one today).
    ///
    /// The `catalog_workspaces` write also syncs the implicit `perm.grants`
    /// row for that (catalog, workspace) pair, in the SAME transaction (SP4a
    /// Task 6 fix round 1, Important 1) — this is the OTHER path that
    /// assigns a catalog to a workspace, alongside the PUT
    /// `/catalogs/{id}/workspaces` handler
    /// (`egret-engine-mgmt::catalogs::set_catalog_workspaces`), and D3's
    /// invariant ("assigning a catalog to a workspace grants it read") must
    /// hold on both or the review-caught defect shape repeats: true on one
    /// write path, silently false on the other. `sync_assignment_grants_in_tx`
    /// is safe to call unconditionally here even though it is a REPLACE (not
    /// an upsert-only) operation — the catalog row was just created in this
    /// same transaction, so it can carry no other implicit grant yet.
    pub async fn ensure_warehouse_catalog(
        &self,
        wh: &WarehouseIdentity<'_>,
        preferred_name: Option<&str>,
    ) -> Result<(CatalogRow, bool), RegistryError> {
        self.ensure_warehouse_catalog_with_compiler(wh, preferred_name, None)
            .await
    }

    /// [`Self::ensure_warehouse_catalog`], plus recompiling the implicit
    /// assignment grant's compiled policy in the SAME transaction as the
    /// grant sync (SP4a Task 16, D3 defect 2 — Task 15 left this open: see
    /// `crate::compiler_port` for why the port, not a direct call, is what
    /// closes it without pulling `egret-perm-compile`/`egret-policy` into
    /// this crate's production dependency graph).
    ///
    /// `compiler` is `None` for every caller that has none wired (every test
    /// in this crate, and any production caller that predates this task) —
    /// behaviour is then byte-identical to the old `ensure_warehouse_catalog`,
    /// exactly as `egret_engine_mgmt::EngineMgmtState::perm_compiler`'s own
    /// `None` case works for `set_catalog_workspaces`.
    ///
    /// **Why this is a no-op for every caller today, and why it is still
    /// correct to add.** `registry.catalogs` is the row `perm.grants.catalog_id`
    /// and `registry.engine_catalog_attachments.catalog_id` both FK to, and
    /// this branch only runs once — the FIRST time a warehouse's catalog is
    /// created (the `get_catalog_for_warehouse` guard above returns early on
    /// every later call). No attachment can reference a catalog row that does
    /// not exist yet, so at the exact moment this grant is written, ZERO
    /// engines can already reach it — `egret_policy::services_for_catalog_via`
    /// is provably empty here for every caller in this codebase today. The
    /// call is still correct to make: it closes the invariant ("every
    /// perm.grants write recompiles in the same transaction, unconditionally")
    /// rather than leaving it true only by an ordering argument a future
    /// caller (a backfill, a different attach order) could invalidate
    /// silently, exactly the "true on one path, false on another" shape this
    /// task exists to close everywhere it appears.
    pub async fn ensure_warehouse_catalog_with_compiler(
        &self,
        wh: &WarehouseIdentity<'_>,
        preferred_name: Option<&str>,
        compiler: Option<&dyn crate::CatalogAssignmentCompiler>,
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
                    egret_perm::sync_assignment_grants_in_tx(&mut tx, row.id, &[wh.workspace_id])
                        .await?;
                    if let Some(c) = compiler {
                        c.recompile_catalog_assignment_in_tx(&mut tx, row.id)
                            .await?;
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

    /// [`insert_attachment`](Self::insert_attachment) on the CALLER's open
    /// transaction, so an attachment and the recompile it makes necessary commit
    /// (or roll back) together. Same conflict mapping.
    pub async fn insert_attachment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        a: &NewAttachment,
    ) -> Result<AttachmentRow, RegistryError> {
        insert_attachment_row(&mut **tx, a).await.map_err(|e| {
            match unique_constraint(&e).as_deref() {
                Some("engine_catalog_attachments_pkey") => RegistryError::AlreadyExists(
                    "the catalog is already attached to this engine".into(),
                ),
                Some("engine_catalog_attachments_primary_key") => {
                    RegistryError::AlreadyExists("this engine already has a primary catalog".into())
                }
                _ => RegistryError::Db(e),
            }
        })
    }

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
                // Migration 0150's partial unique index. An engine has ONE
                // primary catalog, so a second `is_primary` insert is a
                // conflict and not a silent overwrite (SP3c D2).
                Some("engine_catalog_attachments_primary_key") => {
                    RegistryError::AlreadyExists("this engine already has a primary catalog".into())
                }
                _ => RegistryError::Db(e),
            }
        })
    }

    // NOTE (SP3c-1 review F4): there is deliberately no `set_primary_attachment`
    // here. Nothing in SP3c-1 moves primacy after create, and the obvious
    // one-statement implementation (`SET is_primary = (catalog_id = $2)`) turns a
    // catalog_id that is NOT attached to this engine into a silent `Ok(())` that
    // leaves the engine with ZERO primaries — a new door onto exactly the gap
    // `list_engine_attachments` warns about, opened by the change that closed it.
    // It also cannot check, on its own, that the new primary is an egret
    // warehouse catalog, which SP3c-1 still requires. SP3c-2 adds it WITH its
    // caller, its guard and its test.

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
    /// Two attachments can share one client: the (now-gone) boot backfill took
    /// it verbatim from the legacy row and derived the credential secret's
    /// name from it (`catalog-client-{id}`), so `create_or_reuse_secret` deliberately
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

    /// An engine's catalogs in render order: its primary first, then by
    /// attachment time, then name — the order `engine_catalogs` rendered in, so
    /// env-var names stay put. Until SP3c-1 "primary" was the caller's
    /// `engine.warehouse_id` compared against each catalog's; migration 0150
    /// backfilled `is_primary` from exactly that predicate, so this ORDER BY
    /// returns existing rows in exactly the order the old one did
    /// (`attachments_still_come_back_primary_first_then_created_then_name`).
    ///
    /// WHERE "EXACTLY ONE PRIMARY" LIVES. The database gives *at most* one
    /// (`engine_catalog_attachments_primary_key`, a partial unique index —
    /// there is no anchor row to hang a CHECK on, so zero is representable).
    /// The code owns the lower bound: `create_engine` refuses a body without a
    /// primary and `detach_from_engine` refuses to remove one. An engine that
    /// nevertheless has attachments and no primary — SP3a's backfill skipped
    /// it, or someone wrote the table by hand — is a real state with a real
    /// consequence (whichever catalog happens to sort first becomes Trino's
    /// `C0`), so it is WARNED here rather than passed on silently. This is the
    /// one place every render path goes through.
    pub async fn list_engine_attachments(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<AttachedCatalog>, RegistryError> {
        let attachments = sqlx::query_as::<_, AttachmentRow>(
            "SELECT a.engine_id, a.catalog_id, a.catalog_client_id, a.credential_secret_id, a.minted_client, \
                    a.apply_status, a.apply_error, a.applied_at, a.is_primary, a.created_at \
               FROM registry.engine_catalog_attachments a JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = $1 \
              ORDER BY a.is_primary DESC, a.created_at, c.name",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?;
        if !attachments.is_empty() && !attachments.iter().any(|a| a.is_primary) {
            tracing::warn!(
                %engine_id,
                attachments = attachments.len(),
                "engine has catalogs but no primary attachment; the first catalog by \
                 attachment time is being rendered as the engine's leading catalog. \
                 Mark one primary (SP3c D2, migration 0150)"
            );
        }
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
            "SELECT a.engine_id, e.name AS engine_name, e.type AS engine_type, a.apply_status, a.apply_error, a.applied_at, a.is_primary, a.created_at \
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

    /// The warehouse an engine leads with: the `warehouse_id` of its PRIMARY
    /// attachment's catalog. SP3c-2 Task 5 made this the source for the two
    /// facts that used to come off `engines.warehouse_id` — a StarRocks
    /// engine's builtin storage volume and (through `ensure_engine_role`) the
    /// engine's Ranger role.
    ///
    /// `None` covers BOTH ways an engine can fail to name one: no primary
    /// attachment at all (migration 0150's partial unique index gives *at
    /// most* one, never at least one), and a primary on a catalog with no
    /// warehouse. The callers treat them alike — they need a warehouse and
    /// there is none — and refuse rather than pick a catalog to stand in.
    /// `create_engine` refuses a warehouse-less primary before it writes any
    /// row, so the second case needs the table to have been written by
    /// something other than egret.
    pub async fn primary_warehouse_id(
        &self,
        engine_id: Uuid,
    ) -> Result<Option<Uuid>, RegistryError> {
        Ok(sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT c.warehouse_id \
               FROM registry.catalogs c \
               JOIN registry.engine_catalog_attachments a ON a.catalog_id = c.id \
              WHERE a.engine_id = $1 AND a.is_primary",
        )
        .bind(engine_id)
        .fetch_optional(self.pool())
        .await?
        .flatten())
    }

    /// Every warehouse each of `engine_ids` reaches — one per attached egret
    /// warehouse catalog, primary first, then by attachment time. Engines with
    /// no warehouse-backed attachment are absent from the map rather than
    /// present with an empty vec.
    ///
    /// One query, not one per engine: the policy resource picker builds this
    /// for every provisioned engine on every autocomplete keystroke.
    pub async fn warehouse_ids_by_engine(
        &self,
        engine_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<Uuid>>, RegistryError> {
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT a.engine_id, c.warehouse_id \
               FROM registry.engine_catalog_attachments a \
               JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = ANY($1) AND c.warehouse_id IS NOT NULL \
              ORDER BY a.is_primary DESC, a.created_at, c.name",
        )
        .bind(engine_ids)
        .fetch_all(self.pool())
        .await?;
        let mut out: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (engine_id, warehouse_id) in rows {
            let ws = out.entry(engine_id).or_default();
            // Two catalogs of the same warehouse on one engine is legal
            // (different names, different configs); the picker wants the
            // warehouse once.
            if !ws.contains(&warehouse_id) {
                ws.push(warehouse_id);
            }
        }
        Ok(out)
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

    /// One attachment converged (SP3b). `mark_engine_applied` stays as it is:
    /// it is Trino's whole-engine apply, which settles every attachment at once
    /// and compares `catalogs.updated_at`; a StarRocks apply is per catalog.
    /// Returns `NotFound` when the row is gone (a race D2's
    /// `engine_still_current` checkpoints exist precisely because it's real:
    /// the engine, or just this attachment, was deleted mid-bootstrap).
    /// `Ok(())` on zero rows would let Task 5's bootstrap record a
    /// convergence success for an attachment that no longer exists to
    /// converge (fix round 1 review, F-12).
    pub async fn set_apply_applied(
        &self,
        engine_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<(), RegistryError> {
        let result = sqlx::query(
            "UPDATE registry.engine_catalog_attachments \
                SET apply_status = 'applied', apply_error = NULL, applied_at = now() \
              WHERE engine_id = $1 AND catalog_id = $2",
        )
        .bind(engine_id)
        .bind(catalog_id)
        .execute(self.pool())
        .await?;
        if result.rows_affected() == 0 {
            return Err(RegistryError::NotFound(format!(
                "attachment engine={engine_id} catalog={catalog_id}"
            )));
        }
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

    /// Replace the full assignment set. This is the ONLY way to write
    /// `registry.catalog_workspaces` in this crate — deliberately, since
    /// SP4a Task 6 fix round 2 found two production paths writing this table
    /// (this and `ensure_warehouse_catalog`), but only one of them syncing
    /// the `perm` implicit-grant row D3 requires: a self-committing,
    /// pool-opening `set_catalog_workspaces` wrapper existed alongside this
    /// `_in_tx` core (used only by `ensure_warehouse_catalog` and the real
    /// production caller, `egret-engine-mgmt`'s assignment handler), and
    /// nothing stopped a THIRD caller from reaching for the convenient
    /// wrapper and reintroducing the same gap with no grant sync at all —
    /// exactly the "true in one place, false in another" shape D3 already
    /// broke twice. The wrapper is gone; every caller (including this
    /// crate's own tests, `egret-catalog-engine`'s and
    /// `egret-engine-mgmt`'s) must now open its own transaction and, in
    /// production, compose `egret_perm::sync_assignment_grants_in_tx` into
    /// it — there is no entry point left that writes this table without a
    /// transaction the caller controls. "Unrepresentable beats
    /// unforgettable": the invariant now lives in the type signature
    /// (no owned-pool method exists to reach for) rather than in a
    /// reviewer's memory of which callers need the sync.
    pub async fn set_catalog_workspaces_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: Uuid,
        workspace_ids: &[Uuid],
    ) -> Result<(), RegistryError> {
        let ids = workspace_ids.to_vec();
        sqlx::query("DELETE FROM registry.catalog_workspaces WHERE catalog_id = $1 AND NOT (workspace_id = ANY($2))")
            .bind(catalog_id)
            .bind(ids.clone())
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) \
             SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
        )
        .bind(catalog_id)
        .bind(ids)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// The §4.6 "assigned OR owned-and-not-yet-assigned" visibility rule, as
    /// one executable predicate: a workspace sees a warehouse when its registry
    /// catalog is assigned there — or, for a warehouse with no registry catalog
    /// yet (`ensure_warehouse_catalog` has not run for it, or a past attempt
    /// failed), when the warehouse is that workspace's own.
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
}
