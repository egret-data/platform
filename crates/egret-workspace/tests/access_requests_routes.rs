//! P2b requester-endpoint integration tests: the join-request lifecycle a plain
//! authenticated user drives — list `joinable` workspaces, `create` a request
//! (with duplicate + already-member rejection), list `mine`, and `cancel` (own
//! only). DB-gated; self-skips without DATABASE_URL, mirroring the sibling
//! `members_ax1.rs` harness (same `connect`/`uniq`/`ws_state`/`login_cookie`).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{access_request_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use tower::ServiceExt;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user_email(db: &Db, email: &str) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(email, "U", &hash_password("pw").unwrap())
        .await
        .unwrap()
}

async fn build_pdp(db: &Db) -> std::sync::Arc<dyn egret_core::authz::Pdp> {
    std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn ws_state(db: &Db) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
        access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
            db.clone(),
            MemberRepo::new(db.clone()),
        ),
        authz: Authorizer::new(db.clone()),
        pdp: build_pdp(db).await,
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// The full requester flow through the mounted `access_request_router`:
/// joinable → create (201) → duplicate (409) → already-member reject → mine →
/// cancel-by-other (404) → cancel-own (200).
#[tokio::test]
async fn requester_endpoints_flow() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let app = axum::Router::new()
        .nest("/api", access_request_router())
        .with_state(st.clone());

    // A fresh caller who is NOT a member of the target workspace, plus an owner.
    let caller = seed_user_email(&db, &format!("req-{}@t.local", Uuid::new_v4().simple())).await;
    let owner = seed_user_email(&db, &format!("own-{}@t.local", Uuid::new_v4().simple())).await;
    let cookie = login_cookie(caller, &st.sessions).await;

    let ws = st
        .workspaces
        .create(&uniq("ws"), "Target WS", owner)
        .await
        .unwrap();

    // 1) GET joinable → 200 and contains the workspace the caller isn't in.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/api/access-requests/joinable")
                .header("cookie", cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "joinable must 200");
    let joinable = body_json(resp).await;
    let ws_id_s = ws.id.to_string();
    assert!(
        joinable
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == serde_json::Value::String(ws_id_s.clone())),
        "joinable must contain the workspace the caller isn't a member of: {joinable}"
    );

    // 2) POST create → 201.
    let create_body = serde_json::to_vec(&serde_json::json!({
        "workspace_id": ws.id, "reason": "please let me in"
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/access-requests")
                .header("cookie", cookie.clone())
                .header("content-type", "application/json")
                .body(Body::from(create_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create must 201");

    // 3) POST create again (same ws) → 409 duplicate pending.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/access-requests")
                .header("cookie", cookie.clone())
                .header("content-type", "application/json")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "duplicate pending request must 409"
    );

    // 4) already-a-member workspace → create rejected (400/409).
    let ws2 = st
        .workspaces
        .create(&uniq("ws"), "Member WS", owner)
        .await
        .unwrap();
    st.members
        .add(ws2.id, "user", caller.0, "ws_viewer", owner)
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/access-requests")
                .header("cookie", cookie.clone())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "workspace_id": ws2.id })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::CONFLICT || resp.status() == StatusCode::BAD_REQUEST,
        "requesting a workspace the caller is already in must be rejected 400/409, got {}",
        resp.status()
    );

    // 5) GET mine → shows the pending request; grab its id.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/api/access-requests/mine")
                .header("cookie", cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "mine must 200");
    let mine = body_json(resp).await;
    let req = mine
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["workspace_id"] == serde_json::Value::String(ws_id_s.clone()))
        .expect("mine must list the request for the target ws");
    let req_id = req["id"].as_str().unwrap().to_string();

    // 6) cancel by a DIFFERENT user → 404 (not their pending request).
    let other = seed_user_email(&db, &format!("oth-{}@t.local", Uuid::new_v4().simple())).await;
    let other_cookie = login_cookie(other, &st.sessions).await;
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/api/access-requests/{req_id}/cancel"))
                .header("cookie", other_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cancel by a non-owner must 404"
    );

    // 7) cancel by self → 200 ok.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/api/access-requests/{req_id}/cancel"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "cancel own pending must 200");
    let ok = body_json(resp).await;
    assert_eq!(ok["ok"], serde_json::Value::Bool(true));
}
