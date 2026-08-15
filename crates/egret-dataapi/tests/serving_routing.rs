//! P4 Task 3 — turning a URL into a published snapshot.
//!
//! Every test here is about one of two rules:
//!
//! * **Routing follows the endpoint row; content follows the snapshot.** If
//!   serving read the draft, an author saving an edit would change behaviour
//!   under every deployed client and P3's versioning would be decoration.
//! * **404, never 403.** Unknown, disabled, another workspace's, an archived
//!   workspace's — one answer, so a caller cannot map another tenant's URLs.
//!
//! The endpoints here are PUBLISHED through the real management route, not by
//! writing `endpoint_versions` by hand: a snapshot this file built itself could
//! diverge from the one publish actually freezes, and then the draft-vs-snapshot
//! test would be checking a fixture against itself.
//!
//! **What this file cannot check, and where that check lives.** These tests call
//! `serving_router()` directly, so they never mount the CSRF layer and CANNOT
//! see whether the serving group inherited it — a test that could pass either
//! way is worthless for exactly that property. That is why
//! `crates/egret-server/tests/dataapi_serving_mount.rs` exists: it goes through
//! the COMPOSED `build_router`. `render_router` shipped a POST that was rejected
//! in production and green in CI for precisely this reason
//! (`egret-automation/src/routes.rs:115-126`).
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, like the sibling
//! suites — so check the DURATION, not just the `ok`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::{ColumnMeta, Error, TabularResult, UserId};
use egret_dataapi::{management_router, serving_router, ClientStore, DataApiState, EndpointStore};
use egret_machine_idp::{generate_rsa_key, sign_claims, IdpError, KeyMaterial, SigningKeyStore};
use egret_metastore::Db;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A query engine under the test's control.
// ---------------------------------------------------------------------------

/// A fake engine driven entirely by the first integer literal in the SQL.
///
/// Keying off the SQL rather than off shared mutable state is what makes
/// [`a_draft_edit_does_not_change_what_is_served`] evidence: `SELECT 1 AS n` and
/// `SELECT 2 AS n` are distinguishable in the RESPONSE, so the test can tell
/// which text actually reached the engine without inspecting anything internal.
///
/// The other markers exist because their branches are not reachable from a
/// well-behaved engine at all:
///
/// | marker | what it stands for |
/// |---|---|
/// | `44` | a connector bug — a ROW longer than the column list the connector itself reported |
/// | `55` | drift by NAME — same count, `describe` froze `n` and `execute` returns `m` |
/// | `66` | two columns whose `camel` forms collide (`total_revenue`, `totalRevenue`) |
/// | `77` | drift by COUNT — `execute` returns a column `describe` did not (an added table column) |
/// | `88` | drift by ORDER — the same names in a different position |
/// | `99` | the connector fetched `max_rows + 1`, saw the extra row, and set `truncated` |
///
/// The three drift markers are separate because they pin three separate clauses
/// of one predicate. One marker would show that *a* check exists, not that each
/// clause is load-bearing — and the count clause in particular is the only thing
/// standing between an `arrays` response and rows longer than its own
/// `meta.columns`.
struct FakePort;

fn first_int(sql: &str) -> i64 {
    sql.split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
        .unwrap_or(-1)
}

fn col(name: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        ty: "integer".into(),
    }
}

/// What publish freezes into the snapshot for this SQL — i.e. what `describe_as`
/// resolved in the publish window.
fn frozen_columns(sql: &str) -> Vec<ColumnMeta> {
    match first_int(sql) {
        66 => vec![col("total_revenue"), col("totalRevenue")],
        88 => vec![col("a"), col("b")],
        _ => vec![col("n")],
    }
}

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for FakePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(frozen_columns(sql))
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        let n = first_int(sql);
        // The three drifts. A real engine produces each of these when the
        // underlying table is altered between publish and call — renamed (55),
        // a column added (77), reordered (88) — and nothing else can.
        let columns = match n {
            55 => vec![col("m")],
            77 => vec![col("n"), col("added_later")],
            88 => vec![col("b"), col("a")],
            _ => frozen_columns(sql),
        };
        // Rows are length-matched to `columns` BY CONSTRUCTION, which is what a
        // well-behaved connector guarantees — and precisely why the arity guard
        // needs a marker to reach at all. `44` emits one extra value so the row
        // disagrees with the column list the connector itself just reported.
        // That is a connector bug, not engine drift: the column LISTS still
        // agree, so the drift check above passes and the arity guard is the only
        // thing left.
        let width = columns.len() + usize::from(n == 44);
        let rows = vec![(0..width)
            .map(|i| serde_json::json!(n + i as i64))
            .collect()];
        Ok(TabularResult {
            columns,
            rows,
            row_count: 1,
            bytes_scanned: None,
            truncated: n == 99,
        })
    }
}

// ---------------------------------------------------------------------------
// The IdP key, and tokens minted against it.
// ---------------------------------------------------------------------------

/// RSA-2048 generation is expensive in a debug build; one key for the binary.
fn active_key() -> &'static KeyMaterial {
    static KEY: OnceLock<KeyMaterial> = OnceLock::new();
    KEY.get_or_init(|| generate_rsa_key("egret-active").expect("generate key"))
}

struct FakeKeys;

#[async_trait::async_trait]
impl SigningKeyStore for FakeKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        Ok(vec![active_key().public_jwk.clone()])
    }
}

/// The exact claim shape P2's `ClientStore::verify` mints.
fn mint(ws: Uuid, user: Uuid) -> String {
    sign_claims(
        active_key(),
        &serde_json::json!({
            "iss": "https://egret.test",
            "sub": format!("egret-dataapi-{}", Uuid::new_v4().simple()),
            "aud": "data-api",
            "exp": chrono::Utc::now().timestamp() + 600,
            "ws": ws.to_string(),
            "egret_user_id": user.to_string(),
        }),
    )
    .expect("sign")
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fixture {
    db: Db,
    /// The serving router, nested exactly as `egret-server` mounts it.
    serving: axum::Router,
    /// The management router plus a session token holding `data_api_endpoint:*`,
    /// used to PUBLISH and to PATCH drafts through the real routes.
    mgmt: axum::Router,
    session: String,
    ws_slug: String,
    ws_id: Uuid,
    other_ws_id: Uuid,
    /// The service account the serving token is minted for.
    sa_user: Uuid,
    admin: Uuid,
    database_id: Uuid,
}

async fn insert_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn insert_workspace(db: &Db, owner: Uuid, tag: &str) -> (Uuid, String) {
    let slug = format!("{tag}-{}", Uuid::new_v4().simple());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Data API Serving', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn grant(db: &Db, user: Uuid, ws: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_serve_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert role");
    for p in perms {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&db.pool)
            .await
            .expect("grant perm");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant role");
}

fn build_state(db: &Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> DataApiState {
    DataApiState {
        db: db.clone(),
        clients: ClientStore::new(db.clone()),
        endpoints: EndpointStore::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        query: Arc::new(FakePort),
        keys: Arc::new(FakeKeys),
        issuer: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: egret_dataapi::serving::ResponseCache::new(),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
    }
}

async fn fixture() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let admin = insert_user(&db, "serving_admin").await;
    let (ws_id, ws_slug) = insert_workspace(&db, admin, "serving").await;
    let (other_ws_id, _) = insert_workspace(&db, admin, "serving-other").await;
    let sa_user = insert_user(&db, "serving_sa").await;

    let database_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, delegated, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, false, $3) RETURNING id",
    )
    .bind(ws_id)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("insert database");

    let author = insert_user(&db, "serving_author").await;
    grant(
        &db,
        author,
        ws_id,
        &["data_api_endpoint:read", "data_api_endpoint:manage"],
    )
    .await;

    // The REAL PDP: a mocked decision would pin the consumer, not the server.
    // Built after the grant, because `CedarPdp` snapshots policies at build time.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(&db, pdp);
    let session = st
        .sessions
        .create(egret_core::ids::UserId(author), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    Some(Fixture {
        db,
        serving: axum::Router::new()
            .nest("/data-api/v1", serving_router())
            .with_state(st.clone()),
        mgmt: axum::Router::new()
            .nest("/api", management_router())
            .with_state(st),
        session,
        ws_slug,
        ws_id,
        other_ws_id,
        sa_user,
        admin,
        database_id,
    })
}

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

struct Res {
    status: StatusCode,
    body: serde_json::Value,
}

impl Res {
    /// The `data` array, whatever shape it carries, flattened to the first cell
    /// of each row — enough to say *which SQL ran*, which is all these tests ask.
    fn ns(&self) -> Vec<i64> {
        self.body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("no data array in {}", self.body))
            .iter()
            .map(|row| match row {
                serde_json::Value::Object(m) => m.values().next().unwrap().as_i64().unwrap(),
                serde_json::Value::Array(a) => a[0].as_i64().unwrap(),
                other => panic!("unexpected row {other}"),
            })
            .collect()
    }
}

impl Fixture {
    /// A serving call with a token minted for this workspace's service account.
    async fn call(&self, path: &str) -> Res {
        let token = mint(self.ws_id, self.sa_user);
        self.call_with(path, &token).await
    }

    async fn call_with(&self, path: &str, token: &str) -> Res {
        self.request("GET", &self.ws_slug, path, Some(token)).await
    }

    async fn request(&self, method: &str, slug: &str, path: &str, token: Option<&str>) -> Res {
        // Joined so a leading slash in `path` produces a genuinely different
        // wildcard capture, which is the whole point of the normalisation test.
        let uri = format!("/data-api/v1/{slug}/{path}");
        let mut b = Request::builder().method(method).uri(&uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let res = self
            .serving
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        Res {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        }
    }

    async fn mgmt_send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("Cookie", format!("egret_session={}", self.session));
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let res = self
            .mgmt
            .clone()
            .oneshot(
                b.body(
                    body.map(|v| Body::from(v.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// Create a draft and publish it through the REAL publish route, so the
    /// snapshot under test is the one publish actually freezes.
    async fn publish_endpoint(&self, method: &str, path: &str, sql: &str) -> Uuid {
        self.publish_endpoint_with_output(method, path, sql, serde_json::json!({}))
            .await
    }

    async fn publish_endpoint_with_output(
        &self,
        method: &str,
        path: &str,
        sql: &str,
        output: serde_json::Value,
    ) -> Uuid {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, sql, output, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(method)
        .bind(self.database_id)
        .bind(sql)
        .bind(&output)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft");

        let (status, body) = self
            .mgmt_send(
                "POST",
                &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", self.ws_slug),
                Some(serde_json::json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "publish failed: {body}");
        ep
    }

    async fn patch_draft(&self, ep: Uuid, body: serde_json::Value) {
        let (status, b) = self
            .mgmt_send(
                "PATCH",
                &format!("/api/ws/{}/data-api/endpoints/{ep}", self.ws_slug),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "patch failed: {b}");
    }

    async fn disable(&self, ep: Uuid) {
        let (status, b) = self
            .mgmt_send(
                "POST",
                &format!("/api/ws/{}/data-api/endpoints/{ep}/disable", self.ws_slug),
                Some(serde_json::json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "disable failed: {b}");
    }

    async fn archive_workspace(&self) {
        sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
            .bind(self.ws_id)
            .execute(&self.db.pool)
            .await
            .expect("archive workspace");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_inbound_path_is_normalised_before_lookup() {
    // axum percent-decodes the wildcard capture AND strips the leading slash;
    // stored paths were never encoded and always carry one. Without normalising
    // the inbound path this lookup misses and the endpoint is unreachable
    // forever, with nothing reporting why.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/reports/daily", "SELECT 1 AS n")
        .await;

    // All three are producible, and each hands the handler a DIFFERENT capture.
    // Measured against matchit 0.8.4 (the version in Cargo.lock) through this
    // exact route:
    //
    //   /data-api/v1/w/reports/daily    -> path = "reports/daily"
    //   /data-api/v1/w//reports/daily   -> path = "/reports/daily"
    //   /data-api/v1/w/reports/daily/   -> path = "reports/daily/"
    //
    // `reports/daily` is the capture axum produces for an ordinary URL, and it
    // differs from the stored `/reports/daily` — a raw `WHERE path = $1` fails on
    // this one alone. `//` in the URI is the one way to produce a LEADING-slash
    // capture, and `normalize_path` absorbs exactly one leading slash, so it must
    // still hit; it is not an empty segment (`endpoints.rs:379` strips the
    // prefix before the segment check ever runs). The trailing slash is the
    // copy-paste case, absorbed the same way.
    for variant in ["reports/daily", "/reports/daily", "reports/daily/"] {
        let res = f.call(variant).await;
        assert_eq!(
            res.status,
            StatusCode::OK,
            "variant {variant:?} must reach the endpoint: {}",
            res.body
        );
        assert_eq!(res.ns(), vec![1]);
    }
}

#[tokio::test]
async fn a_path_that_cannot_be_canonicalised_is_404_not_400() {
    // These are the captures `normalize_path` genuinely REFUSES: an INTERIOR
    // `//` is an empty segment, and `.`/`..` are dot segments. Each is a 404
    // rather than a 400, so a caller cannot tell "malformed" from "absent" and
    // use the difference to probe.
    //
    // Note what is deliberately NOT in this list. A LEADING `//` is absorbed and
    // hits (see the test above), and `%2F` is percent-decoded by axum into an
    // ordinary separator — `/data-api/v1/w/reports%2Fdaily` arrives as
    // `reports/daily` and reaches the endpoint. Both were measured; neither is a
    // refusal, and writing them here as one would be a false claim about a
    // security-relevant function.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/reports/daily", "SELECT 1 AS n")
        .await;

    for bad in ["reports/../daily", "reports//daily", "reports/./daily"] {
        let res = f.call(bad).await;
        assert_eq!(
            res.status,
            StatusCode::NOT_FOUND,
            "{bad:?} must be 404, got {} {}",
            res.status,
            res.body
        );
    }
}

#[tokio::test]
async fn a_draft_edit_does_not_change_what_is_served() {
    // The invariant P3 exists to protect, asserted through the SERVING surface
    // rather than through the store: routing follows the endpoint row, content
    // follows the snapshot. If this read `e.sql`, versioning is decoration.
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    assert_eq!(f.call("x").await.ns(), vec![1]);

    f.patch_draft(ep, serde_json::json!({ "sql": "SELECT 2 AS n" }))
        .await;

    // Anti-triviality: the draft really did change, so a serving path that read
    // it would return 2 and this assertion would have something to fail on.
    let draft_sql: String = sqlx::query_scalar("SELECT sql FROM dataapi.endpoints WHERE id = $1")
        .bind(ep)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(draft_sql, "SELECT 2 AS n", "the draft edit did not land");

    assert_eq!(
        f.call("x").await.ns(),
        vec![1],
        "serving must follow the published snapshot, not the draft"
    );
}

#[tokio::test]
async fn a_disabled_endpoint_is_404_not_403() {
    // Disable keeps the row and its `live_version_id` (an enable must restore
    // the same version), so only the `state = 'live'` predicate makes this a
    // miss. 404 and not 403: a caller must not learn the endpoint exists.
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    assert_eq!(f.call("x").await.status, StatusCode::OK);

    f.disable(ep).await;
    assert_eq!(f.call("x").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_archived_workspace_stops_serving() {
    // P2's `ClientStore::verify` refuses to MINT for an archived workspace but
    // cannot recall a token already issued. The workspace row is in hand here,
    // so the archive case costs one comparison — and archiving is the operator's
    // most natural decommission action.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    let token = mint(f.ws_id, f.sa_user);
    assert_eq!(f.call_with("x", &token).await.status, StatusCode::OK);

    f.archive_workspace().await;
    assert_eq!(
        f.call_with("x", &token).await.status,
        StatusCode::NOT_FOUND,
        "the same still-unexpired token must stop working"
    );
}

#[tokio::test]
async fn a_token_for_another_workspace_is_404() {
    // Not 403: a caller must not learn the endpoint exists. The slug in the URL
    // is this workspace's; only the TOKEN disagrees.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let other = mint(f.other_ws_id, f.sa_user);
    assert_eq!(f.call_with("x", &other).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_published_endpoint_is_not_reachable_under_another_workspaces_slug() {
    // The mirror of the test above: the TOKEN is right and the SLUG is another
    // workspace's. Both halves must agree, or the slug segment is decoration.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let other_slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
        .bind(f.other_ws_id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let token = mint(f.ws_id, f.sa_user);
    let res = f.request("GET", &other_slug, "x", Some(&token)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_method_is_part_of_the_route() {
    // `UNIQUE (workspace_id, method, path)` is the routing key, so a POST must
    // not reach a GET endpoint. 404 rather than 405: 405 would confirm the path
    // exists.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let token = mint(f.ws_id, f.sa_user);
    let res = f.request("POST", &f.ws_slug, "x", Some(&token)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_post_endpoint_is_served_on_post() {
    // The counterpart to the test above, and the reason `serving_router` accepts
    // POST at all: `endpoints.method` allows GET and POST.
    let f = skip_without_db!();
    f.publish_endpoint("POST", "/w", "SELECT 7 AS n").await;

    let token = mint(f.ws_id, f.sa_user);
    let res = f.request("POST", &f.ws_slug, "w", Some(&token)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.ns(), vec![7]);
}

#[tokio::test]
async fn the_response_carries_the_serving_version_and_the_frozen_columns() {
    // `meta.version` makes a client-side bug reproducible against the exact
    // snapshot that produced it, and the columns are the FROZEN ones — the
    // published contract, not whatever the engine returned today.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let res = f.call("x").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.body["meta"]["version"], 1);
    assert_eq!(res.body["meta"]["row_count"], 1);
    assert_eq!(res.body["meta"]["cached"], false);
    assert_eq!(res.body["meta"]["columns"][0]["name"], "n");
}

#[tokio::test]
async fn an_unpublished_endpoint_is_404() {
    // A draft that was never published has no `live_version_id`; the JOIN alone
    // must not be what excludes it, but the result is the same 404.
    let f = skip_without_db!();
    sqlx::query(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, method, database_id, sql, created_by) \
         VALUES ($1, 'draft-only', '/never', 'GET', $2, 'SELECT 1 AS n', $3)",
    )
    .bind(f.ws_id)
    .bind(f.database_id)
    .bind(f.admin)
    .execute(&f.db.pool)
    .await
    .unwrap();

    assert_eq!(f.call("never").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_request_with_no_bearer_is_401_before_any_lookup() {
    // 401 and not 404: authentication is answered before the endpoint is, so an
    // unauthenticated prober learns nothing about which paths exist either way.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let res = f.request("GET", &f.ws_slug, "x", None).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_truncated_result_is_an_error_not_a_partial_200() {
    // Returning 200 with truncated data is the worst available failure mode for
    // a data API: the client believes it has every row and produces wrong
    // numbers with no signal. Driven by `TabularResult.truncated`, never by
    // comparing `row_count` to the cap — a count equal to the cap cannot
    // distinguish "exactly at" from "over".
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/big", "SELECT 99 AS n").await;

    let res = f.call("big").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
}

#[tokio::test]
async fn a_percent_encoded_slash_is_a_separator_not_a_refusal() {
    // Pinned because the opposite was asserted in a comment before it was
    // measured. axum percent-decodes the `{*path}` capture BEFORE the handler
    // sees it, so `%2F` is indistinguishable from a literal `/` by the time
    // `normalize_path` runs — there is no encoding left to reject.
    //
    // This is the behaviour P3's `normalize_path` doc describes and the reason
    // it refuses to STORE a percent-encoded path: the two sides must agree on
    // one spelling, and the decoded one is the only spelling serving can see.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/reports/daily", "SELECT 1 AS n")
        .await;

    let res = f.call("reports%2Fdaily").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.ns(), vec![1]);
}

#[tokio::test]
async fn a_drifted_output_schema_is_refused_rather_than_reshaped() {
    // `meta.columns` advertises the FROZEN contract. If the engine now returns a
    // different column set, every honest option except refusing produces a body
    // that contradicts its own metadata: keys from the live set disagree with
    // the advertised names, and zipping the two truncates to the shorter side —
    // silently dropping a column from every row.
    //
    // The `55` marker makes publish freeze `n` (via `describe_as`) and serving
    // receive `m` (via `execute_as`), which is what altering the underlying
    // table between publish and call does.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/drift", "SELECT 55 AS n").await;

    let res = f.call("drift").await;
    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a drifted schema must be refused, not reshaped: {} {}",
        res.status,
        res.body
    );
}

#[tokio::test]
async fn a_naming_collision_is_refused_rather_than_dropping_a_column() {
    // `apply_naming` can CREATE a collision that the frozen columns do not have:
    // `total_revenue` and `totalRevenue` both become `totalRevenue` under
    // `camel`, and a JSON object keeps only the last value — so one column
    // vanishes from every row while `meta.columns` still advertises it.
    //
    // Anti-triviality: the same endpoint with `as_is` must serve BOTH columns,
    // so the refusal is caused by the naming rule and not by the column pair.
    let f = skip_without_db!();
    f.publish_endpoint_with_output(
        "GET",
        "/asis",
        "SELECT 66 AS total_revenue",
        serde_json::json!({ "naming": "as_is" }),
    )
    .await;
    let ok = f.call("asis").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(
        ok.body["data"][0].as_object().map(|o| o.len()),
        Some(2),
        "both frozen columns must survive under as_is: {}",
        ok.body
    );

    f.publish_endpoint_with_output(
        "GET",
        "/camel",
        "SELECT 66 AS total_revenue",
        serde_json::json!({ "naming": "camel" }),
    )
    .await;
    let res = f.call("camel").await;
    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a naming collision must be refused, not served one column short: {} {}",
        res.status,
        res.body
    );
}

#[tokio::test]
async fn an_arrays_shape_serves_positional_rows() {
    // The `arrays` branch had no serving test at all, which is how the gap below
    // stayed invisible: every shaping test drove `objects`.
    //
    // `arrays` is positional — a client indexes `data[i][j]` against
    // `meta.columns[j]` — which is why the drift check compares column ORDER and
    // not just the set, and why the count clause matters more here than in
    // `objects` (there is no per-row arity guard on this branch: the row is
    // cloned through verbatim).
    let f = skip_without_db!();
    f.publish_endpoint_with_output(
        "GET",
        "/arr",
        "SELECT 66 AS total_revenue",
        serde_json::json!({ "shape": "arrays" }),
    )
    .await;

    let res = f.call("arr").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.body["data"],
        serde_json::json!([[66, 67]]),
        "rows must be arrays, positionally aligned to meta.columns: {}",
        res.body
    );
    // The alignment the client depends on, asserted rather than assumed.
    let cols = res.body["meta"]["columns"].as_array().unwrap();
    assert_eq!(cols.len(), res.body["data"][0].as_array().unwrap().len());
    assert_eq!(cols[0]["name"], "total_revenue");
    assert_eq!(cols[1]["name"], "totalRevenue");
}

#[tokio::test]
async fn a_wider_live_result_is_refused_even_in_arrays_shape() {
    // Pins the COUNT clause of the drift check specifically, on the branch where
    // its absence is silent: `arrays` clones each row through verbatim with no
    // arity guard, so an added table column emits rows of 2 values under a
    // `meta.columns` of 1 — a body contradicting its own metadata, with a 200.
    //
    // The `objects` branch degrades to a 500 instead (the per-row arity guard
    // catches it), which is wrong but loud. This is the quiet one.
    let f = skip_without_db!();
    f.publish_endpoint_with_output(
        "GET",
        "/wider",
        "SELECT 77 AS n",
        serde_json::json!({ "shape": "arrays" }),
    )
    .await;

    let res = f.call("wider").await;
    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a live result with MORE columns than the snapshot froze must be refused: {} {}",
        res.status,
        res.body
    );
}

#[tokio::test]
async fn a_reordered_live_result_is_refused() {
    // Pins the ORDER sensitivity of the drift check. The frozen columns are
    // [a, b] and the engine now returns [b, a] — the same SET, so a comparison
    // "simplified" to a sorted or set-based one passes every other test in this
    // file while `arrays` responses silently transpose two columns and `objects`
    // responses attach each value to the wrong name.
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/reordered", "SELECT 88 AS a")
        .await;

    let res = f.call("reordered").await;
    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "the same names in a different order are still a different contract: {} {}",
        res.status,
        res.body
    );
}

#[tokio::test]
async fn a_row_wider_than_its_own_column_list_is_refused_in_both_shapes() {
    // The case the column-list drift check cannot see. Here the live and frozen
    // column lists AGREE — so `SchemaDrift` does not fire — and it is the ROW
    // that carries an extra value. A connector bug rather than table drift.
    //
    // The guard used to live inside the `objects` loop only, so `arrays` cloned
    // such a row through verbatim and served `[[44,45]]` under a one-column
    // `meta.columns`, silently, while `objects` refused the identical result.
    // Both shapes are asserted here because the asymmetry — not either shape on
    // its own — was the defect.
    let f = skip_without_db!();
    f.publish_endpoint_with_output(
        "GET",
        "/wide-row-arrays",
        "SELECT 44 AS n",
        serde_json::json!({ "shape": "arrays" }),
    )
    .await;
    f.publish_endpoint_with_output(
        "GET",
        "/wide-row-objects",
        "SELECT 44 AS n",
        serde_json::json!({ "shape": "objects" }),
    )
    .await;

    // Both are collected before either is asserted, so a regression reports what
    // BOTH shapes did. Asserting inside the loop would stop at `arrays` and hide
    // whether `objects` had regressed with it — which, now that the guard is
    // shared, is exactly the thing that would move together.
    let arrays = f.call("wide-row-arrays").await;
    let objects = f.call("wide-row-objects").await;
    assert_eq!(
        (arrays.status, objects.status),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::INTERNAL_SERVER_ERROR
        ),
        "a row wider than its column list must be refused in BOTH shapes.\n\
         arrays:  {} {}\n\
         objects: {} {}",
        arrays.status,
        arrays.body,
        objects.status,
        objects.body
    );
}
