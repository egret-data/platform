//! Security re-review M-a: an editor write of a hand-written policy (edit,
//! enable, disable, delete) on a profile whose engines carry impersonation
//! grants recompiles that profile in the editor's own transaction, so a target
//! who just became near-root (or gained a hop) loses its `perm-engine-` policy
//! at once, and regains it when the write is undone.
//!
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_editor

use egret_metastore::Db;
use egret_perm::{
    create_engine_grant_in_tx, EngineGrant, PermStore, Principal, PrincipalKind, PrincipalSet,
};
use egret_perm_compile::recompile_service;
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// No engine bindings: the editor routes these tests drive never read them.
struct NoEngines;
#[async_trait::async_trait]
impl egret_policy::EngineBindingReader for NoEngines {
    async fn list_engine_bindings(
        &self,
    ) -> Result<Vec<egret_policy::EngineBinding>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn engine_catalog_names(
        &self,
        _e: Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_schemas(
        &self,
        _w: Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_tables(
        &self,
        _w: Uuid,
        _s: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_columns(
        &self,
        _w: Uuid,
        _s: &str,
        _t: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
}

/// The policy editor's router as egret-bin mounts it (with the real
/// recompile seam), and a platform admin's session token.
async fn editor(db: &Db, policy: &PolicyStore) -> (axum::Router, String) {
    use std::sync::Arc;
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let uid = egret_auth::users::UserRepo::new(db.clone())
        .insert_local(&format!("{}@egret.local", unique("refold")), "T", "x")
        .await
        .expect("insert user")
        .0;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let app = egret_policy::policy_admin_router().with_state(egret_policy::PolicyAdminState {
        store: policy.clone(),
        engines: Arc::new(NoEngines),
        users: egret_auth::users::UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions,
        engine_egret_url: "http://egret:8080".into(),
        pdp,
        recompiler: Arc::new(egret_perm_compile::PermRecompiler),
    });
    (app, token)
}

async fn call(
    app: &axum::Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (axum::http::StatusCode, Value) {
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    let req = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("content-type", "application/json")
        .body(match body {
            Some(b) => axum::body::Body::from(b.to_string()),
            None => axum::body::Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn person(pool: &sqlx::PgPool, prefix: &str) -> (Uuid, String) {
    let name = unique(prefix);
    let id = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'Editor test', $2) RETURNING id",
    )
    .bind(format!("{name}@egret.local"))
    .bind(&name)
    .fetch_one(pool)
    .await
    .expect("user");
    (id, name)
}

#[tokio::test]
async fn an_editor_write_that_makes_a_target_near_root_reaps_its_impersonation_grant() {
    let Some(db) = test_db().await else { return };
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("starrocks def");
    let profile_name = unique("edt-sr");
    let profile = policy
        .create_service(&profile_name, "starrocks", &json!({}))
        .await
        .expect("profile")
        .id;
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'starrocks', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(unique("edt-eng"))
    .bind(profile)
    .fetch_one(pool)
    .await
    .expect("engine");
    let (_, alice) = person(pool, "edt-a").await;
    let (ops_id, ops) = person(pool, "edt-ops").await;
    let mut tx = pool.begin().await.expect("begin");
    let grant = create_engine_grant_in_tx(
        &mut tx,
        &EngineGrant {
            id: Uuid::nil(),
            engine_id: engine,
            capability: "starrocks.impersonate_user".into(),
            values: vec![],
            target_user_id: Some(ops_id),
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::User,
                    r#ref: alice.clone(),
                }],
                exclude: vec![],
            },
        },
    )
    .await
    .expect("grant");
    tx.commit().await.expect("commit");
    recompile_service(&perm, &policy, profile)
        .await
        .expect("recompile");
    let compiled = || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM policy.authz_policies WHERE service_id = $1 AND name = $2)",
        )
        .bind(profile)
        .bind(format!("perm-engine-{grant}"))
        .fetch_one(pool)
        .await
        .expect("read")
    };
    assert!(compiled().await, "compiled to start with");

    // An operator's policy from before, disabled: `ops` may operate.
    let name = "ops may operate";
    let body = |users: Value| {
        json!({"name": name, "policyType": 0, "isEnabled": true,
               "resources": {"system": {"values": ["*"], "isExcludes": false, "isRecursive": false}},
               "policyItems": [{"users": users, "groups": [], "roles": [],
                                "accesses": [{"type": "operate", "isAllowed": true}]}]})
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled) \
         VALUES ($1, $2, $3, false) RETURNING id",
    )
    .bind(profile)
    .bind(name)
    .bind(body(json!([ops])))
    .fetch_one(pool)
    .await
    .expect("operator policy");
    assert!(compiled().await, "a disabled policy grants nothing");

    let (app, token) = editor(&db, &policy).await;
    let uri = format!("/policies/{id}/enabled");
    let (status, b) = call(&app, &token, "PUT", &uri, Some(json!({"enabled": true}))).await;
    assert!(status.is_success(), "{status} {b}");
    assert!(
        !compiled().await,
        "enabling it reaps the grant in the same write"
    );

    let (status, b) = call(&app, &token, "PUT", &uri, Some(json!({"enabled": false}))).await;
    assert!(status.is_success(), "{status} {b}");
    assert!(compiled().await, "disabling it compiles the grant again");

    let form = |user: &str| {
        json!({"kind": "access", "name": name, "service": profile_name, "isEnabled": true,
               "resources": {"system": {"values": ["*"]}},
               "items": [{"users": [user], "accesses": ["operate"], "effect": "allow"}]})
    };
    let (status, b) = call(
        &app,
        &token,
        "PUT",
        &format!("/policies/{id}"),
        Some(form(&ops)),
    )
    .await;
    assert!(status.is_success(), "{status} {b}");
    assert!(!compiled().await, "an edit that enables it reaps the grant");

    let (status, b) = call(&app, &token, "DELETE", &format!("/policies/{id}"), None).await;
    assert!(status.is_success(), "{status} {b}");
    assert!(compiled().await, "deleting it compiles the grant again");
}
