//! The pod API (`/api/apps/pod/…`, spec §5.3) from an app pod's side: real database,
//! REAL Cedar PDP, a pod token per request. Grants are seeded workspace roles, the way
//! a real member holds them.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_apps::podtoken::PodTokenRepo;
use egret_apps::store::AppStore;
use egret_apps::PodApiState;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    db: Db,
    ws: Uuid,
    app: Uuid,
    version: Uuid,
    editor: Uuid,
    viewer: Uuid,
    pods: PodTokenRepo,
    router: axum::Router,
}

async fn insert_user(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("apps_pod_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn insert_workspace(db: &Db, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Apps Pod', $2, 'active') RETURNING id",
    )
    .bind(format!("apps-pod-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace")
}

async fn grant_role(db: &Db, user: Uuid, role: &str, ws: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("seeded role");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role_id)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant role");
}

async fn build_router(db: &Db) -> axum::Router {
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = PodApiState {
        db: db.clone(),
        pdp,
        pods: PodTokenRepo::new(db.clone()),
        store: AppStore::new(db.clone()),
    };
    axum::Router::new()
        .nest("/api", egret_apps::pod_router())
        .with_state(st)
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 5).await.expect("connect");
        let owner = insert_user(&db).await;
        let ws = insert_workspace(&db, owner).await;
        let editor = insert_user(&db).await;
        grant_role(&db, editor, "ws_editor", ws).await;
        let viewer = insert_user(&db).await;
        grant_role(&db, viewer, "ws_viewer", ws).await;
        let app: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'pod app', $2) RETURNING id",
        )
        .bind(ws)
        .bind(editor)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO analytics.app_drafts (app_id, source, revision, updated_by) VALUES ($1, 'draft v1', 1, $2)")
            .bind(app)
            .bind(editor)
            .execute(&db.pool)
            .await
            .unwrap();
        let version: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no, source) \
             VALUES ($1, 1, 'published v1') RETURNING id",
        )
        .bind(app)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let router = build_router(&db).await;
        let pods = PodTokenRepo::new(db.clone());
        Self {
            db,
            ws,
            app,
            version,
            editor,
            viewer,
            pods,
            router,
        }
    }

    async fn edit_token(&self, user: Uuid) -> String {
        self.pods
            .mint_edit(UserId(user), WorkspaceId(self.ws))
            .await
            .unwrap()
    }

    async fn run_token(&self, app: Uuid, version: Uuid) -> String {
        self.pods
            .mint_run(WorkspaceId(self.ws), app, version)
            .await
            .unwrap()
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        if_match: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        if let Some(m) = if_match {
            req = req.header("if-match", m);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn draft_uri(&self) -> String {
        format!("/api/apps/pod/apps/{}/draft", self.app)
    }

    fn version_uri(&self, version: Uuid) -> String {
        format!("/api/apps/pod/apps/{}/versions/{}", self.app, version)
    }
}

#[tokio::test]
async fn an_edit_token_reads_the_draft_of_an_app_its_user_can_write() {
    let fx = Fixture::new().await;
    let t = fx.edit_token(fx.editor).await;
    let (status, body) = fx.call("GET", &fx.draft_uri(), Some(&t), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"source": "draft v1", "revision": 1}));
}

#[tokio::test]
async fn the_app_write_check_is_repeated_on_every_call() {
    let fx = Fixture::new().await;
    // A viewer holds `app:read` only. Their pod token must not read the draft.
    let t = fx.edit_token(fx.viewer).await;
    let (status, _) = fx.call("GET", &fx.draft_uri(), Some(&t), None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The editor can, until the grant goes: the next call after revocation is refused.
    let t = fx.edit_token(fx.editor).await;
    assert_eq!(
        fx.call("GET", &fx.draft_uri(), Some(&t), None, None)
            .await
            .0,
        StatusCode::OK
    );
    sqlx::query("DELETE FROM access_grants WHERE principal_id = $1")
        .bind(fx.editor)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let router = build_router(&fx.db).await; // a fresh PDP snapshot, as after a grant change
    let resp = router
        .oneshot(
            Request::builder()
                .uri(fx.draft_uri())
                .header("authorization", format!("Bearer {t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_app_of_another_workspace_is_not_found() {
    let fx = Fixture::new().await;
    let other = Fixture::new().await;
    // The token is bound to `fx.ws`; the app lives in `other.ws`.
    let t = fx.edit_token(fx.editor).await;
    let uri = format!("/api/apps/pod/apps/{}/draft", other.app);
    let (status, _) = fx.call("GET", &uri, Some(&t), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_token_of_the_wrong_kind_is_forbidden_on_the_other_route() {
    let fx = Fixture::new().await;
    let run = fx.run_token(fx.app, fx.version).await;
    let (status, _) = fx
        .call("GET", &fx.draft_uri(), Some(&run), None, None)
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run token on the draft route"
    );

    let edit = fx.edit_token(fx.editor).await;
    let (status, _) = fx
        .call("GET", &fx.version_uri(fx.version), Some(&edit), None, None)
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "edit token on the version route"
    );
}

#[tokio::test]
async fn a_run_token_reads_only_its_own_version() {
    let fx = Fixture::new().await;
    let second: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.app_versions (app_id, version_no, source) \
         VALUES ($1, 2, 'published v2') RETURNING id",
    )
    .bind(fx.app)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let t = fx.run_token(fx.app, fx.version).await;
    let (status, body) = fx
        .call("GET", &fx.version_uri(fx.version), Some(&t), None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"source": "published v1", "version_no": 1}));
    let (status, _) = fx
        .call("GET", &fx.version_uri(second), Some(&t), None, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "bound to version 1, not 2");
}

#[tokio::test]
async fn saving_follows_the_cookie_api_contract() {
    let fx = Fixture::new().await;
    let t = fx.edit_token(fx.editor).await;
    let put = |if_match: Option<&'static str>, source: String| {
        let uri = fx.draft_uri();
        let t = t.clone();
        let fx = &fx;
        async move {
            fx.call(
                "PUT",
                &uri,
                Some(&t),
                if_match,
                Some(json!({ "source": source })),
            )
            .await
        }
    };
    assert_eq!(
        put(None, "x".into()).await.0,
        StatusCode::PRECONDITION_REQUIRED
    );

    let (status, body) = put(Some("\"1\""), "draft v2".into()).await;
    assert_eq!((status, body), (StatusCode::OK, json!({"revision": 2})));

    let (status, body) = put(Some("\"1\""), "stale".into()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["current_revision"], json!(2));

    let (status, _) = put(Some("\"2\""), "x".repeat(1024 * 1024 + 1)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    let stored: String =
        sqlx::query_scalar("SELECT source FROM analytics.app_drafts WHERE app_id = $1")
            .bind(fx.app)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored, "draft v2",
        "a stale or oversized save changes nothing"
    );
}

#[tokio::test]
async fn a_missing_garbage_or_revoked_bearer_is_unauthorized() {
    let fx = Fixture::new().await;
    let uri = fx.draft_uri();
    assert_eq!(
        fx.call("GET", &uri, None, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fx.call("GET", &uri, Some("garbage"), None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // an app-session token is not a pod token
    assert_eq!(
        fx.call("GET", &uri, Some("ega_abc"), None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let t = fx.edit_token(fx.editor).await;
    let id = fx.pods.resolve(&t).await.unwrap().unwrap().id;
    fx.pods.revoke_for_pod(id).await.unwrap();
    assert_eq!(
        fx.call("GET", &uri, Some(&t), None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_trashed_app_is_read_only() {
    let fx = Fixture::new().await;
    sqlx::query("UPDATE analytics.apps SET deleted_at = now() WHERE id = $1")
        .bind(fx.app)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let t = fx.edit_token(fx.editor).await;
    let (status, _) = fx
        .call(
            "PUT",
            &fx.draft_uri(),
            Some(&t),
            Some("\"1\""),
            Some(json!({"source": "x"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = fx.call("GET", &fx.draft_uri(), Some(&t), None, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reading a trashed draft stays possible, as in the cookie API"
    );
}
