//! SP5c Task 4 (DB-gated): the iceberg-rest rule-enforcement plan / switch / undo
//! endpoints over the real `catalog_admin_router`, and the gate's refusals.
//!
//!   env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5474/egret \
//!     CARGO_TARGET_DIR=target-sp5c cargo test -p egret-catalog-engine --test iceberg_rest_gate

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_catalog_engine::store::CatalogStore;
use egret_catalog_engine::{catalog_admin_router, CatalogAdminState};
use egret_metastore::Db;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

struct Env {
    db: Db,
    app: axum::Router,
    admin: String,
    nobody: String,
}

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn insert_user(pool: &PgPool, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'migrate test', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("cm")))
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("user")
}

/// A session for a user who is (or isn't) a platform admin. `PlatformAuth` reads
/// the principal's platform roles directly (not the PDP), so the AllowAll PDP is
/// irrelevant to the 403 path.
async fn session(env: &Env, platform_admin: bool) -> String {
    let uid = insert_user(&env.db.pool, None).await;
    if platform_admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid)
        .execute(&env.db.pool)
        .await
        .expect("grant platform_admin");
    }
    let sessions = SessionService::new(
        env.db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 8).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let app = catalog_admin_router().with_state(CatalogAdminState {
        store: CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        secrets: {
            let crypto =
                egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto");
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
            )
        },
        provisioner: None,
        browser: None,
        sessions,
        pdp: std::sync::Arc::new(AllowAllPdp),
        perm_compiler: None,
    });
    let mut env = Env {
        db,
        app,
        admin: String::new(),
        nobody: String::new(),
    };
    env.admin = session(&env, true).await;
    env.nobody = session(&env, false).await;
    Some(env)
}

/// One grant spec: (securable, namespace levels, table, privilege).
type GrantSpec = (
    &'static str,
    Option<&'static [&'static str]>,
    Option<&'static str>,
    &'static str,
);

struct SeededRole {
    username: String,
}

struct Seeded {
    catalog_id: Uuid,
    warehouse_id: Uuid,
    roles: Vec<SeededRole>,
}

/// Seed a warehouse + registry catalog, and one role per spec, each bound to a
/// fresh service-account client.
async fn seed_catalog(pool: &PgPool, specs: &[(&str, &[GrantSpec])]) -> Seeded {
    let wh = egret_test_db::seed_warehouse(pool).await;
    let (ws, sc): (Uuid, Uuid) =
        sqlx::query_as("SELECT workspace_id, storage_conn_id FROM catalog.warehouses WHERE id=$1")
            .bind(wh)
            .fetch_one(pool)
            .await
            .expect("warehouse row");
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
    .fetch_one(pool)
    .await
    .expect("registry catalog");

    let mut roles = Vec::new();
    for (rname, grants) in specs {
        let rid: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
             VALUES ($1, $2, 'external') RETURNING id",
        )
        .bind(wh)
        .bind(rname)
        .fetch_one(pool)
        .await
        .expect("role");
        for (sec, ns, table, privilege) in *grants {
            sqlx::query(
                "INSERT INTO catalog.catalog_grants \
                   (catalog_role_id, securable_type, namespace, table_name, privilege) \
                 VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(rid)
            .bind(sec)
            .bind(ns.map(|n| n.iter().map(|s| s.to_string()).collect::<Vec<_>>()))
            .bind(table)
            .bind(privilege)
            .execute(pool)
            .await
            .expect("grant");
        }
        let username = format!("{rname}-{}", &tag[..8]);
        let sa = insert_user(pool, Some(&username)).await;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, scope, workspace_id, name) \
             VALUES ($1,'workspace',$2,$3)",
        )
        .bind(sa)
        .bind(ws)
        .bind(format!("n-{sa}"))
        .execute(pool)
        .await
        .expect("sa");
        let client_id = format!("cid-{rname}-{}", &tag[..8]);
        sqlx::query(
            "INSERT INTO catalog.catalog_clients \
               (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, \
                client_secret_hash, service_account_id) \
             VALUES ($1, 'external', $2, $3, $4, $5, 'h', $6)",
        )
        .bind(format!("c-{}", Uuid::new_v4()))
        .bind(ws)
        .bind(wh)
        .bind(rid)
        .bind(&client_id)
        .bind(sa)
        .execute(pool)
        .await
        .expect("client");
        roles.push(SeededRole { username });
    }
    Seeded {
        catalog_id,
        warehouse_id: wh,
        roles,
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

use egret_catalog_engine::catalog_migrate_gate;
use egret_catalog_engine::iceberg_rest_gate;
use egret_catalog_engine::unified_decide::{CatalogEnforcementSnapshotCache, ReadEnforcement};

fn uri(catalog: Uuid, op: &str) -> String {
    format!("/perm/iceberg-rest/{catalog}/{op}")
}

const CORPUS: &[(&str, &[GrantSpec])] = &[
    ("reader", &[("catalog", None, None, "read")]),
    ("exempt", &[("catalog", None, None, "read")]),
];

async fn catalog_name(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("name")
}

/// A mask on `sales.orders.id`, with `exempt` usernames cleared of it.
async fn mask_orders(pool: &PgPool, catalog_id: Uuid, exempt: &[&str]) {
    let rule: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
                                 schema_name, table_name, column_name) \
         VALUES ($1,'mask'::perm.rule_kind,'MASK_NULL','path'::perm.selector_kind, \
                 $2,'sales','orders','id') RETURNING id",
    )
    .bind(unique("sp5c-"))
    .bind(catalog_id)
    .fetch_one(pool)
    .await
    .expect("rule");
    for user in exempt {
        sqlx::query(
            "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
             VALUES ($1,'user',$2,'include')",
        )
        .bind(rule)
        .bind(*user)
        .execute(pool)
        .await
        .expect("exemption");
    }
}

/// Fold + switch the grants to unified (SP5b), as a prerequisite.
async fn grants_to_unified(e: &Env, catalog_id: Uuid) {
    let store = CatalogStore::new(e.db.clone());
    catalog_migrate_gate::plan(&store, catalog_id, "sp5c-test")
        .await
        .expect("fold");
    catalog_migrate_gate::switch(&store, catalog_id, "sp5c-test")
        .await
        .expect("grant switch");
}

async fn enforce_row(pool: &PgPool, id: Uuid) -> Option<(bool, bool, Option<String>)> {
    sqlx::query_as(
        "SELECT enforce, switched_at IS NOT NULL, switched_by \
           FROM perm.catalog_rule_enforcement_switch WHERE catalog_id=$1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .expect("row")
}

async fn denies(e: &Env, catalog_id: Uuid, user: &str) -> bool {
    let cache = CatalogEnforcementSnapshotCache::new();
    let mut c = e.db.pool.acquire().await.unwrap();
    let name = catalog_name(&e.db.pool, catalog_id).await;
    let enf = ReadEnforcement::load(&cache, &mut c, catalog_id).await;
    enf.denies_access(user, &name, &["sales".to_string()], "orders")
}

#[tokio::test]
async fn plan_lists_masked_table_for_non_exempt_readers_only() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    let (reader, exempt) = (&s.roles[0].username, &s.roles[1].username);
    mask_orders(&e.db.pool, s.catalog_id, &[exempt]).await;
    grants_to_unified(&e, s.catalog_id).await;

    let (st, plan) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    assert_eq!(plan["grantSourceUnified"], json!(true), "{plan}");
    assert_eq!(plan["grantsIdentical"], json!(true), "{plan}");
    assert_eq!(plan["grantDivergences"], json!([]), "{plan}");
    let cat = catalog_name(&e.db.pool, s.catalog_id).await;
    assert_eq!(
        plan["newDenies"],
        json!([{"principal": reader, "catalog": cat, "schema": "sales", "table": "orders"}]),
        "only the non-exempt reader is denied: {plan}"
    );
    // plan changes nothing.
    assert_eq!(enforce_row(&e.db.pool, s.catalog_id).await, None);
}

#[tokio::test]
async fn switch_enforces_then_undo_restores() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    let (reader, exempt) = (&s.roles[0].username, &s.roles[1].username);
    mask_orders(&e.db.pool, s.catalog_id, &[exempt]).await;
    grants_to_unified(&e, s.catalog_id).await;
    assert!(!denies(&e, s.catalog_id, reader).await, "control: opt-in");

    let (st, body) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (on, at, by) = enforce_row(&e.db.pool, s.catalog_id).await.expect("row");
    assert!(on && at && by.is_some(), "enforce + audit cols");
    assert!(denies(&e, s.catalog_id, reader).await, "reader denied");
    assert!(
        !denies(&e, s.catalog_id, exempt).await,
        "exempt still reads"
    );

    let (st, _) = call(&e.app, "POST", &uri(s.catalog_id, "undo"), &e.admin, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (on, _, _) = enforce_row(&e.db.pool, s.catalog_id).await.expect("row");
    assert!(!on);
    assert!(!denies(&e, s.catalog_id, reader).await, "allowed again");
}

#[tokio::test]
async fn switch_refuses_a_catalog_not_on_the_unified_source() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    mask_orders(&e.db.pool, s.catalog_id, &[]).await;

    let (st, plan) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    assert_eq!(plan["grantSourceUnified"], json!(false), "{plan}");
    assert_eq!(plan["newDenies"], json!([]));

    let (st, body) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("unified"), "clear error: {body}");
    assert_eq!(
        enforce_row(&e.db.pool, s.catalog_id).await,
        None,
        "unchanged"
    );
}

#[tokio::test]
async fn switch_refuses_when_grants_are_not_identical() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    mask_orders(&e.db.pool, s.catalog_id, &[]).await;
    grants_to_unified(&e, s.catalog_id).await;
    // Legacy drifts after the fold: a new legacy write grant unified never saw.
    sqlx::query(
        "INSERT INTO catalog.catalog_grants (catalog_role_id, securable_type, privilege) \
         SELECT id, 'catalog', 'write' FROM catalog.catalog_roles \
          WHERE warehouse_id=$1 AND name='reader'",
    )
    .bind(s.warehouse_id)
    .execute(&e.db.pool)
    .await
    .expect("drift");

    let (st, plan) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    assert_eq!(plan["grantsIdentical"], json!(false), "{plan}");
    assert!(!plan["grantDivergences"].as_array().unwrap().is_empty());

    let (st, body) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        enforce_row(&e.db.pool, s.catalog_id).await,
        None,
        "unchanged"
    );
}

#[tokio::test]
async fn grant_check_is_skipped_after_legacy_is_dropped() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    mask_orders(&e.db.pool, s.catalog_id, &[]).await;
    grants_to_unified(&e, s.catalog_id).await;
    let store = CatalogStore::new(e.db.clone());
    catalog_migrate_gate::finalize(&store, s.catalog_id)
        .await
        .expect("finalize");

    let cache = CatalogEnforcementSnapshotCache::new();
    let plan = iceberg_rest_gate::plan(&store, &cache, s.catalog_id)
        .await
        .expect("plan");
    assert!(plan.grants_identical && plan.grant_divergences.is_empty());
}

#[tokio::test]
async fn routes_are_platform_admin_only() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    for op in ["plan", "switch", "undo"] {
        let (st, _) = call(&e.app, "POST", &uri(s.catalog_id, op), &e.nobody, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{op}");
    }
    assert_eq!(enforce_row(&e.db.pool, s.catalog_id).await, None);
    let (st, _) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(st, StatusCode::OK);
}

/// A path mask at the given level (`schema`/`table` may be NULL = wider rule).
async fn path_mask(pool: &PgPool, catalog_id: Uuid, schema: Option<&str>, table: Option<&str>) {
    let column = table.map(|_| "id");
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
                                 schema_name, table_name, column_name) \
         VALUES ($1,'mask'::perm.rule_kind,'MASK_NULL','path'::perm.selector_kind,$2,$3,$4,$5)",
    )
    .bind(unique("sp5c-"))
    .bind(catalog_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .execute(pool)
    .await
    .expect("path rule");
}

#[tokio::test]
async fn a_tag_mask_on_a_tagged_table_appears_in_new_denies() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    let pool = &e.db.pool;
    let key = unique("sp5c-tag");
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1,$2) RETURNING id")
        .bind(&key)
        .bind(unique("Sp5cTag"))
        .fetch_one(pool)
        .await
        .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales','orders','id')",
    )
    .bind(tag)
    .bind(s.warehouse_id)
    .execute(pool)
    .await
    .expect("assign");
    // The ONLY rule is a tag mask: no path rule names the table.
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, tag_key) \
         VALUES ($1,'mask'::perm.rule_kind,'MASK_NULL','tag'::perm.selector_kind,$2)",
    )
    .bind(unique("sp5c-"))
    .bind(&key)
    .execute(pool)
    .await
    .expect("tag rule");
    grants_to_unified(&e, s.catalog_id).await;

    let (st, plan) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    let denied: Vec<&str> = plan["newDenies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["principal"].as_str().unwrap())
        .collect();
    assert_eq!(
        denied.len(),
        2,
        "both readers lose the tagged table: {plan}"
    );
    assert_eq!(plan["newDenies"][0]["table"], json!("orders"), "{plan}");
    assert_eq!(plan["previewComplete"], json!(true), "{plan}");
}

#[tokio::test]
async fn preview_complete_is_false_for_wide_rules_true_for_named_tables() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CORPUS).await;
    path_mask(&e.db.pool, s.catalog_id, Some("sales"), Some("orders")).await;
    grants_to_unified(&e, s.catalog_id).await;
    let (_, plan) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(plan["previewComplete"], json!(true), "{plan}");

    // A schema-wide rule restricts tables that cannot be listed.
    path_mask(&e.db.pool, s.catalog_id, Some("sales"), None).await;
    let (_, plan) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(plan["previewComplete"], json!(false), "schema-wide: {plan}");

    // A catalog-wide rule likewise.
    let s2 = seed_catalog(&e.db.pool, CORPUS).await;
    path_mask(&e.db.pool, s2.catalog_id, None, None).await;
    grants_to_unified(&e, s2.catalog_id).await;
    let (_, plan) = call(&e.app, "POST", &uri(s2.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(
        plan["previewComplete"],
        json!(false),
        "catalog-wide: {plan}"
    );
}
