//! `/api/ws/{workspace}/apps…` from the caller's side: real database, REAL Cedar
//! PDP (built after every grant a test relies on), real `WsContext` (sessions,
//! membership, feature switch), and the production CSRF layer.
//!
//! Every test builds its own users and workspaces with fresh random ids, so tests
//! are independent within the process's shared database (`egret_test_db`).

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use egret_apps::grant::GrantRepo;
use egret_apps::session::AppMode;
use egret_apps::store::{AppStore, MAX_SOURCE_BYTES};
use egret_apps::{apps_router, AppsApiState};
use egret_audit::AuditEvent;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::{MemStore, ObjectStore};
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tower::ServiceExt;
use uuid::Uuid;

const TRASHED: &str = "app is in the trash; restore it first";
const APPS_ORIGIN: &str = "https://apps.egret.test";
const TEST_BUCKET: &str = "test-bucket";
const TEST_PREFIX: &str = "apps/";

/// S1-2: `AppStore` needs an object store too now. Every app this suite exercises
/// is made through the store's own `create` (via `Fixture::create_app`'s HTTP call)
/// or `publish`/`save_draft`, all against this one router's one `MemStore` -- so a
/// fresh, empty one here is enough; nothing needs pre-seeding.
fn test_object_store() -> Arc<dyn ObjectStore> {
    Arc::new(MemStore::new())
}

/// `(method, path template with {id}, body, extra headers)`.
type Route = (
    &'static str,
    String,
    Option<Value>,
    Vec<(&'static str, &'static str)>,
);

struct User {
    id: UserId,
    token: String,
}

struct Fixture {
    db: Db,
    /// The router's object store, kept across `rebuild` so content written
    /// before a grant is still there after it, and so a test can read what a
    /// route wrote (`Fixture::store`, `MemStore::keys`).
    mem: Arc<MemStore>,
    router: axum::Router,
    audit_rx: Receiver<AuditEvent>,
    owner: UserId,
    ws: WorkspaceId,
    slug: String,
    ws_b: WorkspaceId,
    viewer: User,
    editor: User,
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn build_router(
    db: &Db,
    objects: Arc<dyn ObjectStore>,
) -> (axum::Router, Receiver<AuditEvent>) {
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let (audit, audit_rx) = egret_audit::AuditSink::new(256);
    let st = AppsApiState {
        store: AppStore::new(
            db.clone(),
            objects,
            TEST_BUCKET.to_string(),
            TEST_PREFIX.to_string(),
        ),
        db: db.clone(),
        sessions: sessions(db),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp,
        audit,
        grants: GrantRepo::new(db.clone()),
        apps_origin: APPS_ORIGIN.to_string(),
        ticket_ttl: std::time::Duration::from_secs(30),
    };
    // Mounted as production mounts it: under `/api`, behind the CSRF layer.
    let router = axum::Router::new()
        .nest("/api", apps_router())
        .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
        .with_state(st);
    (router, audit_rx)
}

fn sessions(db: &Db) -> SessionService {
    SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    )
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url()
            .await
            .expect("test database (DATABASE_URL)");
        let db = Db::connect(&url, 5).await.expect("connect");
        let tag = Uuid::new_v4().simple().to_string();
        let owner = UserRepo::new(db.clone())
            .insert_local(&format!("apps-owner-{tag}@egret.test"), "Owner", "x")
            .await
            .expect("owner");
        let repo = WorkspaceRepo::new(db.clone());
        let slug = format!("appsa{tag}");
        let ws = repo.create(&slug, "Apps A", owner).await.expect("ws A").id;
        let ws_b = repo
            .create(&format!("appsb{tag}"), "Apps B", owner)
            .await
            .expect("ws B")
            .id;
        let mut fx = Self {
            mem: Arc::new(MemStore::new()),
            router: axum::Router::new(),
            audit_rx: tokio::sync::mpsc::channel(1).1,
            owner,
            ws,
            slug,
            ws_b,
            viewer: User {
                id: owner,
                token: String::new(),
            },
            editor: User {
                id: owner,
                token: String::new(),
            },
            db,
        };
        fx.viewer = fx.member(fx.ws, "ws_viewer").await;
        fx.editor = fx.member(fx.ws, "ws_editor").await;
        fx.rebuild().await;
        fx
    }

    /// Rebuild the router (and its PDP snapshot) after adding grants.
    async fn rebuild(&mut self) {
        let (router, rx) = build_router(&self.db, self.mem.clone()).await;
        self.router = router;
        self.audit_rx = rx;
    }

    /// An `AppStore` over the router's own objects.
    fn store(&self) -> AppStore {
        AppStore::new(
            self.db.clone(),
            self.mem.clone(),
            TEST_BUCKET.to_string(),
            TEST_PREFIX.to_string(),
        )
    }

    /// A user with a live session and no workspace role.
    async fn user(&self) -> User {
        let id = UserRepo::new(self.db.clone())
            .insert_local(
                &format!("apps-u-{}@egret.test", Uuid::new_v4().simple()),
                "U",
                "x",
            )
            .await
            .expect("user");
        let token = sessions(&self.db)
            .create(id, "test", "127.0.0.1")
            .await
            .expect("session")
            .token;
        User { id, token }
    }

    async fn member(&self, ws: WorkspaceId, role: &str) -> User {
        let u = self.user().await;
        MemberRepo::new(self.db.clone())
            .add(ws, "user", u.id.0, role, self.owner)
            .await
            .expect("member");
        u
    }

    fn path(&self, rest: &str) -> String {
        format!("/api/ws/{}/apps{rest}", self.slug)
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        who: &User,
        body: Option<Value>,
        headers: &[(&str, &str)],
        csrf: bool,
    ) -> Resp {
        let mut b = Request::builder().method(method).uri(uri);
        b = if csrf {
            b.header(
                "Cookie",
                format!("egret_session={}; egret_csrf=c1", who.token),
            )
            .header("x-egret-csrf", "c1")
        } else {
            b.header("Cookie", format!("egret_session={}", who.token))
        };
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = match body {
            Some(v) => b
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap())),
            None => b.body(Body::empty()),
        }
        .unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes) }));
        Resp {
            status,
            headers,
            body,
        }
    }

    async fn get(&self, uri: &str, who: &User) -> Resp {
        self.send("GET", uri, who, None, &[], false).await
    }

    async fn call(&self, method: &str, uri: &str, who: &User, body: Option<Value>) -> Resp {
        self.send(method, uri, who, body, &[], true).await
    }

    async fn put_draft(&self, id: &str, who: &User, if_match: Option<&str>, source: &str) -> Resp {
        let h: Vec<(&str, &str)> = if_match.map(|m| ("If-Match", m)).into_iter().collect();
        self.send(
            "PUT",
            &self.path(&format!("/{id}/draft")),
            who,
            Some(json!({ "source": source })),
            &h,
            true,
        )
        .await
    }

    /// Create an app as the editor; returns its id.
    async fn create_app(&self, name: &str) -> String {
        let r = self
            .call(
                "POST",
                &self.path(""),
                &self.editor,
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "create: {}", r.body);
        r.body["id"].as_str().unwrap().to_string()
    }

    async fn insert_database(&self, ws: WorkspaceId, name: &str) {
        sqlx::query(
            "INSERT INTO query.databases (workspace_id, name, engine, host, port) \
             VALUES ($1, $2, 'trino', 'localhost', 8080)",
        )
        .bind(ws.0)
        .bind(name)
        .execute(&self.db.pool)
        .await
        .expect("insert database");
    }

    async fn insert_folder(&self, ws: WorkspaceId, kind: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO analytics.folders (workspace_id, entity_type, name, created_by) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(ws.0)
        .bind(kind)
        .bind(format!("f-{}", Uuid::new_v4().simple()))
        .bind(self.owner.0)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert folder")
    }

    async fn app_count(&self, ws: WorkspaceId) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM analytics.apps WHERE workspace_id = $1")
            .bind(ws.0)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// Every `Mutation` action emitted so far (drains the channel).
    fn mutations(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = self.audit_rx.try_recv() {
            if let AuditEvent::Mutation { action, .. } = ev {
                out.push(action);
            }
        }
        out
    }
}

fn etag(r: &Resp) -> &str {
    r.headers
        .get("etag")
        .expect("ETag header")
        .to_str()
        .unwrap()
}

// 1
#[tokio::test]
async fn viewer_reads_but_cannot_write_or_see_the_draft() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let v = &fx.viewer;
    assert_eq!(fx.get(&fx.path(""), v).await.status, StatusCode::OK);
    let r = fx.get(&fx.path(&format!("/{id}")), v).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["is_owner"], json!(false));
    let r = fx
        .call("POST", &fx.path(""), v, Some(json!({ "name": "x" })))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "create: {}", r.body);
    let r = fx
        .call(
            "PATCH",
            &fx.path(&format!("/{id}")),
            v,
            Some(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "update: {}", r.body);
    let r = fx.get(&fx.path(&format!("/{id}/draft")), v).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "draft GET: {}", r.body);
}

// 2
#[tokio::test]
async fn create_with_a_workspace_database_prefills_the_connect_cell() {
    let fx = Fixture::new().await;
    fx.insert_database(fx.ws, "lake").await;
    let r = fx
        .call(
            "POST",
            &fx.path(""),
            &fx.editor,
            Some(json!({ "name": "Sales", "description": "d", "database": "lake" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["draft_revision"], json!(1));
    assert_eq!(r.body["is_owner"], json!(true));
    assert_eq!(r.body["latest_version_no"], Value::Null);
    let id = r.body["id"].as_str().unwrap();
    let d = fx.get(&fx.path(&format!("/{id}/draft")), &fx.editor).await;
    assert!(
        d.body["source"]
            .as_str()
            .unwrap()
            .contains(r#"egret.connect("lake")"#),
        "{}",
        d.body
    );
}

// 3
#[tokio::test]
async fn create_with_a_database_not_in_the_workspace_is_404_and_writes_nothing() {
    let fx = Fixture::new().await;
    // Exists, but in the other workspace: as absent as a name nobody has.
    fx.insert_database(fx.ws_b, "b_only").await;
    for db in ["nope", "b_only"] {
        let r = fx
            .call(
                "POST",
                &fx.path(""),
                &fx.editor,
                Some(json!({ "name": "x", "database": db })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{db}: {}", r.body);
        assert_eq!(r.body["error"], json!("not found: database"));
    }
    assert_eq!(fx.app_count(fx.ws).await, 0);
}

// 4
#[tokio::test]
async fn a_folder_of_another_kind_or_workspace_is_404() {
    let fx = Fixture::new().await;
    let other_kind = fx.insert_folder(fx.ws, "dashboard").await;
    let other_ws = fx.insert_folder(fx.ws_b, "app").await;
    let id = fx.create_app("a").await;
    for fid in [other_kind, other_ws] {
        let r = fx
            .call(
                "POST",
                &fx.path(""),
                &fx.editor,
                Some(json!({ "name": "x", "folder_id": fid })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "create: {}", r.body);
        assert_eq!(r.body["error"], json!("not found: folder"));
        let r = fx
            .call(
                "PATCH",
                &fx.path(&format!("/{id}/folder")),
                &fx.editor,
                Some(json!({ "folder_id": fid })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "move: {}", r.body);
        assert_eq!(r.body["error"], json!("not found: folder"));
    }
    assert_eq!(fx.app_count(fx.ws).await, 1, "only the first app exists");
    // The control: an app folder of this workspace is accepted, both ways.
    let good = fx.insert_folder(fx.ws, "app").await;
    let r = fx
        .call(
            "POST",
            &fx.path(""),
            &fx.editor,
            Some(json!({ "name": "y", "folder_id": good })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["folder_id"], json!(good));
    let r = fx
        .call(
            "PATCH",
            &fx.path(&format!("/{id}/folder")),
            &fx.editor,
            Some(json!({ "folder_id": good })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let r = fx.get(&fx.path(&format!("/{id}")), &fx.editor).await;
    assert_eq!(r.body["folder_id"], json!(good));
}

// 5 — and the controller's rule: EVERY `{id}` route resolves in the caller's workspace.
#[tokio::test]
async fn an_app_of_another_workspace_is_404_on_every_route_like_a_random_id() {
    let fx = Fixture::new().await;
    // An app (with a published version) in workspace B, which the editor of A
    // cannot see. Made through the store: nobody here is a member of B.
    // Its own, separate `MemStore`: every route this test drives against `b_app`
    // 404s on the workspace check before reaching draft/version content, so there
    // is nothing for this store to share with `fx.router`'s own.
    let store = AppStore::new(
        fx.db.clone(),
        test_object_store(),
        TEST_BUCKET.to_string(),
        TEST_PREFIX.to_string(),
    );
    let b_app = store
        .create(fx.ws_b, fx.owner, "b", "", None, "import marimo")
        .await
        .unwrap()
        .id;
    let b_ver = match store.publish(fx.ws_b, b_app, fx.owner, None).await.unwrap() {
        egret_apps::store::PublishOutcome::Published(v) => v.id,
        other => panic!("{other:?}"),
    };
    let random = Uuid::new_v4();
    let ok_get = fx.get(&fx.path(&format!("/{random}")), &fx.editor).await;
    assert_eq!(ok_get.status, StatusCode::NOT_FOUND);

    let routes: Vec<Route> = vec![
        ("GET", "/{id}".to_string(), None, vec![]),
        (
            "PATCH",
            "/{id}".to_string(),
            Some(json!({ "name": "x" })),
            vec![],
        ),
        ("POST", "/{id}/trash".to_string(), None, vec![]),
        ("POST", "/{id}/restore".to_string(), None, vec![]),
        (
            "PATCH",
            "/{id}/folder".to_string(),
            Some(json!({ "folder_id": null })),
            vec![],
        ),
        ("POST", "/{id}/bookmark".to_string(), None, vec![]),
        ("DELETE", "/{id}/bookmark".to_string(), None, vec![]),
        ("GET", "/{id}/draft".to_string(), None, vec![]),
        (
            "PUT",
            "/{id}/draft".to_string(),
            Some(json!({ "source": "x" })),
            vec![("If-Match", "\"1\"")],
        ),
        ("POST", "/{id}/publish".to_string(), Some(json!({})), vec![]),
        (
            "POST",
            "/{id}/ticket".to_string(),
            Some(json!({ "mode": "edit" })),
            vec![],
        ),
        ("POST", "/{id}/duplicate".to_string(), None, vec![]),
        ("GET", "/{id}/versions".to_string(), None, vec![]),
        ("GET", format!("/{{id}}/versions/{b_ver}"), None, vec![]),
        (
            "POST",
            format!("/{{id}}/versions/{b_ver}/restore"),
            None,
            vec![],
        ),
    ];
    for (method, tpl, body, headers) in &routes {
        let mut seen = Vec::new();
        for target in [b_app, random] {
            let uri = fx.path(&tpl.replace("{id}", &target.to_string()));
            let r = fx
                .send(method, &uri, &fx.editor, body.clone(), headers, true)
                .await;
            assert_eq!(
                r.status,
                StatusCode::NOT_FOUND,
                "{method} {uri}: {}",
                r.body
            );
            seen.push(r.body);
        }
        assert_eq!(seen[0], seen[1], "{method} {tpl}: same body as a random id");
        assert_eq!(seen[0]["error"], json!("not found: app"));
    }
    // Nothing in B moved.
    let row = store.get(fx.ws_b, fx.owner, b_app).await.unwrap().unwrap();
    assert_eq!(
        (row.name.as_str(), row.deleted_at, row.draft_revision),
        ("b", None, 1)
    );
    assert_eq!(store.versions(b_app).await.unwrap().len(), 1);
    assert!(!row.bookmarked);
}

// 6
#[tokio::test]
async fn draft_get_carries_the_revision_as_etag() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let r = fx.get(&fx.path(&format!("/{id}/draft")), &fx.editor).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(etag(&r), "\"1\"");
    assert_eq!(r.body["revision"], json!(1));
    assert!(r.body["source"].as_str().unwrap().contains("import marimo"));
}

// 7
#[tokio::test]
async fn draft_put_without_if_match_is_428_and_a_malformed_one_is_400() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let r = fx.put_draft(&id, &fx.editor, None, "x").await;
    assert_eq!(r.status, StatusCode::PRECONDITION_REQUIRED, "{}", r.body);
    for bad in ["*", "\"abc\"", "W/\"1\""] {
        let r = fx.put_draft(&id, &fx.editor, Some(bad), "x").await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    let d = fx.get(&fx.path(&format!("/{id}/draft")), &fx.editor).await;
    assert_eq!(d.body["revision"], json!(1), "nothing saved");
}

// 8
#[tokio::test]
async fn draft_put_is_compare_and_swap_on_the_revision() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let r = fx.put_draft(&id, &fx.editor, Some("\"1\""), "two").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body, json!({ "revision": 2 }));
    assert_eq!(etag(&r), "\"2\"");
    let r = fx.put_draft(&id, &fx.editor, Some("\"1\""), "stale").await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body["current_revision"], json!(2));
    // A bare integer is accepted too.
    let r = fx.put_draft(&id, &fx.editor, Some("2"), "three").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["revision"], json!(3));
    let d = fx.get(&fx.path(&format!("/{id}/draft")), &fx.editor).await;
    assert_eq!(d.body, json!({ "source": "three", "revision": 3 }));
}

// 9
#[tokio::test]
async fn draft_put_over_one_mib_is_413_and_saves_nothing() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let big = "a".repeat(MAX_SOURCE_BYTES + 1);
    assert_eq!(big.len(), 1_048_577);
    let r = fx.put_draft(&id, &fx.editor, Some("\"1\""), &big).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
    // The handler's 413, not the body limit's (which has no JSON body).
    assert_eq!(r.body["error"], json!("source exceeds 1 MiB"));
    let d = fx.get(&fx.path(&format!("/{id}/draft")), &fx.editor).await;
    assert_eq!(d.body["revision"], json!(1));
    // Exactly the limit fits through the body limit and the store.
    let r = fx
        .put_draft(&id, &fx.editor, Some("\"1\""), &big[..MAX_SOURCE_BYTES])
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    // A 1 MiB source of characters JSON must escape is 2 MiB of body plus the
    // envelope: it must still reach the handler and save.
    let quotes = "\"".repeat(MAX_SOURCE_BYTES);
    let r = fx.put_draft(&id, &fx.editor, Some("\"2\""), &quotes).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["revision"], json!(3));
    // A body over the router's explicit 3 MiB limit is refused before the handler.
    let r = fx
        .put_draft(&id, &fx.editor, Some("\"3\""), &"a".repeat(3 * 1024 * 1024))
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
    assert!(
        r.body.get("error").is_none(),
        "axum's limit, not the handler: {}",
        r.body
    );
}

// 10
#[tokio::test]
async fn publish_creates_a_version_and_honours_expected_revision() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let publish = fx.path(&format!("/{id}/publish"));
    let r = fx.call("POST", &publish, &fx.editor, Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["version_no"], json!(1));
    assert_eq!(r.body["from_revision"], json!(1));
    assert_eq!(r.body["published_by"], json!(fx.editor.id.0));
    fx.put_draft(&id, &fx.editor, Some("1"), "two").await;
    let r = fx
        .call(
            "POST",
            &publish,
            &fx.editor,
            Some(json!({ "expected_revision": 1 })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body["current_revision"], json!(2));
    let a = fx.get(&fx.path(&format!("/{id}")), &fx.editor).await;
    assert_eq!(
        a.body["latest_version_no"],
        json!(1),
        "the stale publish wrote nothing"
    );
}

/// A bare POST (no body, no content type) publishes the current draft.
#[tokio::test]
async fn publish_without_a_body_publishes_the_current_draft() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    fx.put_draft(&id, &fx.editor, Some("1"), "two").await;
    let r = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            &fx.editor,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["version_no"], json!(1));
    assert_eq!(r.body["from_revision"], json!(2));
    // What the generated client sends for `appsPublish(ws, id)` / `(ws, id, null)`:
    // a JSON content type with an empty body, or the literal `null`.
    let ct = [("Content-Type", "application/json")];
    let r = fx
        .send(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            &fx.editor,
            None,
            &ct,
            true,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["version_no"], json!(2));
    let r = fx
        .send(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            &fx.editor,
            Some(Value::Null),
            &[],
            true,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["version_no"], json!(3));
    // A body that is present but not an AppPublish is still refused.
    let r = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            &fx.editor,
            Some(json!({ "expected_revision": "two" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
}

// 11
#[tokio::test]
async fn versions_newest_first_with_source_scoped_to_the_app() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let other = fx.create_app("b").await;
    let pubr = |app: String| {
        let fx = &fx;
        async move {
            let r = fx
                .call(
                    "POST",
                    &fx.path(&format!("/{app}/publish")),
                    &fx.editor,
                    Some(json!({})),
                )
                .await;
            assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
            r.body
        }
    };
    let v1 = pubr(id.clone()).await;
    fx.put_draft(&id, &fx.editor, Some("1"), "second").await;
    let v2 = pubr(id.clone()).await;
    let ov = pubr(other.clone()).await;
    let r = fx
        .get(&fx.path(&format!("/{id}/versions")), &fx.editor)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let nos: Vec<i64> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version_no"].as_i64().unwrap())
        .collect();
    assert_eq!(nos, [2, 1]);
    let r = fx
        .get(
            &fx.path(&format!("/{id}/versions/{}", v2["id"].as_str().unwrap())),
            &fx.editor,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["source"], json!("second"));
    assert_eq!(r.body["version_no"], json!(2));
    let r = fx
        .get(
            &fx.path(&format!("/{id}/versions/{}", v1["id"].as_str().unwrap())),
            &fx.editor,
        )
        .await;
    assert!(r.body["source"].as_str().unwrap().contains("import marimo"));
    // A version of another app of the same workspace, addressed under this app.
    let r = fx
        .get(
            &fx.path(&format!("/{id}/versions/{}", ov["id"].as_str().unwrap())),
            &fx.editor,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);
    assert_eq!(r.body["error"], json!("not found: version"));
}

// 12
#[tokio::test]
async fn restoring_a_version_republishes_its_source() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let publish = fx.path(&format!("/{id}/publish"));
    let v1 = fx
        .call("POST", &publish, &fx.editor, Some(json!({})))
        .await
        .body;
    fx.put_draft(&id, &fx.editor, Some("1"), "second").await;
    fx.call("POST", &publish, &fx.editor, Some(json!({}))).await;
    let v1_id = v1["id"].as_str().unwrap();
    let r = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/versions/{v1_id}/restore")),
            &fx.editor,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["version_no"], json!(3));
    assert_eq!(r.body["restored_from"], json!(v1_id));
    assert_eq!(r.body["from_revision"], Value::Null);
    let v3 = fx
        .get(
            &fx.path(&format!(
                "/{id}/versions/{}",
                r.body["id"].as_str().unwrap()
            )),
            &fx.editor,
        )
        .await;
    let v1_src = fx
        .get(&fx.path(&format!("/{id}/versions/{v1_id}")), &fx.editor)
        .await;
    assert_eq!(v3.body["source"], v1_src.body["source"]);
    // The draft is untouched.
    let d = fx.get(&fx.path(&format!("/{id}/draft")), &fx.editor).await;
    assert_eq!(d.body, json!({ "source": "second", "revision": 2 }));
}

// 13
#[tokio::test]
async fn trash_hides_from_the_list_and_restore_brings_it_back() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let ids = |r: &Resp| -> Vec<String> {
        r.body
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_string())
            .collect()
    };
    let r = fx
        .call("POST", &fx.path(&format!("/{id}/trash")), &fx.editor, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    assert!(!ids(&fx.get(&fx.path(""), &fx.editor).await).contains(&id));
    assert!(ids(&fx.get(&fx.path("?trashed=true"), &fx.editor).await).contains(&id));
    let r = fx.get(&fx.path(&format!("/{id}")), &fx.editor).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body["deleted_at"].is_string(), "{}", r.body);
    let r = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/restore")),
            &fx.editor,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    assert!(ids(&fx.get(&fx.path(""), &fx.editor).await).contains(&id));
    assert!(!ids(&fx.get(&fx.path("?trashed=true"), &fx.editor).await).contains(&id));
    let r = fx.get(&fx.path(&format!("/{id}")), &fx.editor).await;
    assert_eq!(r.body["deleted_at"], Value::Null);
}

// 13b — controller decision 2: a trashed app is read-only until restored.
#[tokio::test]
async fn a_trashed_app_is_read_only_until_restored() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let e = &fx.editor;
    let v1 = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            e,
            Some(json!({})),
        )
        .await
        .body;
    let v1_id = v1["id"].as_str().unwrap().to_string();
    let folder = fx.insert_folder(fx.ws, "app").await;
    fx.call("POST", &fx.path(&format!("/{id}/trash")), e, None)
        .await;

    // Every change is refused with the same 409.
    let refused = [
        fx.call(
            "PATCH",
            &fx.path(&format!("/{id}")),
            e,
            Some(json!({ "name": "x" })),
        )
        .await,
        fx.call(
            "PATCH",
            &fx.path(&format!("/{id}/folder")),
            e,
            Some(json!({ "folder_id": folder })),
        )
        .await,
        fx.put_draft(&id, e, Some("1"), "x").await,
        fx.call(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            e,
            Some(json!({})),
        )
        .await,
        fx.call(
            "POST",
            &fx.path(&format!("/{id}/versions/{v1_id}/restore")),
            e,
            None,
        )
        .await,
        fx.call("POST", &fx.path(&format!("/{id}/duplicate")), e, None)
            .await,
    ];
    for (i, r) in refused.iter().enumerate() {
        assert_eq!(r.status, StatusCode::CONFLICT, "#{i}: {}", r.body);
        assert_eq!(r.body, json!({ "error": TRASHED }), "#{i}");
    }
    // ...and none of them happened.
    let a = fx.get(&fx.path(&format!("/{id}")), e).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(
        (a.body["name"].clone(), a.body["folder_id"].clone()),
        (json!("a"), Value::Null)
    );
    assert_eq!(
        (
            a.body["draft_revision"].clone(),
            a.body["latest_version_no"].clone()
        ),
        (json!(1), json!(1))
    );

    // Reads, bookmarks, and un-trashing still work.
    assert_eq!(
        fx.get(&fx.path("?trashed=true"), e).await.status,
        StatusCode::OK
    );
    assert_eq!(
        fx.get(&fx.path(&format!("/{id}/draft")), e).await.status,
        StatusCode::OK
    );
    assert_eq!(
        fx.get(&fx.path(&format!("/{id}/versions")), e).await.status,
        StatusCode::OK
    );
    assert_eq!(
        fx.get(&fx.path(&format!("/{id}/versions/{v1_id}")), e)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        fx.call("POST", &fx.path(&format!("/{id}/bookmark")), e, None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fx.call("DELETE", &fx.path(&format!("/{id}/bookmark")), e, None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let r = fx
        .call("POST", &fx.path(&format!("/{id}/restore")), e, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);

    // Restored: writable again.
    let r = fx
        .call(
            "PATCH",
            &fx.path(&format!("/{id}")),
            e,
            Some(json!({ "name": "renamed" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["name"], json!("renamed"));
    assert_eq!(
        fx.put_draft(&id, e, Some("1"), "x").await.status,
        StatusCode::OK
    );
}

// 14
#[tokio::test]
async fn a_bookmark_is_the_bookmarking_users_own() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let r = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/bookmark")),
            &fx.viewer,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let flag = |r: Resp| {
        r.body
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == json!(id))
            .unwrap()["bookmarked"]
            .clone()
    };
    assert_eq!(flag(fx.get(&fx.path(""), &fx.viewer).await), json!(true));
    assert_eq!(flag(fx.get(&fx.path(""), &fx.editor).await), json!(false));
    let r = fx
        .call(
            "DELETE",
            &fx.path(&format!("/{id}/bookmark")),
            &fx.viewer,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(flag(fx.get(&fx.path(""), &fx.viewer).await), json!(false));
}

// 15
#[tokio::test]
async fn an_object_share_reaches_require_object_but_not_the_workspace() {
    let mut fx = Fixture::new().await;
    let shared = fx.create_app("shared").await;
    let unshared = fx.create_app("unshared").await;
    let guest = fx.user().await;
    // The row the share API (`egret-bi/src/shares.rs`) writes.
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by) \
         SELECT 'user', $1, id, 'object', 'app', $2, 'allow', $3 \
           FROM roles WHERE scope = 'workspace' AND name = 'ws_viewer'",
    )
    .bind(guest.id.0)
    .bind(Uuid::parse_str(&shared).unwrap())
    .bind(fx.owner.0)
    .execute(&fx.db.pool)
    .await
    .expect("share");
    fx.rebuild().await;

    let r = fx.get(&fx.path(&format!("/{shared}")), &guest).await;
    assert_eq!(r.status, StatusCode::OK, "shared app: {}", r.body);
    let r = fx.get(&fx.path(""), &guest).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "list: {}", r.body);
    // The other app exists in a workspace the guest is admitted to (as an
    // object guest), so it resolves and the PDP refuses: 403, as for notebooks.
    let r = fx.get(&fx.path(&format!("/{unshared}")), &guest).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "unshared app: {}", r.body);
    // A viewer share grants read, not write.
    let r = fx.get(&fx.path(&format!("/{shared}/draft")), &guest).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "draft: {}", r.body);
}

// 16
#[tokio::test]
async fn a_mutating_request_without_the_csrf_header_is_403() {
    let fx = Fixture::new().await;
    let r = fx
        .send(
            "POST",
            &fx.path(""),
            &fx.editor,
            Some(json!({ "name": "x" })),
            &[],
            false,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    assert_eq!(fx.app_count(fx.ws).await, 0);
}

// 17
#[tokio::test]
async fn create_publish_and_restore_are_audited_once_and_draft_saves_never() {
    let mut fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    assert_eq!(fx.mutations(), ["app:create"]);
    let r = fx.put_draft(&id, &fx.editor, Some("1"), "two").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        fx.mutations(),
        Vec::<String>::new(),
        "a draft save is not audited"
    );
    let v1 = fx
        .call(
            "POST",
            &fx.path(&format!("/{id}/publish")),
            &fx.editor,
            Some(json!({})),
        )
        .await
        .body;
    assert_eq!(fx.mutations(), ["app:publish"]);
    let r = fx
        .call(
            "POST",
            &fx.path(&format!(
                "/{id}/versions/{}/restore",
                v1["id"].as_str().unwrap()
            )),
            &fx.editor,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(fx.mutations(), ["app:version_restore"]);
}

/// The detail of a publish names the version and revision, never the source.
#[tokio::test]
async fn publish_audit_detail_has_version_and_revision_but_no_source() {
    let mut fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    fx.mutations();
    fx.call(
        "POST",
        &fx.path(&format!("/{id}/publish")),
        &fx.editor,
        Some(json!({})),
    )
    .await;
    let ev = fx.audit_rx.try_recv().expect("publish event");
    let AuditEvent::Mutation {
        action,
        resource_type,
        resource_id,
        workspace_id,
        actor,
        detail,
        ..
    } = ev
    else {
        panic!("not a mutation")
    };
    assert_eq!(action, "app:publish");
    assert_eq!(resource_type, "app");
    assert_eq!(resource_id.as_deref(), Some(id.as_str()));
    assert_eq!(workspace_id, Some(fx.ws.0));
    assert_eq!(actor, Some(fx.editor.id.0));
    assert_eq!(detail, json!({ "version_no": 1, "from_revision": 1 }));
}

// 18
#[tokio::test]
async fn with_the_apps_feature_disabled_every_route_is_403() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let vid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, 'apps', false)",
    )
    .bind(fx.ws.0)
    .execute(&fx.db.pool)
    .await
    .expect("disable apps");
    let routes: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", String::new(), None),
        ("POST", String::new(), Some(json!({ "name": "x" }))),
        ("GET", format!("/{id}"), None),
        ("PATCH", format!("/{id}"), Some(json!({ "name": "x" }))),
        ("POST", format!("/{id}/trash"), None),
        ("POST", format!("/{id}/restore"), None),
        (
            "PATCH",
            format!("/{id}/folder"),
            Some(json!({ "folder_id": null })),
        ),
        ("POST", format!("/{id}/bookmark"), None),
        ("DELETE", format!("/{id}/bookmark"), None),
        ("GET", format!("/{id}/draft"), None),
        (
            "PUT",
            format!("/{id}/draft"),
            Some(json!({ "source": "x" })),
        ),
        ("POST", format!("/{id}/publish"), Some(json!({}))),
        ("POST", format!("/{id}/duplicate"), None),
        ("GET", format!("/{id}/versions"), None),
        ("GET", format!("/{id}/versions/{vid}"), None),
        ("POST", format!("/{id}/versions/{vid}/restore"), None),
    ];
    for (method, rest, body) in routes {
        let r = fx
            .send(
                method,
                &fx.path(&rest),
                &fx.editor,
                body,
                &[("If-Match", "1")],
                true,
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::FORBIDDEN,
            "{method} {rest}: {}",
            r.body
        );
        assert_eq!(r.body["code"], json!("feature_disabled"), "{method} {rest}");
    }
}

// ---------------------------------------------------------------- P4b: tickets

impl Fixture {
    async fn ticket(&self, id: &str, who: &User, body: Value) -> Resp {
        self.call(
            "POST",
            &self.path(&format!("/{id}/ticket")),
            who,
            Some(body),
        )
        .await
    }

    async fn publish_app(&self, id: &str) -> Uuid {
        let r = self
            .call(
                "POST",
                &self.path(&format!("/{id}/publish")),
                &self.editor,
                Some(json!({})),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
        r.body["id"].as_str().unwrap().parse().unwrap()
    }

    /// The `t` of a ticket URL, which must point at the apps origin.
    fn ticket_of(r: &Resp) -> String {
        let url = r.body["url"]
            .as_str()
            .unwrap_or_else(|| panic!("{}", r.body));
        let rest = url
            .strip_prefix(&format!("{APPS_ORIGIN}/_ticket?t="))
            .unwrap_or_else(|| panic!("ticket URL off the apps origin: {url}"));
        assert!(rest.starts_with("egt_"), "{url}");
        rest.to_string()
    }

    async fn login_session_of(&self, who: &User) -> Uuid {
        sessions(&self.db)
            .session_id_for_token(&who.token)
            .await
            .unwrap()
            .expect("live login session")
    }
}

const COOKIE_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

#[tokio::test]
async fn an_author_gets_an_edit_ticket_bound_to_their_login_session_and_the_draft() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let r = fx.ticket(&id, &fx.editor, json!({ "mode": "edit" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let ticket = Fixture::ticket_of(&r);

    let (grant, _cookie) = GrantRepo::new(fx.db.clone())
        .redeem(&ticket, COOKIE_TTL)
        .await
        .unwrap()
        .expect("redeems");
    assert_eq!(grant.mode, AppMode::Edit);
    assert_eq!(grant.user_id, fx.editor.id);
    assert_eq!(grant.app_id.to_string(), id);
    assert_eq!(grant.workspace_id, fx.ws);
    assert_eq!(grant.version_id, None, "edit runs the draft");
    assert_eq!(grant.impersonator, None);
    assert_eq!(grant.login_sid, fx.login_session_of(&fx.editor).await);
}

#[tokio::test]
async fn a_viewer_may_open_but_never_edit() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let r = fx.ticket(&id, &fx.viewer, json!({ "mode": "edit" })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);

    // nothing is published yet: there is nothing to run
    let r = fx.ticket(&id, &fx.viewer, json!({ "mode": "run" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);

    let version = fx.publish_app(&id).await;
    let r = fx.ticket(&id, &fx.viewer, json!({ "mode": "run" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let (grant, _) = GrantRepo::new(fx.db.clone())
        .redeem(&Fixture::ticket_of(&r), COOKIE_TTL)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.mode, AppMode::Run);
    assert_eq!(
        grant.version_id,
        Some(version),
        "run pins the latest version"
    );
    assert_eq!(grant.user_id, fx.viewer.id);
}

#[tokio::test]
async fn an_edit_ticket_needs_query_run_as_well_as_app_write() {
    // An author writes arbitrary SQL, so edit mode needs the same permission SQL Lab does.
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let role_name = format!("app-writer-{}", Uuid::new_v4().simple());
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 't') RETURNING id",
    )
    .bind(&role_name)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    for perm in ["app:write", "app:read"] {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(perm)
            .execute(&fx.db.pool)
            .await
            .unwrap();
    }
    let mut fx = fx;
    // A member whose only role holds `app:write` and `app:read` — and not `query:run`.
    let writer = fx.user().await;
    MemberRepo::new(fx.db.clone())
        .add(fx.ws, "user", writer.id.0, &role_name, fx.owner)
        .await
        .unwrap();
    fx.rebuild().await;
    let r = fx.ticket(&id, &writer, json!({ "mode": "edit" })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
}

#[tokio::test]
async fn a_run_ticket_for_a_version_of_another_app_is_not_found() {
    let fx = Fixture::new().await;
    let a = fx.create_app("a").await;
    let b = fx.create_app("b").await;
    let b_version = fx.publish_app(&b).await;
    fx.publish_app(&a).await;
    let r = fx
        .ticket(
            &a,
            &fx.viewer,
            json!({ "mode": "run", "version_id": b_version }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);
}

#[tokio::test]
async fn an_edit_ticket_that_names_a_version_is_invalid() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let v = fx.publish_app(&id).await;
    let r = fx
        .ticket(&id, &fx.editor, json!({ "mode": "edit", "version_id": v }))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
}

#[tokio::test]
async fn a_trashed_app_gets_no_ticket() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    fx.publish_app(&id).await;
    let r = fx
        .call("POST", &fx.path(&format!("/{id}/trash")), &fx.editor, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    for mode in ["edit", "run"] {
        let r = fx.ticket(&id, &fx.editor, json!({ "mode": mode })).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{mode}: {}", r.body);
    }
}

#[tokio::test]
async fn with_no_apps_origin_configured_the_iframe_cannot_be_opened() {
    let mut fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    // A router whose state has no origin: the same wiring, `apps.origin` left empty.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(fx.db.clone(), &egret_events::EventBus::new())
            .await
            .unwrap(),
    );
    let (audit, _rx) = egret_audit::AuditSink::new(8);
    let st = AppsApiState {
        // Its own `MemStore`: this test only checks the ticket route's response
        // to a missing `apps_origin`, never draft/version content.
        store: AppStore::new(
            fx.db.clone(),
            test_object_store(),
            TEST_BUCKET.to_string(),
            TEST_PREFIX.to_string(),
        ),
        db: fx.db.clone(),
        sessions: sessions(&fx.db),
        workspaces: WorkspaceRepo::new(fx.db.clone()),
        authz: Authorizer::new(fx.db.clone()),
        pdp,
        audit,
        grants: GrantRepo::new(fx.db.clone()),
        apps_origin: String::new(),
        ticket_ttl: std::time::Duration::from_secs(30),
    };
    fx.router = axum::Router::new()
        .nest("/api", apps_router())
        .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
        .with_state(st);
    let r = fx.ticket(&id, &fx.editor, json!({ "mode": "edit" })).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{}", r.body);
}

// ── Per-row capabilities (list actions spec §3.1) ───────────────────────────

impl Fixture {
    /// The row the share API (`egret-bi/src/shares.rs`) writes: `who` holds the
    /// workspace role `role` on this one app.
    async fn share(&self, app: &str, who: &User, role: &str) {
        let n = sqlx::query(
            "INSERT INTO access_grants \
               (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by) \
             SELECT 'user', $1, id, 'object', 'app', $2, 'allow', $3 \
               FROM roles WHERE scope = 'workspace' AND name = $4",
        )
        .bind(who.id.0)
        .bind(Uuid::parse_str(app).unwrap())
        .bind(self.owner.0)
        .bind(role)
        .execute(&self.db.pool)
        .await
        .expect("share")
        .rows_affected();
        assert_eq!(n, 1, "role {role} must exist");
    }

    /// `(can_write, can_manage)` of `app` as `who` sees it in the list.
    async fn listed_caps(&self, app: &str, who: &User) -> (Value, Value) {
        let r = self.get(&self.path(""), who).await;
        assert_eq!(r.status, StatusCode::OK, "list: {}", r.body);
        let row = r
            .body
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == app)
            .unwrap_or_else(|| panic!("app {app} not in the list: {}", r.body))
            .clone();
        (row["can_write"].clone(), row["can_manage"].clone())
    }

    /// `(can_write, can_manage)` of `app` as `who` sees it on the detail route.
    async fn detail_caps(&self, app: &str, who: &User) -> (Value, Value) {
        let r = self.get(&self.path(&format!("/{app}")), who).await;
        assert_eq!(r.status, StatusCode::OK, "get: {}", r.body);
        (r.body["can_write"].clone(), r.body["can_manage"].clone())
    }

    /// What `PATCH` (needs `app:write` on the app) answers `who`.
    async fn patch_status(&self, app: &str, who: &User) -> StatusCode {
        self.call(
            "PATCH",
            &self.path(&format!("/{app}")),
            who,
            Some(json!({ "description": "touched" })),
        )
        .await
        .status
    }
}

#[tokio::test]
async fn a_workspace_admin_can_write_and_manage_every_row() {
    let mut fx = Fixture::new().await;
    let id = fx.create_app("a").await;
    let admin = fx.member(fx.ws, "ws_admin").await;
    fx.rebuild().await;

    assert_eq!(
        fx.listed_caps(&id, &admin).await,
        (json!(true), json!(true))
    );
    assert_eq!(
        fx.detail_caps(&id, &admin).await,
        (json!(true), json!(true))
    );
    assert_eq!(fx.patch_status(&id, &admin).await, StatusCode::OK);
}

#[tokio::test]
async fn a_reader_can_neither_write_nor_manage() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;

    assert_eq!(
        fx.listed_caps(&id, &fx.viewer).await,
        (json!(false), json!(false))
    );
    assert_eq!(
        fx.detail_caps(&id, &fx.viewer).await,
        (json!(false), json!(false))
    );
    assert_eq!(
        fx.patch_status(&id, &fx.viewer).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn an_editor_can_write_but_not_manage() {
    let fx = Fixture::new().await;
    let id = fx.create_app("a").await;

    assert_eq!(
        fx.listed_caps(&id, &fx.editor).await,
        (json!(true), json!(false))
    );
    assert_eq!(fx.patch_status(&id, &fx.editor).await, StatusCode::OK);
}

#[tokio::test]
async fn a_write_share_on_one_app_shows_on_that_row_only() {
    let mut fx = Fixture::new().await;
    let shared = fx.create_app("shared").await;
    let unshared = fx.create_app("unshared").await;
    // A workspace viewer (so the list is theirs to read) with an Editor share on
    // ONE app: a workspace-wide answer would be wrong for one of the two rows.
    let collaborator = fx.member(fx.ws, "ws_viewer").await;
    fx.share(&shared, &collaborator, "ws_editor").await;
    // And a guest with no workspace role at all, holding only the share.
    let guest = fx.user().await;
    fx.share(&shared, &guest, "ws_editor").await;
    fx.rebuild().await;

    assert_eq!(
        fx.listed_caps(&shared, &collaborator).await,
        (json!(true), json!(false))
    );
    assert_eq!(
        fx.listed_caps(&unshared, &collaborator).await,
        (json!(false), json!(false))
    );
    assert_eq!(
        fx.detail_caps(&shared, &guest).await,
        (json!(true), json!(false))
    );

    // The flag and the refusal agree.
    assert_eq!(
        fx.patch_status(&shared, &collaborator).await,
        StatusCode::OK
    );
    assert_eq!(
        fx.patch_status(&unshared, &collaborator).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(fx.patch_status(&shared, &guest).await, StatusCode::OK);
}

#[tokio::test]
async fn create_and_update_report_the_callers_capabilities() {
    let fx = Fixture::new().await;
    let r = fx
        .call(
            "POST",
            &fx.path(""),
            &fx.editor,
            Some(json!({ "name": "a" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["can_write"], json!(true), "{}", r.body);
    assert_eq!(r.body["can_manage"], json!(false), "{}", r.body);
    let id = r.body["id"].as_str().unwrap().to_string();

    let r = fx
        .call(
            "PATCH",
            &fx.path(&format!("/{id}")),
            &fx.editor,
            Some(json!({ "name": "b" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["can_write"], json!(true), "{}", r.body);
    assert_eq!(r.body["can_manage"], json!(false), "{}", r.body);
}

// ── Duplicate (list actions spec §3.2) ──────────────────────────────────────

const DUP_SOURCE: &str = "import marimo\n# the source being copied \u{2713}\n";

impl Fixture {
    /// An app in `folder` whose draft is at revision 2 with `DUP_SOURCE` and a
    /// layout; returns its id and the layout as stored.
    async fn app_with_a_layout(&self, name: &str, folder: Option<Uuid>) -> (String, String) {
        let r = self
            .call(
                "POST",
                &self.path(""),
                &self.editor,
                Some(json!({ "name": name, "description": "what it is for", "folder_id": folder })),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "create: {}", r.body);
        let id = r.body["id"].as_str().unwrap().to_string();
        let layout = json!({ "type": "grid", "data": { "columns": 12, "cells": [{ "position": [0, 0, 6, 4] }] } });
        let r = self
            .send(
                "PUT",
                &self.path(&format!("/{id}/draft")),
                &self.editor,
                Some(json!({ "source": DUP_SOURCE, "layout": layout })),
                &[("If-Match", "1")],
                true,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "save: {}", r.body);
        (id, layout.to_string())
    }

    async fn duplicate(&self, id: &str, who: &User) -> Resp {
        self.call("POST", &self.path(&format!("/{id}/duplicate")), who, None)
            .await
    }
}

#[tokio::test]
async fn a_duplicate_is_a_new_app_with_the_sources_draft_and_nothing_else() {
    let mut fx = Fixture::new().await;
    let folder = fx.insert_folder(fx.ws, "app").await;
    let (src, layout) = fx.app_with_a_layout("Sales", Some(folder)).await;
    // Things a copy must NOT inherit: a version, a share, a bookmark.
    fx.publish_app(&src).await;
    let guest = fx.user().await;
    fx.share(&src, &guest, "ws_viewer").await;
    let admin = fx.member(fx.ws, "ws_admin").await;
    fx.rebuild().await;
    let r = fx
        .call("POST", &fx.path(&format!("/{src}/bookmark")), &admin, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    fx.mutations();

    let r = fx.duplicate(&src, &admin).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let copy = r.body["id"].as_str().unwrap().to_string();
    assert_ne!(copy, src);
    assert_eq!(r.body["name"], json!("Copy of Sales"));
    assert_eq!(r.body["description"], json!("what it is for"));
    assert_eq!(r.body["folder_id"], json!(folder));
    assert_eq!(r.body["owner_user_id"], json!(admin.id.0));
    assert_eq!(r.body["is_owner"], json!(true));
    assert_eq!(r.body["draft_revision"], json!(1));
    assert_eq!(r.body["latest_version_no"], Value::Null);
    assert_eq!(r.body["bookmarked"], json!(false));
    assert_eq!(r.body["deleted_at"], Value::Null);
    assert_eq!(r.body["can_write"], json!(true));

    // Byte for byte, read back through the store.
    let store = fx.store();
    let copy_id = Uuid::parse_str(&copy).unwrap();
    let src_id = Uuid::parse_str(&src).unwrap();
    let d = store.draft(fx.ws, copy_id).await.unwrap().expect("draft");
    let s = store.draft(fx.ws, src_id).await.unwrap().expect("draft");
    assert_eq!(d.source.as_bytes(), DUP_SOURCE.as_bytes());
    assert_eq!(d.source.as_bytes(), s.source.as_bytes());
    assert_eq!(d.layout_json.as_deref(), Some(layout.as_str()));
    assert_eq!(d.layout_json, s.layout_json);
    assert_eq!((d.revision, s.revision), (1, 2));

    assert!(store.versions(copy_id).await.unwrap().is_empty());
    let shares: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(copy_id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(shares, 0);
    let bookmarks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.bookmarks WHERE entity_type = 'app' AND entity_id = $1",
    )
    .bind(copy_id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(bookmarks, 0);

    // Audited once, naming where it came from.
    let mut detail = None;
    let mut n = 0;
    while let Ok(ev) = fx.audit_rx.try_recv() {
        if let AuditEvent::Mutation {
            action,
            resource_id,
            detail: d,
            ..
        } = ev
        {
            assert_eq!(action, "app:duplicate");
            assert_eq!(resource_id.as_deref(), Some(copy.as_str()));
            detail = Some(d);
            n += 1;
        }
    }
    assert_eq!(n, 1);
    assert_eq!(detail.unwrap(), json!({ "from": src }));
}

#[tokio::test]
async fn editing_a_duplicate_leaves_the_source_alone() {
    let fx = Fixture::new().await;
    let (src, layout) = fx.app_with_a_layout("a", None).await;
    let r = fx.duplicate(&src, &fx.editor).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let copy = r.body["id"].as_str().unwrap().to_string();

    // Source only, so the copy's layout is cleared too.
    let r = fx
        .put_draft(&copy, &fx.editor, Some("1"), "changed in the copy")
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);

    let store = fx.store();
    let s = store
        .draft(fx.ws, Uuid::parse_str(&src).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.source, DUP_SOURCE);
    assert_eq!(s.layout_json.as_deref(), Some(layout.as_str()));
    assert_eq!(s.revision, 2);
    let d = store
        .draft(fx.ws, Uuid::parse_str(&copy).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (d.source.as_str(), d.layout_json, d.revision),
        ("changed in the copy", None, 2)
    );
}

#[tokio::test]
async fn a_duplicate_of_an_app_with_no_layout_has_none() {
    let fx = Fixture::new().await;
    let src = fx.create_app("plain").await;
    let r = fx.duplicate(&src, &fx.editor).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let copy = Uuid::parse_str(r.body["id"].as_str().unwrap()).unwrap();
    let store = fx.store();
    let s = store
        .draft(fx.ws, Uuid::parse_str(&src).unwrap())
        .await
        .unwrap()
        .unwrap();
    let d = store.draft(fx.ws, copy).await.unwrap().unwrap();
    assert_eq!(d.source, s.source);
    assert_eq!(d.layout_json, None);
}

#[tokio::test]
async fn duplicating_needs_write_on_the_source_and_in_the_workspace() {
    let mut fx = Fixture::new().await;
    let (src, _) = fx.app_with_a_layout("a", None).await;
    // Write on the source through a share, but no workspace `app:write`: the
    // copy would be a new app in a workspace the caller may not create in.
    let guest = fx.user().await;
    fx.share(&src, &guest, "ws_editor").await;
    fx.rebuild().await;
    let before = fx.app_count(fx.ws).await;
    let keys = fx.mem.keys();
    fx.mutations();

    for (who, what) in [(&fx.viewer, "reader"), (&guest, "share-only writer")] {
        let r = fx.duplicate(&src, who).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{what}: {}", r.body);
    }
    assert_eq!(fx.app_count(fx.ws).await, before);
    assert_eq!(fx.mem.keys(), keys, "no object written");
    assert!(fx.mutations().is_empty());
}

#[tokio::test]
async fn a_trashed_app_cannot_be_duplicated() {
    let fx = Fixture::new().await;
    let src = fx.create_app("a").await;
    let r = fx
        .call("POST", &fx.path(&format!("/{src}/trash")), &fx.editor, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let before = fx.app_count(fx.ws).await;

    let r = fx.duplicate(&src, &fx.editor).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body, json!({ "error": TRASHED }));
    assert_eq!(fx.app_count(fx.ws).await, before);
}

#[tokio::test]
async fn a_long_name_is_truncated_to_the_name_limit() {
    let fx = Fixture::new().await;
    // 200 characters, multi-byte, so a byte-based cut would be wrong twice over.
    let name = "é".repeat(200);
    let src = fx.create_app(&name).await;
    let r = fx.duplicate(&src, &fx.editor).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let got = r.body["name"].as_str().unwrap();
    assert_eq!(got.chars().count(), 200);
    assert_eq!(got, format!("Copy of {}", "é".repeat(192)));
}
