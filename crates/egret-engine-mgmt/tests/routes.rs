//! Engine-mgmt seam HTTP route tests (engine CRUD, catalogs, monitor, exec/log
//! ownership). Moved from `egret-controlplane/tests/routes_api.rs` (E1c).
//! DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-engine-mgmt

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_engine_mgmt::{engine_mgmt_router, EngineMgmtState, FakeStarRocksDdl};
use egret_engine_mgmt_api::EngineProvisioningPort;
use egret_metastore::Db;
use egret_provisioner::KubeClient;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    ranger_enabled: bool,
    /// The `FakeStarRocksDdl` `f.app`'s `EngineMgmtState` was built with.
    /// `FakeStarRocksDdl` is `Clone` over shared `Arc<Mutex<…>>` interior
    /// state, so `f.starrocks_ddl.executed()`/`.catalog(...)`/etc. observe
    /// whatever `f.app` did even across the two `f.app = build_app(...)`
    /// rebuilds below (`attach_real_pdp`, the Ranger-toggle test) — both pass
    /// `f.starrocks_ddl.clone()` through rather than a fresh fake, so a test
    /// holding this handle doesn't have its recorded state silently
    /// discarded by a mid-test rebuild (fix round 1 review, F-8).
    starrocks_ddl: FakeStarRocksDdl,
    /// The SP4a profile compiler `f.app` was built with (`None` for every test
    /// that does not care), threaded through the rebuilds for the same reason
    /// `starrocks_ddl` is.
    perm_compiler: Option<Arc<dyn egret_engine_mgmt::ProfileCompilerPort>>,
}

/// Records which profiles the engine seam asked to have recompiled. The
/// compiler itself is covered by `egret-perm-compile`'s own suite; what is new
/// here — and what this fake is for — is that an engine BIND reaches it at all,
/// with the right `service_id`, and only once every catalog the engine attaches
/// is attached.
#[derive(Clone, Default)]
struct RecordingProfileCompiler {
    calls: Arc<std::sync::Mutex<Vec<i64>>>,
    /// `recompile_profile_in_tx` calls, kept SEPARATE from `calls` so a test
    /// asserting the update path's in-transaction recompile cannot be
    /// satisfied by the create path's fire-and-forget one.
    in_tx_calls: Arc<std::sync::Mutex<Vec<i64>>>,
}

#[async_trait::async_trait]
impl egret_engine_mgmt::ProfileCompilerPort for RecordingProfileCompiler {
    async fn recompile_profile(&self, service_id: i64) -> Result<(), egret_perm::CompileError> {
        self.calls.lock().unwrap().push(service_id);
        Ok(())
    }

    async fn recompile_catalog_assignment_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        _catalog_id: Uuid,
    ) -> Result<(), egret_perm::CompileError> {
        // Engine-bind tests only assert `recompile_profile` was reached with
        // the right `service_id`; the assignment-recompile suite below uses
        // `RealProfileCompiler` instead, which actually compiles.
        Ok(())
    }

    async fn recompile_profile_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_id: i64,
    ) -> Result<(), egret_perm::CompileError> {
        self.in_tx_calls.lock().unwrap().push(service_id);
        Ok(())
    }

    async fn overlap_snapshot_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        _service_ids: &[i64],
    ) -> Result<egret_engine_mgmt::OverlapSnapshot, egret_perm::CompileError> {
        Ok(egret_engine_mgmt::OverlapSnapshot::new(()))
    }

    async fn refuse_created_overlaps_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        _snapshot: &egret_engine_mgmt::OverlapSnapshot,
    ) -> Result<(), egret_perm::CompileError> {
        Ok(())
    }
}

/// A `ProfileCompilerPort` that really recompiles, against the fixture's own
/// DB pool — the same construction `egret-bin`'s `ProfileCompilerImpl` wires
/// in production. `RecordingProfileCompiler` above is enough for the
/// engine-bind tests (they only assert a `service_id` was reached); SP4a Task
/// 15's D3-assignment tests need the compiled effect itself, in
/// `policy.authz_policies`, to actually exist.
#[derive(Clone)]
struct RealProfileCompiler {
    perm: egret_perm::PermStore,
    policy: egret_policy::PolicyStore,
}

#[async_trait::async_trait]
impl egret_engine_mgmt::ProfileCompilerPort for RealProfileCompiler {
    async fn recompile_profile(&self, service_id: i64) -> Result<(), egret_perm::CompileError> {
        egret_perm_compile::recompile_service(&self.perm, &self.policy, service_id)
            .await
            .map(|_| ())
    }

    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: Uuid,
    ) -> Result<(), egret_perm::CompileError> {
        let mut ids = egret_policy::services_for_catalog_via(tx, catalog_id)
            .await
            .map_err(|e| egret_perm::CompileError::Db(e.to_string()))?;
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(());
        }
        egret_perm_compile::lock_services_in_tx(tx, &ids).await?;
        egret_perm_compile::recompile_services_in_tx(tx, &ids).await?;
        Ok(())
    }

    /// The same two calls `egret-bin`'s `recompile_one_profile` makes.
    async fn recompile_profile_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_id: i64,
    ) -> Result<(), egret_perm::CompileError> {
        egret_perm_compile::lock_services_in_tx(tx, &[service_id]).await?;
        egret_perm_compile::recompile_service_in_tx(tx, service_id).await?;
        Ok(())
    }

    async fn overlap_snapshot_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<egret_engine_mgmt::OverlapSnapshot, egret_perm::CompileError> {
        Ok(egret_engine_mgmt::OverlapSnapshot::new(
            egret_perm_compile::overlap::unit_snapshot_via(tx, service_ids).await?,
        ))
    }

    async fn refuse_created_overlaps_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        snapshot: &egret_engine_mgmt::OverlapSnapshot,
    ) -> Result<(), egret_perm::CompileError> {
        let snapshot = snapshot
            .get::<egret_perm_compile::overlap::UnitSnapshot>()
            .expect("a snapshot this compiler produced");
        egret_perm_compile::overlap::refuse_created_overlaps_via(tx, snapshot).await
    }
}

/// Incidental PDP for `EngineMgmtState`: every existing route test authenticates
/// a `platform_admin` (whose bypass predates the PDP) and asserts non-authz
/// behavior, so an allow-all stub keeps those fixtures light. Any test that
/// asserts a 403 (the one below) must instead build a real `CedarPdp` via
/// `attach_real_pdp` — an allow-all stub would falsely 200.
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

/// Build the engine-mgmt `Router` for a given db + provisioner + kube + PDP +
/// StarRocks DDL fake. Shared by `setup_full` (allow-all PDP) and
/// `attach_real_pdp` (real Cedar PDP) so the two only differ in the injected
/// decision point — and by the Ranger-toggle test's mid-test rebuild, which is
/// why `starrocks_ddl` is a parameter (fix round 1 review, F-8) rather than a
/// fresh default built inside: there ARE three callers today (below, and
/// `attach_real_pdp`, and the Ranger-toggle rebuild further down), and a
/// caller that wants to inspect what its app's fake recorded needs the SAME
/// fake threaded through every rebuild, not a new one each time.
#[allow(clippy::too_many_arguments)]
fn build_app(
    db: &Db,
    sessions: &SessionService,
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    ranger_enabled: bool,
    starrocks_ddl: FakeStarRocksDdl,
    perm_compiler: Option<Arc<dyn egret_engine_mgmt::ProfileCompilerPort>>,
) -> axum::Router {
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = egret_query::DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());

    engine_mgmt_router()
        .merge(egret_engine_mgmt::workspace_engines_router())
        .with_state(EngineMgmtState {
            engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
            assignment: egret_engine_mgmt::EngineAssignmentService::new(
                egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
                databases,
            ),
            catalog: std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
            storage_registry: egret_storage::StorageRegistry::new(db.clone()),
            authz: std::sync::Arc::new(egret_policy::PolicyStore::new(db.clone())),
            secrets: workspace_secrets,
            sessions: sessions.clone(),
            users: egret_auth::users::UserRepo::new(db.clone()),
            roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
            pdp,
            engine_provisioner,
            kube,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
            engine_oidc_issuer: "https://idp/realms/egret".into(),
            ranger_enabled,
            sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers::default(),
            starrocks_ddl: std::sync::Arc::new(starrocks_ddl),
            perm_compiler,
            // SP4a Task 16: no test here exercises `ensure_warehouse_catalog`'s
            // own recompile through this crate's create-engine callers (it is
            // a proven no-op there — see `RegistryStore::ensure_warehouse_catalog_with_compiler`'s
            // doc), so `None` for every fixture.
            catalog_assignment_compiler: None,
        })
}

/// Rebuild `f.app` with a REAL `CedarPdp`. The PDP snapshots the
/// `roles`/`role_permissions` catalog at build time and (in these tests) never
/// reloads — raw-SQL seeding emits no `RolesChanged` event — so callers MUST
/// seed any ad-hoc roles the PDP needs to know about BEFORE calling this.
async fn attach_real_pdp(f: &mut Fixture) {
    let pdp = std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        pdp,
        f.ranger_enabled,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
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

async fn setup_full(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
) -> Option<Fixture> {
    setup_with_ranger(engine_provisioner, kube, false).await
}

/// `setup_full` with `config.ranger.enabled` under the test's control: the Trino
/// create path auto-creates (or reuses) a Ranger profile only when it is on.
async fn setup_with_ranger(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    ranger_enabled: bool,
) -> Option<Fixture> {
    setup_with_ranger_and_compiler(engine_provisioner, kube, ranger_enabled, None).await
}

/// `setup_with_ranger` with the SP4a profile compiler under the test's control.
/// `None` — every existing caller — leaves the engine seam exactly as it was.
async fn setup_with_ranger_and_compiler(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    ranger_enabled: bool,
    perm_compiler: Option<Arc<dyn egret_engine_mgmt::ProfileCompilerPort>>,
) -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    // Existing route tests authenticate a platform_admin and assert non-authz
    // behavior, so an allow-all PDP suffices. The authz test rebuilds with a real
    // Cedar PDP via `attach_real_pdp`.
    let starrocks_ddl = FakeStarRocksDdl::default();
    let app = build_app(
        &db,
        &sessions,
        engine_provisioner.clone(),
        kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        ranger_enabled,
        starrocks_ddl.clone(),
        perm_compiler.clone(),
    );

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        engine_provisioner,
        kube,
        ranger_enabled,
        starrocks_ddl,
        perm_compiler,
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

/// Seed a workspace + storage_connection + warehouse with lakekeeper_warehouse_id set.
async fn seed_provisioned_warehouse(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("engine-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .create_warehouse(
            &format!("engine-wh-{}", Uuid::new_v4().simple()),
            ws_id,
            storage_id,
        )
        .await
        .expect("create warehouse");
    catalog_store
        .set_lakekeeper_warehouse_id(wh.id, "wh-123")
        .await
        .expect("set lakekeeper id");
    wh.id
}

/// Like [`seed_provisioned_warehouse`], but the storage connection carries a
/// (global) named secret so its `secret_ref` is set. The StarRocks create path
/// treats a missing secret as a hard error, so its tests need this variant.
async fn seed_provisioned_warehouse_with_secret(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-sec-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace (secret)")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    // Seal a global named secret with the SAME crypto key `build_app` uses, so
    // the app's `resolve_plaintext(None, secret_ref)` can decrypt it.
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(f.db.clone(), Some(crypto));
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(f.db.clone(), secrets);
    let secret_id = ws_secrets
        .create(
            None,
            &format!("engine-sec-{}", Uuid::new_v4().simple()),
            None,
            "STORAGESECRETKEY",
        )
        .await
        .expect("create global storage secret");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key, secret_ref) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(format!("engine-sc-sec-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .bind(secret_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .create_warehouse(
            &format!("engine-wh-sec-{}", Uuid::new_v4().simple()),
            ws_id,
            storage_id,
        )
        .await
        .expect("create warehouse");
    catalog_store
        .set_lakekeeper_warehouse_id(wh.id, "wh-123")
        .await
        .expect("set lakekeeper id");
    wh.id
}

/// A storage connection whose `secret_ref` is a global named secret sealed
/// with the SAME crypto key `build_app` uses, so the app can resolve it. A
/// catalog over this renders BOTH `aws.s3.access_key` (Plain, and a key the FE
/// masks) and `aws.s3.secret_key` (Secret) — the pairing `Drift::Unverifiable`
/// needs.
async fn seed_storage_conn_with_secret(f: &Fixture) -> Uuid {
    let secret_id = secret_store(f)
        .create(
            None,
            &format!("engine-sc-sec-{}", Uuid::new_v4().simple()),
            None,
            "STORAGESECRETKEY",
        )
        .await
        .expect("create global storage secret");
    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key, secret_ref, endpoint) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(format!("engine-sc-only-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .bind(secret_id)
    .bind("http://rustfs:9000")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");
    storage_id
}

/// [`seed_provisioned_warehouse`] under a CHOSEN warehouse name — the registry
/// names a warehouse's catalog after its warehouse, so this is how a test
/// decides what that catalog will be called.
async fn seed_warehouse_named(f: &Fixture, name: &str) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-named-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace (named)")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");
    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("engine-sc-named-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .create_warehouse(name, ws_id, storage_id)
        .await
        .expect("create warehouse");
    catalog_store
        .set_lakekeeper_warehouse_id(wh.id, "wh-123")
        .await
        .expect("set lakekeeper id");
    wh.id
}

/// Seed a workspace + storage_connection + warehouse WITHOUT lakekeeper_warehouse_id.
async fn seed_unprovisioned_warehouse(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-unprov-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace Unprov")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("engine-sc-unprov-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    catalog_store
        .create_warehouse(
            &format!("engine-wh-unprov-{}", Uuid::new_v4().simple()),
            ws_id,
            storage_id,
        )
        .await
        .expect("create warehouse")
        .id
}

// ── Engines ────────────────────────────────────────────────────────────────────

/// Kube-free `EngineProvisioningPort` fake. `status` reports a single running,
/// ready pod named "coordinator-0" (so the pod-ownership check in
/// `pod_logs`/`pod_exec` passes for that name). Every other call is RECORDED in
/// shared `Arc` state, so a test that keeps a clone of the fake can inspect
/// exactly what the app asked the cluster to do. `fail_provision`/`fail_update`
/// make the matching call record its spec and then fail, standing in for a
/// Kubernetes API error; `delete_error`, when `Some`, makes `delete` fail with
/// that message instead (and record nothing) — for the 502/retry tests. The
/// kube-shaped ops (exec/pod-logs/coordinator-request) are NOT on this trait —
/// `egret_provisioner::kube_client::FakeKube` covers those, passed separately as the
/// fixture's `kube` field.
#[derive(Clone, Default)]
struct FakeEnginePort {
    restarted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    provisioned: Arc<std::sync::Mutex<Vec<egret_engine_mgmt_api::EngineSpec>>>,
    updated: Arc<std::sync::Mutex<Vec<egret_engine_mgmt_api::EngineSpec>>>,
    /// `(kind, namespace, name)` per successful `delete`.
    deleted: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
    /// When `Some`, `delete` fails with this message and records nothing.
    delete_error: Arc<std::sync::Mutex<Option<String>>>,
    fail_provision: Arc<std::sync::atomic::AtomicBool>,
    fail_update: Arc<std::sync::atomic::AtomicBool>,
    /// Engine names whose `update` fails (the others succeed) — for propagation.
    fail_update_for: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// When `Some`, `status` returns this instead of the hard-coded
    /// all-pods-ready `running` default — so a test can drive a not-ready /
    /// crash-looping cluster through `GET /engines/{id}` (I1).
    status_response: Arc<std::sync::Mutex<Option<egret_engine_mgmt_api::EngineStatus>>>,
    /// How many times `status` was asked, for any engine. An external engine has
    /// no cluster of egret's to ask about, so a read of one must leave this at 0.
    status_calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl FakeEnginePort {
    fn set_status(&self, status: egret_engine_mgmt_api::EngineStatus) {
        *self.status_response.lock().unwrap() = Some(status);
    }
    fn last_provisioned(&self) -> egret_engine_mgmt_api::EngineSpec {
        self.provisioned
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("provision was called")
    }
    fn last_updated(&self) -> egret_engine_mgmt_api::EngineSpec {
        self.updated
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("update was called")
    }
    fn set_fail_update(&self, fail: bool) {
        self.fail_update
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    fn set_fail_update_for(&self, engine_name: &str, fail: bool) {
        let mut names = self.fail_update_for.lock().unwrap();
        if fail {
            names.insert(engine_name.to_string());
        } else {
            names.remove(engine_name);
        }
    }
    fn set_fail_provision(&self, fail: bool) {
        self.fail_provision
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl EngineProvisioningPort for FakeEnginePort {
    async fn provision(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.provisioned.lock().unwrap().push(req.clone());
        if self
            .fail_provision
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err("injected provision failure".into());
        }
        Ok(())
    }
    async fn status(
        &self,
        _kind: &str,
        _ns: &str,
        _name: &str,
    ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
        self.status_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(s) = self.status_response.lock().unwrap().clone() {
            return Ok(s);
        }
        Ok(egret_engine_mgmt_api::EngineStatus {
            phase: "running".into(),
            message: None,
            pods: vec![egret_engine_mgmt_api::PodStatus {
                name: "coordinator-0".into(),
                phase: "Running".into(),
                ready: true,
            }],
        })
    }
    async fn update(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.updated.lock().unwrap().push(req.clone());
        if self.fail_update.load(std::sync::atomic::Ordering::SeqCst)
            || self.fail_update_for.lock().unwrap().contains(&req.name)
        {
            return Err("injected update failure".into());
        }
        Ok(())
    }
    async fn delete(&self, kind: &str, ns: &str, name: &str) -> Result<(), String> {
        if let Some(msg) = self.delete_error.lock().unwrap().clone() {
            return Err(msg);
        }
        self.deleted
            .lock()
            .unwrap()
            .push((kind.to_string(), ns.to_string(), name.to_string()));
        Ok(())
    }
    async fn restart(&self, _kind: &str, ns: &str, name: &str) -> Result<(), String> {
        self.restarted
            .lock()
            .unwrap()
            .push((ns.to_string(), name.to_string()));
        Ok(())
    }
}

#[tokio::test]
async fn monitor_queries_maps_list_and_maps_bad_gateway() {
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_monitor_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create the engine.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // Coordinator returns a `/v1/query` array → mapped to Vec<QueryDto>.
    fake.set_coordinator_response(
        200,
        br#"[{"queryId":"20260704_x","state":"RUNNING","query":"SELECT 1",
            "session":{"user":"bob"},
            "queryStats":{"elapsedTime":"1.23s","totalCpuTime":"0.50s","userMemoryReservation":"12MB","progressPercentage":42.0}}]"#.to_vec(),
    );
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list queries; body={j}");
    assert_eq!(j[0]["id"], "20260704_x", "mapped query id; body={j}");
    assert_eq!(j[0]["user"], "bob", "mapped query user; body={j}");
    assert_eq!(j[0]["memory"], "12MB", "mapped query memory; body={j}");

    // A non-2xx coordinator response → the route returns 502.
    fake.set_coordinator_response(503, b"<html>service unavailable</html>".to_vec());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "non-2xx coordinator response must 502"
    );
}

/// `POST /engines/{id}/catalogs/{cid}/test`: a canned `FINISHED` (no `nextUri`,
/// no `error`) reports `ok:true`; a canned `error` body reports `ok:false` with
/// the message surfaced; a non-2xx coordinator response reports `ok:false`
/// with a status/body summary (NOT a 502 — this route always answers 200 with
/// its verdict in the body); a catalog id from a different engine 404s. Kept
/// live by SP3c-2 Task 4 alongside its five deleted siblings — see
/// `test_catalog`'s doc comment in `engines.rs`.
///
/// LIMITATION: `FakeKube` holds a single canned `(status, body)`
/// shared across every `coordinator_request` call, so it cannot return a
/// first response with `nextUri` and then a different second response — this
/// test therefore only exercises the immediate-terminal (no `nextUri`) path
/// through `test_catalog`'s poll loop, not a multi-hop `nextUri` chase.
#[tokio::test]
async fn test_catalog_reports_ok_error_and_non_2xx() {
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_catalog_test_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create the engine (auto-wires the primary egret-warehouse catalog).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // The auto-wired primary catalog id — off the store directly, the legacy
    // `GET /engines/{id}/catalogs` list this once came from is gone (Task 4).
    let cats = registry(&f)
        .list_engine_attachments(Uuid::parse_str(&engine_id).unwrap())
        .await
        .expect("list attachments");
    let cat_id = cats[0].catalog.id.to_string();

    let test_uri = format!("/engines/{engine_id}/catalogs/{cat_id}/test");

    // A canned FINISHED response with no nextUri/error → ok:true.
    fake.set_coordinator_response(200, br#"{"id":"q1","stats":{"state":"FINISHED"}}"#.to_vec());
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &test_uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "test catalog; body={j}");
    assert_eq!(j["ok"], true, "finished with no error → ok; body={j}");

    // A canned error body → ok:false + the error message surfaced verbatim.
    fake.set_coordinator_response(
        200,
        br#"{"id":"q2","stats":{"state":"FAILED"},"error":{"message":"catalog not found"}}"#
            .to_vec(),
    );
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &test_uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "test catalog error; body={j}");
    assert_eq!(j["ok"], false, "error body → not ok; body={j}");
    assert_eq!(
        j["message"], "catalog not found",
        "error message surfaced; body={j}"
    );

    // A non-2xx coordinator response → ok:false with a status/body summary,
    // never a 502 (this route's contract is always 200 + a verdict body).
    fake.set_coordinator_response(503, b"<html>service unavailable</html>".to_vec());
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &test_uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "test catalog non-2xx; body={j}");
    assert_eq!(j["ok"], false, "non-2xx → not ok; body={j}");
    assert!(
        j["message"].as_str().unwrap().contains("503"),
        "message surfaces coordinator status; body={j}"
    );

    // A catalog id that belongs to a DIFFERENT engine must 404 (ownership guard).
    let other_name = format!("trino-{}", Uuid::new_v4().simple());
    let other_wh = seed_provisioned_warehouse(&f).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": other_name, "warehouseId": other_wh,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create other engine; body={j}");
    let other_engine_id = j["id"].as_str().expect("other engine id").to_string();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{other_engine_id}/catalogs/{cat_id}/test"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "catalog from a different engine must 404"
    );
}

/// PDP-backed engine authz (this seam migrated onto
/// `egret_core::authz::require`): a non-`platform_admin` principal granted
/// `engine:read` through a platform-scoped role — resolved by a real
/// `CedarPdp`, not the allow-all stub — can GET /engines (200) but is denied
/// POST /engines (403, it lacks `engine:create`). The 403 fires on the
/// handler's first line, before any warehouse lookup, so the body UUIDs need
/// not resolve.
#[tokio::test]
async fn pdp_backed_engine_reader_reads_but_cannot_create() {
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // Seed the reader's `engine:read` platform role BEFORE the PDP snapshots the
    // roles catalog, then swap in the real Cedar PDP.
    let reader = session_for_with_permission(&f, "pdp_engine_reader", "engine:read").await;
    attach_real_pdp(&mut f).await;

    // GET /engines → 200 (the PDP allows engine:read at platform scope).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/engines", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PDP-backed engine:read must list engines; body={body}"
    );

    // POST /engines → 403 (the same principal lacks engine:create).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &reader,
            Some(serde_json::json!({
                "name": format!("trino-{}", Uuid::new_v4().simple()),
                "warehouseId": Uuid::new_v4(),
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "PDP-backed engine:read alone must not create an engine (needs engine:create)"
    );
}

/// M6 (SP1 Part B final review): `rotate_engine_secrets` had no authz test —
/// deleting its `require(..., "engine:update")` call failed nothing. Mirrors
/// `pdp_backed_engine_reader_reads_but_cannot_create`: a real PDP-backed
/// `engine:read`-only principal must be refused. The check is the handler's
/// first line, before any engine lookup, so an unknown id is enough to prove
/// it fires.
#[tokio::test]
async fn pdp_backed_engine_reader_cannot_rotate_secrets() {
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let reader = session_for_with_permission(&f, "pdp_engine_reader_rotate", "engine:read").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/rotate-secrets", Uuid::new_v4()),
            &reader,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "PDP-backed engine:read alone must not rotate secrets (needs engine:update)"
    );
}

#[tokio::test]
async fn create_and_get_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // POST /engines → 201, auto-wired to the provisioned warehouse.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 2,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    assert_eq!(j["name"], serde_json::json!(name), "engine name; body={j}");
    assert_eq!(j["type"], "trino", "engine type; body={j}");
    assert_eq!(j["status"], "provisioning", "engine status; body={j}");
    assert_eq!(
        j["spec"]["workerReplicas"], 2,
        "engine worker replicas; body={j}"
    );
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // GET /engines/{id} → 200 with live phase "running" from the fake provisioner.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(j["status"]["phase"], "running", "live phase; body={j}");
    assert_eq!(j["status"]["pods"][0]["ready"], true, "pod ready; body={j}");
    // The wizard inputs seeded a per-role trinoConfig on the spec.
    assert_eq!(
        j["spec"]["trinoConfig"]["coordinator"]["replicas"], 1,
        "seeded coordinator replicas; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["worker"]["replicas"], 2,
        "seeded worker replicas; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["coordinator"]["resources"]["cpuLimit"], "2",
        "seeded coordinator cpu limit; body={j}"
    );
}

/// `POST /engines` with `kind: "starrocks"` persists a `"starrocks"`-type
/// engine row (not the back-compat-default `"trino"`), auto-wires a PRIMARY
/// `egret-warehouse` catalog row the same way the Trino path does (T6 — the
/// runtime `CREATE EXTERNAL CATALOG` DDL itself is smoke-only, exercised by a
/// spawned best-effort background task this test does not wait on), and still
/// calls through to the fake provisioning port (the create path selects the
/// StarRocks spec-building branch).
#[tokio::test]
async fn create_engine_starrocks_persists_starrocks_type() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin_sr", true).await;
    let wh_id = seed_provisioned_warehouse_with_secret(&f).await;
    let name = format!("sr-{}", Uuid::new_v4().simple());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "kind": "starrocks",
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
                "starrocks": {
                    "feReplicas": 1,
                    "cnReplicas": 2,
                },
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create starrocks engine; body={j}"
    );
    assert_eq!(j["name"], serde_json::json!(name), "engine name; body={j}");
    assert_eq!(j["type"], "starrocks", "engine type; body={j}");
    assert_eq!(j["status"], "provisioning", "engine status; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // The persisted row's type is "starrocks" (not the default "trino"),
    // confirmed straight off the store (not just the DTO projection).
    let (db_type,): (String,) =
        sqlx::query_as("SELECT type FROM engine_mgmt.engines WHERE id = $1")
            .bind(Uuid::parse_str(&engine_id).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .expect("fetch engine row");
    assert_eq!(db_type, "starrocks", "stored engine type");

    // A Catalog Client IS minted + a PRIMARY `egret-warehouse` catalog row
    // recorded for StarRocks now (T6), mirroring the Trino auto-wire exactly.
    let cats = registry(&f)
        .list_engine_attachments(Uuid::parse_str(&engine_id).unwrap())
        .await
        .expect("list attachments");
    assert_eq!(cats.len(), 1, "one auto-wired catalog for starrocks");
    assert_eq!(
        cats[0].catalog.warehouse_id,
        Some(wh_id),
        "an egret-warehouse catalog names its warehouse in the WAREHOUSE_ID column, \
         not `kind` (that stays the typed catalog kind, e.g. `iceberg`)"
    );
    assert!(cats[0].is_primary(), "catalog is primary");
    // D5: an engine reaches its warehouse under the warehouse catalog's name.
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    assert_eq!(cats[0].catalog.name, wh_cat.name, "catalog name");
    assert_eq!(
        cats[0].catalog.id, wh_cat.id,
        "legacy id is the registry id"
    );

    // The client's credential lives in a named secret on the attachment and is
    // revoked when the engine is deleted, same as Trino's; confirm it was minted.
    let (client_count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM catalog.catalog_clients WHERE warehouse_id = $1 AND name = $2",
    )
    .bind(wh_id)
    .bind(format!("engine-{name}"))
    .fetch_one(&f.db.pool)
    .await
    .expect("count catalog_clients");
    assert_eq!(client_count, 1, "catalog client minted for starrocks");

    // PUT (update) on a StarRocks engine is now functional: it merges the body
    // over the stored spec (`build_starrocks_spec`) and re-provisions via the
    // fake port. A config-only PUT that leaves the delegation block untouched
    // succeeds without re-bootstrapping/re-materializing.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 2,
                "workerReplicas": 2,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PUT on a starrocks engine re-provisions and succeeds"
    );

    // POST /restart on a StarRocks engine now rolls the FE/CN pods via the CR
    // annotation merge-patch (the fake port records the call), so it succeeds.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/restart"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "restart on a starrocks engine now rolls FE/CN pods"
    );
}

/// An unrecognized `kind` on create is rejected with 400 (rather than silently
/// falling through to the Trino path).
#[tokio::test]
async fn create_engine_unknown_kind_400() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin_badkind", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": format!("x-{}", Uuid::new_v4().simple()),
                "warehouseId": wh_id,
                "kind": "presto",
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown engine kind must 400"
    );
}

#[tokio::test]
async fn pod_logs_known_pod_ok_unknown_pod_404() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_pod_logs_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // The fake provisioner's `status` reports a single pod named "coordinator-0".
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/pods/coordinator-0/logs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "known pod logs; body={j}");
    assert_eq!(
        j["logs"], "logs for coordinator-0 (fake)\n",
        "canned logs; body={j}"
    );

    // A pod name that isn't in the engine's live pod list is a 404, not a
    // pass-through to the kube client.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/pods/not-my-pod/logs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "unknown pod must 404");
}

// NOTE: no `pod_exec` route test here. `axum::extract::ws::WebSocketUpgrade`'s
// `FromRequestParts` unconditionally rejects with 426 Upgrade Required unless
// `hyper::upgrade::OnUpgrade` is present in the request extensions — an
// extension only a real hyper connection ever inserts. `tower::ServiceExt::
// oneshot`-driven requests (as used throughout this file) never have it, so a
// WS-upgrade request 426s before the handler body (and therefore the
// ownership guard) ever runs, regardless of pod name — there is no way to
// distinguish "unknown pod" from "known pod" through this harness. The guard
// itself is the exact same construct as `pod_logs`'s (already covered by
// `pod_logs_known_pod_ok_unknown_pod_404` above), so its correctness is not
// unverified — only the WS-specific wiring around it is untested here.

/// `POST /engines/{id}/restart` rolls the engine's pods without changing
/// config: the route calls `EngineProvisioningPort::restart` with the engine's
/// namespace + name (from which the real egret-bin adapter derives BOTH the
/// `{name}-coordinator` and `{name}-worker` Deployment names — see
/// `fake_kube_restart_deployment_records_coordinator_and_worker` in
/// `egret-provisioner` for that fan-out). An unknown engine id is a 404.
#[tokio::test]
async fn restart_engine_calls_provisioner_unknown_404() {
    let fake = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(fake.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_restart_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();
    let namespace = j["namespace"]
        .as_str()
        .expect("engine namespace")
        .to_string();

    // Restart the known engine → 200, and the fake provisioner recorded the
    // call with the engine's namespace + name.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/restart"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "restart known engine");
    assert_eq!(
        *fake.restarted.lock().unwrap(),
        vec![(namespace, name)],
        "restart recorded with engine namespace + name"
    );

    // An unknown engine id is a 404, not a pass-through to the provisioner.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/restart", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unknown engine must 404"
    );
    assert_eq!(
        fake.restarted.lock().unwrap().len(),
        1,
        "unknown-engine 404 must not call the provisioner"
    );
}

#[tokio::test]
async fn update_engine_per_role_trino_config_roundtrips() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_update_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT a full per-role trinoConfig (coordinator heap + worker graceful shutdown).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
                "trinoConfig": {
                    "coordinator": { "replicas": 1, "heap": { "maxSize": "3G" } },
                    "worker": {
                        "replicas": 1,
                        "gracefulShutdown": { "enabled": true, "gracePeriodSeconds": 30 }
                    }
                }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update engine; body={j}");

    // GET reflects the persisted per-role config.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(
        j["spec"]["trinoConfig"]["coordinator"]["heap"]["maxSize"], "3G",
        "coordinator heap round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["worker"]["gracefulShutdown"]["enabled"], true,
        "worker graceful shutdown round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["worker"]["gracefulShutdown"]["gracePeriodSeconds"], 30,
        "worker grace period round-trips; body={j}"
    );
}

/// Seed a global storage_connection with a resolvable secret_ref; returns its id.
async fn seed_exchange_storage_conn(f: &Fixture) -> Uuid {
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    );
    let secret_id = secrets
        .create(
            None,
            &format!("exchange-secret-{}", Uuid::new_v4().simple()),
            None,
            "s3cr3t",
        )
        .await
        .expect("create exchange secret");
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key, endpoint, secret_ref, path_style) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(format!("exchange-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-exchange-test")
    .bind("AKIATEST")
    .bind("http://rustfs:9000")
    .bind(secret_id)
    .bind(true)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert exchange storage connection");
    id
}

#[tokio::test]
async fn update_engine_resource_groups_and_fault_tolerant() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_rg_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let exchange_conn = seed_exchange_storage_conn(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT resourceGroups (root `global` + selector {group:"global"}) + faultTolerant.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "resourceGroups": {
                "rootGroups": [{
                    "name": "global", "softMemoryLimit": "80%",
                    "hardConcurrencyLimit": 50, "maxQueued": 100
                }],
                "selectors": [{ "group": "global" }]
            },
            "faultTolerant": {
                "retryPolicy": "TASK",
                "exchangeStorageConnId": exchange_conn,
                "exchangePrefix": "exchange"
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "update engine with rg + ft; body={j}"
    );

    // GET reflects the persisted resourceGroups.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(
        j["spec"]["trinoConfig"]["resourceGroups"]["rootGroups"][0]["name"], "global",
        "resource group round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["faultTolerant"]["retryPolicy"], "TASK",
        "fault-tolerant round-trips; body={j}"
    );

    // PUT with a dangling selector group → 400.
    let bad_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "resourceGroups": {
                "rootGroups": [{
                    "name": "global", "softMemoryLimit": "80%",
                    "hardConcurrencyLimit": 50, "maxQueued": 100
                }],
                "selectors": [{ "group": "nope" }]
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(bad_body),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "dangling resource-group selector must 400"
    );
}

/// Create a global named secret holding `value`; returns its id.
async fn seed_secret(f: &Fixture, value: &str) -> Uuid {
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    );
    secrets
        .create(
            None,
            &format!("pw-secret-{}", Uuid::new_v4().simple()),
            None,
            value,
        )
        .await
        .expect("create password secret")
}

/// The app's named-secret store, built with the SAME test crypto key as
/// `build_app`, so a test can read back what the handlers sealed.
fn secret_store(f: &Fixture) -> egret_secrets::WorkspaceSecretStore {
    egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    )
}

fn registry(f: &Fixture) -> egret_catalog_registry::RegistryStore {
    egret_catalog_registry::RegistryStore::new(f.db.clone())
}

/// The registry catalog of a warehouse (created with the engine that uses it).
async fn warehouse_catalog(f: &Fixture, wh_id: Uuid) -> egret_catalog_registry::CatalogRow {
    registry(f)
        .get_catalog_for_warehouse(wh_id)
        .await
        .expect("read catalog")
        .expect("warehouse has a catalog")
}

/// `(internal_secret_id, monitor_secret_id)` straight off the engine row.
async fn engine_secret_ids(f: &Fixture, engine_id: Uuid) -> (Option<Uuid>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT internal_secret_id, monitor_secret_id FROM engine_mgmt.engines WHERE id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("engine secret ids")
}

/// `POST /engines` for a Trino engine with the wizard defaults and the given
/// `trinoConfig`; returns the status and body.
async fn create_engine_via_api(
    f: &Fixture,
    token: &str,
    name: &str,
    wh_id: Uuid,
    trino_config: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "trinoConfig": trino_config,
            })),
        ))
        .await
        .unwrap();
    into_json(resp).await
}

/// `TRINO_INTERNAL_SECRET` exactly as the env Secret renders it.
fn rendered_internal_secret(spec: &egret_engine_mgmt_api::EngineSpec) -> String {
    egret_provisioner::trino::trino_env_secret(spec)["stringData"]["TRINO_INTERNAL_SECRET"]
        .as_str()
        .expect("TRINO_INTERNAL_SECRET rendered")
        .to_string()
}

/// The `egret-monitor` bcrypt hash exactly as `password.db` renders it,
/// relabelled `$2y$`→`$2b$` for `bcrypt::verify`.
fn monitor_hash(spec: &egret_engine_mgmt_api::EngineSpec) -> String {
    let cm = egret_provisioner::trino::trino_config_map(spec);
    let db = cm["data"]["password.db"]
        .as_str()
        .expect("password.db rendered");
    let line = db
        .lines()
        .find(|l| l.starts_with("egret-monitor:"))
        .expect("egret-monitor entry");
    line["egret-monitor:".len()..].replacen("$2y$", "$2b$", 1)
}

/// Names of every internal/monitor secret minted for this engine.
async fn internal_secret_names(f: &Fixture, engine_id: Uuid) -> Vec<String> {
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM workspace_secrets WHERE workspace_id IS NULL")
            .fetch_all(&f.db.pool)
            .await
            .expect("list global secret names");
    let (a, b) = (
        format!("engine-internal-{engine_id}-"),
        format!("engine-monitor-{engine_id}-"),
    );
    names
        .into_iter()
        .filter(|n| n.starts_with(&a) || n.starts_with(&b))
        .collect()
}

/// P1: a new Trino engine gets random stored secrets; the rendered env Secret
/// and password.db carry them, not the id-derived values.
#[tokio::test]
async fn create_renders_stored_random_internal_secrets() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id.expect("internal secret recorded"))
        .await
        .unwrap();
    let monitor = secrets
        .resolve_plaintext(None, monitor_id.expect("monitor secret recorded"))
        .await
        .unwrap();

    let spec = port.last_provisioned();
    let rendered = rendered_internal_secret(&spec);
    assert_eq!(rendered, internal);
    assert_ne!(rendered, format!("egret-internal-{id}"));
    let hash = monitor_hash(&spec);
    assert!(bcrypt::verify(&monitor, &hash).unwrap());
    assert!(!bcrypt::verify(format!("egret-monitor-{id}"), &hash).unwrap());
}

/// P1 transition + F1 fix: an engine rendered before this change (no secret
/// ids) adopts stored secrets on its FIRST apply attempt EVEN WHEN that
/// attempt fails. `TrinoProvisioner::apply_all` writes the env Secret and the
/// `password.db` ConfigMap before the Deployments and does not roll those
/// back on a later failure, so a failed apply may already have handed the
/// cluster the new pair — discarding it would risk deleting a value the
/// running pods carry. The row therefore COMMITS the pending pair on both the
/// success and the failure path, and the NEXT apply (successful or not)
/// REUSES that same committed pair rather than minting a third generation.
#[tokio::test]
async fn legacy_engine_adopts_internal_secrets_even_from_a_failed_apply() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_legacy_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let trino_config = serde_json::json!({ "delegationMode": "impersonate", "auth": {} });
    let engine = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &name,
            "trino",
            "egret-engines",
            &serde_json::json!({
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "imageVersion": "470", "trinoConfig": trino_config.clone(),
            }),
            None,
        )
        .await
        .expect("seed legacy engine row");
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": trino_config,
    });

    port.set_fail_update(true);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{}", engine.id),
            &token,
            Some(put_body.clone()),
        ))
        .await
        .unwrap();
    assert!(!resp.status().is_success(), "injected failure must surface");
    let (internal_id, monitor_id) = engine_secret_ids(&f, engine.id).await;
    assert!(
        internal_id.is_some() && monitor_id.is_some(),
        "a failed apply still COMMITS the pending pair — the cluster may already carry it"
    );
    assert_eq!(
        internal_secret_names(&f, engine.id).await.len(),
        2,
        "exactly one generation exists after the failed attempt"
    );

    port.set_fail_update(false);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{}", engine.id),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (internal_id2, monitor_id2) = engine_secret_ids(&f, engine.id).await;
    assert_eq!(
        internal_id2, internal_id,
        "the next apply REUSES the already-committed pair"
    );
    assert_eq!(monitor_id2, monitor_id);
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id2.unwrap())
        .await
        .unwrap();
    let monitor = secrets
        .resolve_plaintext(None, monitor_id2.unwrap())
        .await
        .unwrap();
    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), internal);
    assert!(bcrypt::verify(&monitor, &monitor_hash(&spec)).unwrap());
    assert_eq!(
        internal_secret_names(&f, engine.id).await.len(),
        2,
        "no third generation minted — the pair was reused, not rotated"
    );
}

/// Reviewer coverage: a SECOND successful apply on the same (new, this-side-of-
/// the-change) engine reuses the already-committed pair — no third generation
/// minted, ids unchanged.
#[tokio::test]
async fn second_successful_apply_reuses_the_same_internal_secret_pair() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_reuse_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    assert!(internal_id.is_some() && monitor_id.is_some());
    assert_eq!(internal_secret_names(&f, id).await.len(), 2);

    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 2,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": { "delegationMode": "impersonate", "auth": {} },
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let (internal_id2, monitor_id2) = engine_secret_ids(&f, id).await;
    assert_eq!(
        internal_id2, internal_id,
        "ids unchanged across the second apply"
    );
    assert_eq!(monitor_id2, monitor_id);
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        2,
        "no third generation minted"
    );
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id2.unwrap())
        .await
        .unwrap();
    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), internal);
}

/// A jwt-delegation engine has no PASSWORD authenticator (`is_jwt_delegation`
/// skips `inject_monitor_user`, so `password.db` carries no `egret-monitor`
/// entry), but it still needs a `TRINO_INTERNAL_SECRET` for coordinator↔worker
/// traffic — and `resolve_internal_secrets` always mints/commits the pair
/// together, so a monitor secret is still recorded even though nothing
/// references it in the rendered spec.
#[tokio::test]
async fn jwt_delegation_engine_mints_and_commits_internal_secret_without_a_monitor_user() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_jwt_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "jwt" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id.expect("internal secret recorded"))
        .await
        .unwrap();
    monitor_id.expect("monitor secret is still minted+committed alongside the internal one");

    let spec = port.last_provisioned();
    assert_eq!(rendered_internal_secret(&spec), internal);
    let cm = egret_provisioner::trino::trino_config_map(&spec);
    assert!(
        cm["data"].get("password.db").is_none(),
        "jwt-only engines render no password.db at all (no PASSWORD authenticator users)"
    );
}

#[tokio::test]
async fn update_engine_auth_and_ingress() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_auth_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let secret_id = seed_secret(&f, "s3cret").await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT auth (password user `bob` → seeded secret) + ingress → 200. The control
    // plane resolves the secret + bcrypts it for provisioning; the spec persists
    // the RAW secretRef.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "auth": {
                "passwordUsers": [{ "username": "bob", "secretRef": secret_id }],
                "jwt": { "requiredAudience": "trino" }
            },
            "ingress": { "enabled": true, "className": "nginx", "host": "h.local" }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "update engine auth+ingress; body={j}"
    );

    // GET reflects the usernames + host — but NOT the plaintext password.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(
        j["spec"]["trinoConfig"]["auth"]["passwordUsers"][0]["username"], "bob",
        "auth username round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["ingress"]["host"], "h.local",
        "ingress host round-trips; body={j}"
    );
    let raw = j["spec"]["trinoConfig"].to_string();
    assert!(
        !raw.contains("s3cret"),
        "plaintext password must not be persisted; body={j}"
    );

    // PUT with a bogus secretRef → 400 (secret cannot be resolved for bcrypt).
    let bad_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "auth": {
                "passwordUsers": [{ "username": "bob", "secretRef": Uuid::new_v4() }]
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(bad_body),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "bogus password secretRef must 400"
    );
}

#[tokio::test]
async fn update_engine_pod_scheduling_roundtrips() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_pod_sched_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT a trinoConfig with per-role nodeSelector + affinity.rawOverride, a
    // cluster serviceAccount, and an init container → 200.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": {
                "replicas": 1,
                "nodeSelector": { "disktype": "ssd" },
                "affinity": { "rawOverride": { "custom": true } }
            },
            "worker": { "replicas": 1 },
            "serviceAccount": "sa",
            "initContainers": [{ "name": "wait", "image": "busybox" }]
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "update engine pod scheduling; body={j}"
    );

    // GET reflects the persisted pod/scheduling fields.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    let tc = &j["spec"]["trinoConfig"];
    assert_eq!(
        tc["coordinator"]["nodeSelector"]["disktype"], "ssd",
        "per-role nodeSelector round-trips; body={j}"
    );
    assert_eq!(
        tc["serviceAccount"], "sa",
        "cluster serviceAccount round-trips; body={j}"
    );
    assert_eq!(
        tc["initContainers"][0]["name"], "wait",
        "init container round-trips; body={j}"
    );
    assert_eq!(
        tc["coordinator"]["affinity"]["rawOverride"]["custom"], true,
        "raw affinity override round-trips verbatim; body={j}"
    );
}

#[tokio::test]
async fn update_engine_autoscaling_roundtrips() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_autoscale_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT a trinoConfig with HPA autoscaling → 200.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "autoscaling": {
                "mode": "hpa",
                "hpa": { "maxReplicas": 5, "targetCpuPercent": 60 }
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update engine hpa; body={j}");

    // GET reflects the HPA autoscaling block.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    let asc = &j["spec"]["trinoConfig"]["autoscaling"];
    assert_eq!(asc["mode"], "hpa", "autoscaling mode round-trips; body={j}");
    assert_eq!(
        asc["hpa"]["maxReplicas"], 5,
        "hpa maxReplicas round-trips; body={j}"
    );

    // PUT a KEDA config with a cron trigger → 200; GET reflects the trigger kind.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "autoscaling": {
                "mode": "keda",
                "keda": {
                    "minReplicaCount": 0, "maxReplicaCount": 4,
                    "triggers": [{
                        "kind": "cron",
                        "cron": { "start": "0 8 * * *", "end": "0 20 * * *", "desiredReplicas": 3, "timezone": "UTC" }
                    }]
                }
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update engine keda; body={j}");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    let asc = &j["spec"]["trinoConfig"]["autoscaling"];
    assert_eq!(asc["mode"], "keda", "keda mode round-trips; body={j}");
    assert_eq!(
        asc["keda"]["triggers"][0]["kind"], "cron",
        "keda trigger kind round-trips; body={j}"
    );
}

#[tokio::test]
async fn create_engine_unprovisioned_warehouse_400() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin_unprov", true).await;
    let wh_id = seed_unprovisioned_warehouse(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": format!("trino-{}", Uuid::new_v4().simple()),
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unprovisioned warehouse must 400"
    );
}

#[tokio::test]
async fn scoped_permission_engines_read_and_create() {
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // These 403/200 assertions gate on `state.pdp`; seed all ad-hoc scoped roles
    // BEFORE building the Cedar PDP (it snapshots the roles catalog at build),
    // then swap in a real PDP so the allow-all stub can't mask the scoped grants.
    let wrong = session_for_with_permission(&f, "engine_scoped_wrong", "storage:read").await;
    let reader = session_for_with_permission(&f, "engine_scoped_reader", "engine:read").await;
    let creator = session_for_with_permission(&f, "engine_scoped_creator", "engine:create").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/engines", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without engine:read must not list engines"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/engines", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped engine:read principal must list engines; body={body}"
    );

    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_body = serde_json::json!({
        "name": format!("trino-{}", Uuid::new_v4().simple()),
        "warehouseId": wh_id,
        "coordinatorReplicas": 1,
        "workerReplicas": 1,
        "cpuMin": "1",
        "cpuMax": "2",
        "memory": "4Gi",
    });

    // engine:read alone must not create.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/engines", &reader, Some(engine_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "engine:read alone must not create an engine"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/engines", &creator, Some(engine_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped engine:create principal must create; body={body}"
    );
}

#[tokio::test]
async fn scoped_permission_monitor_queries_and_cluster() {
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The monitor gates consult `state.pdp`; seed the admin + scoped roles BEFORE
    // building the Cedar PDP (it snapshots the roles catalog at build), then swap
    // in a real PDP so the under-privileged 403s are genuine (not allow-all).
    let admin_token = session_for(&f, "monitor_scoped_setup", true).await;
    let wrong = session_for_with_permission(&f, "monitor_scoped_wrong", "storage:read").await;
    let reader = session_for_with_permission(&f, "monitor_scoped_reader", "engine:read").await;
    attach_real_pdp(&mut f).await;

    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &admin_token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    fake.set_coordinator_response(200, b"[]".to_vec());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without engine:read must not list queries"
    );
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/cluster"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without engine:read must not fetch cluster state"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped engine:read principal must list queries; body={body}"
    );
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/cluster"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped engine:read principal must fetch cluster state; body={body}"
    );
}

// ── engine ↔ workspace assignment (P1) ─────────────────────────────────────────

/// Seed a JWT-capable Trino engine (`spec.trinoConfig.auth.jwt` object); returns
/// its id. Its warehouse is no longer a column here — the assignment routes
/// this seeds for don't read it, so this bare row needs no primary attachment.
async fn seed_jwt_engine(f: &Fixture) -> Uuid {
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let spec = serde_json::json!({
        "trinoConfig": { "auth": { "jwt": { "jwks_uri": "http://kc/jwks" } } }
    });
    store
        .create_engine(
            &format!("trino-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &spec,
            None,
        )
        .await
        .expect("seed jwt engine")
        .id
}

/// Seed a bare active workspace (assignment target); returns its id.
async fn seed_workspace(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("assign-ws-{}", Uuid::new_v4().simple()))
    .bind("Assignment Target Workspace")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert workspace");
    ws_id
}

/// Count materialized `query.databases` rows for an (workspace, engine) pair.
async fn materialized_count(f: &Fixture, ws: Uuid, engine_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM query.databases WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("count materialized")
}

/// CP `PUT /engines/{id}/workspaces {workspaceIds:[ws]}` records the assignment
/// AND materializes a `query.databases` row; `GET` returns exactly `[ws]`; an
/// empty `PUT` unassigns and removes the materialized row.
#[tokio::test]
async fn cp_set_engine_workspaces_assigns_and_materializes() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_assign_admin", true).await;
    let _wh_id = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let ws = seed_workspace(&f).await;

    // PUT the full set {ws} → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "set workspaces; body={j}");
    assert_eq!(j.as_array().unwrap().len(), 1, "one assigned ws; body={j}");
    assert_eq!(
        j[0]["workspaceId"],
        ws.to_string(),
        "assigned ws id; body={j}"
    );

    // GET reflects the assignment.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list workspaces; body={j}");
    assert_eq!(
        j[0]["workspaceId"],
        ws.to_string(),
        "listed ws id; body={j}"
    );

    // The delegated connection was materialized.
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        1,
        "a query.databases row must be materialized for the assignment"
    );

    // PUT the empty set → unassign; GET is empty; the materialized row is gone.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "clear workspaces; body={j}");
    assert!(
        j.as_array().unwrap().is_empty(),
        "no assignments left; body={j}"
    );
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        0,
        "the materialized row must be removed on unassign"
    );

    // Unknown engine → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{}/workspaces", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unknown engine must 404"
    );
}

/// Workspace-admin `POST /workspaces/{id}/engines {engineId}` assigns; `GET`
/// shows the engine; `DELETE /workspaces/{id}/engines/{engine_id}` removes it.
#[tokio::test]
async fn workspace_admin_assign_list_unassign_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ws_engine_admin", true).await;
    let _wh_id = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let ws = seed_workspace(&f).await;

    // Initially empty.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/workspaces/{ws}/engines"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list ws engines; body={j}");
    assert!(j.as_array().unwrap().is_empty(), "no engines yet; body={j}");

    // POST assigns → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/workspaces/{ws}/engines"),
            &token,
            Some(serde_json::json!({ "engineId": engine_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "assign engine");

    // GET now shows the engine with its DTO fields.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/workspaces/{ws}/engines"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list ws engines; body={j}");
    assert_eq!(j.as_array().unwrap().len(), 1, "one engine; body={j}");
    assert_eq!(j[0]["id"], engine_id.to_string(), "engine id; body={j}");
    assert_eq!(j[0]["type"], "trino", "engine type; body={j}");
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        1,
        "assignment materialized a connection"
    );

    // DELETE unassigns → 204; GET is empty again.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/workspaces/{ws}/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "unassign engine");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/workspaces/{ws}/engines"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list ws engines; body={j}");
    assert!(j.as_array().unwrap().is_empty(), "engine removed; body={j}");
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        0,
        "unassign removed the materialized connection"
    );
}

// ── engine delete (SP1 Part A) ─────────────────────────────────────────────────

/// The stored lifecycle status, or `None` when the row is gone.
async fn engine_status(f: &Fixture, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM engine_mgmt.engines WHERE id = $1")
        .bind(id)
        .fetch_optional(&f.db.pool)
        .await
        .expect("read engine status")
}

/// The materialized connection for (workspace, engine).
async fn materialized_db_id(f: &Fixture, ws: Uuid, engine_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM query.databases WHERE workspace_id = $1 AND engine_id = $2")
        .bind(ws)
        .bind(engine_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("materialized connection")
}

/// `PUT /engines/{id}/workspaces` → 200.
async fn put_workspaces(f: &Fixture, token: &str, engine_id: Uuid, ws: &[Uuid]) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            token,
            Some(serde_json::json!({ "workspaceIds": ws })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "put workspaces; body={j}");
}

/// Seed one blocking dependent of `kind` (a `dependents::EngineDependentKind`
/// wire name) called `name` on connection `database_id`.
async fn seed_dependent(f: &Fixture, ws: Uuid, database_id: Uuid, kind: &str, name: &str) {
    match kind {
        "dataapi_endpoint" | "dataapi_endpoint_trashed" => {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO dataapi.endpoints (workspace_id, name, path, method, database_id, statements, state) \
                 VALUES ($1, $2, $3, 'GET', $4, \
                         jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), 'draft') \
                 RETURNING id",
            )
            .bind(ws)
            .bind(name)
            .bind(format!("/{name}"))
            .bind(database_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("seed endpoint");
            if kind == "dataapi_endpoint_trashed" {
                sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&f.db.pool)
                    .await
                    .expect("trash endpoint");
            }
        }
        "schedule" => {
            sqlx::query(
                "INSERT INTO automation.schedules (workspace_id, type, name, crontab, database_id, sql, validator_type) \
                 VALUES ($1, 'alert', $2, '0 9 * * *', $3, 'SELECT 1', 'not_null')",
            )
            .bind(ws)
            .bind(name)
            .bind(database_id)
            .execute(&f.db.pool)
            .await
            .expect("seed alert");
        }
        "dataset" => {
            sqlx::query(
                "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
                 VALUES ($1, $2, $3, 't')",
            )
            .bind(ws)
            .bind(database_id)
            .bind(name)
            .execute(&f.db.pool)
            .await
            .expect("seed dataset");
        }
        other => panic!("unknown dependent kind {other}"),
    }
}

/// D1/D2/D8: one blocking dependent → 409 naming it; the cluster and the row untouched.
async fn assert_engine_delete_blocked_by(kind: &str) {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_blocked", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let ws = seed_workspace(&f).await;
    put_workspaces(&f, &token, engine_id, &[ws]).await;
    let db = materialized_db_id(&f, ws, engine_id).await;
    seed_dependent(&f, ws, db, kind, "orders-by-day").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "kind={kind}; body={j}");
    assert_eq!(j["code"], "engine_has_dependents", "body={j}");
    let deps = j["dependents"].as_array().expect("dependents array");
    assert_eq!(deps.len(), 1, "body={j}");
    assert_eq!(deps[0]["workspaceId"], ws.to_string(), "body={j}");
    assert_eq!(deps[0]["kind"], kind, "body={j}");
    assert_eq!(deps[0]["count"], 1, "body={j}");
    assert_eq!(
        deps[0]["sample"],
        serde_json::json!(["orders-by-day"]),
        "body={j}"
    );
    assert!(
        port.deleted.lock().unwrap().is_empty(),
        "the cluster must not be touched"
    );
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("provisioning"),
        "the row must survive with its status unchanged"
    );
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        1,
        "the connection survives"
    );
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_data_api_endpoint() {
    assert_engine_delete_blocked_by("dataapi_endpoint").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_trashed_data_api_endpoint() {
    assert_engine_delete_blocked_by("dataapi_endpoint_trashed").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_sql_alert() {
    assert_engine_delete_blocked_by("schedule").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_dataset() {
    assert_engine_delete_blocked_by("dataset").await;
}

/// D7: create refuses without Kubernetes, so delete must too — deleting only the
/// row would orphan the cluster.
#[tokio::test]
async fn delete_engine_without_kubernetes_is_400_and_keeps_the_row() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_nokube", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("kubernetes not configured"),
        "body={j}"
    );
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("provisioning")
    );
}

/// D4/D5/D6: a failed cluster delete is a 502 with the cause, leaves a visible
/// `deleting` row, and the same request succeeds once the cluster answers.
#[tokio::test]
async fn delete_engine_cluster_failure_is_502_deleting_and_retryable() {
    let port = FakeEnginePort::default();
    *port.delete_error.lock().unwrap() = Some("apiserver: 500 Internal Server Error".into());
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_retry", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let uri = format!("/engines/{engine_id}");

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert_eq!(j["code"], "engine_cluster_delete_failed", "body={j}");
    assert!(
        j["error"].as_str().unwrap_or("").contains("apiserver: 500"),
        "body={j}"
    );
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("deleting")
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(
        j["status"]["phase"], "deleting",
        "stored status must not be masked; body={j}"
    );

    *port.delete_error.lock().unwrap() = None;
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "retry succeeds");
    assert_eq!(
        engine_status(&f, engine_id).await,
        None,
        "row gone after the retry"
    );
}

/// Happy path: 204, cluster deleted once, engine-generated secrets gone, the
/// minted catalog client revoked, a client we did NOT mint left alone.
#[tokio::test]
async fn delete_engine_removes_cluster_row_secrets_and_minted_clients_only() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_ok", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id: Uuid = j["id"].as_str().unwrap().parse().unwrap();

    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto)),
    );
    let delegator = secrets
        .create(
            None,
            &format!("test-delegator-{}", Uuid::new_v4().simple()),
            None,
            "pw",
        )
        .await
        .unwrap();
    let root = secrets
        .create(
            None,
            &format!("test-root-{}", Uuid::new_v4().simple()),
            None,
            "pw",
        )
        .await
        .unwrap();
    store
        .set_delegator_secret(engine_id, delegator)
        .await
        .unwrap();
    store.set_root_secret(engine_id, root).await.unwrap();
    let engine = store.get_engine(engine_id).await.unwrap().unwrap();
    let mut engine_secrets = vec![delegator, root];
    engine_secrets.extend(engine.tls_secret_id);

    let reg = registry(&f);
    let primary = reg
        .get_attachment(engine_id, warehouse_catalog(&f, wh_id).await.id)
        .await
        .unwrap()
        .expect("primary attachment");
    let minted = primary.catalog_client_id.expect("minted client");
    engine_secrets.push(primary.credential_secret_id.expect("credential secret"));

    // A second warehouse's catalog, attached with a client the engine did NOT mint.
    let foreign_wh = seed_provisioned_warehouse(&f).await;
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .get_warehouse(foreign_wh)
        .await
        .unwrap()
        .unwrap();
    let role = catalog_store
        .create_role(
            foreign_wh,
            "readonly_analyst",
            "external",
            Some("read-only role"),
        )
        .await
        .unwrap();
    let foreign = catalog_store
        .create_client(&egret_catalog_engine::NewClient {
            name: "not-minted-by-the-engine".into(),
            consumer_type: "external".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: foreign_wh,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    // Its credential is the admin's (not minted by this engine): it must
    // survive the engine delete, like the client it belongs to.
    let foreign_cred = secrets
        .create(
            None,
            &format!("foreign-cred-{}", Uuid::new_v4().simple()),
            None,
            "x:y",
        )
        .await
        .unwrap();
    let (foreign_cat, _) = reg
        .ensure_warehouse_catalog(
            &egret_catalog_registry::WarehouseIdentity {
                id: foreign_wh,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            None,
        )
        .await
        .unwrap();
    reg.insert_attachment(&egret_catalog_registry::NewAttachment {
        engine_id,
        catalog_id: foreign_cat.id,
        catalog_client_id: Some(foreign.id),
        credential_secret_id: Some(foreign_cred),
        minted_client: false,
        applied_now: true,
        is_primary: false,
        created_at: None,
        apply_status: egret_catalog_registry::store::apply_status::APPLIED,
    })
    .await
    .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body={j}");

    assert_eq!(
        *port.deleted.lock().unwrap(),
        vec![(
            "trino".to_string(),
            "egret-engines".to_string(),
            name.clone()
        )]
    );
    assert_eq!(engine_status(&f, engine_id).await, None, "row gone");
    let secrets_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_secrets WHERE id = ANY($1)")
            .bind(engine_secrets.clone())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(
        secrets_left, 0,
        "engine-generated secrets {engine_secrets:?} must be deleted"
    );
    let client_count = |id: Uuid| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM catalog.catalog_clients WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        client_count(minted).await,
        0,
        "the minted client is revoked"
    );
    assert_eq!(
        client_count(foreign.id).await,
        1,
        "a client the engine did not mint survives"
    );
    assert!(
        secrets.get_meta(None, foreign_cred).await.is_ok(),
        "the credential of a client the engine did not mint survives"
    );
}

/// Seed a trino profile named `name` with its `{name}_tag` tag service, as
/// auto-bind creates them, and bind `engines` to it.
async fn seed_ranger_profile(f: &Fixture, name: &str, engines: &[Uuid]) -> i64 {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::ensure_trino_def(&policy)
        .await
        .expect("trino def");
    PolicyPort::ensure_tag_def(&policy).await.expect("tag def");
    let tag = format!("{name}_tag");
    PolicyPort::create_service(&policy, &tag, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let profile = PolicyPort::create_service(
        &policy,
        name,
        "trino",
        &serde_json::json!({ "tagService": tag }),
    )
    .await
    .expect("trino profile");
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    for e in engines {
        store
            .set_engine_authz_service(*e, Some(profile.id))
            .await
            .unwrap();
    }
    profile.id
}

/// Seed a profile of `type_` with NO `tagService`, and no tag service row —
/// the shape the I3 guard refuses.
async fn seed_profile_without_tag_service(f: &Fixture, name: &str, type_: &str) -> i64 {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::ensure_trino_def(&policy)
        .await
        .expect("trino def");
    egret_policy::service_def::seed_starrocks(&policy)
        .await
        .expect("starrocks def");
    PolicyPort::create_service(&policy, name, type_, &serde_json::json!({}))
        .await
        .expect("profile")
        .id
}

/// Seed a profile of `type_`, shaped the way egret's own provisioning shapes
/// that type: a `trino` profile gets a `{name}_tag` tag service and a
/// `tagService` config; a `starrocks` profile gets NEITHER, because the
/// StarRocks Ranger plugin has no tag policies (upstream starrocks#67458) and
/// the compiler expands tag rules into per-column resource policies instead.
async fn seed_typed_profile(f: &Fixture, name: &str, type_: &str) -> i64 {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::ensure_trino_def(&policy)
        .await
        .expect("trino def");
    egret_policy::service_def::seed_starrocks(&policy)
        .await
        .expect("starrocks def");
    if type_ != "trino" {
        return PolicyPort::create_service(&policy, name, type_, &serde_json::json!({}))
            .await
            .expect("profile")
            .id;
    }
    PolicyPort::ensure_tag_def(&policy).await.expect("tag def");
    let tag = format!("{name}_tag");
    PolicyPort::create_service(&policy, &tag, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    PolicyPort::create_service(
        &policy,
        name,
        type_,
        &serde_json::json!({ "tagService": tag }),
    )
    .await
    .expect("profile")
    .id
}

async fn service_exists(f: &Fixture, name: &str) -> bool {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::get_service_opt(&policy, name)
        .await
        .unwrap()
        .is_some()
}

/// D9, by user decision: a profile can hold admin-written policies and nothing
/// records that egret created it, so engine delete unbinds and never deletes.
#[tokio::test]
async fn delete_engine_keeps_its_ranger_profile_and_tag_service() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_keeps_ranger", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let name: String = sqlx::query_scalar("SELECT name FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine_id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let profile_id = seed_ranger_profile(&f, &name, &[engine_id]).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        service_exists(&f, &name).await,
        "the profile survives the engine"
    );
    assert!(
        service_exists(&f, &format!("{name}_tag")).await,
        "and so does its tag service"
    );
    let bound: i64 =
        sqlx::query_scalar("SELECT count(*) FROM engine_mgmt.engines WHERE authz_service_id = $1")
            .bind(profile_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(bound, 0, "no engine is bound to it any more");
}

// ── engine ↔ workspace preflight (SP1 Part A) ────────────────────────────────

/// Every blocking kind refuses an unassign, and nothing changes.
#[tokio::test]
async fn unassign_is_refused_while_the_workspace_uses_the_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "unassign_blocked", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    for kind in [
        "dataapi_endpoint",
        "dataapi_endpoint_trashed",
        "schedule",
        "dataset",
    ] {
        let engine_id = seed_jwt_engine(&f).await;
        let ws = seed_workspace(&f).await;
        put_workspaces(&f, &token, engine_id, &[ws]).await;
        let db = materialized_db_id(&f, ws, engine_id).await;
        seed_dependent(&f, ws, db, kind, "nightly").await;

        let resp = f
            .app
            .clone()
            .oneshot(req(
                "DELETE",
                &format!("/workspaces/{ws}/engines/{engine_id}"),
                &token,
                None,
            ))
            .await
            .unwrap();
        let (status, j) = into_json(resp).await;
        assert_eq!(status, StatusCode::CONFLICT, "kind={kind}; body={j}");
        assert_eq!(j["code"], "engine_has_dependents", "kind={kind}; body={j}");
        assert_eq!(j["dependents"][0]["kind"], kind, "body={j}");
        assert_eq!(
            materialized_count(&f, ws, engine_id).await,
            1,
            "kind={kind}: connection kept"
        );
        let assigned: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engine_workspaces WHERE engine_id = $1 AND workspace_id = $2",
        )
        .bind(engine_id)
        .bind(ws)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
        assert_eq!(assigned, 1, "kind={kind}: assignment kept");
    }
}

/// `PUT {wsB}` over `{wsA}` is one assign + one blocked unassign: nothing applies.
#[tokio::test]
async fn put_workspaces_applies_nothing_when_one_unassign_is_blocked() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "put_ws_blocked", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let ws_a = seed_workspace(&f).await;
    let ws_b = seed_workspace(&f).await;
    put_workspaces(&f, &token, engine_id, &[ws_a]).await;
    let db = materialized_db_id(&f, ws_a, engine_id).await;
    seed_dependent(&f, ws_a, db, "dataapi_endpoint", "orders").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws_b] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    assert_eq!(
        j["dependents"][0]["workspaceId"],
        ws_a.to_string(),
        "body={j}"
    );
    assert_eq!(
        materialized_count(&f, ws_a, engine_id).await,
        1,
        "A untouched"
    );
    assert_eq!(
        materialized_count(&f, ws_b, engine_id).await,
        0,
        "B was NOT assigned first"
    );
}

#[tokio::test]
async fn unknown_workspaces_are_404_and_apply_nothing() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "unknown_ws", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f).await;
    let real = seed_workspace(&f).await;
    let ghost = Uuid::new_v4();

    // PUT with one real and one unknown workspace: 404, the real one not assigned.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [real, ghost] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "PUT unknown workspace (was 500)"
    );
    assert_eq!(materialized_count(&f, real, engine_id).await, 0);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/workspaces/{ghost}/engines"),
            &token,
            Some(serde_json::json!({ "engineId": engine_id })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "assign to unknown workspace (was 500)"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/workspaces/{ghost}/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unassign from unknown workspace (was 204)"
    );
}

async fn stored_status(f: &Fixture, engine_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM engine_mgmt.engines WHERE id = $1")
        .bind(Uuid::parse_str(engine_id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("engine status")
}

/// P6: `GET /engines/{id}` records `running` once live status reports every pod
/// ready, and never overwrites `deleting`. Also (I1: `FakeEnginePort::status`
/// used to be hard-coded to always report `running`/ready, so the guard at
/// `get_engine` gating the write on `s.phase == RUNNING` could never actually
/// be exercised) — a not-ready live status must leave a `provisioning` row
/// alone.
#[tokio::test]
async fn get_engine_records_running_and_leaves_deleting_alone() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p6_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = j["id"].as_str().unwrap().to_string();
    assert_eq!(stored_status(&f, &id).await, "provisioning");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        stored_status(&f, &id).await,
        "running",
        "all pods ready ⇒ running recorded"
    );

    let name2 = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name2, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create 2; body={j}");
    let id2 = j["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE engine_mgmt.engines SET status = 'deleting' WHERE id = $1")
        .bind(Uuid::parse_str(&id2).unwrap())
        .execute(&f.db.pool)
        .await
        .unwrap();
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{id2}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_status(&f, &id2).await, "deleting");

    // A crash-looping (not-ready) live status must NOT flip a `provisioning`
    // row to `running` — only an all-pods-ready `running` phase may.
    let name3 = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name3, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create 3; body={j}");
    let id3 = j["id"].as_str().unwrap().to_string();
    assert_eq!(stored_status(&f, &id3).await, "provisioning");
    port.set_status(egret_engine_mgmt_api::EngineStatus {
        phase: "provisioning".into(),
        message: Some("waiting for pods".into()),
        pods: vec![egret_engine_mgmt_api::PodStatus {
            name: "coordinator-0".into(),
            phase: "CrashLoopBackOff".into(),
            ready: false,
        }],
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{id3}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        stored_status(&f, &id3).await,
        "provisioning",
        "a not-ready live status must not record running"
    );
}

/// Rotate: `apply_built` commits the pending pair on BOTH the success and the
/// failure path (Task 7's reviewed decision), so a failed apply replaces the
/// row's ids — but F2 (standing rule: never delete a value the cluster may
/// still hold) means it must NOT delete the pair it superseded, because a
/// failure can happen either before or after the cluster actually received
/// it. A LATER successful apply deletes ONLY the one pair its own
/// compare-and-set displaced (never a wider sweep — that could catch a
/// DIFFERENT, concurrently in-flight apply's freshly-minted pair, see
/// `commit_internal_secrets`'s doc comment), so a generation left behind by a
/// failed rotation survives as a permanent orphan until engine delete (Task
/// 15) — `internal_secret_names` count asserted at each step to pin exactly
/// which pairs are deleted and which survive. Non-Trino → 400, unknown → 404.
#[tokio::test]
async fn rotate_secrets_replaces_both_values_only_on_a_successful_apply() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_rotate_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let secrets = secret_store(&f);
    let (old_internal_id, old_monitor_id) = engine_secret_ids(&f, id).await;
    let (old_internal_id, old_monitor_id) = (old_internal_id.unwrap(), old_monitor_id.unwrap());
    let old_internal = secrets
        .resolve_plaintext(None, old_internal_id)
        .await
        .unwrap();
    let old_monitor = secrets
        .resolve_plaintext(None, old_monitor_id)
        .await
        .unwrap();
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        2,
        "baseline: one generation exists"
    );
    let rotate = || {
        req(
            "POST",
            &format!("/engines/{id}/rotate-secrets"),
            &token,
            None,
        )
    };

    port.set_fail_update(true);
    let (status, j) = into_json(f.app.clone().oneshot(rotate()).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap()
            .contains("injected update failure"),
        "body={j}"
    );
    // F2 (standing rule: never delete a value the cluster may still hold): the
    // failed apply still commits the pair it attempted to hand the cluster
    // (the row moves on), but it must NOT delete the pair it superseded — the
    // failure could have happened before OR after the cluster received the
    // new pair, so both must stay resolvable until a later success proves
    // which one the cluster actually converged on.
    let (failed_internal_id, failed_monitor_id) = engine_secret_ids(&f, id).await;
    let (failed_internal_id, failed_monitor_id) =
        (failed_internal_id.unwrap(), failed_monitor_id.unwrap());
    assert_ne!(failed_internal_id, old_internal_id);
    assert_ne!(failed_monitor_id, old_monitor_id);
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        4,
        "both generations on record: the failed apply deleted nothing"
    );
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_internal_id)
            .await
            .expect("the superseded pair must still resolve after a failed apply"),
        old_internal
    );
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_monitor_id)
            .await
            .expect("the superseded pair must still resolve after a failed apply"),
        old_monitor
    );

    port.set_fail_update(false);
    let resp = f.app.clone().oneshot(rotate()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let (new_internal_id, new_monitor_id) = engine_secret_ids(&f, id).await;
    let (new_internal_id, new_monitor_id) = (new_internal_id.unwrap(), new_monitor_id.unwrap());
    assert_ne!(new_internal_id, old_internal_id);
    assert_ne!(new_monitor_id, old_monitor_id);
    assert_ne!(new_internal_id, failed_internal_id);
    assert_ne!(new_monitor_id, failed_monitor_id);
    let new_internal = secrets
        .resolve_plaintext(None, new_internal_id)
        .await
        .unwrap();
    let new_monitor = secrets
        .resolve_plaintext(None, new_monitor_id)
        .await
        .unwrap();
    assert_ne!(new_internal, old_internal);

    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), new_internal);
    let hash = monitor_hash(&spec);
    // bcrypt salts every hash, so "the hash changed" proves nothing; verify.
    assert!(bcrypt::verify(&new_monitor, &hash).unwrap());
    assert!(!bcrypt::verify(&old_monitor, &hash).unwrap());
    // F2 round 2 (re-review): the successful apply's own compare-and-set only
    // displaces the ONE pair it read as `expected` — the failed rotation's
    // pair, since that is what the row named when THIS build started. It must
    // NOT reach further back and delete the original pair too: a wider,
    // prefix-based sweep was tried in round 1 and reverted because it could
    // just as easily catch a DIFFERENT, concurrently in-flight apply's
    // freshly-minted (not-yet-committed) pair — see `commit_internal_secrets`'s
    // doc comment. So the original pair survives as a permanent orphan here;
    // only engine delete (Task 15) is a safe place to sweep it.
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_internal_id)
            .await
            .expect("the original pair is NOT this commit's `expected` — it must survive"),
        old_internal
    );
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_monitor_id)
            .await
            .expect("the original pair is NOT this commit's `expected` — it must survive"),
        old_monitor
    );
    assert!(
        matches!(
            secrets.resolve_plaintext(None, failed_internal_id).await,
            Err(egret_secrets::SecretsError::NotFound)
        ),
        "the failed rotation's pair WAS this commit's `expected` — it is deleted"
    );
    assert!(matches!(
        secrets.resolve_plaintext(None, failed_monitor_id).await,
        Err(egret_secrets::SecretsError::NotFound)
    ));
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        4,
        "the original pair is an orphan (2 rows) alongside the current pair (2 rows) — \
         narrowing the delete to only what this apply superseded means it is NOT swept here"
    );

    let sr = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &format!("sr-{}", Uuid::new_v4().simple()),
            "starrocks",
            "egret-engines",
            &serde_json::json!({ "starrocksConfig": {} }),
            None,
        )
        .await
        .unwrap();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/rotate-secrets", sr.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/rotate-secrets", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn stored_spec(f: &Fixture, id: &str) -> serde_json::Value {
    sqlx::query_scalar("SELECT spec FROM engine_mgmt.engines WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("engine spec")
}

/// P3: a rejected apply restores the stored spec and is a 502 carrying the cause;
/// a body whose trinoConfig omits delegationMode keeps the stored mode.
#[tokio::test]
async fn trino_update_failure_restores_the_spec_and_keeps_delegation_mode() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p3_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = j["id"].as_str().unwrap().to_string();
    let before = stored_spec(&f, &id).await;
    assert_eq!(before["trinoConfig"]["delegationMode"], "jwt");

    port.set_fail_update(true);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 5,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap()
            .contains("injected update failure"),
        "body={j}"
    );
    assert_eq!(stored_spec(&f, &id).await, before, "spec restored");

    port.set_fail_update(false);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "trinoConfig": {
                    "coordinator": { "replicas": 1 },
                    "worker": { "replicas": 1 },
                    "auth": { "jwt": {} }
                }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update; body={j}");
    assert_eq!(
        stored_spec(&f, &id).await["trinoConfig"]["delegationMode"],
        "jwt",
        "omitted delegationMode must keep the stored mode"
    );
}

/// P4: a Trino update re-materializes the connections of assigned workspaces.
#[tokio::test]
async fn trino_update_rematerializes_assigned_workspace_connections() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p4_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let ws = seed_workspace(&f).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws] })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Make the materialized connection stale.
    sqlx::query(
        "UPDATE query.databases SET options = '{}'::jsonb WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(id)
    .execute(&f.db.pool)
    .await
    .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let scheme: Option<String> = sqlx::query_scalar(
        "SELECT options->>'http_scheme' FROM query.databases WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        scheme.as_deref(),
        Some("https"),
        "connection re-materialized"
    );
}

/// P5: removing a catalog applies the cluster WITHOUT it first; a rejected apply
/// leaves the row and its minted client in place so a retry can act. Was
/// `remove_catalog_applies_before_deleting_and_revoking`, driven through the
/// legacy `DELETE /engines/{id}/catalogs/{cid}`; the registry's `DELETE
/// /catalogs/{id}/engines/{engine_id}` is a second door onto the same
/// `detach_from_engine` (see `registry_detach_removes_the_attachment_and_keeps_the_catalog`),
/// but its rejected-apply mapping is `into_registry`, not `into_legacy` — 502
/// (`ClusterApplyFailed`), not 400 (spec §4.4's registry-wide rule, not a
/// per-route choice).
#[tokio::test]
async fn registry_detach_applies_before_deleting_and_revoking() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p5_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let extra_wh = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    let (status, j, cat) = attach_warehouse_via_api(&f, &token, &engine_id, extra_wh).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "add warehouse catalog; body={j}"
    );
    let cat_id = cat.id;
    let cat_name = cat.name;
    let engine_uuid = Uuid::parse_str(&engine_id).unwrap();
    let reg = registry(&f);
    let attachment = reg
        .get_attachment(engine_uuid, cat_id)
        .await
        .unwrap()
        .unwrap();
    let client_id = attachment.catalog_client_id.expect("minted client id");
    let credential = attachment.credential_secret_id.expect("credential secret");
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let remove = || {
        req(
            "DELETE",
            &format!("/catalogs/{cat_id}/engines/{engine_id}"),
            &token,
            None,
        )
    };

    port.set_fail_update(true);
    let resp = f.app.clone().oneshot(remove()).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "rejected apply; {resp:?}"
    );
    assert!(
        port.last_updated()
            .catalogs
            .iter()
            .all(|c| c.name != cat_name),
        "the apply rendered the engine without the catalog"
    );
    assert!(
        reg.get_attachment(engine_uuid, cat_id)
            .await
            .unwrap()
            .is_some(),
        "row kept"
    );
    assert!(
        catalog_store
            .list_clients(extra_wh)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client_id),
        "client not revoked"
    );

    port.set_fail_update(false);
    let resp = f.app.clone().oneshot(remove()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        reg.get_attachment(engine_uuid, cat_id)
            .await
            .unwrap()
            .is_none(),
        "row deleted"
    );
    assert!(
        !catalog_store
            .list_clients(extra_wh)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client_id),
        "minted client revoked"
    );
    assert!(
        matches!(
            secret_store(&f).get_meta(None, credential).await,
            Err(egret_secrets::SecretsError::NotFound)
        ),
        "the minted client's credential secret is deleted with the attachment"
    );
    assert!(
        reg.get_catalog(cat_id).await.unwrap().is_some(),
        "detach keeps the registry catalog"
    );
}

/// P2: a create whose provision fails leaves no engine row and no catalog client,
/// and asks the cluster to delete what it may have applied — for both kinds.
#[tokio::test]
async fn failed_provision_rolls_back_row_client_and_cluster() {
    let port = FakeEnginePort::default();
    port.set_fail_provision(true);
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p2_admin", true).await;
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let row_exists = |name: String| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM engine_mgmt.engines WHERE name = $1)",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    // A minted client's credential secret is `catalog-client-<client id>`; one
    // whose client no longer exists was leaked by a rollback.
    let orphaned_credentials = || {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT s.name FROM workspace_secrets s \
                  WHERE s.workspace_id IS NULL AND left(s.name, 15) = 'catalog-client-' \
                    AND NOT EXISTS (SELECT 1 FROM catalog.catalog_clients c \
                                     WHERE 'catalog-client-' || c.id::text = s.name)",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let orphans_before = orphaned_credentials().await;

    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(!row_exists(name.clone()).await, "Trino row rolled back");
    assert!(
        catalog_store.list_clients(wh_id).await.unwrap().is_empty(),
        "Trino client revoked"
    );
    assert!(port.deleted.lock().unwrap().contains(&(
        "trino".to_string(),
        "egret-engines".to_string(),
        name.clone()
    )));

    let sr_wh = seed_provisioned_warehouse_with_secret(&f).await;
    let sr_name = format!("sr-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": sr_name, "warehouseId": sr_wh, "kind": "starrocks",
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        !row_exists(sr_name.clone()).await,
        "StarRocks row rolled back"
    );
    assert!(
        catalog_store.list_clients(sr_wh).await.unwrap().is_empty(),
        "StarRocks client revoked"
    );
    assert!(port.deleted.lock().unwrap().contains(&(
        "starrocks".to_string(),
        "egret-engines".to_string(),
        sr_name
    )));
    assert_eq!(
        orphaned_credentials().await,
        orphans_before,
        "both rollbacks deleted the credential secret of the client they revoked"
    );
}

/// P2 regression: the resource-group validation move (now ahead of the
/// catalog-client mint, alongside the "kubernetes configured" check) actually
/// runs before anything is minted — both an invalid `resourceGroups` selector
/// and an unconfigured provisioner 400 with no catalog client and no engine
/// row left behind. Without this test, moving either check back to AFTER the
/// client mint would leave every other test in this file green.
#[tokio::test]
async fn invalid_resource_groups_and_unconfigured_kubernetes_leak_nothing() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p2_validation_admin", true).await;
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let row_exists = |name: String| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM engine_mgmt.engines WHERE name = $1)",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    // Invalid resource-group selector: 400 with nothing minted. `setup_full`
    // was given no provisioner at all, so this also proves resource-group
    // validation runs BEFORE the "kubernetes configured" check, not after.
    let wh_id = seed_provisioned_warehouse(&f).await;
    let bad_name = format!("trino-bad-rg-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &bad_name,
        wh_id,
        serde_json::json!({
            "resourceGroups": { "selectors": [{ "group": "nope" }] },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(
        !row_exists(bad_name.clone()).await,
        "no row for an invalid resource-group spec"
    );
    assert!(
        catalog_store.list_clients(wh_id).await.unwrap().is_empty(),
        "no client minted for an invalid resource-group spec"
    );

    // A valid spec but no provisioner configured: 400 with nothing minted either.
    let wh_id2 = seed_provisioned_warehouse(&f).await;
    let name2 = format!("trino-no-k8s-{}", Uuid::new_v4().simple());
    let (status2, j2) = create_engine_via_api(
        &f,
        &token,
        &name2,
        wh_id2,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status2, StatusCode::BAD_REQUEST, "body={j2}");
    assert!(
        !row_exists(name2.clone()).await,
        "no row without kubernetes configured"
    );
    assert!(
        catalog_store.list_clients(wh_id2).await.unwrap().is_empty(),
        "no client minted without kubernetes configured"
    );

    // Same "no provisioner configured" check, moved earlier in the StarRocks
    // path too (right after `validate_sr_autoscaling`): 400 with nothing
    // minted there either.
    let sr_wh = seed_provisioned_warehouse_with_secret(&f).await;
    let sr_name = format!("sr-no-k8s-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": sr_name, "warehouseId": sr_wh, "kind": "starrocks",
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        !row_exists(sr_name.clone()).await,
        "no StarRocks row without kubernetes configured"
    );
    assert!(
        catalog_store.list_clients(sr_wh).await.unwrap().is_empty(),
        "no StarRocks client minted without kubernetes configured"
    );
}

async fn authz_service_of(f: &Fixture, name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT authz_service_id FROM engine_mgmt.engines WHERE name = $1")
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .expect("engine row")
}

/// D9 regression: re-creating an engine whose same-name profile survived (no
/// engine bound) reuses that profile, and the FIRST provision is already bound —
/// never an allow-all window.
#[tokio::test]
async fn recreate_with_same_name_binds_the_orphan_profile_before_provisioning() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_d9_orphan_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy_api::PolicyPort::ensure_trino_def(&policy)
        .await
        .unwrap();
    let orphan = egret_policy_api::PolicyPort::create_service(
        &policy,
        &name,
        "trino",
        &serde_json::json!({}),
    )
    .await
    .unwrap();

    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "body={j}");
    assert_eq!(authz_service_of(&f, &name).await, Some(orphan.id));
    let provisioned = port.provisioned.lock().unwrap().clone();
    assert_eq!(provisioned.len(), 1, "one provision, no unbound first pass");
    assert_eq!(
        provisioned[0]
            .ranger
            .as_ref()
            .map(|r| r.service_name.clone()),
        Some(name.clone())
    );
}

/// D9: a same-name profile already bound to another engine is a 409 before
/// anything is minted.
#[tokio::test]
async fn create_whose_profile_is_bound_elsewhere_is_409_and_mints_nothing() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_d9_bound_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy_api::PolicyPort::ensure_trino_def(&policy)
        .await
        .unwrap();
    let profile = egret_policy_api::PolicyPort::create_service(
        &policy,
        &name,
        "trino",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let other = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": other, "warehouseId": wh_id, "authzServiceId": profile.id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(authz_service_of(&f, &other).await, Some(profile.id));
    let provisions_before = port.provisioned.lock().unwrap().len();

    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM engine_mgmt.engines WHERE name = $1)")
            .bind(&name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert!(!exists);
    let clients = egret_catalog_engine::CatalogStore::new(f.db.clone())
        .list_clients(wh_id)
        .await
        .unwrap();
    assert!(!clients.iter().any(|c| c.name == format!("engine-{name}")));
    assert_eq!(port.provisioned.lock().unwrap().len(), provisions_before);
}

/// R1 (SP1 Part B final review, security gap): an engine created while
/// Ranger was DISABLED keeps `authz_service_id = NULL` forever —
/// `set_engine_authz_service` has one caller, reachable only from create — so
/// once an operator turns Ranger back on, every later apply used to keep
/// re-rendering explicit allow-all with no operator-accessible remedy.
/// `build_provision_req_with` now late-binds a profile (create / adopt-orphan
/// / 409-if-bound-elsewhere, same as create) on any apply once Ranger is on.
#[tokio::test]
async fn apply_late_binds_ranger_once_it_is_enabled_after_an_unbound_create() {
    let port = FakeEnginePort::default();
    let Some(mut f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        false, // created while Ranger is OFF
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_r1_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    assert_eq!(
        authz_service_of(&f, &name).await,
        None,
        "created unbound while Ranger was off"
    );
    assert!(
        port.last_provisioned().ranger.is_none(),
        "the initial provision rendered allow-all"
    );
    let id = j["id"].as_str().unwrap().to_string();

    // Operator turns Ranger back on.
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        true,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
    );
    f.ranger_enabled = true;

    // The smallest apply route (rotate) must now late-bind a profile.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{id}/rotate-secrets"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "rotate must succeed");
    assert_eq!(
        port.last_updated()
            .ranger
            .as_ref()
            .map(|r| r.service_name.clone()),
        Some(name.clone()),
        "the apply must now be bound to a profile named after the engine"
    );
    assert!(
        authz_service_of(&f, &name).await.is_some(),
        "the row itself must record the newly-bound profile"
    );
}

/// D9 + P2: a create that auto-created its profile removes it again on failure.
#[tokio::test]
async fn failed_create_removes_the_profile_it_created() {
    let port = FakeEnginePort::default();
    port.set_fail_provision(true);
    let Some(f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_d9_undo_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert_eq!(
        port.last_provisioned()
            .ranger
            .as_ref()
            .map(|r| r.service_name.clone()),
        Some(name.clone()),
        "the attempted provision was already bound to THIS engine's own profile"
    );
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    assert!(
        egret_policy_api::PolicyPort::get_service_opt(&policy, &name)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        egret_policy_api::PolicyPort::get_service_opt(&policy, &format!("{name}_tag"))
            .await
            .unwrap()
            .is_none()
    );
}

/// Deleting an engine removes every secret egret minted for it, including the
/// internal/monitor secrets.
#[tokio::test]
async fn delete_engine_removes_internal_and_monitor_secrets() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_secrets_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let (internal_id, monitor_id) = (internal_id.unwrap(), monitor_id.unwrap());

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let secrets = secret_store(&f);
    for sid in [internal_id, monitor_id] {
        assert!(
            matches!(
                secrets.resolve_plaintext(None, sid).await,
                Err(egret_secrets::SecretsError::NotFound)
            ),
            "secret {sid} must be deleted with the engine"
        );
    }
}

/// A failed rotation (`commit_internal_secrets`'s `Supersede::Keep` path, or a
/// lost compare-and-set) leaves an `engine-internal-{id}-*` /
/// `engine-monitor-{id}-*` pair that is no longer referenced by the row —
/// nothing sweeps it while the engine might still have an apply in flight
/// (see that function's doc comment). Engine delete is the one place that
/// orphan sweep is safe, since by then no apply can still be racing this
/// engine id. The sweep must be scoped to THIS engine's id: a look-alike pair
/// minted for a different engine (same naming scheme, different id) must
/// survive.
#[tokio::test]
async fn delete_engine_sweeps_orphaned_internal_secret_generations() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_orphan_sweep_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    let name_a = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name_a,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create a; body={j}");
    let id_a = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    // A different engine, minted through the same naming scheme, must never
    // be touched by engine A's sweep.
    let name_b = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name_b,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create b; body={j}");
    let id_b = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (b_internal_id, b_monitor_id) = engine_secret_ids(&f, id_b).await;
    let (b_internal_id, b_monitor_id) = (b_internal_id.unwrap(), b_monitor_id.unwrap());

    // Simulate the orphan a failed rotation on engine A leaves: a fresh
    // generation minted under A's id, never recorded on the row (a `Keep` or
    // a lost CAS never gets here).
    let secrets = secret_store(&f);
    let generation = Uuid::new_v4().simple();
    let orphan_internal_id = secrets
        .create(
            None,
            &format!("engine-internal-{id_a}-{generation}"),
            None,
            "orphan-internal-value",
        )
        .await
        .expect("mint orphan internal secret");
    let orphan_monitor_id = secrets
        .create(
            None,
            &format!("engine-monitor-{id_a}-{generation}"),
            None,
            "orphan-monitor-value",
        )
        .await
        .expect("mint orphan monitor secret");

    // M5: the sweep predicate is `starts_with(prefix)`, not `contains(prefix)` —
    // a secret whose name merely CONTAINS engine A's prefix as a substring,
    // without starting with it, must survive. Catches a `starts_with` →
    // `contains` regression the fixtures above (which only ever start with the
    // prefix or belong to an unrelated id) cannot.
    let lookalike_id = secrets
        .create(
            None,
            &format!("other-engine-internal-{id_a}-x"),
            None,
            "lookalike-value",
        )
        .await
        .expect("mint lookalike secret");

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{id_a}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    for sid in [orphan_internal_id, orphan_monitor_id] {
        assert!(
            matches!(
                secrets.resolve_plaintext(None, sid).await,
                Err(egret_secrets::SecretsError::NotFound)
            ),
            "orphaned generation {sid} must be swept with the engine"
        );
    }
    for sid in [b_internal_id, b_monitor_id] {
        assert!(
            secrets.resolve_plaintext(None, sid).await.is_ok(),
            "a different engine's secret {sid} must survive engine A's sweep"
        );
    }
    assert!(
        secrets.resolve_plaintext(None, lookalike_id).await.is_ok(),
        "a secret whose name only CONTAINS the prefix, without starting with it, \
         must survive the sweep"
    );
}

// ── SP3a: engine catalogs over the registry ──────────────────────────────────

/// Attach `wh_id`'s own registry catalog (created alongside the warehouse) to
/// `engine_id`, via the registry's attach route
/// (`POST /engines/{id}/catalogs/{cid}`) — what a legacy
/// `POST /engines/{id}/catalogs` with `{kind: "egret-warehouse", config:
/// {warehouseId}}` used to do in one call, now two: look the catalog up, then
/// attach it by id. Returns status + JSON + the catalog row (its id and name
/// are what most callers need next).
async fn attach_warehouse_via_api(
    f: &Fixture,
    token: &str,
    engine_id: &str,
    wh_id: Uuid,
) -> (
    StatusCode,
    serde_json::Value,
    egret_catalog_registry::CatalogRow,
) {
    // Get-or-create: a warehouse's registry catalog is otherwise made lazily,
    // the first time an engine attaches it (`warehouse_catalog` above assumes
    // it already exists, which only holds once some engine has). This is what
    // `add_catalog`'s `kind: "egret-warehouse"` branch called internally.
    let wh = egret_catalog_engine::CatalogStore::new(f.db.clone())
        .get_warehouse(wh_id)
        .await
        .expect("get warehouse")
        .expect("warehouse exists");
    let (catalog, _) = registry(f)
        .ensure_warehouse_catalog(
            &egret_catalog_registry::WarehouseIdentity {
                id: wh.id,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            None,
        )
        .await
        .expect("ensure warehouse catalog");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs/{}", catalog.id),
            token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    (status, j, catalog)
}

/// A zero-config catalog kind `can_read` on BOTH engines (D4 correction:
/// `tpch`/`tpcds` no longer are — StarRocks has no `BENCHMARK` connector on
/// any shipped release, `c019a148`). Used where a test targets a StarRocks
/// engine and needs the request to get PAST the can_read gate, to exercise
/// something else (an apply, a detach, an orphan-preservation check, …)
/// instead of re-testing can_read itself, which has its own dedicated
/// coverage in egret-catalog-registry. It also renders entirely from plain
/// properties, which is what lets a drift test differ in exactly one key.
fn elasticsearch_body(name: &str) -> serde_json::Value {
    serde_json::json!({ "name": name, "kind": "elasticsearch",
        "config": { "hosts": ["es1"], "port": 9200, "tls": false } })
}

/// `POST /engines` for a StarRocks engine with the wizard defaults; returns
/// `(engine id, its warehouse id)`.
///
/// **Do not give a StarRocks fixture an `-fe-` pod.** The engine's own
/// post-provision FE wire IS on `f.starrocks_ddl` since SP3b Task 5, and in
/// THIS binary it runs at the production budget (`MAX_POLLS = 30` × 10s — the
/// `#[cfg(test)]` shrink applies only to the lib's own unit-test build). It
/// never reaches a statement only because it polls
/// `EngineProvisioningPort::status` for a pod whose name contains `-fe-` and
/// `FakeEnginePort`'s single pod is `coordinator-0`; a `set_status` naming an
/// `-fe-` pod would put a live convergence task alongside the test's own
/// assertions. The assertion below makes that dependency a loud failure here
/// rather than a flake somewhere else (fix round 1, F7). Every statement the
/// fake sees in these tests was sent by the apply path under test.
async fn create_starrocks_via_api(f: &Fixture, token: &str) -> (String, Uuid) {
    if let Some(prov) = f.engine_provisioner.as_ref() {
        let reported = prov
            .status("starrocks", "egret-engines", "probe")
            .await
            .expect("fake status");
        assert!(
            !reported.pods.iter().any(|p| p.name.contains("-fe-")),
            "this fixture reports an `-fe-` pod, so the background bootstrap this \
             create spawns WILL converge catalogs concurrently with the test's own \
             assertions. Drive the bootstrap from the lib's unit tests (where \
             MAX_POLLS is 1) instead, or await it deliberately."
        );
    }
    let wh_id = seed_provisioned_warehouse_with_secret(f).await;
    let (status, j) = call(f, "POST", "/engines", token, Some(serde_json::json!({
        "name": format!("sr-{}", Uuid::new_v4().simple()), "warehouseId": wh_id, "kind": "starrocks",
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
    }))).await;
    assert_eq!(status, StatusCode::CREATED, "create starrocks; body={j}");
    (j["id"].as_str().unwrap().to_string(), wh_id)
}

/// SP3b D1 (migration 0149): `POST /engines` for StarRocks RECORDS its primary
/// attachment and leaves the converging to the background wire, so the row is
/// `pending` — not `applied`, which is what it claimed for the five to ten
/// minutes the FE is unreachable, about a cluster nothing had yet spoken to.
/// Task 5 is the task that made this safe: before it, the bootstrap wrote no
/// per-attachment status at all, so a `pending` row here would have stayed
/// `pending` for ever.
#[tokio::test]
async fn a_new_starrocks_engines_primary_attachment_is_recorded_pending() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_sr_pending_admin", true).await;
    let (engine_id, wh_id) = create_starrocks_via_api(&f, &token).await;

    let cat = warehouse_catalog(&f, wh_id).await;
    let row = registry(&f)
        .get_attachment(Uuid::parse_str(&engine_id).unwrap(), cat.id)
        .await
        .unwrap()
        .expect("primary attachment written");
    assert_eq!(row.apply_status, "pending");
    assert!(row.apply_error.is_none());
    assert!(
        row.applied_at.is_none(),
        "nothing has applied it: the FE has not been spoken to"
    );
    // And the wire really has not spoken to the fake FE — `FakeEnginePort`'s
    // only pod is `coordinator-0`, which is not an FE, so the bootstrap is
    // still in its readiness poll.
    assert!(
        f.starrocks_ddl.executed().is_empty(),
        "{:?}",
        f.starrocks_ddl.executed()
    );
}

/// A `jdbc` catalog body whose password lives in `secret_id` — the kind whose
/// StarRocks statement really carries a credential inline (D6), and the one
/// `SHOW CREATE CATALOG` deletes `password` from.
fn jdbc_body(name: &str, secret_id: Uuid) -> serde_json::Value {
    serde_json::json!({ "name": name, "kind": "jdbc", "config": {
        "dialect": "postgresql", "host": "db.internal", "port": 5432,
        "database": "sales", "user": "reader", "password_secret_id": secret_id,
        "ssl": { "mode": "require" }
    }})
}

/// SP3b D1/D3: the seam is gone — an egret warehouse attached to a StarRocks
/// engine mints its Catalog Client and the catalog is CREATEd on the FE, with
/// the minted credential inline (there is no `${ENV:…}` on StarRocks).
/// Was `attaching_to_a_starrocks_engine_is_refused`.
#[tokio::test]
async fn attaching_an_egret_warehouse_to_a_starrocks_engine_mints_a_client_and_creates_the_catalog()
{
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_sr_attach_admin", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;

    let other = seed_provisioned_warehouse(&f).await;
    let clients_before = egret_catalog_engine::CatalogStore::new(f.db.clone())
        .list_clients(other)
        .await
        .unwrap()
        .len();
    let (status, j, cat) = attach_warehouse_via_api(&f, &token, &engine_id, other).await;
    assert_eq!(status, StatusCode::CREATED, "body={j}");
    assert_eq!(
        egret_catalog_engine::CatalogStore::new(f.db.clone())
            .list_clients(other)
            .await
            .unwrap()
            .len(),
        clients_before + 1,
        "exactly one client minted"
    );
    let row = registry(&f)
        .get_attachment(Uuid::parse_str(&engine_id).unwrap(), cat.id)
        .await
        .unwrap()
        .expect("attachment written");
    assert_eq!(row.apply_status, "applied");
    assert!(row.minted_client);

    let sent = f.starrocks_ddl.executed();
    let create = sent
        .iter()
        .find(|s| s.starts_with("CREATE EXTERNAL CATALOG"))
        .unwrap_or_else(|| panic!("no CREATE was sent; sent={sent:?}"));
    assert!(
        create.contains("\"iceberg.catalog.credential\""),
        "the minted credential is inline in the statement; create={create}"
    );
    // D3: never `IF NOT EXISTS` — that is what silently adopts a hand-made
    // catalog and makes a credential rotation a no-op.
    assert!(!create.contains("IF NOT EXISTS"), "create={create}");
    assert_eq!(
        f.starrocks_ddl.catalog(&cat.name).unwrap()["type"],
        "iceberg"
    );
}

/// D3 + `is_reserved_starrocks_name`: the refusal that replaced the
/// not-supported-yet gate holds the SAME pre-mint position — a catalog whose
/// name StarRocks reserves is 400, and no Catalog Client is minted for it.
#[tokio::test]
async fn a_reserved_starrocks_catalog_name_is_refused_before_a_client_is_minted() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_sr_reserved_admin", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;

    // A warehouse whose name lands on StarRocks' resource-mapping prefix, so
    // its registry catalog (named after the warehouse) is reserved. The
    // prefix, not the bare `default_catalog`, so repeated runs against the
    // same database cannot collide on `registry.catalogs.name`.
    let reserved = seed_warehouse_named(
        &f,
        &format!(
            "resource_mapping_inside_catalog_{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ),
    )
    .await;
    let clients_before = egret_catalog_engine::CatalogStore::new(f.db.clone())
        .list_clients(reserved)
        .await
        .unwrap()
        .len();
    let (status, j, _cat) = attach_warehouse_via_api(&f, &token, &engine_id, reserved).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap()
            .contains("reserves the catalog"),
        "{j}"
    );
    assert_eq!(
        egret_catalog_engine::CatalogStore::new(f.db.clone())
            .list_clients(reserved)
            .await
            .unwrap()
            .len(),
        clients_before,
        "no client minted"
    );
    assert!(
        f.starrocks_ddl.executed().is_empty(),
        "nothing reached the FE: {:?}",
        f.starrocks_ddl.executed()
    );
}

/// Whole-branch review F-5: two attachments can share one Catalog Client —
/// the backfill takes it verbatim from the legacy row and derives the
/// credential secret's name from it, so two engines end up on one client and
/// one secret. Detaching one of them must revoke neither: the other engine's
/// rendered Trino properties still present that credential.
#[tokio::test]
async fn detach_keeps_a_catalog_client_another_attachment_still_uses() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_shared_client_admin", true).await;
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &format!("trino-{}", Uuid::new_v4().simple()),
        seed_provisioned_warehouse(&f).await,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create A; body={j}");
    let engine_a = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &format!("trino-{}", Uuid::new_v4().simple()),
        seed_provisioned_warehouse(&f).await,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create B; body={j}");
    let engine_b = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    // Engine A attaches a SECOND warehouse: a minted client + its credential
    // secret, both retired on detach.
    let shared_wh = seed_provisioned_warehouse(&f).await;
    let (status, j, catalog) =
        attach_warehouse_via_api(&f, &token, &engine_a.to_string(), shared_wh).await;
    assert_eq!(status, StatusCode::CREATED, "attach to A; body={j}");
    let attachment = registry(&f)
        .get_attachment(engine_a, catalog.id)
        .await
        .unwrap()
        .expect("attachment");
    let client_id = attachment.catalog_client_id.expect("minted client");
    let secret_id = attachment.credential_secret_id.expect("credential secret");

    // What the backfill produces: engine B reaches the same catalog through
    // the SAME client and the SAME secret.
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments \
           (engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client) \
         VALUES ($1, $2, $3, $4, true)",
    )
    .bind(engine_b)
    .bind(catalog.id)
    .bind(client_id)
    .bind(secret_id)
    .execute(&f.db.pool)
    .await
    .expect("seed the shared attachment");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/catalogs/{}/engines/{engine_a}", catalog.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "detach from A");
    assert!(
        registry(&f)
            .get_attachment(engine_a, catalog.id)
            .await
            .unwrap()
            .is_none(),
        "A is detached"
    );

    assert!(
        egret_catalog_engine::CatalogStore::new(f.db.clone())
            .list_clients(shared_wh)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client_id),
        "the Catalog Client engine B still uses must not be revoked"
    );
    assert!(
        secret_store(&f)
            .resolve_plaintext(None, secret_id)
            .await
            .is_ok(),
        "the credential secret engine B still holds must survive"
    );
}

// ── Catalog registry (SP3a) ─────────────────────────────────────────────────────

fn unique_catalog_name(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn call(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    into_json(
        f.app
            .clone()
            .oneshot(req(method, uri, token, body))
            .await
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn registry_catalog_crud_validation_and_delete_refusal() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_crud_admin", true).await;
    let pw = seed_secret(&f, "hunter2").await;
    let name = unique_catalog_name("pg");
    let jdbc = serde_json::json!({
        "dialect": "postgresql", "host": "db.internal", "port": 5432, "database": "sales",
        "user": "reader", "password_secret_id": pw, "ssl": { "mode": "require" }
    });

    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": name, "kind": "jdbc", "config": jdbc, "description": "sales db"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(
        (j["name"].as_str(), j["kind"].as_str()),
        (Some(name.as_str()), Some("jdbc"))
    );
    assert_eq!(j["engines"], serde_json::json!([]));
    let id = j["id"].as_str().unwrap().to_string();

    let (status, j) = call(&f, "GET", &format!("/catalogs?secretId={pw}"), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        j.as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == serde_json::json!(id)),
        "{j}"
    );

    let (status, _) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": name, "kind": "jdbc", "config": jdbc })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate name");
    for (why, body) in [
        (
            "bad name",
            serde_json::json!({ "name": "Bad-Name", "kind": "tpch", "config": {} }),
        ),
        (
            "unknown kind",
            serde_json::json!({ "name": unique_catalog_name("x"), "kind": "mongo", "config": {} }),
        ),
        (
            "unknown secret",
            serde_json::json!({ "name": unique_catalog_name("x"), "kind": "jdbc", "config": {
            "dialect": "postgresql", "host": "h", "port": 5432, "database": "d", "user": "u",
            "password_secret_id": Uuid::new_v4(), "ssl": { "mode": "require" } } }),
        ),
        (
            "plaintext advanced",
            serde_json::json!({ "name": unique_catalog_name("x"), "kind": "tpch", "config": {},
            "advanced": { "trino": { "a.password": "hunter2" } } }),
        ),
        (
            "egret warehouse",
            serde_json::json!({ "name": unique_catalog_name("x"), "kind": "iceberg",
            "config": { "metastore": { "type": "egret", "warehouse_id": Uuid::new_v4() } } }),
        ),
    ] {
        let (status, j) = call(&f, "POST", "/catalogs", &token, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {j}");
    }

    // Refused while attached; the 409 names the engine.
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let catalog_id = Uuid::parse_str(&id).unwrap();
    let reg = registry(&f);
    reg.insert_attachment(&egret_catalog_registry::NewAttachment {
        engine_id,
        catalog_id,
        catalog_client_id: None,
        credential_secret_id: None,
        minted_client: false,
        applied_now: true,
        is_primary: false,
        created_at: None,
        apply_status: egret_catalog_registry::store::apply_status::APPLIED,
    })
    .await
    .unwrap();
    let (status, j) = call(&f, "DELETE", &format!("/catalogs/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(j["code"], "catalog_in_use");
    assert_eq!(j["engines"][0]["name"], serde_json::json!(engine_name));
    reg.delete_attachment(engine_id, catalog_id).await.unwrap();
    let (status, _) = call(&f, "DELETE", &format!("/catalogs/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A warehouse catalog goes with its warehouse, never through this route.
    let lone_wh = seed_provisioned_warehouse(&f).await;
    let wh = egret_catalog_engine::CatalogStore::new(f.db.clone())
        .get_warehouse(lone_wh)
        .await
        .unwrap()
        .unwrap();
    let (lone, _) = reg
        .ensure_warehouse_catalog(
            &egret_catalog_registry::WarehouseIdentity {
                id: lone_wh,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            None,
        )
        .await
        .unwrap();
    // Still assigned to its warehouse's workspace (never unassigned): the
    // warehouse refusal fires unconditionally, before the in-use check would
    // otherwise see that assignment and report `catalog_in_use` instead.
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{}", lone.id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(j["code"], "warehouse_catalog", "{j}");
    assert!(j["error"].as_str().unwrap().contains("warehouse"), "{j}");
}

#[tokio::test]
async fn registry_update_propagates_and_flags_the_failed_engine() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_prop_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name_a = format!("trino-a-{}", Uuid::new_v4().simple());
    let name_b = format!("trino-b-{}", Uuid::new_v4().simple());
    let mut engines = Vec::new();
    for n in [&name_a, &name_b] {
        let (status, j) = create_engine_via_api(&f, &token, n, wh_id, serde_json::json!({})).await;
        assert_eq!(status, StatusCode::CREATED, "{j}");
        engines.push(Uuid::parse_str(j["id"].as_str().unwrap()).unwrap());
    }
    let cat_name = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let catalog_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let reg = registry(&f);
    for engine_id in &engines {
        reg.insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: *engine_id,
            catalog_id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: true,
            is_primary: false,
            created_at: None,
            apply_status: egret_catalog_registry::store::apply_status::APPLIED,
        })
        .await
        .unwrap();
    }

    let (status, j) = call(
        &f,
        "GET",
        &format!("/catalogs/{catalog_id}/impact"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["engines"].as_array().unwrap().len(), 2);
    assert!(
        j["engines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["action"] == "restart"),
        "{j}"
    );

    port.set_fail_update_for(&name_b, true);
    let (status, j) = call(
        &f,
        "PUT",
        &format!("/catalogs/{catalog_id}"),
        &token,
        Some(serde_json::json!({
            "config": {}, "advanced": { "trino": { "tpch.column-naming": "STANDARD" } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    let outcome = |n: &str| {
        j["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["engineName"] == n)
            .cloned()
            .unwrap()
    };
    assert_eq!(outcome(&name_a)["status"], "applied");
    assert_eq!(outcome(&name_b)["status"], "failed");
    assert!(outcome(&name_b)["error"]
        .as_str()
        .unwrap()
        .contains("injected update failure"));

    // Engine A holds the new configuration; nothing was reverted.
    let spec_a = port
        .updated
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|s| s.name == name_a)
        .cloned()
        .unwrap();
    let rendered = spec_a
        .catalogs
        .iter()
        .find(|c| c.name == cat_name)
        .expect("catalog rendered on A");
    assert_eq!(rendered.properties["tpch.column-naming"], "STANDARD");
    let stored = reg.get_catalog(catalog_id).await.unwrap().unwrap();
    assert_eq!(stored.advanced["trino"]["tpch.column-naming"], "STANDARD");
    let a = reg
        .get_attachment(engines[0], catalog_id)
        .await
        .unwrap()
        .unwrap();
    let b = reg
        .get_attachment(engines[1], catalog_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.apply_status, "applied");
    assert_eq!(b.apply_status, "failed");
    assert!(b.apply_error.unwrap().contains("injected update failure"));

    // Re-apply once the engine is healthy again.
    port.set_fail_update_for(&name_b, false);
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{catalog_id}/reapply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert!(
        j.as_array()
            .unwrap()
            .iter()
            .all(|o| o["status"] == "applied"),
        "{j}"
    );
    assert_eq!(
        reg.get_attachment(engines[1], catalog_id)
            .await
            .unwrap()
            .unwrap()
            .apply_status,
        "applied"
    );
}

#[tokio::test]
async fn catalog_permissions_with_the_real_pdp() {
    let Some(mut f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let reader = session_for_with_permission(&f, "cat_reader", "catalog_registry:read").await;
    let engine_reader = session_for_with_permission(&f, "cat_engine_reader", "engine:read").await;
    attach_real_pdp(&mut f).await;
    let body =
        serde_json::json!({ "name": unique_catalog_name("x"), "kind": "tpch", "config": {} });
    assert_eq!(
        call(&f, "GET", "/catalogs", &reader, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&f, "POST", "/catalogs", &reader, Some(body)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f, "GET", "/catalogs", &engine_reader, None).await.0,
        StatusCode::FORBIDDEN
    );
    // 0148: every platform role holding warehouse:read now holds catalog_registry:read.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
          WHERE r.scope = 'platform' AND r.name = 'data_steward' AND rp.permission = 'catalog_registry:read'",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn registry_catalog_not_found_warehouse_change_and_permission_checks() {
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "cat_404_admin", true).await;
    let reader = session_for_with_permission(&f, "cat_404_reader", "catalog_registry:read").await;
    attach_real_pdp(&mut f).await;

    // Unknown id: 404 on every route (the admin holds platform_admin's
    // unrestricted PDP bypass, so these reach `load`).
    let missing = Uuid::new_v4();
    assert_eq!(
        call(&f, "GET", &format!("/catalogs/{missing}"), &admin, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f,
            "PUT",
            &format!("/catalogs/{missing}"),
            &admin,
            Some(serde_json::json!({ "config": {} })),
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&f, "DELETE", &format!("/catalogs/{missing}"), &admin, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // A read-only session (holds catalog_registry:read only) is forbidden from
    // updating or deleting, even a catalog that doesn't exist — authz runs
    // before the lookup.
    assert_eq!(
        call(
            &f,
            "PUT",
            &format!("/catalogs/{missing}"),
            &reader,
            Some(serde_json::json!({ "config": {} })),
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f, "DELETE", &format!("/catalogs/{missing}"), &reader, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // PUT that newly ADDS a warehouse_id to a plain (non-egret) iceberg catalog
    // is refused: the warehouse of an egret catalog is immutable, and that
    // includes going from none to one.
    let sc_id = seed_exchange_storage_conn(&f).await;
    let name = unique_catalog_name("ice");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &admin,
        Some(serde_json::json!({
            "name": name, "kind": "iceberg",
            "config": { "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": sc_id }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let catalog_id = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "PUT",
        &format!("/catalogs/{catalog_id}"),
        &admin,
        Some(
            serde_json::json!({ "config": { "metastore": { "type": "egret", "warehouse_id": Uuid::new_v4() } } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert!(j["error"].as_str().unwrap().contains("warehouse"), "{j}");

    // A 409 body that lists `workspaces`: assign a (non-warehouse) catalog to a
    // workspace, then try to delete it.
    let name2 = unique_catalog_name("bench2");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &admin,
        Some(serde_json::json!({ "name": name2, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cat2_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let ws_id = seed_workspace(&f).await;
    // `set_catalog_workspaces` (the pool-opening wrapper) was deleted in
    // SP4a Task 6 fix round 2 — it was the third path writing
    // `registry.catalog_workspaces` with no `perm` grant sync composed in.
    // Only `_in_tx` remains; this test asserts a 409 conflict body, not
    // `perm.grants`, so it composes no sync — the route-level assertions of
    // that (`workspace_assignment_route_syncs_perm_grants`) live elsewhere.
    let mut tx = f.db.pool.begin().await.unwrap();
    egret_catalog_registry::RegistryStore::set_catalog_workspaces_in_tx(&mut tx, cat2_id, &[ws_id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, j) = call(&f, "DELETE", &format!("/catalogs/{cat2_id}"), &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(j["code"], "catalog_in_use");
    assert_eq!(j["workspaces"].as_array().unwrap().len(), 1, "{j}");
    assert_eq!(j["workspaces"][0]["id"], serde_json::json!(ws_id), "{j}");
    assert!(
        j.get("engines")
            .map(|v| v.as_array().unwrap().is_empty())
            .unwrap_or(true),
        "{j}"
    );
}

#[tokio::test]
async fn attach_detach_and_retry_on_a_trino_engine() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_attach_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("bench");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );

    // A rejected apply is a 502 and writes nothing.
    port.set_fail_update(true);
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{j}");
    assert_eq!(j["code"], "engine_cluster_apply_failed");
    assert!(reg.get_attachment(e, c).await.unwrap().is_none());
    port.set_fail_update(false);

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["applyStatus"], "applied");
    assert!(port
        .last_updated()
        .catalogs
        .iter()
        .any(|c| c.name == cat_name));
    let (status, _) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "already attached");

    // A StarRocks-only generic catalog cannot be attached to Trino.
    let sr_only_name = unique_catalog_name("paimon");
    let (_, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({
        "name": sr_only_name, "kind": "generic",
        "config": { "engines": ["starrocks"] }, "advanced": { "starrocks": { "type": "paimon" } }
    }))).await;
    let sr_only = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{sr_only}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    let msg = j["error"].as_str().unwrap();
    assert!(msg.contains("trino"), "{msg} should name the engine type");
    assert!(msg.contains(&sr_only_name), "{msg} should name the catalog");

    // Retry's IDOR guard: `sr_only` exists but was never attached to this
    // (or any) engine — the path's engine id does not make it eligible.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{sr_only}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{j}");
    assert!(reg
        .get_attachment(e, Uuid::parse_str(&sr_only).unwrap())
        .await
        .unwrap()
        .is_none());

    // Retry: a failed attachment is re-applied once the engine is healthy.
    reg.set_apply_failed(e, c, "earlier failure").await.unwrap();
    port.set_fail_update(true);
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(j["code"], "engine_cluster_apply_failed");
    assert!(reg
        .get_attachment(e, c)
        .await
        .unwrap()
        .unwrap()
        .apply_error
        .unwrap()
        .contains("injected update failure"));
    port.set_fail_update(false);
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["status"], "applied");
    let a = reg.get_attachment(e, c).await.unwrap().unwrap();
    assert_eq!((a.apply_status.as_str(), a.apply_error), ("applied", None));

    // Detach (the shared DELETE) applies first, then removes the attachment.
    let (status, _) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(reg.get_attachment(e, c).await.unwrap().is_none());
    assert!(port
        .last_updated()
        .catalogs
        .iter()
        .all(|c| c.name != cat_name));
}

/// D1: an attach to a StarRocks engine whose FE is reachable creates the
/// catalog there and the attachment reads `applied`.
/// Was `nothing_is_attached_to_a_starrocks_engine_yet`. (Not the plan's
/// `tpch`/`benchmark` catalog: D4's correction refuses those on StarRocks
/// outright — the connector is on no shipped release.)
#[tokio::test]
async fn a_catalog_attaches_to_a_starrocks_engine_and_reaches_the_fe() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_admin", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["applyStatus"], "applied", "{j}");
    assert_eq!(f.starrocks_ddl.created(), vec![name.clone()]);
    assert_eq!(f.starrocks_ddl.catalog(&name).unwrap()["type"], "es");
    let row = registry(&f)
        .get_attachment(
            Uuid::parse_str(&engine_id).unwrap(),
            Uuid::parse_str(&cid).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.apply_status, "applied");
    assert!(row.applied_at.is_some(), "applied_at is set");
    // D6: every statement carries the engine's FE coordinates through the one
    // target `fe_target` builds — there is no second, cert-less path any more.
    assert_eq!(f.starrocks_ddl.last_target().port, 9030);
}

/// D1: an attach against an UNREACHABLE FE still answers 201, records the row
/// `failed` with the error, creates nothing — and the Retry route converges it.
#[tokio::test]
async fn an_attach_against_an_unreachable_fe_is_recorded_and_converged_by_retry() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_unreachable", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );

    f.starrocks_ddl
        .set_fail(Some("fe unreachable: connect timed out after 10s"));
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "D1: the row is the point; {j}");
    assert_eq!(j["applyStatus"], "failed", "{j}");
    assert!(
        j["applyError"].as_str().unwrap().contains("fe unreachable"),
        "{j}"
    );
    assert!(
        f.starrocks_ddl
            .executed()
            .iter()
            .all(|s| !s.starts_with("CREATE EXTERNAL CATALOG")),
        "nothing was created: {:?}",
        f.starrocks_ddl.executed()
    );
    assert_eq!(
        registry(&f)
            .get_attachment(e, c)
            .await
            .unwrap()
            .unwrap()
            .apply_status,
        "failed"
    );

    // Retry converges it, with no second attach and no second client.
    f.starrocks_ddl.set_fail(None);
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    let row = registry(&f).get_attachment(e, c).await.unwrap().unwrap();
    assert_eq!(
        (row.apply_status.as_str(), row.apply_error),
        ("applied", None)
    );
    assert!(f.starrocks_ddl.catalog(&name).is_some(), "created on retry");
}

/// D3: a catalog of the same name the FE already holds with DIFFERENT
/// properties is refused, the FE is untouched, and the error names the keys.
/// This is the `egret` catalog today's Warehouse UI tells operators to create
/// by hand, and the one a pre-SP3a engine leaves behind.
#[tokio::test]
async fn a_drifted_catalog_is_refused_and_the_fe_is_left_alone() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_drift", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    // Somebody else's catalog, under the name egret is about to use.
    f.starrocks_ddl.seed(
        &name,
        &[
            ("type", "es"),
            ("hosts", "http://somebody-elses-cluster:9200"),
            ("es.net.ssl", "false"),
        ],
    );
    let before = f.starrocks_ddl.catalog(&name).unwrap();

    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "D1: the row is still written; {j}"
    );
    assert_eq!(j["applyStatus"], "failed", "{j}");
    let err = j["applyError"].as_str().unwrap();
    assert!(err.contains("hosts"), "the differing key is named: {err}");
    assert!(
        !err.contains("somebody-elses-cluster"),
        "key NAMES only — a value could be anything the FE holds: {err}"
    );
    assert_eq!(
        f.starrocks_ddl.catalog(&name).unwrap(),
        before,
        "the FE was left exactly as it was"
    );
    assert!(
        f.starrocks_ddl.executed().iter().all(|s| {
            !s.starts_with("CREATE EXTERNAL CATALOG") && !s.starts_with("ALTER CATALOG")
        }),
        "reads only: {:?}",
        f.starrocks_ddl.executed()
    );
}

/// D3's two reads are not atomic, and the second one is the more specific
/// (fix round 1 review, F7). When `SHOW CATALOGS` lists a name but
/// `SHOW CREATE CATALOG` then answers "Unknown catalog" — the catalog was
/// dropped in between — egret must treat it as ABSENT and try to create it.
/// Before this fix the `None` became an empty property map, every key read as
/// differing, and the operator was told to "detach it or drop it on the
/// engine" about a catalog that no longer existed.
#[tokio::test]
async fn a_catalog_that_vanishes_between_the_two_reads_is_absent_not_drifted() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_vanish", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    // Listed by `SHOW CATALOGS` …
    f.starrocks_ddl.seed(&name, &[("type", "es")]);
    // … but gone by the time the second read asks about it.
    f.starrocks_ddl.fail_matching(
        "SHOW CREATE CATALOG",
        &format!("ERROR 5078 (HY000): Unknown catalog '{name}'"),
    );

    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");

    // Egret tried to CREATE it, which is the only thing "absent" can mean.
    assert!(
        f.starrocks_ddl
            .executed()
            .iter()
            .any(|s| s.starts_with("CREATE EXTERNAL CATALOG")),
        "a CREATE was attempted: {:?}",
        f.starrocks_ddl.executed()
    );
    // Whatever the FE then said is the FE's own answer — never egret's
    // "different properties, detach it" invention about a phantom catalog.
    let err = j["applyError"].as_str().unwrap_or_default();
    assert!(
        !err.contains("different properties"),
        "the drift refusal must not fire for a catalog the FE says is unknown: {err}"
    );
}

/// D3: re-applying an edited catalog ALTERs — it never issues a second CREATE,
/// which `IF NOT EXISTS` would have turned into a silent no-op. The rotated
/// password has to actually reach the FE, which is the whole point: the drift
/// comparison cannot see it (`SHOW CREATE CATALOG` deletes `password`).
#[tokio::test]
async fn a_credential_rotation_reaches_the_fe_through_alter() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_rotate", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let first = seed_secret(&f, "hunter2-the-first").await;
    let second = seed_secret(&f, "hunter2-the-second").await;
    let name = unique_catalog_name("pg");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(jdbc_body(&name, first)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(
        f.starrocks_ddl.catalog(&name).unwrap()["password"],
        "hunter2-the-first"
    );

    // Rotate: the same body with a different password secret. `PUT /catalogs`
    // propagates to every attached engine with no StarRocks-specific code.
    let (status, j) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}"),
        &token,
        Some(serde_json::json!({ "config": jdbc_body(&name, second)["config"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["outcomes"][0]["status"], "applied", "{j}");

    let sent = f.starrocks_ddl.executed();
    assert!(
        sent.iter().any(|s| s.starts_with("ALTER CATALOG")),
        "sent={sent:?}"
    );
    assert_eq!(
        sent.iter()
            .filter(|s| s.starts_with("CREATE EXTERNAL CATALOG"))
            .count(),
        1,
        "exactly the original create; sent={sent:?}"
    );
    assert_eq!(
        f.starrocks_ddl.catalog(&name).unwrap()["password"],
        "hunter2-the-second",
        "the rotation actually reached the FE"
    );
}

/// D7: detach drops first; a catalog the FE never had is still a success, and
/// the primary catalog is still refused without a statement being sent for it.
#[tokio::test]
async fn detach_drops_first_tolerates_an_unknown_catalog_and_still_refuses_the_primary() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_detach", true).await;
    let (engine_id, wh_id) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");

    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");
    assert!(
        f.starrocks_ddl.catalog(&name).is_none(),
        "dropped on the FE"
    );
    assert!(
        f.starrocks_ddl
            .executed()
            .iter()
            .any(|s| s.starts_with("DROP CATALOG IF EXISTS")),
        "a DROP was sent: {:?}",
        f.starrocks_ddl.executed()
    );
    assert!(
        registry(&f).get_attachment(e, c).await.unwrap().is_none(),
        "row gone AFTER the drop"
    );

    // A second attach, then a DROP the FE no longer knows about: still a
    // success, so a row can always be cleaned up.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    f.starrocks_ddl.forget(&name);
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "an unknown catalog is not an error; {j}"
    );
    assert!(registry(&f).get_attachment(e, c).await.unwrap().is_none());

    // The primary is still refused, and nothing was sent to the FE for it.
    let primary = warehouse_catalog(&f, wh_id).await;
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{}/engines/{engine_id}", primary.id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert!(
        f.starrocks_ddl
            .executed()
            .iter()
            .all(|s| !s.contains(&primary.name)),
        "no statement about the primary: {:?}",
        f.starrocks_ddl.executed()
    );
    assert!(registry(&f)
        .get_attachment(e, primary.id)
        .await
        .unwrap()
        .is_some());
}

/// D1's reason for the detach ORDER, which nothing pinned before (fix round 1
/// review, F3): "a `DROP CATALOG` that fails keeps the row, because a row
/// egret has forgotten is worse than one it retries." Moving
/// `delete_attachment` ahead of the DROP left every other detach test green —
/// they only show that a DROP was sent and the row is gone, both true in
/// either order. This is the assertion that can only hold one way round.
#[tokio::test]
async fn a_detach_whose_drop_fails_keeps_the_attachment_row() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_detach_fail", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");

    // The FE refuses every statement naming this catalog.
    f.starrocks_ddl
        .fail_catalog(&name, "ERROR 1064 (HY000): drop refused by the FE");
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::NO_CONTENT,
        "a detach whose DROP the FE refused is not a success; {j}"
    );
    assert!(
        f.starrocks_ddl
            .executed()
            .iter()
            .any(|s| s.starts_with("DROP CATALOG IF EXISTS")),
        "the DROP was attempted: {:?}",
        f.starrocks_ddl.executed()
    );
    // The whole point of the order: egret still remembers the attachment, so
    // the operator can retry the detach instead of owning an orphan on the FE.
    assert!(
        registry(&f).get_attachment(e, c).await.unwrap().is_some(),
        "the row survives a failed DROP (D1)"
    );
    assert!(
        f.starrocks_ddl.catalog(&name).is_some(),
        "and the catalog is still on the FE, which is what that row must keep matching"
    );
    // Nothing was given back either: a detach that did not happen must not
    // retire the credential the surviving attachment still names.
    let row = registry(&f).get_attachment(e, c).await.unwrap().unwrap();
    assert_eq!(row.apply_status, "applied", "the attachment is untouched");
}

/// SP3c-1 task 1: `DELETE /catalogs/{id}/engines/{engine_id}` is a second door
/// onto the SAME `detach_from_engine`, symmetric with `/catalogs/{id}/workspaces`.
/// The registry route detaches the catalog from the engine but keeps the
/// registry catalog itself, so it can be attached elsewhere later.
#[tokio::test]
async fn registry_detach_removes_the_attachment_and_keeps_the_catalog() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_detach_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("regdetach");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");

    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");
    assert!(
        reg.get_attachment(e, c).await.unwrap().is_none(),
        "the attachment is gone"
    );
    assert!(
        port.last_updated()
            .catalogs
            .iter()
            .all(|c| c.name != cat_name),
        "the engine no longer carries the catalog"
    );

    // The registry catalog itself survives: it can be attached elsewhere.
    let (status, j) = call(&f, "GET", &format!("/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "catalog is kept: {j}");
    assert_eq!(j["id"], serde_json::json!(cid));
}

/// The engine's primary catalog cannot be detached through the registry route
/// either — the refusal lives entirely in `detach_from_engine` (D5), which
/// this route reaches too, and nothing is sent to the engine or written.
#[tokio::test]
async fn registry_detach_refuses_the_primary_catalog_with_400() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_detach_primary_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let primary = warehouse_catalog(&f, wh_id).await;
    let updates_before = port.updated.lock().unwrap().len();

    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{}/engines/{engine_id}", primary.id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert_eq!(
        port.updated.lock().unwrap().len(),
        updates_before,
        "a refused detach must not reach the engine"
    );
    assert!(
        registry(&f)
            .get_attachment(Uuid::parse_str(&engine_id).unwrap(), primary.id)
            .await
            .unwrap()
            .is_some(),
        "the primary attachment is untouched"
    );
}

/// A catalog id/engine id pair with no attachment is a 404 (either the catalog
/// or the engine can be real; what's missing is the attachment row), and
/// nothing is written.
#[tokio::test]
async fn registry_detach_is_404_when_the_catalog_is_not_attached() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_detach_404_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("regdetach404");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let updates_before = port.updated.lock().unwrap().len();

    // Never attached.
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{j}");
    assert_eq!(
        port.updated.lock().unwrap().len(),
        updates_before,
        "a 404 must not reach the engine"
    );
}

/// The whole reason the registry route exists: `catalog_registry:update`
/// reaches it and `engine:delete` does not. (Was also proved "symmetrically
/// on the legacy route" — `engine:delete` through, `catalog_registry:update`
/// refused — before SP3c-2 Task 4 deleted that route.)
#[tokio::test]
async fn registry_detach_requires_catalog_registry_update_not_engine_delete() {
    let port = FakeEnginePort::default();
    let Some(mut f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Ad-hoc single-permission roles must exist before the real PDP snapshots
    // roles/permissions at build time (see `attach_real_pdp`).
    let registry_updater =
        session_for_with_permission(&f, "reg_detach_gate_reg_upd", "catalog_registry:update").await;
    let engine_deleter =
        session_for_with_permission(&f, "reg_detach_gate_eng_del", "engine:delete").await;
    attach_real_pdp(&mut f).await;
    let admin = session_for(&f, "reg_detach_gate_admin", true).await;

    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &admin, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("reggate");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &admin,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");

    // `catalog_registry:update` alone gets a 204 on the registry route.
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &registry_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");
    assert!(reg.get_attachment(e, c).await.unwrap().is_none());

    // Re-attach, then prove `engine:delete` alone is refused on the registry
    // route and writes nothing.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &engine_deleter,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{j}");
    assert!(
        reg.get_attachment(e, c).await.unwrap().is_some(),
        "forbidden registry detach wrote nothing"
    );
}

/// SP3c-1 task 1b: `POST /catalogs/{id}/engines/{engine_id}` is a second door
/// onto the SAME `attach_to_engine`, symmetric with the registry detach route
/// and with `/catalogs/{id}/workspaces`. Task 7 puts both attach and detach
/// buttons on one screen for one persona, so the two must be gated alike.
#[tokio::test]
async fn registry_attach_attaches_and_the_attachment_appears() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_attach_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("regattach");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );

    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["applyStatus"], "applied");
    assert_eq!(j["engineId"], serde_json::json!(engine_id));
    assert!(
        reg.get_attachment(e, c).await.unwrap().is_some(),
        "the attachment is recorded"
    );
    assert!(
        port.last_updated()
            .catalogs
            .iter()
            .any(|cat| cat.name == cat_name),
        "the engine carries the catalog"
    );
}

/// The registry attach route reaches the same `check_engine_supports` refusal
/// as the legacy route (D4/F-2): a StarRocks-only generic catalog cannot be
/// attached to a Trino engine, decided before anything is minted or written.
#[tokio::test]
async fn registry_attach_refuses_a_catalog_the_engine_cannot_read() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_attach_unreadable_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let sr_only_name = unique_catalog_name("regattachpaimon");
    let (_, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({
        "name": sr_only_name, "kind": "generic",
        "config": { "engines": ["starrocks"] }, "advanced": { "starrocks": { "type": "paimon" } }
    }))).await;
    let cid = j["id"].as_str().unwrap().to_string();
    let updates_before = port.updated.lock().unwrap().len();

    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    let msg = j["error"].as_str().unwrap();
    assert!(msg.contains("trino"), "{msg} should name the engine type");
    assert!(msg.contains(&sr_only_name), "{msg} should name the catalog");
    assert_eq!(
        port.updated.lock().unwrap().len(),
        updates_before,
        "a refused attach must not reach the engine"
    );
    assert!(
        registry(&f)
            .get_attachment(
                Uuid::parse_str(&engine_id).unwrap(),
                Uuid::parse_str(&cid).unwrap()
            )
            .await
            .unwrap()
            .is_none(),
        "nothing was written"
    );
}

/// An unknown catalog id, or an unknown engine id, is a 404 either way, and
/// nothing is written.
#[tokio::test]
async fn registry_attach_is_404_for_an_unknown_catalog_or_engine() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_attach_404_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("regattach404");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let updates_before = port.updated.lock().unwrap().len();

    // Unknown catalog, real engine.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{}/engines/{engine_id}", Uuid::new_v4()),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{j}");

    // Real catalog, unknown engine.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{cid}/engines/{}", Uuid::new_v4()),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{j}");
    assert_eq!(
        port.updated.lock().unwrap().len(),
        updates_before,
        "a 404 must not reach the engine"
    );
}

/// The whole reason the catalog-scoped attach route exists:
/// `POST /catalogs/{id}/engines/{engine_id}` takes `catalog_registry:update`
/// and refuses `engine:update`, while the engine-scoped
/// `POST /engines/{id}/catalogs/{cid}` is the mirror image. Both are live —
/// the engine-scoped one is NOT the deleted legacy route, which was its
/// `GET`/`PUT`/`DELETE` siblings (SP3c-2 Task 4). The split holds
/// symmetrically on both attach routes exactly as it does for detach.
#[tokio::test]
async fn registry_attach_requires_catalog_registry_update_not_engine_update() {
    let port = FakeEnginePort::default();
    let Some(mut f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Ad-hoc single-permission roles must exist before the real PDP snapshots
    // roles/permissions at build time (see `attach_real_pdp`).
    let registry_updater =
        session_for_with_permission(&f, "reg_attach_gate_reg_upd", "catalog_registry:update").await;
    let engine_updater =
        session_for_with_permission(&f, "reg_attach_gate_eng_upd", "engine:update").await;
    attach_real_pdp(&mut f).await;
    let admin = session_for(&f, "reg_attach_gate_admin", true).await;

    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &admin, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("reggateattach");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &admin,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );

    // `engine:update` alone is refused on the registry route and writes
    // nothing.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &engine_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{j}");
    assert!(
        reg.get_attachment(e, c).await.unwrap().is_none(),
        "forbidden registry attach wrote nothing"
    );

    // `catalog_registry:update` alone gets a 201 on the registry route.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &registry_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert!(reg.get_attachment(e, c).await.unwrap().is_some());

    // Detach (as admin) so the engine-scoped route can attach it again.
    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");

    // The engine-scoped route is the mirror image: `engine:update` gets
    // through, `catalog_registry:update` alone is refused.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &engine_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert!(reg.get_attachment(e, c).await.unwrap().is_some());

    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{cid}/engines/{engine_id}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &registry_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{j}");
    assert!(
        reg.get_attachment(e, c).await.unwrap().is_none(),
        "forbidden legacy attach wrote nothing"
    );
}

/// D3's third present-state, which D3's letter does not name and which nothing
/// drove through `converge_attachment` before (fix round 1 review, F4):
/// `Drift::Unverifiable` — every comparable plain key agrees, but a key the FE
/// MASKS (`aws.s3.access_key`) is present, so "matching" cannot be concluded.
/// Egret ALTERs rather than refusing, deliberately: refusing on "cannot tell"
/// would make a legitimate re-attach of an S3-backed catalog permanently
/// impossible, since nothing an operator can do makes a masked value
/// comparable. This pins the arm so a future change cannot flip it in silence.
#[tokio::test]
async fn an_unverifiable_catalog_is_altered_not_refused() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_unverifiable", true).await;
    let (engine_a, _) = create_starrocks_via_api(&f, &token).await;
    let (engine_b, _) = create_starrocks_via_api(&f, &token).await;

    // A hive catalog over a storage connection: its render carries
    // `aws.s3.access_key` (Plain, so it IS compared, and MASKED by the FE) and
    // `aws.s3.secret_key` (Secret, so it is never compared). That pairing is
    // the only way to reach `Unverifiable`.
    let storage = seed_storage_conn_with_secret(&f).await;
    let name = unique_catalog_name("hv");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(
            serde_json::json!({ "name": name, "kind": "hive", "config": {
                "metastore": { "type": "hive", "uri": "thrift://hms:9083" },
                "storage_connection_id": storage,
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();

    // Put the catalog on the FE by attaching it to engine A — so what the fake
    // holds is what egret's own renderer produces, byte for byte. Hand-writing
    // the property map here would let the premise drift away from the renderer
    // and quietly stop being `Unverifiable` at all.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_a}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(f.starrocks_ddl.created(), vec![name.clone()]);
    f.starrocks_ddl.clear_executed();

    // Now attach it to engine B, which shares the fake's FE. From B's side
    // this is a catalog it did not create (`already_ours` is false: no
    // `applied_at` on B's row), present, with every plain key equal and
    // `aws.s3.access_key` masked — `Drift::Unverifiable`.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_b}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(
        j["applyStatus"], "applied",
        "an unverifiable catalog is converged, not refused; {j}"
    );
    assert!(j["applyError"].is_null(), "no refusal was recorded; {j}");
    let sent = f.starrocks_ddl.executed();
    assert!(
        sent.iter().any(|s| s.starts_with("ALTER CATALOG")),
        "the masked key forces an ALTER rather than a no-op; sent={sent:?}"
    );
    assert!(
        sent.iter()
            .all(|s| !s.starts_with("CREATE EXTERNAL CATALOG")),
        "the catalog was already there; sent={sent:?}"
    );
    // And the ALTER really delivered egret's own credential, which is the
    // whole reason "cannot tell" converges instead of refusing.
    assert_eq!(
        f.starrocks_ddl.catalog(&name).unwrap()["aws.s3.secret_key"],
        "STORAGESECRETKEY"
    );
}

/// D7: the DDL succeeded and the attachment row could not be settled — egret
/// drops the catalog again rather than leaving one nothing will ever prune.
/// SP3a's comment said "the next apply of this engine removes it again"; that
/// is true of a Trino ConfigMap and false of a StarRocks FE, where nothing
/// prunes anything. The attachment row is deleted out from under the apply by
/// the fake's `on_create` hook, which runs between the CREATE reaching the FE
/// and egret recording it.
#[tokio::test]
async fn a_catalog_egret_cannot_remember_is_dropped_again() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_compensate", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("es");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(elasticsearch_body(&name)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );

    let pool = f.db.pool.clone();
    f.starrocks_ddl.on_create(move |_name| {
        let pool = pool.clone();
        Box::pin(async move {
            sqlx::query(
                "DELETE FROM registry.engine_catalog_attachments \
                  WHERE engine_id = $1 AND catalog_id = $2",
            )
            .bind(e)
            .bind(c)
            .execute(&pool)
            .await
            .expect("delete the attachment mid-apply");
        })
    });

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    // Not a success, and pinned exactly: `set_apply_applied` answers
    // `NotFound` for a row that is gone, which the route maps to 404. The
    // invariant is that it is not a 201 — a response claiming the attach
    // happened while the FE has been rolled back would be the worst of both.
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an attach egret could not record is not a success; {j}"
    );
    let sent = f.starrocks_ddl.executed();
    assert!(
        sent.iter()
            .any(|s| s.starts_with("CREATE EXTERNAL CATALOG")),
        "sent={sent:?}"
    );
    assert!(
        sent.iter().any(|s| s.starts_with("DROP CATALOG IF EXISTS")),
        "the compensating drop ran: {sent:?}"
    );
    assert!(
        f.starrocks_ddl.catalog(&name).is_none(),
        "nothing left on the FE"
    );
    assert!(registry(&f).get_attachment(e, c).await.unwrap().is_none());
}

/// D6: no secret an FE error carries reaches `apply_error` or the response —
/// including the residual `starrocks_ddl`'s own redaction documents and cannot
/// close: a SHORT, unquoted secret in an otherwise well-formed message. Only
/// the caller's `FeError::scrubbed(&secrets)` closes that, which is why this
/// test fails the moment an apply path formats an `FeError` any other way.
#[tokio::test]
async fn an_fe_error_stored_on_the_attachment_carries_no_ddl_and_no_secret() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_secret", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    // Short and unquoted where the FE repeats it: under `MIN_OPAQUE_RUN`, so
    // no shape-based pass in `starrocks_ddl` can see it.
    let secret_plaintext = "pw-a1b2";
    let secret_id = seed_secret(&f, secret_plaintext).await;
    let name = unique_catalog_name("pg");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(jdbc_body(&name, secret_id)),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();

    // What StarRocks really does: quote the statement back, secrets and all.
    f.starrocks_ddl.fail_matching(
        "CREATE EXTERNAL CATALOG",
        &format!(
            "ERROR 1064 (HY000): Getting analyzing error. Detail message: \
             CREATE EXTERNAL CATALOG `x` PROPERTIES (\"password\" = \"{secret_plaintext}\") \
             rejected, password {secret_plaintext} is not accepted"
        ),
    );
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let err = registry(&f)
        .get_attachment(
            Uuid::parse_str(&engine_id).unwrap(),
            Uuid::parse_str(&cid).unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
        .apply_error
        .expect("the failure was recorded");
    assert!(
        !err.contains(secret_plaintext),
        "the secret leaked into apply_error: {err}"
    );
    assert!(
        !j["applyError"].as_str().unwrap().contains(secret_plaintext),
        "…and into the response: {j}"
    );
    assert!(
        err.contains("Getting analyzing error"),
        "the diagnosis survived: {err}"
    );
    assert!(
        err.contains("FE error 1064"),
        "the FE's own code survived: {err}"
    );
    assert!(
        err.contains("<redacted>"),
        "the quoted echo of the statement was removed: {err}"
    );
}

#[tokio::test]
async fn workspace_assignment() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_ws_admin", true).await;
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": unique_catalog_name("b"), "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let ws1 = seed_workspace(&f).await;
    let ws2 = seed_workspace(&f).await;

    let (status, j) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [ws1, ws2] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j.as_array().unwrap().len(), 2);
    let (status, j) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [ws2] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(j[0]["workspaceId"], serde_json::json!(ws2));
    let (_, j) = call(
        &f,
        "GET",
        &format!("/catalogs?workspaceId={ws2}"),
        &token,
        None,
    )
    .await;
    assert!(j
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == serde_json::json!(cid)));
    let (status, _) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [Uuid::new_v4()] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown workspace");
    let (status, j) = call(&f, "DELETE", &format!("/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(j["workspaces"][0]["id"], serde_json::json!(ws2));
}

/// The route side of D3 (SP4a Task 6 fix round 1, Important 2): the review
/// found `workspace_assignment` above proves the `registry.catalog_workspaces`
/// side but nothing ever asserted the `perm.grants` side — that the PUT
/// route actually creates the implicit read grant, and that unassigning
/// removes it, through the real HTTP handler rather than a direct call to
/// `sync_assignment_grants`.
#[tokio::test]
async fn workspace_assignment_route_syncs_perm_grants() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_ws_perm_admin", true).await;
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(
            serde_json::json!({ "name": unique_catalog_name("gp"), "kind": "tpch", "config": {} }),
        ),
    )
    .await;
    let cid = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let ws = seed_workspace(&f).await;
    let perm = egret_perm::PermStore::new(f.db.pool.clone());
    let implicit_for = |grants: &[egret_perm::Grant], catalog: Uuid| -> Vec<egret_perm::Grant> {
        grants
            .iter()
            .filter(|g| {
                g.scope.catalog_id == catalog
                    && matches!(
                        g.origin,
                        egret_perm::GrantOrigin::WorkspaceAssignment { .. }
                    )
            })
            .cloned()
            .collect()
    };

    let (status, _) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [ws] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after_assign = implicit_for(&perm.list_grants().await.unwrap(), cid);
    assert_eq!(
        after_assign.len(),
        1,
        "the route must create exactly one implicit grant"
    );
    assert_eq!(after_assign[0].actions, vec![egret_perm::Action::Read]);
    assert!(matches!(
        after_assign[0].origin,
        egret_perm::GrantOrigin::WorkspaceAssignment { workspace_id } if workspace_id == ws
    ));

    let (status, _) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after_unassign = implicit_for(&perm.list_grants().await.unwrap(), cid);
    assert!(
        after_unassign.is_empty(),
        "unassigning through the route must remove the implicit grant"
    );
}

/// The rollback side of D3 (SP4a Task 6 fix round 1, Important 2). The
/// review's own words: "a rollback nobody has ever observed is a
/// hypothesis." This calls the exact two `_in_tx` functions the handler
/// composes (`RegistryStore::set_catalog_workspaces_in_tx` then
/// `egret_perm::sync_assignment_grants_in_tx`) inside one transaction opened
/// the same way the handler opens it, deliberately making the SECOND write
/// fail with a real constraint violation (a workspace id that satisfies no
/// FK) after the FIRST write has already gone through inside the
/// (uncommitted) transaction — then rolls back exactly as the handler does
/// when the `?` on the second call returns before `tx.commit()`, and proves
/// the first write did not survive.
#[tokio::test]
async fn a_failed_grant_sync_rolls_back_the_assignment_write() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_ws_rollback_admin", true).await;
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(
            serde_json::json!({ "name": unique_catalog_name("rb"), "kind": "tpch", "config": {} }),
        ),
    )
    .await;
    let cid = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let valid_ws = seed_workspace(&f).await;
    // Never inserted into `workspaces` — `perm.grants.origin_workspace_id
    // REFERENCES workspaces(id)` (migrations/0154_perm_model.sql) refuses it.
    // `registry.catalog_workspaces` carries no such FK, so the first write
    // below succeeds on its own before the second one fails — the scenario
    // the handler's transaction exists to prevent from ever being observed
    // as two independent facts.
    let bogus_ws = Uuid::new_v4();

    let mut tx = f.db.pool.begin().await.unwrap();
    egret_catalog_registry::RegistryStore::set_catalog_workspaces_in_tx(&mut tx, cid, &[valid_ws])
        .await
        .expect("the registry write alone succeeds");
    let grant_sync_result =
        egret_perm::sync_assignment_grants_in_tx(&mut tx, cid, &[bogus_ws]).await;
    assert!(
        grant_sync_result.is_err(),
        "the bogus workspace id must be refused by the FK, not silently accepted"
    );
    // What the handler does on `?`: never calls `tx.commit()`. Rolled back
    // explicitly here for a deterministic assertion rather than relying on
    // `Transaction::drop`'s background rollback racing the query below.
    tx.rollback().await.unwrap();

    let assigned = registry(&f).list_catalog_workspaces(cid).await.unwrap();
    assert!(
        assigned.is_empty(),
        "the registry write must not survive a grant-sync failure in the same transaction, \
         got: {assigned:?}"
    );
}

/// SP4a Task 15, D3 defect 1 (measured on the live gate's THIRD RUN,
/// `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md:801`):
/// `set_catalog_workspaces` was writing the implicit grant row correctly
/// (`workspace_assignment_route_syncs_perm_grants` above proves that) but
/// never compiling it — the member's query was DENIED immediately after the
/// assign committed, and only started passing once an UNRELATED membership
/// write recompiled the profile and swept the stale assignment in.
///
/// With a REAL `ProfileCompilerPort` wired (`RealProfileCompiler`, not the
/// recording fake the engine-bind tests use), this asserts against
/// `policy.authz_policies` directly — the thing the engine actually reads —
/// with NO call to `recompile_service`/`POST /perm/compile/{id}` anywhere in
/// between the PUT and the read. It also proves the mirror direction:
/// unassigning reaps the compiled policy in the same request.
#[tokio::test]
async fn assigning_a_catalog_compiles_its_implicit_grant_with_no_manual_step() {
    let Some(mut f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let compiler: Arc<dyn egret_engine_mgmt::ProfileCompilerPort> = Arc::new(RealProfileCompiler {
        perm: egret_perm::PermStore::new(f.db.pool.clone()),
        policy: egret_policy::PolicyStore::new(f.db.clone()),
    });
    f.perm_compiler = Some(compiler);
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        f.ranger_enabled,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
    );
    let token = session_for(&f, "d3_recompile_admin", true).await;

    // A StarRocks profile with one engine bound to it — no tag service
    // needed, D3 is about an access policy (same choice
    // `egret-perm-compile/tests/workspace_members.rs` makes).
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = egret_policy_api::PolicyPort::create_service(
        &policy,
        &format!("d3sr_{}", Uuid::new_v4().simple()),
        "starrocks",
        &serde_json::json!({}),
    )
    .await
    .expect("create service");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(format!("d3-eng-{}", Uuid::new_v4().simple()))
    .bind(service.id)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed engine bound to the profile");

    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(
            serde_json::json!({ "name": unique_catalog_name("d3c"), "kind": "tpch", "config": {} }),
        ),
    )
    .await;
    let cid = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(cid)
    .execute(&f.db.pool)
    .await
    .expect("attach catalog to the bound engine");

    // A workspace with ONE real member — an empty-member workspace compiles
    // its implicit grant to a SKIP, not a policy (`compile_desired`'s own
    // "nothing to name" branch), which would make this test pass for the
    // wrong reason.
    let ws = seed_workspace(&f).await;
    let username = format!("d3user_{}", Uuid::new_v4().simple());
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'D3 User',$2) \
         RETURNING id",
    )
    .bind(format!("{username}@egret.local"))
    .bind(&username)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed user");
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'workspace', $2 FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(user_id)
    .bind(ws)
    .execute(&f.db.pool)
    .await
    .expect("seed membership");
    assert_eq!(res.rows_affected(), 1, "the membership row must exist");

    // THE ASSERTION. Assign, then read `policy.authz_policies` — nothing
    // between the PUT and this read calls any recompile.
    let (status, _) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [ws] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let grant_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment'",
    )
    .bind(cid)
    .fetch_one(&f.db.pool)
    .await
    .expect("the implicit grant row exists");
    let policy_name = format!("{}{}", egret_policy::COMPILER_GRANT_PREFIX, grant_id);
    let policy_row: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service.id)
    .bind(&policy_name)
    .fetch_optional(&f.db.pool)
    .await
    .unwrap();
    let policy_row = policy_row.expect(
        "assigning the catalog through the route must compile the implicit grant into \
         policy.authz_policies immediately, with no manual compile step",
    );
    let items = policy_row["policyItems"].as_array().expect("policyItems");
    assert_eq!(
        items.len(),
        1,
        "one expanded item, not one per user: {policy_row:#}"
    );
    let named_users: Vec<String> = items[0]["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        named_users,
        vec![username.clone()],
        "the compiled policy must name the member's stored username"
    );

    // The mirror direction: unassigning reaps the compiled policy too, with
    // no manual step either.
    let (status, _) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &token,
        Some(serde_json::json!({ "workspaceIds": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let reaped: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service.id)
    .bind(&policy_name)
    .fetch_optional(&f.db.pool)
    .await
    .unwrap();
    assert!(
        reaped.is_none(),
        "unassigning through the route must reap the compiled policy, not just the grant row"
    );
}

#[tokio::test]
async fn new_catalog_routes_enforce_their_permission_gates() {
    let port = FakeEnginePort::default();
    let Some(mut f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Ad-hoc single-permission roles must exist before the real PDP snapshots
    // roles/permissions at build time (see `attach_real_pdp`).
    let engine_updater = session_for_with_permission(&f, "cat_gate_eng_upd", "engine:update").await;
    let registry_updater =
        session_for_with_permission(&f, "cat_gate_reg_upd", "catalog_registry:update").await;
    let registry_reader =
        session_for_with_permission(&f, "cat_gate_reg_read", "catalog_registry:read").await;
    attach_real_pdp(&mut f).await;
    let admin = session_for(&f, "cat_gate_admin", true).await;

    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &admin, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("gate");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &admin,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );

    // Attach requires `engine:update`; `catalog_registry:update` alone is not
    // enough — the two gates are distinct even though both mutate.
    let (status, _) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &registry_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        reg.get_attachment(e, c).await.unwrap().is_none(),
        "forbidden attach wrote nothing"
    );

    // Positive: a session holding exactly `engine:update` (not platform_admin)
    // gets through — proves the gate checks the real key, not just "is admin".
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &engine_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["applyStatus"], "applied");

    // Retry requires `catalog_registry:update`; `engine:update` alone is not enough.
    let before = reg.get_attachment(e, c).await.unwrap().unwrap();
    let (status, _) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}/apply"),
        &engine_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let after = reg.get_attachment(e, c).await.unwrap().unwrap();
    assert_eq!(
        (before.apply_status, before.apply_error),
        (after.apply_status, after.apply_error),
        "forbidden retry changed nothing"
    );

    // PUT workspaces requires `catalog_registry:update`; `catalog_registry:read`
    // alone is not enough.
    let ws_id = seed_workspace(&f).await;
    let (status, _) = call(
        &f,
        "PUT",
        &format!("/catalogs/{cid}/workspaces"),
        &registry_reader,
        Some(serde_json::json!({ "workspaceIds": [ws_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        reg.list_catalog_workspaces(c).await.unwrap().is_empty(),
        "forbidden assignment wrote nothing"
    );

    // GET catalog workspaces requires `catalog_registry:read`; `engine:update`
    // is not enough.
    let (status, _) = call(
        &f,
        "GET",
        &format!("/catalogs/{cid}/workspaces"),
        &engine_updater,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn retry_on_a_deleting_engine_is_409_and_writes_nothing() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_retry_deleting_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("del");
    let (_, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let reg = registry(&f);
    let (e, c) = (
        Uuid::parse_str(&engine_id).unwrap(),
        Uuid::parse_str(&cid).unwrap(),
    );
    // Leave the attachment in a known `failed` state so a wrongful write by
    // the 409 path is visible: `set_apply_failed` would flip it back to
    // `failed` with a different message, which the equality check below
    // catches either way, but starting from `failed` also confirms the
    // conflict path does not "helpfully" flip it to `applied`.
    reg.set_apply_failed(e, c, "earlier failure").await.unwrap();
    let before = reg.get_attachment(e, c).await.unwrap().unwrap();

    sqlx::query("UPDATE engine_mgmt.engines SET status = 'deleting' WHERE id = $1")
        .bind(e)
        .execute(&f.db.pool)
        .await
        .unwrap();
    let updates_before = port.updated.lock().unwrap().len();

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(
        port.updated.lock().unwrap().len(),
        updates_before,
        "a refused apply on a deleting engine must not reach the provisioner"
    );
    let after = reg.get_attachment(e, c).await.unwrap().unwrap();
    assert_eq!(
        (before.apply_status, before.apply_error),
        (after.apply_status, after.apply_error),
        "a refused apply on a deleting engine must leave the attachment unchanged"
    );
}

// ── SP3c-1 Task 2: `is_primary` on the attachment, `catalogs` on engine create ──

/// `POST /engines` for a Trino engine, with whatever catalog shape the caller
/// wants (`warehouseId`, `catalogs`, or both).
async fn create_engine_body(
    f: &Fixture,
    token: &str,
    name: &str,
    catalog_shape: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut body = serde_json::json!({
        "name": name,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
    });
    for (k, v) in catalog_shape.as_object().expect("object") {
        body[k] = v.clone();
    }
    call(f, "POST", "/engines", token, Some(body)).await
}

async fn engine_rows_named(f: &Fixture, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM engine_mgmt.engines WHERE name = $1")
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .expect("count engines")
}

async fn engines_fixture() -> Option<Fixture> {
    engines_fixture_with(FakeEnginePort::default()).await
}

/// `engines_fixture` for a test that needs to look at what the provisioner was
/// asked to do — "nothing was provisioned" is only checkable from this handle.
async fn engines_fixture_with(port: FakeEnginePort) -> Option<Fixture> {
    setup_full(
        Some(Arc::new(port)),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
}

/// THE BACK-COMPAT FLOOR. `frontend/tests/e2e/helpers.ts` and
/// `embedded-catalog.spec.ts` both still POST a bare `warehouseId`, and so does
/// every other engine test in this file. Making `warehouseId` optional is a
/// widening; a widening that breaks the old shape is not a widening.
#[tokio::test]
async fn create_engine_still_accepts_a_bare_warehouse_id() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_bare_wh", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "bare warehouseId; body={j}");
    assert_eq!(
        j["catalogAttachErrors"],
        serde_json::json!([]),
        "nothing was asked to attach; body={j}"
    );
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // There is no `warehouseId` on the wire any more (SP3c-2 Task 6); the
    // engine's warehouse comes from the PRIMARY catalog, read the same way
    // production does.
    assert_eq!(
        registry(&f)
            .primary_warehouse_id(Uuid::parse_str(&engine_id).unwrap())
            .await
            .expect("primary_warehouse_id"),
        Some(wh_id),
        "the auto-wired warehouse catalog is the engine's primary"
    );

    let cats = registry(&f)
        .list_engine_attachments(Uuid::parse_str(&engine_id).unwrap())
        .await
        .expect("list attachments");
    assert_eq!(cats.len(), 1, "{cats:?}");
    assert!(
        cats[0].is_primary(),
        "the auto-wired warehouse catalog still leads"
    );
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    let row = registry(&f)
        .get_attachment(Uuid::parse_str(&engine_id).unwrap(), wh_cat.id)
        .await
        .expect("read attachment")
        .expect("attached");
    assert!(row.is_primary, "the COLUMN, not a derived guess");
}

/// `catalogs` attaches several and marks exactly one primary. The primary is
/// attached by the create path itself; the rest go through the same
/// `attach_to_engine` the attach route uses.
#[tokio::test]
async fn create_engine_with_catalogs_marks_exactly_one_primary() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_catalogs", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    // A first engine is what puts the warehouse's catalog in the registry.
    let first = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &first,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let wh_cat = warehouse_catalog(&f, wh_id).await;

    let bench = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": bench, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let bench_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "catalogs": [
            { "catalogId": bench_id, "primary": false },
            { "catalogId": wh_cat.id, "primary": true },
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body={j}");
    assert_eq!(
        j["catalogAttachErrors"],
        serde_json::json!([]),
        "both catalogs attached; body={j}"
    );
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    // There is no `warehouseId` on the wire any more (SP3c-2 Task 6); the
    // engine's warehouse comes from the PRIMARY catalog, read the same way
    // production does.
    assert_eq!(
        registry(&f)
            .primary_warehouse_id(engine_id)
            .await
            .expect("primary_warehouse_id"),
        Some(wh_id),
        "the engine's warehouse comes from the PRIMARY catalog"
    );

    let cats = registry(&f)
        .list_engine_attachments(engine_id)
        .await
        .expect("list attachments");
    assert_eq!(cats.len(), 2, "both catalogs on the engine; {cats:?}");
    assert_eq!(
        cats.iter().map(|a| a.is_primary()).collect::<Vec<_>>(),
        vec![true, false],
        "exactly one primary, and it leads the render order; {cats:?}"
    );
    assert_eq!(
        cats[0].catalog.id, wh_cat.id,
        "the catalog the body marked primary is the one that leads"
    );
}

/// Two primaries and no primary are both refused, by the SAME resolution
/// function, before anything is written.
#[tokio::test]
async fn create_engine_refuses_two_primaries_and_refuses_none() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_primaries", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let first = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &first,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    let bench = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": bench, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let bench_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let two = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &two,
        serde_json::json!({ "catalogs": [
            { "catalogId": wh_cat.id, "primary": true },
            { "catalogId": bench_id, "primary": true },
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "two primaries; body={j}");
    assert_eq!(
        j["error"],
        serde_json::json!("bad request: only one attached catalog may be primary"),
        "body={j}"
    );
    assert_eq!(engine_rows_named(&f, &two).await, 0, "nothing was created");

    let none = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &none,
        serde_json::json!({ "catalogs": [{ "catalogId": wh_cat.id, "primary": false }]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no primary; body={j}");
    assert_eq!(
        j["error"],
        serde_json::json!("bad request: one attached catalog must be primary"),
        "body={j}"
    );
    assert_eq!(engine_rows_named(&f, &none).await, 0, "nothing was created");

    // Neither shape at all.
    let empty = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(&f, &token, &empty, serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no catalogs at all; body={j}"
    );
    assert_eq!(
        j["error"],
        serde_json::json!(
            "bad request: an engine needs a primary catalog: send `catalogs` with one marked primary"
        ),
        "body={j}"
    );
    assert_eq!(
        engine_rows_named(&f, &empty).await,
        0,
        "nothing was created"
    );
}

/// A non-warehouse catalog cannot lead: the engine's Ranger role
/// (`ensure_engine_role`) and a StarRocks engine's builtin storage volume are
/// both derived from the PRIMARY attachment's warehouse (SP3c-2 Task 5 moved
/// those two reads off `engines.warehouse_id` and onto the attachment, which
/// is why the constraint outlives the column). Letting one lead is a feature
/// deferred past SP4 (user decision 2026-09-23). The refusal must SAY so,
/// because the wizard repeats the reason next to the disabled choice (D2, user
/// decision 2026-09-22) — a bare "bad request" would leave it inventing one.
///
/// The refusal is decided from the catalog row BEFORE any registry row is
/// written, so a refused create has nothing to roll back (SP3a F-2); the
/// `engine_rows_named == 0` assertion at the end is the observable half of it.
#[tokio::test]
async fn create_engine_refuses_a_non_warehouse_primary_with_the_reason() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_nonwh", true).await;
    let bench = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": bench, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let bench_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "catalogs": [{ "catalogId": bench_id, "primary": true }]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    let msg = j["error"].as_str().expect("error message");
    assert!(
        msg.contains("must be an egret warehouse catalog"),
        "says what is wrong; msg={msg}"
    );
    assert!(msg.contains("Ranger"), "names Ranger; msg={msg}");
    assert!(msg.contains("storage"), "names storage; msg={msg}");
    assert_eq!(engine_rows_named(&f, &name).await, 0, "nothing was created");
}

/// The lower half of "exactly one primary per engine". There is no anchor row
/// for a CHECK, so migration 0150's PARTIAL UNIQUE INDEX is what turns a second
/// primary into a refusal instead of two rows that both sort first — which for
/// Trino means two catalogs fighting over `C0`.
#[tokio::test]
async fn a_second_primary_attachment_is_refused_by_the_index() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_second_primary", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let reg = registry(&f);
    let bench = reg
        .create_catalog(
            &unique_catalog_name("bench"),
            &egret_catalog_registry::CatalogDef::parse(
                "tpch",
                &serde_json::json!({}),
                &serde_json::json!(null),
            )
            .unwrap(),
            None,
        )
        .await
        .expect("create catalog");

    let err = reg
        .insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id,
            catalog_id: bench.id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: true,
            is_primary: true,
            created_at: None,
            apply_status: egret_catalog_registry::store::apply_status::APPLIED,
        })
        .await
        .expect_err("a second primary must be refused");
    assert!(
        matches!(err, egret_catalog_registry::RegistryError::AlreadyExists(ref m)
            if m.contains("already has a primary catalog")),
        "{err:?}"
    );

    // And the refusal wrote nothing: the engine still has exactly one catalog,
    // the warehouse's, and it is still the primary.
    let listed = reg
        .list_engine_attachments(engine_id)
        .await
        .expect("list attachments");
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    assert_eq!(
        listed
            .iter()
            .map(|a| (a.catalog.id, a.is_primary()))
            .collect::<Vec<_>>(),
        vec![(wh_cat.id, true)],
    );
}

/// `CatalogAttachmentDto` — what `GET /catalogs/{id}` lists under `engines`,
/// and what Task 7's primary badge reads — reports the column.
#[tokio::test]
async fn attachment_dto_reports_is_primary() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_dto", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let wh_cat = warehouse_catalog(&f, wh_id).await;

    let (status, j) = call(&f, "GET", &format!("/catalogs/{}", wh_cat.id), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(
        j["engines"][0]["engineId"],
        serde_json::json!(engine_id),
        "body={j}"
    );
    assert_eq!(
        j["engines"][0]["isPrimary"],
        serde_json::json!(true),
        "the warehouse catalog leads its engine; body={j}"
    );

    // A catalog attached afterwards is NOT primary, through the same DTO.
    let bench = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": bench, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let bench_id = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{bench_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "attach; body={j}");
    assert_eq!(
        j["isPrimary"],
        serde_json::json!(false),
        "an attach never leads; body={j}"
    );
}

/// `CatalogAttachmentDto.createdAt` — the clock the Catalogs list measures a
/// `pending` attachment against (SP3c-1 Task 4, fix round 1 F2).
///
/// The point is not that the field is present; it is that it belongs to the
/// ATTACHMENT and not to the catalog. Attaching an existing catalog inserts an
/// `engine_catalog_attachments` row and never writes `registry.catalogs
/// .updated_at`, so a UI that judged "has this been pending too long?" from the
/// catalog's clock would read an arbitrarily old timestamp at the exact moment
/// the wait began — and cry "still pending" throughout the StarRocks bootstrap
/// the grace window exists to cover. This asserts the ordering that makes the
/// catalog clock unusable and the attachment clock correct: the attachment is
/// created STRICTLY after the catalog it attaches, and the catalog's own
/// `updatedAt` does not move when it is attached.
#[tokio::test]
async fn attachment_dto_reports_its_own_created_at() {
    let Some(f) = engines_fixture().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_att_created", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    let bench = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": bench, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let bench_id = j["id"].as_str().unwrap().to_string();
    let catalog_updated_at = j["updatedAt"].as_str().unwrap().to_string();

    // A real gap, so "strictly after" cannot be satisfied by two readings of
    // the same clock tick. `now()` is transaction time and these are separate
    // transactions, but a coincidence is exactly the failure this ordering
    // assertion would hide.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{bench_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "attach; body={j}");
    let attach_created_at = j["createdAt"]
        .as_str()
        .expect("the attach response carries the attachment's createdAt")
        .to_string();

    // Re-read the catalog: the attach must not have touched its clock…
    let (status, j) = call(&f, "GET", &format!("/catalogs/{bench_id}"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(
        j["updatedAt"].as_str().unwrap(),
        catalog_updated_at,
        "attaching a catalog must not bump the catalog's own updatedAt — the whole \
         reason its clock cannot stand in for the attachment's; body={j}"
    );

    // …and the listed attachment reports the same, strictly later, timestamp.
    let listed = j["engines"][0]["createdAt"]
        .as_str()
        .expect("the listed attachment carries createdAt");
    assert_eq!(listed, attach_created_at, "body={j}");
    assert!(
        listed > catalog_updated_at.as_str(),
        "the attachment was created after the catalog it attaches: attachment={listed} \
         catalog={catalog_updated_at}",
    );
}

// ── SP3c-1 Task 2, fix round 1 ────────────────────────────────────────────────

/// F1: a mistyped catalog id must not buy you a running cluster.
///
/// Before the fix the primary's id was resolved up front (a clean refusal) and
/// every OTHER id was resolved at attach time, AFTER the catalog client was
/// minted, the engine row written and the cluster provisioned — so the same typo
/// was a 400 on one entry and a 201-with-a-message on the next three. The
/// provisioner handle is the point of this test: "nothing was created" is not
/// the same claim as "nothing was provisioned", and it was the second one that
/// was false.
#[tokio::test]
async fn an_unknown_catalog_id_refuses_the_create_before_anything_is_provisioned() {
    let port = FakeEnginePort::default();
    let Some(f) = engines_fixture_with(port.clone()).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_unknown_cat", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    // A first engine puts the warehouse's catalog in the registry.
    let first = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &first,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    let provisions_before = port.provisioned.lock().unwrap().len();
    let ghost = Uuid::new_v4();

    // The typo is on a NON-primary entry: the arm that used to get through.
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "catalogs": [
            { "catalogId": wh_cat.id, "primary": true },
            { "catalogId": ghost, "primary": false },
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert_eq!(
        j["error"],
        serde_json::json!(format!("bad request: no such catalog {ghost}")),
        "names WHICH id is unknown; body={j}"
    );
    assert_eq!(engine_rows_named(&f, &name).await, 0, "no engine row");
    assert_eq!(
        port.provisioned.lock().unwrap().len(),
        provisions_before,
        "a refused create must not have provisioned a cluster"
    );

    // Same refusal when the typo is on the PRIMARY entry (F9: named, not a bare
    // 404 — with four ids in a body the caller has to be told which one).
    let name2 = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name2,
        serde_json::json!({ "catalogs": [{ "catalogId": ghost, "primary": true }]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert_eq!(
        j["error"],
        serde_json::json!(format!("bad request: no such catalog {ghost}")),
        "body={j}"
    );
    assert_eq!(engine_rows_named(&f, &name2).await, 0, "no engine row");
    assert_eq!(
        port.provisioned.lock().unwrap().len(),
        provisions_before,
        "still nothing provisioned"
    );

    // A repeated id is the same outcome by a different route: it would attach
    // once and 409 the second time, after the cluster was up.
    let name3 = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name3,
        serde_json::json!({ "catalogs": [
            { "catalogId": wh_cat.id, "primary": true },
            { "catalogId": wh_cat.id, "primary": false },
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert_eq!(
        j["error"],
        serde_json::json!(format!(
            "bad request: catalog {} is listed twice",
            wh_cat.id
        )),
        "body={j}"
    );
    assert_eq!(engine_rows_named(&f, &name3).await, 0, "no engine row");
    assert_eq!(
        port.provisioned.lock().unwrap().len(),
        provisions_before,
        "still nothing provisioned"
    );
}

/// F2: the field nothing populated. A create whose engine comes up fine but
/// whose extra catalog genuinely cannot be attached returns 201 AND says which
/// catalog it is and why — this is the only state `catalogAttachErrors` exists
/// for, and until now no test entered it, so an implementation that never
/// populated the field passed every one.
///
/// The vehicle is `can_read`: a `generic` catalog declaring only `starrocks`
/// cannot be attached to a Trino engine, and `check_engine_supports` refuses it
/// before anything is minted — a failure that belongs to the catalog, with the
/// engine already provisioned.
#[tokio::test]
async fn a_catalog_that_cannot_attach_is_reported_on_the_created_engine() {
    let port = FakeEnginePort::default();
    let Some(f) = engines_fixture_with(port.clone()).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_attach_err", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let first = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &first,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let wh_cat = warehouse_catalog(&f, wh_id).await;

    let sr_only_name = unique_catalog_name("paimon");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": sr_only_name, "kind": "generic",
            "config": { "engines": ["starrocks"] },
            "advanced": { "starrocks": { "type": "paimon" } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let sr_only = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let provisions_before = port.provisioned.lock().unwrap().len();
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "catalogs": [
            { "catalogId": wh_cat.id, "primary": true },
            { "catalogId": sr_only, "primary": false },
        ]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the engine itself came up; body={j}"
    );
    assert_eq!(
        port.provisioned.lock().unwrap().len(),
        provisions_before + 1,
        "the cluster WAS provisioned — this is not a refusal"
    );
    let errs = j["catalogAttachErrors"]
        .as_array()
        .expect("catalogAttachErrors is always an array");
    assert_eq!(errs.len(), 1, "exactly the one that could not attach; {j}");
    assert_eq!(errs[0]["catalogId"], serde_json::json!(sr_only), "{j}");
    let msg = errs[0]["message"].as_str().expect("message");
    assert!(msg.contains("trino"), "names the engine type; msg={msg}");
    assert!(msg.contains(&sr_only_name), "names the catalog; msg={msg}");

    // And the engine really is missing it: the primary attached, the other did
    // not, and the reported catalog can be retried through the attach route.
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let listed = registry(&f)
        .list_engine_attachments(engine_id)
        .await
        .expect("list");
    assert_eq!(
        listed
            .iter()
            .map(|a| (a.catalog.id, a.is_primary()))
            .collect::<Vec<_>>(),
        vec![(wh_cat.id, true)],
        "only the primary is on the engine"
    );
}

/// F7: `warehouseId` and `catalogs` together. `catalogs` supersedes
/// `warehouseId`, so honouring one and dropping the other without a word is how
/// an integrator spends an afternoon working out which warehouse the engine got.
#[tokio::test]
async fn create_engine_refuses_both_warehouse_id_and_catalogs() {
    let port = FakeEnginePort::default();
    let Some(f) = engines_fixture_with(port.clone()).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_both_shapes", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let first = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &first,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    let provisions_before = port.provisioned.lock().unwrap().len();

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({
            "warehouseId": wh_id,
            "catalogs": [{ "catalogId": wh_cat.id, "primary": true }],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    let msg = j["error"].as_str().expect("error");
    assert!(
        msg.contains("not both"),
        "says the two shapes are exclusive; msg={msg}"
    );
    assert!(
        msg.contains("supersedes"),
        "says which one wins so the caller knows what to drop; msg={msg}"
    );
    assert_eq!(engine_rows_named(&f, &name).await, 0, "no engine row");
    assert_eq!(
        port.provisioned.lock().unwrap().len(),
        provisions_before,
        "nothing provisioned"
    );
}

/// F8: `PUT /engines/{id}` shares `CreateEngineBody` but has never attached
/// anything. Accepting `catalogs` and dropping it would tell the caller their
/// catalogs were attached.
#[tokio::test]
async fn update_engine_refuses_catalogs_and_changes_nothing() {
    let port = FakeEnginePort::default();
    let Some(f) = engines_fixture_with(port.clone()).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sp3c_update_cats", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_body(
        &f,
        &token,
        &name,
        serde_json::json!({ "warehouseId": wh_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let wh_cat = warehouse_catalog(&f, wh_id).await;

    let bench = unique_catalog_name("bench");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": bench, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let bench_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let before: serde_json::Value =
        sqlx::query_scalar("SELECT spec FROM engine_mgmt.engines WHERE id = $1")
            .bind(engine_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("engine spec");
    let updates_before = port.updated.lock().unwrap().len();

    let (status, j) = call(
        &f,
        "PUT",
        &format!("/engines/{engine_id}"),
        &token,
        Some(serde_json::json!({
            "name": name,
            "warehouseId": wh_id,
            "coordinatorReplicas": 3, "workerReplicas": 3,
            "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            "catalogs": [{ "catalogId": bench_id, "primary": false }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    let msg = j["error"].as_str().expect("error");
    assert!(
        msg.contains("not accepted on update"),
        "says the field is create-only; msg={msg}"
    );
    assert!(
        msg.contains("/catalogs/"),
        "points at the route that does attach; msg={msg}"
    );

    // Nothing changed: not the attachments, not the spec, not the cluster.
    let listed = registry(&f)
        .list_engine_attachments(engine_id)
        .await
        .expect("list");
    assert_eq!(
        listed.iter().map(|a| a.catalog.id).collect::<Vec<_>>(),
        vec![wh_cat.id],
        "the refused update attached nothing"
    );
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT spec FROM engine_mgmt.engines WHERE id = $1")
            .bind(engine_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("engine spec");
    assert_eq!(after, before, "the refused update wrote no spec");
    assert_eq!(
        port.updated.lock().unwrap().len(),
        updates_before,
        "the refused update never reached the provisioner"
    );
}

// ---------------------------------------------------------------------------
// SP3c-1 Task 3 — `POST /catalogs/{id}/test` (design D4)
// ---------------------------------------------------------------------------

/// A local port the OS just handed out and we handed straight back, so nothing
/// is listening on it — and, unlike a hard-coded number, never a port some
/// unrelated service on the machine happens to hold.
async fn a_closed_local_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// D4's whole point: egret must not become a port scanner with a login page.
/// The probe's target comes from the STORED catalog row and from nowhere else,
/// so the request is replayed carrying every field an attacker would reach for
/// — `host`, `port`, `uri`, `hosts`, `target` — all naming a socket that IS
/// listening, while the stored configuration names one that is not.
///
/// Three independent witnesses, because one alone could be satisfied by the
/// wrong mechanism: the response's `target` names the stored socket, the
/// verdict is `ok:false` (it would be `true` had the request's open port been
/// dialled), and the decoy listener never sees a connection at all.
#[tokio::test]
async fn the_test_route_takes_its_target_from_the_row_not_the_request() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_test_target", true).await;

    let decoy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let decoy_port = decoy.local_addr().unwrap().port();
    let closed_port = a_closed_local_port().await;

    // Stored: a port nothing listens on. Attached to no engine, so the route
    // takes the probe branch.
    let name = unique_catalog_name("es_row");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": name, "kind": "elasticsearch",
            "config": { "hosts": ["127.0.0.1"], "port": closed_port, "tls": false }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();

    let redirect = serde_json::json!({
        "host": "127.0.0.1",
        "port": decoy_port,
        "uri": format!("thrift://127.0.0.1:{decoy_port}"),
        "hosts": ["127.0.0.1"],
        "target": format!("127.0.0.1:{decoy_port}"),
    });
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{cid}/test"),
        &token,
        Some(redirect),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(
        j["check"], "tcp",
        "no engine is attached, so the probe ran; {j}"
    );
    assert_eq!(
        j["target"],
        serde_json::json!(format!("127.0.0.1:{closed_port}")),
        "the result names the STORED socket, not the one the request asked for; {j}"
    );
    assert_eq!(
        j["ok"], false,
        "the request's listening port must not have been dialled; {j}"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), decoy.accept())
            .await
            .is_err(),
        "the socket named ONLY by the request was never connected to"
    );

    // The mirror, so the assertion above cannot pass by the route simply
    // refusing to dial anything: a stored socket that IS listening reports
    // `ok:true`, while the request names the closed one.
    let open_name = unique_catalog_name("es_open");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": open_name, "kind": "elasticsearch",
            "config": { "hosts": ["127.0.0.1"], "port": decoy_port, "tls": false }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let open_cid = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{open_cid}/test"),
        &token,
        Some(serde_json::json!({ "host": "127.0.0.1", "port": closed_port })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["ok"], true, "the stored, listening socket answered; {j}");
    assert_eq!(
        j["target"],
        serde_json::json!(format!("127.0.0.1:{decoy_port}")),
        "{j}"
    );
    // The sentence itself must say what a green tick here does and does not
    // mean — D4 puts that in the message, not in a tooltip.
    let msg = j["message"].as_str().unwrap();
    assert!(
        msg.contains("reachability check only") && msg.contains("no credentials"),
        "a `tcp` green must disclaim credentials in the sentence; got {msg:?}"
    );
    assert!(j["engineName"].is_null(), "no engine ran this check; {j}");
}

/// Today's per-engine test route reaches only Trino
/// (`monitor::coordinator_call`, fallback service `{name}-trino`), so a
/// catalog attached ONLY to StarRocks falls back to the probe until SP4. That
/// is a narrower answer, not a wrong one — and `check` is what keeps it
/// honest, so the response must say `tcp` and name the socket it dialled.
#[tokio::test]
async fn a_starrocks_only_catalog_falls_back_to_the_probe_and_says_so() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_test_sr", true).await;
    let (engine_id, _) = create_starrocks_via_api(&f, &token).await;
    let closed_port = a_closed_local_port().await;
    let name = unique_catalog_name("es_sr");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": name, "kind": "elasticsearch",
            "config": { "hosts": ["127.0.0.1"], "port": closed_port, "tls": false }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{cid}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "attach to starrocks; {j}");

    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(
        j["check"], "tcp",
        "a StarRocks attachment cannot run the query, so the probe ran; {j}"
    );
    assert_eq!(
        j["target"],
        serde_json::json!(format!("127.0.0.1:{closed_port}")),
        "the probe names what it dialled; {j}"
    );
    assert_eq!(j["ok"], false, "nothing listens there; {j}");
    assert!(
        j["engineName"].is_null(),
        "no engine answered, so none is claimed; {j}"
    );
}

/// A `tpch` catalog generates its own data: it has no network endpoint at all.
/// With nothing attached, the honest answer is `unsupported` — the third
/// discriminant, which a later renderer shows neutral. It is NOT a failed
/// check, and the assertion that matters is `check`, not `ok`.
#[tokio::test]
async fn a_tpch_catalog_with_no_engine_is_unsupported_not_failed() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_test_tpch", true).await;
    let name = unique_catalog_name("tpch");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": name, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();

    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(
        j["check"], "unsupported",
        "nothing was dialled and nothing was queried, and the response says exactly that; {j}"
    );
    assert!(
        j["target"].is_null(),
        "`unsupported` must not invent a target; {j}"
    );
    assert!(j["engineName"].is_null(), "{j}");
    assert!(
        j["message"].as_str().unwrap().contains("tpch"),
        "the sentence names the kind it cannot probe; {j}"
    );
}

/// With a Trino engine attached the route asks the better question:
/// `SHOW SCHEMAS FROM <name>` on that engine, credentials included. The
/// catalog here is the engine's own egret-warehouse primary, whose
/// `probe_target` is `None` — so a route that skipped the engine would answer
/// `unsupported`, and the two branches cannot be confused.
#[tokio::test]
async fn an_attached_trino_engine_answers_with_the_query_and_names_itself() {
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_test_query", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = warehouse_catalog(&f, wh_id).await.id;

    fake.set_coordinator_response(200, br#"{"id":"q1","stats":{"state":"FINISHED"}}"#.to_vec());
    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["check"], "query", "{j}");
    assert_eq!(j["ok"], true, "{j}");
    assert_eq!(
        j["engineName"],
        serde_json::json!(engine_name),
        "the result names the engine that answered; {j}"
    );
    assert!(
        j["target"].is_null(),
        "a query check dialled no socket of its own; {j}"
    );
    // The strongest of the three checks must also SAY what it did — it was the
    // one branch whose whole message was the word "ok" (review F5).
    let msg = j["message"].as_str().unwrap();
    assert!(
        msg.contains("SHOW SCHEMAS") && msg.contains(&engine_name) && msg.contains("credentials"),
        "the query sentence names the check, the engine and what it proved; got {msg:?}"
    );

    // The engine's verdict is the route's verdict: a coordinator that fails
    // the query is `ok:false` on the SAME `query` discriminant, never a
    // silent fall-through to the weaker probe.
    fake.set_coordinator_response(
        200,
        br#"{"id":"q2","stats":{"state":"FAILED"},"error":{"message":"catalog not found"}}"#
            .to_vec(),
    );
    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["check"], "query", "{j}");
    assert_eq!(j["ok"], false, "{j}");
    assert_eq!(j["message"], "catalog not found", "{j}");
}

/// SP3c task-3 review F3. `list_catalog_attachments` is `ORDER BY e.name`, so
/// "the first Trino attachment" is the alphabetically first engine — which may
/// be one the catalog was never applied to. Querying that one answers
/// "catalog not found": a failure that says nothing about the catalog and
/// everything about the attachment. The route must prefer an APPLIED
/// attachment, and when none is applied it must say so rather than report a
/// meaningless red.
#[tokio::test]
async fn the_query_runs_on_an_applied_attachment_not_the_alphabetically_first() {
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_test_applied", true).await;

    // Two Trino engines whose names fix the ORDER BY: `aaa` sorts first.
    let first_name = format!("trino-aaa{}", Uuid::new_v4().simple());
    let last_name = format!("trino-zzz{}", Uuid::new_v4().simple());
    let mut ids = Vec::new();
    for name in [&first_name, &last_name] {
        let wh = seed_provisioned_warehouse(&f).await;
        let (status, j) = create_engine_via_api(&f, &token, name, wh, serde_json::json!({})).await;
        assert_eq!(status, StatusCode::CREATED, "{j}");
        ids.push(j["id"].as_str().unwrap().to_string());
    }

    let closed_port = a_closed_local_port().await;
    let name = unique_catalog_name("es_applied");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": name, "kind": "elasticsearch",
            "config": { "hosts": ["127.0.0.1"], "port": closed_port, "tls": false }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();
    for eid in &ids {
        let (status, j) = call(
            &f,
            "POST",
            &format!("/engines/{eid}/catalogs/{cid}"),
            &token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "attach to {eid}; {j}");
    }

    // The alphabetically first engine never applied it. This is the state
    // SP3c's own list page exists to warn about, not an exotic one.
    let set_status = |eid: String, st: &'static str| {
        let pool = f.db.pool.clone();
        let cid = cid.clone();
        async move {
            sqlx::query(
                "UPDATE registry.engine_catalog_attachments SET apply_status = $3 \
                   WHERE engine_id = $1 AND catalog_id = $2",
            )
            .bind(Uuid::parse_str(&eid).unwrap())
            .bind(Uuid::parse_str(&cid).unwrap())
            .bind(st)
            .execute(&pool)
            .await
            .expect("set apply_status");
        }
    };
    set_status(ids[0].clone(), "pending").await;
    set_status(ids[1].clone(), "applied").await;

    fake.set_coordinator_response(200, br#"{"id":"q1","stats":{"state":"FINISHED"}}"#.to_vec());
    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["check"], "query", "{j}");
    assert_eq!(
        j["engineName"],
        serde_json::json!(last_name),
        "the APPLIED engine answered, not the alphabetically first one; {j}"
    );

    // With nothing applied anywhere, a query would test the attachment rather
    // than the catalog — so the route drops to the weaker check and says why,
    // naming the engine and its status instead of reporting a red that means
    // nothing about the catalog.
    set_status(ids[1].clone(), "failed").await;
    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(
        j["check"], "tcp",
        "no applied attachment, so no query was run; {j}"
    );
    assert!(j["engineName"].is_null(), "no engine answered; {j}");
    let msg = j["message"].as_str().unwrap();
    assert!(
        msg.contains("has this catalog applied") && msg.contains(&first_name),
        "the sentence says why no query ran and names an engine; got {msg:?}"
    );
    assert!(
        !msg.contains("No Trino engine is attached"),
        "two ARE attached — the sentence must not deny it; got {msg:?}"
    );
}

/// SP3c task-3 review F2. `thrift://` used to pass validation, store, and then
/// make the route tell the operator that the KIND `hive` "has no network
/// endpoint" — false, and it points at the wrong remedy. Validation now refuses
/// it at the source; a row written before that rule is answered by naming the
/// URI, never by blaming the kind.
#[tokio::test]
async fn a_stored_uri_naming_no_host_is_never_reported_as_the_kind_having_none() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_test_nohost", true).await;
    let sc = seed_exchange_storage_conn(&f).await;
    let name = unique_catalog_name("hive_nohost");

    // Refused at the source: this can no longer be created at all.
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": name, "kind": "hive",
            "config": { "metastore": { "type": "hive", "uri": "thrift://" },
                        "storage_connection_id": sc }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap_or_default()
            .contains("must name a host"),
        "the refusal names the real problem; {j}"
    );

    // A row written before that rule: create a good one, then break it the only
    // way a pre-existing row could be broken — straight in the column.
    let cid = seed_catalog_then_break_its_uri(&f, &token, &name, sc).await;

    let (status, j) = call(&f, "POST", &format!("/catalogs/{cid}/test"), &token, None).await;
    // The status is the point, not only the words: a row egret cannot parse is
    // a 400, and that is the one status Task 7 has to render for this route.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    let body = j.to_string();
    assert!(
        !body.contains("has no network endpoint"),
        "the kind is fine — only the URI is broken; {j}"
    );
    assert!(
        body.contains("must name a host"),
        "the answer names the URI as the problem; {j}"
    );
}

/// Create a valid `hive` catalog, then break its stored `uri` in the column —
/// the only way a row can hold a shape validation now refuses, and exactly what
/// a database that predates the tightened rule looks like.
async fn seed_catalog_then_break_its_uri(f: &Fixture, token: &str, name: &str, sc: Uuid) -> String {
    let (status, j) = call(
        f,
        "POST",
        "/catalogs",
        token,
        Some(serde_json::json!({
            "name": name, "kind": "hive",
            "config": { "metastore": { "type": "hive", "uri": "thrift://hms:9083" },
                        "storage_connection_id": sc }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let cid = j["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE registry.catalogs SET config = jsonb_set(config, '{metastore,uri}', '\"thrift://\"') WHERE id = $1")
        .bind(Uuid::parse_str(&cid).unwrap())
        .execute(&f.db.pool)
        .await
        .expect("break the stored uri");
    cid
}

/// Attach a catalog to an engine by writing the row, bypassing
/// `POST /engines/{id}/catalogs/{cid}` — which parses the config and would
/// refuse a broken one. The state under test is a row that was attached WHILE
/// it still parsed and broke afterwards, and this is how it is reproduced
/// without a time machine (the same device
/// `one_unparseable_catalog_does_not_take_an_engines_whole_list_with_it` uses).
async fn attach_row_directly(f: &Fixture, engine_id: &str, catalog_id: &str) {
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, apply_status) \
         VALUES ($1, $2, 'applied')",
    )
    .bind(Uuid::parse_str(engine_id).unwrap())
    .bind(Uuid::parse_str(catalog_id).unwrap())
    .execute(&f.db.pool)
    .await
    .expect("attach the pre-existing row");
}

/// THE TRINO/STARROCKS RETRY ASYMMETRY, EXECUTED.
///
/// SP3c-1 Task 9 carried this as the programme's one traced-but-unexecuted
/// invariant: the engine Catalogs tab disables Retry and Detach on EVERY row of
/// a Trino engine while any OTHER row is broken, and does not on StarRocks, and
/// the sentence that explains why is the deliverable. Two people read the
/// source and agreed on the table; nobody ran it. This is the instrument.
///
/// The mechanism, at source:
///   * `apply_trino` maps `CatalogChange::ReapplyOne` to `BuildOpts::default()`
///     because a Trino apply is the whole ConfigMap either way, and
///     `build_provision_req_with` then renders EVERY attachment with a `?` per
///     row (`engines.rs:673-676`). One unrenderable sibling ends the build, so
///     the Retry on an innocent row never reaches Kubernetes.
///   * `apply_starrocks`'s `ReapplyOne` arm loads exactly the one catalog named
///     in the path and sends one statement about it. A broken sibling is not
///     read, so it cannot refuse anything.
///
/// A SECOND FACT THIS PINS: the refused Trino Retry leaves the HEALTHY row
/// exactly as it was. `retry_apply` carves `ApplyError::Build` out of
/// `set_apply_failed` alongside `Unrecorded`, because `apply_status` answers
/// "did this catalog reach that engine" and a build that never left egret is
/// not an answer to it — least of all when the build was ended by a SIBLING's
/// parse error, which is what happens here.
///
/// When this test was first written it asserted the opposite, recorded and
/// explicitly not endorsed: the healthy row went `failed` carrying the broken
/// catalog's message, and SP3c-1 renders that on five surfaces (the row badge,
/// the per-engine error Alert, the list's Status column, the "needing
/// attention" counter and the detail page's warning). `propagate` funnels a
/// Configuration SAVE through the same function, so the repair page
/// manufactured the false state too. Fixed in the whole-branch review's F2;
/// this half of the test is now what proves it stays fixed.
#[tokio::test]
async fn a_broken_sibling_blocks_retry_on_trino_and_not_on_starrocks() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "retry_scope_admin", true).await;
    let sc = seed_exchange_storage_conn(&f).await;

    // ── Trino ───────────────────────────────────────────────────────────────
    let broken_name = unique_catalog_name("hive_retry_scope");
    let broken = seed_catalog_then_break_its_uri(&f, &token, &broken_name, sc).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let trino_id = j["id"].as_str().unwrap().to_string();
    let healthy = warehouse_catalog(&f, wh).await.id;
    attach_row_directly(&f, &trino_id, &broken).await;

    // Read BEFORE the refused Retry, so "unchanged" below is a comparison and
    // not a guess about what engine create seeds.
    let before = registry(&f)
        .get_attachment(Uuid::parse_str(&trino_id).unwrap(), healthy)
        .await
        .unwrap()
        .expect("the healthy attachment exists before the Retry");
    assert_eq!(
        before.apply_status, "applied",
        "the primary catalog of a freshly created Trino engine really did reach it"
    );

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{trino_id}/catalogs/{healthy}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a Trino Retry re-renders the whole engine, so the broken sibling refuses it; {j}"
    );
    assert!(
        j.to_string().contains("must name a host"),
        "and the refusal is the SIBLING's parse error, which is why the tab's copy has to \
         name the scope rather than the row; {j}"
    );

    // A REFUSED APPLY WRITES NOTHING. The build ended inside egret, so nothing
    // new is known about this catalog's state on this engine and the row's
    // last true state stands. Before the whole-branch review's F2 fix this read
    // `failed`, carrying the BROKEN catalog's message — a verdict about a
    // different catalog, rendered on five SP3c-1 surfaces.
    let after = registry(&f)
        .get_attachment(Uuid::parse_str(&trino_id).unwrap(), healthy)
        .await
        .unwrap()
        .expect("the healthy attachment still exists");
    assert_eq!(
        after.apply_status, before.apply_status,
        "a Trino Retry refused by a SIBLING's parse error must leave the row it was called \
         for exactly as it was"
    );
    assert_eq!(
        after.apply_error, None,
        "and above all it must not carry the sibling's message: {:?}",
        after.apply_error
    );

    // ── StarRocks: the same shape, the opposite outcome ─────────────────────
    let (sr_id, sr_wh) = create_starrocks_via_api(&f, &token).await;
    let sr_healthy = warehouse_catalog(&f, sr_wh).await.id;
    attach_row_directly(&f, &sr_id, &broken).await;

    let (status, j) = call(
        &f,
        "POST",
        &format!("/engines/{sr_id}/catalogs/{sr_healthy}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a StarRocks Retry reads only the catalog in the path, so a broken sibling is \
         invisible to it; {j}"
    );
    let sr_row = registry(&f)
        .get_attachment(Uuid::parse_str(&sr_id).unwrap(), sr_healthy)
        .await
        .unwrap()
        .expect("the StarRocks primary attachment");
    assert_eq!(
        sr_row.apply_status, "applied",
        "and it really converged rather than merely not-failing"
    );
    // The broken sibling is untouched by either engine's Retry: nothing read
    // it, so nothing may have judged it.
    let sr_broken = registry(&f)
        .get_attachment(
            Uuid::parse_str(&sr_id).unwrap(),
            Uuid::parse_str(&broken).unwrap(),
        )
        .await
        .unwrap()
        .expect("the broken attachment");
    assert_eq!(
        sr_broken.apply_status, "applied",
        "the row written by `attach_row_directly`, unchanged"
    );
}

/// THE REPAIR PAGE MUST NOT MANUFACTURE THE STATE IT REPAIRS.
///
/// `propagate` (a `PUT /catalogs/{id}`, i.e. Save on the catalog's
/// Configuration tab) funnels every attached engine through `retry_apply`. On
/// Trino the apply is the whole ConfigMap, so ONE unparseable sibling on that
/// engine ends the build — and before the whole-branch review's F2 fix that
/// wrote `failed`, with the sibling's parse error, onto the attachment of the
/// catalog that had just been saved. A healthy catalog was flagged broken by
/// the act of editing it, and SP3c-1's list, detail page and Engines tab all
/// showed it.
///
/// The companion `registry_update_propagates_and_flags_the_failed_engine` is
/// the control for the other direction: an engine that genuinely REJECTS the
/// update still flags the row, which is what `apply_status` is for.
#[tokio::test]
async fn saving_a_healthy_catalog_is_not_a_verdict_about_it() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "propagate_sibling_admin", true).await;
    let sc = seed_exchange_storage_conn(&f).await;
    let reg = registry(&f);

    let broken = seed_catalog_then_break_its_uri(
        &f,
        &token,
        &unique_catalog_name("hive_propagate_scope"),
        sc,
    )
    .await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &engine_name, wh, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    // The catalog under edit: healthy, applied, and NOT the engine's primary,
    // so what happens to it cannot be confused with the warehouse mirror's own
    // lifecycle.
    let cat_name = unique_catalog_name("bench_propagate");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let healthy = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    reg.insert_attachment(&egret_catalog_registry::NewAttachment {
        engine_id,
        catalog_id: healthy,
        catalog_client_id: None,
        credential_secret_id: None,
        minted_client: false,
        applied_now: true,
        is_primary: false,
        created_at: None,
        apply_status: egret_catalog_registry::store::apply_status::APPLIED,
    })
    .await
    .unwrap();
    attach_row_directly(&f, &engine_id.to_string(), &broken).await;

    let (status, j) = call(
        &f,
        "PUT",
        &format!("/catalogs/{healthy}"),
        &token,
        Some(serde_json::json!({
            "config": {}, "advanced": { "trino": { "tpch.column-naming": "STANDARD" } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the save itself succeeds; {j}");
    let outcome = j["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["engineName"] == engine_name.as_str())
        .cloned()
        .expect("an outcome for the engine");
    assert_eq!(
        outcome["status"], "failed",
        "the engine did not get the new configuration, and the response says so; {j}"
    );
    assert!(
        outcome["error"]
            .as_str()
            .unwrap()
            .contains("must name a host"),
        "…naming the SIBLING's parse error, because that is what refused the build; {j}"
    );

    // AND THE ROW IS UNTOUCHED. The response above is the honest report; the
    // attachment's `apply_status` is a claim about the engine, and nothing
    // reached the engine to make one.
    let row = reg
        .get_attachment(engine_id, healthy)
        .await
        .unwrap()
        .expect("the saved catalog's attachment");
    assert_eq!(
        row.apply_status, "applied",
        "saving a healthy catalog must not flag it `failed` because a DIFFERENT catalog on \
         the same engine cannot be rendered"
    );
    assert_eq!(
        row.apply_error, None,
        "and it must not carry the other catalog's message: {:?}",
        row.apply_error
    );
}

/// SP4a Task 9 review: binding an engine to a policy profile changes which
/// catalogs that profile reaches, but mutates no grant and no rule — so the
/// `perm` compiler's affected-service sets, which are all derived from the
/// MUTATED ROW, never fire for it. Before this the newly bound profile served
/// a stale policy set until an operator happened to call
/// `POST /api/control-plane/perm/compile/{service_id}` by hand.
#[tokio::test]
async fn creating_an_engine_bound_to_a_profile_recompiles_that_profile() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_sp4a_bind_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    let profile_name = format!("trino-{}", Uuid::new_v4().simple());
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy_api::PolicyPort::ensure_trino_def(&policy)
        .await
        .unwrap();
    let profile = egret_policy_api::PolicyPort::create_service(
        &policy,
        &profile_name,
        "trino",
        &serde_json::json!({}),
    )
    .await
    .unwrap();

    assert!(
        compiler.calls.lock().unwrap().is_empty(),
        "nothing should have been recompiled before the engine exists"
    );

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id, "authzServiceId": profile.id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(authz_service_of(&f, &name).await, Some(profile.id));

    assert_eq!(
        *compiler.calls.lock().unwrap(),
        vec![profile.id],
        "the create must have asked for exactly the profile it bound to be \
         recompiled — otherwise that profile keeps serving a policy set that \
         predates every catalog this engine just attached"
    );
}

// ---------------------------------------------------------------------------
// `PUT /engines/{id}` honours `authz_service_id` (StarRocks/Ranger blockers,
// Task 3)
// ---------------------------------------------------------------------------
//
// `update_engine` takes `Json<CreateEngineBody>`, so `authz_service_id` was a
// DECLARED field of the update body — accepted, then silently dropped. The
// handler right beside it refuses `catalogs` for exactly that reason
// ("accepting the field and dropping it would tell the caller their catalogs
// were attached"), but `catalogs` has another route to be sent to and this had
// none: SP4a's live gate had to bind a StarRocks engine to a profile with a
// direct SQL `UPDATE`. So it is HONOURED rather than refused.

/// PUT body that changes nothing but the profile binding.
fn rebind_body(name: &str, wh_id: Uuid, authz_service_id: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "authzServiceId": authz_service_id,
    })
}

/// An update carrying a valid `authz_service_id` binds the engine to that
/// profile AND recompiles it, in one transaction.
#[tokio::test]
async fn update_with_authz_service_id_binds_and_recompiles() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_update", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    // A SECOND profile, not the one auto-bind created for this engine.
    let other = format!("other-{}", Uuid::new_v4().simple());
    let other_id = seed_ranger_profile(&f, &other, &[]).await;
    let before = authz_service_of(&f, &name).await;
    assert_ne!(
        before,
        Some(other_id),
        "the engine must not already be bound to the profile under test"
    );
    compiler.in_tx_calls.lock().unwrap().clear();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(rebind_body(&name, wh_id, serde_json::json!(other_id))),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update; body={j}");

    assert_eq!(
        authz_service_of(&f, &name).await,
        Some(other_id),
        "the engine row must record the profile the update named"
    );
    assert_eq!(
        compiler.in_tx_calls.lock().unwrap().clone(),
        vec![other_id],
        "the newly bound profile must be recompiled, in the binding's own \
         transaction — a bound-but-uncompiled profile serves a policy set that \
         predates the binding"
    );
}

/// **The discriminating test.** An `authz_service_id` naming no profile is
/// REFUSED, and the refusal NAMES the id. Silence is the defect: before this,
/// the field was accepted and dropped, so a caller who mistyped an id got a
/// `200` and an engine still bound wherever it was.
#[tokio::test]
async fn update_with_unknown_authz_service_id_is_refused_naming_it() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_unknown", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let before = authz_service_of(&f, &name).await;
    compiler.in_tx_calls.lock().unwrap().clear();

    // An id no `policy.authz_services` row carries. Taken from MAX(id) so it
    // cannot collide with a profile another concurrent test in this binary
    // created.
    let max: Option<i64> = sqlx::query_scalar("SELECT max(id) FROM policy.authz_services")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let unknown = max.unwrap_or(0) + 10_000;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(rebind_body(&name, wh_id, serde_json::json!(unknown))),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    let msg = j.to_string();
    assert!(
        msg.contains(&unknown.to_string()),
        "the refusal must name the id the caller sent; body={j}"
    );
    assert_eq!(
        authz_service_of(&f, &name).await,
        before,
        "a refused bind must not move the engine's binding"
    );
    assert!(
        compiler.in_tx_calls.lock().unwrap().is_empty(),
        "nothing is compiled for a profile that does not exist"
    );
}

/// Re-pointing an engine at a different profile leaves the PREVIOUS profile
/// row intact. Standing constraint: binding and unbinding never delete a
/// Ranger profile — a profile can hold admin-written policies and nothing
/// records that egret created it.
#[tokio::test]
async fn update_rebind_leaves_the_previous_profile_row_intact() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_rebind", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    let first = format!("first-{}", Uuid::new_v4().simple());
    let first_id = seed_ranger_profile(&f, &first, &[]).await;
    let second = format!("second-{}", Uuid::new_v4().simple());
    let second_id = seed_ranger_profile(&f, &second, &[]).await;

    for sid in [first_id, second_id] {
        let resp = f
            .app
            .clone()
            .oneshot(req(
                "PUT",
                &format!("/engines/{engine_id}"),
                &token,
                Some(rebind_body(&name, wh_id, serde_json::json!(sid))),
            ))
            .await
            .unwrap();
        let (status, j) = into_json(resp).await;
        assert_eq!(status, StatusCode::OK, "rebind to {sid}; body={j}");
        assert_eq!(authz_service_of(&f, &name).await, Some(sid));
    }

    assert!(
        service_exists(&f, &first).await,
        "re-pointing the engine must NOT delete the profile it left"
    );
    assert!(
        service_exists(&f, &format!("{first}_tag")).await,
        "nor its tag service"
    );
}

/// An update that omits `authz_service_id` leaves the binding ALONE. The wire
/// shape cannot tell an absent field from an explicit `null`, and the update
/// wizard sends the whole body — so treating `None` as a detach would silently
/// unbind every engine on every unrelated spec edit, a worse silence than the
/// one being fixed.
#[tokio::test]
async fn update_without_authz_service_id_leaves_the_binding_alone() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_absent", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let before = authz_service_of(&f, &name).await;
    assert!(before.is_some(), "auto-bind gave this engine a profile");
    compiler.in_tx_calls.lock().unwrap().clear();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 2,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(
        authz_service_of(&f, &name).await,
        before,
        "an omitted authz_service_id must never unbind"
    );
    assert!(compiler.in_tx_calls.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// The four guards the PUT bind was missing (whole-branch review I1-I4)
// ---------------------------------------------------------------------------

/// I1: a profile of the WRONG TYPE is refused with a 409, the same status the
/// create path already raises for it — and the binding does not move.
///
/// Not cosmetic: the plugin downloads the service-def of the profile's own
/// type, so a trino-typed profile on a starrocks engine evaluates trino-shaped
/// definitions against starrocks-shaped compiled policies. Rules whose resource
/// names no longer match are not denials, they are ABSENT — an unmasked read.
#[tokio::test]
async fn update_refuses_a_policy_profile_of_the_wrong_type() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_type", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let before = authz_service_of(&f, &name).await;
    compiler.in_tx_calls.lock().unwrap().clear();

    // A STARROCKS-typed profile, fully tag-wired, offered to a TRINO engine.
    let sr_name = format!("srprofile-{}", Uuid::new_v4().simple());
    let sr_profile = seed_typed_profile(&f, &sr_name, "starrocks").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(rebind_body(&name, wh_id, serde_json::json!(sr_profile))),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    let msg = j.to_string();
    assert!(
        msg.contains("starrocks"),
        "must name the profile's type; body={j}"
    );
    assert!(msg.contains("trino"), "and the engine's kind; body={j}");
    assert_eq!(
        authz_service_of(&f, &name).await,
        before,
        "a refused bind must not move the binding"
    );
    assert!(
        compiler.in_tx_calls.lock().unwrap().is_empty(),
        "nothing is compiled for a refused bind"
    );
}

/// I2: a profile another engine already binds is refused with a 409, mirroring
/// create's `count_engines_bound_to_authz_service` arm. Two engines sharing one
/// profile silently share one policy set.
#[tokio::test]
async fn update_refuses_a_policy_profile_another_engine_already_binds() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_shared", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    // Two engines. The second one's auto-bind profile is what the first will
    // be offered — a profile that is demonstrably bound elsewhere.
    let a = format!("trino-{}", Uuid::new_v4().simple());
    let (s, j) = create_engine_via_api(&f, &token, &a, wh_id, serde_json::json!({})).await;
    assert_eq!(s, StatusCode::CREATED, "create a; body={j}");
    let a_id = j["id"].as_str().unwrap().to_string();
    let b = format!("trino-{}", Uuid::new_v4().simple());
    let (s, j) = create_engine_via_api(&f, &token, &b, wh_id, serde_json::json!({})).await;
    assert_eq!(s, StatusCode::CREATED, "create b; body={j}");

    let b_profile = authz_service_of(&f, &b)
        .await
        .expect("engine b is auto-bound to a profile");
    let a_before = authz_service_of(&f, &a).await;
    assert_ne!(a_before, Some(b_profile));
    compiler.in_tx_calls.lock().unwrap().clear();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{a_id}"),
            &token,
            Some(rebind_body(&a, wh_id, serde_json::json!(b_profile))),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    assert!(
        j.to_string().contains(&b_profile.to_string()),
        "the refusal must name the profile; body={j}"
    );
    assert_eq!(
        authz_service_of(&f, &a).await,
        a_before,
        "engine a keeps its own profile"
    );
    assert_eq!(
        authz_service_of(&f, &b).await,
        Some(b_profile),
        "engine b keeps its profile — a refused bind steals nothing"
    );
    assert!(compiler.in_tx_calls.lock().unwrap().is_empty());
}

/// I3, fail-open: a profile with no usable `tagService` is REFUSED.
///
/// Binding one compiles zero tag policies, so every tag-based column mask and
/// row filter silently disappears and the columns are served raw —
/// `recompile_service_in_tx` records the skip and carries on by design. The
/// decision is to refuse: nothing is provisioned yet, the engine keeps its
/// working binding, and this is the only moment a human is present to fix it.
#[tokio::test]
async fn update_refuses_a_policy_profile_with_no_usable_tag_service() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_notag", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let before = authz_service_of(&f, &name).await;
    compiler.in_tx_calls.lock().unwrap().clear();

    let tagless_name = format!("tagless-{}", Uuid::new_v4().simple());
    let tagless = seed_profile_without_tag_service(&f, &tagless_name, "trino").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(rebind_body(&name, wh_id, serde_json::json!(tagless))),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    let msg = j.to_string();
    assert!(
        msg.contains("tag service"),
        "the refusal must say what is missing; body={j}"
    );
    assert!(
        msg.contains(&tagless.to_string()),
        "and name the profile; body={j}"
    );
    assert_eq!(
        authz_service_of(&f, &name).await,
        before,
        "a refused bind must not move the binding"
    );
    assert!(compiler.in_tx_calls.lock().unwrap().is_empty());

    // The SAME profile with a usable tag service is accepted — so the refusal
    // is about the tag service and nothing else.
    let wired_name = format!("wired-{}", Uuid::new_v4().simple());
    let wired = seed_typed_profile(&f, &wired_name, "trino").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(rebind_body(&name, wh_id, serde_json::json!(wired))),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a tag-wired profile binds; body={j}"
    );
    assert_eq!(authz_service_of(&f, &name).await, Some(wired));
}

/// I4: a PUT that ends in a 400 must NOT have bound. The engine-kind check, the
/// kubernetes check and the StarRocks body validators all used to run
/// downstream of the bind, so a refused PUT had already re-pointed the engine
/// and recompiled the profile it named.
///
/// Driven through the StarRocks body validators, because those are the ones
/// that produce a 400 from a body a caller can actually send.
#[tokio::test]
async fn a_refused_update_never_binds_the_profile_it_carried() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_order", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let sr_name = format!("sr-{}", Uuid::new_v4().simple());
    let sr_id = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &sr_name,
            "starrocks",
            "egret-engines",
            &serde_json::json!({ "starrocksConfig": {} }),
            None,
        )
        .await
        .unwrap()
        .id;
    let before = authz_service_of(&f, &sr_name).await;

    // A perfectly valid, tag-wired, starrocks-typed, unbound profile...
    let profile_name = format!("srp-{}", Uuid::new_v4().simple());
    let profile = seed_typed_profile(&f, &profile_name, "starrocks").await;
    assert_ne!(before, Some(profile));
    compiler.in_tx_calls.lock().unwrap().clear();

    // ...carried on a body the StarRocks validators refuse (`delegationMode`).
    let mut body = rebind_body(&sr_name, wh_id, serde_json::json!(profile));
    body["kind"] = serde_json::json!("starrocks");
    body["starrocks"] = serde_json::json!({ "delegationMode": "not-a-mode" });

    let resp = f
        .app
        .clone()
        .oneshot(req("PUT", &format!("/engines/{sr_id}"), &token, Some(body)))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert_eq!(
        authz_service_of(&f, &sr_name).await,
        before,
        "a PUT that ends in a 400 must not have bound the profile it carried"
    );
    assert!(
        compiler.in_tx_calls.lock().unwrap().is_empty(),
        "nor compiled it"
    );
}

/// I3's other half, and the one that keeps the guard from being over-strict: a
/// STARROCKS profile with no tag service is ACCEPTED.
///
/// StarRocks' Ranger plugin has no tag policies at all (upstream
/// starrocks#67458), so a StarRocks profile is not supposed to have a tag
/// service — the compiler expands a tag-selector rule into per-column resource
/// policies on the profile itself, and `MissingTagService` is raised only on
/// the Trino branch. Without this test the I3 guard would refuse every
/// legitimate StarRocks bind, including the one the live masking gate needs,
/// and every other test here would still pass.
#[tokio::test]
async fn update_accepts_a_starrocks_profile_with_no_tag_service() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_sr_notag", true).await;
    // `_with_secret`: provisioning StarRocks needs the warehouse storage
    // connection's secret for the FE's builtin storage volume.
    let wh_id = seed_provisioned_warehouse_with_secret(&f).await;
    let sr_name = format!("sr-{}", Uuid::new_v4().simple());
    // Created through the API so it gets a PRIMARY warehouse attachment — a
    // bare store row 400s later in `build_starrocks_provision_req`, which would
    // make this test fail for a reason that has nothing to do with tag services.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": sr_name, "warehouseId": wh_id, "kind": "starrocks",
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create starrocks; body={j}");
    let sr_id = j["id"].as_str().unwrap().to_string();
    compiler.in_tx_calls.lock().unwrap().clear();

    // A starrocks-typed profile with NO `tagService` and no tag service row.
    let p_name = format!("srp-{}", Uuid::new_v4().simple());
    let profile = seed_profile_without_tag_service(&f, &p_name, "starrocks").await;

    let mut body = rebind_body(&sr_name, wh_id, serde_json::json!(profile));
    body["kind"] = serde_json::json!("starrocks");
    body["starrocks"] = serde_json::json!({ "feReplicas": 1, "cnReplicas": 1 });

    let resp = f
        .app
        .clone()
        .oneshot(req("PUT", &format!("/engines/{sr_id}"), &token, Some(body)))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a StarRocks profile needs no tag service; body={j}"
    );
    assert_eq!(authz_service_of(&f, &sr_name).await, Some(profile));
    assert_eq!(
        compiler.in_tx_calls.lock().unwrap().clone(),
        vec![profile],
        "and it is still recompiled"
    );
}

/// I4's general half: a refusal decided from the BUILT spec — which no hoisted
/// guard can pre-empt — still leaves the binding where it was.
///
/// A StarRocks engine row with no primary warehouse attachment 400s inside
/// `build_starrocks_provision_req`, well after the bind has committed. The
/// compensating restore is what keeps "the caller was told no" and "the engine
/// was re-pointed anyway" from both being true.
#[tokio::test]
async fn a_late_refusal_restores_the_previous_binding() {
    let compiler = RecordingProfileCompiler::default();
    let Some(f) = setup_with_ranger_and_compiler(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
        Some(Arc::new(compiler.clone())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_bind_late", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let sr_name = format!("sr-{}", Uuid::new_v4().simple());
    let sr_id = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &sr_name,
            "starrocks",
            "egret-engines",
            &serde_json::json!({ "starrocksConfig": {} }),
            None,
        )
        .await
        .unwrap()
        .id;

    // Give it a starting binding, so the restore has something to restore TO
    // (and so this cannot pass merely because both values are NULL).
    let start_name = format!("srstart-{}", Uuid::new_v4().simple());
    let start = seed_profile_without_tag_service(&f, &start_name, "starrocks").await;
    egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .set_engine_authz_service(sr_id, Some(start))
        .await
        .unwrap();

    let target_name = format!("srtarget-{}", Uuid::new_v4().simple());
    let target = seed_profile_without_tag_service(&f, &target_name, "starrocks").await;
    compiler.in_tx_calls.lock().unwrap().clear();

    // This engine has no primary warehouse attachment, so the StarRocks
    // provision-req build 400s — after the bind.
    let mut body = rebind_body(&sr_name, wh_id, serde_json::json!(target));
    body["kind"] = serde_json::json!("starrocks");
    body["starrocks"] = serde_json::json!({ "feReplicas": 1, "cnReplicas": 1 });

    let resp = f
        .app
        .clone()
        .oneshot(req("PUT", &format!("/engines/{sr_id}"), &token, Some(body)))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert!(!status.is_success(), "the late guard must refuse; body={j}");
    assert_eq!(
        authz_service_of(&f, &sr_name).await,
        Some(start),
        "a refused update must leave the engine on the profile it started on"
    );
    // Both profile rows survive — a restore unbinds, never deletes.
    assert!(service_exists(&f, &start_name).await);
    assert!(service_exists(&f, &target_name).await);
}

// ---------------------------------------------------------------------------
// External engines (SP4d-4 Part C, Task 7)
// ---------------------------------------------------------------------------
//
// An external engine is a row egret holds for a cluster it never provisions.
// Every test here wires the REAL profile compiler, because what is being
// proved is the compiled effect (`policy.authz_policies`, which is what the
// plugin's download serves) and the refusal of a created overlap, neither of
// which a recording fake can show.

/// A fixture whose engine seam compiles for real, and the fake provisioner it
/// was built with (so a test can assert nothing was provisioned).
async fn setup_external() -> Option<(Fixture, FakeEnginePort)> {
    let port = FakeEnginePort::default();
    let mut f = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await?;
    f.perm_compiler = Some(Arc::new(RealProfileCompiler {
        perm: egret_perm::PermStore::new(f.db.pool.clone()),
        policy: egret_policy::PolicyStore::new(f.db.clone()),
    }));
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        f.ranger_enabled,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
    );
    Some((f, port))
}

async fn starrocks_profile(f: &Fixture) -> i64 {
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    egret_policy_api::PolicyPort::create_service(
        &policy,
        &format!("extsr_{}", Uuid::new_v4().simple()),
        "starrocks",
        &serde_json::json!({}),
    )
    .await
    .expect("starrocks profile")
    .id
}

/// A registry catalog both engine types can read.
async fn jdbc_catalog(f: &Fixture, token: &str) -> Uuid {
    let pw = seed_secret(f, "hunter2").await;
    let (status, j) = call(
        f,
        "POST",
        "/catalogs",
        token,
        Some(jdbc_body(&unique_catalog_name("extpg"), pw)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    Uuid::parse_str(j["id"].as_str().unwrap()).unwrap()
}

async fn allow_read_grant(f: &Fixture, catalog_id: Uuid) -> Uuid {
    use egret_perm::{
        Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
    };
    egret_perm::PermStore::new(f.db.pool.clone())
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Allow,
            actions: vec![Action::Read],
            scope: Scope {
                catalog_id,
                schema: None,
                table: None,
                column: None,
            },
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::User,
                    r#ref: "analyst".into(),
                }],
                exclude: vec![],
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("grant")
}

fn register_body(
    name: &str,
    kind: &str,
    version: &str,
    authz: Option<i64>,
    catalogs: &[Uuid],
) -> serde_json::Value {
    serde_json::json!({
        "name": name, "type": kind, "host": "sr.example.com",
        "port": if kind == "trino" { 8080 } else { 9030 },
        "rangerPluginVersion": version, "authzServiceId": authz, "catalogs": catalogs,
    })
}

fn external_name(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..10])
}

fn assert_nothing_provisioned(port: &FakeEnginePort) {
    assert!(port.provisioned.lock().unwrap().is_empty(), "provisioned");
    assert!(port.updated.lock().unwrap().is_empty(), "updated");
    assert!(port.deleted.lock().unwrap().is_empty(), "deleted");
    assert!(port.restarted.lock().unwrap().is_empty(), "restarted");
}

#[tokio::test]
async fn registering_an_external_starrocks_binds_it_and_declares_its_catalogs() {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_register_admin", true).await;
    let profile = starrocks_profile(&f).await;
    let catalog = jdbc_catalog(&f, &token).await;
    let grant = allow_read_grant(&f, catalog).await;
    let name = external_name("ext-sr");

    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &name,
            "starrocks",
            "2.8.0",
            Some(profile),
            &[catalog],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["origin"], "external", "{j}");
    assert_eq!(j["host"], "sr.example.com");
    assert_eq!(j["port"], 9030);
    assert_eq!(j["rangerPluginVersion"], "2.8.0");
    assert_eq!(j["rangerPluginVersionModelled"], true);

    let (origin, bound, namespace): (String, Option<i64>, String) = sqlx::query_as(
        "SELECT origin, authz_service_id, namespace FROM engine_mgmt.engines WHERE name = $1",
    )
    .bind(&name)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (origin.as_str(), bound, namespace.as_str()),
        ("external", Some(profile), "")
    );

    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let attachment: (String, bool, bool, Option<Uuid>) = sqlx::query_as(
        "SELECT apply_status, minted_client, is_primary, catalog_client_id \
           FROM registry.engine_catalog_attachments WHERE engine_id = $1 AND catalog_id = $2",
    )
    .bind(engine_id)
    .bind(catalog)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(attachment, ("declared".to_string(), false, false, None));

    // The compiled effect, which is what the plugin's download serves: the
    // bound profile carries a policy for the grant on the declared catalog.
    let compiled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(profile)
    .bind(format!("{}{}", egret_policy::COMPILER_GRANT_PREFIX, grant))
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        compiled, 1,
        "the declared catalog's grant must be compiled at once"
    );

    assert_nothing_provisioned(&port);
}

#[tokio::test]
async fn registering_against_a_profile_of_the_other_type_is_409() {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_type_admin", true).await;
    let trino_profile = seed_ranger_profile(&f, &external_name("exttr"), &[]).await;
    let name = external_name("ext-sr");

    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &name,
            "starrocks",
            "2.8.0",
            Some(trino_profile),
            &[],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(engine_rows_named(&f, &name).await, 0, "nothing is written");
    assert_nothing_provisioned(&port);
}

#[tokio::test]
async fn registering_against_a_profile_another_engine_binds_is_409() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_bound_admin", true).await;
    let profile = starrocks_profile(&f).await;
    let first = external_name("ext-sr");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &first,
            "starrocks",
            "2.8.0",
            Some(profile),
            &[],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");

    let second = external_name("ext-sr");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &second,
            "starrocks",
            "2.8.0",
            Some(profile),
            &[],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(
        engine_rows_named(&f, &second).await,
        0,
        "nothing is written"
    );
}

#[tokio::test]
async fn an_unknown_catalog_is_404_naming_it_and_writes_nothing() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_404_admin", true).await;
    let profile = starrocks_profile(&f).await;
    let missing = Uuid::new_v4();
    let name = external_name("ext-sr");

    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &name,
            "starrocks",
            "2.8.0",
            Some(profile),
            &[missing],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{j}");
    assert!(
        j["error"].as_str().unwrap().contains(&missing.to_string()),
        "the 404 must name the catalog id: {j}"
    );
    assert_eq!(engine_rows_named(&f, &name).await, 0, "nothing is written");
    let bound: i64 =
        sqlx::query_scalar("SELECT count(*) FROM engine_mgmt.engines WHERE authz_service_id = $1")
            .bind(profile)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(bound, 0, "the profile stays unbound");
}

#[tokio::test]
async fn get_engine_never_asks_the_provisioner_for_an_external_engine() {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_get_admin", true).await;
    let name = external_name("ext-tr");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(&name, "trino", "2.5.0", None, &[])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let id = j["id"].as_str().unwrap().to_string();

    let (status, got) = call(&f, "GET", &format!("/engines/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["origin"], "external", "{got}");
    assert_eq!(
        port.status_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "an external engine has no cluster of egret's to ask"
    );

    // The list asks the provisioner about every PROVISIONED engine in the
    // database (other tests' included), so a call count says nothing there; what
    // matters is that the external row is listed as itself.
    let (status, list) = call(&f, "GET", "/engines", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id.as_str())
        .expect("listed");
    assert_eq!(row["origin"], "external");
    assert_eq!(row["rangerPluginVersion"], "2.5.0");
}

#[tokio::test]
async fn registration_validates_its_input_and_states_an_unmodelled_version() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_validate_admin", true).await;
    let ok = |name: &str| register_body(name, "trino", "2.5.0", None, &[]);
    let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
    let mut with = |why, field: &str, v: serde_json::Value| {
        let mut b = ok(&external_name("ext-bad"));
        b[field] = v;
        cases.push((why, b));
    };
    with("risingwave type", "type", "risingwave".into());
    with("scheme in host", "host", "http://trino.example.com".into());
    with("path in host", "host", "trino.example.com/x".into());
    with("port zero", "port", 0.into());
    with(
        "unknown plugin version",
        "rangerPluginVersion",
        "2.55.0".into(),
    );
    with(
        "malformed plugin version",
        "rangerPluginVersion",
        "2.5".into(),
    );
    with("upper-case name", "name", "Bad_Name".into());
    for (why, body) in cases {
        let name = body["name"].as_str().unwrap().to_string();
        let (status, j) = call(&f, "POST", "/engines/external", &token, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {j}");
        assert_eq!(
            engine_rows_named(&f, &name).await,
            0,
            "{why}: nothing is written"
        );
    }

    // A real Ranger release the evaluator does not model is accepted as
    // declared, and the response says it is not modelled.
    let name = external_name("ext-260");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(&name, "trino", "2.6.0", None, &[])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["rangerPluginVersion"], "2.6.0");
    assert_eq!(j["rangerPluginVersionModelled"], false, "{j}");

    // "Modelled" is a (type, version) PAIR (user ruling 2026-10-12): 2.8.0 is
    // modelled for StarRocks and 2.5.0 for Trino, and neither the other way.
    for (kind, version, modelled) in [
        ("trino", "2.5.0", true),
        ("trino", "2.8.0", false),
        ("starrocks", "2.5.0", false),
        ("starrocks", "2.8.0", true),
    ] {
        let n = external_name("ext-pair");
        let (status, j) = call(
            &f,
            "POST",
            "/engines/external",
            &token,
            Some(register_body(&n, kind, version, None, &[])),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{kind}@{version}: {j}");
        assert_eq!(
            j["rangerPluginVersionModelled"], modelled,
            "{kind}@{version}: {j}"
        );
    }

    // A taken name is a 409.
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(&name, "trino", "2.5.0", None, &[])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
}

/// Last of the external tests by name only; reports the unscoped count and
/// asserts the scoped one. The database is shared, so the unscoped count is
/// printed, not asserted.
#[tokio::test]
async fn the_whole_table_agrees_declared_means_external() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_agree_admin", true).await;
    let catalog = jdbc_catalog(&f, &token).await;
    let name = external_name("ext-agree");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(&name, "trino", "2.5.0", None, &[catalog])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let scoped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments a \
           JOIN engine_mgmt.engines e ON e.id = a.engine_id \
          WHERE (a.apply_status = 'declared') <> (e.origin = 'external') AND e.id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(scoped, 0, "this test's engine: declared <=> external");
    let unscoped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments a \
           JOIN engine_mgmt.engines e ON e.id = a.engine_id \
          WHERE (a.apply_status = 'declared') <> (e.origin = 'external')",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    eprintln!("declared<=>external disagreements across the whole table: {unscoped}");
}

// -- a bind and an attachment refuse the mask / row-filter pairs they create --
//
// SP4d-4 Task 3 re-review M-2, closed here. A profile bind and an engine <->
// catalog attachment change which catalogs, hence which tag assignments, a
// profile's unit resolves, and every overlap check reads those as stored. The
// fixture is the one `egret-bi`'s `catalog_tag_overlap` uses: two HAND-WRITTEN
// tag masks (so no compile claims or rewrites them) under the profile's tag
// service, and two tags assigned to one column of a warehouse. Until an engine
// that attaches the warehouse's catalog is bound to the profile, the profile's
// unit resolves no assignment and the pair does not exist.

struct OverlapUnit {
    /// The tag `both_tags = false` leaves unassigned.
    second_tag: Uuid,
    profile: i64,
    warehouse_id: Uuid,
    catalog_id: Uuid,
}

const OVERLAP_COLUMN: &str = "email";

async fn insert_tag(f: &Fixture, label: &str) -> (Uuid, String) {
    let key = format!("{label}-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
        .bind(&key)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag");
    (id, key)
}

async fn assign_tag(f: &Fixture, tag: Uuid, warehouse_id: Uuid) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
                                              column_name) \
         VALUES ($1, $2, 'sales', 'orders', $3)",
    )
    .bind(tag)
    .bind(warehouse_id)
    .bind(OVERLAP_COLUMN)
    .execute(&f.db.pool)
    .await
    .expect("assign tag");
}

/// A Trino profile whose tag service holds two hand-written tag masks, tagged
/// onto one column of `warehouse_id` (or of a fresh warehouse with its registry
/// catalog). `both_tags` false assigns only the first tag, so no pair exists
/// whatever binds the profile.
async fn overlap_unit(f: &Fixture, warehouse: Option<Uuid>, both_tags: bool) -> OverlapUnit {
    overlap_unit_named(f, warehouse, both_tags, &external_name("ovl")).await
}

/// [`overlap_unit`] with the profile's name chosen (a late-bind adopts a profile
/// named after the engine). A `warehouse` with no registry catalog yet gets one,
/// the row an engine create would ensure for it.
async fn overlap_unit_named(
    f: &Fixture,
    warehouse: Option<Uuid>,
    both_tags: bool,
    profile_name: &str,
) -> OverlapUnit {
    let pool = &f.db.pool;
    let profile_name = profile_name.to_string();
    let profile = seed_ranger_profile(f, &profile_name, &[]).await;
    let tag_service = format!("{profile_name}_tag");
    let tag_service_id: i64 =
        sqlx::query_scalar("SELECT id FROM policy.authz_services WHERE name = $1")
            .bind(&tag_service)
            .fetch_one(pool)
            .await
            .expect("tag service");
    let (tag_a, key_a) = insert_tag(f, "ovla").await;
    let (tag_b, key_b) = insert_tag(f, "ovlb").await;
    let store = egret_policy::PolicyStore::new(f.db.clone());
    for (label, key, mask) in [("a", &key_a, "MASK"), ("b", &key_b, "MASK_NULL")] {
        let name = format!("hand {label} {}", Uuid::new_v4().simple());
        let body = serde_json::json!({
            "name": name, "service": tag_service, "serviceType": "tag",
            "policyType": 1, "isEnabled": true, "policyPriority": 0,
            "resources": {"tag": {"values": [key], "isExcludes": false, "isRecursive": false}},
            "dataMaskPolicyItems": [{
                "users": [], "groups": ["public"], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": mask}
            }],
            "policyItems": [], "denyPolicyItems": []
        });
        store
            .create_policy(tag_service_id, &name, &body)
            .await
            .expect("hand-written tag mask");
    }
    let (warehouse_id, catalog_id) = match warehouse {
        Some(wh) => {
            let existing: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM registry.catalogs WHERE warehouse_id = $1")
                    .bind(wh)
                    .fetch_optional(pool)
                    .await
                    .expect("the warehouse's registry catalog");
            let catalog = match existing {
                Some(c) => c,
                None => sqlx::query_scalar(
                    "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
                     VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                             jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
                     RETURNING id",
                )
                .bind(unique_catalog_name("ovlcat"))
                .bind(wh)
                .fetch_one(pool)
                .await
                .expect("registry catalog"),
            };
            (wh, catalog)
        }
        None => {
            let wh = seed_provisioned_warehouse(f).await;
            let catalog: Uuid = sqlx::query_scalar(
                "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
                 VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                         jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
                 RETURNING id",
            )
            .bind(unique_catalog_name("ovlcat"))
            .bind(wh)
            .fetch_one(pool)
            .await
            .expect("registry catalog");
            (wh, catalog)
        }
    };
    assign_tag(f, tag_a, warehouse_id).await;
    if both_tags {
        assign_tag(f, tag_b, warehouse_id).await;
    }
    OverlapUnit {
        second_tag: tag_b,
        profile,
        warehouse_id,
        catalog_id,
    }
}

fn assert_rule_overlap(status: StatusCode, body: &serde_json::Value) {
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    assert_eq!(body["kind"], "mask", "{body}");
    assert!(
        body["coordinate"]
            .as_str()
            .unwrap()
            .ends_with(".sales.orders.email"),
        "{body}"
    );
}

/// The bind: a `PUT` that re-points an engine at the profile completes the pair
/// (the engine attaches the warehouse whose column carries both tags).
#[tokio::test]
async fn a_put_bind_that_creates_an_overlap_is_409_and_binds_nothing() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_bind_ovl_admin", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let before = authz_service_of(&f, &name).await;
    let unit = overlap_unit(&f, Some(wh), true).await;

    let (status, body) = call(
        &f,
        "PUT",
        &format!("/engines/{engine_id}"),
        &token,
        Some(rebind_body(&name, wh, serde_json::json!(unit.profile))),
    )
    .await;
    assert_rule_overlap(status, &body);
    assert_eq!(authz_service_of(&f, &name).await, before, "nothing bound");
}

/// Accept cell of the same bind: one tag on the column, no pair, the bind lands.
#[tokio::test]
async fn a_put_bind_that_creates_no_overlap_still_binds() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_bind_ok_admin", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let unit = overlap_unit(&f, Some(wh), false).await;

    let (status, body) = call(
        &f,
        "PUT",
        &format!("/engines/{engine_id}"),
        &token,
        Some(rebind_body(&name, wh, serde_json::json!(unit.profile))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(authz_service_of(&f, &name).await, Some(unit.profile));
}

/// Registration binds AND declares in one transaction: the same pair is refused
/// and the row is not left behind.
#[tokio::test]
async fn registering_an_external_engine_that_creates_an_overlap_is_409_and_writes_nothing() {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_reg_ovl_admin", true).await;
    let unit = overlap_unit(&f, None, true).await;
    let name = external_name("ext-ovl");

    let (status, body) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &name,
            "trino",
            "2.5.0",
            Some(unit.profile),
            &[unit.catalog_id],
        )),
    )
    .await;
    assert_rule_overlap(status, &body);
    assert_eq!(
        engine_rows_named(&f, &name).await,
        0,
        "the row is rolled back"
    );
    assert_nothing_provisioned(&port);

    // Accept cell: the same registration over a unit with one tag succeeds.
    let ok_unit = overlap_unit(&f, None, false).await;
    let ok_name = external_name("ext-ovl-ok");
    let (status, body) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &ok_name,
            "trino",
            "2.5.0",
            Some(ok_unit.profile),
            &[ok_unit.catalog_id],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Declaring a catalog on an external engine that already binds the profile.
#[tokio::test]
async fn declaring_a_catalog_that_creates_an_overlap_is_409_and_attaches_nothing() {
    let Some((f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_decl_ovl_admin", true).await;
    let unit = overlap_unit(&f, None, true).await;
    let name = external_name("ext-ovl");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &name,
            "trino",
            "2.5.0",
            Some(unit.profile),
            &[],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "no catalog, no pair: {j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    let (status, body) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{}", unit.catalog_id),
        &token,
        None,
    )
    .await;
    assert_rule_overlap(status, &body);
    let attached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE catalog_id = $1",
    )
    .bind(unit.catalog_id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(attached, 0, "the attachment is rolled back");

    // Accept cell.
    let ok_unit = overlap_unit(&f, None, false).await;
    let ok_name = external_name("ext-ovl-ok");
    let (status, j) = call(
        &f,
        "POST",
        "/engines/external",
        &token,
        Some(register_body(
            &ok_name,
            "trino",
            "2.5.0",
            Some(ok_unit.profile),
            &[],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let ok_id = j["id"].as_str().unwrap().to_string();
    let (status, body) = call(
        &f,
        "POST",
        &format!("/engines/{ok_id}/catalogs/{}", ok_unit.catalog_id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["applyStatus"], "declared", "{body}");
}

/// A PROVISIONED engine's attach cannot share a transaction with its recompile
/// (the cluster is applied first), so the pair is looked for on a dry run
/// before anything is applied.
#[tokio::test]
async fn attaching_to_a_provisioned_engine_a_catalog_that_creates_an_overlap_is_409() {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_attach_ovl_admin", true).await;
    let unit = overlap_unit(&f, None, true).await;
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(external_name("prov-ovl"))
    .bind(unit.profile)
    .fetch_one(&f.db.pool)
    .await
    .expect("provisioned engine bound to the profile");

    let (status, body) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{}", unit.catalog_id),
        &token,
        None,
    )
    .await;
    assert_rule_overlap(status, &body);
    let attached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(attached, 0, "nothing attached");
    assert!(
        port.updated.lock().unwrap().is_empty(),
        "nothing applied to the cluster"
    );
    let _ = unit.warehouse_id;
}

// -- fix round 1 (C1, I2): the create binds, the late-bind and the attach's
// post-apply comparison --------------------------------------------------------

fn real_compiler(f: &Fixture) -> Arc<dyn egret_engine_mgmt::ProfileCompilerPort> {
    Arc::new(RealProfileCompiler {
        perm: egret_perm::PermStore::new(f.db.pool.clone()),
        policy: egret_policy::PolicyStore::new(f.db.clone()),
    })
}

fn trino_create_body(name: &str, wh: Uuid, profile: i64) -> serde_json::Value {
    serde_json::json!({
        "name": name, "warehouseId": wh, "authzServiceId": profile,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
    })
}

fn starrocks_create_body(name: &str, wh: Uuid, profile: i64) -> serde_json::Value {
    serde_json::json!({
        "name": name, "warehouseId": wh, "kind": "starrocks", "authzServiceId": profile,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
    })
}

/// C1, Trino: `POST /engines` adopting an existing profile binds the new
/// engine's primary catalog into the profile's unit. The pair that completes is
/// refused, the create is undone (no row, nothing provisioned), and the same
/// create over a unit with one tag succeeds.
#[tokio::test]
async fn a_trino_create_adopting_a_profile_that_creates_an_overlap_is_409_and_creates_nothing() {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "create_trino_ovl_admin", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let unit = overlap_unit(&f, Some(wh), true).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    let (status, body) = call(
        &f,
        "POST",
        "/engines",
        &token,
        Some(trino_create_body(&name, wh, unit.profile)),
    )
    .await;
    assert_rule_overlap(status, &body);
    assert_eq!(
        engine_rows_named(&f, &name).await,
        0,
        "the create is undone"
    );
    assert!(
        port.provisioned.lock().unwrap().is_empty(),
        "nothing provisioned"
    );

    // Accept cell.
    let wh2 = seed_provisioned_warehouse(&f).await;
    let ok = overlap_unit(&f, Some(wh2), false).await;
    let ok_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, body) = call(
        &f,
        "POST",
        "/engines",
        &token,
        Some(trino_create_body(&ok_name, wh2, ok.profile)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(authz_service_of(&f, &ok_name).await, Some(ok.profile));
}

/// C1, StarRocks: `POST /engines` with `authzServiceId`.
#[tokio::test]
async fn a_starrocks_create_with_authz_service_id_that_creates_an_overlap_is_409_and_creates_nothing(
) {
    let Some((f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "create_sr_ovl_admin", true).await;
    let wh = seed_provisioned_warehouse_with_secret(&f).await;
    let unit = overlap_unit(&f, Some(wh), true).await;
    let name = format!("sr-{}", Uuid::new_v4().simple());

    let (status, body) = call(
        &f,
        "POST",
        "/engines",
        &token,
        Some(starrocks_create_body(&name, wh, unit.profile)),
    )
    .await;
    assert_rule_overlap(status, &body);
    assert_eq!(
        engine_rows_named(&f, &name).await,
        0,
        "the create is undone"
    );
    assert!(
        port.provisioned.lock().unwrap().is_empty(),
        "nothing provisioned"
    );

    // Accept cell.
    let wh2 = seed_provisioned_warehouse_with_secret(&f).await;
    let ok = overlap_unit(&f, Some(wh2), false).await;
    let ok_name = format!("sr-{}", Uuid::new_v4().simple());
    let (status, body) = call(
        &f,
        "POST",
        "/engines",
        &token,
        Some(starrocks_create_body(&ok_name, wh2, ok.profile)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(authz_service_of(&f, &ok_name).await, Some(ok.profile));
}

/// C1, the late-bind inside an apply: Ranger turned on after an unbound create
/// adopts the profile named after the engine. A bind that would create a pair is
/// SKIPPED (the apply still succeeds, the engine stays unbound); one that does
/// not is made.
#[tokio::test]
async fn a_late_bind_that_would_create_an_overlap_is_skipped_and_one_that_would_not_is_made() {
    let port = FakeEnginePort::default();
    let Some(mut f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        false,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    f.perm_compiler = Some(real_compiler(&f));
    let rebuild = |f: &Fixture, ranger: bool| {
        build_app(
            &f.db,
            &f.sessions,
            f.engine_provisioner.clone(),
            f.kube.clone(),
            std::sync::Arc::new(AllowAllPdp),
            ranger,
            f.starrocks_ddl.clone(),
            f.perm_compiler.clone(),
        )
    };
    f.app = rebuild(&f, false);
    let token = session_for(&f, "late_bind_ovl_admin", true).await;

    let mut ids = Vec::new();
    for both_tags in [true, false] {
        let wh = seed_provisioned_warehouse(&f).await;
        let name = format!("trino-{}", Uuid::new_v4().simple());
        let (status, j) = create_engine_via_api(&f, &token, &name, wh, serde_json::json!({})).await;
        assert_eq!(status, StatusCode::CREATED, "create; body={j}");
        assert_eq!(authz_service_of(&f, &name).await, None);
        // The orphan profile the late-bind will adopt, named after the engine.
        let unit = overlap_unit_named(&f, Some(wh), both_tags, &name).await;
        ids.push((j["id"].as_str().unwrap().to_string(), name, unit.profile));
    }

    f.app = rebuild(&f, true);
    f.ranger_enabled = true;
    for (id, ..) in &ids {
        let resp = f
            .app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/engines/{id}/rotate-secrets"),
                &token,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NO_CONTENT,
            "the apply itself succeeds"
        );
    }
    let (_, ovl_name, _) = &ids[0];
    assert_eq!(
        authz_service_of(&f, ovl_name).await,
        None,
        "a bind that creates a pair is skipped"
    );
    let (_, ok_name, ok_profile) = &ids[1];
    assert_eq!(
        authz_service_of(&f, ok_name).await,
        Some(*ok_profile),
        "a bind that creates none is made"
    );
}

/// A test double over the real compiler that commits a conflicting write at the
/// moment the real recompile of an attach runs, i.e. AFTER the dry run and
/// BEFORE the post-apply comparison. `conflict` is run once, on the first
/// `recompile_profile` (the fire-and-forget one `attach_to_engine` makes).
struct ConflictAfterDryRun {
    /// Fail every `overlap_snapshot_in_tx` (a dry run that cannot complete).
    fail_snapshot: bool,
    inner: RealProfileCompiler,
    conflict:
        tokio::sync::Mutex<Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>>,
}

#[async_trait::async_trait]
impl egret_engine_mgmt::ProfileCompilerPort for ConflictAfterDryRun {
    async fn recompile_profile(&self, service_id: i64) -> Result<(), egret_perm::CompileError> {
        if let Some(fut) = self.conflict.lock().await.take() {
            fut.await;
        }
        self.inner.recompile_profile(service_id).await
    }
    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: Uuid,
    ) -> Result<(), egret_perm::CompileError> {
        self.inner
            .recompile_catalog_assignment_in_tx(tx, catalog_id)
            .await
    }
    async fn recompile_profile_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_id: i64,
    ) -> Result<(), egret_perm::CompileError> {
        self.inner.recompile_profile_in_tx(tx, service_id).await
    }
    async fn overlap_snapshot_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<egret_engine_mgmt::OverlapSnapshot, egret_perm::CompileError> {
        if self.fail_snapshot {
            return Err(egret_perm::CompileError::Db("snapshot unavailable".into()));
        }
        self.inner.overlap_snapshot_in_tx(tx, service_ids).await
    }
    async fn refuse_created_overlaps_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        snapshot: &egret_engine_mgmt::OverlapSnapshot,
    ) -> Result<(), egret_perm::CompileError> {
        self.inner.refuse_created_overlaps_in_tx(tx, snapshot).await
    }
}

/// I2: a pair created by a write that commits between the attach's dry run and
/// its real recompile is caught by the comparison against the PRE-APPLY
/// snapshot, and the attachment is undone. The dry run alone passes here (the
/// second tag is not assigned yet when it runs).
#[tokio::test]
async fn a_pair_committed_between_the_dry_run_and_the_real_recompile_refuses_the_attach() {
    let Some((mut f, _port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "attach_race_admin", true).await;
    // One tag assigned: the dry run sees no pair. The race assigns the second.
    let unit = overlap_unit(&f, None, false).await;
    let second_tag = unit.second_tag;
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(external_name("prov-race"))
    .bind(unit.profile)
    .fetch_one(&f.db.pool)
    .await
    .expect("provisioned engine bound to the profile");

    let (pool, wh) = (f.db.pool.clone(), unit.warehouse_id);
    let race = Box::pin(async move {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, \
                                                  table_name, column_name) \
             VALUES ($1, $2, 'sales', 'orders', $3)",
        )
        .bind(second_tag)
        .bind(wh)
        .bind(OVERLAP_COLUMN)
        .execute(&pool)
        .await
        .expect("the conflicting write");
    });
    f.perm_compiler = Some(Arc::new(ConflictAfterDryRun {
        fail_snapshot: false,
        inner: RealProfileCompiler {
            perm: egret_perm::PermStore::new(f.db.pool.clone()),
            policy: egret_policy::PolicyStore::new(f.db.clone()),
        },
        conflict: tokio::sync::Mutex::new(Some(race)),
    }));
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        f.ranger_enabled,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
    );

    let (status, body) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{}", unit.catalog_id),
        &token,
        None,
    )
    .await;
    assert_rule_overlap(status, &body);
    let attached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(attached, 0, "the refused attachment is undone");
}

/// I2: a dry run that cannot complete for a reason that is NOT an overlap used
/// to be logged and the attach went ahead unchecked (fail-open). It refuses.
#[tokio::test]
async fn a_dry_run_that_cannot_complete_refuses_the_attach_instead_of_attaching_unchecked() {
    let Some((mut f, port)) = setup_external().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "attach_failopen_admin", true).await;
    let unit = overlap_unit(&f, None, false).await;
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(external_name("prov-failopen"))
    .bind(unit.profile)
    .fetch_one(&f.db.pool)
    .await
    .expect("provisioned engine bound to the profile");
    f.perm_compiler = Some(Arc::new(ConflictAfterDryRun {
        fail_snapshot: true,
        inner: RealProfileCompiler {
            perm: egret_perm::PermStore::new(f.db.pool.clone()),
            policy: egret_policy::PolicyStore::new(f.db.clone()),
        },
        conflict: tokio::sync::Mutex::new(None),
    }));
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        f.ranger_enabled,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
    );

    let (status, body) = call(
        &f,
        "POST",
        &format!("/engines/{engine_id}/catalogs/{}", unit.catalog_id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let attached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(attached, 0, "nothing attached unchecked");
    assert!(port.updated.lock().unwrap().is_empty(), "nothing applied");
}

// ---------------------------------------------------------------------------
// External engines: every lifecycle operation refuses or no-ops (Task 8)
// ---------------------------------------------------------------------------
//
// egret must never provision, scale, restart, rotate secrets on, exec into,
// query or delete the cluster of an external engine. Each test here registers a
// real external row and then asserts, besides the answer, that the fake
// provisioner and the fake Kubernetes client recorded NOTHING.

/// `setup_external`, with the provisioner and the Kubernetes client under the
/// test's control (`None` for either leaves it unconfigured).
async fn external_fixture(
    port: Option<FakeEnginePort>,
    kube: Option<Arc<egret_provisioner::kube_client::FakeKube>>,
) -> Option<Fixture> {
    let mut f = setup_with_ranger(
        port.map(|p| Arc::new(p) as Arc<dyn EngineProvisioningPort>),
        kube.map(|k| k as Arc<dyn KubeClient>),
        true,
    )
    .await?;
    f.perm_compiler = Some(Arc::new(RealProfileCompiler {
        perm: egret_perm::PermStore::new(f.db.pool.clone()),
        policy: egret_policy::PolicyStore::new(f.db.clone()),
    }));
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        f.ranger_enabled,
        f.starrocks_ddl.clone(),
        f.perm_compiler.clone(),
    );
    Some(f)
}

async fn register_external(
    f: &Fixture,
    token: &str,
    kind: &str,
    version: &str,
    authz: Option<i64>,
    catalogs: &[Uuid],
) -> (Uuid, String) {
    let name = external_name("ext-life");
    let (status, j) = call(
        f,
        "POST",
        "/engines/external",
        token,
        Some(register_body(&name, kind, version, authz, catalogs)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    (Uuid::parse_str(j["id"].as_str().unwrap()).unwrap(), name)
}

/// Neither the provisioner nor Kubernetes was touched, in any way.
fn assert_cluster_untouched(
    port: &FakeEnginePort,
    kube: &egret_provisioner::kube_client::FakeKube,
) {
    assert_nothing_provisioned(port);
    assert_eq!(
        port.status_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "status was asked"
    );
    assert!(kube.applied.lock().unwrap().is_empty(), "kube applied");
    assert!(kube.deleted.lock().unwrap().is_empty(), "kube deleted");
    assert!(kube.restarted.lock().unwrap().is_empty(), "kube restarted");
    assert!(kube.patched.lock().unwrap().is_empty(), "kube patched");
    assert!(
        kube.coordinator_calls.lock().unwrap().is_empty(),
        "the coordinator was called"
    );
}

/// The route answers 409 `engine_is_external`, naming the engine.
async fn assert_refused_as_external(
    f: &Fixture,
    token: &str,
    engine_name: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) {
    let (status, j) = call(f, method, uri, token, body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{method} {uri}: {j}");
    assert_eq!(j["code"], "engine_is_external", "{method} {uri}: {j}");
    assert!(
        j["error"].as_str().unwrap_or("").contains(engine_name),
        "{method} {uri}: the sentence names the engine: {j}"
    );
}

#[tokio::test]
async fn put_on_an_external_engine_is_409_and_never_updates_the_cluster() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_put_admin", true).await;
    let (id, name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;
    let body = serde_json::json!({
        "name": name, "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
    });
    assert_refused_as_external(
        &f,
        &token,
        &name,
        "PUT",
        &format!("/engines/{id}"),
        Some(body),
    )
    .await;
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn restart_and_rotate_on_an_external_engine_are_409() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_restart_admin", true).await;
    let (id, name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;
    for path in ["restart", "rotate-secrets"] {
        assert_refused_as_external(
            &f,
            &token,
            &name,
            "POST",
            &format!("/engines/{id}/{path}"),
            None,
        )
        .await;
    }
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn pod_logs_follow_and_exec_on_an_external_engine_are_409() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_logs_admin", true).await;
    let (id, name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;
    // The websocket routes answer before the upgrade is even looked at: a plain
    // GET is enough to reach the refusal.
    for path in ["logs", "logs/follow", "exec"] {
        assert_refused_as_external(
            &f,
            &token,
            &name,
            "GET",
            &format!("/engines/{id}/pods/coordinator-0/{path}"),
            None,
        )
        .await;
    }
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn monitor_routes_on_an_external_engine_are_409() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_monitor_admin", true).await;
    let (id, name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;
    for (method, path) in [
        ("GET", "queries"),
        ("GET", "queries/q1"),
        ("DELETE", "queries/q1"),
        ("GET", "cluster"),
    ] {
        assert_refused_as_external(
            &f,
            &token,
            &name,
            method,
            &format!("/engines/{id}/{path}"),
            None,
        )
        .await;
    }
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn catalog_test_and_apply_on_an_external_engine_are_409_and_the_registry_test_skips_it() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_cattest_admin", true).await;
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": unique_catalog_name("extbench"), "kind": "tpch", "config": {}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let catalog = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (id, name) = register_external(&f, &token, "trino", "2.5.0", None, &[catalog]).await;

    assert_refused_as_external(
        &f,
        &token,
        &name,
        "POST",
        &format!("/engines/{id}/catalogs/{catalog}/test"),
        None,
    )
    .await;
    assert_refused_as_external(
        &f,
        &token,
        &name,
        "POST",
        &format!("/engines/{id}/catalogs/{catalog}/apply"),
        None,
    )
    .await;

    // The registry's own test prefers an applied Trino attachment; a declared
    // one is not that, so no query runs and the coordinator is never asked.
    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{catalog}/test"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_ne!(j["check"], "query", "{j}");
    assert!(
        !j["message"].as_str().unwrap_or("").contains(&name),
        "a declared attachment is not offered as the engine that failed: {j}"
    );
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn workspace_assignment_of_an_external_engine_is_409() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_assign_admin", true).await;
    let (id, name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;
    let ws = seed_workspace(&f).await;
    assert_refused_as_external(
        &f,
        &token,
        &name,
        "PUT",
        &format!("/engines/{id}/workspaces"),
        Some(serde_json::json!({ "workspaceIds": [ws] })),
    )
    .await;
    assert_refused_as_external(
        &f,
        &token,
        &name,
        "POST",
        &format!("/workspaces/{ws}/engines"),
        Some(serde_json::json!({ "engineId": id })),
    )
    .await;
    assert_eq!(
        materialized_count(&f, ws, id).await,
        0,
        "no connection was materialized"
    );
    // Reading what is assigned stays fine: it is egret's own rows, and empty.
    let (status, j) = call(
        &f,
        "GET",
        &format!("/engines/{id}/workspaces"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert!(j.as_array().unwrap().is_empty(), "{j}");
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn deleting_an_external_engine_removes_egrets_rows_only() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_delete_admin", true).await;
    let profile = starrocks_profile(&f).await;
    let catalog = jdbc_catalog(&f, &token).await;
    let (id, _name) =
        register_external(&f, &token, "starrocks", "2.8.0", Some(profile), &[catalog]).await;

    let (status, j) = call(&f, "DELETE", &format!("/engines/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");

    let engines: i64 = sqlx::query_scalar("SELECT count(*) FROM engine_mgmt.engines WHERE id = $1")
        .bind(id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(engines, 0, "the engine row is gone");
    let attachments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(attachments, 0, "its declared attachments are gone");
    let catalogs: i64 = sqlx::query_scalar("SELECT count(*) FROM registry.catalogs WHERE id = $1")
        .bind(catalog)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(
        catalogs, 1,
        "the registry catalog is not egret's to drop here"
    );
    let profiles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM policy.authz_services WHERE id = $1")
            .bind(profile)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(profiles, 1, "the profile is unbound, not deleted");
    assert_cluster_untouched(&port, &kube);
}

/// A delete needs no Kubernetes for an external engine (there is no cluster), but
/// still needs it for a provisioned one: deleting only that row would orphan it.
#[tokio::test]
async fn deleting_an_external_engine_needs_no_kubernetes_and_a_provisioned_one_still_does() {
    let Some(f) = external_fixture(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_delete_nokube_admin", true).await;
    let _wh = seed_provisioned_warehouse(&f).await;
    let provisioned = seed_jwt_engine(&f).await;
    let (external, _name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;

    let (status, j) = call(&f, "DELETE", &format!("/engines/{external}"), &token, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");
    assert_eq!(engine_status(&f, external).await, None, "the row is gone");

    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/engines/{provisioned}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("kubernetes not configured"),
        "{j}"
    );
    assert!(engine_status(&f, provisioned).await.is_some(), "kept");
}

#[tokio::test]
async fn deleting_an_external_engine_with_dependents_is_409_and_keeps_it() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_delete_dep_admin", true).await;
    let (id, _name) = register_external(&f, &token, "trino", "2.5.0", None, &[]).await;
    // An external engine cannot be assigned through the API, so a connection
    // that names it is put there by hand: the preflight reads the table.
    let ws = seed_workspace(&f).await;
    let db: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases \
           (name, engine, host, port, options, workspace_id, delegated, engine_id) \
         VALUES ($1, 'trino', 'trino.example.com', 8080, '{}'::jsonb, $2, true, $3) RETURNING id",
    )
    .bind(external_name("ext-conn"))
    .bind(ws)
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    seed_dependent(&f, ws, db, "dataset", "orders-by-day").await;

    let (status, j) = call(&f, "DELETE", &format!("/engines/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(j["code"], "engine_has_dependents", "{j}");
    assert!(engine_status(&f, id).await.is_some(), "the row survives");
    assert_cluster_untouched(&port, &kube);
}

#[tokio::test]
async fn detaching_a_declared_catalog_removes_the_row_recompiles_and_touches_no_cluster() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_detach_admin", true).await;
    let profile = starrocks_profile(&f).await;
    let catalog = jdbc_catalog(&f, &token).await;
    let grant = allow_read_grant(&f, catalog).await;
    let (id, _name) =
        register_external(&f, &token, "starrocks", "2.8.0", Some(profile), &[catalog]).await;
    let compiled = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
        )
        .bind(profile)
        .bind(format!("{}{}", egret_policy::COMPILER_GRANT_PREFIX, grant))
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
    };
    assert_eq!(compiled().await, 1, "compiled while declared");

    let (status, j) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{catalog}/engines/{id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{j}");
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(left, 0, "the declared row is gone");
    assert_eq!(compiled().await, 0, "the profile was recompiled without it");
    assert_cluster_untouched(&port, &kube);

    // Detaching again: the catalog is no longer attached.
    let (status, _) = call(
        &f,
        "DELETE",
        &format!("/catalogs/{catalog}/engines/{id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// `POST /catalogs/{id}/reapply` converges the provisioned engine and SKIPS the
/// external one, and says so — a test that skipped everything would fail the
/// first half.
#[tokio::test]
async fn reapplying_a_catalog_converges_the_provisioned_engine_and_skips_the_external_one() {
    let port = FakeEnginePort::default();
    let kube = Arc::new(egret_provisioner::kube_client::FakeKube::default());
    let Some(f) = external_fixture(Some(port.clone()), Some(kube.clone())).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ext_reapply_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let provisioned_name = format!("trino-re-{}", Uuid::new_v4().simple());
    let (status, j) =
        create_engine_via_api(&f, &token, &provisioned_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let provisioned = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({
            "name": unique_catalog_name("rebench"), "kind": "tpch", "config": {}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let catalog = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    registry(&f)
        .insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: provisioned,
            catalog_id: catalog,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: true,
            is_primary: false,
            created_at: None,
            apply_status: egret_catalog_registry::store::apply_status::APPLIED,
        })
        .await
        .unwrap();
    let (_external, external_name_) =
        register_external(&f, &token, "trino", "2.5.0", None, &[catalog]).await;
    let updates_before = port.updated.lock().unwrap().len();

    let (status, j) = call(
        &f,
        "POST",
        &format!("/catalogs/{catalog}/reapply"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    let outcome = |n: &str| {
        j.as_array()
            .unwrap()
            .iter()
            .find(|o| o["engineName"] == n)
            .cloned()
            .unwrap_or_else(|| panic!("{n} is listed: {j}"))
    };
    assert_eq!(outcome(&provisioned_name)["status"], "applied", "{j}");
    let skipped = outcome(&external_name_);
    assert_eq!(skipped["status"], "skipped", "{j}");
    assert!(
        skipped["error"].as_str().unwrap_or("").contains("external"),
        "the list says why: {j}"
    );
    let updated = port.updated.lock().unwrap().clone();
    assert_eq!(
        updated.len(),
        updates_before + 1,
        "exactly the provisioned engine was applied"
    );
    assert_eq!(updated.last().unwrap().name, provisioned_name);
    assert!(
        updated.iter().all(|u| u.name != external_name_),
        "the external engine was never applied"
    );
}
