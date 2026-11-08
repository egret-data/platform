//! Governance SP2b Task 15 (carry 5, Task 12 review): the mechanism switch
//! driven through the route with the REAL `PermRecompiler`. On a profile whose
//! compiled policies are current, a switch bumps `policy_version` by exactly
//! one (the handler's own bump; the Ranger recompile finds nothing to write),
//! and the Ranger download is the same artifact before and after (S8).
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-perm-compile --test mechanism_switch_recompiler

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The editor's engine-binding seam; the mask/row-filter authoring paths do not
/// read it (they touch only `policy.authz_*`), so an empty reader is honest.
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

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    policy: PolicyStore,
    app: axum::Router,
    token: String,
}

/// One profile with one engine bound to it and one registry catalog attached.
struct Profile {
    service_id: i64,
    catalog_id: Uuid,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    // The bare usernames the grants and exemptions below name. Since J-F3 a
    // write naming a username no `users` row carries is refused
    // (`egret_perm_compile::principal_refs`), so they must exist; idempotent and
    // shared across the database, and whoever already holds the name serves.
    sqlx::query(
        "INSERT INTO users (provider, email, username) \
         SELECT 'local', n || '@perm-overlap-fixture.egret.local', n \
           FROM unnest(ARRAY['alice', 'analyst']) n \
         ON CONFLICT DO NOTHING",
    )
    .execute(&db.pool)
    .await
    .expect("fixture usernames");
    let uid = UserRepo::new(db.clone())
        .insert_local(
            &format!("overlap_{}@egret.local", Uuid::new_v4().simple()),
            "Test",
            "x",
        )
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
    // The policy editor too (fix round 1, review C1): the hand-written half of
    // a compiled × hand-written pair is authored through `POST /policies`.
    let admin = egret_policy::policy_admin_router().with_state(egret_policy::PolicyAdminState {
        store: policy.clone(),
        engines: Arc::new(NoEngines),
        users: UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions: sessions.clone(),
        engine_egret_url: "http://egret:8080".into(),
        engine_opa_url: "http://egret:8082".into(),
        pdp: pdp.clone(),
        recompiler: Arc::new(egret_perm_compile::PermRecompiler),
        reconfigurer: std::sync::Arc::new(egret_policy::NoReconfigure),
        opa_enabled: true,
    });
    let app = perm_router()
        .with_state(PermState {
            opa_cache: None,
            perm: perm.clone(),
            policy: policy.clone(),
            sessions,
            pdp,
        })
        .merge(admin);
    Some(Env {
        db,
        policy,
        app,
        token,
    })
}

/// `engine` is `"trino"` (with a tag service of its own) or `"starrocks"`.
async fn profile(e: &Env, engine: &str) -> Profile {
    let tag = if engine == "trino" {
        let tag = unique("ovl_tag");
        e.policy
            .create_service(&tag, "tag", &json!({}))
            .await
            .expect("tag service");
        Some(tag)
    } else {
        None
    };
    profile_on(e, engine, tag.as_deref()).await
}

/// A profile naming `tag_service` (an existing tag service, possibly shared
/// with other profiles), engine-bound, with one catalog of its own attached.
async fn profile_on(e: &Env, engine: &str, tag_service: Option<&str>) -> Profile {
    let pool = &e.db.pool;
    let config = match tag_service {
        Some(tag) => json!({ "tagService": tag }),
        None => json!({}),
    };
    let svc = e
        .policy
        .create_service(&unique(&format!("ovl_{engine}")), engine, &config)
        .await
        .expect("profile");
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name) VALUES ($1,'Overlap') RETURNING id",
    )
    .bind(unique("ovl-ws"))
    .fetch_one(pool)
    .await
    .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-overlap','AKIATEST') RETURNING id",
    )
    .bind(unique("ovl-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("ovl_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,$2,'egret',CASE WHEN $2 = 'trino' THEN '{\"imageVersion\":\"483\"}'::jsonb ELSE '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb END,$3) RETURNING id",
    )
    .bind(unique("ovl-eng"))
    .bind(engine)
    .bind(svc.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("ovl_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    Profile {
        service_id: svc.id,
        catalog_id,
    }
}

async fn send(e: &Env, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={}", e.token));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = e.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn path_mask(name: &str, p: &Profile, column: &str) -> Value {
    json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": "MASK", "value_expr": null },
        "selector": { "type": "path", "catalog_id": p.catalog_id,
                      "schema": "sales", "table": "orders", "column": column },
        "exempt": { "include": [], "exclude": [] }
    })
}

async fn row(e: &Env, id: i64) -> (String, String, i64) {
    sqlx::query_as(
        "SELECT name, mechanism, policy_version FROM policy.authz_services WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&e.db.pool)
    .await
    .expect("profile row")
}

/// The Ranger download of `service`, as the plugin receives it, less the two
/// fields a version bump changes by definition.
async fn download(e: &Env, service: &str) -> (i64, Value) {
    let app = egret_policy::ranger_router(egret_policy::RangerState {
        store: e.policy.clone(),
        auth_token: Arc::from(""),
        mode: egret_policy::ranger::AuthMode::Anonymous,
        verifier: None,
    });
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/plugins/policies/download/{service}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut v: Value = serde_json::from_slice(&bytes).expect("download json");
    let obj = v.as_object_mut().unwrap();
    let version = obj.remove("policyVersion").unwrap().as_i64().unwrap();
    obj.remove("policyUpdateTime");
    (version, v)
}

#[tokio::test]
async fn a_switch_on_a_compiled_profile_bumps_the_version_once_and_keeps_the_ranger_artifact() {
    let _serial = SERIAL.lock().await;
    // Arrange: a generated Trino profile with a compiled path mask, current.
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    sqlx::query("UPDATE policy.authz_services SET origin = 'generated' WHERE id = $1")
        .bind(p.service_id)
        .execute(&e.db.pool)
        .await
        .expect("generated");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&unique("mask email"), &p, "email")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (service, mechanism, version_before) = row(&e, p.service_id).await;
    assert_eq!(mechanism, "ranger");
    let (served_before, artifact_before) = download(&e, &service).await;
    assert_eq!(served_before, version_before);
    assert!(
        artifact_before["policies"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "the compiled mask is served: {artifact_before}"
    );

    // Act
    let (status, body) = send(
        &e,
        "PATCH",
        &format!("/policies/services/{service}"),
        Some(json!({ "mechanism": "opa" })),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, mechanism, version_after) = row(&e, p.service_id).await;
    assert_eq!(mechanism, "opa");
    assert_eq!(
        version_after,
        version_before + 1,
        "exactly the switch's own bump: the real recompile wrote nothing"
    );
    let (served_after, artifact_after) = download(&e, &service).await;
    assert_eq!(served_after, version_after);
    assert_eq!(
        artifact_after, artifact_before,
        "S8: the Ranger artifact is the same, apart from its version"
    );

    // And back: the same again.
    let (status, body) = send(
        &e,
        "PATCH",
        &format!("/policies/services/{service}"),
        Some(json!({ "mechanism": "ranger" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, mechanism, version_back) = row(&e, p.service_id).await;
    assert_eq!(mechanism, "ranger");
    assert_eq!(version_back, version_after + 1);
    assert_eq!(download(&e, &service).await.1, artifact_before);
}
