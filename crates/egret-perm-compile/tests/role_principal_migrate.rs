//! SP3c-1: role principals -> managed `role:<name> holders` groups.

use egret_metastore::Db;
use uuid::Uuid;

#[tokio::test]
async fn a_managed_group_source_is_accepted() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let id = Uuid::new_v4();
    let r = sqlx::query("INSERT INTO public.groups (id, name, source) VALUES ($1,$2,'managed')")
        .bind(id)
        .bind(format!("role:data_steward holders {id}"))
        .execute(&db.pool)
        .await;
    assert!(r.is_ok(), "managed source must be accepted: {r:?}");
}

use egret_perm_compile::role_holder_group::{role_holder_group_name, sync_role_holder_group_in_tx};

async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, provider, email, name) VALUES ($1,'local',$2,'T')")
        .bind(id)
        .bind(format!("rh_{id}@example.test"))
        .execute(pool)
        .await
        .expect("insert user");
    id
}

async fn grant_role(pool: &sqlx::PgPool, role_id: Uuid, user: Uuid) {
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user',$1,$2,'platform')",
    )
    .bind(user)
    .bind(role_id)
    .execute(pool)
    .await
    .expect("insert grant");
}

/// A fresh platform role with one direct user holder: (role name, role id, user id).
async fn seed_role_with_one_user_holder(pool: &sqlx::PgPool) -> (String, Uuid, Uuid) {
    let role_id = Uuid::new_v4();
    let role = format!("rh_{}", &role_id.simple().to_string()[..12]);
    sqlx::query("INSERT INTO public.roles (id, scope, name) VALUES ($1,'platform',$2)")
        .bind(role_id)
        .bind(&role)
        .execute(pool)
        .await
        .expect("insert role");
    let u1 = seed_user(pool).await;
    grant_role(pool, role_id, u1).await;
    (role, role_id, u1)
}

/// Revoke `u1`'s grant and grant the role to a new user; returns the new user.
async fn revoke_u1_grant_u2(pool: &sqlx::PgPool, role_id: Uuid, u1: Uuid) -> Uuid {
    sqlx::query("DELETE FROM public.access_grants WHERE principal_id=$1 AND role_id=$2")
        .bind(u1)
        .bind(role_id)
        .execute(pool)
        .await
        .expect("revoke");
    let u2 = seed_user(pool).await;
    grant_role(pool, role_id, u2).await;
    u2
}

#[tokio::test]
async fn the_holder_group_mirrors_the_roles_holders() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, _role_id, user_id) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    let gid = sync_role_holder_group_in_tx(&mut tx, &role)
        .await
        .expect("sync");
    tx.commit().await.unwrap();

    let (name, source): (String, String) =
        sqlx::query_as("SELECT name, source FROM public.groups WHERE id=$1")
            .bind(gid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name, role_holder_group_name(&role));
    assert_eq!(source, "managed");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(members, vec![user_id]);
}

#[tokio::test]
async fn re_syncing_reconciles_added_and_removed_holders() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, role_id, u1) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    let gid = sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    let u2 = revoke_u1_grant_u2(&pool, role_id, u1).await;
    let mut tx = pool.begin().await.unwrap();
    let gid2 = sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(gid, gid2, "idempotent: same group");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(members, vec![u2]);
}

#[tokio::test]
async fn a_holder_change_resyncs_the_managed_group_via_the_chokepoint() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, role_id, _u1) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    let u2 = seed_user(&pool).await;
    grant_role(&pool, role_id, u2).await;
    egret_perm_compile::role_membership::recompile_role_referencing_profiles(&pool)
        .await
        .unwrap();

    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.user_groups ug \
         JOIN public.groups g ON g.id = ug.group_id \
         WHERE g.name = $1 AND ug.user_id = $2)",
    )
    .bind(role_holder_group_name(&role))
    .bind(u2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(present, "chokepoint must resync the holder group");
}

// ---------------------------------------------------------------------------
// SP3c-1 Task 4: the reversible, equivalence-gated role -> holder-group rewrite.
// ---------------------------------------------------------------------------

use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::recompile_service_in_tx;
use egret_perm_compile::role_principal_migrate::{
    migrate_role_principals_in_tx, undo_role_principals_in_tx,
};
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use sqlx::PgPool;

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Seeded {
    service_id: i64,
    role: String,
    grant_id: Uuid,
}

/// A StarRocks profile (one bound engine, one attached Iceberg catalog) with a
/// read grant whose only principal is `kind='role'` naming a fresh role that has
/// one (username-less) user holder. The service is NOT compiled yet.
async fn seed_service_with_role_grant(db: &Db) -> Seeded {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&uniq("sr_rolemig"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(uniq("rolemig-ws"))
            .bind("Role Mig WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-rolemig-test','AKIATEST') RETURNING id",
    )
    .bind(uniq("rolemig-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(uniq("rolemig_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(uniq("rolemig-eng"))
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
    .bind(uniq("rolemig_cat"))
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

    let (role, _role_id, _user_id) = seed_role_with_one_user_holder(pool).await;
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
    let grant_id = perm.create_grant(&grant).await.expect("create grant");

    Seeded {
        service_id: service.id,
        role,
        grant_id,
    }
}

/// Recompile the profile and return its compiled bodies (name + policy_jsonb,
/// ordered by name) as a JSON array — the artifact the engine downloads.
async fn compile_service_to_ranger_json(pool: &PgPool, service_id: i64) -> Value {
    let mut tx = pool.begin().await.unwrap();
    recompile_service_in_tx(&mut tx, service_id)
        .await
        .expect("recompile");
    tx.commit().await.unwrap();
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .unwrap();
    Value::Array(
        rows.into_iter()
            .map(|(name, body)| json!({ "name": name, "body": body }))
            .collect(),
    )
}

/// Count rows with `kind='role'` via a literal query (sqlx 0.9 wants a
/// `&'static str`).
async fn role_kind_count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Delete the seeded grant (cascades its principals) so a global role-ref count
/// in another test is not polluted by this one.
async fn cleanup(pool: &PgPool, grant_id: Uuid) {
    sqlx::query("DELETE FROM perm.grants WHERE id = $1")
        .bind(grant_id)
        .execute(pool)
        .await
        .expect("cleanup grant");
}

#[tokio::test]
async fn migrating_role_refs_leaves_the_compile_byte_identical_and_removes_role_kind() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    let before = compile_service_to_ranger_json(&pool, seeded.service_id).await;
    assert!(
        !before.as_array().unwrap().is_empty(),
        "the role grant must compile to at least one policy before the rewrite: {before}"
    );

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();

    for (tbl, n) in [
        (
            "perm.grant_principals",
            role_kind_count(
                &pool,
                "SELECT count(*) FROM perm.grant_principals WHERE kind::text = 'role'",
            )
            .await,
        ),
        (
            "perm.rule_exemptions",
            role_kind_count(
                &pool,
                "SELECT count(*) FROM perm.rule_exemptions WHERE kind::text = 'role'",
            )
            .await,
        ),
        (
            "perm.engine_grant_principals",
            role_kind_count(
                &pool,
                "SELECT count(*) FROM perm.engine_grant_principals WHERE kind::text = 'role'",
            )
            .await,
        ),
    ] {
        assert_eq!(n, 0, "{tbl} still has role refs");
    }
    assert!(
        m.roles.contains(&seeded.role),
        "the role was migrated: {m:?}"
    );
    assert_eq!(m.rewritten, 1, "exactly the one grant principal: {m:?}");

    let after = compile_service_to_ranger_json(&pool, seeded.service_id).await;
    assert_eq!(
        before, after,
        "compile must be byte-identical after the rewrite"
    );

    cleanup(&pool, seeded.grant_id).await;
}

#[tokio::test]
async fn undo_restores_role_refs_when_rows_are_untouched() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    undo_role_principals_in_tx(&mut tx, m.id).await.unwrap();
    tx.commit().await.unwrap();

    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.grant_principals \
          WHERE kind::text = 'role' AND ref = $1 AND grant_id = $2",
    )
    .bind(&seeded.role)
    .bind(seeded.grant_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "undo restores the role ref");

    cleanup(&pool, seeded.grant_id).await;
}

#[tokio::test]
async fn undo_refuses_when_a_rewritten_row_changed() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Tamper with the rewritten group principal so its recorded hash no longer
    // matches what is stored.
    sqlx::query(
        "UPDATE perm.grant_principals SET ref = ref || '_tampered' \
          WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind",
    )
    .bind(seeded.grant_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let err = undo_role_principals_in_tx(&mut tx, m.id)
        .await
        .expect_err("undo must refuse a changed row");
    tx.rollback().await.unwrap();
    assert!(
        err.to_string().contains("undo refused"),
        "a 409 naming the change: {err}"
    );

    cleanup(&pool, seeded.grant_id).await;
}
