//! Narrowing 2 (`egret_perm_compile::members`): a grant whose `expires_at`
//! lapses is not a write, so nothing recompiles the profile that named the
//! holder. `recompile_lapsed_grants` (the hourly sweep) must recompile it.
//!
//! Every assertion asks the engine (`egret-ranger-eval` over the profile's
//! download) as the holder's username.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test expiry_sweep

#[path = "support/engine_eval.rs"]
mod engine_eval;

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::expiry::recompile_lapsed_grants;
use egret_perm_compile::recompile_service;
use egret_policy::PolicyStore;
use engine_eval::{allowed, starrocks_decides};
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
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    service_id: i64,
    catalog_id: Uuid,
    catalog_name: String,
}

/// A StarRocks profile with one engine bound to it and one catalog attached,
/// the shape `user_delete_recompile.rs` uses.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_expiry"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("expiry-ws"))
            .bind("Expiry WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-expiry-test','AKIATEST') RETURNING id",
    )
    .bind(unique("expiry-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("expiry_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret',$3,$2) RETURNING id",
    )
    .bind(unique("expiry-eng"))
    .bind(service.id)
    .bind(serde_json::json!({
        "starrocksConfig": {"feImage": egret_perm::DEFAULT_ENGINE_IMAGES.starrocks_fe_image}
    }))
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("expiry_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog_name)
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
    Fixture {
        db: db.clone(),
        perm,
        policy,
        service_id: service.id,
        catalog_id,
        catalog_name,
    }
}

async fn new_user(db: &Db, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'T',$2) RETURNING id",
    )
    .bind(format!("{username}@expiry.test"))
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn new_role(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert role")
}

/// A platform-scoped role grant to `user_id` that lapses at `expires_at`
/// (`None` = never).
async fn role_grant(db: &Db, user_id: Uuid, role_id: Uuid, expires_in_secs: Option<i64>) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, expires_at) \
         VALUES ('user', $1, $2, 'platform', NULL, \
                 CASE WHEN $3::bigint IS NULL THEN NULL \
                      ELSE now() + make_interval(secs => $3::bigint::double precision) END)",
    )
    .bind(user_id)
    .bind(role_id)
    .bind(expires_in_secs)
    .execute(&db.pool)
    .await
    .expect("access grant");
}

fn p(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.to_string(),
    }
}

fn allow_role(f: &Fixture, role: &str) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: f.catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![p(PrincipalKind::Role, role)],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}

/// `select` on a column of the fixture's catalog, as `user`.
async fn can_select(f: &Fixture, user: &str) -> bool {
    allowed(
        &starrocks_decides(
            &f.db,
            f.service_id,
            user,
            "select",
            &[
                ("catalog", f.catalog_name.as_str()),
                ("database", "sales"),
                ("table", "orders"),
                ("column", "email"),
            ],
        )
        .await,
    )
}

async fn recompile(f: &Fixture) {
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
}

#[tokio::test]
async fn a_lapsed_role_grant_is_swept_out_of_its_profile() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let role = unique("analyst");
    let role_id = new_role(&db, &role).await;
    let lapsed_name = unique("lapsed");
    let lapsed = new_user(&db, &lapsed_name).await;
    let live_name = unique("live");
    let live = new_user(&db, &live_name).await;
    role_grant(&db, lapsed, role_id, Some(3600)).await;
    role_grant(&db, live, role_id, Some(3600)).await;
    f.perm
        .create_grant(&allow_role(&f, &role))
        .await
        .expect("grant");
    recompile(&f).await;
    assert!(can_select(&f, &lapsed_name).await, "the arrangement");
    assert!(can_select(&f, &live_name).await, "the arrangement");

    // The lapse: the row's expiry passes one minute ago. Nothing recompiles.
    sqlx::query(
        "UPDATE access_grants SET expires_at = now() - interval '1 minute' WHERE principal_id = $1",
    )
    .bind(lapsed)
    .execute(&db.pool)
    .await
    .expect("lapse");
    assert!(
        can_select(&f, &lapsed_name).await,
        "before the sweep the lapsed holder is still named (narrowing 2)"
    );

    let summary = recompile_lapsed_grants(&db.pool).await.expect("sweep");
    assert!(summary.lapsed_rows >= 1, "the lapsed row is detected");
    assert!(
        summary.services.contains(&f.service_id),
        "the sweep recompiles the profile the role grant reaches"
    );
    assert!(
        !can_select(&f, &lapsed_name).await,
        "after the sweep the lapsed holder is no longer named"
    );
    assert!(
        can_select(&f, &live_name).await,
        "an unexpired holder is still named"
    );
}

#[tokio::test]
async fn the_sweep_is_idempotent() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let role = unique("auditor");
    let role_id = new_role(&db, &role).await;
    let user = new_user(&db, &unique("once")).await;
    role_grant(&db, user, role_id, Some(3600)).await;
    f.perm
        .create_grant(&allow_role(&f, &role))
        .await
        .expect("grant");
    recompile(&f).await;
    sqlx::query(
        "UPDATE access_grants SET expires_at = now() - interval '1 minute' WHERE principal_id = $1",
    )
    .bind(user)
    .execute(&db.pool)
    .await
    .expect("lapse");

    recompile_lapsed_grants(&db.pool)
        .await
        .expect("first sweep");
    let second = recompile_lapsed_grants(&db.pool)
        .await
        .expect("second sweep");
    assert!(
        second.services.contains(&f.service_id),
        "the window still holds the lapse, so the profile is compiled again"
    );
    assert_eq!(
        second.rewritten, 0,
        "an unchanged profile writes nothing on a repeat sweep"
    );
}
