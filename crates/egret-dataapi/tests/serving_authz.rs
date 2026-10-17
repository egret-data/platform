//! P4 Task 6 — spec §6.2 step 4: `data_api_endpoint:invoke`.
//!
//! **The finding this file exists for.** Tasks 1–5 implemented nine of the ten
//! steps in the spec's request chain, and step 4 — the authorization check — had
//! no owner. The consequence was not a missing nicety: any valid, unexpired
//! `data-api` token for workspace W reached EVERY live endpoint in W, so a
//! service account created for one consumer could invoke every other consumer's
//! published endpoint and receive its full result set. Service-account
//! granularity collapsed to workspace granularity and the endpoint was not an
//! access-control boundary at all.
//!
//! The residual defence was thinner than it looks. The query does run AS the
//! service account, so where Ranger is enforcing, per-principal row filtering
//! still applied — but on an engine without Ranger, or on StarRocks where
//! enforcement is resource-only, there was no second line.
//!
//! [`a_token_cannot_invoke_an_endpoint_it_was_not_granted`] is that finding,
//! written as a test. Delete the `invoke_allowed` call in
//! `serving/router.rs::run` and it fails with 200 where it wanted 403.
//!
//! # Why the grants are written through the REAL routes
//!
//! Every grant here goes through `POST …/endpoints/{id}/grants` rather than an
//! `INSERT` this file composes. Two reasons, and the second is the load-bearing
//! one:
//!
//! * the route is half of what this task built, so a test that bypassed it would
//!   check the reader against a fixture rather than against the writer;
//! * `access_grants` has six columns that all have to agree for
//!   `AccessGrantStore::grants_for` to load the row and for `link_grants` to link
//!   a policy that matches an Object resource — `scope_type`, `object_type`,
//!   `scope_id`, `role_id`, `principal_type`, `effect`. A hand-written row that
//!   got any one of them wrong (a platform-scoped role, say, whose template uses
//!   `resource == ?resource` and therefore never matches an object) would be
//!   silently inert, and a test built on it would pass while proving nothing.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no `DATABASE_URL` FAILS, so check the
//! DURATION, not just the `ok`.

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
// A trivially successful engine.
// ---------------------------------------------------------------------------

/// Always returns one row of one column. Nothing in this file is about what the
/// engine does — the whole subject is whether the request reaches it at all, so
/// a port that can fail would only add ways for a test to be red for the wrong
/// reason.
struct FakePort;

fn col(name: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        ty: "integer".into(),
    }
}

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for FakePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![col("n")])
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        Ok(TabularResult {
            columns: vec![col("n")],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }
}

// ---------------------------------------------------------------------------
// The IdP key, and tokens minted against it.
// ---------------------------------------------------------------------------

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

/// P2's claim shape. The token is VALID in every test below — that is the point:
/// this suite is entirely about what a well-formed, unexpired, correct-audience
/// credential is and is not allowed to reach.
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
    serving: axum::Router,
    mgmt: axum::Router,
    /// A session holding `data_api_endpoint:read|manage` + `service_account:*`
    /// in [`Self::ws_id`]. Publishes, grants and revokes.
    manager: String,
    /// A session holding `data_api_endpoint:read` and NOTHING else in
    /// [`Self::ws_id`]. It may look at endpoints; it may not hand out the right
    /// to call one.
    reader: String,
    /// A session holding `data_api_endpoint:manage` in [`Self::other_ws_id`] —
    /// a legitimate manager, of the wrong workspace.
    other_manager: String,
    ws_slug: String,
    ws_id: Uuid,
    other_ws_slug: String,
    /// The service account whose token every `call*` below presents.
    service_account: Uuid,
    /// A second one, so "a grant on one account" can be told from "a grant".
    other_account: Uuid,
    /// A service account belonging to [`Self::other_ws_id`].
    foreign_account: Uuid,
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
         VALUES ($1, 'Data API Authz', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

/// A service account: a real `users` row with `provider = 'service'` plus its
/// `dataapi.service_accounts` binding, which is what `scoped_account` reads.
async fn insert_service_account(db: &Db, ws: Uuid, creator: Uuid, tag: &str) -> Uuid {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('service', $1, $2, 'active', NULL) RETURNING id",
    )
    .bind(format!(
        "svc.{tag}.{}@ws.service.egret.local",
        Uuid::new_v4().simple()
    ))
    .bind(tag)
    .fetch_one(&db.pool)
    .await
    .expect("insert service account user");
    sqlx::query(
        "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, created_by) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user)
    .bind(ws)
    .bind(format!("{tag}-{}", Uuid::new_v4().simple()))
    .bind(creator)
    .execute(&db.pool)
    .await
    .expect("insert service account row");
    user
}

/// A workspace-scoped role carrying `perms`, granted to `user` in `ws`.
///
/// **Every role this fixture needs must exist before the PDP is built**:
/// `CedarPdp` compiles one template per role at construction and links a grant
/// only if its `role_id` has a compiled template. A role created afterwards
/// produces grants that are silently skipped — which would make
/// [`granting_requires_manage_not_read`] pass for the wrong reason, by denying a
/// caller whose permission never reached the PDP at all.
async fn grant(db: &Db, user: Uuid, ws: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_t6_{}", Uuid::new_v4().simple()))
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

async fn fixture() -> Option<Fixture> {
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");

    let admin = insert_user(&db, "t6_admin").await;
    let (ws_id, ws_slug) = insert_workspace(&db, admin, "t6").await;
    let (other_ws_id, other_ws_slug) = insert_workspace(&db, admin, "t6-other").await;

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

    let service_account = insert_service_account(&db, ws_id, admin, "primary").await;
    let other_account = insert_service_account(&db, ws_id, admin, "secondary").await;
    let foreign_account = insert_service_account(&db, other_ws_id, admin, "foreign").await;

    let manager_user = insert_user(&db, "t6_manager").await;
    grant(
        &db,
        manager_user,
        ws_id,
        &[
            "data_api_endpoint:read",
            "data_api_endpoint:manage",
            "service_account:read",
            "service_account:manage",
        ],
    )
    .await;
    let reader_user = insert_user(&db, "t6_reader").await;
    grant(&db, reader_user, ws_id, &["data_api_endpoint:read"]).await;
    let other_manager_user = insert_user(&db, "t6_other_manager").await;
    grant(
        &db,
        other_manager_user,
        other_ws_id,
        &["data_api_endpoint:read", "data_api_endpoint:manage"],
    )
    .await;

    // The REAL PDP, and the real `data_api_invoker` role migration 0124 seeds.
    // A mocked decision here would pin the consumer, not the server — and the
    // server is the thing that was missing.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = DataApiState {
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
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // This file is about serving-path authz, not Activity — a fixed
        // retention is enough to satisfy the field.
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
    };

    let mut sessions = Vec::new();
    for u in [manager_user, reader_user, other_manager_user] {
        sessions.push(
            st.sessions
                .create(egret_core::ids::UserId(u), "test", "127.0.0.1")
                .await
                .expect("create session")
                .token,
        );
    }

    Some(Fixture {
        db,
        serving: axum::Router::new()
            .nest("/data-api/v1", serving_router())
            .with_state(st.clone()),
        mgmt: axum::Router::new()
            .nest("/api", management_router())
            .with_state(st),
        manager: sessions[0].clone(),
        reader: sessions[1].clone(),
        other_manager: sessions[2].clone(),
        ws_slug,
        ws_id,
        other_ws_slug,
        service_account,
        other_account,
        foreign_account,
        admin,
        database_id,
    })
}

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
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
    /// Spec §7's machine-readable code, which is what a client branches on. The
    /// STATUS alone cannot separate `not_granted` from `data_access_denied` —
    /// both are 403 — and those are two different systems to go and fix.
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or("")
    }
}

impl Fixture {
    /// A serving call as [`Self::service_account`].
    async fn call(&self, path: &str) -> Res {
        self.call_as(self.service_account, path).await
    }

    async fn call_as(&self, sa: Uuid, path: &str) -> Res {
        let token = mint(self.ws_id, sa);
        let uri = format!("/data-api/v1/{}/{path}", self.ws_slug);
        let res = self
            .serving
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
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
        session: &str,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Res {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("Cookie", format!("egret_session={session}"));
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
        Res {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        }
    }

    /// Create a draft and publish it through the REAL publish route.
    async fn publish_endpoint(&self, method: &str, path: &str, sql: &str) -> Uuid {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, created_by) \
             VALUES ($1, $2, $3, $4, $5, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', $6::text)), $7) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(method)
        .bind(self.database_id)
        .bind(sql)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft");

        let r = self
            .mgmt_send(
                &self.manager,
                "POST",
                &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", self.ws_slug),
                Some(serde_json::json!({})),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "publish failed: {}", r.body);
        ep
    }

    /// Grant invoke through the real route, as the workspace's manager.
    async fn grant_invoke(&self, sa: Uuid, ep: Uuid) -> Res {
        self.grant_as(&self.manager, &self.ws_slug, sa, ep).await
    }

    async fn grant_as(&self, session: &str, ws_slug: &str, sa: Uuid, ep: Uuid) -> Res {
        self.mgmt_send(
            session,
            "POST",
            &format!("/api/ws/{ws_slug}/data-api/endpoints/{ep}/grants"),
            Some(serde_json::json!({ "service_account_id": sa })),
        )
        .await
    }

    async fn revoke_invoke(&self, sa: Uuid, ep: Uuid) -> Res {
        self.mgmt_send(
            &self.manager,
            "DELETE",
            &format!(
                "/api/ws/{}/data-api/endpoints/{ep}/grants/{sa}",
                self.ws_slug
            ),
            None,
        )
        .await
    }

    async fn list_grants(&self, session: &str, ep: Uuid) -> Res {
        self.mgmt_send(
            session,
            "GET",
            &format!("/api/ws/{}/data-api/endpoints/{ep}/grants", self.ws_slug),
            None,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// **The finding, as a test.** Before step 4 existed this asserted 200 twice.
#[tokio::test]
async fn a_token_cannot_invoke_an_endpoint_it_was_not_granted() {
    let f = skip_without_db!();
    let mine = f.publish_endpoint("GET", "/mine", "SELECT 1 AS n").await;
    let _theirs = f.publish_endpoint("GET", "/theirs", "SELECT 1 AS n").await;
    let g = f.grant_invoke(f.service_account, mine).await;
    assert_eq!(g.status, StatusCode::OK, "grant failed: {}", g.body);

    let ok = f.call("mine").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);

    let denied = f.call("theirs").await;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "a grant on one endpoint must not reach another — this is the whole \
         finding: without step 4 a token for the workspace reached every live \
         endpoint in it. Got {}",
        denied.body
    );
}

/// 403 and not 404, and the reason is the reverse of every step above it.
#[tokio::test]
async fn an_ungranted_call_is_403_and_not_404() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let denied = f.call("x").await;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "by step 4 the caller has proved it holds a valid credential for THIS \
         workspace, so hiding the endpoint's existence buys nothing it could not \
         already enumerate — and a 404 would send the operator hunting for a \
         deleted endpoint instead of a missing grant: {}",
        denied.body
    );
    // A path with no endpoint behind it is still a 404, so the assertion above
    // is about the GRANT and not about this suite reaching a 403 for any reason.
    assert_eq!(
        f.call("no-such-path").await.status,
        StatusCode::NOT_FOUND,
        "an absent endpoint must still be indistinguishable from one in another \
         workspace"
    );
}

/// The two 403s are different systems' problems, so they are different codes.
#[tokio::test]
async fn the_refusal_carries_not_granted_and_not_data_access_denied() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    let denied = f.call("x").await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(
        denied.code(),
        "not_granted",
        "`data_access_denied` means the ENGINE refused the data and the fix is a \
         Ranger grant; this means egret refused the CALL and the fix is a grant \
         on the endpoint. Sharing one code sends the operator to the wrong \
         system: {}",
        denied.body
    );
    assert!(
        !denied.body.to_string().contains("data_access_denied"),
        "{}",
        denied.body
    );
}

/// **The asymmetry worth writing into the spec.** A grant is read on every
/// request, so revoking one takes effect on the NEXT call. Revoking the client
/// cannot do that — the serving path verifies a stateless JWT, so an issued
/// token stays good until `exp` and the 600s TTL is an irreducible bound.
#[tokio::test]
async fn revoking_the_grant_stops_the_calls_immediately() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    let g = f.grant_invoke(f.service_account, ep).await;
    assert_eq!(g.status, StatusCode::OK, "grant failed: {}", g.body);
    assert_eq!(f.call("x").await.status, StatusCode::OK);

    let r = f.revoke_invoke(f.service_account, ep).await;
    assert_eq!(r.status, StatusCode::OK, "revoke failed: {}", r.body);

    // The SAME credential, still unexpired, still the right audience. Nothing
    // about the token changed; the grant did, and the grant is re-read per
    // request.
    let after = f.call("x").await;
    assert_eq!(
        after.status,
        StatusCode::FORBIDDEN,
        "unlike revoking the client, a grant revocation IS immediate — it is the \
         operational answer to 'cut this consumer off right now': {}",
        after.body
    );
}

/// A grant names one account, not the workspace's accounts.
#[tokio::test]
async fn a_grant_reaches_only_the_account_it_names() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    f.grant_invoke(f.service_account, ep).await;

    assert_eq!(
        f.call_as(f.service_account, "x").await.status,
        StatusCode::OK
    );
    assert_eq!(
        f.call_as(f.other_account, "x").await.status,
        StatusCode::FORBIDDEN,
        "a second service account in the same workspace holds no grant of its own"
    );
}

/// Handing out invoke rights mutates the authorization graph. `:read` is a read.
#[tokio::test]
async fn granting_requires_manage_not_read() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let refused = f
        .grant_as(&f.reader, &f.ws_slug, f.service_account, ep)
        .await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "handing out invoke rights is a manage operation: {}",
        refused.body
    );

    // And the refusal was real, not a 403 the reader would get on anything —
    // the same session can read the endpoint it was just refused a grant on.
    let readable = f
        .mgmt_send(
            &f.reader,
            "GET",
            &format!("/api/ws/{}/data-api/endpoints/{ep}", f.ws_slug),
            None,
        )
        .await;
    assert_eq!(
        readable.status,
        StatusCode::OK,
        "the reader must genuinely hold :read, or the assertion above proves \
         nothing about which permission was checked: {}",
        readable.body
    );

    // Nothing was written, so the call is still refused.
    assert_eq!(f.call("x").await.status, StatusCode::FORBIDDEN);
}

/// Listing the grantees enumerates every consumer of the endpoint, so it is
/// manage-only too — the same rule `egret-bi`'s `grant_list` applies.
#[tokio::test]
async fn listing_the_grants_requires_manage_not_read() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    f.grant_invoke(f.service_account, ep).await;

    assert_eq!(
        f.list_grants(&f.reader, ep).await.status,
        StatusCode::FORBIDDEN
    );

    let listed = f.list_grants(&f.manager, ep).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    let rows = listed.body.as_array().expect("array");
    assert_eq!(rows.len(), 1, "{}", listed.body);
    assert_eq!(
        rows[0]["service_account_id"].as_str().unwrap(),
        f.service_account.to_string()
    );
}

/// The ENDPOINT id is scoped to the caller's workspace.
///
/// **The grantee is `foreign_account`, deliberately, and getting that wrong once
/// is why it is spelled out.** With `service_account` (which lives in the OTHER
/// workspace from `other_manager`'s point of view) the call is refused by
/// `scoped_account` before the endpoint predicate is ever consulted — so the
/// test passed with the endpoint predicate deleted and proved nothing about it.
/// Pairing the other workspace's manager with the other workspace's own account
/// makes the endpoint the ONLY id that can refuse the call.
#[tokio::test]
async fn a_grant_cannot_be_written_across_workspaces() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    // A legitimate `:manage` holder of the OTHER workspace, naming its own
    // service account and THIS workspace's endpoint.
    let refused = f
        .grant_as(&f.other_manager, &f.other_ws_slug, f.foreign_account, ep)
        .await;
    assert_eq!(
        refused.status,
        StatusCode::NOT_FOUND,
        "the endpoint does not belong to that workspace, and a 403 would confirm \
         it exists: {}",
        refused.body
    );

    // And nothing was written: the serving path still refuses.
    assert_eq!(f.call("x").await.status, StatusCode::FORBIDDEN);
    let written: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(ep)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(written, 0, "a cross-workspace grant row was written");
}

/// The SERVICE ACCOUNT id is scoped too — the other half of the pair, and the
/// one `EndpointStore`'s built-in predicate does not cover.
#[tokio::test]
async fn a_grant_cannot_name_another_workspaces_service_account() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;

    let refused = f
        .grant_as(&f.manager, &f.ws_slug, f.foreign_account, ep)
        .await;
    assert_eq!(
        refused.status,
        StatusCode::NOT_FOUND,
        "the account belongs to another workspace: {}",
        refused.body
    );
    let written: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(ep)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        written, 0,
        "{refused:?} wrote a grant it should have refused"
    );
}

impl std::fmt::Debug for Res {
    fn fmt(&self, fm: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fm, "{} {}", self.status, self.body)
    }
}

/// Spec §5's shape, asserted on the stored row.
///
/// Not a restatement of the route: it is the contract between this writer and
/// `AccessGrantStore::grants_for`, which is the only reader and matches on
/// `scope_type`/`object_type`/`scope_id` in SQL. Any of these three drifting
/// makes every grant silently inert — the row is written, the list shows it, and
/// every call is refused.
#[tokio::test]
async fn the_stored_row_is_the_shape_the_pdp_reads() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    f.grant_invoke(f.service_account, ep).await;

    let (ptype, scope_type, object_type, scope_id, effect, role_name, granted_by): (
        String,
        String,
        Option<String>,
        Option<Uuid>,
        String,
        String,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT ag.principal_type, ag.scope_type, ag.object_type, ag.scope_id, ag.effect, \
                r.name, ag.granted_by \
         FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         WHERE ag.principal_id = $1 AND ag.scope_id = $2",
    )
    .bind(f.service_account)
    .bind(ep)
    .fetch_one(&f.db.pool)
    .await
    .expect("the grant row");

    assert_eq!(ptype, "user", "a service account IS a user row");
    assert_eq!(scope_type, "object");
    assert_eq!(object_type.as_deref(), Some("data_api_endpoint"));
    assert_eq!(scope_id, Some(ep));
    assert_eq!(effect, "allow");
    assert_eq!(
        role_name, "data_api_invoker",
        "the role must carry ONLY data_api_endpoint:invoke — an object grant \
         links its role's whole action set onto the object"
    );
    assert!(granted_by.is_some(), "who granted it is part of the record");

    // The role's action list is the blast radius of every invoke grant ever
    // written, so it is asserted rather than assumed.
    let perms: Vec<String> = sqlx::query_scalar(
        "SELECT rp.permission FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'data_api_invoker' ORDER BY 1",
    )
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        perms,
        vec!["data_api_endpoint:invoke".to_string()],
        "adding a permission to this role widens every already-written grant on \
         every endpoint at once"
    );
}

/// A grant must not outlive the endpoint it names.
///
/// `access_grants.scope_id` is polymorphic with no FK and no cleanup trigger, so
/// nothing at the database level does this. An orphan is not exploitable — the
/// serving path can never resolve a deleted endpoint — but it accumulates
/// without bound and the grants list renders it with a null name.
#[tokio::test]
async fn deleting_an_endpoint_takes_its_grants_with_it() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    let keep = f.publish_endpoint("GET", "/keep", "SELECT 1 AS n").await;
    f.grant_invoke(f.service_account, ep).await;
    f.grant_invoke(f.service_account, keep).await;

    let r = f
        .mgmt_send(
            &f.manager,
            "DELETE",
            &format!("/api/ws/{}/data-api/endpoints/{ep}", f.ws_slug),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "delete failed: {}", r.body);

    let orphans: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(ep)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(orphans, 0, "the deleted endpoint's grants outlived it");

    // And the sweep is scoped: the OTHER endpoint's grant is untouched.
    let survivors: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(keep)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        survivors, 1,
        "the sweep took a grant on an endpoint it was not deleting"
    );
    assert_eq!(f.call("keep").await.status, StatusCode::OK);
}

/// A 404 delete must not sweep anything — the containment check has to come
/// first, or a caller could clear another workspace's grants on an id it was
/// never allowed to name.
#[tokio::test]
async fn a_refused_delete_sweeps_no_grants() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    f.grant_invoke(f.service_account, ep).await;

    // The other workspace's manager, naming this workspace's endpoint.
    let r = f
        .mgmt_send(
            &f.other_manager,
            "DELETE",
            &format!("/api/ws/{}/data-api/endpoints/{ep}", f.other_ws_slug),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);

    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(ep)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(kept, 1, "a refused delete swept a grant it never owned");
    assert_eq!(f.call("x").await.status, StatusCode::OK);
}

/// Granting twice is a no-op, not a 500 on the unique key.
#[tokio::test]
async fn granting_twice_is_idempotent() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    let first = f.grant_invoke(f.service_account, ep).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let second = f.grant_invoke(f.service_account, ep).await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.body);
    assert_eq!(
        first.body["id"], second.body["id"],
        "the second grant must describe the row the first wrote"
    );

    // And revoking twice is likewise a no-op.
    assert_eq!(
        f.revoke_invoke(f.service_account, ep).await.status,
        StatusCode::OK
    );
    assert_eq!(
        f.revoke_invoke(f.service_account, ep).await.status,
        StatusCode::OK
    );
}

/// A grant is on the ENDPOINT, so it survives a re-publish.
///
/// Worth pinning because the alternative design — keying the grant on the
/// VERSION — is superficially attractive ("the consumer agreed to this
/// contract") and would silently break every consumer the moment an author
/// published a fix.
#[tokio::test]
async fn a_grant_survives_a_republish() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    f.grant_invoke(f.service_account, ep).await;
    assert_eq!(f.call("x").await.status, StatusCode::OK);

    let r = f
        .mgmt_send(
            &f.manager,
            "POST",
            &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", f.ws_slug),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "re-publish failed: {}", r.body);
    assert_eq!(
        f.call("x").await.status,
        StatusCode::OK,
        "a grant is on the endpoint, not on the version it happened to be \
         written against"
    );
}
