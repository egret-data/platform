//! Tasks 8/9: the control-plane feature-entitlements API
//! (`GET`/`PUT /api/admin/workspaces/{id}/features`) plus the `features` field
//! `GET /api/me/workspaces` returns to tell the frontend which modules a
//! workspace has. DB-gated; self-skips without `DATABASE_URL`, mirroring the
//! sibling `workspace_db.rs`/`access_requests_routes.rs` harnesses (own
//! `connect`/`uniq`/`seed_user`/`ws_state`/`login_cookie` copies — each
//! integration-test binary in this crate is self-contained).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{admin_workspace_router, me_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
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

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    repo.insert_local(
        &format!("{}@test.local", uuid::Uuid::new_v4()),
        "U",
        &hash_password("pw").unwrap(),
    )
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

async fn ws_state_with_audit(db: &Db, audit: egret_audit::AuditSink) -> WorkspaceState {
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
        audit,
    }
}

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

/// Harness for the feature-entitlements HTTP surface: a built app combining
/// `admin_workspace_router` and `me_router`, a logged-in caller, and a
/// workspace to exercise. Kept alongside the audit-sink receiver so emitted
/// `Mutation` events aren't dropped before a test can drain them (mirrors
/// `egret-controlplane/tests/routes_api.rs`'s `Fixture::audit_rx`).
struct Fixture {
    app: axum::Router,
    cookie: String,
    ws_id: Uuid,
    audit_rx: tokio::sync::mpsc::Receiver<egret_audit::AuditEvent>,
}

impl Fixture {
    async fn build(actor_is_platform_admin: bool) -> Option<Self> {
        let db = connect().await?;
        let actor = seed_user(&db).await;
        let (audit, audit_rx) = egret_audit::AuditSink::new(16);
        let st = ws_state_with_audit(&db, audit).await;
        if actor_is_platform_admin {
            st.authz
                .replace_platform_roles(actor, &["platform_admin".into()])
                .await
                .unwrap();
        }
        let cookie = login_cookie(actor, &st.sessions).await;
        // Owned by a separate user so the plain-user 403 case isn't accidentally
        // passing because the actor happens to own the workspace.
        let creator = seed_user(&db).await;
        let ws = st
            .workspaces
            .create(&uniq("feat-ws"), "Feature WS", creator)
            .await
            .unwrap();
        let app = axum::Router::new()
            .nest("/admin", admin_workspace_router())
            .nest("/me", me_router())
            .with_state(st);
        Some(Fixture {
            app,
            cookie,
            ws_id: ws.id.0,
            audit_rx,
        })
    }

    /// A platform_admin actor, which is what holds `workspace:read`/`workspace:update`.
    async fn new() -> Option<Self> {
        Self::build(true).await
    }

    /// A plain authenticated user with no platform role at all.
    async fn new_as_plain_user() -> Option<Self> {
        Self::build(false).await
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> axum::response::Response {
        let b = match body {
            Some(s) => Body::from(s.to_string()),
            None => Body::empty(),
        };
        let mut builder = HttpRequest::builder()
            .method(method)
            .uri(path)
            .header("cookie", &self.cookie);
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        self.app
            .clone()
            .oneshot(builder.body(b).unwrap())
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> axum::response::Response {
        self.request("GET", path, None).await
    }

    async fn put(&self, path: &str, body: &str) -> axum::response::Response {
        self.request("PUT", path, Some(body)).await
    }

    async fn json(&self, resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[tokio::test]
async fn list_returns_every_feature_enabled_by_default() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let resp = f
        .get(&format!("/admin/workspaces/{}/features", f.ws_id))
        .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let json = f.json(resp).await;
    let arr = json.as_array().expect("array");
    assert_eq!(arr.len(), 5);
    assert!(arr.iter().all(|f| f["enabled"] == true));
    assert!(arr
        .iter()
        .all(|f| f["label"].is_string() && f["description"].is_string()));
}

#[tokio::test]
async fn put_disables_a_feature_and_list_reflects_it() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"notebooks":false}}"#,
        )
        .await;
    assert_eq!(put.status(), StatusCode::OK);

    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    let nb = json
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "notebooks")
        .unwrap();
    assert_eq!(nb["enabled"], false);
}

#[tokio::test]
async fn put_rejects_an_unknown_key_and_writes_nothing() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"notebooks":false,"teleporter":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::BAD_REQUEST);
    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    assert!(json
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["enabled"] == true));
}

#[tokio::test]
async fn a_user_without_workspace_update_cannot_toggle() {
    let Some(f) = Fixture::new_as_plain_user().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"notebooks":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::FORBIDDEN);
}

/// The write is audited: a `Mutation` event naming the actor lands in the
/// audit channel, carrying before/after detail. The audit sink's `emit` is a
/// synchronous `try_send`, so with the receiver held open (unlike the
/// `workspace_db.rs`/other fixtures here, which drop it) the event is
/// available immediately after the request returns — same technique as
/// `egret-controlplane/tests/routes_api.rs`'s `audit_rx.try_recv()`.
#[tokio::test]
async fn put_features_emits_an_audit_event_naming_the_actor() {
    let Some(mut f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"notebooks":false}}"#,
        )
        .await;
    assert_eq!(put.status(), StatusCode::OK);

    let ev = f
        .audit_rx
        .try_recv()
        .expect("a Mutation event must be queued after toggling a feature");
    match ev {
        egret_audit::AuditEvent::Mutation {
            actor,
            workspace_id,
            action,
            resource_type,
            resource_id,
            detail,
        } => {
            assert!(actor.is_some(), "actor must be recorded");
            assert_eq!(workspace_id, Some(f.ws_id), "workspace_id");
            assert_eq!(action, "workspace.features.update", "action");
            assert_eq!(resource_type, "workspace", "resource_type");
            assert_eq!(
                resource_id.as_deref(),
                Some(f.ws_id.to_string().as_str()),
                "resource_id is the workspace id"
            );
            let after: Vec<String> = detail["after"]
                .as_array()
                .expect("detail.after array")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert!(
                after.contains(&"notebooks".to_string()),
                "detail.after must record notebooks as disabled; detail={detail}"
            );
        }
        other => panic!("expected a Mutation event, got {other:?}"),
    }
}
