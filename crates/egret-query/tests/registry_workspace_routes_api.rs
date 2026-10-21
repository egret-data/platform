//! Workspace-scoped database registry HTTP route tests (Epic A: DB per workspace).
//! DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! These prove the `/ws/{workspace}/databases` router
//! (`egret_query::workspace_database_router`):
//!   - a `ws_admin` member (holds `workspace:manage_members`) can create, list,
//!     fetch, update, and delete database connections scoped to its workspace,
//!   - a connection created in workspace A is listed/fetchable from A but ABSENT
//!     from / NotFound through workspace B (cross-tenant IDOR closed → 404),
//!   - a non-admin member is Forbidden (403) on every route,
//!   - testing an unsaved connection against an unreachable host returns 200 with
//!     `{ ok: false, .. }` (the tester yields an outcome, not an error).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-query --test registry_workspace_routes_api -- --test-threads=1

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_query::{workspace_database_router, DatabaseRepo, DatabaseState};
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    db: Db,
    repo: DatabaseRepo,
    users: UserRepo,
    members: MemberRepo,
    sessions: SessionService,
    workspace_secrets: egret_secrets::WorkspaceSecretStore,
    slug_a: String,
    slug_b: String,
    ws_a: WorkspaceId,
    ws_b: WorkspaceId,
    owner: UserId,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::new_v4().simple().to_string();
    let owner = users
        .insert_local(
            &format!("dbrtowner-{suffix}@egret.test"),
            "DB RT Owner",
            "x",
        )
        .await
        .expect("seed owner");

    let workspaces = WorkspaceRepo::new(db.clone());
    let slug_a = format!("dbrta{suffix}");
    let slug_b = format!("dbrtb{suffix}");
    let ws_a = workspaces
        .create(&slug_a, "DB RT A", owner)
        .await
        .expect("seed ws A");
    let ws_b = workspaces
        .create(&slug_b, "DB RT B", owner)
        .await
        .expect("seed ws B");

    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    // Built-in roles (ws_admin/ws_viewer/…) + their permissions are seeded by
    // migration 0003, so a real `CedarPdp` snapshots them as soon as it's
    // built here — no per-test role seeding needed for this fixture.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let repo = DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());
    let app = workspace_database_router().with_state(DatabaseState {
        repo: repo.clone(),
        authz: Authorizer::new(db.clone()),
        pdp,
        sessions: sessions.clone(),
        workspaces,
        workspace_secrets: workspace_secrets.clone(),
    });

    Some(Fixture {
        app,
        db: db.clone(),
        repo,
        users,
        members: MemberRepo::new(db.clone()),
        sessions,
        workspace_secrets,
        slug_a,
        slug_b,
        ws_a: ws_a.id,
        ws_b: ws_b.id,
        owner,
    })
}

/// Add a member of `workspace` with `role` and return its session token.
async fn add_member(f: &Fixture, workspace: WorkspaceId, tag: &str, role: &str) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(workspace, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
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

fn db_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "engine": "trino",
        "host": "trino.local",
        "port": 8080,
    })
}

#[tokio::test]
async fn workspace_admin_crud_and_idor_isolation() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // A distinct ws_admin in each workspace; the B admin is used to probe
    // cross-tenant access (it can legitimately read its own workspace B).
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let token_b = add_member(&f, f.ws_b, "adminb", "ws_admin").await;

    let base_a = format!("/ws/{}/databases", f.slug_a);
    let base_b = format!("/ws/{}/databases", f.slug_b);

    // CREATE in A.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base_a, &token, Some(db_body("conn-a"))))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create in A; body={json}");
    let id = json["id"].as_str().expect("id in body").to_string();

    // LIST A contains it; LIST B does not.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base_a, &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list A; body={json}");
    assert!(
        json.as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"].as_str() == Some(id.as_str())),
        "created conn must be listed for its own workspace; body={json}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base_b, &token_b, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list B; body={json}");
    assert!(
        json.as_array()
            .unwrap()
            .iter()
            .all(|d| d["id"].as_str() != Some(id.as_str())),
        "conn must NOT be listed for another workspace; body={json}"
    );

    // GET A/{id} → 200; GET B/{id} → 404 (IDOR closed).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base_a}/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "detail in A");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base_b}/{id}"), &token_b, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "fetching another workspace's db must 404 (IDOR closed)"
    );

    // PUT B/{id} → 404 (foreign mutation rejected before any change).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("{base_b}/{id}"),
            &token_b,
            Some(db_body("conn-a")),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "updating another workspace's db must 404"
    );

    // DELETE A/{id} → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{base_a}/{id}"), &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "delete in A; body={json}");
    assert_eq!(json["ok"], serde_json::json!(true));
}

/// The router over a FRESH `CedarPdp`. The PDP compiles role templates when it
/// is built, so a custom role seeded after `setup()` is invisible to `f.app`.
async fn fresh_app(f: &Fixture) -> axum::Router {
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    workspace_database_router().with_state(DatabaseState {
        repo: f.repo.clone(),
        authz: Authorizer::new(f.db.clone()),
        pdp,
        sessions: f.sessions.clone(),
        workspaces: WorkspaceRepo::new(f.db.clone()),
        workspace_secrets: f.workspace_secrets.clone(),
    })
}

/// A workspace role holding exactly `perms`.
async fn custom_role(f: &Fixture, perms: &[&str]) -> Uuid {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dbrt_{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("role");
    for p in perms {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&f.db.pool)
            .await
            .expect("perm");
    }
    role
}

async fn new_user(f: &Fixture, tag: &str) -> (UserId, String) {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    let token = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    (uid, token)
}

/// `principal_type` is `user` or `group`; `role` is a role id.
async fn grant(f: &Fixture, principal_type: &str, principal: Uuid, role: Uuid, ws: WorkspaceId) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ($1, $2, $3, 'workspace', $4)",
    )
    .bind(principal_type)
    .bind(principal)
    .bind(role)
    .bind(ws.0)
    .execute(&f.db.pool)
    .await
    .expect("grant");
}

async fn builtin_role(f: &Fixture, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .expect("builtin role")
}

/// The keys a non-admin listing may carry (D9: no host, port, options/TLS,
/// secret reference, status or catalog config).
const SUMMARY_KEYS: [&str; 4] = ["delegated", "engine", "id", "name"];

/// KI-159 / D9. A member holding `query:run` or `dataset:read` lists the
/// connections granted to THEIR workspace, redacted to id/name/engine; a
/// principal with neither, a non-member and a member of another workspace get
/// 403; the admin listing keeps the full shape.
#[tokio::test]
async fn connection_list_is_redacted_for_query_and_dataset_readers() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Arrange: one connection in each workspace, A's with a secret and TLS options.
    let admin_a = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let admin_b = add_member(&f, f.ws_b, "adminb", "ws_admin").await;
    let body_a = serde_json::json!({
        "name": "conn-visible",
        "engine": "trino",
        "host": "secret-host.internal",
        "port": 8443,
        "options": { "user": "svc_reader", "http_scheme": "https", "catalog": "hidden_catalog" },
        "new_secret": { "name": "conn-visible-creds", "value": "p4ssw0rd-value" },
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/databases", f.slug_a),
            &admin_a,
            Some(body_a),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create in A; body={json}");
    let id_a = json["id"].as_str().unwrap().to_string();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/databases", f.slug_b),
            &admin_b,
            Some(db_body("conn-other-ws")),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "create in B");

    let viewer = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;
    let editor_direct = add_member(&f, f.ws_a, "editor", "ws_editor").await;
    // ws_editor through a group.
    let (group_user, editor_via_group) = new_user(&f, "grpeditor").await;
    let group: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("dbrt_{}", Uuid::new_v4().simple()))
        .fetch_one(&f.db.pool)
        .await
        .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(group_user.0)
        .bind(group)
        .execute(&f.db.pool)
        .await
        .expect("membership");
    let ws_editor = builtin_role(&f, "ws_editor").await;
    grant(&f, "group", group, ws_editor, f.ws_a).await;
    // Custom roles: only query:run; only dataset:read; neither.
    let (qr_user, query_only) = new_user(&f, "queryonly").await;
    let qr_role = custom_role(&f, &["query:run"]).await;
    grant(&f, "user", qr_user.0, qr_role, f.ws_a).await;
    let (dr_user, dataset_only) = new_user(&f, "datasetonly").await;
    let dr_role = custom_role(&f, &["dataset:read"]).await;
    grant(&f, "user", dr_user.0, dr_role, f.ws_a).await;
    let (cr_user, chart_only) = new_user(&f, "chartonly").await;
    let cr_role = custom_role(&f, &["chart:read"]).await;
    grant(&f, "user", cr_user.0, cr_role, f.ws_a).await;
    let (_nobody, non_member) = new_user(&f, "nonmember").await;
    let app = fresh_app(&f).await;
    let list_a = format!("/ws/{}/databases", f.slug_a);

    // Act + Assert: the readers get the redacted listing of A only.
    for (who, token) in [
        ("ws_viewer (dataset:read)", &viewer),
        ("ws_editor direct", &editor_direct),
        ("ws_editor through a group", &editor_via_group),
        ("custom role with only query:run", &query_only),
        ("custom role with only dataset:read", &dataset_only),
    ] {
        let resp = app
            .clone()
            .oneshot(req("GET", &list_a, token, None))
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let raw = String::from_utf8_lossy(&bytes).to_string();
        assert_eq!(status, StatusCode::OK, "{who} may list; body={raw}");
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rows.len(), 1, "{who}: only A's connection; body={raw}");
        assert_eq!(rows[0]["id"].as_str(), Some(id_a.as_str()));
        assert_eq!(rows[0]["name"], "conn-visible");
        assert_eq!(rows[0]["engine"], "trino");
        let mut keys: Vec<&str> = rows[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, SUMMARY_KEYS, "{who}: redacted shape only; body={raw}");
        for leak in [
            "secret-host.internal",
            "8443",
            "svc_reader",
            "hidden_catalog",
            "conn-visible-creds",
            "p4ssw0rd-value",
            "conn-other-ws",
        ] {
            assert!(!raw.contains(leak), "{who}: body leaks {leak:?}: {raw}");
        }
    }

    // A principal with none of the permissions, a non-member and a member of
    // another workspace are refused.
    for (who, token) in [
        ("custom role with only chart:read", &chart_only),
        ("non-member", &non_member),
        ("ws_admin of another workspace", &admin_b),
    ] {
        let resp = app
            .clone()
            .oneshot(req("GET", &list_a, token, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{who} must get 403");
    }

    // The admin listing is unchanged: the full connection detail.
    let resp = app
        .clone()
        .oneshot(req("GET", &list_a, &admin_a, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "admin list; body={json}");
    let row = &json.as_array().unwrap()[0];
    assert_eq!(row["host"], "secret-host.internal", "body={json}");
    assert_eq!(row["port"], 8443);
    assert_eq!(row["options"]["user"], "svc_reader");
    assert_eq!(row["has_secret"], true);
    for key in ["status", "governed", "engine_id"] {
        assert!(row.get(key).is_some(), "admin row keeps {key}; body={json}");
    }
}

/// D9 keeps every write, test and per-connection detail admin-only: a member
/// who may now LIST still gets 403 on everything else.
#[tokio::test]
async fn non_admin_member_cannot_manage_or_test_connections() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let base = format!("/ws/{}/databases", f.slug_a);
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &admin, Some(db_body("conn-a"))))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create; body={json}");
    let id = json["id"].as_str().unwrap().to_string();

    for role in ["ws_viewer", "ws_editor"] {
        let token = add_member(&f, f.ws_a, role, role).await;
        let test_body = serde_json::json!({
            "engine": "trino", "host": "127.0.0.1", "port": 1,
        });
        for (method, uri, body) in [
            ("POST", base.clone(), Some(db_body("nope"))),
            ("GET", format!("{base}/{id}"), None),
            ("PUT", format!("{base}/{id}"), Some(db_body("renamed"))),
            ("DELETE", format!("{base}/{id}"), None),
            ("POST", format!("{base}/{id}/test"), None),
            ("POST", format!("{base}/test"), Some(test_body)),
        ] {
            let resp = f
                .app
                .clone()
                .oneshot(req(method, &uri, &token, body))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "{role} must not {method} {uri}"
            );
        }
    }
    // Nothing changed.
    let row = f.repo.get_in_workspace(f.ws_a.0, id.parse().unwrap()).await;
    assert_eq!(row.expect("still there").name, "conn-a");
}

#[tokio::test]
async fn create_with_new_secret_mints_named_secret() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let base = format!("/ws/{}/databases", f.slug_a);

    // Epic B: `new_secret` mints a named secret inline and binds the connection.
    let body = serde_json::json!({
        "name": "conn-newsecret",
        "engine": "trino",
        "host": "trino.local",
        "port": 8080,
        "new_secret": { "name": "db-creds", "value": "s3cr3t" },
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &token, Some(body)))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create with new_secret; body={json}"
    );
    assert_eq!(
        json["has_secret"],
        serde_json::json!(true),
        "a minted named secret must make has_secret true; body={json}"
    );
    // The named secret exists in the workspace with used_by = 1.
    let secrets = f
        .workspace_secrets
        .list(Some(f.ws_a.0))
        .await
        .expect("list secrets");
    let meta = secrets
        .iter()
        .find(|s| s.name == "db-creds")
        .expect("minted named secret listed");
    assert_eq!(meta.used_by, 1, "the new connection references the secret");
}

#[tokio::test]
async fn create_with_secret_id_references_existing_named_secret() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let base = format!("/ws/{}/databases", f.slug_a);

    // Pre-create a named secret, then reference it by id.
    let secret_id = f
        .workspace_secrets
        .create(Some(f.ws_a.0), "shared-db-creds", None, "pre-made")
        .await
        .expect("pre-create secret");

    let body = serde_json::json!({
        "name": "conn-secretid",
        "engine": "trino",
        "host": "trino.local",
        "port": 8080,
        "secret_id": secret_id,
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &token, Some(body)))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create with secret_id; body={json}");
    assert_eq!(
        json["has_secret"],
        serde_json::json!(true),
        "referencing an existing named secret must make has_secret true; body={json}"
    );
    let meta = f
        .workspace_secrets
        .get_meta(Some(f.ws_a.0), secret_id)
        .await
        .expect("get_meta");
    assert_eq!(meta.used_by, 1, "the connection now references the secret");
}

/// Seed a storage connection + warehouse (owned by `workspace_id`) + engine and
/// return the engine id (the FK target that marks a `query.databases` row
/// provisioned). Raw SQL against the shared schemas — no catalog/engine dep.
async fn seed_engine(db: &Db, workspace_id: Uuid) -> Uuid {
    let uniq = |p: &str| format!("{p}_{}", Uuid::new_v4().simple());
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(uniq("store"))
    .fetch_one(&db.pool)
    .await
    .expect("seed storage connection");
    let _wh_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(uniq("wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed warehouse");
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, 'trino', 'egret-engines', '{}'::jsonb) RETURNING id",
    )
    .bind(uniq("eng"))
    .fetch_one(&db.pool)
    .await
    .expect("seed engine")
}

#[tokio::test]
async fn provisioned_databases_are_read_only() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;

    // Materialize a provisioned (engine-assigned) connection in workspace A.
    let engine_id = seed_engine(&f.db, f.ws_a.0).await;
    let provisioned = f
        .repo
        .create_provisioned(
            f.ws_a.0,
            engine_id,
            &format!("prov-{}", Uuid::new_v4().simple()),
            "trino",
            "trino-prod-coordinator.egret-engines.svc",
            8080,
            &serde_json::json!({}),
            None,
            None,
        )
        .await
        .expect("materialize provisioned connection");

    let base = format!("/ws/{}/databases", f.slug_a);
    let path = format!("{base}/{}", provisioned.id);

    // PUT (edit) → 409.
    let resp = f
        .app
        .clone()
        .oneshot(req("PUT", &path, &token, Some(db_body("renamed"))))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "editing a provisioned connection must be rejected with 409"
    );

    // Test-connection → 409.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("{path}/test"), &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "testing a provisioned connection must be rejected with 409"
    );

    // DELETE → 409.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &path, &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "deleting a provisioned connection must be rejected with 409"
    );

    // The read path still works — the row was not touched.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &path, &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a provisioned connection remains readable"
    );
}

/// B1: the workspace database list/detail DTO carries `engine_id` so the
/// frontend can tell a provisioned (read-only) row from a self-service one.
#[tokio::test]
async fn ws_database_list_and_detail_expose_engine_id() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;

    let engine_id = seed_engine(&f.db, f.ws_a.0).await;
    let provisioned = f
        .repo
        .create_provisioned(
            f.ws_a.0,
            engine_id,
            &format!("prov-{}", Uuid::new_v4().simple()),
            "trino",
            "trino-prod-coordinator.egret-engines.svc",
            8080,
            &serde_json::json!({}),
            None,
            None,
        )
        .await
        .expect("materialize provisioned connection");

    let base = format!("/ws/{}/databases", f.slug_a);

    // LIST carries engine_id on the provisioned row.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list; body={json}");
    let row = json
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"].as_str() == Some(provisioned.id.to_string().as_str()))
        .expect("provisioned row listed");
    assert_eq!(
        row["engine_id"].as_str(),
        Some(engine_id.to_string().as_str()),
        "list DTO carries engine_id; body={row}"
    );

    // DETAIL carries engine_id too.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("{base}/{}", provisioned.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "detail; body={json}");
    assert_eq!(
        json["engine_id"].as_str(),
        Some(engine_id.to_string().as_str()),
        "detail DTO carries engine_id; body={json}"
    );

    // A self-service connection has engine_id null.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &token, Some(db_body("self-service"))))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create self-service; body={json}");
    assert!(
        json["engine_id"].is_null(),
        "a self-service connection has engine_id null; body={json}"
    );
}

/// Seed an engine assigned to `workspace_id`, with one registry catalog
/// (attached to the engine) whose secret ref points at a named workspace
/// secret. The engine name is `{prefix}-{uuid}` (engines.name is globally
/// UNIQUE across the shared test DB). Returns `(engine_id, engine_name,
/// storage_name, secret_name, catalog_name)` for the ws-member view
/// assertions.
async fn seed_assigned_engine_with_catalog(
    db: &Db,
    workspace_secrets: &egret_secrets::WorkspaceSecretStore,
    workspace_id: Uuid,
    owner: UserId,
    prefix: &str,
) -> (Uuid, String, String, String, String) {
    let engine_name = format!("{prefix}-{}", Uuid::new_v4().simple());
    // A storage connection + warehouse + engine (known storage name).
    let store_name = format!("store_{}", Uuid::new_v4().simple());
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(&store_name)
    .fetch_one(&db.pool)
    .await
    .expect("seed storage connection");
    let wh_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("wh_{}", Uuid::new_v4().simple()))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status) \
         VALUES ($1, 'trino', 'egret-engines', '{}'::jsonb, 'running') RETURNING id",
    )
    .bind(&engine_name)
    .fetch_one(&db.pool)
    .await
    .expect("seed engine");

    // A named workspace secret (via the store, which seals the underlying
    // `secrets` row) to bind as the catalog's secret_ref.
    let secret_name = format!("cat-creds-{}", Uuid::new_v4().simple());
    let secret_ref = workspace_secrets
        .create(Some(workspace_id), &secret_name, None, "catalog-password")
        .await
        .expect("seed named catalog secret");

    // A registry catalog (the engine's warehouse) referencing the secret,
    // attached to the engine.
    let catalog_name = format!("lakehouse_{}", Uuid::new_v4().simple());
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&catalog_name)
    .bind(wh_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed registry catalog");
    sqlx::query("INSERT INTO registry.catalog_secret_refs (catalog_id, secret_id) VALUES ($1, $2)")
        .bind(catalog_id)
        .bind(secret_ref)
        .execute(&db.pool)
        .await
        .expect("seed secret ref");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect("seed attachment");

    // The assignment row.
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_workspaces (engine_id, workspace_id, assigned_by) \
         VALUES ($1, $2, $3)",
    )
    .bind(engine_id)
    .bind(workspace_id)
    .bind(owner.0)
    .execute(&db.pool)
    .await
    .expect("seed engine_workspaces assignment");

    (
        engine_id,
        engine_name,
        store_name,
        secret_name,
        catalog_name,
    )
}

/// B2: a workspace member sees its assigned engines read-only, with each
/// catalog resolved to the storage/secret NAME it maps to; an engine that is
/// not assigned to this workspace is 404 (never leaked).
#[tokio::test]
async fn ws_engine_view_lists_assigned_engines_with_catalogs() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;

    let (engine_id, engine_name, storage_name, secret_name, catalog_name) =
        seed_assigned_engine_with_catalog(
            &f.db,
            &f.workspace_secrets,
            f.ws_a.0,
            f.owner,
            "trino-prod",
        )
        .await;

    // LIST → exactly the one assigned engine.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list engines; body={json}");
    let arr = json.as_array().expect("array");
    assert_eq!(arr.len(), 1, "one assigned engine; body={json}");
    assert_eq!(arr[0]["id"].as_str(), Some(engine_id.to_string().as_str()));
    assert_eq!(arr[0]["name"].as_str(), Some(engine_name.as_str()));
    assert_eq!(arr[0]["type"], serde_json::json!("trino"));
    assert_eq!(arr[0]["status"], serde_json::json!("running"));

    // DETAIL → name/type/status + catalogs with storage/secret names.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines/{engine_id}", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, d) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "engine detail; body={d}");
    assert_eq!(d["name"].as_str(), Some(engine_name.as_str()));
    assert_eq!(d["type"], serde_json::json!("trino"));
    let cats = d["catalogs"].as_array().expect("catalogs array");
    assert_eq!(cats.len(), 1, "one catalog; body={d}");
    assert_eq!(cats[0]["name"], serde_json::json!(catalog_name));
    assert_eq!(cats[0]["kind"], serde_json::json!("egret-warehouse"));
    assert_eq!(
        cats[0]["storage_name"].as_str(),
        Some(storage_name.as_str()),
        "storage_name resolved via the engine's warehouse; body={d}"
    );
    assert_eq!(
        cats[0]["secret_name"].as_str(),
        Some(secret_name.as_str()),
        "secret_name resolved from secret_ref (name only); body={d}"
    );

    // An engine NOT assigned to this workspace → 404.
    let other = seed_engine(&f.db, f.ws_b.0).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines/{other}", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "an unassigned engine must 404 through this workspace"
    );
}

/// Whole-branch review F-8: `storage_name` is a property of the CATALOG, not
/// of the engine. Since the view started listing every attached catalog, the
/// engine's primary warehouse storage was copied onto all of them — a
/// confidently wrong storage for a second warehouse's catalog, and an invented
/// one for a jdbc catalog that has no warehouse at all.
#[tokio::test]
async fn ws_engine_view_resolves_storage_name_per_catalog() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let (engine_id, _engine_name, primary_storage, _secret_name, primary_catalog) =
        seed_assigned_engine_with_catalog(
            &f.db,
            &f.workspace_secrets,
            f.ws_a.0,
            f.owner,
            "trino-storage",
        )
        .await;

    // A SECOND warehouse, on its own storage connection, attached to the same
    // engine as a non-primary catalog.
    let second_storage = format!("store_{}", Uuid::new_v4().simple());
    let sc2: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'eu-west-1', 'second', 'egret', true) RETURNING id",
    )
    .bind(&second_storage)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed second storage connection");
    let wh2: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("wh_{}", Uuid::new_v4().simple()))
    .bind(f.ws_a.0)
    .bind(sc2)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed second warehouse");
    let second_catalog = format!("second_{}", Uuid::new_v4().simple());
    let cat2: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&second_catalog)
    .bind(wh2)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed second warehouse catalog");

    // A jdbc catalog: no warehouse, therefore no storage at all. Assigned to
    // this workspace, since only warehouse catalogs have the owner fallback.
    let jdbc_catalog = format!("pg_{}", Uuid::new_v4().simple());
    let cat3: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'jdbc', jsonb_build_object('dialect', 'postgresql', 'host', 'db', \
                 'port', 5432, 'database', 'sales', 'user', 'u')) RETURNING id",
    )
    .bind(&jdbc_catalog)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed jdbc catalog");
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1,$2)",
    )
    .bind(cat3)
    .bind(f.ws_a.0)
    .execute(&f.db.pool)
    .await
    .expect("assign jdbc catalog");
    for cat in [cat2, cat3] {
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
        )
        .bind(engine_id)
        .bind(cat)
        .execute(&f.db.pool)
        .await
        .expect("attach catalog");
    }

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines/{engine_id}", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, d) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "engine detail; body={d}");
    let by_name: std::collections::HashMap<&str, &serde_json::Value> = d["catalogs"]
        .as_array()
        .expect("catalogs array")
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c))
        .collect();
    assert_eq!(by_name.len(), 3, "three attached catalogs; body={d}");
    assert_eq!(
        by_name[primary_catalog.as_str()]["storage_name"].as_str(),
        Some(primary_storage.as_str()),
        "the primary warehouse catalog keeps its own storage; body={d}"
    );
    assert_eq!(
        by_name[second_catalog.as_str()]["storage_name"].as_str(),
        Some(second_storage.as_str()),
        "the second warehouse's catalog must show ITS storage; body={d}"
    );
    assert_eq!(
        by_name[jdbc_catalog.as_str()]["storage_name"],
        serde_json::Value::Null,
        "a jdbc catalog has no warehouse and therefore no storage; body={d}"
    );
    assert_eq!(
        by_name[jdbc_catalog.as_str()]["kind"],
        serde_json::json!("postgresql")
    );
}

/// SP3 catalog-registry review N2 (user ruling: filter by registry
/// assignment now, in SP3a). The workspace engine view's per-engine catalog
/// list is visibility-filtered by `registry.catalog_workspaces`, mirroring
/// `RegistryStore::warehouse_visible_to_workspace`'s two-branch shape:
///   1. a catalog explicitly assigned to this workspace is listed, even when
///      a DIFFERENT workspace owns the underlying warehouse;
///   2. a catalog attached to the SAME engine but assigned only to another
///      workspace is absent;
///   3. an un-backfilled warehouse catalog (no `catalog_workspaces` row at
///      all yet) still appears for the workspace that OWNS the warehouse
///      directly — the fallback.
/// This is READ VISIBILITY only, not enforcement: a hidden catalog's engine
/// would still answer a direct query for it (unchanged, untested here).
#[tokio::test]
async fn ws_engine_view_catalogs_filtered_by_registry_assignment() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;

    // Scenario 3: the engine's PRIMARY warehouse catalog, seeded directly (no
    // `catalog_workspaces` row at all), owned by ws_a via
    // `catalog.warehouses.workspace_id` — the un-backfilled fallback.
    let (engine_id, _engine_name, _storage_name, _secret_name, primary_catalog_name) =
        seed_assigned_engine_with_catalog(
            &f.db,
            &f.workspace_secrets,
            f.ws_a.0,
            f.owner,
            "n2-primary",
        )
        .await;

    // Scenario 1: a SECOND warehouse, owned by ws_B, whose registry catalog is
    // attached to the SAME engine as a secondary catalog and explicitly
    // ASSIGNED to ws_A — proves assignment, not warehouse ownership, drives
    // visibility.
    let sc2: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("n2-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("seed second storage connection");
    let wh2: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("n2-wh-{}", Uuid::new_v4().simple()))
    .bind(f.ws_b.0)
    .bind(sc2)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed second warehouse");
    let assigned_catalog_name = format!("n2_assigned_{}", Uuid::new_v4().simple());
    let assigned_catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&assigned_catalog_name)
    .bind(wh2)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed second registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine_id)
    .bind(assigned_catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach second catalog");
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2)",
    )
    .bind(assigned_catalog_id)
    .bind(f.ws_a.0)
    .execute(&f.db.pool)
    .await
    .expect("assign second catalog to ws_a");

    // Scenario 2: a THIRD catalog, attached to the SAME engine, assigned ONLY
    // to ws_B — must be absent from ws_A's view.
    let sc3: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("n2-sc3-{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("seed third storage connection");
    let wh3: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("n2-wh3-{}", Uuid::new_v4().simple()))
    .bind(f.ws_b.0)
    .bind(sc3)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed third warehouse");
    let hidden_catalog_name = format!("n2_hidden_{}", Uuid::new_v4().simple());
    let hidden_catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&hidden_catalog_name)
    .bind(wh3)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed third registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine_id)
    .bind(hidden_catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach third catalog");
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2)",
    )
    .bind(hidden_catalog_id)
    .bind(f.ws_b.0)
    .execute(&f.db.pool)
    .await
    .expect("assign third catalog to ws_b only");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines/{engine_id}", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, d) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "engine detail; body={d}");
    let names: Vec<String> = d["catalogs"]
        .as_array()
        .expect("catalogs array")
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        names.contains(&primary_catalog_name),
        "the un-backfilled owned warehouse catalog must still appear; got {names:?}"
    );
    assert!(
        names.contains(&assigned_catalog_name),
        "a catalog explicitly assigned to ws_a must appear even though ws_b owns its warehouse; got {names:?}"
    );
    assert!(
        !names.contains(&hidden_catalog_name),
        "a catalog attached to the engine but assigned only to ws_b must NOT appear; got {names:?}"
    );
    assert_eq!(
        names.len(),
        2,
        "exactly the two visible catalogs, got {names:?}"
    );
}

/// B2: a non-admin member (ws_viewer holds `catalog:read`) may read the engine
/// view — it is a member read, not a manage-only action.
#[tokio::test]
async fn ws_engine_view_allows_non_admin_member() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;
    let (_engine_id, _name, _s, _sec, _cat) = seed_assigned_engine_with_catalog(
        &f.db,
        &f.workspace_secrets,
        f.ws_a.0,
        f.owner,
        "trino-viewer",
    )
    .await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a viewer holds catalog:read and may see assigned engines; body={json}"
    );
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_unsaved_unreachable_host_is_200_not_ok() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let uri = format!("/ws/{}/databases/test", f.slug_a);

    let body = serde_json::json!({
        "engine": "trino",
        "host": "127.0.0.1",
        "port": 1,
        "secret": null,
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &uri, &token, Some(body)))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the tester returns an outcome, not an error; body={json}"
    );
    assert_eq!(
        json["ok"],
        serde_json::json!(false),
        "an unreachable host must report ok=false; body={json}"
    );
}

/// Pins the B1b-2 `==` anti-leak for the SAME-NAME case: even if a platform
/// role held `catalog:read` ITSELF — the literal workspace-scope key from
/// 0024 — Cedar still cannot let that platform grant satisfy the workspace
/// check, because `template_for_role` (`crates/egret-auth/src/pdp/templates.rs`)
/// gives a non-`platform_admin` platform-scoped role `resource == ?resource`,
/// and `scope_uid("platform", ..)` (`entities.rs`) binds that slot to
/// `Egret::Platform::"root"` — which can never `==` an `Egret::Workspace::<id>`
/// entity, same action name or not. (SP3 still keeps `catalog_registry:*`
/// DISTINCT from `catalog:read` — not because Cedar would confuse them, but
/// because permission keys are scope-blind strings everywhere else they're
/// read: `rbac::is_known`, the effective-permission lists the admin
/// permission UI renders, and the single Cedar action namespace a policy
/// names by string.) The HTTP 403 below is also produced by WsContext's
/// membership gate, so the direct PDP assertions are what actually
/// discriminate: they fail if `template_for_role`'s platform-scope operator
/// ever regresses from `==` to `in` (verified by hand — see the task-8 fix
/// report for the RED/GREEN transcript of temporarily making that change).
#[tokio::test]
async fn platform_grant_never_satisfies_workspace_scope_check() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let email = format!("regreader_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') RETURNING id",
    )
    .bind(format!("test_role_{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("insert role");
    // The COLLIDING key itself: this role holds `catalog:read` — the literal
    // workspace-scope permission — but only ever granted at PLATFORM scope.
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'catalog:read')")
        .bind(role_id)
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
    let token = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    // Not a member of ws_a: the workspace engine view is 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/engines", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The PDP itself: the role was seeded after `setup()` built its PDP, so build
    // a fresh one that sees it.
    let pdp = egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
        .await
        .expect("build CedarPdp");
    use egret_core::authz::{Pdp, Resource, Subject};
    let subject = Subject {
        user_id: uid,
        group_ids: vec![],
    };
    let platform = pdp
        .authorize(&subject, "catalog:read", &Resource::Platform)
        .await
        .expect("authorize");
    assert!(platform.allow, "positive control: the platform grant works");
    let workspace = pdp
        .authorize(&subject, "catalog:read", &Resource::Workspace(f.ws_a))
        .await
        .expect("authorize");
    assert!(
        !workspace.allow,
        "a platform-scope grant of catalog:read must not satisfy the workspace-scope \
         catalog:read check on a specific workspace (B1b-2 anti-leak)"
    );
}
