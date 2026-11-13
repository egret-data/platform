//! SP3c-1 review Fix 1 (sibling live path): the bulk platform-role replace flow
//! `egret_workspace::rbac::Authorizer::replace_platform_roles` — the live admin
//! route `PUT /api/admin/users/{id}/platform-roles` — must re-sync a role's
//! managed `role:<name> holders` group BEFORE it recompiles. Otherwise, after the
//! role-principal migration, revoking a platform role through this path recompiles
//! the group-ref grant from STALE `user_groups` and the ex-holder keeps access
//! until the next boot repair — a fail-OPEN regression on a path that was
//! fail-CLOSED before the migration.
//!
//! DB-gated; self-skips without `DATABASE_URL` only under `EGRET_ALLOW_DB_SKIP`.
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-workspace --test platform_role_recompile

use egret_core::UserId;
use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::role_holder_group::role_holder_group_name;
use egret_perm_compile::role_principal_migrate::migrate_role_principals_in_tx;
use egret_policy::PolicyStore;
use egret_workspace::rbac::Authorizer;
use serde_json::Value;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    service_id: i64,
    role: String,
    carol: UserId,
    username: String,
}

/// A StarRocks profile (one bound engine, one attached Iceberg catalog) with an
/// explicit read grant whose only principal is `kind='role'` naming a fresh
/// platform role that one user (carol, with a username) holds at platform scope.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_prr"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("prr-ws"))
            .bind("Platform Role Recompile WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-prr-test','AKIATEST') RETURNING id",
    )
    .bind(unique("prr-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("prr_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("prr-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("prr_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    // A fresh platform role carol holds at platform scope.
    let role = unique("prr_role");
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (id, scope, name) VALUES (gen_random_uuid(),'platform',$1) RETURNING id",
    )
    .bind(&role)
    .fetch_one(pool)
    .await
    .expect("role");
    let username = unique("prr-carol");
    let carol: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local',$1,'Carol',$2) RETURNING id",
    )
    .bind(format!("{username}@egret.local"))
    .bind(&username)
    .fetch_one(pool)
    .await
    .expect("carol");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user',$1,$2,'platform',NULL)",
    )
    .bind(carol)
    .bind(role_id)
    .execute(pool)
    .await
    .expect("grant platform role to carol");

    let perm = PermStore::new(pool.clone());
    let grant = Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::Role,
                r#ref: role.clone(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    perm.create_grant(&grant).await.expect("create role grant");

    Fixture {
        service_id: service.id,
        role,
        carol: UserId(carol),
        username,
    }
}

async fn group_has_user(pool: &sqlx::PgPool, group_name: &str, user: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.user_groups ug \
         JOIN public.groups g ON g.id = ug.group_id \
         WHERE g.name = $1 AND ug.user_id = $2)",
    )
    .bind(group_name)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Whether any of the service's committed compiled policies names `username`.
async fn service_grants_user(pool: &sqlx::PgPool, service_id: i64, username: &str) -> bool {
    let bodies: Vec<Value> =
        sqlx::query_scalar("SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1")
            .bind(service_id)
            .fetch_all(pool)
            .await
            .unwrap();
    bodies.iter().any(|b| b.to_string().contains(username))
}

#[tokio::test]
async fn revoking_a_platform_role_via_replace_resyncs_and_recompiles_the_holder_group() {
    let Some(db) = test_db().await else {
        return;
    };
    let f = seed(&db).await;
    let pool = db.pool.clone();

    // Migrate: the role grant now names the managed `role:<role> holders` group.
    let mut tx = pool.begin().await.unwrap();
    migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();

    let gname = role_holder_group_name(&f.role);
    assert!(
        group_has_user(&pool, &gname, f.carol.0).await,
        "carol must be in the holder group right after the migration"
    );
    assert!(
        service_grants_user(&pool, f.service_id, &f.username).await,
        "the migrated group-ref grant must still grant carol before the revoke"
    );

    // REVOKE via the LIVE bulk admin path (replace carol's platform roles with none).
    Authorizer::new(db.clone())
        .replace_platform_roles(f.carol, &[])
        .await
        .expect("replace_platform_roles");

    // Fail-CLOSED: carol is dropped from the managed holder group AND the committed
    // compiled artifact no longer names her — in the same path, without a restart.
    assert!(
        !group_has_user(&pool, &gname, f.carol.0).await,
        "the bulk replace must drop carol from the managed holder group"
    );
    assert!(
        !service_grants_user(&pool, f.service_id, &f.username).await,
        "the bulk replace must recompile the group-ref grant so carol is no longer granted"
    );
}
