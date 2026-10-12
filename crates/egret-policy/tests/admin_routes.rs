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
    // The whole response, byte for byte (profile name normalised to `SVC`).
    let content = |name: &str| {
        files
            .iter()
            .find(|file| file["name"] == name)
            .and_then(|file| file["content"].as_str())
            .unwrap_or_default()
            .replace(&svc_name, "SVC")
    };
    let (props, xml) = (
        content("access-control.properties"),
        content("ranger-trino-security.xml"),
    );
    assert_eq!(
        props,
        include_str!("golden/trino-config.access-control.properties")
    );
    assert_eq!(
        xml,
        include_str!("golden/trino-config.ranger-trino-security.xml")
    );
    // What changed against the response captured BEFORE the route moved onto the
    // shared renderer (`golden/pre-sp4d4-*`): every property the old file set is
    // still set, and the old file's keys are a subset of the new one's.
    let old_xml = include_str!("golden/pre-sp4d4-trino-config.ranger-trino-security.xml");
    for name in old_xml.split("<name>").skip(1) {
        let key = name.split("</name>").next().unwrap();
        assert!(xml.contains(&format!("<name>{key}</name>")), "lost {key}");
    }
    assert_ne!(
        props,
        include_str!("golden/pre-sp4d4-trino-config.access-control.properties")
    );
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

/// list_profiles surfaces the tag policy service a trino profile's OWN
/// `config.tagService` names (drives the FE Tags tab), and only when that
/// service actually exists.
///
/// **⚠ THIS ASSERTION CHANGED DIRECTION — SP4d debt Task 9, fix round.** It
/// previously created `<svc_name>_tag` and asserted that `list_profiles`
/// surfaced it from a profile carrying **no `tagService` at all**, pinning the
/// `<name>_tag` NAMING-CONVENTION fallback as desired behaviour. It is not
/// desired behaviour:
///
/// * nothing can ever give an existing profile a `tagService` — `createProfile`
///   sends only name and type, and no `UPDATE` of `config_jsonb` exists in any
///   `src/` (Task 2's sweep, re-measured 2026-10-10) — so the Tags tab that
///   fallback produced led to a workflow that **cannot complete, ever**: the
///   write path (`resolve_tag_service_and_body`) requires the configured link
///   and answers 400. A tab that leads to a guaranteed 400 is worse than no tab,
///   because it tells a steward the capability exists;
/// * the fallback's stated purpose was WRITE safety, and the write side no
///   longer needs it.
///
/// So the premise moved: the profile now carries the link, and the test asserts
/// that `list_profiles` reports what `usable_tag_service` and the plugin
/// download report. The reverse half — a profile with no link and a
/// conventionally-named row beside it gets **no** tab — is asserted in
/// `a_tag_policy_is_refused_when_the_profile_configures_no_tag_service`'s
/// control 2, which changed direction with it.
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
    // The link itself — the thing that makes the tag service USABLE, and the
    // only reason the tab is now offered. `seed_service` creates the profile
    // with an empty config, so this is the `UPDATE` no production code path has
    // (hence the raw SQL: see this test's doc comment).
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(svc_id)
        .bind(serde_json::json!({ "tagService": tag_name }))
        .execute(&f.db.pool)
        .await
        .expect("link the tag service");

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

/// **SP4d debt Task 5: the POLICY EDITOR is the second surface that can author
/// the mask half, and on this host it is the one that actually did.**
///
/// A tag masking policy (`POST /policies`, `kind: "tag-masking"`) plus a
/// TABLE-level attachment of that tag makes the engine type-check the one mask
/// against every column of the scanned table, so the table becomes unreadable
/// rather than masked — measured live on Trino 470 during SP4d-2b's live gate.
/// When that gate swept this development host, every live instance of the pair
/// it found was an editor policy and none was a `perm.rules` row — which is why
/// guarding only `POST /perm/rules` would have left the reachable half open.
/// **The count is deliberately not restated here.** The sweep's database
/// (`egret_sp4d2b`) has since been dropped, and a re-measurement on 2026-10-10
/// finds 0 live pairs across the 9 databases that carry both schemas, so the
/// figure cannot be re-derived; `egret_policy::unreadable_table_mask`'s module
/// doc carries what remains checkable. The reach of this test does not depend
/// on it either way: the editor surface is reachable from the browser, so it
/// needs a guard whether or not anyone has used it yet.
///
/// Four assertions, and the last two are what a DB-level test cannot make: the
/// refusal is a 400, it carries all three remedies (Task 16 added the third),
/// nothing was stored under the tag
/// service, and the same policy authored as a tag ROW FILTER — which carries no
/// column type check — is still accepted through the same route.
#[tokio::test]
async fn a_tag_masking_policy_is_refused_when_its_tag_is_on_a_whole_table() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t5pol", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store).await.expect("seed tag");

    let suffix = Uuid::new_v4().simple().to_string();
    let tag_svc_name = format!("t5tag-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_svc_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let trino_name = format!("t5trino-{suffix}");
    let trino_svc = f
        .store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_svc_name }),
        )
        .await
        .expect("create trino profile");

    // A classification tag, attached to the WHOLE table. The tag's `name` is
    // what a tag policy addresses (D6); its `key` is deliberately a different
    // string so a key/name confusion in the guard cannot read as a pass.
    let tag_key = format!("t5-k-{suffix}");
    let tag_name = format!("T5-N-{suffix}");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(&tag_key)
            .bind(&tag_name)
            .fetch_one(&f.db.pool)
            .await
            .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 'sales', 'orders', '')",
    )
    .bind(tag_id)
    .bind(Uuid::new_v4())
    .execute(&f.db.pool)
    .await
    .expect("table-level attachment");

    let policy_name = format!("t5-mask-{suffix}");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": policy_name,
                    "service": trino_name, "tag": tag_name, "isEnabled": true,
                    "items": [{"groups": ["analysts"], "maskType": "MASK_HASH"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a tag masking policy over a table-level tag must be refused; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("sales.orders") && msg.contains(&policy_name),
        "the refusal must name the table and the policy; got {msg}"
    );
    // SP4d debt Task 16 changed this from BOTH to ALL THREE: `PUT
    // /policies/{id}` now has a tag branch, so "edit the policy so it addresses
    // the table by path" exists as a remedy and the shared `refusal()` names it.
    // Before that branch the remedy for an editor-origin pair was
    // delete-and-recreate, which is why the sentence had only two.
    assert!(
        msg.contains("to the columns that need masking instead of to the table")
            && msg.contains("by catalog/schema/table/column path instead of by tag")
            && msg.contains("so it addresses sales.orders by path"),
        "the refusal must carry all THREE remedies; got {msg}"
    );
    assert_eq!(
        f.store
            .enabled_policies_for_service(tag_svc.id)
            .await
            .expect("enabled policies")
            .len(),
        0,
        "a refused policy must not have been stored under the tag service"
    );

    // ⚠ The accept-control, and it is the one that keeps this guard from being
    // "no tag policy may name a table-level tag": a tag ROW FILTER is
    // table-granular on both engines by construction, so a table-level
    // attachment is its natural match. Refusing it would withdraw a working
    // control.
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(serde_json::json!({
                    "kind": "tag-rowfilter", "name": format!("t5-filter-{suffix}"),
                    "service": trino_name, "tag": tag_name, "isEnabled": true,
                    "items": [{"groups": ["analysts"], "filterExpr": "1=1"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a tag ROW FILTER over the same table-level tag must still be accepted; body={body}"
    );

    f.store.delete_service(trino_svc.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// SP4d debt Task 6, item 1 — the POLICY EDITOR's half of the one rule.
// ---------------------------------------------------------------------------
//
// The compiler's half is `egret-perm-compile/tests/recompile.rs`. This is the
// surface a steward reaches by hand, and a route handler's guard is not covered
// by `--lib` — so it is exercised through the real `POST`/`PUT /policies`
// handlers, not through `refuse_ambiguous_arbitration` directly.

/// A second row filter on a table that already has one, with a DIFFERENT
/// expression, must be refused — SP4d-1 §8's F2 shape, authored by hand.
///
/// ⚠ Three accept-controls sit in the same test, because a refusal that fires
/// for the wrong reason looks identical to one that fires for the right one:
/// the FIRST row filter must be accepted (there is nothing to tie with), an
/// AGREEING second one must be accepted, and a second one on a DIFFERENT table
/// must be accepted.
#[tokio::test]
async fn a_second_row_filter_on_one_table_is_refused_only_when_it_disagrees() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ambigrf", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let filter = |name: &str, table: &str, expr: &str| {
        serde_json::json!({
            "kind": "rowfilter", "name": name, "service": svc_name, "isEnabled": true,
            "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                          "table": {"values": [table]}},
            "items": [{"groups": ["public"], "filterExpr": expr}]
        })
    };
    let post = |body: serde_json::Value| {
        let app = f.app_empty();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req("POST", "/policies", &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        }
    };

    // Control 1: the first one has nothing to tie with.
    let (status, body) = post(filter("rf-us", "orders", "country = 'US'")).await;
    assert_eq!(status, StatusCode::OK, "the first row filter; body={body}");

    // Control 2: a DIFFERENT table is a different resource, so a different band.
    let (status, body) = post(filter("rf-other", "invoices", "id > 2")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a filter on another table; body={body}"
    );
    let other_id = body["id"].as_i64().expect("id");

    // Control 3: an AGREEING second filter on the SAME table is harmless —
    // whichever evaluator runs first, the same rows are withheld.
    let (status, body) = post(filter("rf-us-again", "orders", "country = 'US'")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an agreeing tie must be accepted; body={body}"
    );

    // The real case: same table, same priority, DIFFERENT expression.
    let (status, body) = post(filter("rf-id", "orders", "id > 2")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a disagreeing second filter on one table must be refused with 409 (SP4d-4 \
         Part A: the body is well-formed, it conflicts with a stored policy); body={body}"
    );
    let msg = body["error"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        msg.contains("row filter") && msg.contains("table=[orders]"),
        "the refusal must name the decision and the resource: {body}"
    );
    assert!(
        msg.contains("\"rf-id\""),
        "the refusal must name the policy being written: {body}"
    );
    assert!(
        msg.contains("or set Override on the one that should win"),
        "the editor's refusal must offer Override, the remedy only a hand-written \
         policy has: {body}"
    );

    // ⚠ **A PUT must not pair the new body against the row it REPLACES.**
    // `rf-other` is the ONLY filter on `invoices`, so changing its expression
    // is one policy changing and nothing else is in its band — the only thing
    // that can tie with it is its own stored version. Without
    // `refuse_ambiguous_arbitration`'s `replacing` argument this 409s, and
    // every retry 409s the same way, so the steward can never edit a row
    // filter's expression again. Deliberately done on `invoices` and not on
    // `orders`: editing `rf-us` while `rf-us-again` still carries the old
    // expression really WOULD create a new disagreeing pair, so that version
    // of this assertion would have passed for the wrong reason.
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{other_id}"),
                &token,
                Some(filter("rf-other", "invoices", "id > 7")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a PUT must exclude the row it replaces from the comparison; body={body}"
    );
    // …and the edit really landed, so the 200 is not a no-op.
    let (_, stored) = into_json(
        f.app_empty()
            .oneshot(req("GET", &format!("/policies/{other_id}"), &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        stored.to_string().contains("id > 7"),
        "the PUT must have replaced the expression; body={stored}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// **SP4d-4 Task 2 review I2: the editor refuses only a pair the write
/// CREATES.** Part A promises that "the writes that remove the pair still
/// succeed — deleting or narrowing one rule". With the editor refusing ANY pair
/// in the profile, two stored pairs deadlocked it: narrowing a member of one
/// was refused on the other.
///
/// The two stored pairs are built the one way a route can still build them —
/// create the second half DISABLED (a disabled policy is in no band), then
/// re-enable it through the deliberately unguarded `PUT …/enabled`.
///
/// ⚠ Refusal controls in the same test: a NEW policy overlapping a stored pair
/// is still refused, and so is moving the narrowed policy back onto its old
/// partner (the pair is new again, because it no longer exists in BEFORE).
#[tokio::test]
async fn narrowing_one_stored_pair_succeeds_while_another_pair_is_stored() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ambigpairs", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let filter = |name: &str, table: &str, expr: &str, enabled: bool| {
        serde_json::json!({
            "kind": "rowfilter", "name": name, "service": svc_name, "isEnabled": enabled,
            "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                          "table": {"values": [table]}},
            "items": [{"groups": ["public"], "filterExpr": expr}]
        })
    };
    let send = |method: &'static str, uri: String, body: serde_json::Value| {
        let app = f.app_empty();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req(method, &uri, &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        }
    };

    // Two stored differing pairs: (pa, pb) on t1 and (pc, pd) on t2.
    let mut ids = std::collections::BTreeMap::new();
    for (name, table, expr) in [
        ("pa", "t1", "x = 1"),
        ("pb", "t1", "x = 2"),
        ("pc", "t2", "y = 1"),
        ("pd", "t2", "y = 2"),
    ] {
        let (status, body) =
            send("POST", "/policies".into(), filter(name, table, expr, false)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "create {name} disabled; body={body}"
        );
        ids.insert(name, body["id"].as_i64().expect("id"));
    }
    for name in ["pa", "pb", "pc", "pd"] {
        let (status, body) = send(
            "PUT",
            format!("/policies/{}/enabled", ids[name]),
            serde_json::json!({"enabled": true}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "enable {name}; body={body}");
    }

    // The write that REMOVES (pa, pb): narrow pa onto t3. Before the fix this
    // was refused on the unrelated (pc, pd).
    let (status, body) = send(
        "PUT",
        format!("/policies/{}", ids["pa"]),
        filter("pa", "t3", "x = 1", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "narrowing one member of a stored pair must succeed while another stored pair \
         exists; body={body}"
    );

    // Refusal control 1: a NEW policy overlapping the stored (pc, pd).
    let (status, body) = send(
        "POST",
        "/policies".into(),
        filter("pe", "t2", "y = 9", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a new overlapping policy must still be refused; body={body}"
    );
    let msg = body.to_string();
    assert!(
        msg.contains("\\\"pe\\\"") && msg.contains("or set Override on the one that should win"),
        "the refusal must name the policy being written and offer Override: {body}"
    );

    // Refusal control 2: moving pa back onto t1 re-creates (pa, pb).
    let (status, body) = send(
        "PUT",
        format!("/policies/{}", ids["pa"]),
        filter("pa", "t1", "x = 1", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "re-creating a removed pair must be refused; body={body}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// SP4d debt Task 9 — the same "silently inert" symptom on the LEGITIMATE type.
// ---------------------------------------------------------------------------
//
// Task 2's guards test key PRESENCE, deliberately, to agree with migration
// 0176's `->>`. So `{"tagService": ""}` was accepted, and a Trino profile with
// no `tagService` at all was accepted too — and then the readers of that field
// disagreed: the tag editor's write target fell back to `{profile}_tag` and
// filed policies there, while `download_policies` read the field with no
// fallback and shipped the download with NO `tagPolicies` block, so every one
// of those policies was inert. These are route tests because a route handler's
// guard is not covered by `--lib`.

/// Step 4: the VALUE shape is refused at the profile write path, and the row is
/// absent afterwards — a refusal that still inserted would be the same silence
/// with a nicer log line.
///
/// ⚠ The accept-controls are in the same test. An empty, whitespace-only or
/// non-string `tagService` is refused; a REAL tag profile name (what
/// `bind_trino_profile` writes) and a profile with no `tagService` key at all
/// are both still created. Without those, "refuses everything" would pass.
#[tokio::test]
async fn create_profile_refuses_a_tag_service_that_names_nothing() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t9_tsval", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store)
        .await
        .expect("seed tag def");

    let suffix = Uuid::new_v4().simple().to_string();
    let tag_name = format!("t9ts-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");

    // Every shape that reads as "unset" through `as_str()` — which is how four
    // of the five readers of this key read it — while `->>` renders it as text.
    for (i, value) in [
        serde_json::json!(""),
        serde_json::json!("   "),
        serde_json::json!(7),
        serde_json::json!(true),
        serde_json::json!(["x"]),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("t9bad{i}-{suffix}");
        let (status, body) = into_json(
            f.app_empty()
                .oneshot(req(
                    "POST",
                    "/policies/services",
                    &token,
                    Some(serde_json::json!({
                        "name": name, "type": "trino",
                        "config": { "tagService": value.clone() },
                    })),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a tagService of {value} names no profile and must be refused; body={body}"
        );
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains("tagService"),
            "the 400 must name the key to fix; value={value} body={body}"
        );
        assert!(
            msg.contains("tagPolicies"),
            "the 400 must name the consequence (the download omits the block); \
             value={value} body={body}"
        );
        assert!(
            f.store.get_service(&name).await.is_err(),
            "the refused profile must NOT have been inserted; value={value}"
        );
    }

    // ACCEPT control 1: the shape `bind_trino_profile` writes.
    let good = format!("t9good-{suffix}");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies/services",
                &token,
                Some(serde_json::json!({
                    "name": good, "type": "trino",
                    "config": { "tagService": tag_name },
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a real name must create; body={body}"
    );
    let good_svc = f.store.get_service(&good).await.expect("created");

    // ACCEPT control 2: no `tagService` key at all is the ordinary console
    // profile (`createProfile(name, type)` sends no config) and must still work.
    let bare = format!("t9bare-{suffix}");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies/services",
                &token,
                Some(serde_json::json!({ "name": bare, "type": "trino" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "no tagService key; body={body}");
    let bare_svc = f.store.get_service(&bare).await.expect("created");

    f.store.delete_service(good_svc.id).await.expect("cleanup");
    f.store.delete_service(bare_svc.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

/// Steps 2 + 3: the tag editor's WRITE target no longer falls back to
/// `{profile}_tag`, so it cannot file a policy into a profile the plugin
/// download does not serve.
///
/// The premise is the state `engines.rs:2595` documents as real — a stranded
/// `{profile}_tag` row "left by a deleted same-name engine: reuse, never
/// delete" — beside a profile whose `config` carries no link. Before Task 9
/// this POST returned **200** and stored the policy in `p_tag`, while
/// `download_policies` served the profile with no `tagPolicies` block at all.
///
/// ⚠ Three controls, because "refuses tag policies" would pass the first
/// assertion on its own:
/// 1. the SAME request against a profile whose `tagService` IS configured is
///    accepted, and the policy is served in the download's `tagPolicies`;
/// 2. `list_profiles` still offers `tag_service` for the fallback profile —
///    that read-side fallback is pinned behaviour and checks existence first,
///    so it cannot invent a write target; keeping it is what lets the Tags tab
///    still LIST what is stranded there;
/// 3. a STARROCKS profile with a stranded `{name}_tag` is refused too. A
///    non-Trino profile can never carry a `tagService` (Task 2 + migration
///    0176), so the removed fallback was also Task 2's hole on this surface.
#[tokio::test]
async fn a_tag_policy_is_refused_when_the_profile_configures_no_tag_service() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t9_nofb", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_starrocks(&f.store)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&f.store)
        .await
        .expect("seed tag def");

    let suffix = Uuid::new_v4().simple().to_string();

    // ── The premise: a profile with NO `tagService`, beside a stranded
    // `{profile}_tag` row that the old fallback would have resolved to.
    let unlinked = format!("t9unlinked-{suffix}");
    let unlinked_svc = f
        .store
        .create_service(&unlinked, "trino", &serde_json::json!({}))
        .await
        .expect("create unlinked trino profile");
    let stranded = f
        .store
        .create_service(&format!("{unlinked}_tag"), "tag", &serde_json::json!({}))
        .await
        .expect("create stranded tag service");

    let body_for = |profile: &str, name: &str| {
        serde_json::json!({
            "kind": "tag-masking", "name": name, "service": profile,
            "tag": "pii", "isEnabled": true,
            "items": [{"groups": ["pii"], "maskType": "MASK_HASH"}]
        })
    };

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(body_for(&unlinked, &format!("t9m-{suffix}"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a tag policy against a profile with no configured tagService must be refused, \
         not filed into the stranded `{{profile}}_tag`; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&unlinked) && msg.contains("tagService"),
        "the 400 must name the profile and the missing link; got {msg}"
    );
    assert!(
        msg.contains("tagPolicies"),
        "the 400 must say WHY (the download would carry no tagPolicies block); got {msg}"
    );
    assert_eq!(
        f.store
            .enabled_policies_for_service(stranded.id)
            .await
            .expect("enabled policies")
            .len(),
        0,
        "nothing may have been mis-filed into the stranded tag service"
    );

    // ── CONTROL 2: and `list_profiles` must not OFFER a Tags tab here either.
    //
    // **⚠ THIS ASSERTION CHANGED DIRECTION in the Task 9 fix round.** As first
    // written it asserted the opposite — that `list_profiles`' `<name>_tag`
    // convention fallback still surfaced the stranded row, on the reasoning that
    // the fallback checks existence and so cannot invent a WRITE target. The
    // ruling went the other way: nothing can give this profile a `tagService`
    // (no `UPDATE` of `config_jsonb` exists in `src/`, and `createProfile` sends
    // no `config`), so the tab it offered led to the 400 asserted directly
    // above — a workflow that cannot complete, ever. The profile is still
    // LISTED; it just carries no tab.
    let (status, profiles) = into_json(
        f.app_empty()
            .oneshot(req("GET", "/policies/services", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list profiles; body={profiles}");
    let listed = profiles
        .as_array()
        .expect("array")
        .iter()
        .find(|p| p["id"] == serde_json::json!(unlinked_svc.id))
        .unwrap_or_else(|| panic!("profile not listed; body={profiles}"));
    assert_eq!(
        listed["tag_service"],
        serde_json::Value::Null,
        "a profile whose own `config.tagService` names nothing must get NO Tags tab, \
         even with a conventionally-named `{{name}}_tag` row beside it — the tab would \
         lead to the 400 asserted above; body={profiles}"
    );

    // ── CONTROL 1: with the link configured, the same request is accepted and
    // the policy really is in the download the tag plugin polls.
    let linked_tag = format!("t9lt-{suffix}");
    let linked_tag_svc = f
        .store
        .create_service(&linked_tag, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let linked = format!("t9linked-{suffix}");
    let linked_svc = f
        .store
        .create_service(
            &linked,
            "trino",
            &serde_json::json!({ "tagService": linked_tag }),
        )
        .await
        .expect("create linked trino profile");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(body_for(&linked, &format!("t9ok-{suffix}"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a configured tagService must still accept the same policy; body={body}"
    );
    assert_eq!(
        f.store
            .enabled_policies_for_service(linked_tag_svc.id)
            .await
            .expect("enabled policies")
            .len(),
        1,
        "the accepted policy is served under the CONFIGURED tag service"
    );
    // ⚠ And the tab IS offered for this one — the control that keeps control 2
    // from being satisfied by "`tag_service` is always null". Both halves read
    // the same join, so this is the one assertion that makes the pair
    // discriminating.
    let (_, profiles) = into_json(
        f.app_empty()
            .oneshot(req("GET", "/policies/services", &token, None))
            .await
            .unwrap(),
    )
    .await;
    let listed_linked = profiles
        .as_array()
        .expect("array")
        .iter()
        .find(|p| p["id"] == serde_json::json!(linked_svc.id))
        .unwrap_or_else(|| panic!("linked profile not listed; body={profiles}"));
    assert_eq!(
        listed_linked["tag_service"],
        serde_json::json!(linked_tag),
        "a profile whose own `config.tagService` resolves MUST get the Tags tab; \
         body={profiles}"
    );

    // ── CONTROL 3: a StarRocks profile with a stranded `{name}_tag`. It can
    // never carry a `tagService`, so the old fallback ALWAYS fired for one.
    let sr = format!("t9sr-{suffix}");
    let sr_svc = f
        .store
        .create_service(&sr, "starrocks", &serde_json::json!({}))
        .await
        .expect("create starrocks profile");
    let sr_stranded = f
        .store
        .create_service(&format!("{sr}_tag"), "tag", &serde_json::json!({}))
        .await
        .expect("create stranded tag service");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(body_for(&sr, &format!("t9srm-{suffix}"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a tag policy aimed at a StarRocks profile must be refused; body={body}"
    );
    assert_eq!(
        f.store
            .enabled_policies_for_service(sr_stranded.id)
            .await
            .expect("enabled policies")
            .len(),
        0,
        "nothing mis-filed under the StarRocks profile's stranded tag service"
    );

    f.store
        .delete_service(sr_stranded.id)
        .await
        .expect("cleanup");
    f.store.delete_service(sr_svc.id).await.expect("cleanup");
    f.store
        .delete_service(linked_svc.id)
        .await
        .expect("cleanup");
    f.store
        .delete_service(linked_tag_svc.id)
        .await
        .expect("cleanup");
    f.store.delete_service(stranded.id).await.expect("cleanup");
    f.store
        .delete_service(unlinked_svc.id)
        .await
        .expect("cleanup");
}

// ---------------------------------------------------------------------------
// SP4d debt Task 16 — a tag policy can now be EDITED, not only created and
// deleted.
// ---------------------------------------------------------------------------

/// `PUT /policies/{id}` with a `tag-*` kind used to answer 400 "unknown policy
/// kind: tag-masking", because the handler had no `is_tag_kind` branch. The UI
/// reaches it: `PolicyEditorPage`'s `buildPayload` sends `{kind:"tag-masking",
/// service:<trino profile>, tag}` and the Tags tab links every row through the
/// main profile.
///
/// Asserts the round trip the editor actually performs, and the three things a
/// "replaced it somehow" assertion would miss: the row stays under the TAG
/// service (not the trino profile), the body keeps `serviceType:"tag"` and the
/// single `tag` resource, and the new `isEnabled` is honoured.
#[tokio::test]
async fn a_tag_policy_can_be_edited_through_put_policies() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t16edit", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store)
        .await
        .expect("seed tag def");

    let suffix = Uuid::new_v4().simple().to_string();
    let tag_name = format!("t16tag-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let trino_name = format!("t16trino-{suffix}");
    let trino_svc = f
        .store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_name }),
        )
        .await
        .expect("create trino profile");

    let policy_name = format!("t16-mask-{suffix}");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": policy_name, "service": trino_name,
                    "tag": "pii", "isEnabled": true,
                    "items": [{"groups": ["analysts"], "maskType": "MASK_HASH"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create tag mask; body={body}");
    let id = body["id"].as_i64().expect("id");

    // The edit the editor sends: a different tag, a different mask type, a
    // different principal, disabled.
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{id}"),
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": policy_name, "service": trino_name,
                    "tag": "phi", "isEnabled": false,
                    "items": [{"groups": ["auditors"], "maskType": "MASK_NULL"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a tag policy must be EDITABLE, not only creatable and deletable; body={body}"
    );
    assert_eq!(body["id"].as_i64(), Some(id), "same row; body={body}");

    // It is still the tag service's row, still a tag body, and the edit landed.
    let served = f
        .store
        .enabled_policies_for_service(tag_svc.id)
        .await
        .expect("enabled policies");
    assert_eq!(
        served.len(),
        0,
        "the edit set isEnabled:false, so the tag service serves nothing now"
    );
    let stored = f.store.get_policy(id).await.expect("get policy");
    assert!(
        !stored.enabled,
        "the form's disabled toggle must be honoured"
    );
    assert_eq!(
        stored.service_id, tag_svc.id,
        "the row must still live under the TAG service, not the trino profile"
    );
    assert_eq!(stored.policy_jsonb["serviceType"], "tag");
    assert_eq!(stored.policy_jsonb["service"], tag_name.as_str());
    assert_eq!(
        stored.policy_jsonb["resources"]["tag"]["values"][0], "phi",
        "the new tag must be the one selector; body={}",
        stored.policy_jsonb
    );
    assert_eq!(
        stored.policy_jsonb["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"], "MASK_NULL",
        "the new mask type must have replaced the old; body={}",
        stored.policy_jsonb
    );
    assert_eq!(
        stored.policy_jsonb["dataMaskPolicyItems"][0]["groups"][0], "auditors",
        "the new principal must have replaced the old; body={}",
        stored.policy_jsonb
    );

    f.store.delete_service(trino_svc.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

/// The edit branch runs the SAME guards the create branch runs — Task 5's
/// unreadable-table-mask refusal and Task 6's ambiguous-arbitration refusal —
/// because an edit is an authoring surface. Before Task 16 this was vacuously
/// true: there was no branch to guard.
///
/// ⚠ Both accept-controls are in this test, and the second is the one that
/// matters most: re-PUTting a policy's OWN body must be accepted. Without
/// `replacing: Some(id)` the arbitration guard pairs the new body against the
/// stored version of itself and produces a refusal no retry can escape.
#[tokio::test]
async fn editing_a_tag_policy_runs_the_same_refusals_as_creating_one() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t16guard", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store)
        .await
        .expect("seed tag def");

    let suffix = Uuid::new_v4().simple().to_string();
    let tag_svc_name = format!("t16gtag-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_svc_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let trino_name = format!("t16gtrino-{suffix}");
    let trino_svc = f
        .store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_svc_name }),
        )
        .await
        .expect("create trino profile");

    // A tag attached to a WHOLE TABLE — Task 5's half. The key and the name are
    // different strings so a key/name confusion cannot read as a pass.
    let table_tag_name = format!("T16-TBL-{suffix}");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("t16-k-{suffix}"))
            .bind(&table_tag_name)
            .fetch_one(&f.db.pool)
            .await
            .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 'sales', 'orders', '')",
    )
    .bind(tag_id)
    .bind(Uuid::new_v4())
    .execute(&f.db.pool)
    .await
    .expect("table-level attachment");

    // A mask on a tag nothing attaches — accepted, and the row we will edit.
    let policy_name = format!("t16g-mask-{suffix}");
    let create = serde_json::json!({
        "kind": "tag-masking", "name": policy_name, "service": trino_name,
        "tag": format!("t16-unattached-{suffix}"), "isEnabled": true,
        "items": [{"groups": ["analysts"], "maskType": "MASK_HASH"}]
    });
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req("POST", "/policies", &token, Some(create)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create tag mask; body={body}");
    let id = body["id"].as_i64().expect("id");

    // ── Task 5's guard, on the EDIT: repoint the mask at the table-level tag.
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{id}"),
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": policy_name, "service": trino_name,
                    "tag": table_tag_name, "isEnabled": true,
                    "items": [{"groups": ["analysts"], "maskType": "MASK_HASH"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an EDIT that creates the unreadable-table pair must be refused too; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("sales.orders") && msg.contains(&policy_name),
        "the refusal must name the table and the policy; got {msg}"
    );
    // Task 16's own change to that sentence: with editing possible, "edit the
    // policy to address it by path" is a third and better remedy, and it comes
    // from the ONE shared `refusal()` so every write path gained it together.
    assert!(
        msg.contains("so it addresses sales.orders by path"),
        "the refusal must now name the edit-in-place remedy; got {msg}"
    );
    let stored = f.store.get_policy(id).await.expect("get policy");
    assert_ne!(
        stored.policy_jsonb["resources"]["tag"]["values"][0],
        serde_json::json!(table_tag_name),
        "a refused edit must have written nothing; body={}",
        stored.policy_jsonb
    );

    // ── Task 6's guard, on the EDIT: a second mask on the same tag with
    // different items. Create it on a DIFFERENT tag first (accepted), then edit
    // it onto the first mask's tag.
    let second_name = format!("t16g-mask2-{suffix}");
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": second_name, "service": trino_name,
                    "tag": format!("t16-other-{suffix}"), "isEnabled": true,
                    "items": [{"groups": ["auditors"], "maskType": "MASK_NULL"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create second tag mask; body={body}"
    );
    let second_id = body["id"].as_i64().expect("id");

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{second_id}"),
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": second_name, "service": trino_name,
                    "tag": format!("t16-unattached-{suffix}"), "isEnabled": true,
                    "items": [{"groups": ["auditors"], "maskType": "MASK_NULL"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an EDIT onto a tag another mask already selects, with different items, must be \
         refused with 409 (SP4d-4 Part A); body={body}"
    );
    assert!(
        body.to_string()
            .contains("or set Override on the one that should win"),
        "the refusal must be the arbitration guard's, carrying the editor's Override \
         remedy; body={body}"
    );

    // ── ACCEPT control, and the one that proves `replacing: Some(id)` is
    // wired: edit the first policy IN PLACE — same tag, DIFFERENT items.
    //
    // ⚠ This was first written as a re-PUT of the policy's own unchanged body,
    // and the `M5-replacing-arg` mutation (`Some(id)` → `None`) SURVIVED it:
    // Task 6's rule refuses two policies selecting the same resource with
    // DIFFERENT items, and a body byte-identical to the stored one disagrees
    // with nothing, so it passed either way. An assertion that cannot fail is
    // not an assertion. Changing the items is what makes the self-pair a
    // disagreeing one, so the exclusion is the only thing that admits it.
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{id}"),
                &token,
                Some(serde_json::json!({
                    "kind": "tag-masking", "name": policy_name, "service": trino_name,
                    "tag": format!("t16-unattached-{suffix}"), "isEnabled": true,
                    "items": [{"groups": ["analysts"], "maskType": "MASK_NULL"}]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "editing a tag policy in place must be accepted — the arbitration guard must not \
         pair the new body against the row it replaces; body={body}"
    );
    let stored = f.store.get_policy(id).await.expect("get policy");
    assert_eq!(
        stored.policy_jsonb["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"], "MASK_NULL",
        "and the accepted edit must have landed; body={}",
        stored.policy_jsonb
    );

    f.store.delete_service(trino_svc.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

/// **A guard pointed at the wrong policy set is not running.** `PUT
/// /policies/{id}` took the profile to validate against from the REQUEST BODY
/// while `store.update_policy` wrote the row under its OWN `service_id`, which
/// it never changes — so naming a different, empty profile had every guard in
/// the handler (Task 6's arbitration guard above all) evaluate against that
/// profile's policy set and pass, while the row stayed where it was and was
/// served alongside the policy it ties with.
///
/// Found while routing Task 16's new tag branch through the create path's
/// guards; pre-existing on the resource-addressed path since Task 6, and
/// reachable on the tag path too (a different trino profile resolves a
/// different tag service).
///
/// ⚠ The accept-control is in the same test and it is the one that keeps this
/// from being "PUT is refused": re-sending the SAME profile is accepted, and
/// the edit lands. The console only ever sends that one — the editor route
/// carries the profile and the Tags tab links every row through it.
#[tokio::test]
async fn put_policies_refuses_a_service_that_is_not_the_policys_own() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t16mismatch", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");

    let suffix = Uuid::new_v4().simple().to_string();
    let (home_name, home_id) = seed_service(&f).await;
    let elsewhere_name = format!("t16else-{suffix}");
    let elsewhere = f
        .store
        .create_service(&elsewhere_name, "trino", &serde_json::json!({}))
        .await
        .expect("create a second, empty profile");

    // A row filter under `home`. It is a row filter because that is the class
    // Task 6's guard reads, so this is the bypass that mattered.
    let policy_name = format!("t16rf-{suffix}");
    let body_for = |service: &str, expr: &str| {
        serde_json::json!({
            "kind": "rowfilter", "name": policy_name, "service": service,
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}
            },
            "items": [{"groups": ["analysts"], "filterExpr": expr}]
        })
    };
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                "/policies",
                &token,
                Some(body_for(&home_name, "region = 'eu'")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create row filter; body={body}");
    let id = body["id"].as_i64().expect("id");

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{id}"),
                &token,
                Some(body_for(&elsewhere_name, "region = 'us'")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a PUT naming a profile the policy does not belong to must be refused — every \
         guard in the handler would otherwise be evaluated against that profile; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&elsewhere_name) && msg.contains(&home_id.to_string()),
        "the refusal must name both the profile sent and the profile the policy belongs \
         to; got {msg}"
    );
    let stored = f.store.get_policy(id).await.expect("get policy");
    assert_eq!(
        stored.policy_jsonb["rowFilterPolicyItems"][0]["rowFilterInfo"]["filterExpr"],
        "region = 'eu'",
        "a refused PUT must have written nothing; body={}",
        stored.policy_jsonb
    );
    assert_eq!(
        stored.service_id, home_id,
        "and must not have moved the row"
    );

    // ACCEPT control: the policy's OWN profile, same request otherwise.
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "PUT",
                &format!("/policies/{id}"),
                &token,
                Some(body_for(&home_name, "region = 'us'")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the policy's own profile must still be accepted; body={body}"
    );
    let stored = f.store.get_policy(id).await.expect("get policy");
    assert_eq!(
        stored.policy_jsonb["rowFilterPolicyItems"][0]["rowFilterInfo"]["filterExpr"],
        "region = 'us'",
        "the accepted PUT must have landed; body={}",
        stored.policy_jsonb
    );

    f.store.delete_service(elsewhere.id).await.expect("cleanup");
    f.store.delete_service(home_id).await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// SP4d-3 Task 2, fix round 1 — the infrastructure refusals win over the
// route's authoring guards.
// ---------------------------------------------------------------------------

/// A human `POST`/`PUT` of an infrastructure name whose body ALSO trips the
/// ambiguous-arbitration guard must answer 409 `InfrastructureOwned`, not the
/// guard's refusal: the store's own refusal sits inside `create_policy`/
/// `update_policy`, after the guards, so the route needs the pre-flights.
/// Controls: the same disagreeing body under an ordinary name IS the guard's
/// refusal (so the guard is genuinely armed and the infra 409 is not a
/// vacuous pass). Since SP4d-4 the guard answers 409 too, so the two are told
/// apart by their messages, which both arms assert.
#[tokio::test]
async fn an_infra_name_is_refused_with_409_before_the_arbitration_guard() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "infra409", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let filter = |name: &str, expr: &str| {
        serde_json::json!({
            "kind": "rowfilter", "name": name, "service": svc_name, "isEnabled": true,
            "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                          "table": {"values": ["orders"]}},
            "items": [{"groups": ["public"], "filterExpr": expr}]
        })
    };
    let send = |method: &'static str, uri: String, body: serde_json::Value| {
        let app = f.app_empty();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req(method, &uri, &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    let count = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1",
        )
        .bind(svc_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count")
    };

    let (status, body) = send(
        "POST",
        "/policies".into(),
        filter("rf-us", "country = 'US'"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "seed filter; body={body}");
    let (status, body) = send(
        "POST",
        "/policies".into(),
        filter("rf-us-b", "country = 'US'"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agreeing filter; body={body}");
    let b_id = body["id"].as_i64().expect("id");
    let rows = count().await;

    // Control: an ordinary name with the disagreeing body is the guard's 409.
    // Since SP4d-4 both refusals are 409, so the status alone no longer tells
    // them apart — the guard's own remedy does.
    let (status, body) = send("POST", "/policies".into(), filter("rf-id", "id > 2")).await;
    assert_eq!(status, StatusCode::CONFLICT, "armed guard; body={body}");
    assert!(
        body.to_string()
            .contains("or set Override on the one that should win"),
        "the control must be the arbitration guard's refusal: {body}"
    );

    // POST: infra names + a guard-tripping body -> 409, nothing created.
    for name in ["egret-query-timeout", "all - catalog, schema, table"] {
        let (status, body) = send("POST", "/policies".into(), filter(name, "id > 2")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{name}; body={body}");
        assert!(
            body.to_string()
                .contains("reserved for egret's own plumbing"),
            "{name}: must carry the InfrastructureOwned message: {body}"
        );
        assert!(
            !body.to_string().contains("set Override"),
            "{name}: must be the infra refusal, not the arbitration guard's: {body}"
        );
    }
    assert_eq!(count().await, rows, "no row created");

    // PUT-rename: same, and the row keeps its name.
    for name in ["egret-query-timeout", "all - catalog, schema, table"] {
        let (status, body) = send("PUT", format!("/policies/{b_id}"), filter(name, "id > 2")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{name}; body={body}");
        assert!(
            body.to_string()
                .contains("reserved for egret's own plumbing"),
            "{name}: must carry the InfrastructureOwned message: {body}"
        );
    }
    assert_eq!(f.store.get_policy(b_id).await.expect("get").name, "rf-us-b");

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// SP4d-3 final review I-1: seed-defaults builds the Trino default set, so on a
/// StarRocks profile it is refused with a sentence and writes nothing. The
/// control (a Trino profile, same caller, same route) seeds, so the 400 is the
/// type check and not a broken route.
#[tokio::test]
async fn seed_defaults_refuses_a_starrocks_profile_and_writes_nothing() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "seed_sr", true).await;
    egret_policy::seed_starrocks(&f.store)
        .await
        .expect("seed starrocks def");
    let sr = format!("sr-seed-{}", Uuid::new_v4().simple());
    let sr_id = f
        .store
        .create_service(&sr, "starrocks", &serde_json::json!({}))
        .await
        .expect("create starrocks profile")
        .id;

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                &format!("/policies/services/{sr}/seed-defaults"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    let msg = body.to_string();
    assert!(
        msg.contains("Trino default policy set") && msg.contains("starrocks"),
        "the refusal must say what seed-defaults seeds and the profile's type; body={body}"
    );
    assert!(
        f.store.list_policies(sr_id).await.expect("list").is_empty(),
        "a refused seed wrote rows"
    );

    // Control: the same call on a Trino profile seeds the default set.
    let (trino, trino_id) = seed_service(&f).await;
    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "POST",
                &format!("/policies/services/{trino}/seed-defaults"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["created"], serde_json::json!(13), "body={body}");

    f.store.delete_service(sr_id).await.expect("cleanup");
    f.store.delete_service(trino_id).await.expect("cleanup");
}

/// SP4d-3 final review I-1: the profile page's list carries `managed_by`, so it
/// can withhold the toggle and trash the server would refuse on infra rows.
#[tokio::test]
async fn the_policy_list_carries_each_rows_owner() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "list_owner", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    f.store
        .create_infra_policy(
            svc_id,
            "egret-query-timeout",
            &serde_json::json!({"name": "egret-query-timeout"}),
        )
        .await
        .expect("infra row");
    f.store
        .create_policy(
            svc_id,
            "sales can read",
            &serde_json::json!({"name": "sales can read"}),
        )
        .await
        .expect("operator row");

    let (status, body) = into_json(
        f.app_empty()
            .oneshot(req(
                "GET",
                &format!("/policies?service={svc_name}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let mut owners: Vec<(String, String)> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_string(),
                s["managed_by"]
                    .as_str()
                    .expect("managed_by present")
                    .to_string(),
            )
        })
        .collect();
    owners.sort();
    assert_eq!(
        owners,
        vec![
            ("egret-query-timeout".to_string(), "infra".to_string()),
            ("sales can read".to_string(), "operator".to_string()),
        ]
    );
    f.store.delete_service(svc_id).await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// SP4d-4 Task 3 — the editor's half, widened to the evaluation unit.
// ---------------------------------------------------------------------------

/// N3: `PUT /policies/{id}` with `isEnabled: true` is a guarded write, unlike
/// `PUT …/enabled` — a re-enable through it that completes a differing pair is
/// refused, and one that completes an AGREEING tie is not.
#[tokio::test]
async fn re_enabling_through_put_policies_that_completes_a_pair_is_409() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t3reenable", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let filter = |name: &str, expr: &str, enabled: bool| {
        serde_json::json!({
            "kind": "rowfilter", "name": name, "service": svc_name, "isEnabled": enabled,
            "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                          "table": {"values": ["t1"]}},
            "items": [{"groups": ["public"], "filterExpr": expr}]
        })
    };
    let send = |method: &'static str, uri: String, body: serde_json::Value| {
        let app = f.app_empty();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req(method, &uri, &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    let (status, body) = send("POST", "/policies".into(), filter("ra", "x = 1", false)).await;
    assert_eq!(status, StatusCode::OK, "ra disabled; body={body}");
    let ra = body["id"].as_i64().expect("id");
    let (status, body) = send("POST", "/policies".into(), filter("rb", "x = 2", true)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "rb, nothing enabled to pair with; body={body}"
    );

    let (status, body) = send(
        "PUT",
        format!("/policies/{ra}"),
        filter("ra", "x = 1", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "re-enabling ra through PUT /policies/{{id}} completes (ra, rb); body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(msg.contains("\"ra\"") && msg.contains("\"rb\""), "{msg}");
    // Accept-control: the same re-enable with an AGREEING expression.
    let (status, body) = send(
        "PUT",
        format!("/policies/{ra}"),
        filter("ra", "x = 2", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an agreeing tie is admitted; body={body}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// The pair identity carries both decisive arrays and the coordinate: an edit
/// that keeps its stored partner but changes WHAT it decides, or WHERE the two
/// meet, is a different pair and is refused; an edit that touches neither, or
/// that removes the pair, passes.
#[tokio::test]
async fn an_edit_that_keeps_its_partner_but_changes_the_pair_is_409() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t3partner", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;
    let filter = |name: &str, table: &str, expr: &str, enabled: bool| {
        serde_json::json!({
            "kind": "rowfilter", "name": name, "service": svc_name, "isEnabled": enabled,
            "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                          "table": {"values": [table]}},
            "items": [{"groups": ["public"], "filterExpr": expr}]
        })
    };
    let send = |method: &'static str, uri: String, body: serde_json::Value| {
        let app = f.app_empty();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req(method, &uri, &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    // A stored differing pair (pa, pb) on t1, built through the unguarded
    // `PUT …/enabled`.
    let mut ids = std::collections::BTreeMap::new();
    for (name, expr) in [("pa", "x = 1"), ("pb", "x = 2")] {
        let (status, body) =
            send("POST", "/policies".into(), filter(name, "t1", expr, false)).await;
        assert_eq!(status, StatusCode::OK, "create {name}; body={body}");
        ids.insert(name, body["id"].as_i64().expect("id"));
    }
    for name in ["pa", "pb"] {
        let (status, body) = send(
            "PUT",
            format!("/policies/{}/enabled", ids[name]),
            serde_json::json!({"enabled": true}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "enable {name}; body={body}");
    }

    // Control: an edit that changes neither side's decision nor the coordinate.
    let (status, body) = send(
        "PUT",
        format!("/policies/{}", ids["pa"]),
        filter("pa", "t1", "x = 1", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an unchanged pair is not new; body={body}"
    );

    // Cell 1: same partner, different decisive items.
    let (status, body) = send(
        "PUT",
        format!("/policies/{}", ids["pa"]),
        filter("pa", "t1", "x = 3", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "changing what pa decides while it still meets pb is a new pair; body={body}"
    );

    // Cell 2: same partner, different coordinate.
    let (status, body) = send(
        "PUT",
        format!("/policies/{}", ids["pa"]),
        filter("pa", "t*", "x = 1", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "moving where pa meets pb is a new pair; body={body}"
    );

    // The write that REMOVES the pair still passes.
    let (status, body) = send(
        "PUT",
        format!("/policies/{}", ids["pa"]),
        filter("pa", "t3", "x = 3", true),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "removing the pair passes; body={body}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// A Trino profile with a tag service, one engine bound to it, and one registry
/// catalog (named `catalog`) attached — the shape `catalogs_for_service_via`
/// resolves tag assignments through. Returns `(profile name, profile id, tag
/// service id, warehouse id, catalog name)`.
async fn trino_unit(f: &Fixture, label: &str) -> (String, i64, i64, Uuid, String) {
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store).await.expect("seed tag");
    let suffix = Uuid::new_v4().simple().to_string();
    let tag_svc_name = format!("{label}tag-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_svc_name, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let trino_name = format!("{label}trino-{suffix}");
    let trino = f
        .store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_svc_name }),
        )
        .await
        .expect("trino profile");
    let pool = &f.db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'T3 WS') RETURNING id")
            .bind(format!("{label}-ws-{suffix}"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-t3','AKIATEST') RETURNING id",
    )
    .bind(format!("{label}-sc-{suffix}"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("{label}_wh_{suffix}"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(format!("{label}-eng-{suffix}"))
    .bind(trino.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = format!("{label}_cat_{}", &suffix[..12]);
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(wh)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    (trino_name, trino.id, tag_svc.id, wh, catalog)
}

async fn new_tag_on(f: &Fixture, wh: Uuid, label: &str, columns: &[&str]) -> String {
    let name = format!("{label}-{}", Uuid::new_v4().simple());
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(name.to_lowercase())
        .bind(&name)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag");
    for column in columns {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments \
                 (tag_id, warehouse_id, schema_name, table_name, column_name) \
             VALUES ($1, $2, 'sales', 'orders', $3)",
        )
        .bind(id)
        .bind(wh)
        .bind(column)
        .execute(&f.db.pool)
        .await
        .expect("assignment");
    }
    name
}

/// User ruling (SP4d-4 Task 3): hand-written TAG policies on a manual profile
/// were treated as never overlapping, but a column carrying both tags is
/// decided by both. The editor now refuses the write that forms such a pair
/// given the current assignments — tag ↔ tag and tag ↔ path — and admits a tag
/// policy whose tag shares no column with the other.
#[tokio::test]
async fn hand_written_tag_masks_meeting_on_a_tagged_column_are_409() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t3tags", true).await;
    let (trino, trino_id, tag_svc, wh, catalog) = trino_unit(&f, "t3h").await;
    let pii = new_tag_on(&f, wh, "PII", &["email"]).await;
    let gdpr = new_tag_on(&f, wh, "GDPR", &["email"]).await;
    let phone = new_tag_on(&f, wh, "PHONE", &["phone"]).await;
    let send = |body: serde_json::Value| {
        let app = f.app_empty();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req("POST", "/policies", &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    let tag_mask = |name: &str, tag: &str, mask: &str| {
        serde_json::json!({
            "kind": "tag-masking", "name": name, "service": trino, "tag": tag,
            "isEnabled": true, "items": [{"groups": ["public"], "maskType": mask}]
        })
    };

    let (status, body) = send(tag_mask("tm-pii", &pii, "MASK")).await;
    assert_eq!(status, StatusCode::OK, "the first tag mask; body={body}");
    // Accept-control: a tag on another column only.
    let (status, body) = send(tag_mask("tm-phone", &phone, "MASK_HASH")).await;
    assert_eq!(status, StatusCode::OK, "no shared column; body={body}");

    // Tag ↔ tag: `email` carries both PII and GDPR.
    let (status, body) = send(tag_mask("tm-gdpr", &gdpr, "MASK_NULL")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "tag ↔ tag on email; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("\"tm-gdpr\"")
            && msg.contains("\"tm-pii\"")
            && msg.contains(&format!("{catalog}.sales.orders.email")),
        "names both policies and the column: {msg}"
    );
    assert_eq!(
        f.store
            .enabled_policies_for_service(tag_svc)
            .await
            .unwrap()
            .len(),
        2,
        "the refused tag mask was not stored"
    );

    // Tag ↔ path: a hand-written resource mask on the tagged column.
    let path = serde_json::json!({
        "kind": "masking", "name": "pm-email", "service": trino, "isEnabled": true,
        "resources": {"catalog": {"values": [catalog]}, "schema": {"values": ["sales"]},
                      "table": {"values": ["orders"]}, "column": {"values": ["email"]}},
        "items": [{"groups": ["public"], "maskType": "MASK_NULL"}]
    });
    let (status, body) = send(path).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "tag ↔ path on email; body={body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("\"tm-pii\"") && msg.contains("\"pm-email\"") && msg.contains("tag phase"),
        "{msg}"
    );
    // Accept-control: the same resource mask on an untagged column.
    let untagged = serde_json::json!({
        "kind": "masking", "name": "pm-sku", "service": trino, "isEnabled": true,
        "resources": {"catalog": {"values": [catalog]}, "schema": {"values": ["sales"]},
                      "table": {"values": ["orders"]}, "column": {"values": ["sku"]}},
        "items": [{"groups": ["public"], "maskType": "MASK_NULL"}]
    });
    let (status, body) = send(untagged).await;
    assert_eq!(status, StatusCode::OK, "an untagged column; body={body}");

    f.store.delete_service(trino_id).await.expect("cleanup");
    f.store.delete_service(tag_svc).await.expect("cleanup");
}

/// SP4d-4 Task 3 fix round 1, review I1: the editor's overlap check and its
/// write are ONE transaction under the unit's service row locks. Two editor
/// writes that would form a differing pair on one column, both started while a
/// second connection holds the unit's rows (a profile and its tag service —
/// what a recompile or a tag assignment holds), must not both commit: once the
/// holder lets go, exactly one is stored and the other answers 409.
///
/// Before the fix each request checked on a pooled connection with no lock,
/// then parked on the row at its INSERT, so both checks passed and both
/// committed. Driven without a sleep for synchronisation: the holder's pid is
/// polled in `pg_blocking_pids` until BOTH requests are parked behind it
/// (directly or behind the other).
#[tokio::test]
async fn two_concurrent_editor_writes_forming_a_pair_store_exactly_one() {
    use sqlx::Connection as _;
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let dsn = egret_test_db::url().await.expect("dsn");
    let token = session_for(&f, "t3race", true).await;
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino");
    egret_policy::seed_tag(&f.store).await.expect("seed tag");
    let suffix = Uuid::new_v4().simple().to_string();
    let tag_name = format!("t3race-tag-{suffix}");
    let tag_svc = f
        .store
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let trino_name = format!("t3race-trino-{suffix}");
    let trino = f
        .store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_name }),
        )
        .await
        .expect("trino profile");

    let mut holder = sqlx::PgConnection::connect(&dsn).await.expect("holder");
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut holder)
        .await
        .expect("holder pid");
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    let mut held = [tag_svc.id, trino.id];
    held.sort_unstable();
    for id in held {
        sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
            .bind(id)
            .execute(&mut holder)
            .await
            .expect("hold the unit's rows");
    }

    let write = |name: &str, mask: &str| {
        let app = f.app_empty();
        let token = token.clone();
        let body = serde_json::json!({
            "kind": "masking", "name": name, "service": trino_name, "isEnabled": true,
            "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                          "table": {"values": ["t"]}, "column": {"values": ["email"]}},
            "items": [{"groups": ["public"], "maskType": mask}]
        });
        tokio::spawn(async move {
            into_json(
                app.oneshot(req("POST", "/policies", &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        })
    };
    let a = write("race-a", "MASK");
    let b = write("race-b", "MASK_NULL");

    let mut parked = 0_i64;
    for _ in 0..400 {
        // Transitively: the second waiter on a row queues behind the FIRST
        // waiter's tuple lock, so `pg_blocking_pids` names that waiter, not
        // the holder.
        parked = sqlx::query_scalar(
            "WITH RECURSIVE w(pid) AS ( \
                 SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) \
                 UNION \
                 SELECT a.pid FROM pg_stat_activity a JOIN w ON w.pid = ANY(pg_blocking_pids(a.pid)) \
             ) SELECT count(*) FROM w",
        )
        .bind(holder_pid)
        .fetch_one(&f.db.pool)
        .await
        .expect("read pg_blocking_pids");
        if parked >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        parked >= 2,
        "both writes must park on the held rows: {parked}"
    );
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();

    let (sa, ba) = a.await.expect("a");
    let (sb, bb) = b.await.expect("b");
    let mut statuses = [sa, sb];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::CONFLICT],
        "exactly one stored: a={sa} {ba}; b={sb} {bb}"
    );
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
            .bind(trino.id)
            .fetch_one(&f.db.pool)
            .await
            .expect("count");
    assert_eq!(stored, 1, "one policy in the profile");
    let refused = if sa == StatusCode::CONFLICT { &ba } else { &bb };
    // M1: the editor's 409 carries the same structured fields as the perm
    // routes; both members are hand-written, so neither has a rule id.
    assert_eq!(refused["code"], "rule_overlap", "{refused}");
    assert_eq!(refused["kind"], "mask", "{refused}");
    assert!(refused["coordinate"].is_string(), "{refused}");
    let members = refused["rules"].as_array().expect("rules");
    let mut names: Vec<&str> = members.iter().filter_map(|m| m["name"].as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["race-a", "race-b"], "{refused}");
    assert!(members.iter().all(|m| m["id"].is_null()), "{refused}");

    f.store.delete_service(trino.id).await.expect("cleanup");
    f.store.delete_service(tag_svc.id).await.expect("cleanup");
}

// ── SP4d-4 Task 11 (B1): the editor's absent levels ──────────────────────────
//
// Measured on the real Ranger jars (docs/en/plans/2026-10-11-sp4d-4-task1-
// levels-result.md): a policy that stops above `column` is denied `select` at
// column depth on both engines. An emptied level is therefore saved as `*`;
// an emptied CATALOG is refused; a level the form removed ("— none —") stays
// absent; a gap is refused. Every test below runs against the per-process
// test database and deletes its own service.

fn level(values: &[&str]) -> serde_json::Value {
    serde_json::json!({ "values": values, "isExcludes": false, "isRecursive": false })
}

async fn post_policy(
    f: &Fixture,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    into_json(
        f.app_empty()
            .oneshot(req("POST", "/policies", token, Some(body)))
            .await
            .unwrap(),
    )
    .await
}

async fn stored_policy_count(f: &Fixture, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
        .bind(service_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn an_editor_body_with_a_level_gap_is_refused() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11gap", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    // `column` is named but `schema` and `table` are absent: Ranger decides
    // nothing for it (Task 1, `gap`).
    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "gap", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"]), "column": level(&["email"])},
            "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"]
        .as_str()
        .or(body["message"].as_str())
        .unwrap_or_default();
    assert!(
        msg.contains("`column`") && msg.contains("`schema`"),
        "the refusal must name the named level and the missing one: {body}"
    );
    assert_eq!(stored_policy_count(&f, svc_id).await, 0, "nothing stored");

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// A Ranger-shaped body (`policyItems`, `dataMaskPolicyItems`,
/// `rowFilterPolicyItems`) on the editor-form endpoint used to answer 200 and
/// store a policy with EMPTY items. It is now a 400 that names the keys.
#[tokio::test]
async fn a_ranger_shaped_body_is_refused_naming_the_keys_and_stores_nothing() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11ranger", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "ranger-shaped", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"])},
            "policyItems": [{"users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}]}],
            "dataMaskPolicyItems": [], "rowFilterPolicyItems": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"]
        .as_str()
        .or(body["message"].as_str())
        .unwrap_or_default();
    for key in ["policyItems", "dataMaskPolicyItems", "rowFilterPolicyItems"] {
        assert!(msg.contains(key), "the refusal names `{key}`: {body}");
    }
    assert!(
        msg.contains("items"),
        "and points at the form's `items`: {body}"
    );
    assert_eq!(stored_policy_count(&f, svc_id).await, 0, "nothing stored");

    // The editor form itself is untouched.
    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "form-shaped", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"])},
            "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(stored_policy_count(&f, svc_id).await, 1);

    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn an_empty_catalog_is_refused() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11cat", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "no-catalog", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&[]), "schema": level(&["s"])},
            "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"]
        .as_str()
        .or(body["message"].as_str())
        .unwrap_or_default();
    assert!(
        msg.contains("name a catalog") && msg.contains("every catalog"),
        "the refusal must say why `*` is not offered here: {body}"
    );
    assert_eq!(stored_policy_count(&f, svc_id).await, 0, "nothing stored");

    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn an_empty_non_root_level_is_stored_as_star() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11fill", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "table-all-columns", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                          "table": level(&["orders"]), "column": level(&[])},
            "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["filledLevels"],
        serde_json::json!(["column"]),
        "{body}"
    );
    let note = body["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("broader policy") && !note.contains("more specific one wins"),
        "the note must say the BROADER policy is evaluated first, never that the specific one wins: {body}"
    );
    let stored = f
        .store
        .get_policy(body["id"].as_i64().unwrap())
        .await
        .expect("get");
    assert_eq!(
        stored.policy_jsonb["resources"]["column"],
        serde_json::json!({"values": ["*"], "isExcludes": false, "isRecursive": false}),
        "the emptied level is stored as `*`: {}",
        stored.policy_jsonb
    );
    // The named levels are untouched.
    assert_eq!(
        stored.policy_jsonb["resources"]["table"],
        level(&["orders"])
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

#[tokio::test]
async fn a_truncated_chain_stays_truncated() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11trunc", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    // The form removed `column` ("— none —"): the KEY is absent, which is not
    // the same as an emptied level.
    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "stops-at-table", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                          "table": level(&["orders"])},
            "items": [{"users": ["alice"], "accesses": ["insert"], "effect": "allow"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.get("filledLevels").is_none() && body.get("note").is_none(),
        "{body}"
    );
    let stored = f
        .store
        .get_policy(body["id"].as_i64().unwrap())
        .await
        .expect("get");
    assert!(
        stored.policy_jsonb["resources"].get("column").is_none(),
        "an absent level stays absent: {}",
        stored.policy_jsonb
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// The ordering ruling (user, 2026-10-12). `deny-secret` and the new policy are
/// both access policies with a deny item (so they share a `hasDeny` band) and
/// the new one's emptied `column` becomes `*`: its evalOrder drops by 25 and it
/// is now consulted BEFORE `deny-secret`, which names the column literally.
#[tokio::test]
async fn a_fill_that_moves_ahead_of_a_deny_policy_warns_naming_it() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11ord", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let (status, other) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "deny-secret", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                          "table": level(&["orders"]), "column": level(&["secret"])},
            "items": [{"users": ["bob"], "accesses": ["select"], "effect": "deny"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert!(
        other.get("warnings").is_none(),
        "no fill, no warning: {other}"
    );

    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "name": "orders-all-columns", "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                          "table": level(&["orders"]), "column": level(&[])},
            "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"},
                      {"users": ["carol"], "accesses": ["delete"], "effect": "deny"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let warnings = body["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1, "{body}");
    let w = &warnings[0];
    assert_eq!(w["kind"], "evaluation_order_changed", "{body}");
    assert_eq!(w["policy"], "deny-secret", "{body}");
    assert_eq!(w["policyId"], other["id"], "{body}");
    assert_eq!(w["policyKind"], "access", "{body}");
    let sentence = w["sentence"].as_str().unwrap_or_default();
    assert!(
        sentence.contains("\"orders-all-columns\"")
            && sentence.contains("\"deny-secret\"")
            && sentence.contains("broader policy")
            && sentence.contains("not the more specific one"),
        "the sentence names both policies and states the real direction: {body}"
    );
    assert!(
        body["note"]
            .as_str()
            .unwrap_or_default()
            .contains("EARLIER"),
        "the note explains the reorder: {body}"
    );

    // An UPDATE of the same policy reports the same pair (and does not pair
    // the new body against the row it replaces).
    let new_id = body["id"].as_i64().unwrap();
    let resp = f
        .app_empty()
        .oneshot(req(
            "PUT",
            &format!("/policies/{new_id}"),
            &token,
            Some(serde_json::json!({
                "name": "orders-all-columns", "service": svc_name, "isEnabled": true,
                "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                              "table": level(&["orders"]), "column": level(&[])},
                "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"},
                          {"users": ["carol"], "accesses": ["delete"], "effect": "deny"}]
            })),
        ))
        .await
        .unwrap();
    let (status, put) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "{put}");
    assert_eq!(put["warnings"].as_array().map(Vec::len), Some(1), "{put}");

    f.store.delete_service(svc_id).await.expect("cleanup");
}

/// The controls for the test above: each is a fill (or not) that moves the
/// policy ahead of nobody that matters, so no warning is carried.
#[tokio::test]
async fn a_fill_that_does_not_reorder_warns_nobody() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "t11ordctl", true).await;
    let (svc_name, svc_id) = seed_service(&f).await;

    let body_for = |name: &str, table: &str, column: &[&str], effect: &str, priority: bool| {
        serde_json::json!({
            "name": name, "service": svc_name, "isEnabled": true, "isOverride": priority,
            "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                          "table": level(&[table]), "column": level(column)},
            "items": [{"users": ["bob"], "accesses": ["select"], "effect": effect},
                      {"users": ["carol"], "accesses": ["delete"], "effect": "deny"}]
        })
    };
    // Four stored neighbours, each one a reason NOT to warn about the fill below.
    for (name, table, column, effect, override_band) in [
        // a different table: the two cannot decide the same request
        ("other-table", "invoices", vec!["secret"], "allow", false),
        // a different priority band (Override): Ranger orders bands first
        ("override-band", "orders", vec!["secret"], "allow", true),
        // already BROADER than the filled policy (table `*` and column `*`):
        // it stays ahead of it, so the fill reorders nothing against it
        ("broader", "*", vec!["*"], "allow", false),
    ] {
        let (status, b) = post_policy(
            &f,
            &token,
            body_for(name, table, &column, effect, override_band),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{name}: {b}");
    }
    // Allow items only on both sides (no deny, no exception): any order reaches
    // the same union, so there is nothing to warn about. Needs its own band
    // (hasDeny false), hence its own policy pair.
    let allow_only = |name: &str, column: &[&str]| {
        serde_json::json!({
            "name": name, "service": svc_name, "isEnabled": true,
            "resources": {"catalog": level(&["c"]), "schema": level(&["s"]),
                          "table": level(&["lines"]), "column": level(column)},
            "items": [{"users": ["bob"], "accesses": ["select"], "effect": "allow"}]
        })
    };
    let (status, b) = post_policy(&f, &token, allow_only("lines-secret", &["secret"])).await;
    assert_eq!(status, StatusCode::OK, "{b}");

    let (status, b) =
        post_policy(&f, &token, body_for("filled", "orders", &[], "deny", false)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(
        b["filledLevels"],
        serde_json::json!(["column"]),
        "the fill happened: {b}"
    );
    assert!(
        b.get("warnings").is_none(),
        "no stored same-band policy is overtaken by this fill: {b}"
    );

    let (status, b) = post_policy(&f, &token, allow_only("lines-all", &[])).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["filledLevels"], serde_json::json!(["column"]), "{b}");
    assert!(
        b.get("warnings").is_none(),
        "allow-only pairs are order-free: {b}"
    );

    f.store.delete_service(svc_id).await.expect("cleanup");
}
