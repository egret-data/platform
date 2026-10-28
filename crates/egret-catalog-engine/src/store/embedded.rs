//! Embedded-catalog RBAC store: warehouses (per workspace/domain), catalog-roles
//! (keyed by consumer-type), grants, and per-consumer Catalog Clients. egret owns
//! this store; the mini-IdP's [`ClientVerifier`] + the lakekeeper Authorizer (EB-3)
//! read from it.

use crate::store::CatalogError;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::Rng as _;
use uuid::Uuid;

use super::handle::CatalogStore;

// ── warehouses ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Warehouse {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    pub storage_conn_id: Uuid,
    pub lakekeeper_warehouse_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ── roles + grants ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogRole {
    pub id: Uuid,
    pub warehouse_id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogGrant {
    pub id: Uuid,
    pub catalog_role_id: Uuid,
    pub securable_type: String,
    pub namespace: Option<Vec<String>>,
    pub table_name: Option<String>,
    pub privilege: String,
}

pub struct GrantInput {
    pub securable_type: String,
    pub namespace: Option<Vec<String>>,
    pub table_name: Option<String>,
    pub privilege: String,
}

// ── clients ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogClient {
    pub id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
    pub client_id: String,
    pub created_at: DateTime<Utc>,
    /// Last full secret verification. The IdP serves repeats from a
    /// verification cache, so this advances at most about once per 10 minutes
    /// per (client, secret) — not on every token request. An RFC 8693 token
    /// exchange never verifies a secret, so a client that only refreshes by
    /// exchange (Iceberg's catalog client, after its first token) does not
    /// advance it: it can read hours old for a client in constant use.
    pub last_used_at: Option<DateTime<Utc>>,
}

pub struct NewClient {
    pub name: String,
    pub consumer_type: String,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
}

/// The one-time create result: the generated `client_id` + plaintext `client_secret`.
pub struct CreatedClient {
    pub id: Uuid,
    pub client_id: String,
    /// Shown ONCE; only the argon2 hash is persisted.
    pub client_secret: String,
}

impl CatalogStore {
    // warehouses ---------------------------------------------------------------

    pub async fn create_warehouse(
        &self,
        name: &str,
        workspace_id: Uuid,
        storage_conn_id: Uuid,
    ) -> Result<Warehouse, CatalogError> {
        sqlx::query_as::<_, Warehouse>(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) \
             RETURNING id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at",
        )
        .bind(name)
        .bind(workspace_id)
        .bind(storage_conn_id)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("warehouses_workspace_id_name_key") => {
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn set_lakekeeper_warehouse_id(
        &self,
        id: Uuid,
        lk_id: &str,
    ) -> Result<(), CatalogError> {
        sqlx::query("UPDATE catalog.warehouses SET lakekeeper_warehouse_id = $2 WHERE id = $1")
            .bind(id)
            .bind(lk_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn list_warehouses(&self) -> Result<Vec<Warehouse>, CatalogError> {
        Ok(sqlx::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM catalog.warehouses ORDER BY name",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_warehouse(&self, id: Uuid) -> Result<Option<Warehouse>, CatalogError> {
        Ok(sqlx::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM catalog.warehouses WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn get_warehouse_by_name(
        &self,
        name: &str,
    ) -> Result<Option<Warehouse>, CatalogError> {
        Ok(sqlx::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM catalog.warehouses WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_warehouse(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog.warehouses WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Delete a warehouse, releasing its registry catalog's workspace
    /// assignments and letting the catalog go with it (FK cascade) — unless an
    /// engine still attaches the catalog, in which case nothing is mutated and
    /// this returns `CatalogError::Conflict` naming the attached engine(s).
    ///
    /// Runs in ONE transaction (SP3 catalog-registry review M2/M3):
    /// `registry.catalog_workspaces.catalog_id` is NO ACTION, not cascaded from
    /// `registry.catalogs` (migration 0147 §decision 5), so the assignments
    /// must be released before the warehouse row goes or the delete hits a raw
    /// FK 500 — and checking attachments in a SEPARATE statement before the
    /// transaction let a race land between the check and the delete (a
    /// concurrent attach lands, then the delete hits the attachments FK — also
    /// NO ACTION — as a raw 500, not this 409). `SELECT … FOR UPDATE` locks the
    /// catalog row before the attachment check, so a racing attach either
    /// commits first (and is then seen and 409s here) or blocks until this
    /// transaction ends (and then finds the catalog gone). A warehouse with no
    /// registry catalog (`ensure_warehouse_catalog` has not run for it, or a
    /// past attempt failed) falls straight through to the plain delete.
    ///
    /// SP3c-2 Task 6: `engines.warehouse_id` — and the `engines_warehouse_id_fkey`
    /// FK (migration 0056) this function used to also catch as a second,
    /// belt-and-suspenders 409 — are both gone. Be precise about what that
    /// cost, because an earlier draft of this comment was not: the FK fired
    /// whenever ANY engine still named this warehouse in the column, including
    /// when a registry catalog existed but that engine held no attachment to
    /// it. It guarded a strictly LARGER set than the attachment check above.
    ///
    /// Dropping it is safe anyway, and not because the two sets coincide. The
    /// column WAS the dependency it was guarding: an engine that holds no
    /// attachment now holds no reference to this warehouse through any column,
    /// FK or table, so there is nothing left for the delete to dangle. An
    /// engine that DOES hold an attachment is still caught above — and it is
    /// caught completely, because an attachment cannot exist without a
    /// `registry.catalogs` row for this warehouse (`catalogs_warehouse_id_key`
    /// is unique, and `engine_catalog_attachments.catalog_id` FKs to it), so
    /// "no registry catalog" implies "no attachment, from any engine".
    /// See `warehouse_with_no_registry_catalog_deletes_cleanly_even_with_other_engines_present`
    /// (catalog-engine `warehouse_admin_routes` tests), which replaced the old
    /// FK-fallback test.
    pub async fn delete_warehouse_releasing_catalog(&self, id: Uuid) -> Result<(), CatalogError> {
        self.delete_warehouse_releasing_catalog_with_compiler(id, None)
            .await
    }

    /// [`Self::delete_warehouse_releasing_catalog`], plus sweeping any
    /// compiled policy this catalog's (about to be cascade-deleted) grants
    /// left behind (SP4a Task 16, D3 defect 3).
    ///
    /// **Why the affected set can't be "services this catalog reaches now."**
    /// The guard just above refuses the whole delete while any engine is
    /// still attached, so by the time this function does anything, ZERO
    /// engines reach this catalog — `egret_policy::services_for_catalog_via`
    /// would always answer empty here, exactly like
    /// `RegistryStore::ensure_warehouse_catalog_with_compiler`'s own no-op
    /// case. What is NOT guaranteed empty is `perm.compiled`: a profile that
    /// reached this catalog in the PAST may still carry a `perm-grant-*`
    /// policy for it if the detach that preceded this delete had its own
    /// recompile fail silently (`recompile_bound_profile` is fire-and-forget
    /// by contract). `egret_perm::compiled_service_ids_for_catalog_via` reads
    /// exactly that provenance, so the affected set here is sourced from it —
    /// captured BEFORE the deletes below, because `perm.grants` (and with it
    /// `perm.compiled`, both `ON DELETE CASCADE`) is gone the moment the
    /// warehouse row cascades away.
    pub async fn delete_warehouse_releasing_catalog_with_compiler(
        &self,
        id: Uuid,
        compiler: Option<&dyn egret_catalog_registry::CatalogAssignmentCompiler>,
    ) -> Result<(), CatalogError> {
        let mut tx = self.pool().begin().await?;
        let catalog: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, name FROM registry.catalogs WHERE warehouse_id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let mut affected_services: Vec<i64> = Vec::new();
        if let Some((catalog_id, catalog_name)) = catalog {
            let engine_names: Vec<String> = sqlx::query_scalar(
                "SELECT e.name FROM registry.engine_catalog_attachments a \
                   JOIN engine_mgmt.engines e ON e.id = a.engine_id \
                  WHERE a.catalog_id = $1 ORDER BY e.name",
            )
            .bind(catalog_id)
            .fetch_all(&mut *tx)
            .await?;
            if !engine_names.is_empty() {
                // Dropping `tx` here rolls back (nothing was mutated yet).
                return Err(CatalogError::Conflict(format!(
                    "the warehouse's catalog {} is attached to engine(s) {}; detach it first",
                    catalog_name,
                    engine_names.join(", ")
                )));
            }
            // Captured before the deletes below: `perm.grants`/`perm.compiled`
            // for this catalog are about to cascade away, and there is
            // nothing left to join against once they do.
            affected_services =
                egret_perm::compiled_service_ids_for_catalog_via(&mut tx, catalog_id).await?;
            affected_services.sort_unstable();
            affected_services.dedup();
            sqlx::query("DELETE FROM registry.catalog_workspaces WHERE catalog_id = $1")
                .bind(catalog_id)
                .execute(&mut *tx)
                .await?;
        }
        let result = sqlx::query("DELETE FROM catalog.warehouses WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await;
        match result {
            Ok(_) => {
                // Runs AFTER the cascade above so the recompile's own "desired
                // state" read no longer sees this catalog's (now-deleted)
                // grants — otherwise a compiled policy that is genuinely still
                // wanted elsewhere on the same profile would be indistinguishable
                // from the stale one this sweep exists to remove.
                if let Some(c) = compiler {
                    if !affected_services.is_empty() {
                        c.recompile_services_in_tx(&mut tx, &affected_services)
                            .await?;
                    }
                }
                tx.commit().await?;
                Ok(())
            }
            // SP3 catalog-registry review N4: our own `SELECT … FOR UPDATE`
            // above only locks a registry catalog that already exists when we
            // read it — it cannot lock the ABSENCE of one. If the warehouse had
            // no registry catalog yet, the SELECT returns nothing and we fall
            // straight through to this DELETE; a concurrent
            // `ensure_warehouse_catalog` — engine create, or an attach to this
            // warehouse — that creates the catalog (+ its `catalog_workspaces`
            // assignment, atomically, in its own transaction) between our
            // SELECT and this DELETE commits fine, and this DELETE then
            // cascades into deleting that brand-new catalog row — which its
            // `catalog_workspaces` row (NO ACTION) blocks. Losing that race is
            // rare but the failure mode without this arm is a raw 500, not a
            // 409, so it is mapped the same way.
            //
            // The boot backfill was this arm's original racer (review N4) and
            // is gone as of SP3c-2; the engine-create path races identically,
            // so the arm stays. (SP3c-2 Task 6: this used to be one of TWO
            // fallback arms alongside `engines_warehouse_id_fkey`; that other
            // arm is deleted along with the column and FK it caught — see the
            // function doc.)
            Err(sqlx::Error::Database(d))
                if d.constraint() == Some("catalog_workspaces_catalog_id_fkey") =>
            {
                Err(CatalogError::Conflict(
                    "the warehouse's registry catalog was just created and assigned to a \
                     workspace; retry the delete"
                        .into(),
                ))
            }
            Err(e) => Err(CatalogError::Db(e)),
        }
    }

    /// `RegistryStore::warehouse_visible_to_workspace` through the catalog
    /// seam's store. Like it, this has NO production caller today (review
    /// F-10): it is the executable form of the §4.6 assigned-or-owned rule
    /// that `egret-query` mirrors in SQL, held for SP4's per-workspace
    /// notebook role. It is not an authorization gate, and notebook warehouse
    /// selection deliberately does not use it — read that function's doc
    /// before calling this one.
    pub async fn warehouse_visible_to_workspace(
        &self,
        warehouse_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<bool, CatalogError> {
        self.registry()
            .warehouse_visible_to_workspace(warehouse_id, workspace_id)
            .await
            .map_err(|e| match e {
                egret_catalog_registry::RegistryError::Db(e) => CatalogError::Db(e),
                other => CatalogError::BadRequest(other.to_string()),
            })
    }

    // roles + grants -----------------------------------------------------------

    pub async fn create_role(
        &self,
        warehouse_id: Uuid,
        name: &str,
        consumer_type: &str,
        description: Option<&str>,
    ) -> Result<CatalogRole, CatalogError> {
        sqlx::query_as::<_, CatalogRole>(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type, description) VALUES ($1,$2,$3,$4) \
             RETURNING id, warehouse_id, name, consumer_type, description",
        )
        .bind(warehouse_id)
        .bind(name)
        .bind(consumer_type)
        .bind(description)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d)
                if d.constraint() == Some("catalog_roles_warehouse_id_name_key") =>
            {
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn list_roles(&self, warehouse_id: Uuid) -> Result<Vec<CatalogRole>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogRole>(
            "SELECT id, warehouse_id, name, consumer_type, description FROM catalog.catalog_roles \
             WHERE warehouse_id = $1 ORDER BY name",
        )
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Delete a role. Its grants cascade; blocked by FK if any Catalog Client
    /// still references it (surfaced as `CatalogError::InUse`).
    pub async fn delete_role(&self, role_id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog.catalog_roles WHERE id = $1")
            .bind(role_id)
            .execute(self.pool())
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_foreign_key_violation() => CatalogError::InUse,
                _ => CatalogError::Db(e),
            })?;
        Ok(())
    }

    pub async fn add_grant(
        &self,
        catalog_role_id: Uuid,
        g: &GrantInput,
    ) -> Result<CatalogGrant, CatalogError> {
        // Reject a duplicate of an identical grant (same securable + privilege).
        // NULL namespace/table are compared with IS NOT DISTINCT FROM so
        // catalog-level duplicates are caught too.
        let exists: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM catalog.catalog_grants \
             WHERE catalog_role_id = $1 AND securable_type = $2 \
               AND namespace IS NOT DISTINCT FROM $3 AND table_name IS NOT DISTINCT FROM $4 \
               AND privilege = $5",
        )
        .bind(catalog_role_id)
        .bind(&g.securable_type)
        .bind(g.namespace.as_deref())
        .bind(g.table_name.as_deref())
        .bind(&g.privilege)
        .fetch_optional(self.pool())
        .await?;
        if exists.is_some() {
            return Err(CatalogError::AlreadyExists("grant".to_string()));
        }
        Ok(sqlx::query_as::<_, CatalogGrant>(
            "INSERT INTO catalog.catalog_grants (catalog_role_id, securable_type, namespace, table_name, privilege) \
             VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, catalog_role_id, securable_type, namespace, table_name, privilege",
        )
        .bind(catalog_role_id)
        .bind(&g.securable_type)
        .bind(g.namespace.as_deref())
        .bind(g.table_name.as_deref())
        .bind(&g.privilege)
        .fetch_one(self.pool())
        .await?)
    }

    pub async fn list_grants(
        &self,
        catalog_role_id: Uuid,
    ) -> Result<Vec<CatalogGrant>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogGrant>(
            "SELECT id, catalog_role_id, securable_type, namespace, table_name, privilege \
             FROM catalog.catalog_grants WHERE catalog_role_id = $1",
        )
        .bind(catalog_role_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// All grants for a role identified by (warehouse name, role name) — the
    /// lookup the Authorizer uses from a JWT's `warehouse`+`roles` claims (EB-3).
    pub async fn grants_for_role(
        &self,
        warehouse_name: &str,
        role_name: &str,
    ) -> Result<Vec<CatalogGrant>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogGrant>(
            "SELECT g.id, g.catalog_role_id, g.securable_type, g.namespace, g.table_name, g.privilege \
             FROM catalog.catalog_grants g \
             JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
             JOIN catalog.warehouses w ON w.id = r.warehouse_id \
             WHERE w.name = $1 AND r.name = $2",
        )
        .bind(warehouse_name)
        .bind(role_name)
        .fetch_all(self.pool())
        .await?)
    }

    /// Which of `pairs` are equal the way lakekeeper compares namespace and
    /// table names: its ICU `case_insensitive` collation (`und-u-ks-level2`,
    /// non-deterministic), so case, width, compatibility forms, canonical
    /// equivalence and default-ignorable code points do not tell names apart,
    /// while accents do (rename-authz spec §10.4). Asked of Postgres itself, so
    /// the guard cannot drift from the collation lakekeeper's unique indexes
    /// use.
    pub async fn names_equal_in_catalog(
        &self,
        pairs: &[(String, String)],
    ) -> Result<std::collections::HashSet<(String, String)>, CatalogError> {
        if pairs.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let (left, right): (Vec<String>, Vec<String>) = pairs.iter().cloned().unzip();
        let equal: Vec<(String, String)> = sqlx::query_as(
            "SELECT a, b FROM unnest($1::text[], $2::text[]) AS p(a, b) \
             WHERE a = b COLLATE lakekeeper.case_insensitive",
        )
        .bind(&left)
        .bind(&right)
        .fetch_all(self.pool())
        .await?;
        Ok(equal.into_iter().collect())
    }

    pub async fn revoke_grant(&self, grant_id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog.catalog_grants WHERE id = $1")
            .bind(grant_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // clients ------------------------------------------------------------------

    /// Create a Catalog Client: generate a `client_id` + secret (argon2-hashed at
    /// rest), returning the plaintext secret ONCE.
    pub async fn create_client(&self, c: &NewClient) -> Result<CreatedClient, CatalogError> {
        // Generate all random material in a block so the `!Send` ThreadRng is
        // dropped BEFORE the sqlx await (otherwise the future is !Send and can't
        // be used as an axum handler).
        let (client_id, client_secret) = {
            let mut rng = rand::thread_rng();
            let client_id = format!("egret_{}", hex::encode(rng.gen::<[u8; 8]>()));
            let secret_bytes: [u8; 32] = rng.gen();
            (
                client_id,
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes),
            )
        };
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(client_secret.as_bytes(), &salt)
            .map_err(|e| CatalogError::BadRequest(format!("hash: {e}")))?
            .to_string();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.catalog_clients \
               (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, client_secret_hash) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id",
        )
        .bind(&c.name)
        .bind(&c.consumer_type)
        .bind(c.workspace_id)
        .bind(c.warehouse_id)
        .bind(c.catalog_role_id)
        .bind(&client_id)
        .bind(&hash)
        .fetch_one(self.pool())
        .await?;
        Ok(CreatedClient {
            id,
            client_id,
            client_secret,
        })
    }

    pub async fn list_clients(
        &self,
        warehouse_id: Uuid,
    ) -> Result<Vec<CatalogClient>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogClient>(
            "SELECT id, name, consumer_type, workspace_id, warehouse_id, catalog_role_id, \
                    client_id, created_at, last_used_at \
             FROM catalog.catalog_clients WHERE warehouse_id = $1 ORDER BY name",
        )
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn revoke_client(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog.catalog_clients WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// The row a Catalog Client mints from: `(id, client_secret_hash,
    /// warehouse name, role name)`. Read by both [`Self::verify_client`] and the
    /// `ClientVerifier::current` impl, so the secret path and the secret-free
    /// path cannot disagree about which clients exist.
    async fn client_row(
        &self,
        client_id: &str,
    ) -> Result<Option<(Uuid, String, String, String)>, CatalogError> {
        Ok(sqlx::query_as(
            "SELECT c.id, c.client_secret_hash, w.name, r.name \
             FROM catalog.catalog_clients c \
             JOIN catalog.warehouses w ON w.id = c.warehouse_id \
             JOIN catalog.catalog_roles r ON r.id = c.catalog_role_id \
             WHERE c.client_id = $1",
        )
        .bind(client_id)
        .fetch_optional(self.pool())
        .await?)
    }

    /// Verify a client's secret; on success return its warehouse name + role name
    /// (for the JWT claims), and touch `last_used_at`.
    pub async fn verify_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<(String, String)>, CatalogError> {
        Ok(self
            .verified_row(client_id, client_secret)
            .await?
            .map(|(_, warehouse, role)| (warehouse, role)))
    }

    /// The client's row — `(secret hash, warehouse name, role name)` — if the
    /// secret verifies against the hash in THAT row; touches `last_used_at`.
    ///
    /// Only a full verify touches `last_used_at`. The IdP serves a proven
    /// repeat of a secret from its verification cache without calling this, so
    /// the column advances at most about once per cache TTL (600 s) per
    /// (client, secret). A token exchange never calls this, so it does not
    /// advance the column at all.
    async fn verified_row(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<(String, String, String)>, CatalogError> {
        let Some((id, hash, warehouse, role)) = self.client_row(client_id).await? else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(&hash)
            .map_err(|e| CatalogError::BadRequest(format!("hash parse: {e}")))?;
        if Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        let _ =
            sqlx::query("UPDATE catalog.catalog_clients SET last_used_at = now() WHERE id = $1")
                .bind(id)
                .execute(self.pool())
                .await;
        Ok(Some((hash, warehouse, role)))
    }
}

// ── catalog grants follow a renamed table (grants-follow spec §5.5, D8) ──────

/// One `table` catalog grant a rename re-pointed, or merged into the same
/// grant already on the new name.
#[derive(Clone, Debug)]
pub struct MovedCatalogGrant {
    pub id: Uuid,
    pub role: String,
    pub privilege: String,
    pub before_namespace: Vec<String>,
    pub before_table: String,
    /// Deleted instead of re-pointed: the role already held the same grant on
    /// the new name (a union, D8).
    pub merged: bool,
}

/// Are two names one name in lakekeeper's catalog (its ICU
/// `case_insensitive` collation, as the guard compares them)? Element by
/// element for namespaces.
pub async fn catalog_names_equal_via(
    conn: &mut sqlx::PgConnection,
    a_ns: &[String],
    a: &str,
    b_ns: &[String],
    b: &str,
) -> Result<bool, sqlx::Error> {
    if a_ns.len() != b_ns.len() {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT $3 = $4 COLLATE lakekeeper.case_insensitive \
            AND NOT EXISTS (SELECT 1 FROM unnest($1::text[], $2::text[]) AS p(x, y) \
                             WHERE NOT (x = y COLLATE lakekeeper.case_insensitive))",
    )
    .bind(a_ns)
    .bind(b_ns)
    .bind(a)
    .bind(b)
    .fetch_one(&mut *conn)
    .await
}

/// The live table id of `ns.table` (any depth) in egret warehouse
/// `warehouse_id`, `None` when it is not a live table.
pub async fn live_table_id_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    ns: &[String],
    table: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT t.tabular_id FROM catalog.warehouses w \
           JOIN lakekeeper.tabular t ON t.warehouse_id::text = w.lakekeeper_warehouse_id \
          WHERE w.id = $1 AND t.typ = 'table' AND t.deleted_at IS NULL \
            AND t.metadata_location IS NOT NULL \
            AND cardinality(t.tabular_namespace_name) = cardinality($2::text[]) \
            AND NOT EXISTS (SELECT 1 FROM unnest(t.tabular_namespace_name, $2::text[]) AS p(x, y) \
                             WHERE NOT (x = y COLLATE lakekeeper.case_insensitive)) \
            AND t.name = $3 COLLATE lakekeeper.case_insensitive",
    )
    .bind(warehouse_id)
    .bind(ns)
    .bind(table)
    .fetch_optional(&mut *conn)
    .await
}

/// Re-point, on `conn` (the caller's transaction), every `table` catalog
/// grant of `warehouse_id`'s roles on `src_ns.src` to `dst_ns.dst`, the names
/// compared as the guard compares them (lakekeeper's collation, element by
/// element). A grant the role already holds on the destination (same
/// privilege) is deleted instead. `namespace` and `catalog` grants stay: they
/// name a location.
pub async fn move_table_grants_in_tx(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    src_ns: &[String],
    src: &str,
    dst_ns: &[String],
    dst: &str,
) -> Result<Vec<MovedCatalogGrant>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, String, String, Vec<String>, String)> = sqlx::query_as(
        "SELECT g.id, g.catalog_role_id, r.name, g.privilege, g.namespace, g.table_name \
           FROM catalog.catalog_grants g \
           JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
          WHERE r.warehouse_id = $1 AND g.securable_type = 'table' \
            AND g.namespace IS NOT NULL AND g.table_name IS NOT NULL \
            AND cardinality(g.namespace) = cardinality($2::text[]) \
            AND NOT EXISTS (SELECT 1 FROM unnest(g.namespace, $2::text[]) AS p(x, y) \
                             WHERE NOT (x = y COLLATE lakekeeper.case_insensitive)) \
            AND g.table_name = $3 COLLATE lakekeeper.case_insensitive \
          ORDER BY g.id \
            FOR UPDATE OF g",
    )
    .bind(warehouse_id)
    .bind(src_ns)
    .bind(src)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, role_id, role, privilege, namespace, table) in rows {
        let twin: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM catalog.catalog_grants g \
              WHERE g.catalog_role_id = $1 AND g.id <> $2 AND g.securable_type = 'table' \
                AND g.privilege = $3 AND g.namespace IS NOT NULL \
                AND cardinality(g.namespace) = cardinality($4::text[]) \
                AND NOT EXISTS (SELECT 1 FROM unnest(g.namespace, $4::text[]) AS p(x, y) \
                                 WHERE NOT (x = y COLLATE lakekeeper.case_insensitive)) \
                AND g.table_name = $5 COLLATE lakekeeper.case_insensitive)",
        )
        .bind(role_id)
        .bind(id)
        .bind(&privilege)
        .bind(dst_ns)
        .bind(dst)
        .fetch_one(&mut *conn)
        .await?;
        if twin {
            sqlx::query("DELETE FROM catalog.catalog_grants WHERE id = $1")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        } else {
            sqlx::query(
                "UPDATE catalog.catalog_grants SET namespace = $2, table_name = $3 WHERE id = $1",
            )
            .bind(id)
            .bind(dst_ns)
            .bind(dst)
            .execute(&mut *conn)
            .await?;
        }
        out.push(MovedCatalogGrant {
            id,
            role,
            privilege,
            before_namespace: namespace,
            before_table: table,
            merged: twin,
        });
    }
    Ok(out)
}

/// Delete, on `conn`, every `table` catalog grant of `warehouse_id`'s roles
/// in namespace `ns` or below it (`namespace[1]` equal under lakekeeper's
/// collation): a recursive drop of `ns` removed every table there for good
/// (grants-follow D15). `namespace` and `catalog` grants stay.
pub async fn delete_table_grants_under_in_tx(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    ns: &str,
) -> Result<Vec<MovedCatalogGrant>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String, Vec<String>, String)> = sqlx::query_as(
        "DELETE FROM catalog.catalog_grants g USING catalog.catalog_roles r \
          WHERE r.id = g.catalog_role_id AND r.warehouse_id = $1 \
            AND g.securable_type = 'table' AND g.table_name IS NOT NULL \
            AND cardinality(g.namespace) >= 1 \
            AND g.namespace[1] = $2 COLLATE lakekeeper.case_insensitive \
         RETURNING g.id, r.name, g.privilege, g.namespace, g.table_name",
    )
    .bind(warehouse_id)
    .bind(ns)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, role, privilege, namespace, table)| MovedCatalogGrant {
                id,
                role,
                privilege,
                before_namespace: namespace,
                before_table: table,
                merged: false,
            },
        )
        .collect())
}

/// The mini-IdP's client verifier, backed by the Catalog Client registry.
#[async_trait::async_trait]
impl egret_machine_idp::ClientVerifier for CatalogStore {
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<egret_machine_idp::ClientState>, egret_machine_idp::IdpError> {
        let row = self
            .verified_row(client_id, client_secret)
            .await
            .map_err(|e| egret_machine_idp::IdpError::Db(e.to_string()))?;
        // The row the Argon2 check used, whole: its hash is the one that
        // verified, which is what the IdP caches.
        Ok(row.map(
            |(secret_hash, warehouse, role)| egret_machine_idp::ClientState {
                verified: catalog_verified_client(client_id, warehouse, role),
                secret_hash,
            },
        ))
    }

    /// A Catalog Client has no refusal beyond existing: revoking one DELETEs
    /// its row. So the row read alone is the whole answer.
    async fn current(
        &self,
        client_id: &str,
    ) -> Result<Option<egret_machine_idp::ClientState>, egret_machine_idp::IdpError> {
        let row = self
            .client_row(client_id)
            .await
            .map_err(|e| egret_machine_idp::IdpError::Db(e.to_string()))?;
        Ok(row.map(
            |(_, secret_hash, warehouse, role)| egret_machine_idp::ClientState {
                verified: catalog_verified_client(client_id, warehouse, role),
                secret_hash,
            },
        ))
    }
}

/// The token shape a Catalog Client gets: `aud: catalog`, its warehouse + role,
/// and a 1h TTL. A published contract with every deployed Iceberg client — kept
/// as a pure function so it can be pinned by a unit test without a database.
fn catalog_verified_client(
    client_id: &str,
    warehouse: String,
    catalog_role: String,
) -> egret_machine_idp::VerifiedClient {
    egret_machine_idp::VerifiedClient {
        client_id: client_id.to_string(),
        audience: "catalog".into(),
        extra_claims: serde_json::json!({
            "warehouse": warehouse,
            "roles": [catalog_role],
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
        ttl_secs: 3600,
    }
}

#[cfg(test)]
mod tests {
    use super::catalog_verified_client;

    /// Pins the catalog half of the multi-audience IdP contract: the machine-IdP
    /// copies these straight into the JWT, so a change here changes every
    /// deployed Iceberg client's token.
    #[test]
    fn catalog_client_token_shape_is_fixed() {
        let vc = catalog_verified_client("cat1", "wh1".into(), "role1".into());
        assert_eq!(vc.client_id, "cat1");
        assert_eq!(vc.audience, "catalog");
        assert_eq!(vc.ttl_secs, 3600, "catalog TTL must stay 3600");
        assert_eq!(vc.extra_claims["warehouse"], "wh1");
        assert_eq!(vc.extra_claims["roles"], serde_json::json!(["role1"]));
    }
}
