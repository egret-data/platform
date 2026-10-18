//! Control-plane Storage + Secrets HTTP route tests (platform-admin gate).
//! DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Proves `egret_controlplane::control_plane_router`:
//!   - a non-platform-admin session is Forbidden (403) on the global routes,
//!   - a `platform_admin` session can list storage (200), create a global named
//!     secret (201) and create a global storage connection (201).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-controlplane

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{control_plane_router, CpState};
use egret_core::UserId;
use egret_metastore::Db;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
    /// Receiver end of the audit sink wired into `CpState.audit`. Kept alive so
    /// emitted `Mutation` events aren't dropped; the mutation-emit test drains it.
    audit_rx: tokio::sync::mpsc::Receiver<egret_audit::AuditEvent>,
}

/// Incidental PDP for `CpState`: the storage + access route tests gate on
/// `require_platform_admin` / `require_permission` (session-derived), NOT on
/// `state.pdp` — those routes don't consult the PDP — so their 403 assertions
/// are independent of it. An allow-all stub keeps those fixtures light.
///
/// The `secrets` routes DO consult `state.pdp` (B1b-1 Task 4), so their tests
/// build a real `CedarPdp` via `attach_real_pdp` instead.
struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

/// Build the control-plane `Router` for a given db + repos + PDP. Shared by
/// `setup` (allow-all PDP) and `attach_real_pdp` (real Cedar PDP) so the two
/// only differ in the injected decision point.
fn build_app(
    db: &Db,
    users: &UserRepo,
    sessions: &SessionService,
    pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    audit: egret_audit::AuditSink,
) -> axum::Router {
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);

    control_plane_router().with_state(CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        browser: None,
        pdp,
        audit,
        audit_store: egret_audit::AuditStore::new(db.clone()),
        tags: egret_bi::TagRepo::new(db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            db.clone(),
            egret_events::EventBus::new(),
        ),
        db: db.clone(),
        mv_lister: std::sync::Arc::new(NoListing),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: std::sync::Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: std::sync::Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: std::sync::Arc::new(egret_controlplane::UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
    })
}

/// Rebuild `f.app` with a REAL `CedarPdp`. The PDP snapshots the
/// `roles`/`role_permissions` catalog at build time and (in these tests) never
/// reloads — raw-SQL seeding emits no `RolesChanged` event — so callers MUST
/// seed any ad-hoc roles the PDP needs to know about BEFORE calling this. The
/// secrets routes consult `state.pdp`, so their tests use this in place of the
/// allow-all stub, matching pre-migration `require_permission` outcomes.
async fn attach_real_pdp(f: &mut Fixture) {
    let pdp = std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    f.app = build_app(
        &f.db,
        &f.users,
        &f.sessions,
        pdp,
        egret_audit::AuditSink::new(16).0,
    );
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let (audit, audit_rx) = egret_audit::AuditSink::new(16);
    let app = build_app(
        &db,
        &users,
        &sessions,
        std::sync::Arc::new(AllowAllPdp),
        audit,
    );

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        audit_rx,
    })
}

/// Create a user (optionally a platform_admin) and return its session token.
async fn session_for(f: &Fixture, tag: &str, platform_admin: bool) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    if platform_admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("grant platform_admin");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Create a NON-admin user holding exactly one fine-grained permission key
/// (via a fresh ad-hoc platform-scoped role), and return its session token.
/// Exercises the `require_permission` scoped-principal path (as opposed to
/// `session_for`'s `platform_admin` bypass path): `platform_roles` stays
/// empty, `permissions` carries only `permission`.
async fn session_for_with_permission(f: &Fixture, tag: &str, permission: &str) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");

    let role_name = format!("test_role_{}", Uuid::new_v4().simple());
    let (role_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') \
         RETURNING id",
    )
    .bind(&role_name)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert role");

    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
        .bind(role_id)
        .bind(permission)
        .execute(&f.db.pool)
        .await
        .expect("grant permission");

    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(uid.0)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("assign role");

    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

/// Regression: `control_plane_router()` is nested at `/api/control-plane` in
/// `egret-server`, so every route inside it MUST be relative to that nest.
/// `audit_router` once used `/control-plane/audit`, which double-prefixed to
/// `/api/control-plane/control-plane/audit` and 404'd the FE (it calls
/// `/api/control-plane/audit`). `build_app` mounts the router BARE, so it can't
/// catch this — mount it exactly as production does and assert the audit path
/// resolves (a doubled route would 404 here).
#[tokio::test]
async fn audit_route_resolves_under_the_control_plane_nest() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };
    let nested = axum::Router::new().nest("/api/control-plane", f.app.clone());
    let resp = nested
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/api/control-plane/audit?limit=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // No session cookie → 401 (route exists, auth required). A double-prefixed
    // route would 404 here — the invariant is: NOT 404.
    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "audit must resolve at /api/control-plane/audit (route must be relative to the nest)"
    );
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_admin_is_forbidden() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // The `/secrets` gate consults `state.pdp`; use a real Cedar PDP so a
    // grant-less principal is genuinely denied (an allow-all stub would 201).
    attach_real_pdp(&mut f).await;
    let token = session_for(&f, "plain", false).await;

    // GET /storage → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not list storage"
    );

    // POST /secrets → 403 (gate runs before persistence).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/secrets",
            &token,
            Some(serde_json::json!({ "name": "nope", "value": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not create secrets"
    );
}

#[tokio::test]
async fn platform_admin_can_manage_storage_and_secrets() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // The `/secrets` gate consults `state.pdp`; a real Cedar PDP proves the
    // platform_admin bypass (not the allow-all stub) grants `secret:create`.
    attach_real_pdp(&mut f).await;
    let token = session_for(&f, "admin", true).await;
    let suffix = Uuid::new_v4().simple().to_string();

    // GET /storage → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list storage; body={json}");
    assert!(json.is_array(), "storage list is an array; body={json}");

    // POST /secrets → 201, global named secret.
    let secret_name = format!("cp-secret-{suffix}");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/secrets",
            &token,
            Some(serde_json::json!({ "name": secret_name, "value": "s3cr3t" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create secret; body={json}");
    let secret_id = json["id"].as_str().expect("secret id in body").to_string();
    assert_eq!(json["name"], serde_json::json!(secret_name));

    // POST /storage → 201, global storage connection referencing the secret.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/storage",
            &token,
            Some(serde_json::json!({
                "name": format!("cp-store-{suffix}"),
                "provider": "s3",
                "region": "us-east-1",
                "bucket": "egret-cp-test",
                "access_key": "AKIATEST",
                "secret_ref": secret_id,
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create storage; body={json}");
    assert_eq!(json["bucket"], serde_json::json!("egret-cp-test"));
    assert_eq!(json["secret_ref"], serde_json::json!(secret_id));
}

/// Storage: `storage:read` gates `GET /storage`, `storage:create` gates
/// `POST /storage`. A principal with only one of the two must not get the other.
#[tokio::test]
async fn scoped_permission_storage_read_and_create() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The `/storage` gate consults `state.pdp`; seed the ad-hoc roles BEFORE
    // building the PDP (the Cedar PDP snapshots the roles catalog at build time)
    // then swap in a real Cedar PDP so an allow-all stub can't mask the scoped
    // grants.
    let wrong = session_for_with_permission(&f, "storage_scoped_wrong", "secret:read").await;
    let reader = session_for_with_permission(&f, "storage_scoped_reader", "storage:read").await;
    let creator = session_for_with_permission(&f, "storage_scoped_creator", "storage:create").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without storage:read must not list storage"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped storage:read principal must list storage; body={body}"
    );

    // storage:read alone must not create.
    let suffix = Uuid::new_v4().simple().to_string();
    let create_body = serde_json::json!({
        "name": format!("scoped-store-{suffix}"),
        "provider": "s3",
        "region": "us-east-1",
        "bucket": "egret-scoped-test",
        "access_key": "AKIATEST",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/storage", &reader, Some(create_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "storage:read alone must not create a storage connection"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/storage", &creator, Some(create_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped storage:create principal must create; body={body}"
    );
}

/// Secrets: `secret:read` gates `GET /secrets`, `secret:create` gates
/// `POST /secrets`.
#[tokio::test]
async fn scoped_permission_secrets_read_and_create() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // Seed the ad-hoc roles BEFORE building the PDP: the `/secrets` gate consults
    // `state.pdp`, and the Cedar PDP snapshots the roles catalog at build time.
    let wrong = session_for_with_permission(&f, "secret_scoped_wrong", "storage:read").await;
    let reader = session_for_with_permission(&f, "secret_scoped_reader", "secret:read").await;
    let creator = session_for_with_permission(&f, "secret_scoped_creator", "secret:create").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/secrets", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without secret:read must not list secrets"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/secrets", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped secret:read principal must list secrets; body={body}"
    );

    // secret:read alone must not create.
    let suffix = Uuid::new_v4().simple().to_string();
    let create_body = serde_json::json!({
        "name": format!("scoped-secret-{suffix}"),
        "value": "s3cr3t",
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/secrets", &reader, Some(create_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "secret:read alone must not create a secret"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/secrets", &creator, Some(create_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped secret:create principal must create; body={body}"
    );
}

/// Proves the `/secrets` gate now flows through `state.pdp` (B1b-1 Task 4): a
/// non-`platform_admin` principal granted `secret:read` through a platform-scoped
/// role — resolved by the real `CedarPdp`, not the allow-all stub — reads
/// (200 on GET) but cannot create (403 on POST). If the handler still short-
/// circuited on the session-derived `require_permission`, this would still pass;
/// what makes it a PDP proof is that the decision comes from a `CedarPdp` built
/// over `access_grants` (the platform-scope grant seeded directly).
#[tokio::test]
async fn pdp_backed_secret_reader_reads_but_cannot_create() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // Seed the reader's platform role FIRST, then snapshot it into the PDP.
    let reader = session_for_with_permission(&f, "pdp_secret_reader", "secret:read").await;
    attach_real_pdp(&mut f).await;

    // GET /secrets → 200 (the PDP allows `secret:read` at platform scope).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/secrets", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PDP-backed secret:read must list secrets; body={body}"
    );

    // POST /secrets → 403 (the same principal lacks `secret:create`).
    let create_body = serde_json::json!({
        "name": format!("pdp-secret-{}", Uuid::new_v4().simple()),
        "value": "s3cr3t",
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/secrets", &reader, Some(create_body)))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "PDP-backed secret:read alone must not create a secret"
    );
}

// ── Access (Users / Groups / Roles / Permissions) ─────────────────────────────

/// Build a `Fixture` backed by a dedicated, freshly-migrated Postgres DATABASE
/// (not the shared one every other test in this file uses).
///
/// `last_admin_guard` needs this: `guard_last_admin` blocks on
/// `RoleRepo::count_platform_admins()` being GLOBALLY exactly 1, but dozens of
/// other `#[tokio::test]`s in this file call `session_for(.., true)`, each
/// inserting its own `platform_admin` assignment into the shared database, and
/// tests run concurrently — so the ambient count there is never reliably 1. A
/// private database (starting at 0 assignments) sidesteps the race instead of
/// asserting on an unstable global.
///
/// Isolation is by **database**, not by `search_path` schema: the seam
/// migrations (0062-0066) do `ALTER TABLE … SET SCHEMA <global>` into shared
/// schemas (`engine_mgmt`/`policy`/`storage`/`catalog`/`query`) whose names a
/// `search_path` pin cannot rename, so a schema-isolated pool collides with the
/// shared-database tests' already-moved tables ("relation … already exists in
/// schema …"). A per-database pool gives every schema — public AND the seam
/// schemas — its own namespace, so `migrate()` runs cleanly. `iso_db` names the
/// database, DROP+CREATE'd each run (drop stale backends first), so temp
/// databases don't accumulate; callers pass a name unique to their test so two
/// isolated-schema tests running concurrently don't race each other's
/// DROP/CREATE.
async fn setup_isolated_schema(iso_db: &str) -> Option<Fixture> {
    use sqlx::{ConnectOptions, Connection};
    use std::str::FromStr;

    let dsn = egret_test_db::url().await?;
    let base_opts = sqlx::postgres::PgConnectOptions::from_str(&dsn).expect("parse DATABASE_URL");

    // On a maintenance connection (the DATABASE_URL's own db), drop any stale
    // backends holding the isolated db open, then DROP+CREATE it fresh. (CREATE/
    // DROP DATABASE cannot run inside a transaction; `sqlx::query` runs the
    // statement directly, so no wrapper is needed.)
    //
    // AssertSqlSafe below: a database name cannot be a bind parameter, so it has
    // to be spliced. Every caller of this function passes a hard-coded literal.
    let mut admin = base_opts
        .clone()
        .connect()
        .await
        .expect("connect for isolated-db setup");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = '{iso_db}' AND pid <> pg_backend_pid()"
    )))
    .execute(&mut admin)
    .await
    .ok();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {iso_db}"
    )))
    .execute(&mut admin)
    .await
    .expect("drop isolated db");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {iso_db}")))
        .execute(&mut admin)
        .await
        .expect("create isolated db");
    admin.close().await.ok();

    // Connect the fixture pool to the fresh, empty database and migrate it. Every
    // schema (public + the seam schemas) is private to this db, so the seam
    // `SET SCHEMA` migrations apply without colliding.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect_with(base_opts.database(iso_db))
        .await
        .expect("connect isolated db pool");

    let db = Db { pool };
    db.migrate().await.expect("migrate isolated db");

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let (audit, audit_rx) = egret_audit::AuditSink::new(16);
    let app = control_plane_router().with_state(CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        browser: None,
        pdp: std::sync::Arc::new(AllowAllPdp),
        audit,
        audit_store: egret_audit::AuditStore::new(db.clone()),
        tags: egret_bi::TagRepo::new(db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            db.clone(),
            egret_events::EventBus::new(),
        ),
        db: db.clone(),
        mv_lister: std::sync::Arc::new(NoListing),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: std::sync::Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: std::sync::Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: std::sync::Arc::new(egret_controlplane::UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        audit_rx,
    })
}

/// `POST /users` creates a local user, `GET /users` lists it (with its
/// assigned role), and `POST`/`DELETE .../roles(/{roleId})` assign/unassign a
/// custom (non-admin) role — so no `guard_last_admin` guard is in play here.
#[tokio::test]
async fn users_crud_and_role_assign() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "users_admin", true).await;
    let email = format!("cruduser_{}@egret.local", Uuid::new_v4().simple());

    // POST /users → 201, a local user.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/users",
            &admin,
            Some(serde_json::json!({
                "email": email,
                "name": "Crud User",
                "password": "S3curePassw0rd!",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create user; body={j}");
    assert_eq!(j["email"], serde_json::json!(email), "user email; body={j}");
    assert_eq!(j["provider"], "local", "user provider; body={j}");
    let user_id = j["id"].as_str().expect("user id in body").to_string();

    // GET /users → the new user shows up.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/users", &admin, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list users; body={j}");
    assert!(
        j.as_array()
            .expect("users array")
            .iter()
            .any(|u| u["id"] == user_id),
        "new user must be listed; body={j}"
    );

    // Create a custom (non-admin) role to assign/unassign.
    let role_name = format!("crud_role_{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "platform",
                "name": role_name,
                "description": "crud test role",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create role; body={j}");
    let role_id = j["id"].as_str().expect("role id in body").to_string();

    // Assign the role to the new user.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{user_id}/roles"),
            &admin,
            Some(serde_json::json!({ "roleId": role_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "assign role to user");

    // GET /users → the user's roles now include it.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/users", &admin, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list users after assign; body={j}");
    let entry = j
        .as_array()
        .expect("users array")
        .iter()
        .find(|u| u["id"] == user_id)
        .expect("assigned user still listed");
    assert!(
        entry["roles"]
            .as_array()
            .expect("roles array")
            .contains(&serde_json::json!(role_name)),
        "assigned role must show up on the user; entry={entry}"
    );

    // Unassign — a non-admin role, so `guard_last_admin` never fires.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{user_id}/roles/{role_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "unassign a non-admin role"
    );
}

/// A successful user role `assign` emits an `AuditEvent::Mutation` on the sink
/// wired into `CpState.audit` (B4): `action="role:assign"`, actor = the calling
/// principal, and `detail` carries the user + role. We keep the sink's receiver
/// alive on the fixture so the best-effort event isn't dropped.
#[tokio::test]
async fn role_assign_emits_audit_mutation() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "audit_admin", true).await;
    let email = format!("audituser_{}@egret.local", Uuid::new_v4().simple());

    // Create a user to assign the role to.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/users",
            &admin,
            Some(serde_json::json!({
                "email": email,
                "name": "Audit User",
                "password": "S3curePassw0rd!",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create user; body={j}");
    let user_id = j["id"].as_str().expect("user id in body").to_string();

    // Create a custom platform role.
    let role_name = format!("audit_role_{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "platform",
                "name": role_name,
                "description": "audit test role",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create role; body={j}");
    let role_id = j["id"].as_str().expect("role id in body").to_string();

    // Assign the role — this is the mutation that must emit an audit event.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{user_id}/roles"),
            &admin,
            Some(serde_json::json!({ "roleId": role_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "assign role to user");

    // The emit is best-effort try_send; with the receiver held it lands in the
    // channel synchronously during the handler, so it's available immediately.
    let ev = f
        .audit_rx
        .try_recv()
        .expect("a Mutation event must be queued after a successful assign");
    match ev {
        egret_audit::AuditEvent::Mutation {
            action,
            resource_type,
            resource_id,
            detail,
            ..
        } => {
            assert_eq!(action, "role:assign", "action");
            assert_eq!(resource_type, "grant", "resource_type");
            assert_eq!(
                resource_id.as_deref(),
                Some(role_id.as_str()),
                "resource_id is the role id"
            );
            assert_eq!(detail["principal_type"], "user", "detail.principal_type");
            assert_eq!(
                detail["principal_id"],
                serde_json::json!(user_id),
                "detail.principal_id is the target user"
            );
            assert_eq!(
                detail["role_id"],
                serde_json::json!(role_id),
                "detail.role_id"
            );
        }
        other => panic!("expected a Mutation event, got {other:?}"),
    }
}

/// `guard_last_admin` (shared by the user- and group-role unassign routes)
/// blocks removing the platform-wide LAST `platform_admin` assignment — 409,
/// not 200/204. Run against `setup_isolated_schema` (see its docstring for
/// why the shared-schema fixture can't give this a deterministic count).
#[tokio::test]
async fn last_admin_guard() {
    let Some(f) = setup_isolated_schema("egret_test_iso_last_admin").await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The lone platform_admin in this isolated schema.
    let token = session_for(&f, "solo_admin", true).await;
    let (admin_id, role_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT pra.principal_id, pra.role_id FROM access_grants pra \
         JOIN roles r ON r.id = pra.role_id \
         WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform'",
    )
    .fetch_one(&f.db.pool)
    .await
    .expect("the sole platform_admin assignment");

    // Removing the only platform_admin assignment must 409, not succeed.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}/roles/{role_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "removing the last platform_admin must 409; body={j}"
    );
    assert!(
        j["error"]
            .as_str()
            .unwrap_or_default()
            .contains("last platform admin"),
        "409 body should explain the last-admin guard; body={j}"
    );

    // A second admin breaks the tie: the same removal now succeeds.
    let _second_admin_token = session_for(&f, "second_admin", true).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}/roles/{role_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "removal succeeds once a second platform_admin exists"
    );
}

/// Local groups support create/add-member/delete; an OIDC-synced group
/// (`upsert_oidc_group`) cannot be deleted (409) — mirrors the OIDC rename
/// guard on the same `groups` table (`rows == 0` ⇒ `CpError::Conflict`).
#[tokio::test]
async fn groups_local_and_oidc() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "groups_admin", true).await;
    let group_name = format!("crud-group-{}", Uuid::new_v4().simple());

    // POST /groups → 201, a local group.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/groups",
            &admin,
            Some(serde_json::json!({ "name": group_name })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create local group; body={j}");
    assert_eq!(j["source"], "local", "group source; body={j}");
    let group_id = j["id"].as_str().expect("group id in body").to_string();

    // Add a member.
    let member_email = format!("groupmember_{}@egret.local", Uuid::new_v4().simple());
    let member_id = f
        .users
        .insert_local(&member_email, "Group Member", "x")
        .await
        .expect("insert member")
        .0;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/groups/{group_id}/members"),
            &admin,
            Some(serde_json::json!({ "userId": member_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "add group member");

    // GET /groups/{id}/members lists the member back (prefill for the
    // manage-members dialog).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/groups/{group_id}/members"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list group members; body={j}");
    let members = j.as_array().expect("members array");
    assert_eq!(members.len(), 1, "one member; body={j}");
    assert_eq!(
        members[0]["id"],
        member_id.to_string(),
        "member id; body={j}"
    );
    assert_eq!(members[0]["email"], member_email, "member email; body={j}");

    // DELETE the local group → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/groups/{group_id}"), &admin, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete local group");

    // An OIDC-synced group cannot be deleted through the admin route.
    let oidc_name = format!("oidc-group-{}", Uuid::new_v4().simple());
    let oidc_repo = egret_auth::groups::GroupRepo::new(f.db.clone());
    let oidc_group_id = oidc_repo
        .upsert_oidc_group(&oidc_name)
        .await
        .expect("upsert oidc group");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/groups/{oidc_group_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "deleting an oidc-synced group must 409; body={j}"
    );

    // Design R4: nor can an admin add a member to it — the IdP owns that
    // membership. The 409 carries the reason the console shows.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/groups/{oidc_group_id}/members"),
            &admin,
            Some(serde_json::json!({ "userId": member_id })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "adding a member to an oidc-synced group must 409; body={j}"
    );
    assert!(
        j.to_string()
            .contains("Cannot add members to an OIDC-synced group"),
        "the 409 says why; body={j}"
    );
    assert!(
        !oidc_repo
            .members_of(oidc_group_id)
            .await
            .unwrap()
            .contains(&member_id),
        "the refused add wrote no row"
    );
}

/// `POST /access/roles` creates a custom role; `PUT .../permissions` accepts
/// a known permission (204) and rejects an unknown one (400, validated
/// against `egret_core::rbac::is_known` before any DB write); a seeded role
/// (`data_steward`) cannot be deleted (409) but the custom role can (204).
#[tokio::test]
async fn roles_seeded_and_permissions() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // These RBAC-graph routes (create/set-permissions/delete role) now consult
    // `state.pdp`; use a real Cedar PDP so the platform_admin bypass over the
    // admin-graph `forbid` is exercised for real (not the allow-all stub).
    let admin = session_for(&f, "roles_admin", true).await;
    attach_real_pdp(&mut f).await;
    let role_name = format!("custom_role_{}", Uuid::new_v4().simple());

    // POST /access/roles → 201, a custom role.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "platform",
                "name": role_name,
                "description": "custom test role",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create custom role; body={j}");
    let role_id = j["id"].as_str().expect("role id in body").to_string();

    // PUT permissions with a known permission → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{role_id}/permissions"),
            &admin,
            Some(serde_json::json!({ "permissions": ["engine:read"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "set a known permission"
    );

    // PUT permissions with an unknown permission → 400, no partial write.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{role_id}/permissions"),
            &admin,
            Some(serde_json::json!({ "permissions": ["engine:frobnicate"] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown permission must 400; body={j}"
    );

    // Seeded roles (migration 0003) cannot be deleted.
    let (seeded_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM roles WHERE scope = 'platform' AND name = 'data_steward'")
            .fetch_one(&f.db.pool)
            .await
            .expect("data_steward seeded by migration 0003");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/access/roles/{seeded_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "deleting a seeded role must 409; body={j}"
    );

    // The custom role, not seeded, can be deleted.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/access/roles/{role_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete the custom role"
    );
}

/// A scoped principal without `role:read` is 403 on `GET /access/roles`; with
/// it (or platform_admin), 200. `GET /permissions` returns the non-empty
/// code-defined catalog (`egret_core::rbac::PERMISSIONS`, independent of DB
/// content) for an authorized caller — it too is gated on `role:read`.
///
/// Writes to the authorization graph are a different story: they're
/// `platform_admin`-only now (not delegatable), so even a principal holding
/// `role:read` — enough to *see* roles — must still be 403 on
/// `POST /access/roles`, `PUT .../permissions`, and `POST /users/{id}/roles`.
/// The guard is the handler's first line, so these 403 before any DB lookup —
/// the path/body UUIDs below don't need to resolve to anything real.
#[tokio::test]
async fn permission_enforcement() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The `/access/*` gates consult `state.pdp`; seed the ad-hoc roles BEFORE
    // building the PDP (it snapshots the roles catalog at build), then swap in a
    // real Cedar PDP. This is what makes the RBAC-graph 403s meaningful: the
    // `role:read` reader's create/assign attempts are denied by the admin-graph
    // `forbid`, not by an allow-all stub.
    let wrong = session_for_with_permission(&f, "access_scoped_wrong", "user:read").await;
    let reader = session_for_with_permission(&f, "access_scoped_reader", "role:read").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/access/roles", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without role:read must not list roles"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/access/roles", &reader, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped role:read principal must list roles; body={j}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/permissions", &reader, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list permissions; body={j}");
    let groups = j.as_array().expect("permission groups array");
    assert!(
        !groups.is_empty(),
        "permission catalog must not be empty; body={j}"
    );
    assert!(
        groups
            .iter()
            .any(|g| g["permissions"].as_array().is_some_and(|p| !p.is_empty())),
        "at least one permission group must list permissions; body={j}"
    );

    // A role:read holder can see roles but must not be able to create one —
    // role definitions are part of the authorization graph, platform_admin
    // only.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &reader,
            Some(serde_json::json!({
                "scope": "platform",
                "name": format!("escalation_attempt_{}", Uuid::new_v4().simple()),
                "description": "should never be created",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must not create roles (platform_admin only)"
    );

    // Nor set a role's permissions — this is exactly how the closed
    // escalation worked (a delegated `role:assign_permission` let a
    // non-admin grant themselves anything).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{}/permissions", Uuid::new_v4()),
            &reader,
            Some(serde_json::json!({ "permissions": ["engine:read"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must not set role permissions (platform_admin only)"
    );

    // Nor assign a role to a user — the other half of the closed escalation
    // (a delegated `user:assign_role` let a non-admin hand themselves any
    // role, including platform_admin).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{}/roles", Uuid::new_v4()),
            &reader,
            Some(serde_json::json!({ "roleId": Uuid::new_v4() })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must not assign roles to users (platform_admin only)"
    );
}

/// The admin-graph `forbid` proof, isolated: a non-`platform_admin` principal
/// holding the grantable `role:read` permission can *see* roles
/// (`GET /access/roles` → 200) but is denied by the `forbid` on the graph-
/// mutating `POST /access/roles` (create_role) → 403 — even though `role:read`
/// would, absent the forbid, be a delegatable permission. Runs under a real
/// `CedarPdp` so the forbid actually decides.
#[tokio::test]
async fn role_read_holder_can_list_but_not_create_roles() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // Seed the reader's `role:read` grant BEFORE the PDP snapshots the catalog.
    let reader = session_for_with_permission(&f, "forbid_role_reader", "role:read").await;
    attach_real_pdp(&mut f).await;

    // GET /access/roles → 200 (role:read is enough to see roles).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/access/roles", &reader, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a role:read principal must be able to list roles"
    );

    // POST /access/roles → 403 (create_role is admin-graph, denied by forbid).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &reader,
            Some(serde_json::json!({
                "scope": "platform",
                "name": format!("forbid_denied_{}", Uuid::new_v4().simple()),
                "description": "should be denied by the admin-graph forbid",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must be denied create_role by the admin-graph forbid"
    );
}

/// `GET /access/roles/{id}/assignees` lists a role's assignees by display
/// name (users by email, groups by name), for the admin console's
/// drill-into-a-role view. A scoped principal without `role:read` is 403.
#[tokio::test]
async fn role_assignees_lists_users_and_groups() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // `GET /access/roles/{id}/assignees` gates on `role:read` via `state.pdp`;
    // use a real Cedar PDP so the `role:read`-less principal is genuinely denied
    // (the platform_admin bypass and the deny both hold regardless of the custom
    // role being created after build).
    let admin = session_for(&f, "role_assignees_admin", true).await;
    attach_real_pdp(&mut f).await;
    let roles = egret_auth::roles::RoleRepo::new(f.db.clone(), egret_events::EventBus::new());
    let groups = egret_auth::groups::GroupRepo::new(f.db.clone());

    // A fresh custom role, with no assignees yet.
    let role_name = format!("assignee_role_{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "platform",
                "name": role_name,
                "description": "role assignees test",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create custom role; body={j}");
    let role_id: Uuid = j["id"].as_str().expect("role id in body").parse().unwrap();

    // Assign the role to a user and a group directly (bypassing the
    // assign-role HTTP routes, which are exercised elsewhere).
    let member_email = format!("assignee_user_{}@egret.local", Uuid::new_v4().simple());
    let member_id = f
        .users
        .insert_local(&member_email, "Assignee User", "x")
        .await
        .expect("insert user")
        .0;
    roles
        .assign("user", member_id, role_id)
        .await
        .expect("assign role to user");

    let group_name = format!("assignee-group-{}", Uuid::new_v4().simple());
    let group_id = groups
        .create_local(&group_name, &egret_perm_compile::PermGroupRecompiler)
        .await
        .expect("create group");
    roles
        .assign("group", group_id, role_id)
        .await
        .expect("assign role to group");

    // A scoped principal without `role:read` is 403.
    let wrong = session_for_with_permission(&f, "role_assignees_wrong", "user:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/roles/{role_id}/assignees"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without role:read must not list role assignees"
    );

    // The platform_admin sees both assignees, named.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/roles/{role_id}/assignees"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list role assignees; body={j}");
    let assignees = j.as_array().expect("assignees array");
    assert_eq!(assignees.len(), 2, "two assignees; body={j}");
    assert!(
        assignees
            .iter()
            .any(|a| a["kind"] == "user" && a["name"] == member_email),
        "user assignee named by email; body={j}"
    );
    assert!(
        assignees
            .iter()
            .any(|a| a["kind"] == "group" && a["name"] == group_name),
        "group assignee named by group name; body={j}"
    );
}

/// A workspace role that inherits a parent (`parentRoleId` on create) exposes
/// the parent's permissions via the single-role `GET /access/roles/{id}` as
/// `inheritedPermissions`. Proves the role-hierarchy control-plane surface:
/// create parent → give it `dashboard:read` → create child inheriting it →
/// GET child shows `parentRoleId` + inherited `dashboard:read`.
#[tokio::test]
async fn create_role_with_parent_exposes_inherited_permissions() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // The RBAC-graph routes consult `state.pdp`; the platform_admin bypass makes
    // create/set-permissions/GET all pass. A real Cedar PDP mirrors production.
    let admin = session_for(&f, "role_parent_admin", true).await;
    attach_real_pdp(&mut f).await;

    // Create the parent (workspace scope) and grant it dashboard:read.
    let parent_name = format!("b2_3_api_parent_{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "workspace",
                "name": parent_name,
                "description": "",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create parent role; body={j}");
    let parent_id = j["id"].as_str().expect("parent id in body").to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{parent_id}/permissions"),
            &admin,
            Some(serde_json::json!({ "permissions": ["dashboard:read"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "set parent permissions"
    );

    // Create the child, inheriting the parent.
    let child_name = format!("b2_3_api_child_{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "workspace",
                "name": child_name,
                "description": "",
                "parentRoleId": parent_id,
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create child role; body={j}");
    let child_id = j["id"].as_str().expect("child id in body").to_string();
    assert_eq!(
        j["parentRoleId"].as_str(),
        Some(parent_id.as_str()),
        "child create echoes parentRoleId; body={j}"
    );

    // GET the child role → parentRoleId + inheritedPermissions populated.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/roles/{child_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, dto) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get child role; body={dto}");
    assert_eq!(
        dto["parentRoleId"].as_str(),
        Some(parent_id.as_str()),
        "child GET carries parentRoleId; body={dto}"
    );
    let inherited: Vec<String> =
        serde_json::from_value(dto["inheritedPermissions"].clone()).expect("inheritedPermissions");
    assert!(
        inherited.contains(&"dashboard:read".to_string()),
        "child inherits parent's dashboard:read; body={dto}"
    );
}

/// `GET /control-plane/audit` is platform-admin-gated and keyset-paginates the
/// append-only audit trail (B4). A platform_admin sees the seeded rows newest
/// first with a `nextCursor` when the page is full, the cursor fetches the older
/// row, and a non-admin is 403 (the gate is `require(role:read)` on `state.pdp`).
#[tokio::test]
async fn audit_query_paginates_and_gates() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // The audit route consults `state.pdp` (`require(role:read)`), so a real
    // Cedar PDP is needed for the non-admin 403 to be genuine.
    attach_real_pdp(&mut f).await;
    let admin = session_for(&f, "audit_query_admin", true).await;

    // Seed two decision rows for a unique actor, straight into the store the
    // handler reads (same pool). The unique actor isolates the assertion from
    // any other rows in the shared audit_log.
    let actor = Uuid::new_v4();
    let store = egret_audit::AuditStore::new(f.db.clone());
    let mk = || egret_audit::AuditEvent::Decision {
        actor: Some(actor),
        workspace_id: None,
        action: "dashboard:read".into(),
        resource_type: "workspace".into(),
        resource_id: Some("w".into()),
        allow: false,
        reasons: vec!["role:x".into()],
    };
    store.insert_batch(&[mk()]).await.expect("seed audit row 1");
    store.insert_batch(&[mk()]).await.expect("seed audit row 2");

    // Page 1: newest row + a nextCursor (the page is full at limit=1).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/audit?actor={actor}&kind=decision&limit=1"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "admin audit query; body={j}");
    assert_eq!(
        j["entries"].as_array().expect("entries array").len(),
        1,
        "exactly one entry at limit=1; body={j}"
    );
    assert_eq!(
        j["entries"][0]["action"], "dashboard:read",
        "action; body={j}"
    );
    assert_eq!(j["entries"][0]["decision"], "deny", "deny row; body={j}");
    let first_id = j["entries"][0]["id"].as_i64().expect("id");
    let cursor = j["nextCursor"]
        .as_str()
        .expect("nextCursor present on a full page")
        .to_string();
    // A raw request string must percent-encode the `+` in the rfc3339 offset
    // (the generated client does this); otherwise `+` decodes to a space.
    let cursor_enc = cursor.replace('+', "%2B");

    // Page 2 via the cursor: the older row (a strictly smaller id, no further cursor).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/audit?actor={actor}&kind=decision&limit=1&cursor={cursor_enc}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j2) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "admin audit query page 2; body={j2}"
    );
    assert_eq!(
        j2["entries"].as_array().expect("entries array").len(),
        1,
        "the older row; body={j2}"
    );
    let second_id = j2["entries"][0]["id"].as_i64().expect("id");
    assert!(
        second_id < first_id,
        "page 2 is strictly older (id {second_id} < {first_id})"
    );

    // A non-admin (no role:read) is 403.
    let plain = session_for(&f, "audit_query_plain", false).await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/audit", &plain, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-admin is forbidden on the audit query"
    );

    // Security regression (B4 review finding A): the audit trail is admin-only.
    // A non-admin governance role that holds `role:read` (e.g. `data_steward`)
    // must STILL be forbidden — the gate is the admin-graph `audit:read`, not the
    // grantable `role:read`.
    let steward = session_for_with_permission(&f, "audit_query_steward", "role:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/audit", &steward, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read holder (non-admin) must not read the audit trail"
    );
}

// ---------- Who-can-access (reverse effective-access) ----------

/// Seed a `chart` (with its owning workspace → database → dataset chain) plus a
/// direct object grant on it, and return the chart id. The grant makes the
/// chart's `who_can_access` entries non-empty.
async fn seed_chart_with_grant(f: &Fixture) -> Uuid {
    let ws_slug = format!("wca-ws-{}", Uuid::new_v4().simple());
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, 'WCA WS') RETURNING id")
            .bind(&ws_slug)
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let db_name = format!("wca-db-{}", Uuid::new_v4().simple());
    let (db_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1, 'trino', 'localhost', 8080, $2) RETURNING id",
    )
    .bind(&db_name)
    .bind(ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert database");

    let ds_name = format!("wca-ds-{}", Uuid::new_v4().simple());
    let (ds_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
         VALUES ($1, $2, $3, 't') RETURNING id",
    )
    .bind(ws_id)
    .bind(db_id)
    .bind(&ds_name)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert dataset");

    let chart_name = format!("wca-chart-{}", Uuid::new_v4().simple());
    let (chart_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO analytics.charts (workspace_id, dataset_id, name, viz_type, params) \
         VALUES ($1, $2, $3, 'table', '{}'::jsonb) RETURNING id",
    )
    .bind(ws_id)
    .bind(ds_id)
    .bind(&chart_name)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert chart");

    // A grantee user holding some role directly on this chart (object scope).
    let grantee_email = format!("wca_grantee_{}@egret.local", Uuid::new_v4().simple());
    let grantee = f
        .users
        .insert_local(&grantee_email, "Grantee", "x")
        .await
        .expect("insert grantee")
        .0;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, object_type) \
         SELECT 'user', $1, id, 'object', $2, 'chart' FROM roles WHERE scope='workspace' LIMIT 1",
    )
    .bind(grantee)
    .bind(chart_id)
    .execute(&f.db.pool)
    .await
    .expect("insert object grant");

    chart_id
}

/// `GET /access/who-can-access?kind=&id=` (reverse effective-access): a
/// `role:read` caller gets the object + its non-empty access entries (200); an
/// unknown kind or a missing object is 404; a caller without `role:read` is 403
/// (the gate is `require(role:read)` on `state.pdp`, so run under a real PDP).
#[tokio::test]
async fn who_can_access_lists_principals_and_gates() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Seed the scoped principals BEFORE building the PDP (it snapshots the roles
    // catalog at build); data seeding is PDP-independent.
    let reader = session_for_with_permission(&f, "wca_reader", "role:read").await;
    let wrong = session_for_with_permission(&f, "wca_wrong", "user:read").await;
    let chart_id = seed_chart_with_grant(&f).await;
    attach_real_pdp(&mut f).await;

    // role:read → 200 with an object + non-empty entries.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/who-can-access?kind=chart&id={chart_id}"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "who-can-access; body={j}");
    assert_eq!(j["object"]["kind"], "chart", "object kind; body={j}");
    let entries = j["entries"].as_array().expect("entries array");
    assert!(!entries.is_empty(), "entries must be non-empty; body={j}");

    // Unknown kind → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/who-can-access?kind=bogus&id={chart_id}"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "an unknown object kind must be 404"
    );

    // A random (nonexistent) object → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/who-can-access?kind=chart&id={}", Uuid::new_v4()),
            &reader,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a missing object must be 404"
    );

    // A caller without role:read → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/who-can-access?kind=chart&id={chart_id}"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without role:read must not query who-can-access"
    );
}

// ---------- Tag catalog (Access P3.1) ----------

/// **SP4d debt Task 15 at the route: `PATCH /api/control-plane/tags/{id}` is the
/// third surface that can create the pair that makes a table unreadable, and it
/// refuses with a 400 carrying both remedies.**
///
/// A tag-addressed MASK plus a TABLE-level attachment of the same tag makes the
/// engine type-check that one mask against every column of the scanned table —
/// `SELECT item`, `SELECT id` and even `count(*)` fail, measured live on Trino
/// 470 (`egret_policy::unreadable_table_mask`). Task 5 guarded the two halves;
/// a RENAME writes neither and completes the pair by moving the name they are
/// joined on.
///
/// This test exists at the ROUTE because the repo-level refusal
/// (`egret_bi::TagRepo::update`) is not the thing the Tags console sees: what it
/// renders is `err.message` out of the HTTP body, so the two assertions that
/// matter here are the STATUS (`BiError::BadRequest` → `CpError::BadRequest` →
/// 400, not the 500 a raw error would give) and the remedies surviving into the
/// body. The guard's own matrix is
/// `egret-policy/tests/unreadable_table_mask_db.rs`, and the three-route
/// sequence that confirmed the hole is
/// `egret-bi/tests/tag_rename_unreadable_pair.rs`.
#[tokio::test]
async fn renaming_a_tag_into_a_masked_name_is_refused_at_the_route() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let manager = session_for(&f, "t15mgr", true).await;
    let suffix = Uuid::new_v4().simple().to_string();

    // The tag, created through the route, and attached to a WHOLE table. The
    // attachment goes in by SQL because its own route lives in egret-bi (where
    // Task 5's attachment-direction route test is); what this test drives is
    // the rename.
    let key = format!("t15-tag-{}", &suffix[..12]);
    let (status, j) = into_json(
        f.app
            .clone()
            .oneshot(req(
                "POST",
                "/tags",
                &manager,
                Some(serde_json::json!({ "key": key, "name": format!("PII-{suffix}") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create tag; body={j}");
    let tag_id: Uuid = j["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 'sales', 'orders', '')",
    )
    .bind(tag_id)
    .bind(Uuid::new_v4())
    .execute(&f.db.pool)
    .await
    .expect("table-level attachment");

    // The mask half: a hand-authored `policyType: 1` policy under a TAG
    // service, addressing a name NO tag carries — which is why the policy
    // editor accepted it and why neither of Task 5's guards ran.
    let masked_name = format!("SENSITIVE-{suffix}");
    let policy_name = format!("t15-mask-{suffix}");
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) \
         VALUES ($1, $2, '{}'::jsonb) ON CONFLICT DO NOTHING",
    )
    .bind(900_000_000_i64 + (suffix.as_bytes()[0] as i64))
    .bind("tag")
    .execute(&f.db.pool)
    .await
    .expect("tag service def");
    let svc: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'tag', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("t15-tagsvc-{suffix}"))
    .fetch_one(&f.db.pool)
    .await
    .expect("tag service");
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
         VALUES ($1, $2, jsonb_build_object( \
             'policyType', 1, 'serviceType', 'tag', \
             'resources', jsonb_build_object('tag', \
                 jsonb_build_object('values', jsonb_build_array($3::text)))))",
    )
    .bind(svc)
    .bind(&policy_name)
    .bind(&masked_name)
    .execute(&f.db.pool)
    .await
    .expect("tag masking policy");

    // The rename INTO the masked name → 400, both remedies in the body.
    let (status, j) = into_json(
        f.app
            .clone()
            .oneshot(req(
                "PATCH",
                &format!("/tags/{tag_id}"),
                &manager,
                Some(serde_json::json!({ "name": masked_name })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "renaming a table-attached tag into a masked name must be a 400, not a 500 and not a          silent 200; body={j}"
    );
    let msg = j["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("sales.orders") && msg.contains(&policy_name) && msg.contains(&masked_name),
        "the refusal must name the table, the policy to edit and the tag; got {msg}"
    );
    assert!(
        msg.contains("to the columns that need masking instead of to the table")
            && msg.contains("by catalog/schema/table/column path instead of by tag"),
        "the refusal must carry BOTH remedies into the HTTP body — the Tags console renders \
         exactly this string; got {msg}"
    );

    // Nothing was committed, and the OTHER direction still works through the
    // same route: renaming to a name nothing masks is how a steward escapes.
    let (status, j) = into_json(
        f.app
            .clone()
            .oneshot(req(
                "PATCH",
                &format!("/tags/{tag_id}"),
                &manager,
                Some(serde_json::json!({ "name": format!("BENIGN-{suffix}") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a rename into an unmasked name must still be accepted; body={j}"
    );
    assert_eq!(j["name"], format!("BENIGN-{suffix}"), "body={j}");
}

/// `GET/POST/PATCH/DELETE /api/control-plane/tags`: `tag:read` gates the list,
/// `tag:manage` gates every write. Proves the full CRUD lifecycle plus the two
/// permission gates and the repo's duplicate-key `Conflict` → 409 mapping.
#[tokio::test]
async fn tag_catalog_crud_and_permissions() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let manager = session_for_with_permission(&f, "tag_manager", "tag:manage").await;
    let reader = session_for_with_permission(&f, "tag_reader", "tag:read").await;
    let plain = session_for(&f, "tag_plain", false).await;
    attach_real_pdp(&mut f).await;

    // A caller without tag:read must not list tags.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &plain, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without tag:read must not list tags"
    );

    // tag:manage creates a tag.
    let key = format!("pii-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/tags",
            &manager,
            Some(serde_json::json!({ "key": key, "name": "PII" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create tag; body={j}");
    assert_eq!(j["key"], key, "created tag key; body={j}");
    assert_eq!(j["name"], "PII", "created tag name; body={j}");
    assert_eq!(
        j["assignment_count"], 0,
        "fresh tag has 0 assignments (snake_case, matches FE); body={j}"
    );
    let id = j["id"].as_str().expect("tag id").to_string();

    // tag:read sees it in the list.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &reader, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list tags; body={j}");
    let tags = j.as_array().expect("tags array");
    assert!(
        tags.iter().any(|t| t["id"] == id),
        "listed tags must include the created tag; body={j}"
    );

    // tag:read alone must not create a tag.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/tags",
            &reader,
            Some(serde_json::json!({ "key": format!("nope-{}", Uuid::new_v4().simple()), "name": "Nope" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a tag:read-only principal must not create tags"
    );

    // Duplicate key → 409.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/tags",
            &manager,
            Some(serde_json::json!({ "key": key, "name": "PII dup" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a duplicate tag key must be 409"
    );

    // tag:manage updates the tag.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/tags/{id}"),
            &manager,
            Some(serde_json::json!({ "name": "PII Updated" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update tag; body={j}");
    assert_eq!(j["name"], "PII Updated", "updated name; body={j}");
    assert_eq!(j["key"], key, "key is immutable across update; body={j}");

    // tag:manage deletes the tag.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/tags/{id}"), &manager, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete tag must be 204"
    );

    // It's gone from the list.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &reader, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list tags after delete; body={j}");
    let tags = j.as_array().expect("tags array");
    assert!(
        !tags.iter().any(|t| t["id"] == id),
        "deleted tag must not be listed; body={j}"
    );
}

/// `PUT/DELETE /api/control-plane/tags/{id}/access-policy` (Access P3.3):
/// `tag:manage` sets the required clearance roles (surfaced by `GET /tags` as
/// `required_roles`); DELETE clears them; a principal without `tag:manage` is
/// Forbidden. Model + CRUD only — no PDP enforcement is asserted here.
#[tokio::test]
async fn tag_access_policy_crud_and_permissions() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let manager = session_for_with_permission(&f, "clsp_manager", "tag:manage").await;
    let reader = session_for_with_permission(&f, "clsp_reader", "tag:read").await;
    attach_real_pdp(&mut f).await;

    // Seed a tag to attach the policy to.
    let key = format!("secret-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/tags",
            &manager,
            Some(serde_json::json!({ "key": key, "name": "Secret" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create tag; body={j}");
    let id = j["id"].as_str().expect("tag id").to_string();

    // A fresh tag has no policy → required_roles is [] in the list.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &reader, None))
        .await
        .unwrap();
    let (_, j) = into_json(resp).await;
    let entry = j
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .expect("tag in list")
        .clone();
    assert_eq!(
        entry["required_roles"],
        serde_json::json!([]),
        "a tag with no policy has empty required_roles; body={entry}"
    );

    // A non-tag:manage principal must NOT set the policy.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/tags/{id}/access-policy"),
            &reader,
            Some(serde_json::json!({ "required_roles": ["clearance_high"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "tag:read alone must not set an access policy"
    );

    // tag:manage sets the required clearance roles.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/tags/{id}/access-policy"),
            &manager,
            Some(serde_json::json!({ "required_roles": ["clearance_high", "auditor"] })),
        ))
        .await
        .unwrap();
    // Regression: the response MUST carry a parseable JSON body — the FE `apiJson`
    // calls `.json()`, and an empty 200 body throws "Unexpected end of JSON input"
    // (caught in a live browser smoke, not by a status-only assertion).
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "set access policy must be 200");
    assert_eq!(
        j["ok"], true,
        "set access policy returns a JSON body; body={j}"
    );

    // GET /tags now reflects the required_roles.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &reader, None))
        .await
        .unwrap();
    let (_, j) = into_json(resp).await;
    let entry = j
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .expect("tag in list")
        .clone();
    assert_eq!(
        entry["required_roles"],
        serde_json::json!(["clearance_high", "auditor"]),
        "list must surface the set required_roles; body={entry}"
    );

    // A non-tag:manage principal must NOT clear the policy.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/tags/{id}/access-policy"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "tag:read alone must not clear an access policy"
    );

    // tag:manage clears the policy.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/tags/{id}/access-policy"),
            &manager,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "clear access policy must be 200");
    assert_eq!(
        j["ok"], true,
        "clear access policy returns a JSON body; body={j}"
    );

    // required_roles is empty again.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &reader, None))
        .await
        .unwrap();
    let (_, j) = into_json(resp).await;
    let entry = j
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .expect("tag in list")
        .clone();
    assert_eq!(
        entry["required_roles"],
        serde_json::json!([]),
        "cleared policy → empty required_roles; body={entry}"
    );
}

/// Access P3.4: `PUT /tags/{id}/access-policy` accepts `required_groups` (group
/// ids as strings), the list surfaces them, and a NON-uuid group id is rejected
/// `400` (never spliced unvalidated into a Cedar policy string — the P3.3 lesson).
#[tokio::test]
async fn tag_access_policy_group_clearance_and_uuid_validation() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let manager = session_for_with_permission(&f, "clspg_manager", "tag:manage").await;
    let reader = session_for_with_permission(&f, "clspg_reader", "tag:read").await;
    attach_real_pdp(&mut f).await;

    let key = format!("dept-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/tags",
            &manager,
            Some(serde_json::json!({ "key": key, "name": "Dept" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create tag; body={j}");
    let id = j["id"].as_str().expect("tag id").to_string();

    // Set a GROUP-only clearance policy with two valid uuids.
    let g1 = Uuid::new_v4().to_string();
    let g2 = Uuid::new_v4().to_string();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/tags/{id}/access-policy"),
            &manager,
            Some(serde_json::json!({ "required_roles": [], "required_groups": [g1, g2] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "set group policy must be 200"
    );

    // GET /tags surfaces required_groups.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/tags", &reader, None))
        .await
        .unwrap();
    let (_, j) = into_json(resp).await;
    let entry = j
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .expect("tag in list")
        .clone();
    assert_eq!(
        entry["required_groups"],
        serde_json::json!([g1, g2]),
        "list must surface the set required_groups; body={entry}"
    );

    // A NON-uuid group id → 400 BadRequest (never spliced into Cedar).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/tags/{id}/access-policy"),
            &manager,
            Some(serde_json::json!({ "required_groups": ["not-a-uuid"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a non-uuid required_groups entry must be rejected 400"
    );
}

/// `PATCH /users/{id}` with `status: "disabled"` must take effect
/// IMMEDIATELY: `SessionService::validate` used to only reject a disabled
/// user on their NEXT request (sub-project 1), leaving any already-live
/// session valid until it naturally expired. Task 1 wires in
/// `revoke_all_for_user` on the disable branch so the user's current session
/// stops validating the moment the admin disables them.
///
/// Also asserts the self-guard half of `guard_not_last_admin`: the admin
/// cannot disable their own account (400), independent of the last-admin
/// count.
#[tokio::test]
async fn disabling_a_user_revokes_their_sessions() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "disable_admin", true).await;

    // The admin cannot disable themselves, regardless of how many other
    // admins exist.
    let (admin_id,): (Uuid,) = sqlx::query_as(
        "SELECT pra.principal_id FROM access_grants pra \
         JOIN roles r ON r.id = pra.role_id \
         JOIN sessions s ON s.token_hash = $1 \
         WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform' \
           AND pra.principal_id = s.user_id",
    )
    .bind(egret_auth::token::hash_token(&admin_token))
    .fetch_one(&f.db.pool)
    .await
    .expect("the admin's own platform_admin grant");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/users/{admin_id}"),
            &admin_token,
            Some(serde_json::json!({ "status": "disabled" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an admin cannot disable their own account; body={j}"
    );

    // Create a target (non-admin) user and a live session for them.
    let email = format!("lockout_{}@egret.local", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/users",
            &admin_token,
            Some(serde_json::json!({
                "email": email,
                "name": "Lockout Target",
                "password": "S3curePassw0rd!",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create target user; body={j}");
    let user_id: Uuid = j["id"].as_str().expect("user id in body").parse().unwrap();

    let issued = f
        .sessions
        .create(UserId(user_id), "test-agent", "127.0.0.1")
        .await
        .expect("create a live session for the target user");
    assert!(
        f.sessions
            .validate(&issued.token)
            .await
            .expect("validate")
            .is_some(),
        "the target's session must be live before disabling them"
    );

    // Disable the target — must succeed and immediately kill their session.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/users/{user_id}"),
            &admin_token,
            Some(serde_json::json!({ "status": "disabled" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "disable target user; body={j}");
    assert_eq!(j["status"], "disabled", "user status in body; body={j}");

    assert!(
        f.sessions
            .validate(&issued.token)
            .await
            .expect("validate")
            .is_none(),
        "disabling the user must revoke their live session immediately, \
         not just on their next request"
    );
}

/// `guard_not_last_admin` blocks disabling the platform-wide LAST
/// `platform_admin` — 409 Conflict, not 200. Run against `setup_isolated_schema` (see
/// its docstring for why the shared-schema fixture can't give this a
/// deterministic count).
#[tokio::test]
async fn cannot_disable_the_last_platform_admin() {
    let Some(f) = setup_isolated_schema("egret_test_iso_disable_last_admin").await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The lone platform_admin in this isolated schema.
    let _lone_admin_token = session_for(&f, "lone_admin", true).await;
    let (admin_id,): (Uuid,) = sqlx::query_as(
        "SELECT pra.principal_id FROM access_grants pra \
         JOIN roles r ON r.id = pra.role_id \
         WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform'",
    )
    .fetch_one(&f.db.pool)
    .await
    .expect("the sole platform_admin assignment");

    // A distinct caller with just `user:disable` (not platform_admin), so the
    // self-guard doesn't mask the last-admin guard under test.
    let caller_token = session_for_with_permission(&f, "disabler", "user:disable").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/users/{admin_id}"),
            &caller_token,
            Some(serde_json::json!({ "status": "disabled" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "disabling the last platform_admin must 409; body={j}"
    );
    assert!(
        j["error"]
            .as_str()
            .unwrap_or_default()
            .contains("last platform admin"),
        "409 body should explain the last-admin guard; body={j}"
    );

    // A second admin breaks the tie: the same disable now succeeds.
    let _second_admin_token = session_for(&f, "second_admin", true).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/users/{admin_id}"),
            &caller_token,
            Some(serde_json::json!({ "status": "disabled" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "disable succeeds once a second platform_admin exists"
    );
}

/// `POST /users/{id}/password` (Task 2): an admin reset rehashes a LOCAL
/// user's password and immediately revokes their live sessions — mirroring
/// the disable branch's `revoke_all_for_user` call — so a session issued
/// under the old password can't linger past the reset.
#[tokio::test]
async fn reset_password_rehashes_and_revokes_sessions() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "resetpw_admin", true).await;

    let email = format!("resetpw_{}@egret.local", Uuid::new_v4().simple());
    let user_id = f
        .users
        .insert_local(&email, "Reset Target", "old-hash-placeholder")
        .await
        .expect("insert target user");

    // A live session, issued before the reset.
    let issued = f
        .sessions
        .create(user_id, "test-agent", "127.0.0.1")
        .await
        .expect("create a live session for the target user");
    assert!(
        f.sessions
            .validate(&issued.token)
            .await
            .expect("validate")
            .is_some(),
        "the target's session must be live before the reset"
    );

    let new_password = "N3wSecurePassw0rd!";
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{}/password", user_id.0),
            &admin_token,
            Some(serde_json::json!({ "password": new_password })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "resetting a local user's password should 204"
    );

    // The old session must no longer validate.
    assert!(
        f.sessions
            .validate(&issued.token)
            .await
            .expect("validate")
            .is_none(),
        "resetting the password must revoke the user's live sessions immediately"
    );

    // The stored hash must verify the NEW password (and not the placeholder).
    let updated = f
        .users
        .find_by_id(user_id)
        .await
        .expect("find target")
        .expect("target exists");
    let stored_hash = updated
        .password_hash
        .expect("local user has a password_hash");
    assert!(
        egret_auth::password::verify_password(new_password, &stored_hash).expect("verify_password"),
        "the stored hash must verify the new password"
    );
}

/// `POST /users/{id}/password` must reject a target that isn't a LOCAL
/// account — an OIDC-provisioned user has no password to reset.
#[tokio::test]
async fn reset_password_rejects_sso_user() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "resetpw_admin_sso", true).await;

    let sub = format!("sso-subject-{}", Uuid::new_v4().simple());
    let email = format!("resetpw_sso_{}@egret.local", Uuid::new_v4().simple());
    let user_id = f
        .users
        .insert_oidc(&sub, &email, "SSO Target")
        .await
        .expect("insert oidc user");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{}/password", user_id.0),
            &admin_token,
            Some(serde_json::json!({ "password": "whatever-Passw0rd!" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "resetting a non-local (SSO) user's password should 400; body={j}"
    );
}

/// `DELETE /users/{id}` (Task 4): an admin cannot delete their own account
/// (400, `guard_not_last_admin`'s self-target branch); deleting a normal
/// (non-admin) user succeeds (204) and hard-removes the row, its group
/// membership, and its live sessions in one transaction.
#[tokio::test]
async fn delete_user_route_enforces_guards() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "delete_admin", true).await;
    let (admin_id,): (Uuid,) = sqlx::query_as(
        "SELECT pra.principal_id FROM access_grants pra \
         JOIN roles r ON r.id = pra.role_id \
         JOIN sessions s ON s.token_hash = $1 \
         WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform' \
           AND pra.principal_id = s.user_id",
    )
    .bind(egret_auth::token::hash_token(&admin_token))
    .fetch_one(&f.db.pool)
    .await
    .expect("the admin's own platform_admin grant");

    // Self-delete is rejected — 400, not a 200/204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an admin cannot delete their own account; body={j}"
    );
    assert!(
        f.users
            .find_by_id(UserId(admin_id))
            .await
            .unwrap()
            .is_some(),
        "self-delete must be a no-op"
    );

    // A normal (non-admin) target, with a group membership and a live session.
    let email = format!("delete_target_{}@egret.local", Uuid::new_v4().simple());
    let user_id = f
        .users
        .insert_local(&email, "Delete Target", "x")
        .await
        .expect("insert target user");
    let groups = egret_auth::groups::GroupRepo::new(f.db.clone());
    let group_id = groups
        .create_local(
            &format!("delete-test-group-{}", Uuid::new_v4().simple()),
            &egret_perm_compile::PermGroupRecompiler,
        )
        .await
        .expect("create group");
    groups
        .add_member(group_id, user_id, &egret_perm_compile::PermGroupRecompiler)
        .await
        .expect("add member");
    let issued = f
        .sessions
        .create(user_id, "test-agent", "127.0.0.1")
        .await
        .expect("create a live session for the target user");
    assert!(f.sessions.validate(&issued.token).await.unwrap().is_some());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{}", user_id.0),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "deleting a normal user should 204"
    );

    assert!(
        f.users.find_by_id(user_id).await.unwrap().is_none(),
        "user row must be gone after hard-delete"
    );
    assert!(
        groups.groups_of_user(user_id).await.unwrap().is_empty(),
        "group membership must be cleaned up"
    );
    assert!(
        f.sessions.validate(&issued.token).await.unwrap().is_none(),
        "the deleted user's live session must no longer validate"
    );

    // Deleting an id that no longer exists (or never did) is a 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{}", user_id.0),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "deleting an already-deleted user should 404"
    );
}

/// `guard_not_last_admin` blocks deleting the platform-wide LAST
/// `platform_admin` — 409 Conflict. Run against `setup_isolated_schema` for a
/// deterministic admin count (see its docstring, and `cannot_disable_the_last_platform_admin`
/// for the analogous disable-route test).
#[tokio::test]
async fn cannot_delete_the_last_platform_admin() {
    let Some(f) = setup_isolated_schema("egret_test_iso_delete_last_admin").await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let _lone_admin_token = session_for(&f, "lone_admin_del", true).await;
    let (admin_id,): (Uuid,) = sqlx::query_as(
        "SELECT pra.principal_id FROM access_grants pra \
         JOIN roles r ON r.id = pra.role_id \
         WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform'",
    )
    .fetch_one(&f.db.pool)
    .await
    .expect("the sole platform_admin assignment");

    // A distinct caller with just `user:delete` (not platform_admin), so the
    // self-guard doesn't mask the last-admin guard under test.
    let caller_token = session_for_with_permission(&f, "deleter", "user:delete").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}"),
            &caller_token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "deleting the last platform_admin must 409; body={j}"
    );
    assert!(
        j["error"]
            .as_str()
            .unwrap_or_default()
            .contains("last platform admin"),
        "409 body should explain the last-admin guard; body={j}"
    );
    assert!(
        f.users
            .find_by_id(UserId(admin_id))
            .await
            .unwrap()
            .is_some(),
        "the last admin must not be deleted"
    );

    // A second admin breaks the tie: the same delete now succeeds.
    let _second_admin_token = session_for(&f, "second_admin_del", true).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}"),
            &caller_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete succeeds once a second platform_admin exists"
    );
}

/// `DELETE /users/{id}` (Task 4, then UDR Task 2): a user who OWNS content — a
/// dashboard via `analytics.dashboards.created_by` (nullable, `ON DELETE SET
/// NULL` FK per mig 0093) — can now be hard-deleted: the dashboard's
/// attribution is nulled out by its FK, so nothing trips
/// `foreign_key_violation` (23503) and the content survives.
#[tokio::test]
async fn deleting_a_content_owner_nulls_attribution_and_succeeds() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "delete_owner_admin", true).await;

    // A normal target user who will own a dashboard.
    let email = format!("content_owner_{}@egret.local", Uuid::new_v4().simple());
    let user_id = f
        .users
        .insert_local(&email, "Content Owner", "x")
        .await
        .expect("insert target user");

    // A workspace + a dashboard created_by the target (`analytics.dashboards`.
    // `created_by` is nullable and carries `ON DELETE SET NULL` since mig 0093).
    let (ws_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO workspaces (slug, name) VALUES ($1, 'Delete FK Test') RETURNING id",
    )
    .bind(format!("delete-fk-{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("insert workspace");
    let (dashboard_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO analytics.dashboards (workspace_id, name, created_by) \
         VALUES ($1, 'Owned', $2) RETURNING id",
    )
    .bind(ws_id)
    .bind(user_id.0)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert dashboard owned by the target");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{}", user_id.0),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "deleting a content-owning user must now succeed; body={j}"
    );

    // The user is gone.
    assert!(
        f.users.find_by_id(user_id).await.unwrap().is_none(),
        "the content owner must be deleted"
    );

    // The dashboard survives, attribution nulled out.
    let dashboard_created_by: Option<Uuid> =
        sqlx::query_scalar("SELECT created_by FROM analytics.dashboards WHERE id = $1")
            .bind(dashboard_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("dashboard must still exist");
    assert_eq!(
        dashboard_created_by, None,
        "dashboard.created_by must be nulled out by its ON DELETE SET NULL FK"
    );
}

/// These tests never reach the materialized-view report; a lister that is asked
/// anyway fails loudly instead of reporting an empty engine.
struct NoListing;

#[async_trait::async_trait]
impl egret_controlplane::MaterializedViewLister for NoListing {
    async fn list(
        &self,
        _engine: &egret_controlplane::ExposureEngine,
    ) -> Result<Vec<egret_controlplane::MvRow>, String> {
        Err("no StarRocks in this test".to_string())
    }
}

/// 2026-10-18 design R1 at the route: `DELETE` and `PATCH /groups/{id}` on a
/// local group that a rule exemption still names answer 409, and the message
/// names the entry (the console's dialogs show it). Nothing is written.
#[tokio::test]
async fn a_referenced_group_answers_409_naming_the_entry() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "groups_ref_admin", true).await;
    let group_name = format!("ref-group-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/groups",
            &admin,
            Some(serde_json::json!({ "name": group_name })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "body={j}");
    let group_id = j["id"].as_str().expect("group id").to_string();

    // A tag row filter exempting the group, written behind the perm routes'
    // back — this test is about the group routes.
    let tag_key = format!("ref-tag-{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&tag_key)
        .execute(&f.db.pool)
        .await
        .expect("tag");
    let rule_name = format!("ref-rule-{}", Uuid::new_v4().simple());
    let rule_id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, filter_expr, selector_kind, tag_key) \
         VALUES ($1, 'row_filter', '1 = 1', 'tag', $2) RETURNING id",
    )
    .bind(&rule_name)
    .bind(&tag_key)
    .fetch_one(&f.db.pool)
    .await
    .expect("rule");
    sqlx::query(
        "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
         VALUES ($1, 'group', $2, 'include')",
    )
    .bind(rule_id)
    .bind(&group_name)
    .execute(&f.db.pool)
    .await
    .expect("exemption");

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/groups/{group_id}"), &admin, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    assert!(
        j.to_string().contains(&format!(
            "group `{group_name}` is named by 1 permission entry; remove them first: \
             rule `{rule_name}` (exempt include)"
        )),
        "the 409 names the entry; body={j}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/groups/{group_id}"),
            &admin,
            Some(serde_json::json!({ "name": format!("{group_name}-renamed") })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    assert!(
        j.to_string().contains(&format!(
            "rule `{rule_name}` (exempt include) naming `{group_name}`"
        )),
        "the 409 names the entry and the name it refers to; body={j}"
    );

    let still: i64 = sqlx::query_scalar("SELECT count(*) FROM groups WHERE name = $1")
        .bind(&group_name)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(still, 1, "neither request wrote anything");
}
