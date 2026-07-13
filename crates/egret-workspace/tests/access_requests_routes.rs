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
use egret_workspace::routes::{access_request_router, ws_member_router, WorkspaceState};
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

/// The approver flow through the workspace-scoped `ws_member_router`:
/// a ws_admin lists the pending inbox (a ws_viewer is 403), approves one request
/// (→ member with the chosen role + expiry, request `approved`), is blocked from
/// approving a request that belongs to a DIFFERENT workspace (404), and denies
/// another with a note.
#[tokio::test]
async fn approver_endpoints_flow() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());

    // A ws_admin, a plain ws_viewer member, and two outside requesters.
    let admin = seed_user_email(&db, &format!("adm-{}@t.local", Uuid::new_v4().simple())).await;
    let viewer = seed_user_email(&db, &format!("viw-{}@t.local", Uuid::new_v4().simple())).await;
    let req_email = format!("rqr-{}@t.local", Uuid::new_v4().simple());
    let requester = seed_user_email(&db, &req_email).await;
    let requester2 =
        seed_user_email(&db, &format!("rq2-{}@t.local", Uuid::new_v4().simple())).await;

    let ws = st
        .workspaces
        .create(&uniq("ws"), "Approver WS", admin)
        .await
        .unwrap();
    st.members
        .add(ws.id, "user", admin.0, "ws_admin", admin)
        .await
        .unwrap();
    st.members
        .add(ws.id, "user", viewer.0, "ws_viewer", admin)
        .await
        .unwrap();

    // Seed two pending requests in THIS workspace.
    let pending = st
        .access_requests
        .create(requester, ws.id, "ws_viewer", "let me in")
        .await
        .unwrap();
    let pending2 = st
        .access_requests
        .create(requester2, ws.id, "ws_viewer", "me too")
        .await
        .unwrap();

    let admin_cookie = login_cookie(admin, &st.sessions).await;
    let viewer_cookie = login_cookie(viewer, &st.sessions).await;

    // 1) ws_admin GET inbox (default status=pending) → 200 with the request + requester_name.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/access-requests", ws.slug))
                .header("cookie", admin_cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin inbox must 200");
    let inbox = body_json(resp).await;
    let row = inbox
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == serde_json::Value::String(pending.id.to_string()))
        .expect("inbox must contain the pending request");
    assert_eq!(
        row["requester_name"],
        serde_json::Value::String(req_email.clone()),
        "inbox row carries the requester's display name"
    );

    // 2) ws_viewer GET inbox → 403 (not manage_members).
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/access-requests", ws.slug))
                .header("cookie", viewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-admin inbox must 403"
    );

    // 3) approve with role=ws_editor + 7d expiry → 200; member added with that role+expiry; status approved.
    let expires = chrono::Utc::now() + chrono::Duration::days(7);
    let approve_body = serde_json::to_vec(&serde_json::json!({
        "role": "ws_editor", "expires_at": expires.to_rfc3339()
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!(
                    "/api/ws/{}/access-requests/{}/approve",
                    ws.slug, pending.id
                ))
                .header("cookie", admin_cookie.clone())
                .header("content-type", "application/json")
                .body(Body::from(approve_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "approve must 200");
    assert_eq!(body_json(resp).await["ok"], serde_json::Value::Bool(true));

    assert_eq!(
        st.members
            .role_of(ws.id, "user", requester.0)
            .await
            .unwrap()
            .as_deref(),
        Some("ws_editor"),
        "approve must add the requester as ws_editor"
    );
    let (grant_role, grant_expiry): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as(
            "SELECT r.name, ag.expires_at FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
               AND ag.principal_type = 'user' AND ag.principal_id = $2",
        )
        .bind(ws.id.0)
        .bind(requester.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(grant_role, "ws_editor");
    let ge = grant_expiry.expect("grant must carry the 7d expiry");
    assert!(
        (ge - expires).num_seconds().abs() < 5,
        "grant expiry must match the requested 7d expiry"
    );
    assert_eq!(
        st.access_requests
            .get(pending.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "approved",
        "request status must be approved"
    );

    // 4) approve a request that belongs to a DIFFERENT workspace, via THIS ws's path → 404.
    let ws_other = st
        .workspaces
        .create(&uniq("ws"), "Other WS", admin)
        .await
        .unwrap();
    let foreign = st
        .access_requests
        .create(requester2, ws_other.id, "ws_viewer", "elsewhere")
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!(
                    "/api/ws/{}/access-requests/{}/approve",
                    ws.slug, foreign.id
                ))
                .header("cookie", admin_cookie.clone())
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "approving a cross-workspace request must 404"
    );

    // 5) deny another pending request with a note → 200, status denied, note stored.
    let deny_body = serde_json::to_vec(&serde_json::json!({ "note": "nope" })).unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!(
                    "/api/ws/{}/access-requests/{}/deny",
                    ws.slug, pending2.id
                ))
                .header("cookie", admin_cookie)
                .header("content-type", "application/json")
                .body(Body::from(deny_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "deny must 200");
    assert_eq!(body_json(resp).await["ok"], serde_json::Value::Bool(true));
    let denied = st.access_requests.get(pending2.id).await.unwrap().unwrap();
    assert_eq!(denied.status, "denied", "request must be denied");
    assert_eq!(denied.decision_note.as_deref(), Some("nope"), "note stored");
}
