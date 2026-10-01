//! Admin Policies router (`/policies`, `/principals`) HTTP tests — moved with the module
//! out of egret-controlplane in E2d. DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Auth is a session cookie (the CSRF layer is
//! applied above this router in egret-server, not here). The cross-seam engine-binding read
//! is a mock `EngineBindingReader`; the real impl (over EngineMgmtStore) and the engine
//! auto-bind round-trip are covered in egret-engine-mgmt.
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::groups::GroupRepo;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_policy::{
    policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
    PolicyStore,
};
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

/// Configurable stand-in for the engine-provisioning seam's binding read.
#[derive(Default)]
struct MockEngineReader {
    bindings: Vec<EngineBinding>,
    catalogs: HashMap<Uuid, Vec<String>>,
    /// warehouse_id -> schema names.
    schemas: HashMap<Uuid, Vec<String>>,
    /// (warehouse_id, schema) -> table names.
    tables: HashMap<(Uuid, String), Vec<String>>,
    /// (warehouse_id, schema, table) -> column names.
    columns: HashMap<(Uuid, String, String), Vec<String>>,
}

#[async_trait::async_trait]
impl EngineBindingReader for MockEngineReader {
    async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError> {
        Ok(self.bindings.clone())
    }
    async fn engine_catalog_names(&self, engine_id: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(self.catalogs.get(&engine_id).cloned().unwrap_or_default())
    }
    async fn warehouse_schemas(&self, wh: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(self.schemas.get(&wh).cloned().unwrap_or_default())
    }
    async fn warehouse_tables(
        &self,
        wh: Uuid,
        schema: &str,
    ) -> Result<Vec<String>, EngineReadError> {
        Ok(self
            .tables
            .get(&(wh, schema.to_string()))
            .cloned()
            .unwrap_or_default())
    }
    async fn warehouse_columns(
        &self,
        wh: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>, EngineReadError> {
        Ok(self
            .columns
            .get(&(wh, schema.to_string(), table.to_string()))
            .cloned()
            .unwrap_or_default())
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    groups: GroupRepo,
    sessions: SessionService,
    store: PolicyStore,
    pdp: Arc<dyn egret_core::authz::Pdp>,
}

/// Incidental PDP for `PolicyAdminState`: every existing route test
/// authenticates a `platform_admin` (whose bypass predates the PDP) and
/// asserts non-authz behavior, so an allow-all stub keeps those fixtures
/// light. Any test that asserts a 403 (`non_admin_cannot_list_policies`,
/// `scoped_permission_policies_read_and_create`) must instead build a real
/// `CedarPdp` via `attach_real_pdp` — an allow-all stub would falsely 200.
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

impl Fixture {
    /// Build the router with a given engine-binding reader.
    fn app(&self, engines: Arc<dyn EngineBindingReader>) -> axum::Router {
        policy_admin_router().with_state(PolicyAdminState {
            store: self.store.clone(),
            engines,
            users: self.users.clone(),
            groups: self.groups.clone(),
            sessions: self.sessions.clone(),
            engine_egret_url: "http://egret:8080".into(),
            pdp: self.pdp.clone(),
        })
    }
    /// Router with no bound engines (the common case).
    fn app_empty(&self) -> axum::Router {
        self.app(Arc::new(MockEngineReader::default()))
    }
}

/// Rebuild `f.pdp` with a REAL `CedarPdp`. The PDP snapshots the
/// `roles`/`role_permissions` catalog at build time and (in these tests) never
/// reloads — raw-SQL seeding emits no `RolesChanged` event — so callers MUST
/// seed any ad-hoc roles the PDP needs to know about BEFORE calling this.
async fn attach_real_pdp(f: &mut Fixture) {
    f.pdp = Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    Some(Fixture {
        users: UserRepo::new(db.clone()),
        groups: GroupRepo::new(db.clone()),
        sessions,
        store: PolicyStore::new(db.clone()),
        pdp: Arc::new(AllowAllPdp),
        db,
    })
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

/// Create a user (optionally platform_admin) and return its session token.
async fn session_for(f: &Fixture, tag: &str, platform_admin: bool) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    if platform_admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("grant platform_admin");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Create a NON-admin user holding exactly one fine-grained permission key.
async fn session_for_with_permission(f: &Fixture, tag: &str, permission: &str) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    let role_name = format!("test_role_{}", Uuid::new_v4().simple());
    let (role_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') \
         RETURNING id",
    )
    .bind(&role_name)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert role");
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
        .bind(role_id)
        .bind(permission)
        .execute(&f.db.pool)
        .await
        .expect("grant permission");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(uid.0)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("assign role");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

async fn seed_service(f: &Fixture) -> (String, i64) {
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = f
        .store
        .create_service(&name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");
    (name, svc.id)
}

#[tokio::test]
async fn platform_admin_can_list_policies_by_type() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "policies_admin", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    f.store
        .create_policy(
            svc_id,
            "allow-analysts-select",
            &serde_json::json!({
                "name": "allow-analysts-select", "policyType": 0,
                "resources": {"catalog": {"values": ["iceberg"]}, "schema": {"values": ["sales"]},
                              "table": {"values": ["orders"]}, "column": {"values": ["*"]}},
                "policyItems": [{"users": ["alice"], "groups": ["analysts"], "accesses": [{"type": "select"}]}]
            }),
        )
        .await
        .expect("create policy");

    let resp = f
        .app_empty()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}&type=access"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list policies; body={body}");
    let arr = body.as_array().expect("array body");
    assert_eq!(arr.len(), 1, "expected one summary; body={body}");
    assert_eq!(arr[0]["policy_type"], serde_json::json!("access"));
    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn non_admin_cannot_list_policies() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "policies_plain", false).await;
    // A real Cedar PDP (not the allow-all stub) must back this 403 — the
    // principal holds no platform role/permission, so the PDP denies
    // `policy:read`.
    attach_real_pdp(&mut f).await;
    let resp = f
        .app_empty()
        .oneshot(req("GET", "/policies?service=whatever", &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_create_and_delete_access_policy() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "crud", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let create = serde_json::json!({
        "name": "p-create", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });
    let resp = f
        .app_empty()
        .oneshot(req("POST", "/policies", &token, Some(create)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create policy; body={body}");
    let id = body["id"].as_i64().expect("id");

    let resp = f
        .app_empty()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}&type=access"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (_, list) = into_json(resp).await;
    assert_eq!(list.as_array().expect("array").len(), 1, "body={list}");

    let resp = f
        .app_empty()
        .oneshot(req("DELETE", &format!("/policies/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn admin_can_toggle_policy_enabled() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "toggle", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let create = serde_json::json!({
        "name": "p-toggle", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });
    let resp = f
        .app_empty()
        .oneshot(req("POST", "/policies", &token, Some(create)))
        .await
        .unwrap();
    let (_, body) = into_json(resp).await;
    let id = body["id"].as_i64().expect("id");

    let resp = f
        .app_empty()
        .oneshot(req(
            "PUT",
            &format!("/policies/{id}/enabled"),
            &token,
            Some(serde_json::json!({ "enabled": false })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = f
        .app_empty()
        .oneshot(req("GET", &format!("/policies/{id}"), &token, None))
        .await
        .unwrap();
    let (_, body) = into_json(resp).await;
    assert_eq!(body["isEnabled"], false, "body={body}");
    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn admin_can_create_masking_and_rowfilter_policies() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "mrf", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let mask = serde_json::json!({
        "kind": "masking", "name": "m1", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                      "table": {"values": ["t"]}, "column": {"values": ["email"]}},
        "items": [{"groups": ["pii"], "maskType": "MASK_HASH"}]
    });
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("POST", "/policies", &token, Some(mask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "masking; body={body}");

    let rf = serde_json::json!({
        "kind": "rowfilter", "name": "r1", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                      "table": {"values": ["t"]}},
        "items": [{"users": ["bob"], "filterExpr": "country = 'US'"}]
    });
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("POST", "/policies", &token, Some(rf)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rowfilter; body={body}");

    for (kind, _) in [("masking", 1), ("rowfilter", 2)] {
        let (_, list) = into_json(
            f.app_empty()
                .oneshot(req(
                    "GET",
                    &format!("/policies?service={svc_name}&type={kind}"),
                    &token,
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            list.as_array().expect("array").len(),
            1,
            "one {kind} summary; body={list}"
        );
    }
    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// Access P3.2 — a tag policy authored against a trino profile is stored under
/// that profile's tag service (resolved via `config.tagService`), lists under
/// the tag service, and is served by `enabled_policies_for_service`.
#[tokio::test]
async fn admin_can_create_tag_policy_under_tag_service() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "tagpol", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store).await.expect("seed tag");

    let suffix = Uuid::new_v4().simple();
    let tag_name = format!("tag-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let trino_name = format!("trino-{suffix}");
    let trino_svc = f
        .store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_name }),
        )
        .await
        .expect("create trino profile");

    // Author a tag-masking policy against the TRINO profile.
    let create = serde_json::json!({
        "kind": "tag-masking", "name": "mask-pii", "service": trino_name, "tag": "pii",
        "isEnabled": true,
        "items": [{"groups": ["pii"], "maskType": "MASK_HASH"}]
    });
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("POST", "/policies", &token, Some(create)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create tag policy; body={body}");
    let id = body["id"].as_i64().expect("id");

    // It lists under the TAG service (not the trino profile) with a tag path.
    let (status, list) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!("/policies?service={tag_name}&type=masking"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list tag policies; body={list}");
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 1, "one tag summary; body={list}");
    assert_eq!(arr[0]["resource_path"], serde_json::json!("tag: pii"));
    assert_eq!(arr[0]["service"], serde_json::json!(tag_name));

    // The trino profile itself has no policies.
    let (_, trino_list) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!("/policies?service={trino_name}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(trino_list.as_array().expect("array").len(), 0);

    // download_tags path: enabled_policies_for_service(tagService) returns it.
    let served = f
        .store
        .enabled_policies_for_service(tag_svc.id)
        .await
        .expect("enabled policies");
    assert_eq!(served.len(), 1, "served under tag service");
    assert_eq!(served[0]["serviceType"], "tag");
    assert_eq!(served[0]["resources"]["tag"]["values"][0], "pii");
    assert_eq!(served[0]["id"].as_i64(), Some(id));

    f.store.delete_service(trino_svc.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

#[tokio::test]
async fn admin_gets_trino_config_for_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cfg", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!("/policies/services/{svc_name}/trino-config"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "trino-config; body={body}");
    let files = body.as_array().expect("array");
    assert_eq!(files.len(), 2);
    assert!(files
        .iter()
        .any(|file| file["name"] == "access-control.properties"
            && file["content"]
                .as_str()
                .unwrap()
                .contains(&format!("ranger.service.name={svc_name}"))));
    assert!(files
        .iter()
        .any(|file| file["name"] == "ranger-trino-security.xml"
            && file["content"]
                .as_str()
                .unwrap()
                .contains("ranger.plugin.trino.policy.rest.url")));
    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn admin_can_list_principals() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "principals", true).await;
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("GET", "/principals?q=", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "principals; body={body}");
    assert!(
        body.as_array()
            .expect("array")
            .iter()
            .any(|p| p["kind"] == "user"),
        "expected a user principal; body={body}"
    );
}

/// THE test that matters (task-2b): the picker's whole job is to hand back
/// the engine-visible `ref`, and `ensure_username` does not merely slugify —
/// it appends `-2`, `-3`, … on collision. A fixture where every user's
/// username happens to equal the naive slug of their email passes whether or
/// not `list_principals` exposes the real, allocated value; only a SECOND
/// user colliding with an already-taken base can tell the two apart. This
/// also covers the "never allocated" state: a third user who never triggers
/// `ensure_username` must come back with `username` absent, not a guess.
#[tokio::test]
async fn list_principals_exposes_the_allocated_username_including_on_collision() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "principals_collision_admin", true).await;

    let tag = Uuid::new_v4().simple().to_string();
    // Both local-parts slugify to the SAME base ("collide-<tag>"): a `.` and a
    // `-` both collapse to a single `-` in `slugify_localpart`.
    let email1 = format!("collide.{tag}@egret.local");
    let email2 = format!("collide-{tag}@egret.local");
    let email3 = format!("collide3.{tag}@egret.local");

    let uid1 = f
        .users
        .insert_local(&email1, "Collide One", "x")
        .await
        .expect("insert user1");
    let uid2 = f
        .users
        .insert_local(&email2, "Collide Two", "x")
        .await
        .expect("insert user2");
    let _uid3 = f
        .users
        .insert_local(&email3, "Collide Three", "x")
        .await
        .expect("insert user3");

    let base = format!("collide-{tag}");
    let un1 = f
        .users
        .ensure_username(uid1)
        .await
        .expect("ensure username 1");
    assert_eq!(un1, base, "first allocation is the bare slug");
    let un2 = f
        .users
        .ensure_username(uid2)
        .await
        .expect("ensure username 2");
    assert_eq!(un2, format!("{base}-2"), "collision must append -2");
    // user3 deliberately never calls ensure_username.

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("GET", &format!("/principals?q={tag}"), &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "principals; body={body}");
    let arr = body.as_array().expect("array");

    let row1 = arr
        .iter()
        .find(|p| p["name"] == email1)
        .unwrap_or_else(|| panic!("user1 missing from principals; body={body}"));
    assert_eq!(row1["username"], base, "row1; body={body}");

    let row2 = arr
        .iter()
        .find(|p| p["name"] == email2)
        .unwrap_or_else(|| panic!("user2 missing from principals; body={body}"));
    assert_eq!(
        row2["username"],
        format!("{base}-2"),
        "collision suffix must be exposed, not the naive slug; body={body}"
    );

    let row3 = arr
        .iter()
        .find(|p| p["name"] == email3)
        .unwrap_or_else(|| panic!("user3 missing from principals; body={body}"));
    assert!(
        row3["username"].is_null(),
        "a user who never allocated a username must come back with none, not a guess; body={body}"
    );
}

#[tokio::test]
async fn platform_admin_can_lookup_resource_values() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "resource_lookup_admin", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    f.store
        .create_policy(
            svc_id,
            "allow-analysts-select",
            &serde_json::json!({
                "name": "allow-analysts-select", "policyType": 0,
                "resources": {"catalog": {"values": ["iceberg"]}, "schema": {"values": ["sales"]}},
                "policyItems": [{"users": ["alice"], "accesses": [{"type": "select"}]}]
            }),
        )
        .await
        .expect("create policy");

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!(
                    "/policies/resource-lookup?service={svc_name}&level=schema&catalog=iceberg"
                ),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resource-lookup; body={body}");
    assert!(
        body.as_array().expect("array").iter().any(|v| v == "sales"),
        "expected 'sales'; body={body}"
    );

    // parent-filter mismatch excludes it.
    let (_, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!("/policies/resource-lookup?service={svc_name}&level=schema&catalog=other"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !body.as_array().expect("array").iter().any(|v| v == "sales"),
        "parent mismatch should exclude 'sales'; body={body}"
    );
    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// `list_profiles` surfaces the engines the `EngineBindingReader` reports as
/// bound to a profile (the cross-seam read the admin UI shows).
#[tokio::test]
async fn list_profiles_shows_bound_engines_from_reader() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "profiles", true).await;
    let (_svc_name, svc_id) = seed_service(&f).await;
    let engine_id = Uuid::new_v4();
    let reader = Arc::new(MockEngineReader {
        bindings: vec![EngineBinding {
            id: engine_id,
            name: "trino-prod".into(),
            status: "Running".into(),
            authz_service_id: Some(svc_id),
            warehouse_ids: Vec::new(),
        }],
        ..Default::default()
    });

    let (status, body) = into_json(
        f.app(reader)
            .oneshot(req("GET", "/policies/services", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list profiles; body={body}");
    let profile = body
        .as_array()
        .expect("array")
        .iter()
        .find(|p| p["id"] == serde_json::json!(svc_id))
        .unwrap_or_else(|| panic!("profile {svc_id} not found; body={body}"));
    assert!(
        profile["engines"]
            .as_array()
            .expect("engines")
            .iter()
            .any(|e| e["id"] == serde_json::json!(engine_id)),
        "bound engine should appear under its profile; body={body}"
    );
    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// POST /policies/services — the manual create-profile endpoint (StarRocks has
/// no provisioner auto-bind, so an admin creates its profile directly).
#[tokio::test]
async fn create_profile_makes_a_starrocks_service() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "create_profile", true).await;
    egret_policy::seed_starrocks(&f.store)
        .await
        .expect("seed starrocks def");

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies/services",
                &token,
                Some(serde_json::json!({ "name": "sr1", "type": "starrocks" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create profile; body={body}");

    let svc = f.store.get_service("sr1").await.expect("service created");
    assert_eq!(svc.r#type, "starrocks");
    f.store.delete_service(svc.id).await.expect("cleanup");
}

/// SP4d debt Task 2 — **the reachable surface.** `POST /policies/services`
/// stores `config` verbatim, so it is the one route that can hand a StarRocks
/// profile a `tagService`. That profile would download tag attachments naming the
/// resource level `schema`, which StarRocks' service-def does not have (its
/// middle level is `database`), Ranger's own `RangerTagEnricher` drops every one
/// of them, and every tag policy in the profile's `tagPolicies` block is
/// SILENTLY inert — no masking, no row filtering, no error.
///
/// Asserted here as a 400 whose message NAMES the missing level (not merely
/// "invalid"), and with the row absent afterwards: a refusal that still inserted
/// would be the same silence with a nicer log line. The Trino arm is asserted in
/// the same test so an over-refusal cannot hide behind the StarRocks one — that
/// combination is exactly what `bind_trino_profile` writes on every engine
/// create.
#[tokio::test]
async fn create_profile_refuses_a_tag_service_on_a_starrocks_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sr_tagsvc", true).await;
    egret_policy::seed_starrocks(&f.store)
        .await
        .expect("seed starrocks def");
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store)
        .await
        .expect("seed tag def");

    let tag_name = format!("tsg-{}", Uuid::new_v4().simple());
    let tag_svc = f
        .store
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");

    let sr_name = format!("sr-tagsvc-{}", Uuid::new_v4().simple());
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies/services",
                &token,
                Some(serde_json::json!({
                    "name": sr_name,
                    "type": "starrocks",
                    "config": { "tagService": tag_name },
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a starrocks profile carrying a tagService must be refused; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("schema"),
        "the 400 must name the level StarRocks lacks; body={body}"
    );
    assert!(
        msg.contains("database"),
        "the 400 must name StarRocks' actual middle level; body={body}"
    );
    assert!(
        msg.contains("tagService"),
        "the 400 must name the config key to remove; body={body}"
    );
    assert!(
        f.store.get_service(&sr_name).await.is_err(),
        "the refused profile must NOT have been inserted"
    );

    // Not an over-refusal: the Trino arm is the shape `bind_trino_profile` writes.
    let trino_name = format!("tr-tagsvc-{}", Uuid::new_v4().simple());
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies/services",
                &token,
                Some(serde_json::json!({
                    "name": trino_name,
                    "type": "trino",
                    "config": { "tagService": tag_name },
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "trino + tagService; body={body}");
    let trino_svc = f.store.get_service(&trino_name).await.expect("created");
    f.store.delete_service(trino_svc.id).await.expect("cleanup");

    // And a StarRocks profile with NO tagService still creates.
    let sr_ok = format!("sr-plain-{}", Uuid::new_v4().simple());
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies/services",
                &token,
                Some(serde_json::json!({ "name": sr_ok, "type": "starrocks" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "starrocks, no tagService; body={body}"
    );
    let sr_svc = f.store.get_service(&sr_ok).await.expect("created");
    f.store.delete_service(sr_svc.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

/// DELETE /policies/services/{service} — an unbound profile deletes, taking its
/// policies with it via `ON DELETE CASCADE` (a seeded policy proves no FK 500).
#[tokio::test]
async fn admin_can_delete_unbound_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "delete_profile", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    f.store
        .create_policy(svc_id, "p1", &serde_json::json!({ "name": "p1" }))
        .await
        .expect("seed policy");

    let resp = f
        .app_empty()
        .oneshot(req(
            "DELETE",
            &format!("/policies/services/{svc_name}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        f.store.get_service(&svc_name).await.is_err(),
        "profile should be gone"
    );
}

/// DELETE is refused (409) while an engine still enforces the profile — deleting
/// it would silently strip that engine's access control.
#[tokio::test]
async fn delete_profile_refused_while_engine_bound() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "delete_profile_bound", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let reader = Arc::new(MockEngineReader {
        bindings: vec![EngineBinding {
            id: Uuid::new_v4(),
            name: "trino-prod".into(),
            status: "Running".into(),
            authz_service_id: Some(svc_id),
            warehouse_ids: Vec::new(),
        }],
        ..Default::default()
    });

    let (status, body) = into_json(
        f.app(reader)
            .oneshot(req(
                "DELETE",
                &format!("/policies/services/{svc_name}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "bound profile delete refused; body={body}"
    );
    assert!(
        f.store.get_service(&svc_name).await.is_ok(),
        "profile must survive a refused delete"
    );
    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// resource-lookup surfaces LIVE catalog metadata (real schemas/tables from the
/// bound engine's warehouse), not only values already used in existing policies.
#[tokio::test]
async fn resource_lookup_suggests_live_catalog_metadata() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reslookup", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let wh = Uuid::new_v4();
    let mut schemas = HashMap::new();
    schemas.insert(wh, vec!["sales".to_string(), "hr".to_string()]);
    let mut tables = HashMap::new();
    tables.insert(
        (wh, "sales".to_string()),
        vec!["orders".to_string(), "customers".to_string()],
    );
    let reader = Arc::new(MockEngineReader {
        bindings: vec![EngineBinding {
            id: Uuid::new_v4(),
            name: "trino-prod".into(),
            status: "Running".into(),
            authz_service_id: Some(svc_id),
            warehouse_ids: vec![wh],
        }],
        schemas,
        tables,
        ..Default::default()
    });

    // schema level → the warehouse's live namespaces.
    let (status, body) = into_json(
        f.app(reader.clone())
            .oneshot(req(
                "GET",
                &format!("/policies/resource-lookup?service={svc_name}&level=schema"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "schema lookup; body={body}");
    let vals: Vec<String> = serde_json::from_value(body.clone()).unwrap_or_default();
    assert!(
        vals.contains(&"sales".to_string()) && vals.contains(&"hr".to_string()),
        "live schemas surfaced; body={body}"
    );

    // table level scoped to schema=sales → that namespace's live tables.
    let (status, body) = into_json(
        f.app(reader)
            .oneshot(req(
                "GET",
                &format!("/policies/resource-lookup?service={svc_name}&level=table&schema=sales"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "table lookup; body={body}");
    let vals: Vec<String> = serde_json::from_value(body.clone()).unwrap_or_default();
    assert!(
        vals.contains(&"orders".to_string()) && vals.contains(&"customers".to_string()),
        "live tables surfaced; body={body}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// The picker offers the schemas of EVERY warehouse a bound engine reads, not
/// only the one it leads with (SP3c-2 Task 5). An engine has attached several
/// egret catalogs — each with its own warehouse — since SP3a; `EngineBinding`
/// carried a single `warehouse_id` off the `engines` column and was the last
/// place still pretending otherwise, so a policy author writing a rule for the
/// engine's SECOND catalog got no suggestions at all.
///
/// The assertion that can fail either way: `second` must be offered (the
/// widening) AND `first` must still be (the old behaviour, not replaced).
#[tokio::test]
async fn resource_lookup_suggests_every_warehouse_the_engine_reads() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reslookup_multi", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let primary_wh = Uuid::new_v4();
    let second_wh = Uuid::new_v4();
    let mut schemas = HashMap::new();
    schemas.insert(primary_wh, vec!["from_primary".to_string()]);
    schemas.insert(second_wh, vec!["from_second".to_string()]);
    let reader = Arc::new(MockEngineReader {
        bindings: vec![EngineBinding {
            id: Uuid::new_v4(),
            name: "trino-prod".into(),
            status: "Running".into(),
            authz_service_id: Some(svc_id),
            warehouse_ids: vec![primary_wh, second_wh],
        }],
        schemas,
        ..Default::default()
    });

    let (status, body) = into_json(
        f.app(reader)
            .oneshot(req(
                "GET",
                &format!("/policies/resource-lookup?service={svc_name}&level=schema"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "schema lookup; body={body}");
    let vals: Vec<String> = serde_json::from_value(body.clone()).unwrap_or_default();
    assert!(
        vals.contains(&"from_primary".to_string()),
        "the primary's schemas are still offered; body={body}"
    );
    assert!(
        vals.contains(&"from_second".to_string()),
        "a non-primary attachment's schemas are offered too; body={body}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// list_profiles surfaces a trino profile's `<name>_tag` policy service (drives
/// the FE Tags tab), and only when that service actually exists.
#[tokio::test]
async fn list_profiles_surfaces_tag_service() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "tagsvc", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let tag_name = format!("{svc_name}_tag");
    let tag_svc = f
        .store
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("GET", "/policies/services", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list profiles; body={body}");
    let profile = body
        .as_array()
        .expect("array")
        .iter()
        .find(|p| p["id"] == serde_json::json!(svc_id))
        .unwrap_or_else(|| panic!("profile {svc_id} not found; body={body}"));
    assert_eq!(
        profile["tag_service"],
        serde_json::json!(tag_name),
        "tag_service surfaced; body={body}"
    );

    f.store.delete_service(tag_svc.id).await.ok();
    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// Policies: `policy:read` gates `GET /policies`, `policy:create` gates
/// `POST /policies`. Proves the NEW `resource:action` gate: a scoped,
/// non-admin principal holding the wrong (or no) permission is Forbidden, and
/// one holding exactly the right permission succeeds — without the superuser
/// bypass.
#[tokio::test]
async fn scoped_permission_policies_read_and_create() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc_name, svc_id) = seed_service(&f).await;

    // Seed all three ad-hoc platform roles BEFORE the real Cedar PDP snapshots
    // the roles catalog (raw-SQL seeding emits no RolesChanged event, so the
    // PDP never reloads). This is the required real-PDP authz assertion: a
    // non-platform_admin `policy:read` principal lists policies (200) but is
    // denied create (403), resolved by a real `CedarPdp` — not the allow-all
    // stub.
    let wrong = session_for_with_permission(&f, "policy_scoped_wrong", "engine:read").await;
    let reader = session_for_with_permission(&f, "policy_scoped_reader", "policy:read").await;
    let creator = session_for_with_permission(&f, "policy_scoped_creator", "policy:create").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app_empty()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "without policy:read must not list"
    );

    let (status, _) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!("/policies?service={svc_name}"),
                &reader,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "policy:read must list");

    let create_body = serde_json::json!({
        "name": "scoped-create", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });
    let resp = f
        .app_empty()
        .oneshot(req("POST", "/policies", &reader, Some(create_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "policy:read alone must not create"
    );

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("POST", "/policies", &creator, Some(create_body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "policy:create must create; body={body}"
    );
    f.store.delete_service(svc_id).await.expect("cleanup");
}
