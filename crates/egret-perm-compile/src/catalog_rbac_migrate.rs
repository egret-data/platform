//! SP5b Task 2: the reversible fold of one embedded catalog's RBAC
//! (`catalog.catalog_roles` + `catalog_grants`) into `perm.grants`.
//!
//! A catalog role is bound to the service accounts that own its clients
//! (`catalog_clients.service_account_id`). Each of the role's grants is mapped
//! with [`egret_perm::catalog_map`] and written as an `allow` grant whose
//! principals are those service accounts, as `kind='user'` principals named by
//! `users.username` (the only identity an engine sees; see `principal_refs`).
//! A role with no bound service account contributes nothing and is reported in
//! [`FoldReport::skipped_roles`]. Revoked clients are not counted.
//!
//! # Reversible
//!
//! One provenance row per catalog (`perm.catalog_rbac_migrated`, migration
//! 0234). Its `source_hash` is the SHA-256 of the canonical roles+grants of the
//! catalog's warehouse. [`undo_fold`] refuses with [`FoldError::SourceChanged`]
//! if the live RBAC no longer hashes to it, deletes exactly the recorded
//! `created_*` grants, then the provenance row. The source tables are never
//! written by either function.
//!
//! This task is additive and does not recompile; enforcement reads are switched
//! in later SP5b tasks. The shared `engine` role folds like any other role here
//! and is reconciled in Task 4.

use egret_perm::catalog_map::{map_privilege, map_securable, MapError};
use egret_perm::model::{
    Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm::store::{create_grant_in_tx, delete_grant_in_tx};
use serde::Serialize;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::migrate::canonical_json;

#[derive(Debug, thiserror::Error)]
pub enum FoldError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("perm error: {0}")]
    Perm(#[from] egret_perm::PermError),
    #[error("cannot map catalog grant {grant_id}: {source}")]
    Map { grant_id: Uuid, source: MapError },
    #[error("catalog grant {0} is malformed (empty namespace or table without a name)")]
    Malformed(Uuid),
    #[error("catalog {0} has no embedded warehouse")]
    NoWarehouse(Uuid),
    #[error("catalog {0} has not been folded")]
    NotFolded(Uuid),
    #[error("catalog RBAC changed since the fold; refusing to undo")]
    SourceChanged,
    #[error("could not resolve a username for service account {0}: {1}")]
    Username(Uuid, String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoldReport {
    pub catalog_id: Uuid,
    pub warehouse_id: Uuid,
    pub source_hash: String,
    pub created_grant_ids: Vec<Uuid>,
    /// Names of roles with no bound service account (nothing was folded for them).
    pub skipped_roles: Vec<String>,
    /// `true` when the catalog had already been folded and this is the stored report.
    pub already_folded: bool,
}

#[derive(sqlx::FromRow)]
struct RoleRow {
    id: Uuid,
    name: String,
    consumer_type: String,
}

#[derive(sqlx::FromRow)]
struct GrantRow {
    id: Uuid,
    catalog_role_id: Uuid,
    securable_type: String,
    namespace: Option<Vec<String>>,
    table_name: Option<String>,
    privilege: String,
}

struct Source {
    roles: Vec<RoleRow>,
    grants: Vec<GrantRow>,
    hash: String,
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The roles and grants of a warehouse, id-ordered, with their canonical hash.
async fn load_source(c: &mut PgConnection, warehouse_id: Uuid) -> Result<Source, sqlx::Error> {
    let roles: Vec<RoleRow> = sqlx::query_as(
        "SELECT id, name, consumer_type FROM catalog.catalog_roles \
          WHERE warehouse_id = $1 ORDER BY id",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *c)
    .await?;
    let grants: Vec<GrantRow> = sqlx::query_as(
        "SELECT g.id, g.catalog_role_id, g.securable_type, g.namespace, g.table_name, g.privilege \
           FROM catalog.catalog_grants g \
           JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
          WHERE r.warehouse_id = $1 ORDER BY g.id",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *c)
    .await?;
    let v = serde_json::json!({
        "roles": roles.iter().map(|r| serde_json::json!({
            "id": r.id, "name": r.name, "consumer_type": r.consumer_type,
        })).collect::<Vec<_>>(),
        "grants": grants.iter().map(|g| serde_json::json!({
            "id": g.id, "role": g.catalog_role_id, "securable": g.securable_type,
            "namespace": g.namespace, "table": g.table_name, "privilege": g.privilege,
        })).collect::<Vec<_>>(),
    });
    Ok(Source {
        roles,
        grants,
        hash: sha256_hex(&canonical_json(&v)),
    })
}

async fn warehouse_of(c: &mut PgConnection, catalog_id: Uuid) -> Result<Uuid, FoldError> {
    let w: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *c)
            .await?;
    w.flatten().ok_or(FoldError::NoWarehouse(catalog_id))
}

#[derive(sqlx::FromRow)]
struct Provenance {
    warehouse_id: Uuid,
    source_hash: String,
    created_grant_ids: Vec<Uuid>,
    skipped_role_names: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct UndoRow {
    warehouse_id: Uuid,
    source_hash: String,
    created_grant_ids: Vec<Uuid>,
    created_owner_ids: Vec<Uuid>,
    created_engine_grant_ids: Vec<Uuid>,
}

/// Fold `catalog_id`'s RBAC into `perm.grants` in ONE transaction. Idempotent:
/// an already-folded catalog returns its stored report and writes nothing.
pub async fn fold_catalog(
    c: &mut PgConnection,
    catalog_id: Uuid,
    actor: &str,
) -> Result<FoldReport, FoldError> {
    let mut tx = c.begin().await?;
    let warehouse_id = warehouse_of(&mut tx, catalog_id).await?;
    // Serialise concurrent folds of one catalog on the registry row.
    sqlx::query("SELECT 1 FROM registry.catalogs WHERE id = $1 FOR UPDATE")
        .bind(catalog_id)
        .execute(&mut *tx)
        .await?;

    let existing: Option<Provenance> = sqlx::query_as(
        "SELECT warehouse_id, source_hash, created_grant_ids, skipped_role_names \
           FROM perm.catalog_rbac_migrated WHERE catalog_id = $1",
    )
    .bind(catalog_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(p) = existing {
        tx.commit().await?;
        return Ok(FoldReport {
            catalog_id,
            warehouse_id: p.warehouse_id,
            source_hash: p.source_hash,
            created_grant_ids: p.created_grant_ids,
            skipped_roles: p.skipped_role_names,
            already_folded: true,
        });
    }

    let src = load_source(&mut tx, warehouse_id).await?;
    let mut created = Vec::new();
    let mut skipped = Vec::new();
    for role in &src.roles {
        let sa_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT service_account_id FROM catalog.catalog_clients \
              WHERE catalog_role_id = $1 AND service_account_id IS NOT NULL \
                AND revoked_at IS NULL ORDER BY 1",
        )
        .bind(role.id)
        .fetch_all(&mut *tx)
        .await?;
        if sa_ids.is_empty() {
            skipped.push(role.name.clone());
            continue;
        }
        let mut principals = Vec::new();
        for sa in sa_ids {
            let username = egret_auth::users::ensure_username_via(&mut tx, egret_core::UserId(sa))
                .await
                .map_err(|e| FoldError::Username(sa, e.to_string()))?;
            let p = Principal {
                kind: PrincipalKind::User,
                r#ref: username,
            };
            if !principals.contains(&p) {
                principals.push(p);
            }
        }
        for g in src.grants.iter().filter(|g| g.catalog_role_id == role.id) {
            if g.securable_type == "namespace" && g.namespace.as_ref().is_none_or(|n| n.is_empty())
                || g.securable_type == "table" && g.table_name.is_none()
            {
                return Err(FoldError::Malformed(g.id));
            }
            let map_err = |source| FoldError::Map {
                grant_id: g.id,
                source,
            };
            let scope = map_securable(
                &g.securable_type,
                g.namespace.as_deref(),
                g.table_name.as_deref(),
            )
            .map_err(map_err)?;
            let actions = map_privilege(&g.privilege).map_err(map_err)?;
            let id = create_grant_in_tx(
                &mut tx,
                &Grant {
                    id: Uuid::nil(),
                    effect: Effect::Allow,
                    actions,
                    scope: Scope {
                        catalog_id,
                        schema: scope.schema_name,
                        table: scope.table_name,
                        column: None,
                    },
                    principals: PrincipalSet {
                        include: principals.clone(),
                        exclude: Vec::new(),
                    },
                    origin: GrantOrigin::Explicit,
                },
            )
            .await?;
            created.push(id);
        }
    }

    let role_ids: Vec<Uuid> = src.roles.iter().map(|r| r.id).collect();
    let grant_ids: Vec<Uuid> = src.grants.iter().map(|g| g.id).collect();
    sqlx::query(
        "INSERT INTO perm.catalog_rbac_migrated \
           (catalog_id, warehouse_id, source_role_ids, source_grant_ids, source_hash, \
            created_grant_ids, skipped_role_names, migrated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(catalog_id)
    .bind(warehouse_id)
    .bind(&role_ids)
    .bind(&grant_ids)
    .bind(&src.hash)
    .bind(&created)
    .bind(&skipped)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(FoldReport {
        catalog_id,
        warehouse_id,
        source_hash: src.hash,
        created_grant_ids: created,
        skipped_roles: skipped,
        already_folded: false,
    })
}

/// Reverse [`fold_catalog`]: delete exactly the recorded `created_*` grants and
/// the provenance row, refusing with [`FoldError::SourceChanged`] (writing
/// nothing) if the live catalog RBAC no longer hashes to the recorded value.
pub async fn undo_fold(c: &mut PgConnection, catalog_id: Uuid) -> Result<(), FoldError> {
    let mut tx = c.begin().await?;
    let prov: Option<UndoRow> = sqlx::query_as(
        "SELECT warehouse_id, source_hash, created_grant_ids, created_owner_ids, \
                created_engine_grant_ids \
           FROM perm.catalog_rbac_migrated WHERE catalog_id = $1 FOR UPDATE",
    )
    .bind(catalog_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(UndoRow {
        warehouse_id,
        source_hash: hash,
        created_grant_ids: grants,
        created_owner_ids: owners,
        created_engine_grant_ids: engine,
    }) = prov
    else {
        return Err(FoldError::NotFolded(catalog_id));
    };
    if load_source(&mut tx, warehouse_id).await?.hash != hash {
        return Err(FoldError::SourceChanged);
    }
    for id in grants.iter().chain(&owners).chain(&engine) {
        delete_grant_in_tx(&mut tx, *id).await?;
    }
    sqlx::query("DELETE FROM perm.catalog_rbac_migrated WHERE catalog_id = $1")
        .bind(catalog_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    struct Fx {
        pool: PgPool,
        catalog_id: Uuid,
        wh: Uuid,
        role_a: Uuid,
        grant_a1: Uuid,
        sa1: Uuid,
        sa2: Uuid,
    }

    async fn user(pool: &PgPool, tag: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, username) \
             VALUES ('local', $1, $1, $2) RETURNING id",
        )
        .bind(format!("{tag}@service.invalid"))
        .bind(tag)
        .fetch_one(pool)
        .await
        .expect("user")
    }

    async fn role(pool: &PgPool, wh: Uuid, name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
             VALUES ($1, $2, 'external') RETURNING id",
        )
        .bind(wh)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("role")
    }

    async fn grant(
        pool: &PgPool,
        role: Uuid,
        sec: &str,
        ns: Option<Vec<&str>>,
        table: Option<&str>,
        privilege: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO catalog.catalog_grants \
               (catalog_role_id, securable_type, namespace, table_name, privilege) \
             VALUES ($1,$2,$3,$4,$5) RETURNING id",
        )
        .bind(role)
        .bind(sec)
        .bind(ns)
        .bind(table)
        .bind(privilege)
        .fetch_one(pool)
        .await
        .expect("grant")
    }

    async fn client(pool: &PgPool, wh: Uuid, role: Uuid, sa: Uuid, ws: Uuid) {
        sqlx::query(
            "INSERT INTO catalog.catalog_clients \
               (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, \
                client_secret_hash, service_account_id) \
             VALUES ($1, 'external', $2, $3, $4, $5, 'h', $6)",
        )
        .bind(format!("c-{}", Uuid::new_v4()))
        .bind(ws)
        .bind(wh)
        .bind(role)
        .bind(format!("cid-{}", Uuid::new_v4()))
        .bind(sa)
        .execute(pool)
        .await
        .expect("client");
    }

    /// Warehouse + registry catalog; role `a` (catalog read + table write) bound
    /// to two service accounts via two clients; role `b` (namespace manage) with
    /// no client. Returns None when no test DB is available and skipping is allowed.
    async fn fixture() -> Option<Fx> {
        let dsn = egret_test_db::url().await?;
        let db = egret_metastore::Db::connect(&dsn, 2)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        let pool = db.pool.clone();
        let wh = egret_test_db::seed_warehouse(&pool).await;
        let ws: Uuid =
            sqlx::query_scalar("SELECT workspace_id FROM catalog.warehouses WHERE id=$1")
                .bind(wh)
                .fetch_one(&pool)
                .await
                .expect("ws");
        let sc: Uuid =
            sqlx::query_scalar("SELECT storage_conn_id FROM catalog.warehouses WHERE id=$1")
                .bind(wh)
                .fetch_one(&pool)
                .await
                .expect("sc");
        let tag = Uuid::new_v4().simple().to_string();
        let catalog_id: Uuid = sqlx::query_scalar(
            "INSERT INTO registry.catalogs (name, kind, config, warehouse_id, storage_connection_id) \
             VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                jsonb_build_object('warehouse_id', $2::text), \
                'storage_connection_id', $3::text), $2, $3) RETURNING id",
        )
        .bind(format!("c{}", &tag[..12]))
        .bind(wh)
        .bind(sc)
        .fetch_one(&pool)
        .await
        .expect("registry catalog");
        let sa1 = user(&pool, &format!("sa1-{}", &tag[..10])).await;
        let sa2 = user(&pool, &format!("sa2-{}", &tag[..10])).await;
        for sa in [sa1, sa2] {
            sqlx::query(
                "INSERT INTO service_accounts (user_id, scope, workspace_id, name) \
                 VALUES ($1,'workspace',$2,$3)",
            )
            .bind(sa)
            .bind(ws)
            .bind(format!("n-{sa}"))
            .execute(&pool)
            .await
            .expect("sa");
        }
        let role_a = role(&pool, wh, "a").await;
        let grant_a1 = grant(&pool, role_a, "catalog", None, None, "read").await;
        grant(
            &pool,
            role_a,
            "table",
            Some(vec!["sales", "eu"]),
            Some("orders"),
            "write",
        )
        .await;
        client(&pool, wh, role_a, sa1, ws).await;
        client(&pool, wh, role_a, sa2, ws).await;
        let role_b = role(&pool, wh, "b").await;
        grant(&pool, role_b, "namespace", Some(vec!["hr"]), None, "manage").await;
        Some(Fx {
            pool,
            catalog_id,
            wh,
            role_a,
            grant_a1,
            sa1,
            sa2,
        })
    }

    async fn rbac_snapshot(pool: &PgPool, wh: Uuid) -> String {
        let mut c = pool.acquire().await.expect("conn");
        load_source(&mut c, wh).await.expect("src").hash
    }

    async fn perm_grant_count(pool: &PgPool, catalog_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
            .bind(catalog_id)
            .fetch_one(pool)
            .await
            .expect("count")
    }

    #[tokio::test]
    async fn fold_writes_mapped_grants_for_bound_service_accounts() {
        let Some(f) = fixture().await else { return };
        let mut c = f.pool.acquire().await.unwrap();
        let r = fold_catalog(&mut c, f.catalog_id, "tester")
            .await
            .expect("fold");
        assert!(!r.already_folded);
        assert_eq!(r.created_grant_ids.len(), 2, "role a's two grants");
        assert_eq!(r.skipped_roles, vec!["b".to_string()]);
        type PermRow = (Uuid, Vec<String>, Option<String>, Option<String>);
        let rows: Vec<PermRow> = sqlx::query_as(
            "SELECT id, actions, schema_name, table_name FROM perm.grants \
              WHERE catalog_id = $1 ORDER BY schema_name NULLS FIRST",
        )
        .bind(f.catalog_id)
        .fetch_all(&f.pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, vec!["read"]);
        assert_eq!((rows[0].2.clone(), rows[0].3.clone()), (None, None));
        assert_eq!(rows[1].1, vec!["read", "write"]);
        assert_eq!(rows[1].2.as_deref(), Some("sales.eu"));
        assert_eq!(rows[1].3.as_deref(), Some("orders"));
        let mut want: Vec<String> =
            sqlx::query_scalar("SELECT username::text FROM users WHERE id = ANY($1) ORDER BY 1")
                .bind(vec![f.sa1, f.sa2])
                .fetch_all(&f.pool)
                .await
                .unwrap();
        want.sort();
        for (id, ..) in &rows {
            let got: Vec<(String, String)> = sqlx::query_as(
                "SELECT kind::text, ref FROM perm.grant_principals \
                  WHERE grant_id = $1 AND mode = 'include' ORDER BY ref",
            )
            .bind(id)
            .fetch_all(&f.pool)
            .await
            .unwrap();
            assert!(got.iter().all(|(k, _)| k == "user"));
            assert_eq!(got.into_iter().map(|(_, r)| r).collect::<Vec<_>>(), want);
        }
        let prov: i64 = sqlx::query_scalar(
            "SELECT cardinality(created_grant_ids)::bigint FROM perm.catalog_rbac_migrated WHERE catalog_id=$1",
        )
        .bind(f.catalog_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(prov, 2);
    }

    #[tokio::test]
    async fn refolding_is_a_no_op() {
        let Some(f) = fixture().await else { return };
        let mut c = f.pool.acquire().await.unwrap();
        let first = fold_catalog(&mut c, f.catalog_id, "t").await.unwrap();
        let second = fold_catalog(&mut c, f.catalog_id, "t").await.unwrap();
        assert!(second.already_folded);
        assert_eq!(first.created_grant_ids, second.created_grant_ids);
        assert_eq!(first.skipped_roles, second.skipped_roles);
        assert_eq!(perm_grant_count(&f.pool, f.catalog_id).await, 2);
    }

    #[tokio::test]
    async fn undo_removes_only_created_rows_and_leaves_the_source_untouched() {
        let Some(f) = fixture().await else { return };
        let before = rbac_snapshot(&f.pool, f.wh).await;
        // A pre-existing, unrelated perm grant on the same catalog must survive.
        let mut c = f.pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO perm.grants (effect, actions, catalog_id) \
             VALUES ('allow', ARRAY['read'], $1)",
        )
        .bind(f.catalog_id)
        .execute(&f.pool)
        .await
        .unwrap();
        fold_catalog(&mut c, f.catalog_id, "t").await.unwrap();
        assert_eq!(perm_grant_count(&f.pool, f.catalog_id).await, 3);
        assert_eq!(rbac_snapshot(&f.pool, f.wh).await, before);
        undo_fold(&mut c, f.catalog_id).await.expect("undo");
        assert_eq!(perm_grant_count(&f.pool, f.catalog_id).await, 1);
        let prov: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM perm.catalog_rbac_migrated WHERE catalog_id=$1",
        )
        .bind(f.catalog_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(prov, 0);
        assert_eq!(rbac_snapshot(&f.pool, f.wh).await, before);
        assert!(matches!(
            undo_fold(&mut c, f.catalog_id).await,
            Err(FoldError::NotFolded(_))
        ));
    }

    #[tokio::test]
    async fn undo_refuses_after_a_source_grant_changed() {
        let Some(f) = fixture().await else { return };
        let mut c = f.pool.acquire().await.unwrap();
        fold_catalog(&mut c, f.catalog_id, "t").await.unwrap();
        sqlx::query("UPDATE catalog.catalog_grants SET privilege = 'manage' WHERE id = $1")
            .bind(f.grant_a1)
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            undo_fold(&mut c, f.catalog_id).await,
            Err(FoldError::SourceChanged)
        ));
        assert_eq!(perm_grant_count(&f.pool, f.catalog_id).await, 2);
        let _ = f.role_a;
    }
}
