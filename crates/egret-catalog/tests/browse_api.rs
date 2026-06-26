//! Session-authenticated catalog browse API tests (DB-gated; self-skip without
//! `DATABASE_URL`).
//!
//! Proves the RBAC gate on the internal `/ws/{workspace}/catalog/...` tree:
//! - a workspace member WITH `catalog:read` (ws_viewer) can list namespaces /
//!   tables and read table detail (200),
//! - a member WITHOUT the perm / a non-member is Forbidden (403),
//! - an unauthenticated request is 401.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog::{browse_router, BrowseState};
use egret_metastore::Db;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use std::collections::HashMap;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    store: CatalogStore,
    sessions: SessionService,
    users: UserRepo,
    slug: String,
    workspace_id: Uuid,
    catalog_id: Uuid,
    ns: Vec<String>,
}

async fn setup() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let store = CatalogStore::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let users = UserRepo::new(db.clone());

    // Seed a fresh workspace + its catalog + a namespace with one table.
    let workspace_id = Uuid::new_v4();
    let slug = format!("ws_{}", workspace_id.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(workspace_id)
        .bind(&slug)
        .bind(&slug)
        .execute(store.pool())
        .await
        .expect("seed workspace");
    let catalog = store
        .create_catalog(workspace_id, "default", "s3://wh")
        .await
        .expect("seed catalog");

    let ns = vec![format!("ns_{}", Uuid::new_v4().simple())];
    let ns_row = store
        .create_namespace(catalog.id, &ns, &HashMap::new())
        .await
        .expect("seed namespace");

    let table_uuid = Uuid::new_v4();
    let loc = "s3://wh/ns/t";
    let mloc = "s3://wh/ns/t/metadata/0.json";
    let md = egret_catalog::service::test_support::minimal_table_metadata(loc, table_uuid);
    store
        .insert_table(ns_row.id, "events", table_uuid, &md, mloc, loc)
        .await
        .expect("seed table");

    let app = browse_router().with_state(BrowseState {
        store: store.clone(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        store,
        sessions,
        users,
        slug,
        workspace_id,
        catalog_id: catalog.id,
        ns,
    })
}

/// Create a user and mint a session token (the cookie value).
async fn mint_session(f: &Fixture, email_tag: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    let issued = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session");
    issued.token
}

/// Add the user behind `token` as a workspace member with `role`.
async fn add_member(f: &Fixture, email_tag: &str, role: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role) \
         VALUES ($1, 'user', $2, $3)",
    )
    .bind(f.workspace_id)
    .bind(uid.0)
    .bind(role)
    .execute(f.store.pool())
    .await
    .expect("seed member");
    let issued = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session");
    issued.token
}

/// Seed a second, independent workspace + its catalog. Returns its slug.
/// The fixture user is NOT a member of this workspace.
async fn seed_other_workspace(f: &Fixture) -> String {
    let ws = Uuid::new_v4();
    let slug = format!("ws_{}", ws.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(ws)
        .bind(&slug)
        .bind(&slug)
        .execute(f.store.pool())
        .await
        .expect("seed other workspace");
    f.store
        .create_catalog(ws, "default", "s3://wh2")
        .await
        .expect("seed other catalog");
    slug
}

fn get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri);
    if let Some(t) = token {
        b = b.header("Cookie", format!("egret_session={t}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn unauthenticated_is_401() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let uri = format!("/ws/{}/catalog/namespaces", f.slug);
    let resp = f.app.clone().oneshot(get(&uri, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn viewer_can_browse_namespaces_tables_and_detail() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    // summary
    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{}/catalog", f.slug), Some(&token)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "summary");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"], f.catalog_id.to_string());
    assert_eq!(json["warehouse_location"], "s3://wh");

    // list namespaces
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces", f.slug),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "namespaces");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let found = json
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["levels"] == serde_json::json!(f.ns));
    assert!(found, "seeded namespace must be listed: {json}");

    // list tables in the namespace
    let ns_path = f.ns.join(".");
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces/{}/tables", f.slug, ns_path),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "tables");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["tables"], serde_json::json!(["events"]));
    assert_eq!(json["views"], serde_json::json!([]));

    // table detail DTO
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!(
                "/ws/{}/catalog/namespaces/{}/tables/events",
                f.slug, ns_path
            ),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "table detail");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], "events");
    assert_eq!(json["columns"][0]["name"], "id");
    assert_eq!(json["columns"][0]["type"], "long");
    assert_eq!(json["columns"][0]["required"], true);
    assert_eq!(json["snapshot_count"], 0);
    assert!(json["current_snapshot_id"].is_null());
}

#[tokio::test]
async fn non_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Authenticated user but NOT a member of the workspace.
    let token = mint_session(&f, "stranger").await;
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces", f.slug),
            Some(&token),
        ))
        .await
        .unwrap();
    // The WsContext extractor rejects non-members (no workspace role) at membership
    // resolution → 403.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// Cross-workspace IDOR: a member of workspace X requesting workspace Y's catalog
/// must be Forbidden. This is the core tenant-isolation guarantee: holding a role
/// in ANY workspace must not leak another workspace's catalog.
#[tokio::test]
async fn member_of_x_cannot_read_y_catalog_idor() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // The user is a ws_viewer of X (the fixture workspace) — a legitimately
    // authorized member there.
    let token = add_member(&f, "x_member", "ws_viewer").await;

    // Seed a separate workspace Y (user is NOT a member) and hit Y's slug.
    let slug_y = seed_other_workspace(&f).await;

    // Sanity: the same token DOES work against the user's own workspace X.
    let ok = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{}/catalog", f.slug), Some(&token)))
        .await
        .unwrap();
    assert_eq!(
        ok.status(),
        StatusCode::OK,
        "member must reach own workspace"
    );

    // IDOR attempt: X-member token against Y's slug → 403.
    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{slug_y}/catalog"), Some(&token)))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a member of X must not read workspace Y's catalog"
    );
}

/// Member WITHOUT `catalog:read`: a workspace member whose role grants no
/// catalog permission must be Forbidden. All STANDARD workspace roles
/// (ws_viewer/ws_editor/ws_admin) hold `catalog:read` per migration 0024, so the
/// "member but no read perm" state is only reachable via a role that carries no
/// such grant. We seed membership under a role name that has no `role_permissions`
/// row: the WsContext extractor accepts the membership (role resolves) but
/// `require_workspace("catalog:read")` finds no permission → 403.
#[tokio::test]
async fn member_without_catalog_read_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // `ws_norole` is not in the seeded `roles` table, so its permission union is
    // empty — membership is valid but `catalog:read` is absent.
    let token = add_member(&f, "norole", "ws_norole").await;
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces", f.slug),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "member whose role lacks catalog:read must be 403"
    );
}
