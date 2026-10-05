//! The apps-origin proxy end to end: a REAL listener (so hyper, not a test shortcut, parses
//! the request target — S0's bypass lived in exactly that gap), raw TCP clients (an HTTP client
//! library would normalise `..` before the proxy ever saw it), a fake marimo pod that records
//! what reaches it, the real database and the real Cedar PDP.
//!
//! Every property here is one the spec's §10 names; each was shown to fail when the code it
//! guards was broken on purpose.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use egret_apps::grant::{GrantRepo, NewGrant};
use egret_apps::proxy::{PodEndpoint, PodResolver, ProxyConfig, ProxyState};
use egret_apps::session::{AppMode, AppSessionRepo};
use egret_core::{Error, UserId, WorkspaceId};
use egret_metastore::Db;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

const MARIMO_TOKEN: &str = "marimo-secret";
const EGRET_ORIGIN: &str = "https://egret.test";
const COOKIE: &str = "egret_apps_grant";
const IDLE: Duration = Duration::from_secs(1800);

// ------------------------------------------------------------------ fake pod

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    /// The request target exactly as the pod received it.
    target: String,
    headers: HashMap<String, Vec<String>>,
}

#[derive(Clone, Default)]
struct PodLog(Arc<Mutex<Vec<Seen>>>);

impl PodLog {
    fn all(&self) -> Vec<Seen> {
        self.0.lock().unwrap().clone()
    }
    fn last(&self) -> Seen {
        self.all().pop().expect("the pod saw a request")
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

fn header_map(h: &axum::http::HeaderMap) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in h {
        out.entry(k.as_str().to_string())
            .or_default()
            .push(v.to_str().unwrap_or("").to_string());
    }
    out
}

async fn pod_handler(
    State(log): State<PodLog>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
    req: Request,
) -> Response {
    let ws = ws.ok();
    log.0.lock().unwrap().push(Seen {
        method: req.method().to_string(),
        target: req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default(),
        headers: header_map(req.headers()),
    });
    if let Some(ws) = ws {
        return ws.on_upgrade(|mut socket| async move {
            while let Some(Ok(msg)) = socket.recv().await {
                match msg {
                    Message::Text(_) | Message::Binary(_) => {
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });
    }
    (
        [
            ("set-cookie", "pod=1"),
            ("access-control-allow-origin", "*"),
            ("x-frame-options", "SAMEORIGIN"),
            ("content-type", "text/plain"),
        ],
        "from the pod",
    )
        .into_response()
}

async fn spawn_pod() -> (url::Url, PodLog) {
    let log = PodLog::default();
    let app = axum::Router::new()
        .fallback(pod_handler)
        .with_state(log.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url::Url::parse(&format!("http://{addr}")).unwrap(), log)
}

struct FakePods {
    base: url::Url,
}

/// A resolver whose pod is set after the fixture exists (the real-marimo test starts the pod
/// only once it knows the pod reference and has a pod token).
#[derive(Default)]
struct SwitchPods(Mutex<Option<PodEndpoint>>);

#[async_trait]
impl PodResolver for SwitchPods {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String {
        egret_apps::edit_pod_ref(user, ws)
    }
    async fn edit_pod(&self, _: UserId, _: WorkspaceId) -> egret_core::Result<PodEndpoint> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::NotFound("pod".into()))
    }
    fn run_ref(&self, user: UserId, version: Uuid) -> String {
        egret_apps::run_pod_ref(user, version)
    }
    async fn run_pod(&self, _: UserId, _: Uuid, _: Uuid) -> egret_core::Result<PodEndpoint> {
        Err(Error::NotFound("pod".into()))
    }
}

#[async_trait]
impl PodResolver for FakePods {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String {
        egret_apps::edit_pod_ref(user, ws)
    }
    async fn edit_pod(&self, _user: UserId, _ws: WorkspaceId) -> egret_core::Result<PodEndpoint> {
        Ok(PodEndpoint {
            base: self.base.clone(),
            marimo_token: MARIMO_TOKEN.into(),
        })
    }
    fn run_ref(&self, user: UserId, version: Uuid) -> String {
        // A REAL per-(viewer, version) reference, same function production uses (D24): this is
        // what lets `two_viewers_of_the_same_version_get_two_different_run_pod_references`
        // actually distinguish them, even though every run pod in this fake is the same upstream.
        egret_apps::run_pod_ref(user, version)
    }
    async fn run_pod(
        &self,
        _user: UserId,
        _app: Uuid,
        _version: Uuid,
    ) -> egret_core::Result<PodEndpoint> {
        Ok(PodEndpoint {
            base: self.base.clone(),
            marimo_token: MARIMO_TOKEN.into(),
        })
    }
}

// ------------------------------------------------------------------- fixture

struct Person {
    id: UserId,
    login_sid: Uuid,
}

struct Fixture {
    db: Db,
    addr: SocketAddr,
    pod: PodLog,
    ws: WorkspaceId,
    app: Uuid,
    version: Uuid,
    editor: Person,
    viewer: Person,
    grants: GrantRepo,
    sessions: AppSessionRepo,
}

async fn insert_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!(
        "proxy_{tag}_{}@egret.local",
        Uuid::new_v4().simple()
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn login_session(db: &Db, user: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
         VALUES ($1, $2, now() + interval '8 hours', now() + interval '30 minutes') RETURNING id",
    )
    .bind(format!("px{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn grant_role(db: &Db, user: Uuid, role: &str, ws: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role_id)
    .bind(ws)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// No test needs the cap itself; this is just large enough that nothing in the suite trips it.
const NO_PRACTICAL_CAP: u32 = 1000;

impl Fixture {
    async fn new() -> Self {
        let (pod_url, pod) = spawn_pod().await;
        Self::with_resolver(Arc::new(FakePods { base: pod_url }), pod, NO_PRACTICAL_CAP).await
    }

    /// A fixture whose run surface enforces `cap` concurrent WebSockets per (viewer, version).
    async fn with_run_cap(cap: u32) -> Self {
        let (pod_url, pod) = spawn_pod().await;
        Self::with_resolver(Arc::new(FakePods { base: pod_url }), pod, cap).await
    }

    async fn with_resolver(pods: Arc<dyn PodResolver>, pod: PodLog, run_session_cap: u32) -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 10).await.expect("connect");
        let owner = insert_user(&db, "owner").await;
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Proxy', $2, 'active') RETURNING id",
        )
        .bind(format!("proxy-{}", Uuid::new_v4().simple()))
        .bind(owner)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let editor_id = insert_user(&db, "editor").await;
        grant_role(&db, editor_id, "ws_editor", ws).await;
        let viewer_id = insert_user(&db, "viewer").await;
        grant_role(&db, viewer_id, "ws_viewer", ws).await;
        let app: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'proxied', $2) RETURNING id",
        )
        .bind(ws)
        .bind(editor_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let version: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no, source) \
             VALUES ($1, 1, 'import marimo') RETURNING id",
        )
        .bind(app)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let editor = Person {
            id: UserId(editor_id),
            login_sid: login_session(&db, editor_id).await,
        };
        let viewer = Person {
            id: UserId(viewer_id),
            login_sid: login_session(&db, viewer_id).await,
        };

        let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
            egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
                .await
                .unwrap(),
        );
        let grants = GrantRepo::new(db.clone());
        let sessions = AppSessionRepo::new(db.clone());
        let state = ProxyState::new(
            db.clone(),
            pdp,
            grants.clone(),
            sessions.clone(),
            pods,
            ProxyConfig {
                egret_origin: EGRET_ORIGIN.into(),
                secure: false,
                session_idle: IDLE,
                session_absolute: Duration::from_secs(8 * 3600),
                cookie_ttl: Duration::from_secs(3600),
                run_session_cap,
            },
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = egret_apps::proxy::proxy_router(state);
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        Self {
            db,
            addr,
            pod,
            ws: WorkspaceId(ws),
            app,
            version,
            editor,
            viewer,
            grants,
            sessions,
        }
    }

    fn edit_ref(&self, who: &Person) -> String {
        egret_apps::edit_pod_ref(who.id, self.ws)
    }

    /// A second (or third…) viewer in this fixture's workspace, with `app:read` only.
    async fn new_person(&self, tag: &str, role: &str) -> Person {
        let id = insert_user(&self.db, tag).await;
        grant_role(&self.db, id, role, self.ws.0).await;
        Person {
            id: UserId(id),
            login_sid: login_session(&self.db, id).await,
        }
    }

    fn new_grant(&self, who: &Person, mode: AppMode) -> NewGrant {
        NewGrant {
            user_id: who.id,
            workspace_id: self.ws,
            app_id: self.app,
            version_id: (mode == AppMode::Run).then_some(self.version),
            mode,
            login_sid: who.login_sid,
            impersonator: None,
        }
    }

    /// A ticket, redeemed straight through the repo: `(cookie value, grant id)`.
    async fn cookie_for(&self, who: &Person, mode: AppMode) -> (String, Uuid) {
        let ticket = self
            .grants
            .issue_ticket(&self.new_grant(who, mode), Duration::from_secs(30))
            .await
            .unwrap();
        let (grant, cookie) = self
            .grants
            .redeem(&ticket, Duration::from_secs(3600))
            .await
            .unwrap()
            .unwrap();
        (cookie, grant.id)
    }

    async fn ticket_for(&self, who: &Person, mode: AppMode) -> String {
        self.grants
            .issue_ticket(&self.new_grant(who, mode), Duration::from_secs(30))
            .await
            .unwrap()
    }

    async fn get(&self, target: &str, cookie: Option<&str>) -> Raw {
        let c = cookie.map(|c| format!("{COOKIE}={c}"));
        let mut h: Vec<(&str, &str)> = vec![];
        if let Some(c) = &c {
            h.push(("Cookie", c));
        }
        raw(self.addr, "GET", target, &h, b"").await
    }
}

// ------------------------------------------------------------- raw HTTP client

#[derive(Debug)]
struct Raw {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Raw {
    fn header(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

async fn raw(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Raw {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!(
        "{method} {target} HTTP/1.1\r\nHost: apps.test\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).to_string();
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status line in {text:?}"));
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Raw { status, headers }
}

fn token_of(seen: &Seen) -> String {
    let v = seen
        .headers
        .get("x-egret-app-token")
        .unwrap_or_else(|| panic!("no x-egret-app-token in {seen:?}"));
    assert_eq!(v.len(), 1, "exactly one token header: {v:?}");
    v[0].clone()
}

// ----------------------------------------------------------------- the tests

#[tokio::test]
async fn a_ticket_becomes_a_path_scoped_cookie_and_a_redirect_to_the_draft_and_replays_fail() {
    let fx = Fixture::new().await;
    let ticket = fx.ticket_for(&fx.editor, AppMode::Edit).await;
    let r = fx.get(&format!("/_ticket?t={ticket}"), None).await;
    assert_eq!(r.status, 302, "{r:?}");
    let eref = fx.edit_ref(&fx.editor);
    assert_eq!(
        r.header("location"),
        [format!("/e/{eref}/?file={}.py", fx.app)]
    );
    let cookie = r.header("set-cookie");
    assert_eq!(cookie.len(), 1);
    assert!(
        cookie[0].starts_with(&format!("{COOKIE}=egc_")),
        "{cookie:?}"
    );
    assert!(cookie[0].contains("HttpOnly"), "{cookie:?}");
    assert!(cookie[0].contains("SameSite=Lax"), "{cookie:?}");
    assert!(
        cookie[0].contains(&format!("Path=/e/{eref}/")),
        "scoped to its own pod so an edit tab and a run tab cannot overwrite each other: {cookie:?}"
    );
    assert!(r
        .header("cache-control")
        .iter()
        .any(|v| v.contains("no-store")));
    assert_eq!(r.header("referrer-policy"), ["no-referrer"]);

    let replay = fx.get(&format!("/_ticket?t={ticket}"), None).await;
    assert_eq!(replay.status, 403, "a spent ticket");
    assert!(replay.header("set-cookie").is_empty());
    assert_eq!(
        fx.get("/_ticket", None).await.status,
        400,
        "no ticket at all"
    );
    assert_eq!(fx.get("/_ticket?t=garbage", None).await.status, 403);
    assert!(fx.pod.all().is_empty(), "the pod saw nothing of this");
}

#[tokio::test]
async fn no_cookie_a_garbage_cookie_or_a_cookie_of_the_wrong_surface_is_401() {
    let fx = Fixture::new().await;
    let eref = fx.edit_ref(&fx.editor);
    let target = format!("/e/{eref}/api/kernel/save");
    assert_eq!(
        fx.get(&format!("/e/{eref}/assets/a.js"), None).await.status,
        401
    );
    assert_eq!(
        fx.get(&format!("/e/{eref}/assets/a.js"), Some("egc_nope"))
            .await
            .status,
        401
    );
    // A run grant's cookie on the edit surface: refused, as measured in S0.
    let (run_cookie, _) = fx.cookie_for(&fx.viewer, AppMode::Run).await;
    assert_eq!(
        fx.get(&format!("/e/{eref}/assets/a.js"), Some(&run_cookie))
            .await
            .status,
        401
    );
    // …and an edit grant's on the run surface.
    let (edit_cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    assert_eq!(
        fx.get(
            &format!("/r/{}/assets/a.js", fx.version),
            Some(&edit_cookie)
        )
        .await
        .status,
        401
    );
    let _ = target;
    assert!(fx.pod.all().is_empty());
}

#[tokio::test]
async fn the_pod_gets_exactly_our_token_and_never_the_clients_header_cookie_or_credentials() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let r = raw(
        fx.addr,
        "POST",
        &format!("/e/{eref}/api/kernel/run"),
        &[
            ("Cookie", &format!("{COOKIE}={cookie}; other=1")),
            ("X-Egret-App-Token", "ega_forged"),
            ("x-egret-user", "admin"),
            ("Authorization", "Bearer client-supplied"),
            ("Marimo-Session-Id", "s_abc"),
            ("Marimo-Server-Token", "skew"),
            ("Content-Type", "application/json"),
        ],
        b"{}",
    )
    .await;
    assert_eq!(r.status, 200, "{r:?}");
    let seen = fx.pod.last();
    let token = token_of(&seen);
    assert!(
        token.starts_with("ega_") && token != "ega_forged",
        "{token}"
    );
    assert!(!seen.headers.contains_key("cookie"), "{seen:?}");
    assert!(!seen.headers.contains_key("x-egret-user"));
    assert_eq!(
        seen.headers["authorization"],
        [format!("Bearer {MARIMO_TOKEN}")]
    );
    assert_eq!(seen.headers["marimo-session-id"], ["s_abc"]);
    assert_eq!(seen.headers["marimo-server-token"], ["skew"]);

    // the token is real: it resolves, as the editor, for this app, in edit mode
    let sess = fx
        .sessions
        .resolve(&token, IDLE)
        .await
        .unwrap()
        .expect("live");
    assert_eq!(sess.user_id, fx.editor.id);
    assert_eq!(sess.app_id, fx.app);
    assert_eq!(sess.mode, AppMode::Edit);
}

#[tokio::test]
async fn a_request_naming_an_app_the_user_cannot_write_never_reaches_the_pod() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    // an app in another workspace, and one that exists nowhere
    let other = Fixture::new().await;
    for file in [
        format!("{}.py", other.app),
        format!("{}.py", Uuid::new_v4()),
    ] {
        let r = fx
            .get(&format!("/e/{eref}/?file={file}"), Some(&cookie))
            .await;
        assert!(matches!(r.status, 403 | 404), "{file}: {r:?}");
    }
    // the viewer holds only `app:read` on the app: a viewer's edit grant cannot open its draft
    let (vcookie, _) = fx.cookie_for(&fx.viewer, AppMode::Edit).await;
    let vref = fx.edit_ref(&fx.viewer);
    let r = fx
        .get(&format!("/e/{vref}/?file={}.py", fx.app), Some(&vcookie))
        .await;
    assert_eq!(r.status, 403, "{r:?}");
    // malformed names and a repeated one are 400, not forwarded
    for q in [
        "file=../x.py",
        "file=notes.py",
        &format!("file={}.py%0A", fx.app),
        &format!("file={0}.py&file={0}.py", fx.app),
    ] {
        let r = fx.get(&format!("/e/{eref}/?{q}"), Some(&cookie)).await;
        assert_eq!(r.status, 400, "{q}: {r:?}");
    }
    assert!(fx.pod.all().is_empty(), "{:?}", fx.pod.all());

    // and the author's own draft is fine
    let r = fx
        .get(&format!("/e/{eref}/?file={}.py", fx.app), Some(&cookie))
        .await;
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(fx.pod.all().len(), 1);
}

#[tokio::test]
async fn the_edit_page_without_a_file_goes_to_the_grants_own_draft_not_marimos_file_picker() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let r = fx.get(&format!("/e/{eref}/"), Some(&cookie)).await;
    assert_eq!(r.status, 302, "{r:?}");
    assert_eq!(
        r.header("location"),
        [format!("/e/{eref}/?file={}.py", fx.app)]
    );
    assert!(fx.pod.all().is_empty());
}

#[tokio::test]
async fn the_apps_listener_serves_nothing_but_tickets_and_pods() {
    // D17: egret's API is not reachable through the apps host, whatever the cookie says.
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    for target in [
        "/",
        "/healthz",
        "/metrics",
        "/api/auth/me",
        "/api/apps/query",
        "/api/apps/pod/apps/x/draft",
        "/api/admin/workspaces",
        "/e",
        "/e/",
        "/x/y/z",
    ] {
        let r = fx.get(target, Some(&cookie)).await;
        assert_eq!(r.status, 404, "{target}");
    }
    assert!(fx.pod.all().is_empty());
}

#[tokio::test]
async fn another_users_pod_reference_is_refused() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let theirs = fx.edit_ref(&fx.viewer);
    let r = fx
        .get(&format!("/e/{theirs}/assets/a.js"), Some(&cookie))
        .await;
    assert_eq!(r.status, 403, "{r:?}");
    let r = fx
        .get(&format!("/r/{}/assets/a.js", Uuid::new_v4()), Some(&cookie))
        .await;
    assert_eq!(r.status, 401, "an edit grant on the run surface");
    assert!(fx.pod.all().is_empty());
}

/// D24: the fix to the gap D24 named in the already-merged P4c proxy — a run grant's "entitled"
/// pod used to be computed from the version alone, so every viewer of one version was entitled
/// to the same `{ref}`. After the fix, two different viewers of the SAME version are entitled to
/// two different run pods, and neither's cookie opens the other's.
#[tokio::test]
async fn two_viewers_of_the_same_version_get_two_different_run_pod_references() {
    let fx = Fixture::new().await;
    let other = fx.new_person("second_viewer", "ws_viewer").await;

    let ticket1 = fx.ticket_for(&fx.viewer, AppMode::Run).await;
    let r1 = fx.get(&format!("/_ticket?t={ticket1}"), None).await;
    let ticket2 = fx.ticket_for(&other, AppMode::Run).await;
    let r2 = fx.get(&format!("/_ticket?t={ticket2}"), None).await;
    let loc1 = r1.header("location")[0].to_string();
    let loc2 = r2.header("location")[0].to_string();
    assert_ne!(
        loc1, loc2,
        "two different viewers were redirected to the same run pod reference"
    );

    let (c1, _) = fx.cookie_for(&fx.viewer, AppMode::Run).await;
    let (c2, _) = fx.cookie_for(&other, AppMode::Run).await;
    let ref1 = loc1.split('/').nth(2).unwrap().to_string();
    assert_eq!(
        fx.get(&format!("/r/{ref1}/api/version"), Some(&c1))
            .await
            .status,
        200,
        "the viewer's own cookie opens their own pod"
    );
    assert_eq!(
        fx.get(&format!("/r/{ref1}/api/version"), Some(&c2))
            .await
            .status,
        403,
        "the OTHER viewer's cookie must not open it — same version, different pod"
    );
}

#[tokio::test]
async fn blocked_routes_are_403_on_both_surfaces_and_the_pod_never_hears_of_them() {
    let fx = Fixture::new().await;
    let (ecookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let (rcookie, _) = fx.cookie_for(&fx.viewer, AppMode::Run).await;
    let eref = fx.edit_ref(&fx.editor);
    for route in [
        "/api/packages/list",
        "/api/files/list_files",
        "/api/secrets/keys",
        "/api/ai/chat",
        "/api/kernel/copy",
        "/api/kernel/rename",
        "/api/home/workspace_files",
        "/api/brand/new/route",
        "/auth/login",
        "/og/thumbnail",
    ] {
        let r = fx.get(&format!("/e/{eref}{route}"), Some(&ecookie)).await;
        assert_eq!(r.status, 403, "edit {route}");
        let r = fx
            .get(&format!("/r/{}{route}", fx.version), Some(&rcookie))
            .await;
        assert_eq!(r.status, 403, "run {route}");
    }
    // an edit-only route is not reachable by a run grant
    let r = raw(
        fx.addr,
        "POST",
        &format!("/r/{}/api/kernel/save", fx.version),
        &[("Cookie", &format!("{COOKIE}={rcookie}"))],
        b"{}",
    )
    .await;
    assert_eq!(r.status, 403);
    assert!(fx.pod.all().is_empty(), "{:?}", fx.pod.all());
}

#[tokio::test]
async fn every_ambiguous_spelling_of_a_blocked_route_is_stopped_before_the_pod() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let mut tried = 0;
    for base in [
        "api/packages/list",
        "api/files/list_files",
        "api/secrets/keys",
    ] {
        for target in [
            format!("/e/{eref}/../{base}"),
            format!("/e/{eref}/%2e%2e/{base}"),
            format!("/e/{eref}/%2E%2E/{base}"),
            format!("/e/{eref}/..%2f{base}"),
            format!("/e/{eref}/./{base}"),
            format!("/e/{eref}//{base}"),
            format!("/e/{eref}%5c{base}"),
            format!("/e/{eref}/%252e%252e/{base}"),
        ] {
            let r = fx.get(&target, Some(&cookie)).await;
            assert!(matches!(r.status, 400 | 403), "{target} -> {}", r.status);
            tried += 1;
        }
    }
    // POST forms too: S0 saw two of these reach marimo
    for base in ["api/files/list_files", "api/packages/add"] {
        let r = raw(
            fx.addr,
            "POST",
            &format!("/e/{eref}/x/../{base}"),
            &[("Cookie", &format!("{COOKIE}={cookie}"))],
            b"{}",
        )
        .await;
        assert!(matches!(r.status, 400 | 403), "{base} -> {}", r.status);
        tried += 1;
    }
    assert_eq!(tried, 26);
    assert!(fx.pod.all().is_empty(), "{:?}", fx.pod.all());
}

#[tokio::test]
async fn the_path_the_pod_receives_is_the_path_the_client_sent() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    for target in [
        format!("/e/{eref}/@file/12-a%20b.png?x=1&y=%20z"),
        format!("/e/{eref}/assets/index-AbC.js"),
    ] {
        fx.pod.clear();
        let r = fx.get(&target, Some(&cookie)).await;
        assert_eq!(r.status, 200, "{target}: {r:?}");
        assert_eq!(fx.pod.last().target, target, "forwarded unchanged");
        assert_eq!(fx.pod.last().method, "GET");
    }
}

#[tokio::test]
async fn the_pods_cookies_and_cors_headers_are_dropped_and_only_egret_may_frame() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let r = fx
        .get(&format!("/e/{eref}/assets/a.js"), Some(&cookie))
        .await;
    assert_eq!(r.status, 200);
    assert!(r.header("set-cookie").is_empty(), "{r:?}");
    assert!(r.header("access-control-allow-origin").is_empty());
    assert!(r.header("x-frame-options").is_empty());
    assert_eq!(
        r.header("content-security-policy"),
        [format!("frame-ancestors {EGRET_ORIGIN}")]
    );
    assert_eq!(r.header("content-type"), ["text/plain"]);
}

#[tokio::test]
async fn a_static_file_costs_no_token_and_carries_none() {
    let fx = Fixture::new().await;
    let (cookie, grant) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let r = fx
        .get(&format!("/e/{eref}/assets/index.js"), Some(&cookie))
        .await;
    assert_eq!(r.status, 200);
    assert!(!fx.pod.last().headers.contains_key("x-egret-app-token"));
    let minted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM analytics.app_sessions WHERE grant_id = $1")
            .bind(grant)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(minted, 0);
}

#[tokio::test]
async fn logging_out_kills_the_next_request_and_the_token() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let r = raw(
        fx.addr,
        "POST",
        &format!("/e/{eref}/api/kernel/run"),
        &[("Cookie", &format!("{COOKIE}={cookie}"))],
        b"{}",
    )
    .await;
    assert_eq!(r.status, 200);
    let token = token_of(&fx.pod.last());
    assert!(fx.sessions.resolve(&token, IDLE).await.unwrap().is_some());

    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(fx.editor.login_sid)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    fx.pod.clear();
    let r = raw(
        fx.addr,
        "POST",
        &format!("/e/{eref}/api/kernel/run"),
        &[("Cookie", &format!("{COOKIE}={cookie}"))],
        b"{}",
    )
    .await;
    assert_eq!(r.status, 401);
    assert!(fx.pod.all().is_empty());
    assert!(fx.sessions.resolve(&token, IDLE).await.unwrap().is_none());
}

// ---- WebSocket

async fn ws_connect(
    fx: &Fixture,
    path: &str,
    cookie: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    let mut req = format!("ws://{}{path}", fx.addr)
        .into_client_request()
        .unwrap();
    if let Some(c) = cookie {
        req.headers_mut()
            .insert("cookie", format!("{COOKIE}={c}").parse().unwrap());
    }
    tokio_tungstenite::connect_async(req).await.map(|(s, _)| s)
}

#[tokio::test]
async fn a_blocked_websocket_never_upgrades() {
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    for path in [
        format!("/e/{eref}/terminal/ws"),
        format!("/e/{eref}/x/../terminal/ws"),
        format!("/e/{eref}/%2e%2e/terminal/ws"),
        format!("/e/{eref}/TERMINAL/ws"),
    ] {
        match ws_connect(&fx, &path, Some(&cookie)).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert!(matches!(resp.status().as_u16(), 400 | 403), "{path}");
            }
            other => panic!("{path}: expected a refused handshake, got {other:?}"),
        }
    }
    assert!(ws_connect(&fx, &format!("/e/{eref}/ws"), None)
        .await
        .is_err());
    assert!(fx.pod.all().is_empty(), "{:?}", fx.pod.all());
}

#[tokio::test]
async fn a_websocket_relays_and_its_close_revokes_the_token_but_a_later_request_remints() {
    let fx = Fixture::new().await;
    let (cookie, grant) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);

    let mut ws = ws_connect(
        &fx,
        &format!("/e/{eref}/ws?file={}.py&session_id=s1", fx.app),
        Some(&cookie),
    )
    .await
    .expect("upgrade");
    ws.send(tokio_tungstenite::tungstenite::Message::Text("ping".into()))
        .await
        .unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(echoed.into_text().unwrap().as_str(), "ping");

    let seen = fx.pod.last();
    assert!(
        seen.target.starts_with(&format!("/e/{eref}/ws?file=")),
        "{seen:?}"
    );
    let token = token_of(&seen);
    assert_eq!(
        seen.headers["authorization"],
        [format!("Bearer {MARIMO_TOKEN}")]
    );
    assert!(!seen.headers.contains_key("cookie"));
    assert!(!seen.headers.contains_key("origin"));
    assert!(fx.sessions.resolve(&token, IDLE).await.unwrap().is_some());

    // an HTTP request of the same grant carries the SAME token
    let r = raw(
        fx.addr,
        "POST",
        &format!("/e/{eref}/api/kernel/run"),
        &[("Cookie", &format!("{COOKIE}={cookie}"))],
        b"{}",
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(token_of(&fx.pod.last()), token);

    // closing the only WebSocket revokes the grant's tokens
    ws.close(None).await.unwrap();
    drop(ws);
    let mut revoked = false;
    for _ in 0..50 {
        if fx.sessions.resolve(&token, IDLE).await.unwrap().is_none() {
            revoked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(revoked, "the token survived the WebSocket closing");

    // …but the grant is still good, so the next request mints a fresh token (a reload works)
    let r = raw(
        fx.addr,
        "POST",
        &format!("/e/{eref}/api/kernel/run"),
        &[("Cookie", &format!("{COOKIE}={cookie}"))],
        b"{}",
    )
    .await;
    assert_eq!(r.status, 200);
    let fresh = token_of(&fx.pod.last());
    assert_ne!(fresh, token);
    let sess = fx
        .sessions
        .resolve(&fresh, IDLE)
        .await
        .unwrap()
        .expect("live");
    assert_eq!(sess.user_id, fx.editor.id);
    let _ = grant;
}

#[tokio::test]
async fn closing_one_of_two_websockets_leaves_the_token_alive_for_the_other() {
    // The reload case: the new page's WebSocket opens before the old one's close arrives.
    let fx = Fixture::new().await;
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let eref = fx.edit_ref(&fx.editor);
    let path = format!("/e/{eref}/ws?file={}.py", fx.app);
    let mut old = ws_connect(&fx, &path, Some(&cookie)).await.unwrap();
    let token = token_of(&fx.pod.last());
    let mut new = ws_connect(&fx, &path, Some(&cookie)).await.unwrap();
    assert_eq!(token_of(&fx.pod.last()), token, "one token per grant");

    old.close(None).await.unwrap();
    drop(old);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        fx.sessions.resolve(&token, IDLE).await.unwrap().is_some(),
        "the other WebSocket is still open"
    );
    new.close(None).await.unwrap();
    drop(new);
    let mut revoked = false;
    for _ in 0..50 {
        if fx.sessions.resolve(&token, IDLE).await.unwrap().is_none() {
            revoked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(revoked, "the last close must revoke");
}

/// D24's `run_session_cap`: a run pod's own viewer is capped at N concurrent sessions, keyed by
/// (viewer, version) rather than by grant — a page reload mints a fresh grant for the same person
/// and the same pod, and the cap must still hold across that.
#[tokio::test]
async fn a_run_pods_own_viewer_is_capped_at_n_concurrent_sessions() {
    let fx = Fixture::with_run_cap(2).await;
    let rref = egret_apps::run_pod_ref(fx.viewer.id, fx.version);
    let path = format!("/r/{rref}/ws");

    let (c1, _) = fx.cookie_for(&fx.viewer, AppMode::Run).await;
    let mut a = ws_connect(&fx, &path, Some(&c1))
        .await
        .expect("1st session");
    // a second grant for the SAME viewer and version (e.g. the app reopened in another tab)
    let (c2, _) = fx.cookie_for(&fx.viewer, AppMode::Run).await;
    let b = ws_connect(&fx, &path, Some(&c2))
        .await
        .expect("2nd session, a different grant, the same pod");

    match ws_connect(&fx, &path, Some(&c2)).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status().as_u16(),
                503,
                "the pod, not the grant, is at capacity"
            );
        }
        other => panic!("expected a 503 refusal, got {other:?}"),
    }
    assert_eq!(
        fx.pod.all().len(),
        2,
        "the refused 3rd session must never reach the pod"
    );

    // closing one frees a slot for the next
    a.close(None).await.unwrap();
    drop(a);
    let mut opened = false;
    for _ in 0..50 {
        if ws_connect(&fx, &path, Some(&c2)).await.is_ok() {
            opened = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(opened, "closing a session must free a slot for the next");
    let _ = b;
}

// ---- the real thing

/// marimo 0.25.0 itself, started by P3's launcher against the REAL pod API, behind the REAL
/// proxy: does the allow-list leave the editor working? Every other test here uses a fake pod,
/// which can only prove the proxy does what the proxy was written to do.
///
/// Not run by default (it needs the apps image's Python environment):
///
/// ```text
/// EGRET_APPS_BIN=$PWD/apps-image/.venv/bin/egret-apps DATABASE_URL=… \
///   cargo test -p egret-apps --test proxy -- --ignored real_marimo
/// ```
#[tokio::test]
#[ignore = "needs marimo: set EGRET_APPS_BIN to apps-image/.venv/bin/egret-apps"]
async fn real_marimo_behind_the_real_proxy_and_the_real_pod_api() {
    let bin = std::env::var("EGRET_APPS_BIN").expect("EGRET_APPS_BIN");
    let switch = Arc::new(SwitchPods::default());
    let fx = Fixture::with_resolver(switch.clone(), PodLog::default(), NO_PRACTICAL_CAP).await;

    // the draft the pod will fetch through the real pod API
    let draft_source =
        "import marimo\n\napp = marimo.App()\n\n\n@app.cell\ndef _():\n    x = 1\n    return\n\n\n\
         if __name__ == \"__main__\":\n    app.run()\n";
    sqlx::query(
        "INSERT INTO analytics.app_drafts (app_id, source, revision, updated_by) VALUES ($1, $2, 1, $3)",
    )
    .bind(fx.app)
    .bind(draft_source)
    .bind(fx.editor.id.0)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    // `draft()` reads the source from the object store first, falling back to
    // the row above only on a miss -- seed the matching key so the real pod
    // (an external process) gets the same content from the object store.
    let bucket = "test-bucket".to_string();
    let prefix = "apps/".to_string();
    let store: Arc<dyn egret_storage::ObjectStore> = Arc::new(egret_storage::MemStore::new());
    store
        .put(
            &format!(
                "s3://{bucket}/{prefix}{}/{}/draft.py",
                fx.ws.0.hyphenated(),
                fx.app.hyphenated()
            ),
            draft_source.as_bytes().to_vec(),
        )
        .await
        .unwrap();

    // the real pod API on its own port, and a pod token for the editor
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(fx.db.clone(), &egret_events::EventBus::new())
            .await
            .unwrap(),
    );
    let pods = egret_apps::podtoken::PodTokenRepo::new(fx.db.clone());
    let api = axum::Router::new()
        .nest("/api", egret_apps::pod_router())
        .with_state(egret_apps::PodApiState {
            db: fx.db.clone(),
            pdp,
            pods: pods.clone(),
            store: egret_apps::store::AppStore::new(fx.db.clone(), store, bucket, prefix),
        });
    let api_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_addr = api_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(api_listener, api).await.unwrap() });
    let pod_token = pods.mint_edit(fx.editor.id, fx.ws).await.unwrap();

    // start marimo
    let eref = fx.edit_ref(&fx.editor);
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let tmp = std::env::temp_dir().join(format!("egret-real-marimo-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(tmp.join("home")).unwrap();
    let config = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps-image/config");
    let log = std::fs::File::create(tmp.join("pod.log")).unwrap();
    let mut child = std::process::Command::new(&bin);
    child
        .arg("edit")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", tmp.join("home"))
        .env("EGRET_URL", format!("http://{api_addr}"))
        .env("EGRET_POD_TOKEN", &pod_token)
        .env("EGRET_MARIMO_TOKEN", MARIMO_TOKEN)
        .env("EGRET_BASE_URL", format!("/e/{eref}"))
        .env("APPS_WORKDIR", tmp.join("work"))
        .env("EGRET_APPS_CONFIG", config.canonicalize().unwrap())
        .env("EGRET_MARIMO_PORT", port.to_string())
        // the launcher honours its test-only overrides only under pytest
        .env("PYTEST_CURRENT_TEST", "egret-apps proxy test")
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    std::os::unix::process::CommandExt::process_group(&mut child, 0);
    let mut child = child.spawn().expect("start egret-apps");
    let pgid = child.id();
    struct Kill(u32);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &format!("-{}", self.0)])
                .status();
        }
    }
    let _kill = Kill(pgid);

    let http = reqwest::Client::new();
    let health = format!("http://127.0.0.1:{port}/e/{eref}/health");
    let mut up = false;
    for _ in 0..240 {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "egret-apps exited {status}:\n{}",
                std::fs::read_to_string(tmp.join("pod.log")).unwrap_or_default()
            );
        }
        if let Ok(r) = http.get(&health).bearer_auth(MARIMO_TOKEN).send().await {
            if r.status().is_success() {
                up = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        up,
        "marimo never came up:\n{}",
        std::fs::read_to_string(tmp.join("pod.log")).unwrap_or_default()
    );
    *switch.0.lock().unwrap() = Some(PodEndpoint {
        base: url::Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
        marimo_token: MARIMO_TOKEN.into(),
    });

    // 1. the editor page, through the proxy, for the author's own draft
    let (cookie, _) = fx.cookie_for(&fx.editor, AppMode::Edit).await;
    let page = http
        .get(format!("http://{}/e/{eref}/?file={}.py", fx.addr, fx.app))
        .header("cookie", format!("{COOKIE}={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(
        html.to_lowercase().contains("marimo"),
        "{}",
        &html[..html.len().min(300)]
    );

    // 2. a static asset the page names
    let asset = html
        .split('"')
        .find(|s| s.contains("/assets/") && s.ends_with(".js"))
        .unwrap_or_else(|| panic!("no script in the page: {}", &html[..html.len().min(600)]))
        .to_string();
    // the page names assets relative to its own base URL (`./assets/…`)
    let page_url = url::Url::parse(&format!("http://{}/e/{eref}/", fx.addr)).unwrap();
    let asset_url = page_url.join(&asset).unwrap().to_string();
    let r = http
        .get(&asset_url)
        .header("cookie", format!("{COOKIE}={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{asset_url}");

    // 3. what the allow-list blocks, against the real thing
    for route in [
        "/api/packages/list",
        "/api/files/list_files",
        "/api/secrets/keys",
    ] {
        let r = http
            .get(format!("http://{}/e/{eref}{route}", fx.addr))
            .header("cookie", format!("{COOKIE}={cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "{route}");
    }

    // 4. the editor's WebSocket opens and marimo speaks on it
    let mut ws = ws_connect(
        &fx,
        &format!("/e/{eref}/ws?file={}.py&session_id=s_proxytest", fx.app),
        Some(&cookie),
    )
    .await
    .expect("marimo WebSocket through the proxy");
    let first = tokio::time::timeout(Duration::from_secs(60), ws.next())
        .await
        .expect("marimo sent nothing within 60 s")
        .expect("socket closed")
        .expect("socket error");
    assert!(first.is_text() || first.is_binary(), "{first:?}");

    // marimo's skew protection: the page carries a token the browser echoes on every POST
    let server_token = html
        .split("<marimo-server-token")
        .nth(1)
        .and_then(|t| t.split("data-token=\"").nth(1))
        .and_then(|t| t.split('"').next())
        .unwrap_or_else(|| {
            panic!(
                "no server token in the page: {}",
                &html[..html.len().min(800)]
            )
        })
        .to_string();

    // 5. the kernel really started for the author's draft: instantiate it through the proxy,
    // with the session header marimo uses to find the session
    let r = http
        .post(format!(
            "http://{}/e/{eref}/api/kernel/instantiate",
            fx.addr
        ))
        .header("cookie", format!("{COOKIE}={cookie}"))
        .header("marimo-session-id", "s_proxytest")
        .header("marimo-server-token", server_token)
        .header("content-type", "application/json")
        .body(r#"{"objectIds":[],"values":[],"auto_run":true}"#)
        .send()
        .await
        .unwrap();
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    assert!(status.is_success(), "instantiate: {status} {body}");

    // the pod's own log must not show either token
    let log = std::fs::read_to_string(tmp.join("pod.log")).unwrap_or_default();
    assert!(
        !log.contains(MARIMO_TOKEN) && !log.contains(&pod_token),
        "{log}"
    );
    ws.close(None).await.ok();
    let _ = std::fs::remove_dir_all(&tmp);
}
