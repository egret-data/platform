//! Governance events for hand-written Ranger policies (DG #3, spec §3.2):
//! create, update, delete and the enabled toggle each record one
//! `ranger_policy:*` event on their own transaction; a write the
//! compiler-owned guard refuses records none.
//!
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set.
//! Every read is scoped to the policy ids a test created.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::groups::GroupRepo;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_policy::{
    policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
    PolicyStore,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct NoEngines;
#[async_trait::async_trait]
impl EngineBindingReader for NoEngines {
    async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn engine_catalog_names(&self, _e: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn warehouse_schemas(&self, _w: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn warehouse_tables(&self, _w: Uuid, _s: &str) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn warehouse_columns(
        &self,
        _w: Uuid,
        _s: &str,
        _t: &str,
    ) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
}

struct NoRecompile;
#[async_trait::async_trait]
impl egret_policy::ProfileRecompiler for NoRecompile {
    async fn recompile_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        _service_ids: &[i64],
    ) -> Result<(), egret_policy::PolicyHttpError> {
        Ok(())
    }
}

struct Fixture {
    db: Db,
    app: axum::Router,
    store: PolicyStore,
    token: String,
    actor: Uuid,
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&store).await.expect("seed trino");
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let uid = users
        .insert_local(
            &format!("polev_{}@egret.local", Uuid::new_v4().simple()),
            "Test",
            "x",
        )
        .await
        .expect("insert user");
    // A steward holding the three policy permissions the routes require.
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(format!("polev_{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("role");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission) \
         SELECT $1, unnest(ARRAY['policy:create','policy:update','policy:delete'])",
    )
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("permissions");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(uid.0)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("access grant");
    let token = sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let app = policy_admin_router().with_state(PolicyAdminState {
        store: store.clone(),
        engines: Arc::new(NoEngines),
        users,
        groups: GroupRepo::new(db.clone()),
        sessions,
        engine_egret_url: "http://egret:8080".into(),
        pdp,
        recompiler: Arc::new(NoRecompile),
    });
    Some(Fixture {
        db,
        app,
        store,
        token,
        actor: uid.0,
    })
}

async fn send(f: &Fixture, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={}", f.token));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = f.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[derive(Debug, sqlx::FromRow)]
struct Event {
    kind: String,
    actor_user_id: Option<Uuid>,
    action: String,
    resource_type: String,
    detail: Value,
}

async fn events(f: &Fixture, policy_id: i64) -> Vec<Event> {
    sqlx::query_as(
        "SELECT kind, actor_user_id, action, resource_type, detail FROM audit.audit_log \
          WHERE resource_type = 'ranger_policy' AND resource_id = $1 ORDER BY id",
    )
    .bind(policy_id.to_string())
    .fetch_all(&f.db.pool)
    .await
    .expect("events")
}

async fn profile(f: &Fixture) -> String {
    let name = format!("polev-{}", Uuid::new_v4().simple());
    f.store
        .create_service(&name, "trino", &json!({}))
        .await
        .expect("profile");
    name
}

fn access(service: &str, name: &str, catalogs: &[String], users: &[&str]) -> Value {
    json!({
        "name": name, "service": service, "isEnabled": true,
        "resources": {"catalog": {"values": catalogs}},
        "items": [{"users": users, "accesses": ["select"], "effect": "allow"}]
    })
}

#[tokio::test]
async fn create_update_toggle_delete_each_record_one_event() {
    let Some(f) = setup().await else { return };
    let svc = profile(&f).await;
    let name = format!("p-{}", Uuid::new_v4().simple());
    // 25 catalogs: the summary keeps 20.
    let catalogs: Vec<String> = (0..25).map(|i| format!("c{i:02}")).collect();
    let (status, body) = send(
        &f,
        "POST",
        "/policies",
        Some(access(&svc, &name, &catalogs, &["alice"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_i64().expect("id");

    let evs = events(&f, id).await;
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(evs[0].kind, "mutation");
    assert_eq!(evs[0].action, "ranger_policy:create");
    assert_eq!(evs[0].actor_user_id, Some(f.actor));
    let created = evs[0].detail["after"].clone();
    assert_eq!(evs[0].detail["before"], Value::Null);
    assert_eq!(created["service"], json!(svc));
    assert_eq!(created["name"], json!(name));
    assert_eq!(created["policy_type"], "access");
    assert_eq!(created["enabled"], true);
    let listed = created["resources"]["catalog"]
        .as_array()
        .expect("catalog values");
    assert_eq!(listed.len(), 20, "{created}");
    assert_eq!(listed[0], "c00");
    assert_eq!(created["resources_truncated"], true, "{created}");

    let (status, body) = send(
        &f,
        "PUT",
        &format!("/policies/{id}"),
        Some(access(&svc, &name, &["ice".to_string()], &["bob"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evs = events(&f, id).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].action, "ranger_policy:update");
    assert_eq!(evs[1].detail["before"], created);
    let updated = evs[1].detail["after"].clone();
    assert_eq!(updated["resources"]["catalog"], json!(["ice"]));
    assert_eq!(updated.get("resources_truncated"), None, "{updated}");

    let (status, _) = send(
        &f,
        "PUT",
        &format!("/policies/{id}/enabled"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let evs = events(&f, id).await;
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert_eq!(evs[2].action, "ranger_policy:disable");
    assert_eq!(evs[2].detail["before"]["enabled"], true);
    assert_eq!(evs[2].detail["after"]["enabled"], false);
    let disabled = evs[2].detail["after"].clone();

    let (status, _) = send(
        &f,
        "PUT",
        &format!("/policies/{id}/enabled"),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let evs = events(&f, id).await;
    assert_eq!(evs.len(), 4, "{evs:?}");
    assert_eq!(evs[3].action, "ranger_policy:enable");
    let enabled = evs[3].detail["after"].clone();

    let (status, _) = send(&f, "DELETE", &format!("/policies/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let evs = events(&f, id).await;
    assert_eq!(evs.len(), 5, "{evs:?}");
    assert_eq!(evs[4].action, "ranger_policy:delete");
    assert_eq!(evs[4].resource_type, "ranger_policy");
    assert_eq!(evs[4].detail, json!({"before": enabled, "after": null}));
    assert_ne!(disabled, enabled);
}

#[tokio::test]
async fn a_write_the_compiler_owned_guard_refuses_records_nothing() {
    let Some(f) = setup().await else { return };
    let svc = profile(&f).await;
    sqlx::query("UPDATE policy.authz_services SET origin = 'generated' WHERE name = $1")
        .bind(&svc)
        .execute(&f.db.pool)
        .await
        .unwrap();
    let owned = format!("perm-grant-{}", Uuid::new_v4());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         SELECT id, $2, $3, 'compiler' FROM policy.authz_services WHERE name = $1 RETURNING id",
    )
    .bind(&svc)
    .bind(&owned)
    .bind(json!({
        "name": owned, "policyType": 0,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "policyItems": [{"users": ["alice"], "accesses": [{"type": "select"}]}]
    }))
    .fetch_one(&f.db.pool)
    .await
    .expect("a compiler-owned policy");

    let (status, body) = send(&f, "DELETE", &format!("/policies/{id}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = send(
        &f,
        "PUT",
        &format!("/policies/{id}/enabled"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = send(
        &f,
        "PUT",
        &format!("/policies/{id}"),
        Some(access(&svc, &owned, &["ice".to_string()], &["bob"])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = send(
        &f,
        "POST",
        "/policies",
        Some(access(&svc, "fresh", &["ice".to_string()], &["bob"])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(events(&f, id).await.is_empty());
    let any_on_profile: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.audit_log \
          WHERE resource_type = 'ranger_policy' AND detail::text LIKE '%' || $1 || '%'",
    )
    .bind(&svc)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(any_on_profile, 0);
}
