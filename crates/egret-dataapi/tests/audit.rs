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
//! walk straight past a key-only check.
//!
//! DB-gated. Self-skips (passes) when DATABASE_URL is unset, like every sibling
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
        authz: egret_workspace::rbac::Authorizer::new(db),
        pdp,
        query: Arc::new(OkPort),
        issuer: "https://egret.test".into(),
        audit,
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

/// Everything the six audited routes gate on, held by ONE principal so a single
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
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let admin = insert_user(&db, "audit_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "audit").await;
    let database_id = insert_database(&db, ws_id, admin).await;
    // Generous capacity: `emit` DROPS on a full channel rather than blocking, so
    // a tight one would turn "the handler emitted nothing" and "the test never
    // drained fast enough" into the same failure.
    let (sink, rx) = AuditSink::new(256);
    let manager = principal_with_in(&db, sink.clone(), MANAGER, ws_id).await;
    Some(Fixture {
        db,
        ws,
        ws_id,
        admin,
        database_id,
        manager,
        sink,
        rx,
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
        out
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
               (workspace_id, name, path, method, database_id, sql, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, 'SELECT 1 AS n', $5) RETURNING id",
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
    /// action, a wrong resource type and a wrong id all read as one diff.
    fn shape(&self) -> (&str, &str, Option<&str>) {
        (
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
/// The value check is not redundant with the key check. A future edit that
/// renamed the field — `secret`, `credential`, or folded it into a "created"
/// blob — would pass a key-only assertion while writing exactly the same bytes
/// into an append-only table.
#[tokio::test]
async fn issuing_a_client_audits_the_client_id_and_never_the_secret() {
    let mut f = skip_without_db!();
    let sa = f.service_account("reporting").await;

    let (status, created) = f
        .send(
            "POST",
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
            Some(serde_json::json!({ "name": "prod" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let secret = created["client_secret"].as_str().expect("client_secret");
    assert!(!secret.is_empty(), "the fixture must issue a real secret");
    let client_uuid = created["id"].as_str().expect("id");

    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            "data_api_client:issue",
            "data_api_client",
            Some(client_uuid)
        )
    );
    assert_eq!(
        ev.actor,
        Some(f.manager.user),
        "the issuing human, not the SA"
    );
    assert_eq!(ev.workspace_id, Some(f.ws_id));
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
        !ev.detail.to_string().contains(secret),
        "the secret's VALUE reached the audit detail under some other name: {:?}",
        ev.detail
    );
}

// ---------- The rest of the six ----------

#[tokio::test]
async fn revoking_a_client_is_audited() {
    let mut f = skip_without_db!();
    let sa = f.service_account("reporting").await;
    let (status, created) = f
        .send(
            "POST",
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
            Some(serde_json::json!({ "name": "prod" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
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
            "data_api_client:revoke",
            "data_api_client",
            Some(client_uuid.as_str())
        )
    );
    assert_eq!(ev.actor, Some(f.manager.user));
    assert_eq!(ev.workspace_id, Some(f.ws_id));
}

/// Publish, rollback and disable in one sequence, because the interesting part
/// is the VERSION each event names: a rollback that recorded the version it left
/// rather than the one it made live would be worse than no record.
#[tokio::test]
async fn the_publish_lifecycle_is_audited_with_the_version_it_made_live() {
    let mut f = skip_without_db!();
    let ep = f.endpoint("orders", "/orders").await;
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
            "data_api_endpoint:publish",
            "data_api_endpoint",
            Some(ep.to_string().as_str())
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
            "data_api_endpoint:rollback",
            "data_api_endpoint",
            Some(ep.to_string().as_str())
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
            "data_api_endpoint:disable",
            "data_api_endpoint",
            Some(ep.to_string().as_str())
        )
    );
    assert_eq!(ev.detail["state"], serde_json::json!("disabled"));

    let (status, body) = f.send("DELETE", &format!("{url}/{ep}"), None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            "data_api_endpoint:delete",
            "data_api_endpoint",
            Some(ep.to_string().as_str())
        )
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
