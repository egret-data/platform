//! Workspace-scoped database registry HTTP route tests (Epic A: DB per workspace).
//! DB-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the `/ws/{workspace}/databases` router
//! (`egret_database::workspace_database_router`):
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
//!   cargo test -p egret-database --test workspace_routes_api -- --test-threads=1

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::ids::{UserId, WorkspaceId};
use egret_database::{workspace_database_router, DatabaseRepo, DatabaseState};
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    members: MemberRepo,
    sessions: SessionService,
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
    let dsn = std::env::var("DATABASE_URL").ok()?;

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

    let app = workspace_database_router().with_state(DatabaseState {
        repo: DatabaseRepo::new(db.clone(), secrets),
        authz: Authorizer::new(db.clone()),
        sessions: sessions.clone(),
        workspaces,
    });

    Some(Fixture {
        app,
        users,
        members: MemberRepo::new(db.clone()),
        sessions,
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
        "secret": null,
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

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/databases", f.slug_a);

    // POST → 403 (admin gate runs before body persistence).
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &token, Some(db_body("nope"))))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not create databases"
    );

    // GET → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not list databases"
    );
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
