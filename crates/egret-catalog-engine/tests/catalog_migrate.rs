//! SP5b Task 6 (DB-gated): the equivalence-gated catalog store migration
//! endpoints — plan / switch / undo / finalize — over the real
//! `catalog_admin_router`, plus the admin-only gate and the roleless-client
//! survival `finalize` depends on.
//!
//! Every assertion is scoped to this fixture's own rows (tests in one binary
//! share a database), so names carry a per-run uuid.
//!
//!   env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5473/egret \
//!     CARGO_TARGET_DIR=target-sp5b cargo test -p egret-catalog-engine --test catalog_migrate

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_catalog_engine::store::CatalogStore;
use egret_catalog_engine::{catalog_admin_router, CatalogAdminState};
use egret_machine_idp::ClientVerifier;
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
    client_id: String,
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
        roles.push(SeededRole {
            client_id,
            username,
        });
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

fn uri(catalog: Uuid, op: &str) -> String {
    format!("/perm/catalog-migrate/{catalog}/{op}")
}

async fn switch_source(pool: &PgPool, catalog_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT source FROM perm.catalog_grant_switch WHERE catalog_id=$1")
        .bind(catalog_id)
        .fetch_optional(pool)
        .await
        .expect("switch")
}

async fn perm_grant_count(pool: &PgPool, catalog_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id=$1")
        .bind(catalog_id)
        .fetch_one(pool)
        .await
        .expect("count")
}

/// A clean ASCII corpus: catalog read, a table write, a namespace manage —
/// distinct principals, no dot-collision.
const CLEAN: &[(&str, &[GrantSpec])] = &[
    ("reader", &[("catalog", None, None, "read")]),
    (
        "tablewriter",
        &[("table", Some(&["hr"]), Some("salaries"), "write")],
    ),
    (
        "nsmanager",
        &[("namespace", Some(&["sales", "eu"]), None, "manage")],
    ),
];

#[tokio::test]
async fn plan_is_clean_then_switch_goes_live() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CLEAN).await;

    let (status, report) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["divergences"], json!([]), "clean: {report}");
    assert!(report["principals"].as_u64().unwrap() >= 3, "{report}");
    assert!(report["comparisons"].as_u64().unwrap() > 0, "{report}");
    // plan folded without switching.
    assert_eq!(switch_source(&e.db.pool, s.catalog_id).await, None);
    assert!(perm_grant_count(&e.db.pool, s.catalog_id).await >= 3);

    let (status, report) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        switch_source(&e.db.pool, s.catalog_id).await.as_deref(),
        Some("unified"),
        "the switch is live"
    );

    // Unified decision resolves the service account and matches legacy for a probe.
    let mut c = e.db.pool.acquire().await.unwrap();
    for r in &s.roles {
        let u =
            egret_catalog_engine::unified_decide::service_account_username(&mut c, &r.client_id)
                .await
                .unwrap();
        assert_eq!(u.as_deref(), Some(r.username.as_str()));
    }
}

#[tokio::test]
async fn a_dot_in_a_namespace_level_is_refused_at_plan() {
    let Some(e) = env().await else { return };
    // A namespace level that itself contains a dot (`["a.b"]`) cannot be
    // represented in the dot-joined unified schema (it would be indistinguishable
    // from `["a","b"]`): the mapping REFUSES it, so plan fails naming the grant and
    // nothing is folded — the switch then has nothing to switch.
    const SPECS: &[(&str, &[GrantSpec])] =
        &[("dotns", &[("namespace", Some(&["a.b"]), None, "read")])];
    let s = seed_catalog(&e.db.pool, SPECS).await;

    let (status, body) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("'.'"),
        "names the offending dot: {body}"
    );
    assert_eq!(
        perm_grant_count(&e.db.pool, s.catalog_id).await,
        0,
        "no fold"
    );

    let (status, body) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "not folded: {body}");
    assert_eq!(switch_source(&e.db.pool, s.catalog_id).await, None);
}

#[tokio::test]
async fn a_shared_service_account_across_roles_diverges_and_blocks_switch() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    // One service account bound (through two clients) to two roles with disjoint
    // grants. Legacy decides by the TOKEN's role (one role's grants); unified
    // decides by the service-account principal, which the fold gave BOTH roles'
    // grants — a union that over-grants relative to either role alone. The gate
    // samples the SA once per role and must report the divergence and refuse the
    // switch.
    let wh = egret_test_db::seed_warehouse(pool).await;
    let (ws, sc): (Uuid, Uuid) =
        sqlx::query_as("SELECT workspace_id, storage_conn_id FROM catalog.warehouses WHERE id=$1")
            .bind(wh)
            .fetch_one(pool)
            .await
            .unwrap();
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
    .unwrap();
    // One shared service account.
    let sa = insert_user(pool, Some(&format!("shared-{}", &tag[..8]))).await;
    sqlx::query("INSERT INTO service_accounts (user_id, scope, workspace_id, name) VALUES ($1,'workspace',$2,$3)")
        .bind(sa)
        .bind(ws)
        .bind(format!("n-{sa}"))
        .execute(pool)
        .await
        .unwrap();
    for (rname, ns) in [("alpha", "alpha"), ("beta", "beta")] {
        let rid: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) VALUES ($1,$2,'external') RETURNING id",
        )
        .bind(wh)
        .bind(rname)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.catalog_grants (catalog_role_id, securable_type, namespace, privilege) \
             VALUES ($1,'namespace',$2,'manage')",
        )
        .bind(rid)
        .bind(vec![ns.to_string()])
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.catalog_clients \
               (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, client_secret_hash, service_account_id) \
             VALUES ($1,'external',$2,$3,$4,$5,'h',$6)",
        )
        .bind(format!("c-{}", Uuid::new_v4()))
        .bind(ws)
        .bind(wh)
        .bind(rid)
        .bind(format!("cid-{rname}-{}", &tag[..8]))
        .bind(sa)
        .execute(pool)
        .await
        .unwrap();
    }

    let (status, report) = call(&e.app, "POST", &uri(catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let divs = report["divergences"].as_array().unwrap();
    assert!(
        divs.iter()
            .any(|d| d["legacy"] == json!(false) && d["unified"] == json!(true)),
        "the union over-grant must surface as legacy=false/unified=true: {report}"
    );

    let (status, body) = call(&e.app, "POST", &uri(catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "switch must refuse: {body}");
    assert_eq!(
        switch_source(&e.db.pool, catalog_id).await,
        None,
        "nothing switched"
    );
}

#[tokio::test]
async fn undo_after_switch_restores_the_legacy_store() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CLEAN).await;
    call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    let (status, _) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(perm_grant_count(&e.db.pool, s.catalog_id).await >= 3);

    let (status, body) = call(&e.app, "POST", &uri(s.catalog_id, "undo"), &e.admin, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        switch_source(&e.db.pool, s.catalog_id).await.as_deref(),
        Some("catalog_rbac"),
        "back to legacy enforcement"
    );
    assert_eq!(
        perm_grant_count(&e.db.pool, s.catalog_id).await,
        0,
        "folded grants removed"
    );
    let prov: i64 =
        sqlx::query_scalar("SELECT count(*) FROM perm.catalog_rbac_migrated WHERE catalog_id=$1")
            .bind(s.catalog_id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(prov, 0, "provenance gone");
    // Legacy RBAC untouched: the roles and grants still exist.
    let roles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog.catalog_roles WHERE warehouse_id=$1")
            .bind(s.warehouse_id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(roles, 3, "legacy roles intact");
}

#[tokio::test]
async fn finalize_drops_legacy_nulls_roles_and_blocks_undo() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CLEAN).await;
    call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    let (status, _) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, summary) = call(
        &e.app,
        "POST",
        &uri(s.catalog_id, "finalize"),
        &e.admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["rolesDropped"], json!(3), "{summary}");
    assert_eq!(summary["clientsDetached"], json!(3), "{summary}");
    assert!(summary["grantsDropped"].as_u64().unwrap() >= 3, "{summary}");

    // Legacy store retired; clients survive with a NULL role.
    let roles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog.catalog_roles WHERE warehouse_id=$1")
            .bind(s.warehouse_id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(roles, 0, "roles dropped");
    let (clients, null_roles): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE catalog_role_id IS NULL) \
           FROM catalog.catalog_clients WHERE warehouse_id=$1",
    )
    .bind(s.warehouse_id)
    .fetch_one(&e.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (clients, null_roles),
        (3, 3),
        "clients survive, role NULLed"
    );

    // Functional survival: a roleless client still mints a token (roles: []), and
    // its service-account principal still resolves (unified enforcement path).
    let store = CatalogStore::new(e.db.clone());
    let r = &s.roles[0];
    let state = ClientVerifier::current(&store, &r.client_id)
        .await
        .expect("current")
        .expect("the finalized client still mints");
    assert_eq!(
        state.verified.extra_claims["roles"],
        json!([]),
        "a roleless client's token has an empty roles claim"
    );
    let mut c = e.db.pool.acquire().await.unwrap();
    let u = egret_catalog_engine::unified_decide::service_account_username(&mut c, &r.client_id)
        .await
        .unwrap();
    assert_eq!(
        u.as_deref(),
        Some(r.username.as_str()),
        "principal resolves"
    );

    // No more undo.
    let (status, body) = call(&e.app, "POST", &uri(s.catalog_id, "undo"), &e.admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "finalize is terminal: {body}");
    assert_eq!(
        switch_source(&e.db.pool, s.catalog_id).await.as_deref(),
        Some("unified"),
        "the refused undo changed nothing"
    );
}

#[tokio::test]
async fn finalize_is_refused_for_a_non_ascii_catalog() {
    let Some(e) = env().await else { return };
    // A non-ASCII namespace folds to a non-ASCII schema name: unified would
    // under-grant relative to lakekeeper's collation, so finalize must refuse —
    // even though the exact-spelling probes make the switch itself clean.
    const SPECS: &[(&str, &[GrantSpec])] = &[(
        "accents",
        &[("namespace", Some(&["caf\u{e9}"]), None, "read")],
    )];
    let s = seed_catalog(&e.db.pool, SPECS).await;
    let (status, report) = call(&e.app, "POST", &uri(s.catalog_id, "plan"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["divergences"], json!([]), "exact spellings agree");
    let (status, _) = call(&e.app, "POST", &uri(s.catalog_id, "switch"), &e.admin, None).await;
    assert_eq!(status, StatusCode::OK, "switch is allowed");

    let (status, body) = call(
        &e.app,
        "POST",
        &uri(s.catalog_id, "finalize"),
        &e.admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "finalize must refuse: {body}");
    // Legacy store NOT dropped.
    let roles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog.catalog_roles WHERE warehouse_id=$1")
            .bind(s.warehouse_id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(roles, 1, "legacy kept");
}

#[tokio::test]
async fn every_catalog_migrate_route_is_platform_admin_only() {
    let Some(e) = env().await else { return };
    let s = seed_catalog(&e.db.pool, CLEAN).await;
    for op in ["plan", "switch", "undo", "finalize"] {
        let (status, _) = call(&e.app, "POST", &uri(s.catalog_id, op), &e.nobody, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{op} must be admin-only");
    }
    // Nothing a non-admin touched took effect.
    assert_eq!(perm_grant_count(&e.db.pool, s.catalog_id).await, 0);
    assert_eq!(switch_source(&e.db.pool, s.catalog_id).await, None);
}
