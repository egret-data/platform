//! The per-workspace feature gate, proven over a real router.
//!
//! The gate lives in `WsContext`, so any workspace-scoped router exercises it;
//! notebooks are used here because this crate already has the fixture.
//!
//! The app below is composed exactly the way `egret-server` composes the real
//! one — `nest("/api", notebook_router())` alongside
//! `nest("/api/ws", ws_member_router())` — so the gate is exercised against the
//! mount shapes production actually produces, plus one that it does not yet
//! (see `a_deeper_mount_prefix_does_not_open_the_gate`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_notebook::{notebook_router, NotebookState, NotebookStore};
use egret_workspace::context::WsContext;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{ws_member_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

/// Returns a canned engine JWT (no IdP in tests).
struct FakeEngineMinter;

#[async_trait::async_trait]
impl egret_notebook::engine_token::EngineTokenMinter for FakeEngineMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<egret_notebook::engine_token::EngineToken, egret_notebook::engine_token::BrokerError>
    {
        Ok(egret_notebook::engine_token::EngineToken {
            access_token: "fake.engine.jwt".into(),
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(4102444800, 0).unwrap(),
        })
    }
}

/// Who makes the requests. The gate must answer identically for all three.
#[derive(Clone, Copy)]
enum Caller {
    /// A `ws_admin` member — holds every notebook permission, so a 403 from a
    /// request of theirs can only have come from the gate.
    WorkspaceAdmin,
    /// A `platform_admin` with no membership. Bypasses permission checks
    /// everywhere else in egret; must NOT bypass an entitlement.
    PlatformAdmin,
    /// Neither a member nor a platform admin.
    NonMember,
}

struct Fixture {
    app: Router,
    db: Db,
    slug: String,
    ws: WorkspaceId,
    token: String,
}

impl Fixture {
    async fn send(&self, method: &str, uri: &str, body: Option<&str>) -> Response {
        let mut b = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("Cookie", format!("egret_session={}", self.token));
        let body = match body {
            Some(json) => {
                b = b.header("Content-Type", "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        self.app
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn get(&self, uri: &str) -> Response {
        self.send("GET", uri, None).await
    }

    async fn post(&self, uri: &str, body: &str) -> Response {
        self.send("POST", uri, Some(body)).await
    }

    async fn json(&self, resp: Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(
            |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
        )
    }
}

/// A workspace-scoped route shaped like `ws_member_router`'s: its own path
/// carries no `/ws` literal, because it is meant to be mounted at `/api/ws`.
/// The handler does nothing but prove the extractor let the request through.
///
/// Nothing gated is mounted that way in production today, which is exactly why
/// this exists: the gate must not depend on a router happening to be nested at
/// the one depth that leaves `/ws/{slug}/…` intact in `parts.uri`.
async fn deep_probe(_ctx: WsContext) -> StatusCode {
    StatusCode::OK
}

/// `kernel` is one of the `notebooks` module's gated prefixes, and no router in
/// this test app already claims `/api/ws/{workspace}/kernel`, so the probe can
/// occupy it without colliding with the real notebook routes.
fn deep_probe_router() -> Router<WorkspaceState> {
    Router::new().route("/{workspace}/kernel", get(deep_probe))
}

async fn build_pdp(db: &Db) -> Arc<dyn egret_core::authz::Pdp> {
    Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn setup(pool: sqlx::PgPool, caller: Caller) -> Fixture {
    let db = Db { pool };
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let authz = Authorizer::new(db.clone());
    let members = MemberRepo::new(db.clone());
    let workspaces = WorkspaceRepo::new(db.clone());

    let uniq = Uuid::new_v4().simple().to_string();
    let owner = users
        .insert_local(&format!("fg-owner-{uniq}@egret.test"), "Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("fg{uniq}");
    let ws = workspaces
        .create(&slug, "Feature gate ws", owner)
        .await
        .expect("seed workspace")
        .id;

    let caller_id: UserId = users
        .insert_local(&format!("fg-caller-{uniq}@egret.test"), "Caller", "x")
        .await
        .expect("seed caller");
    match caller {
        Caller::WorkspaceAdmin => {
            members
                .add(ws, "user", caller_id.0, "ws_admin", owner)
                .await
                .expect("seed membership");
        }
        Caller::PlatformAdmin => {
            authz
                .replace_platform_roles(caller_id, &["platform_admin".to_string()])
                .await
                .expect("grant platform_admin");
        }
        Caller::NonMember => {}
    }
    let token = sessions
        .create(caller_id, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let pdp = build_pdp(&db).await;

    // Mirrors `egret_server::build_router`: two independently-stated groups,
    // each nested at its production prefix, merged into one app.
    let notebooks = Router::new()
        .nest("/api", notebook_router())
        .with_state(NotebookState {
            store: NotebookStore::new(db.clone()),
            catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
            sessions: sessions.clone(),
            workspaces: workspaces.clone(),
            authz: authz.clone(),
            pdp: pdp.clone(),
            users: UserRepo::new(db.clone()),
            catalog_base_url: "http://localhost:8181".into(),
            engine_minter: Arc::new(FakeEngineMinter),
            keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
            idp_issuer: "http://localhost:8080".into(),
        });
    let workspace_admin = Router::new()
        .nest("/api/ws", ws_member_router().merge(deep_probe_router()))
        .with_state(WorkspaceState {
            workspaces,
            members: MemberRepo::new(db.clone()),
            access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
            access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
                db.clone(),
                MemberRepo::new(db.clone()),
            ),
            authz,
            pdp,
            users: UserRepo::new(db.clone()),
            sessions,
            audit: egret_audit::AuditSink::new(16).0,
            impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
            oidc: None,
            cookie_key: None,
            session_secure: false,
        });

    Fixture {
        app: notebooks.merge(workspace_admin),
        db,
        slug,
        ws,
        token,
    }
}

/// Turn `feature` OFF for `ws` by writing the row the platform-admin API will
/// write. An absent row means enabled, so this is the only way to say "off".
async fn disable(db: &Db, ws: WorkspaceId, feature: &str) {
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, $2, false)",
    )
    .bind(ws.0)
    .bind(feature)
    .execute(&db.pool)
    .await
    .expect("disable feature");
}

#[sqlx::test(migrations = "../../migrations")]
async fn disabled_module_returns_403_with_the_feature_disabled_code(pool: sqlx::PgPool) {
    let f = setup(pool, Caller::WorkspaceAdmin).await;
    disable(&f.db, f.ws, "notebooks").await;

    let resp = f.get(&format!("/api/ws/{}/notebooks", f.slug)).await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = f.json(resp).await;
    // The `code` is what proves this 403 came from the gate and not from an
    // unrelated authorization denial: no other error carries it.
    assert_eq!(json["code"], "feature_disabled", "body={json}");
    assert_eq!(json["error"], "feature disabled: notebooks", "body={json}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_ungated_surface_survives_the_module_being_off(pool: sqlx::PgPool) {
    let f = setup(pool, Caller::WorkspaceAdmin).await;
    disable(&f.db, f.ws, "notebooks").await;

    // Same workspace, an ungated surface: the gate must not leak across.
    // `members` is workspace administration — it belongs to no module and so
    // must survive every module being switched off. The gate does parse this
    // path as workspace-scoped; it lets it through because no `GateRule` claims
    // `members`, which is also what makes it skip the DB lookup. (`UNGATED` in
    // the catalog is documentation for the CI coverage test, not a runtime
    // allowlist — nothing consults it on this path.)
    let resp = f.get(&format!("/api/ws/{}/members", f.slug)).await;

    assert_eq!(resp.status(), StatusCode::OK);
}

/// One module being off must not gate another. Without this, an implementation
/// that ignored the feature key entirely — "any disabled module gates
/// everything" — would satisfy every other test in this file.
#[sqlx::test(migrations = "../../migrations")]
async fn disabling_one_module_does_not_gate_another(pool: sqlx::PgPool) {
    let f = setup(pool, Caller::WorkspaceAdmin).await;
    disable(&f.db, f.ws, "charts").await;

    let resp = f.get(&format!("/api/ws/{}/notebooks", f.slug)).await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "notebooks is on; disabling charts must not touch it"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn re_enabling_restores_access_and_preserves_content(pool: sqlx::PgPool) {
    let f = setup(pool, Caller::WorkspaceAdmin).await;
    let created = f
        .post(&format!("/api/ws/{}/notebooks", f.slug), r#"{"name":"nb"}"#)
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    disable(&f.db, f.ws, "notebooks").await;
    assert_eq!(
        f.get(&format!("/api/ws/{}/notebooks", f.slug))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    sqlx::query("UPDATE workspace_features SET enabled = true WHERE workspace_id = $1")
        .bind(f.ws.0)
        .execute(&f.db.pool)
        .await
        .unwrap();

    let back = f.get(&format!("/api/ws/{}/notebooks", f.slug)).await;
    assert_eq!(back.status(), StatusCode::OK);
    let json = f.json(back).await;
    assert_eq!(
        json.as_array().map(|a| a.len()),
        Some(1),
        "disabling a module must not delete its content; body={json}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn platform_admin_does_not_bypass_the_gate(pool: sqlx::PgPool) {
    let f = setup(pool, Caller::PlatformAdmin).await;

    // Control: without the entitlement row this same caller is allowed through.
    assert_eq!(
        f.get(&format!("/api/ws/{}/notebooks", f.slug))
            .await
            .status(),
        StatusCode::OK,
        "a platform_admin reads notebooks while the module is on"
    );

    disable(&f.db, f.ws, "notebooks").await;
    let resp = f.get(&format!("/api/ws/{}/notebooks", f.slug)).await;

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "entitlement is a property of the workspace, not of the caller"
    );
    assert_eq!(f.json(resp).await["code"], "feature_disabled");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_gate_precedes_membership_resolution(pool: sqlx::PgPool) {
    // A non-member of a workspace with notebooks off must get the same 403 as a
    // member, so the response leaks nothing about membership.
    let f = setup(pool, Caller::NonMember).await;

    // Control: with the module ON this caller is denied for a DIFFERENT reason
    // (no membership) and the body carries no `code`. That is what makes the
    // assertion below evidence of ordering rather than a coincidence.
    let before = f.get(&format!("/api/ws/{}/notebooks", f.slug)).await;
    assert_eq!(before.status(), StatusCode::FORBIDDEN);
    assert!(
        f.json(before).await["code"].is_null(),
        "membership denial must not carry the feature_disabled code"
    );

    disable(&f.db, f.ws, "notebooks").await;

    let resp = f.get(&format!("/api/ws/{}/notebooks", f.slug)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.json(resp).await["code"], "feature_disabled");
}

/// The gate must key off the path the CLIENT sent, not the one `nest` left
/// behind. `deep_probe_router` is mounted the way `ws_member_router` is — at
/// `/api/ws`, with its own paths starting at `/{workspace}` — so `parts.uri`
/// arrives here as `/{slug}/kernel`, with the `/ws/` marker eaten by the mount
/// prefix. Reading `parts.uri` the gate sees nothing workspace-scoped and fails
/// OPEN; reading `OriginalUri` it sees `/api/ws/{slug}/kernel` — a path the
/// `notebooks` module claims — and gates correctly.
///
/// Nothing gated is mounted at that depth in production today, so this is the
/// regression test for a fail-open that a future contributor would otherwise
/// introduce silently — the catalog and its coverage test would both still
/// look correct.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deeper_mount_prefix_does_not_open_the_gate(pool: sqlx::PgPool) {
    let f = setup(pool, Caller::WorkspaceAdmin).await;

    // Control: the probe is reachable while the module is on, so a 403 below
    // can only be the gate and not a routing or membership accident.
    assert_eq!(
        f.get(&format!("/api/ws/{}/kernel", f.slug)).await.status(),
        StatusCode::OK,
        "the probe route is reachable while notebooks is on"
    );

    disable(&f.db, f.ws, "notebooks").await;

    let resp = f.get(&format!("/api/ws/{}/kernel", f.slug)).await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a gated module must stay gated however deeply its router is nested"
    );
    assert_eq!(f.json(resp).await["code"], "feature_disabled");
}

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
