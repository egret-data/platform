//! The audit trail for the Data API's mutating routes (P3 Task 5).
//!
//! Driven end-to-end through the HTTP routes against the REAL Cedar PDP and a
//! real Postgres, reading the events off the sink's own receiver — not by
//! calling `mutation_for` directly, which would pin the helper rather than the
//! handlers that are supposed to call it.
//!
//! The load-bearing test is [`issuing_a_client_audits_the_client_id_and_never_the_secret`].
//! `audit_log` is append-only and is read by more people, for longer, than the
//! one-shot issue response ever reaches: a `client_secret` written there
//! outlives every rotation, every revocation, and the `dataapi.clients` row
//! itself. It is the one credential lifetime nothing in the product can shorten,
//! so the assertion checks both the KEY and the secret's VALUE — a rename would
//! walk straight past a key-only check. [`Fixture::drained`] then applies the
//! value half to EVERY event of EVERY test, so the guard is crate-wide and not
//! one assertion on one handler.
//!
//! The other property worth naming is
//! [`deleting_an_endpoint_records_what_it_was`]: `delete` is the one audited
//! route whose subject does not survive it, so its event is the only record
//! that the endpoint existed.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like every sibling
//! suite in this crate — so check the DURATION, not just the `ok`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_audit::{AuditEvent, AuditSink};
use egret_core::{ColumnMeta, Error, UserId};
use egret_dataapi::{management_router, ClientStore, DataApiState, EndpointStore};
use egret_metastore::Db;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tower::ServiceExt;
use uuid::Uuid;

/// A `describe_as` that always resolves, so the publish lifecycle can be driven
/// without a live engine. What this file measures is which AUDIT events the
/// routes emit; the engine's own behaviour is pinned by `lifecycle.rs` and by
/// `egret-query`'s `port_describe.rs`.
struct OkPort;

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for OkPort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![ColumnMeta {
            name: "n".into(),
            ty: "integer".into(),
        }])
    }

    /// This file exercises the AUTHORING mutations only; nothing here serves a
    /// request, so a call would mean a route reached the engine unexpectedly.
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, Error> {
        unreachable!("authoring mutations must never execute a query")
    }
}

/// One emitted mutation, flattened to what an assertion actually reads.
#[derive(Debug)]
struct Emitted {
    actor: Option<Uuid>,
    workspace_id: Option<Uuid>,
    action: String,
    resource_type: String,
    resource_id: Option<String>,
    detail: serde_json::Value,
}

struct Fixture {
    db: Db,
    ws: String,
    ws_id: Uuid,
    admin: Uuid,
    database_id: Uuid,
    manager: Caller,
    /// The sending half, kept so a second principal can be built onto the SAME
    /// channel — otherwise "that caller emitted nothing" would be true of any
    /// caller whose events went somewhere else.
    sink: AuditSink,
    /// The receiving half of the very sink the router holds. `emit` is a
    /// synchronous `try_send`, so an event is in this queue by the time the
    /// response has been returned — no sleeping, no polling.
    rx: Receiver<AuditEvent>,
    /// Every client secret this fixture has issued. [`Fixture::drained`] sweeps
    /// every event against all of them, so the no-secret guard is crate-wide
    /// rather than one assertion on one handler.
    secrets: Vec<String>,
}

struct Caller {
    app: axum::Router,
    tok: String,
    user: Uuid,
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>, audit: AuditSink) -> DataApiState {
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
        query: Arc::new(OkPort),
        // The real Postgres key store. Nothing in this file authenticates a
        // machine token, so `active()` is never called and no key is generated.
        keys: Arc::new(egret_machine_idp::PgSigningKeyStore::new(db.clone())),
        issuer: "https://egret.test".into(),
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        audit,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // This file is about the audit trail, not Activity — a fixed retention
        // is enough to satisfy the field.
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
    }
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
         VALUES ($1, 'Data API Audit', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn insert_database(db: &Db, ws: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, delegated, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, false, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

/// Everything the seven audited routes gate on, held by ONE principal so a single
/// caller can drive the whole surface. Permissions are flat, so this is the pair
/// migrations 0120/0122 seed on `ws_admin` for each resource.
const MANAGER: &[&str] = &[
    "service_account:read",
    "service_account:manage",
    "data_api_endpoint:read",
    "data_api_endpoint:manage",
];

async fn principal_with_in(db: &Db, audit: AuditSink, perms: &[&str], ws: Uuid) -> Caller {
    let user = insert_user(db, "audit_caller").await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_audit_{}", Uuid::new_v4().simple()))
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

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp, audit);
    let tok = st
        .sessions
        .create(egret_core::ids::UserId(user), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    let app = axum::Router::new()
        .nest("/api", management_router())
        .with_state(st);
    Caller { app, tok, user }
}

async fn fixture() -> Option<Fixture> {
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    let admin = insert_user(&db, "audit_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "audit").await;
    let database_id = insert_database(&db, ws_id, admin).await;
    // Generous capacity: `emit` DROPS on a full channel rather than blocking, so
    // a tight one would turn "the handler emitted nothing" and "the test never
    // drained fast enough" into the same failure.
    let (sink, rx) = AuditSink::new(256);
    let manager = principal_with_in(&db, sink.clone(), MANAGER, ws_id).await;
    let mut f = Fixture {
        db,
        ws,
        ws_id,
        admin,
        database_id,
        manager,
        sink,
        rx,
        secrets: Vec::new(),
    };
    // Issue one client before any test body runs, and drain its event. Two
    // things follow: every test starts with a LIVE secret registered, so
    // `drained`'s sweep is never vacuous even in the tests that never touch a
    // client; and the sweep is exercised once here on a real issue event, so a
    // sweep that had been broken into a no-op would not sit unnoticed behind
    // tests that only ever assert on endpoints.
    let sa = f.service_account("audit-bootstrap").await;
    f.issue_client(sa).await;
    let bootstrap = f.drained();
    assert_eq!(bootstrap.len(), 1, "bootstrap issue: {bootstrap:?}");
    Some(f)
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

impl Fixture {
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        self.send_as(&self.manager, method, path, body).await
    }

    async fn send_as(
        &self,
        c: &Caller,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("Cookie", format!("egret_session={}", c.tok));
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let res = c
            .app
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

    /// Everything emitted so far. `Decision` events from the auditing PDP are not
    /// possible here — these tests build a bare `CedarPdp`, not the decorated one
    /// — so anything in the queue is a handler's own mutation.
    ///
    /// **Every drained event is swept for every client secret this fixture has
    /// ever seen**, which is the crate-wide half of the no-secret guard. The
    /// named assertions in `issuing_a_client_…` pin `issue_client`'s detail
    /// specifically; this sweep runs on EVERY event of EVERY test, so a future
    /// emit anywhere in the crate that carried secret material trips it without
    /// anyone having to remember to add an assertion. `fixture()` issues a client
    /// before any test body runs precisely so this is never vacuous.
    fn drained(&mut self) -> Vec<Emitted> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AuditEvent::Mutation {
                    actor,
                    workspace_id,
                    action,
                    resource_type,
                    resource_id,
                    detail,
                    ..
                } => out.push(Emitted {
                    actor,
                    workspace_id,
                    action,
                    resource_type,
                    resource_id,
                    detail,
                }),
                other => panic!("unexpected audit event: {other:?}"),
            }
        }
        for ev in &out {
            let rendered = ev.detail.to_string();
            for secret in &self.secrets {
                assert!(
                    !rendered.contains(secret.as_str()),
                    "`{}` carried a live client secret in its audit detail — a secret in an \
                     append-only table outlives every rotation: {}",
                    ev.action,
                    rendered
                );
            }
        }
        out
    }

    /// Issue a client through the route, RECORDING its secret so every later
    /// `drained()` sweeps for it. Does not drain: the caller decides whether the
    /// resulting event is the one under test.
    async fn issue_client(&mut self, sa: Uuid) -> serde_json::Value {
        let (status, created) = self
            .send(
                "POST",
                &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", self.ws),
                Some(serde_json::json!({ "name": "prod" })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "body: {created}");
        let secret = created["client_secret"]
            .as_str()
            .expect("client_secret")
            .to_string();
        assert!(!secret.is_empty(), "the fixture must issue a real secret");
        self.secrets.push(secret);
        created
    }

    /// Exactly one event, or a failure naming what was actually emitted.
    fn one_event(&mut self) -> Emitted {
        let mut evs = self.drained();
        assert_eq!(evs.len(), 1, "expected exactly one audit event: {evs:?}");
        evs.remove(0)
    }

    async fn service_account(&self, name: &str) -> Uuid {
        let sa: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status) \
             VALUES ('service', $1, $2, 'active') RETURNING id",
        )
        .bind(format!(
            "svc.{name}.{}@svc.egret.local",
            Uuid::new_v4().simple()
        ))
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .expect("service account user");
        sqlx::query(
            "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, created_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(sa)
        .bind(self.ws_id)
        .bind(name)
        .bind(self.admin)
        .execute(&self.db.pool)
        .await
        .expect("service_accounts row");
        sa
    }

    async fn endpoint(&self, name: &str, path: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1 AS n')), $5) \
             RETURNING id",
        )
        .bind(self.ws_id)
        .bind(name)
        .bind(format!("/{}", path.trim_start_matches('/')))
        .bind(self.database_id)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft")
    }

    fn endpoints_url(&self) -> String {
        format!("/api/ws/{}/data-api/endpoints", self.ws)
    }
}

impl Emitted {
    /// The event's shape, minus the detail — asserted as a tuple so a wrong
    /// actor, workspace, action, resource type or id all read as one diff.
    ///
    /// **Actor and workspace are in here rather than asserted separately** so
    /// they are pinned for EVERY event, not for the two it occurred to someone
    /// to check. They are what `mutation_for` stamps, and a handler that passed
    /// `None` for the workspace or the service account's id for the actor would
    /// otherwise emit a plausible-looking event that attributes the act to the
    /// wrong principal.
    fn shape(&self) -> (Option<Uuid>, Option<Uuid>, &str, &str, Option<&str>) {
        (
            self.actor,
            self.workspace_id,
            self.action.as_str(),
            self.resource_type.as_str(),
            self.resource_id.as_deref(),
        )
    }
}

// ---------- The one that matters: no secret in the trail ----------

/// **A client secret must never reach `audit_log`.**
///
/// The mutation that proves this test: add `"client_secret": c.client_secret` to
/// the `detail` in `issue_client` and both assertions below go red.
///
/// **The two assertions catch strictly different mutations**, which is why both
/// are here rather than one standing in for the other:
///
/// * `"credential": c.client_secret` — the right bytes under a different name.
///   Caught by the VALUE check (and by [`Fixture::drained`]'s crate-wide sweep,
///   which is the same check applied everywhere); the key check sails past it.
/// * `"client_secret": "redacted"` — the right name over harmless bytes. Caught
///   by the KEY check only; the value check sails past it. A field named
///   `client_secret` carrying a truncation, a prefix or a hash is still a
///   credential-shaped column in an append-only table, and is refused on sight.
///
/// The secret is base64url, whose alphabet contains nothing JSON escapes, so the
/// substring test over the rendered detail is exact rather than approximate.
///
/// This test pins `issue_client` specifically; `Fixture::drained`'s sweep covers
/// every other emit in the crate.
#[tokio::test]
async fn issuing_a_client_audits_the_client_id_and_never_the_secret() {
    let mut f = skip_without_db!();
    let sa = f.service_account("reporting").await;

    let created = f.issue_client(sa).await;
    let secret = created["client_secret"]
        .as_str()
        .expect("client_secret")
        .to_string();
    let client_uuid = created["id"].as_str().expect("id").to_string();

    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_client:issue",
            "data_api_client",
            Some(client_uuid.as_str()),
        ),
        "the actor is the issuing human, never the service account",
    );
    // What SHOULD be there — so a detail that leaked nothing because it carried
    // nothing at all cannot pass the two assertions below.
    assert_eq!(
        ev.detail["client_id"].as_str(),
        created["client_id"].as_str(),
        "the trail must name WHICH credential was minted: {:?}",
        ev.detail
    );
    assert_eq!(
        ev.detail["service_account_id"].as_str(),
        Some(sa.to_string().as_str())
    );

    // 1. No key by that name, anywhere in the detail.
    assert!(
        ev.detail.get("client_secret").is_none(),
        "a client secret in the audit trail outlives every rotation: {:?}",
        ev.detail
    );
    // 2. And the secret's own bytes appear nowhere under any key.
    assert!(
        !ev.detail.to_string().contains(&secret),
        "the secret's VALUE reached the audit detail under some other name: {:?}",
        ev.detail
    );
}

// ---------- The rest of the seven ----------

#[tokio::test]
async fn revoking_a_client_is_audited() {
    let mut f = skip_without_db!();
    let sa = f.service_account("reporting").await;
    let created = f.issue_client(sa).await;
    let client_uuid = created["id"].as_str().expect("id").to_string();
    let _issue = f.one_event();

    let (status, body) = f
        .send(
            "DELETE",
            &format!("/api/ws/{}/data-api/clients/{client_uuid}", f.ws),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_client:revoke",
            "data_api_client",
            Some(client_uuid.as_str()),
        )
    );
    // Deliberately an empty detail: `ClientStore::revoke` is a soft
    // `UPDATE revoked_at`, so the `dataapi.clients` row — with its `client_id`,
    // name and service account — is still there for an auditor to join to. The
    // hard-delete route is the one that cannot rely on that; see
    // `deleting_an_endpoint_records_what_it_was`.
    assert_eq!(ev.detail, serde_json::json!({}));
}

/// Publish, rollback, disable and enable in one sequence, because the
/// interesting part is the VERSION each event names: a rollback that recorded
/// the version it left rather than the one it made live would be worse than no
/// record.
#[tokio::test]
async fn the_publish_lifecycle_is_audited_with_the_version_it_made_live() {
    let mut f = skip_without_db!();
    let ep = f.endpoint("orders", "/orders").await;
    let eps = ep.to_string();
    let url = f.endpoints_url();

    // v1
    let (status, body) = f
        .send(
            "POST",
            &format!("{url}/{ep}/publish"),
            Some(serde_json::json!({ "note": "first" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v1_id = body["version_id"].as_str().expect("version_id").to_string();
    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:publish",
            "data_api_endpoint",
            Some(eps.as_str()),
        )
    );
    assert_eq!(ev.detail["version"], serde_json::json!(1));
    assert_eq!(ev.detail["version_id"].as_str(), Some(v1_id.as_str()));
    assert_eq!(ev.detail["note"], serde_json::json!("first"));

    // v2
    let (status, body) = f
        .send(
            "POST",
            &format!("{url}/{ep}/publish"),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(f.one_event().detail["version"], serde_json::json!(2));

    // …and back to v1.
    let (status, body) = f
        .send(
            "POST",
            &format!("{url}/{ep}/rollback"),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:rollback",
            "data_api_endpoint",
            Some(eps.as_str()),
        )
    );
    assert_eq!(
        ev.detail["version"],
        serde_json::json!(1),
        "the rollback must record the version it made live, not the one it left"
    );
    assert_eq!(ev.detail["version_id"].as_str(), Some(v1_id.as_str()));

    let (status, body) = f.send("POST", &format!("{url}/{ep}/disable"), None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:disable",
            "data_api_endpoint",
            Some(eps.as_str()),
        )
    );
    assert_eq!(ev.detail["state"], serde_json::json!("disabled"));

    // **The other half of the state machine.** Auditing `disable` alone would
    // leave the trail showing an endpoint permanently out of service while it
    // answered requests.
    let (status, body) = f.send("POST", &format!("{url}/{ep}/enable"), None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:enable",
            "data_api_endpoint",
            Some(eps.as_str()),
        )
    );
    assert_eq!(ev.detail["state"], serde_json::json!("live"));
    assert_eq!(
        ev.detail["live_version_id"].as_str(),
        Some(v1_id.as_str()),
        "enable resumes the version that was live before, and must say which"
    );
}

/// **The delete event is the only surviving record of the endpoint.**
///
/// Every other audited route leaves its subject behind: `revoke` is a soft
/// `UPDATE revoked_at`, `disable` keeps the row and its version pointer. This
/// one is a hard `DELETE` and `dataapi.endpoint_versions` cascades with it, so
/// after it runs the `resource_id` uuid resolves to nothing. Without the columns
/// asserted below, an auditor could see that SOMETHING was deleted and never
/// which URL stopped being served.
///
/// Mutation: replace the detail in `delete_endpoint` with `json!({})` — the
/// three assertions on `path`, `method` and `name` go red.
#[tokio::test]
async fn deleting_an_endpoint_records_what_it_was() {
    let mut f = skip_without_db!();
    let ep = f.endpoint("legacy orders", "/legacy/orders").await;
    let eps = ep.to_string();
    let url = f.endpoints_url();

    // Publish first, so the delete really is destroying a version history and
    // not just an unused draft — the case where the record matters most.
    let (status, body) = f
        .send(
            "POST",
            &format!("{url}/{ep}/publish"),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let _publish = f.one_event();

    let (status, body) = f.send("DELETE", &format!("{url}/{ep}"), None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:delete",
            "data_api_endpoint",
            Some(eps.as_str()),
        )
    );
    assert_eq!(
        ev.detail["path"],
        serde_json::json!("/legacy/orders"),
        "the URL that stopped being served is the one thing an auditor needs: {:?}",
        ev.detail
    );
    assert_eq!(ev.detail["method"], serde_json::json!("GET"));
    assert_eq!(ev.detail["name"], serde_json::json!("legacy orders"));
    assert_eq!(
        ev.detail["database_id"].as_str(),
        Some(f.database_id.to_string().as_str())
    );

    // And the row really is gone, so the assertions above are about a record of
    // something destroyed rather than a record beside something still readable.
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoints WHERE id = $1")
        .bind(ep)
        .fetch_one(&f.db.pool)
        .await
        .expect("count endpoints");
    assert_eq!(left, 0);
}

/// **The hardest of the four remaining routes to add later, and the most
/// important to have.** `delete_service_account` hard-deletes a machine
/// principal along with its `access_grants` and every OAuth2 client it owns
/// (the `dataapi.clients`/`service_accounts` cascade off `users`, migration
/// 0119) — so once it returns, the `service_account:delete` event is the
/// ONLY surviving record that the principal ever existed. A bare uuid there
/// would resolve to nothing; `name` and `email` are what let an auditor say
/// which principal was destroyed.
///
/// Also exercises `service_account:create` in passing — the POST that seeds
/// this test's own account would leave a stray event in the queue for the
/// `one_event()` below to trip on if it did not emit exactly once, so the
/// drain of it right after the POST is itself a (loose) check that create
/// emits precisely one event and not zero or two.
///
/// Mutation: replace the detail in `delete_service_account` with `json!({})`
/// — the `name` and `email` assertions below go red.
#[tokio::test]
async fn deleting_a_service_account_records_what_it_destroyed() {
    let mut f = skip_without_db!();

    let (status, created) = f
        .send(
            "POST",
            &format!("/api/ws/{}/data-api/service-accounts", f.ws),
            Some(serde_json::json!({ "name": "doomed", "description": "" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let sa_id = created["id"].as_str().expect("id").to_string();
    let email = created["email"]
        .as_str()
        .expect("the synthetic address identifies it")
        .to_string();
    let create_ev = f.one_event();
    assert_eq!(create_ev.action, "service_account:create");

    let (status, body) = f
        .send(
            "DELETE",
            &format!("/api/ws/{}/data-api/service-accounts/{sa_id}", f.ws),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "service_account:delete",
            "service_account",
            Some(sa_id.as_str()),
        )
    );
    // The row is gone and its grants and clients cascaded with it, so this
    // event is the ONLY surviving record that the principal existed. A bare
    // uuid would say something was deleted without ever saying what.
    assert_eq!(ev.detail["name"], serde_json::json!("doomed"));
    assert_eq!(
        ev.detail["email"],
        serde_json::json!(email),
        "the synthetic address must match what the delete itself returned, \
         not a value fetched by a second, racing query: {:?}",
        ev.detail
    );

    // And the row really is gone, so the event above is a record of something
    // destroyed rather than a record beside something still readable.
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1::uuid")
        .bind(&sa_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count users");
    assert_eq!(left, 0);
}

/// A Try-it trial is an IMPERSONATION — the console runs a query as a service
/// account — so it is audited like one (precedent: user impersonation, #332).
///
/// Two halves, and the second is the reason this test is in THIS file. The
/// event must attribute the act to the human and name the account they borrowed;
/// and it must carry no part of the token minted to make the call. A `data-api`
/// JWT in an append-only table is the same class of mistake as a client secret
/// there — `Fixture::drained` sweeps every event for issued secrets, and this
/// adds the JWT shape, which no secret sweep would catch.
///
/// The trial itself is refused (`403 not_granted`, no invoke grant) and the
/// event records that: what an auditor needs to know is that someone ran an
/// endpoint as somebody else and what came back, not whether it succeeded.
#[tokio::test]
async fn a_trial_is_audited_as_an_impersonation_and_carries_no_token() {
    let mut f = skip_without_db!();
    let sa = f.service_account("trialled").await;
    let ep = f.endpoint("trial-target", "/trial-target").await;
    let eps = ep.to_string();
    let url = f.endpoints_url();

    let (status, body) = f
        .send(
            "POST",
            &format!("{url}/{ep}/publish"),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let _publish = f.one_event();

    let (status, body) = f
        .send(
            "POST",
            &format!("{url}/{ep}/try"),
            Some(serde_json::json!({ "service_account_id": sa, "params": {} })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], 403, "no invoke grant: {body}");

    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:try",
            "data_api_endpoint",
            Some(eps.as_str()),
        ),
        "the actor is the human who ran the trial, never the account it ran as",
    );
    assert_eq!(
        ev.detail["service_account_id"].as_str(),
        Some(sa.to_string().as_str()),
        "whose access was borrowed is the whole question an auditor asks: {:?}",
        ev.detail
    );
    assert_eq!(
        ev.detail["status"],
        serde_json::json!(403),
        "and what the borrowed access got back: {:?}",
        ev.detail
    );
    // A JWT is three base64url segments; every one this deployment signs starts
    // `eyJ` (`{"` under base64). Checked over the WHOLE detail, so a token under
    // any key trips it.
    assert!(
        !ev.detail.to_string().contains("eyJ"),
        "a trial token reached the audit trail: {:?}",
        ev.detail
    );
}

/// **Anti-triviality for every assertion above.** Each of them reads a queue,
/// and a queue that were somehow filled by something other than these handlers
/// would still satisfy `one_event`. A REFUSED call must add nothing: an audit
/// trail that records an attempt with the same shape as a success is worse than
/// one that records neither, because a reader cannot tell them apart.
#[tokio::test]
async fn a_refused_call_emits_nothing() {
    let mut f = skip_without_db!();
    let sa = f.service_account("reporting").await;
    // A real, authenticated workspace member holding NO Data API permission,
    // wired to the SAME sink the assertions drain.
    let nobody = principal_with_in(&f.db, f.sink.clone(), &[], f.ws_id).await;

    let (status, body) = f
        .send_as(
            &nobody,
            "POST",
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
            Some(serde_json::json!({ "name": "sneaky" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");

    let evs = f.drained();
    assert!(
        evs.is_empty(),
        "a refused issue must leave no audit record: {evs:?}"
    );
}
