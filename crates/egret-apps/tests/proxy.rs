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

#[async_trait]
impl PodResolver for FakePods {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String {
        format!(
            "pod{}{}",
            &user.0.simple().to_string()[..6],
            &ws.0.simple().to_string()[..6]
        )
    }
    async fn edit_pod(&self, _user: UserId, _ws: WorkspaceId) -> egret_core::Result<PodEndpoint> {
        Ok(PodEndpoint {
            base: self.base.clone(),
            marimo_token: MARIMO_TOKEN.into(),
        })
    }
    async fn run_pod(&self, _app: Uuid, _version: Uuid) -> egret_core::Result<PodEndpoint> {
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
    pods: Arc<FakePods>,
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

impl Fixture {
    async fn new() -> Self {
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

        let (pod_url, pod) = spawn_pod().await;
        let pods = Arc::new(FakePods { base: pod_url });
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
            pods.clone(),
            ProxyConfig {
                egret_origin: EGRET_ORIGIN.into(),
                secure: false,
                session_idle: IDLE,
                session_absolute: Duration::from_secs(8 * 3600),
                cookie_ttl: Duration::from_secs(3600),
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
            pods,
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
        self.pods.edit_ref(who.id, self.ws)
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
    assert!(seen.headers.get("cookie").is_none(), "{seen:?}");
    assert!(seen.headers.get("x-egret-user").is_none());
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
    assert!(fx.pod.last().headers.get("x-egret-app-token").is_none());
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
    assert!(seen.headers.get("cookie").is_none());
    assert!(seen.headers.get("origin").is_none());
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

#[allow(dead_code)]
fn _unused(_: Error) {}
