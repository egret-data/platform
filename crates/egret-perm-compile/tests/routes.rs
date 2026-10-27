//! The `perm` HTTP surface (SP4a Task 9), driven through the real router.
//!
//! **What these tests are actually for.** A CRUD suite that only asserted
//! "POST returns 201 and the row comes back from GET" would pass while
//! shipping the exact defect this task exists to prevent: a mutation surface
//! that writes `perm.grants` and never recompiles, leaving the engine
//! enforcing the previous Ranger policy set while egret's UI lists the grant
//! as live. So the load-bearing tests here
//! (`creating_a_grant_over_http_compiles_its_policy`,
//! `deleting_a_grant_over_http_reaps_its_policy`,
//! `creating_a_tag_rule_over_http_compiles_its_policy`) look at
//! `policy.authz_policies` — the compiled output the engine downloads — and
//! not at `perm.grants` at all.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test routes
//!
//! One database is shared by every concurrently-running `#[tokio::test]` in
//! this binary, so every assertion below is scoped to the fixture's OWN
//! `service_id` and to the exact policy NAME the compiler derives from the id
//! the request minted. A whole-summary count would be racy for a second
//! reason on top of that: a tag-selected rule is catalog-unscoped on Trino by
//! design, so ONE tag-rule mutation recompiles every compiler-targeted
//! profile in the database, including other tests' — `compile.services`
//! therefore has no bounded length and nothing here asserts one.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::model::Selector;
use egret_perm::PermStore;
// `confine_to_overlay_workspace` and `confine_rule_selector` are `pub` solely so
// these tests can call them: no HTTP entry point reaches either since
// `391c3c3a6` — see their ⚠ paragraphs, and
// `confine_to_overlay_workspace_allows_its_own_workspace_and_refuses_another`.
use egret_perm_compile::routes::{confine_rule_selector, confine_to_overlay_workspace};

#[path = "support/lakekeeper_stub.rs"]
mod lakekeeper_stub;
#[path = "support/pausing.rs"]
mod pausing;
use egret_perm_compile::{perm_router, PermHttpError, PermState};
use egret_policy::{PolicyStore, COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// Advisory-lock key shared by the two tests that would otherwise race
/// through the installation-wide tag-rule recompile — see [`exclusive`].
const TAG_FANOUT_LOCK: i64 = 0x5004_A009;

/// **Not test hygiene — a consequence of the behaviour under test.** Creating
/// or deleting a tag-selected rule recompiles EVERY compiler-targeted profile
/// in the database, other tests' fixtures included, and such a recompile will
/// happily re-create a policy another test has just deleted on purpose. The
/// tests that care (`creating_a_tag_rule_over_http_compiles_its_policy`, which
/// causes the fan-out; `compile_restores_a_profile_that_drifted`, which is the
/// one damaged by it; and `preview_and_write_agree_about_a_tag_rules_fan_out`)
/// take this lock so none has to weaken its assertion to a "either the endpoint
/// repaired it or somebody else did" shape.
///
/// **It also guards the UNIVERSE, not just the fan-out** (SP4b). The
/// preview-equals-write test compares two installation-wide affected sets taken
/// a moment apart, so a compiler-targeted profile appearing between them would
/// make them differ for a reason that is not a defect. Every helper here that
/// mints such a profile — `setup`, `extra_profile`, `extra_trino_profile` —
/// therefore holds this lock while it inserts the `engine_mgmt.engines` row
/// that makes the profile compiler-targeted, and releases it before returning.
/// Weakening that comparison to "both contain my own profile" would have left
/// the "cannot disagree" property untested, which is the one thing the
/// rolled-back-write design needs a guard for.
///
/// **And the universe is more than the set of profiles.** That test also
/// compares the two compile SUMMARIES, and a recompile reconciles every profile
/// in full: any profile whose desired state is committed but not yet compiled
/// gets its missing policy created by whoever recompiles it next. A test that
/// commits desired state in one step and compiles it in another (as
/// `egret_perm::sync_assignment_grants` followed by `recompile_service` does)
/// therefore opens a window in which a preview creates that policy, rolls it
/// back, and the write a moment later finds it already there: same profiles,
/// `created` one apart. That was a real flake (one failure in a handful of full
/// runs, never when the target ran alone). So such a pair of steps takes this
/// lock too, for exactly as long as the two are apart. A test that commits
/// desired state which compiles to NOTHING (an assignment to a workspace with no
/// members) opens no window and needs no lock.
///
/// A dedicated, NON-pooled connection: a session advisory lock is released
/// when the session ends, and a pooled connection returned to the pool keeps
/// its session — and its lock — alive.
///
/// **`pg_try_advisory_lock` in a bounded loop, never the blocking
/// `pg_advisory_lock`** (SP4b-1 review, I-4). The helpers that mint a profile
/// take this lock on a fresh session while three tests hold it for a whole body,
/// so a future test that calls `setup`/`extra_profile`/`extra_trino_profile`
/// while already holding it via `exclusive()` would wait on itself. With the
/// blocking form that is a hang: the run is consumed and reports nothing, which
/// is far worse than a failure, because a failure names its cause. Waiting is
/// still correct in the normal case — those three tests legitimately hold the
/// lock for seconds — so this waits, with a deadline, and then says what to look
/// for.
async fn advisory_lock(dsn: &str) -> sqlx::PgConnection {
    use sqlx::Connection as _;
    let mut conn = sqlx::PgConnection::connect(dsn).await.expect("lock conn");
    for _ in 0..1_200 {
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(TAG_FANOUT_LOCK)
            .fetch_one(&mut conn)
            .await
            .expect("try advisory lock");
        if got {
            return conn;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "waited 60s for TAG_FANOUT_LOCK. The usual cause is a test calling `setup`, \
         `extra_profile` or `extra_trino_profile` while already holding this lock \
         through `exclusive()` — those helpers take it themselves, so that test is \
         waiting on itself and would hang forever under the blocking \
         `pg_advisory_lock`. Do the fixture setup BEFORE taking `exclusive()`."
    );
}

/// Hold [`TAG_FANOUT_LOCK`] for the rest of the caller's test.
async fn exclusive(f: &Fixture) -> sqlx::PgConnection {
    advisory_lock(&f.dsn).await
}

struct Fixture {
    db: Db,
    dsn: String,
    perm: PermStore,
    policy: PolicyStore,
    users: UserRepo,
    sessions: SessionService,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    /// A StarRocks profile with one engine bound to it and one registry
    /// catalog attached to that engine.
    service_id: i64,
    catalog_id: Uuid,
    /// A `tags.key` nothing else in the database uses, already on
    /// `sales.orders.email` in this fixture's warehouse.
    ///
    /// Its `tags.name` is a DIFFERENT string on purpose — the wire reports the
    /// name (what the engine matches tag policies on) and the rule selectors take
    /// the key, so a fixture where they coincide cannot tell a key/name confusion
    /// from a correct answer. The name is **not** carried here: the one test that
    /// needs it reads it back out of `tags` by this key
    /// (`the_read_surface_names_the_catalog_and_the_tag_for_a_policy_read_only_caller`),
    /// which is what lets it assert the two differ rather than assume it.
    tag_key: String,
}

impl Fixture {
    fn app(&self) -> axum::Router {
        perm_router().with_state(PermState {
            perm: self.perm.clone(),
            policy: self.policy.clone(),
            sessions: self.sessions.clone(),
            pdp: self.pdp.clone(),
        })
    }

    /// The names of every policy THIS COMPILER owns under the fixture's own
    /// service — the compiled output an engine would download. Scoped to the
    /// fixture's `service_id`, so other tests' profiles cannot perturb it.
    async fn compiled_policy_names(&self) -> Vec<String> {
        self.compiled_policy_names_under(self.service_id).await
    }

    /// The same read for ANY profile — the patch tests need it for the profile
    /// a grant has just been moved AWAY from as well as the one it moved to,
    /// which is the whole point of the union recompile.
    async fn compiled_policy_names_under(&self, service_id: i64) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM policy.authz_policies \
             WHERE service_id = $1 AND (name LIKE $2 || '%' OR name LIKE $3 || '%') \
             ORDER BY name",
        )
        .bind(service_id)
        .bind(COMPILER_GRANT_PREFIX)
        .bind(COMPILER_RULE_PREFIX)
        .fetch_all(&self.db.pool)
        .await
        .expect("read compiled policies")
    }

    /// One compiled policy's body, under this fixture's own profile.
    async fn compiled_policy_body(&self, name: &str) -> Value {
        sqlx::query_scalar::<_, Value>(
            "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
        )
        .bind(self.service_id)
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .unwrap_or_else(|e| panic!("read compiled policy {name:?}: {e}"))
    }

    /// How many grants name this fixture's catalog — a scoped population, used
    /// to prove a read-only route wrote nothing without counting the whole
    /// (shared, racy) table.
    async fn grant_count(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
            .bind(self.catalog_id)
            .fetch_one(&self.db.pool)
            .await
            .expect("count grants")
    }

    async fn policy_version(&self) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT policy_version FROM policy.authz_services WHERE id = $1",
        )
        .bind(self.service_id)
        .fetch_one(&self.db.pool)
        .await
        .expect("read policy_version")
    }
}

/// A StarRocks profile with a bound engine, an attached registry catalog, and
/// one tagged column. StarRocks rather than Trino on purpose: a StarRocks tag
/// rule compiles to real, per-column policies under THIS service (D5), so a
/// tag-rule test can assert a concrete policy name under the fixture's own id
/// instead of a tag policy addressed to some other profile's tag service.
async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    // Held until this function returns: everything below adds a
    // compiler-targeted profile to the installation-wide universe a tag rule
    // fans out over — see `TAG_FANOUT_LOCK`.
    let _universe = advisory_lock(&dsn).await;

    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    // The embedded catalog's tables the StarRocks column source reads (D2);
    // in production they always exist.
    lakekeeper_stub::install(&db).await;
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_routes"), "starrocks", &json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("perm-routes-ws"))
            .bind("Perm Routes WS")
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-perm-routes-test','AKIATEST') RETURNING id",
    )
    .bind(unique("perm-routes-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("perm_routes_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("perm-routes-eng"))
    .bind(service.id)
    .fetch_one(&pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("perm_routes_cat"))
    .bind(warehouse_id)
    .fetch_one(&pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect("attach catalog");

    let tag_key = unique("pii");
    // A DIFFERENT string from the key, on purpose — see `Fixture::tag_key`.
    let tag_name = unique("PII");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(&tag_key)
            .bind(&tag_name)
            .fetch_one(&pool)
            .await
            .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .execute(&pool)
    .await
    .expect("tag assignment");

    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    ensure_fixture_principals(&pool).await;

    Some(Fixture {
        dsn,
        users: UserRepo::new(db.clone()),
        sessions,
        pdp,
        perm,
        policy,
        db,
        service_id: service.id,
        catalog_id,
        tag_key,
    })
}

/// The bare usernames the tests here author grants for (`grant_body(..,
/// "analyst")` and friends), and the role names they exempt.
///
/// **Since J-F3 every ref must name someone egret knows**, or the write is
/// refused (`egret_perm_compile::principal_refs`): a ref naming nobody fails OPEN
/// as a deny or an exclude. So these exist as real `users.username` / `roles.name`
/// rows rather than as strings nothing resolves. Idempotent and shared by every
/// test in the database (`ON CONFLICT DO NOTHING` — on the username as much as
/// the email, so a person some other test already minted as `analyst` serves just
/// as well: what matters is that the name resolves).
const FIXTURE_USERNAMES: &[&str] = &["alice", "analyst", "auditor", "contractor", "unused"];
const FIXTURE_ROLES: &[&str] = &["auditor"];

async fn ensure_fixture_principals(pool: &sqlx::PgPool) {
    for name in FIXTURE_USERNAMES {
        sqlx::query(
            "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(format!("{name}@perm-routes-fixture.egret.local"))
        .bind(name)
        .execute(pool)
        .await
        .expect("fixture user");
    }
    for name in FIXTURE_ROLES {
        sqlx::query(
            "INSERT INTO roles (scope, name, description) \
             VALUES ('workspace', $1, 'perm routes test fixture') ON CONFLICT DO NOTHING",
        )
        .bind(name)
        .execute(pool)
        .await
        .expect("fixture role");
    }
}

/// A user whose `users.username` is exactly `username` — for the tests that
/// mint a unique subject name and then grant to it (J-F3: the grant is refused
/// unless the name resolves). Returns the name, so a call site reads as the
/// `unique(..)` it replaces.
async fn known_user(f: &Fixture, username: String) -> String {
    sqlx::query("INSERT INTO users (provider, email, username) VALUES ('local', $1, $2)")
        .bind(format!("{username}@perm-routes-fixture.egret.local"))
        .bind(&username)
        .execute(&f.db.pool)
        .await
        .expect("known user");
    username
}

async fn into_json(resp: Response) -> (StatusCode, Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn req(method: &str, uri: &str, token: &str, body: Option<Value>) -> HttpRequest<Body> {
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

async fn new_user(f: &Fixture, tag: &str) -> Uuid {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    f.users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user")
        .0
}

/// A platform_admin session token. `platform_admin` bypasses inside the real
/// `CedarPdp`, which is what every non-authz test here wants.
async fn admin_session(f: &Fixture) -> String {
    let uid = new_user(f, "perm_admin").await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&f.db.pool)
    .await
    .expect("grant platform_admin");
    f.sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Give `uid` exactly one fine-grained permission key, at PLATFORM scope —
/// the only scope that can satisfy it, since `egret_core::rbac` declares the
/// `policy:*` family `Scope::Platform` and the PDP loads `Resource::Platform`
/// with no workspace, so `egret_auth::grants::grants_for` admits only
/// `scope_type = 'platform'` rows. The PDP snapshots the role catalog at build
/// time, so it is rebuilt here.
async fn grant_platform_permission(f: &mut Fixture, uid: Uuid, permission: &str) {
    let (role_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') \
         RETURNING id",
    )
    .bind(unique("perm_routes_role"))
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
    .bind(uid)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("assign role");
    f.pdp = Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("rebuild CedarPdp"),
    );
}

/// A NON-admin session holding exactly one fine-grained permission key.
async fn session_with_permission(f: &mut Fixture, permission: &str) -> String {
    let uid = new_user(f, "perm_scoped").await;
    grant_platform_permission(f, uid, permission).await;
    f.sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// A `ws_admin`'s own session token carrying a **workspace-scoped impersonation
/// overlay row** over a target who holds `permissions` at platform scope — the
/// exact fixture of the privilege escalation `391c3c3a6` (2026-10-10) closed.
///
/// Built the way production built it: the overlay row hangs off the
/// IMPERSONATOR's own session (`SessionService::validate` consults
/// `active_for_session(row.session_id)`), written directly through
/// `ImpersonationRepo::start` because no entry point will mint this combination.
///
/// ⚠ **Resolving this token no longer yields the target.** It yields the ADMIN,
/// with `impersonator: None`, because `validate` re-evaluates
/// `egret_auth::impersonation::overlay_refused_by_platform_authority` on every
/// request and a ws-scoped overlay over a platform-role holder is **inert**. That
/// is the property this helper now exists to let a test observe, at the HTTP layer
/// the escalation lived on — see
/// `a_ws_scoped_overlay_over_a_platform_role_target_is_decided_as_the_admin`.
///
/// It used to exist for the opposite reason: to hand `/api/control-plane` a
/// principal carrying platform authority AND `workspace_scope = Some(ws)`, so the
/// containment in `egret_perm_compile::routes::confine_to_overlay_workspace` could
/// be driven over HTTP. That state is now unrepresentable, so those tests call the
/// containment directly instead
/// (`confine_to_overlay_workspace_allows_its_own_workspace_and_refuses_another`).
///
/// Returns `(token, admin_user_id)`. The second half is load-bearing: giving the
/// ADMIN the same platform permission afterwards is what distinguishes "the
/// overlay is inert" from "any session with an overlay row is refused", and
/// without that arm a refusal and a `return Err(Forbidden)` look identical.
async fn ws_scoped_overlay_session_with(
    f: &mut Fixture,
    scope: Uuid,
    permissions: &[&str],
) -> (String, Uuid) {
    let steward = new_user(f, "perm_overlay_steward").await;
    for permission in permissions {
        grant_platform_permission(f, steward, permission).await;
    }
    let ws_admin = new_user(f, "perm_overlay_wsadmin").await;
    let issued = f
        .sessions
        .create(egret_core::UserId(ws_admin), "test", "127.0.0.1")
        .await
        .expect("create ws_admin session");
    let session_id = f
        .sessions
        .session_id_for_token(&issued.token)
        .await
        .expect("resolve session id")
        .expect("the session exists");
    egret_auth::impersonation::ImpersonationRepo::new(f.db.clone())
        .start(egret_auth::impersonation::NewImpersonation {
            admin_session_id: session_id,
            impersonator_id: egret_core::UserId(ws_admin),
            target_user_id: egret_core::UserId(steward),
            workspace_scope: Some(egret_core::WorkspaceId(scope)),
            reason: "SP4d debt Task 4 containment test".to_string(),
            ttl_minutes: 10,
        })
        .await
        .expect("start overlay");
    (issued.token, ws_admin)
}

/// A synthesized `Principal` **inside an impersonation overlay**, for the direct
/// tests of the confinement guards: `Some(ws)` is a `ws_admin`'s ws-scoped
/// overlay (confined to `ws`), `None` is a `platform_admin`'s unscoped one
/// (unrestricted — see `confine_to_overlay_workspace`).
///
/// Synthesized rather than resolved from a session on purpose: the ws-scoped
/// shape with platform authority is exactly what `SessionService::validate`
/// refuses to build since `391c3c3a6`, so a test that needs that value has to
/// construct it. The guards under test read nothing but
/// `principal.impersonator.workspace_scope`, so no other field can matter.
fn principal_under_overlay(workspace_scope: Option<Uuid>) -> egret_core::Principal {
    egret_core::Principal {
        impersonator: Some(egret_core::context::Impersonator {
            user_id: egret_core::UserId(Uuid::new_v4()),
            email: "admin@egret.local".to_string(),
            workspace_scope: workspace_scope.map(egret_core::WorkspaceId),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(30),
        }),
        ..principal_without_overlay()
    }
}

/// A synthesized `Principal` that is NOT inside an overlay — the control cell for
/// the confinement guards, which must confine nobody here.
fn principal_without_overlay() -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(Uuid::new_v4()),
        email: "steward@egret.local".to_string(),
        platform_roles: vec!["data_steward".to_string()],
        permissions: vec!["policy:update".to_string()],
        group_ids: Vec::new(),
        impersonator: None,
    }
}

/// A fresh workspace with no catalogs of its own.
async fn new_workspace(f: &Fixture, tag: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
        .bind(unique(tag))
        .bind("Perm Task4 WS")
        .fetch_one(&f.db.pool)
        .await
        .expect("workspace")
}

/// Assign `catalog_id` to `workspace_id` — `registry.catalog_workspaces`, the
/// relation the `perm` model derives its implicit `workspace_assignment` grants
/// from, and the one the overlay containment is keyed to.
///
/// Written with raw SQL on purpose: production's only writer is
/// `egret_catalog_registry::RegistryStore::set_catalog_workspaces_in_tx`, which
/// this crate does not depend on, and whose point is that it composes the grant
/// sync — nothing here needs the grant, and reaching for a convenience wrapper is
/// exactly what that crate removed.
async fn assign_catalog_to_workspace(f: &Fixture, catalog_id: Uuid, workspace_id: Uuid) {
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1,$2)",
    )
    .bind(catalog_id)
    .bind(workspace_id)
    .execute(&f.db.pool)
    .await
    .expect("assign catalog to workspace");
}

fn grant_body(catalog_id: Uuid, user: &str) -> Value {
    json!({
        "effect": "allow",
        "actions": ["read"],
        "scope": { "catalog_id": catalog_id, "schema": "sales", "table": "orders", "column": null },
        "principals": { "include": [{ "kind": "user", "ref": user }], "exclude": [] }
    })
}

// ---------------------------------------------------------------------------
// the sync: a write over HTTP reaches the compiled output
// ---------------------------------------------------------------------------

/// **The test this task exists for.** A grant written through the endpoint
/// must be COMPILED by the same request — asserted against
/// `policy.authz_policies`, the table the engine's Ranger plugin downloads,
/// not against `perm.grants`. Delete the recompile call from `create_grant`
/// and this dies; delete it and assert only on `perm.grants` and nothing
/// does.
#[tokio::test]
async fn creating_a_grant_over_http_compiles_its_policy() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let before = f.compiled_policy_names().await;
    assert!(
        before.is_empty(),
        "fresh fixture profile must start with no compiled policies, had {before:?}"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");

    let grant_id = body["id"].as_str().expect("minted grant id").to_string();
    let expected = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(
        f.compiled_policy_names().await,
        vec![expected.clone()],
        "the grant's Ranger policy must exist under the profile the moment the \
         request returns — a grant stored but never compiled reads as live in \
         the UI while the engine enforces the previous policy set"
    );
    assert!(
        body["compile"]["services"]
            .as_array()
            .expect("compile.services")
            .iter()
            .any(|v| v.as_i64() == Some(f.service_id)),
        "the response must name the profile it recompiled; body: {body}"
    );
    assert_eq!(
        body["compile"]["created"], 1,
        "exactly one policy created for this grant; body: {body}"
    );
}

/// The other half: deleting the grant must REAP the policy in the same
/// request. This is the direction that fails dangerously — a policy left
/// behind keeps granting access egret believes it revoked.
#[tokio::test]
async fn deleting_a_grant_over_http_reaps_its_policy() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let grant_id = body["id"].as_str().expect("minted grant id").to_string();
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(f.compiled_policy_names().await, vec![name.clone()]);

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/perm/grants/{grant_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["deleted"], 1,
        "the delete must report reaping exactly one policy; body: {body}"
    );
    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "the Ranger policy must be GONE — a compiled policy outliving its grant \
         keeps granting access egret's own model no longer states"
    );
}

/// A tag-selected rule, on StarRocks, compiles per tagged column (D5). The
/// fixture has exactly one tagged column, so exactly one policy must appear
/// under the fixture's own profile — and disappear again on delete.
#[tokio::test]
async fn creating_a_tag_rule_over_http_compiles_its_policy() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(json!({
                    "name": unique("pii-hash"),
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let rule_id = body["id"].as_str().expect("minted rule id").to_string();

    let names = f.compiled_policy_names().await;
    assert_eq!(
        names.len(),
        1,
        "one tagged column in this fixture ⇒ one StarRocks masking policy; got {names:?}"
    );
    assert!(
        names[0].starts_with(&format!("{COMPILER_RULE_PREFIX}{rule_id}")),
        "the compiled policy must be derived from THIS rule's id; got {names:?}"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/perm/rules/{rule_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "deleting the rule must reap the mask it compiled to"
    );
}

/// A grant naming a catalog NO engine reaches affects no profile — so the
/// write still succeeds, and the fixture's profile is untouched. This pins
/// the affected-service rule from the other side: it is derived from the
/// grant's own catalog, not "every profile, always".
#[tokio::test]
async fn a_grant_on_an_unreachable_catalog_touches_no_profile() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let orphan = orphan_catalog(&f, "perm-orphan").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(orphan, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert_eq!(
        body["compile"]["services"].as_array().unwrap().len(),
        0,
        "no engine reaches that catalog, so no profile is affected; body: {body}"
    );
    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "the fixture's own profile must be untouched"
    );

    // **The other half, and the one that matters.** An empty affected set means
    // the recompile validates NOTHING, so a grant that can never compile would
    // commit here and lie in wait: attach an engine to that catalog later and
    // every recompile of the profile 400s, which also rolls back every tag
    // write on the warehouse (`egret-bi::catalog_tags` recompiles inside the
    // tag write's transaction). The service-INDEPENDENT half of compilation is
    // therefore checked at write time, affected set or no affected set.
    let before = grants_on(&f, orphan).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(orphan, "analyst@egret.local")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an ill-formed grant must be refused even when it affects no profile — \
         otherwise it poisons the first profile that ever reaches its catalog; \
         body: {body}"
    );
    assert_eq!(
        grants_on(&f, orphan).await,
        before,
        "the refused grant must not have been stored"
    );
}

/// A registry catalog attached to NO engine at all, under its own warehouse
/// (`registry.catalogs` is UNIQUE on `warehouse_id`). Its affected-service set
/// is empty, which is the case where the service-independent shape check is the
/// only thing that can refuse an uncompilable row.
async fn orphan_catalog(f: &Fixture, tag: &str) -> Uuid {
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique(tag))
            .bind("Perm Orphan WS")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-perm-orphan-test','AKIATEST') RETURNING id",
    )
    .bind(unique(tag))
    .fetch_one(&f.db.pool)
    .await
    .expect("storage connection");
    let orphan_wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("perm_orphan_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("warehouse");
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("perm_routes_orphan"))
    .bind(orphan_wh)
    .fetch_one(&f.db.pool)
    .await
    .expect("orphan catalog")
}

/// Grants naming one catalog — the scoped population a test can count without
/// racing the shared database.
async fn grants_on(f: &Fixture, catalog_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
        .bind(catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count grants")
}

/// The rule-side twin of the assertion above: a rule whose exempt set can never
/// compile is refused at write time, even though its tag selector's affected
/// set would have caught it here. The point is that the refusal does not depend
/// on the affected set being non-empty.
#[tokio::test]
async fn a_rule_that_can_never_compile_is_refused_at_write_time() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let name = unique("unexpressible");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(json!({
                    "name": name,
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "path", "catalog_id": f.catalog_id,
                                  "schema": "sales", "table": "orders", "column": "email" },
                    // A masking item has no roles bucket at all — this can
                    // never compile against any engine or profile.
                    "exempt": { "include": [{ "kind": "role", "ref": "auditor" }], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(
        rules_named(&f, &name).await,
        0,
        "the refused rule must not have been stored"
    );
}

async fn rules_named(f: &Fixture, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perm.rules WHERE name = $1")
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .expect("count rules")
}

/// `create_grant` refuses an empty `actions`; `create_rule` must refuse a blank
/// name the same way rather than storing one and letting every later skip log
/// and compile error identify the rule by `""`.
#[tokio::test]
async fn a_rule_with_a_blank_name_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(json!({
                    "name": "   ",
                    "kind": { "type": "row_filter", "filter_expr": "1=1" },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

/// A grant the compiler cannot express must be REFUSED, not stored. An email
/// address in a `user` principal is exactly that case
/// (`CompileError::UnusableUserRef` — Ranger matches bare usernames), and the
/// recompile that refuses it runs inside the write's transaction, so the row
/// has to be gone afterwards.
#[tokio::test]
async fn a_grant_that_cannot_compile_is_refused_and_not_stored() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let before = f.grant_count().await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst@egret.local")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an uncompilable grant must be refused, not stored; body: {body}"
    );
    assert_eq!(
        f.grant_count().await,
        before,
        "the refused grant must have been ROLLED BACK — a grant stored without \
         its policies is worse than a rejected one, because the UI shows it as active"
    );
    assert_eq!(f.compiled_policy_names().await, Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// PATCH: editing in place (SP4b D1)
// ---------------------------------------------------------------------------

/// A SECOND StarRocks profile, with its own engine, warehouse and registry
/// catalog attached to that engine. Returned as `(service_id, catalog_id)`.
///
/// The catalog-move patch test needs two profiles that reach DIFFERENT
/// catalogs: the defect it discriminates against (recompiling only the NEW
/// affected set) is invisible when both catalogs land in the same profile.
async fn extra_profile(f: &Fixture, tag: &str) -> (i64, Uuid) {
    // Held until this function returns — see `TAG_FANOUT_LOCK`.
    let _universe = advisory_lock(&f.dsn).await;
    let service = f
        .policy
        .create_service(&unique(tag), "starrocks", &json!({}))
        .await
        .expect("create second service");
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique(tag))
            .bind("Perm Patch WS")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-perm-patch-test','AKIATEST') RETURNING id",
    )
    .bind(unique(tag))
    .fetch_one(&f.db.pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("perm_patch_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique(tag))
    .bind(service.id)
    .fetch_one(&f.db.pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("perm_patch_cat"))
    .bind(warehouse_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach catalog");
    (service.id, catalog_id)
}

// ---------------------------------------------------------------------------
// Two-profile fixture (SP4d-2a Task 2)
// ---------------------------------------------------------------------------

/// One registry catalog attached to TWO engines whose authz profiles are of
/// DIFFERENT types — one `trino`, one `starrocks`. Returns
/// `(trino_service_id, starrocks_service_id, catalog_id)`.
///
/// This is the shape `extra_profile` deliberately is NOT: that helper reaches
/// a second, DIFFERENT catalog. This one exists live — warehouse `sp4a10_wh`
/// is served by profile 52 (Trino) and 55 (StarRocks), and both perm grants
/// on it compiled to `{"services":[52,55]}` — and until this fixture existed,
/// nowhere in the test suite. A single `RequestResource` cannot serve both
/// profiles: `write` is `insert,delete` on Trino and `insert,update,delete`
/// on StarRocks (`egret_perm::actions`), and the second resource level is
/// named `schema` on Trino vs `database` on StarRocks (`resource_level_names()`
/// in `grants.rs`). That is what forces a genuinely per-profile answer instead
/// of one that only happens to look right with a single profile.
async fn two_profile_catalog(f: &Fixture, tag: &str) -> (i64, i64, Uuid) {
    // Held until this function returns — see `TAG_FANOUT_LOCK`.
    let _universe = advisory_lock(&f.dsn).await;
    egret_policy::seed_trino(&f.policy)
        .await
        .expect("seed trino service-def");
    let trino_service = f
        .policy
        .create_service(&unique(&format!("{tag}_trino")), "trino", &json!({}))
        .await
        .expect("create trino service");
    let starrocks_service = f
        .policy
        .create_service(&unique(&format!("{tag}_sr")), "starrocks", &json!({}))
        .await
        .expect("create starrocks service");
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique(tag))
            .bind("Perm Two-Profile WS")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-perm-two-profile-test','AKIATEST') RETURNING id",
    )
    .bind(unique(tag))
    .fetch_one(&f.db.pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("perm_two_profile_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("warehouse");
    let trino_engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{\"imageVersion\":\"483\"}'::jsonb,$2) RETURNING id",
    )
    .bind(unique(&format!("{tag}_trino_eng")))
    .bind(trino_service.id)
    .fetch_one(&f.db.pool)
    .await
    .expect("trino engine");
    let starrocks_engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique(&format!("{tag}_sr_eng")))
    .bind(starrocks_service.id)
    .fetch_one(&f.db.pool)
    .await
    .expect("starrocks engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("perm_two_profile_cat"))
    .bind(warehouse_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(trino_engine_id)
    .bind(catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach trino engine to catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(starrocks_engine_id)
    .bind(catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach starrocks engine to catalog");
    (trino_service.id, starrocks_service.id, catalog_id)
}

/// The fixture every downstream per-profile test (Tasks 5, 6, 8, 9) depends
/// on: this asserts the fixture is what it claims to be, rather than trusting
/// it. `services_for_catalog_via` is "what an engine actually downloads" —
/// the exact semantics the served-artifact package cares about — so if either
/// attachment silently failed to take, or a second engine were created
/// without an `authz_service_id`, this would degrade to one id and every
/// per-profile test downstream would pass vacuously.
#[tokio::test]
async fn two_profile_catalog_is_really_served_by_two_different_engine_types() {
    let Some(f) = setup().await else { return };
    let (trino_service_id, starrocks_service_id, catalog_id) =
        two_profile_catalog(&f, "two-profile-honesty").await;

    let mut conn = f.db.pool.acquire().await.expect("acquire conn");
    let mut service_ids = egret_policy::services_for_catalog_via(&mut conn, catalog_id)
        .await
        .expect("services_for_catalog_via");
    service_ids.sort_unstable();
    let mut expected = vec![trino_service_id, starrocks_service_id];
    expected.sort_unstable();
    assert_eq!(
        service_ids, expected,
        "the catalog must resolve to exactly the two services the fixture \
         just attached — a silently-dropped attachment would make every \
         per-profile test downstream vacuously green; got {service_ids:?}"
    );

    let trino = f
        .policy
        .get_service_by_id(trino_service_id)
        .await
        .expect("trino service row");
    let starrocks = f
        .policy
        .get_service_by_id(starrocks_service_id)
        .await
        .expect("starrocks service row");
    assert_eq!(
        trino.r#type, "trino",
        "the first profile must genuinely be a Trino profile, not merely a \
         second StarRocks one"
    );
    assert_eq!(
        starrocks.r#type, "starrocks",
        "the second profile must genuinely be a StarRocks profile"
    );
}

/// Create one grant through the endpoint and return its id.
async fn create_grant_returning_id(f: &Fixture, token: &str, body: Value) -> String {
    let (status, created) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/grants", token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    created["id"].as_str().expect("minted grant id").to_string()
}

/// The users a compiled access policy's first item names — the compiled output
/// an edit has to actually change, rather than the `perm.grants` row.
fn policy_item_users(body: &Value) -> Vec<String> {
    body["policyItems"][0]["users"]
        .as_array()
        .unwrap_or_else(|| panic!("policyItems[0].users in {body}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// **The id survives, and the compiled POLICY BODY changes.** A patch that
/// stored the new principals without recompiling would pass a `perm.grants`
/// assertion and leave the engine enforcing the old principal — so this looks
/// at `policy.authz_policies`, and at the policy NAME, which carries the grant
/// id and therefore fails if the handler delete-and-recreated instead.
#[tokio::test]
async fn patching_a_grants_principals_recompiles_and_keeps_the_id() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(f.compiled_policy_names().await, vec![name.clone()]);
    assert_eq!(
        policy_item_users(&f.compiled_policy_body(&name).await),
        vec!["analyst".to_string()]
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(grant_body(f.catalog_id, "auditor")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["updated"], 1,
        "the patch must have UPDATED the compiled policy in place — 1 created plus \
         1 deleted would mean the id churned; body: {body}"
    );
    assert_eq!(
        f.compiled_policy_names().await,
        vec![name.clone()],
        "the policy name carries the grant id, so an unchanged name is the id surviving"
    );
    assert_eq!(
        policy_item_users(&f.compiled_policy_body(&name).await),
        vec!["auditor".to_string()],
        "the COMPILED policy must name the patched principal — a patch that wrote \
         `perm.grants` and skipped the recompile leaves the engine enforcing the old one"
    );

    // And the row itself kept its id, which is what an `explain` reason links to.
    let (status, listed) = into_json(
        f.app()
            .oneshot(req("GET", "/perm/grants", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mine = listed
        .as_array()
        .expect("array")
        .iter()
        .find(|g| g["id"] == grant_id)
        .unwrap_or_else(|| panic!("the patched grant must still carry its id; got {listed}"));
    assert_eq!(mine["principals"]["include"][0]["ref"], "auditor");
}

/// **THE test this task exists for.** Moving a grant to another catalog must
/// REAP the policy from the profile it used to reach and create it in the new
/// one. Asserting only the new profile passes against the bug: recompiling
/// just the new affected set leaves the old policy in place, served to that
/// engine forever with nothing in the model producing it — SP4a's "a deleted
/// grant's policy is enforced forever" Critical, through the edit door.
#[tokio::test]
async fn patching_a_grants_catalog_reaps_the_policy_from_the_old_profile() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (other_service, other_catalog) = extra_profile(&f, "perm-patch-move").await;

    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(
        f.compiled_policy_names().await,
        vec![name.clone()],
        "the grant starts out compiled under the FIRST profile"
    );
    assert_eq!(
        f.compiled_policy_names_under(other_service).await,
        Vec::<String>::new(),
        "and not under the second one"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(grant_body(other_catalog, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "THE OLD PROFILE MUST BE REAPED. Its engine still downloads this policy, and \
         after the patch nothing in the model produces it and no later recompile of \
         that profile would sweep it — recompiling only the NEW affected set leaves \
         the old grant enforced forever; body: {body}"
    );
    assert_eq!(
        f.compiled_policy_names_under(other_service).await,
        vec![name],
        "and the policy must exist under the profile the grant now reaches, under the \
         SAME id; body: {body}"
    );
    let services = body["services"].as_array().expect("services");
    assert!(
        services.iter().any(|v| v.as_i64() == Some(f.service_id)),
        "the response must name the OLD profile it recompiled; body: {body}"
    );
    assert!(
        services.iter().any(|v| v.as_i64() == Some(other_service)),
        "and the new one; body: {body}"
    );
    assert_eq!(body["deleted"], 1, "one policy reaped; body: {body}");
    assert_eq!(body["created"], 1, "one policy created; body: {body}");
}

/// A patch that could store an uncompilable row would reopen at update the
/// defect SP4a closed at create, so the patch path runs the SAME
/// `validate_grant_shape` — and the refusal must leave both the row and its
/// compiled policy exactly as they were.
///
/// **Over-determined on purpose, and no single mutation kills it.** Two
/// independent guards refuse this input: `validate_grant_shape` before the write,
/// and the recompile after it. Remove either and the other still returns the 400
/// this asserts. It is kept as a regression guard, and each guard has its own
/// test that DOES die alone — the pre-write check's is
/// `patching_a_grant_that_affects_no_profile_still_validates_its_shape` (an empty
/// affected set means the recompile validates nothing), and the rollback's is
/// `a_patch_the_profile_cannot_compile_rolls_the_stored_row_back` (an
/// `UnsupportedMaskType` is service-dependent, so only the recompile can see it).
/// Do not read this test's passing as evidence about either guard on its own.
#[tokio::test]
async fn patching_a_grant_into_an_uncompilable_shape_is_refused_and_changes_nothing() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");

    // Ranger matches bare usernames, so an email address in a `user` principal
    // can never compile (`CompileError::UnusableUserRef`).
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(grant_body(f.catalog_id, "analyst@egret.local")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an uncompilable patch must be refused; body: {body}"
    );
    assert_eq!(
        f.compiled_policy_names().await,
        vec![name.clone()],
        "the refused patch must have ROLLED BACK — a stored-but-uncompiled grant is \
         worse than a rejected one, because the UI shows it as active"
    );
    assert_eq!(
        policy_item_users(&f.compiled_policy_body(&name).await),
        vec!["analyst".to_string()],
        "and the policy body must be the one the grant had before the refused patch"
    );
    // The ROW too, not just the policy: a handler that committed the UPDATE and
    // only then discovered the compile failure would leave the grant reading as
    // patched-and-active in the UI while no engine had ever been told.
    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT ref FROM perm.grant_principals WHERE grant_id = $1 AND mode = 'include'",
    )
    .bind(Uuid::parse_str(&grant_id).unwrap())
    .fetch_all(&f.db.pool)
    .await
    .expect("read principals");
    assert_eq!(
        stored,
        vec!["analyst".to_string()],
        "the refused patch must not have committed its own UPDATE"
    );
}

/// **SP4d debt Task 12 — the write-path 400, on the one path where only the Rust
/// guard can produce it.**
///
/// `schema: ""` compiles to `"schema": {"values": [""]}`, Ranger's
/// `init()` drops an empty policy value, and the level becomes `isMatchAny` — so
/// the grant is enforced in every schema of the catalog
/// (`compile_grants.rs::an_empty_scope_level_would_be_enforced_in_every_schema`).
/// `perm.grants`' pre-existing level-skip CHECK could not see it, because `''` is
/// not NULL.
///
/// **Why an ORPHAN catalog, and why the assertion is `400` and not "not 2xx".**
/// Three guards can refuse this input and the test has to be pointed at one.
/// `0178`'s CHECK would refuse the INSERT whatever happens in Rust, and on a
/// catalog an engine reaches `compile_grant`'s own copy of the guard would refuse
/// the recompile. An orphan catalog affects no profile, so
/// `recompile_services_in_tx(&[])` validates nothing, and the only thing between
/// the body and the database is `validate_grant_shape`. Remove it and this request
/// still fails — but as a **500**, because the constraint catches it with a
/// message naming a schema internal. Task 2 of this plan mutation-measured exactly
/// that, which is why the status code is the assertion and the message is checked
/// for the level name.
///
/// **The control is `'*'` in the same field**: it is a legitimate Ranger policy
/// value — a steward scopes a grant to a whole catalog or schema routinely, and
/// `scope_to_resources` emits `*` itself for every unnamed level — so a guard that
/// refused it would break a real scope. It must be CREATED, 201. Without that arm
/// a predicate refusing every non-literal would pass.
#[tokio::test]
async fn an_empty_scope_level_is_a_400_and_a_star_is_created() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "perm-empty-level").await;

    // ⚠ `table` is a parameter and not a constant because SP4d debt Task 14 made
    // the `*` CONTROL's own shape matter: `('*', 'orders')` is now a 400 of its
    // own (a named level under a match-any one is the level skip `grants_check`
    // already forbids), so this test's control moved to `('*', '*')` — a wildcard
    // with nothing named beneath it, which is what `0178` must not refuse.
    // `a_wildcard_level_over_a_named_one_is_a_400_and_a_wildcard_tail_is_created`
    // owns the moved shape.
    let body_with = |schema: Value, table: Value| {
        json!({
            "effect": "allow",
            "actions": ["read"],
            "scope": { "catalog_id": orphan, "schema": schema, "table": table, "column": null },
            "principals": { "include": [{ "kind": "user", "ref": "analyst" }], "exclude": [] }
        })
    };

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(body_with(json!(""), json!("orders"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an empty scope level must be a 400 from the write-path guard, not a 500 from \
         the CHECK; body: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("schema"),
        "the refusal must name the level the author has to fix; body: {body}"
    );
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
        .bind(orphan)
        .fetch_one(&f.db.pool)
        .await
        .expect("count");
    assert_eq!(stored, 0, "the refused create must have stored nothing");

    // CONTROL: `*` is a real policy value and stays authorable.
    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(body_with(json!("*"), json!("*"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "`*` at a level is a legitimate scope and must still be writable; body: {created}"
    );
    let grant_id = created["id"].as_str().expect("minted id").to_string();

    // And the PATCH path runs the same guard, leaving the stored row alone.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(body_with(json!(""), json!("orders"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the patch path must refuse it too; body: {body}"
    );
    let schema: Option<String> =
        sqlx::query_scalar("SELECT schema_name FROM perm.grants WHERE id = $1")
            .bind(Uuid::parse_str(&grant_id).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .expect("read back");
    assert_eq!(
        schema.as_deref(),
        Some("*"),
        "the refused patch must not have committed its own UPDATE"
    );
}

/// **SP4d debt Task 14 over HTTP — the status code is the assertion.**
///
/// **Why an ORPHAN catalog, and why `400` and not "not 2xx".** Three guards can
/// refuse this input and the test has to be pointed at one. `0179`'s CHECK would
/// refuse the INSERT whatever happens in Rust, and on a catalog an engine reaches
/// `compile_grant`'s own copy of the guard would refuse the recompile. An orphan
/// catalog affects no profile, so `recompile_services_in_tx(&[])` validates
/// nothing and the only thing between the body and the database is
/// `validate_grant_shape`. Remove that call and this request still fails — but as
/// a **500**, because the constraint catches it with a message naming a schema
/// internal. Tasks 2, 3 and 12 mutation-measured exactly that, three times, which
/// is why the status code is asserted and the body is checked for both level
/// names.
///
/// **The control is a wildcard TAIL in the same field**, which must be CREATED,
/// 201. This is the arm that keeps the fix from being "`*` is illegitimate": a
/// steward scopes a grant to every table of every schema on purpose, and
/// `scope_to_resources` emits that shape itself for an unnamed level. Without it
/// a predicate refusing `*` outright would pass.
#[tokio::test]
async fn a_wildcard_level_over_a_named_one_is_a_400_and_a_wildcard_tail_is_created() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "perm-wildcard-gap").await;

    let body_with = |schema: Value, table: Value| {
        json!({
            "effect": "allow",
            "actions": ["read"],
            "scope": { "catalog_id": orphan, "schema": schema, "table": table, "column": null },
            "principals": { "include": [{ "kind": "user", "ref": "analyst" }], "exclude": [] }
        })
    };

    // THE FINDING, as two ordinary keystrokes in `ScopePicker` produce it — and
    // the second spelling, which keying on the literal `*` would have missed.
    for schema in ["*", "**"] {
        let (status, body) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/grants",
                    &token,
                    Some(body_with(json!(schema), json!("orders"))),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a match-any {schema:?} schema over a named table must be a 400 from the \
             write-path guard, not a 500 from the CHECK; body: {body}"
        );
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("schema") && error.contains("orders"),
            "the refusal must name the match-any level AND what is named under it, so \
             the author knows which to change; body: {body}"
        );
        assert!(
            error.contains("does not express"),
            "and it must read as consistency with the model's existing decision rather \
             than as a new restriction; body: {body}"
        );
    }
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
        .bind(orphan)
        .fetch_one(&f.db.pool)
        .await
        .expect("count");
    assert_eq!(stored, 0, "the refused creates must have stored nothing");

    // CONTROL: a wildcard with nothing named beneath it is a real scope and stays
    // authorable.
    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(body_with(json!("*"), json!("*"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "`('*','*')` names nothing under a wildcard and must still be writable; \
         body: {created}"
    );
    let grant_id = created["id"].as_str().expect("minted id").to_string();

    // And the PATCH path runs the same guard, leaving the stored row alone.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(body_with(json!("*"), json!("orders"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the patch path must refuse it too; body: {body}"
    );
    let table: Option<String> =
        sqlx::query_scalar("SELECT table_name FROM perm.grants WHERE id = $1")
            .bind(Uuid::parse_str(&grant_id).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .expect("read back");
    assert_eq!(
        table.as_deref(),
        Some("*"),
        "the refused patch must not have committed its own UPDATE"
    );
}

/// **The half the recompile cannot cover.** A grant on a catalog no engine
/// reaches affects no profile, so the recompile validates NOTHING — exactly as
/// on the create path — and only the service-independent shape check can refuse
/// an uncompilable patch. Without it such a row commits and lies in wait:
/// attach an engine to that catalog later and every recompile of the profile
/// 400s, rolling back every tag write on the warehouse with it.
#[tokio::test]
async fn patching_a_grant_that_affects_no_profile_still_validates_its_shape() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "perm-patch-orphan").await;
    let grant_id = create_grant_returning_id(&f, &token, grant_body(orphan, "analyst")).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                // Ranger matches bare usernames: an email can never compile.
                Some(grant_body(orphan, "analyst@egret.local")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an ill-formed patch must be refused even when it affects no profile; body: {body}"
    );
    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT ref FROM perm.grant_principals WHERE grant_id = $1 AND mode = 'include'",
    )
    .bind(Uuid::parse_str(&grant_id).unwrap())
    .fetch_all(&f.db.pool)
    .await
    .expect("read principals");
    assert_eq!(
        stored,
        vec!["analyst".to_string()],
        "the refused patch must not have been stored"
    );
}

/// An empty `actions` is refused on patch for the same reason it is on create.
#[tokio::test]
async fn patching_a_grant_to_no_actions_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let mut body = grant_body(f.catalog_id, "analyst");
    body["actions"] = json!([]);
    let (status, got) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(body),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {got}");
}

/// A `workspace_assignment` grant belongs to the assignment sync, which
/// derives it from `(catalog, workspace)` and recreates it on demand (D3). A
/// patch here would be reverted by the next sync or collide with
/// `grants_one_per_assignment` — the same reason `NewGrant` cannot MINT one.
#[tokio::test]
async fn patching_a_workspace_assignment_grant_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("perm-patch-assign-ws"))
            .bind("Perm Patch Assign WS")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    egret_perm::sync_assignment_grants(&f.perm, f.catalog_id, &[ws_id])
        .await
        .expect("assign catalog to workspace");
    let implicit: Uuid = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment'",
    )
    .bind(f.catalog_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("implicit grant");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{implicit}"),
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a grant the assignment sync owns must not be editable here; body: {body}"
    );
    let origin: String = sqlx::query_scalar("SELECT origin FROM perm.grants WHERE id = $1")
        .bind(implicit)
        .fetch_one(&f.db.pool)
        .await
        .expect("read origin");
    assert_eq!(origin, "workspace_assignment", "the row must be untouched");
}

#[tokio::test]
async fn patching_an_unknown_grant_or_rule_is_a_404() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let missing = Uuid::new_v4();
    for (uri, body) in [
        (
            format!("/perm/grants/{missing}"),
            grant_body(f.catalog_id, "analyst"),
        ),
        (
            format!("/perm/rules/{missing}"),
            json!({
                "name": unique("nope"),
                "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                "selector": { "type": "path", "catalog_id": f.catalog_id,
                              "schema": "sales", "table": "orders", "column": "email" },
                "exempt": { "include": [], "exclude": [] }
            }),
        ),
    ] {
        let (status, got) = into_json(
            f.app()
                .oneshot(req("PATCH", &uri, &token, Some(body)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} -> {got}");
    }
}

/// **A PATCH that omits `principals` must not silently revoke the grant**
/// (SP4b-1 review, I-2). `NewGrant.principals` is `#[serde(default)]`, so the
/// field is optional on the wire and optional in the generated TypeScript; an
/// empty include then reaches `compile_desired`, which SKIPS the grant, so the
/// recompile produces nothing and the sweep REAPS the policy the grant had. A
/// 200 and a dropped grant is the worst answer available, so the write paths
/// refuse the body instead.
///
/// Asserted on the patch path AND the create path, because it is one guard in
/// one function and the create half is where a UI most easily sends it.
#[tokio::test]
async fn a_grant_naming_no_principal_is_refused_rather_than_revoking() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let scope = json!({ "catalog_id": f.catalog_id, "schema": "sales",
                        "table": "orders", "column": null });

    // The PATCH first: it carries the load-bearing assertion (the live grant's
    // policy survives the refusal), so it is the one a mutation reports.
    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(f.compiled_policy_names().await, vec![name.clone()]);

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                // The full-body-replace contract's sharp edge: a UI that omits
                // the field is asking for "this grant, with nobody in it".
                Some(json!({ "effect": "allow", "actions": ["read"], "scope": scope })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a patch that includes nobody must be refused, not accepted with a 200; body: {body}"
    );
    assert_eq!(
        f.compiled_policy_names().await,
        vec![name.clone()],
        "THE POLICY MUST STILL BE THERE. An empty include compiles to a SKIP, so \
         accepting this patch reaps the policy and the grant stops being enforced \
         while the UI reports success"
    );
    assert_eq!(
        policy_item_users(&f.compiled_policy_body(&name).await),
        vec!["analyst".to_string()],
        "and it must still name the principal it named before"
    );

    // A preview of the same body is refused for the same reason, since it runs
    // the same write.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "id": grant_id,
                             "grant": { "effect": "allow", "actions": ["read"], "scope": scope } })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");

    // And the create path — one guard in one function, but the create half is
    // where a UI most easily sends it. Both the omitted field and an explicitly
    // empty one, since `#[serde(default)]` makes them the same value.
    for payload in [
        json!({ "effect": "allow", "actions": ["read"], "scope": scope }),
        json!({ "effect": "allow", "actions": ["read"], "scope": scope,
                "principals": { "include": [], "exclude": [] } }),
    ] {
        let (status, body) = into_json(
            f.app()
                .oneshot(req("POST", "/perm/grants", &token, Some(payload)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a grant that includes nobody must be refused; body: {body}"
        );
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("principal"),
            "and the refusal must say what is missing; body: {body}"
        );
    }
    assert_eq!(
        f.compiled_policy_names().await,
        vec![name],
        "and no refused create compiled anything of its own"
    );
}

/// **The rollback assertion, on an input the pre-write shape check cannot
/// catch.** `validate_rule_shape` is service-INDEPENDENT by construction, so
/// `UnsupportedMaskType` — which reads the target engine's service-def — can
/// only be discovered by the recompile, i.e. AFTER the `UPDATE`. StarRocks'
/// def declares no `MASK_SHOW_LAST_4` (Trino's does), so this is exactly that
/// case: the request must fail AND the stored row must be the one that was
/// there before. A handler that committed the write and only then reported the
/// compile failure would leave the rule reading as patched-and-active in the UI
/// while the engine had never been told — the state this model refuses to have.
#[tokio::test]
async fn a_patch_the_profile_cannot_compile_rolls_the_stored_row_back() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let rule_name = unique("pii-mask-rollback");
    let path_rule = |mask: &str| {
        json!({
            "name": rule_name,
            "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
            "selector": { "type": "path", "catalog_id": f.catalog_id,
                          "schema": "sales", "table": "orders", "column": "email" },
            "exempt": { "include": [], "exclude": [] }
        })
    };
    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(path_rule("MASK_HASH")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let rule_id = created["id"].as_str().expect("minted rule id").to_string();
    let name = f.compiled_policy_names().await.remove(0);

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/rules/{rule_id}"),
                &token,
                Some(path_rule("MASK_SHOW_LAST_4")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a mask type this profile's service-def does not declare must fail the \
         request; body: {body}"
    );
    let stored: Option<String> =
        sqlx::query_scalar("SELECT mask_type FROM perm.rules WHERE id = $1")
            .bind(Uuid::parse_str(&rule_id).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .expect("read mask_type");
    assert_eq!(
        stored.as_deref(),
        Some("MASK_HASH"),
        "the UPDATE ran BEFORE the failing recompile, so the transaction must have \
         rolled it back — a stored-but-uncompiled rule is worse than a rejected one"
    );
    assert_eq!(
        f.compiled_policy_body(&name).await["dataMaskPolicyItems"][0]["dataMaskInfo"]
            ["dataMaskType"],
        "MASK_HASH",
        "and the compiled policy must still be the one the engine was told about"
    );
}

/// The rule half of D1. A path selector (not a tag one) so the write stays
/// catalog-scoped and this test does not fan out over the shared database.
/// `update_rule_in_tx` rewrites every kind and selector column, so the
/// assertion is on the compiled mask type, which only changes if that write
/// and the recompile both happened.
#[tokio::test]
async fn patching_a_rule_recompiles_and_keeps_the_id() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let rule_name = unique("pii-mask");
    let path_rule = |mask: &str, name: &str| {
        json!({
            "name": name,
            "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
            "selector": { "type": "path", "catalog_id": f.catalog_id,
                          "schema": "sales", "table": "orders", "column": "email" },
            "exempt": { "include": [], "exclude": [] }
        })
    };

    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(path_rule("MASK_HASH", &rule_name)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let rule_id = created["id"].as_str().expect("minted rule id").to_string();
    let names = f.compiled_policy_names().await;
    assert_eq!(names.len(), 1, "one path mask policy; got {names:?}");
    assert!(names[0].starts_with(&format!("{COMPILER_RULE_PREFIX}{rule_id}")));
    let name = names[0].clone();
    assert_eq!(
        f.compiled_policy_body(&name).await["dataMaskPolicyItems"][0]["dataMaskInfo"]
            ["dataMaskType"],
        "MASK_HASH"
    );

    let renamed = unique("pii-nulled");
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/rules/{rule_id}"),
                &token,
                Some(path_rule("MASK_NULL", &renamed)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        f.compiled_policy_names().await,
        vec![name.clone()],
        "the compiled policy name is derived from the rule id, so it must not churn"
    );
    assert_eq!(
        f.compiled_policy_body(&name).await["dataMaskPolicyItems"][0]["dataMaskInfo"]
            ["dataMaskType"],
        "MASK_NULL",
        "the COMPILED mask type must be the patched one; body: {body}"
    );
    let stored: String = sqlx::query_scalar("SELECT name FROM perm.rules WHERE id = $1")
        .bind(Uuid::parse_str(&rule_id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("read rule name");
    assert_eq!(
        stored, renamed,
        "the rule's own name must be the patched one"
    );
}

/// Hold `service_id`'s `policy.authz_services` row lock on a session of its own,
/// and return that session with its backend pid. Releasing means `COMMIT` on the
/// returned connection.
///
/// Used to park a PATCH *after* its pre-lock read and *before* it takes the
/// profile locks — the only window in which the row it read can go stale.
async fn hold_profile_lock(f: &Fixture, service_id: i64) -> (sqlx::PgConnection, i32) {
    use sqlx::Connection as _;
    let mut holder = sqlx::PgConnection::connect(&f.dsn).await.expect("holder");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut holder)
        .await
        .expect("holder pid");
    sqlx::query("BEGIN")
        .execute(&mut holder)
        .await
        .expect("begin");
    sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
        .bind(service_id)
        .execute(&mut holder)
        .await
        .expect("hold the profile lock");
    (holder, pid)
}

/// Block until some backend is waiting on `holder_pid`'s transaction.
///
/// Scoped to that one pid on purpose: "somebody, somewhere is waiting" would be
/// satisfied by another test's contention, and the caller would then race ahead
/// while the request it means to park has not reached the lock yet — which would
/// make the test pass for the wrong reason. Polled, never slept on blindly.
async fn wait_until_parked_on(f: &Fixture, holder_pid: i32) {
    for _ in 0..400 {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
               SELECT 1 FROM pg_locks w \
                 JOIN pg_locks h ON h.locktype = 'transactionid' \
                                AND h.transactionid = w.transactionid \
                                AND h.granted \
                WHERE NOT w.granted AND h.pid = $1)",
        )
        .bind(holder_pid)
        .fetch_one(&f.db.pool)
        .await
        .expect("read pg_locks");
        if blocked {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!(
        "no backend ever blocked on the held profile lock — this test's premise (that \
         the request reaches `lock_services_in_tx` and parks there) no longer holds"
    );
}

/// **The stale-read window, closed** (SP4b-1 review, I-3). The old row is read
/// without `FOR UPDATE` and before the profile locks exist, because taking a
/// `perm.grants` row lock first is the deadlock `lock_services_in_tx`
/// documents. So two overlapping PATCHes of one grant can both read the same
/// old catalog, and the second one's union then misses the profile the first
/// moved the grant to — that profile's compiled policy never gets swept again.
///
/// Driven deterministically rather than by racing: a separate session holds the
/// fixture profile's row lock, so the PATCH gets past its pre-lock read and then
/// blocks inside `lock_services_in_tx`. While it is parked there the grant is
/// moved to a THIRD catalog behind its back, and the lock is released. The
/// re-read under the locks must see that and refuse with a 409 rather than
/// writing over it. Waiting is polled on `pg_locks` against the holder's own
/// pid, not slept on, so nothing here depends on timing.
#[tokio::test]
async fn a_patch_whose_row_moved_while_it_waited_is_a_409() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (service_b, catalog_b) = extra_profile(&f, "race-b").await;
    let (service_c, catalog_c) = extra_profile(&f, "race-c").await;
    // **After the helpers above, never before** — they take this same lock.
    // Held because a tag-rule fan-out in another test locks EVERY
    // compiler-targeted profile, this fixture's included: it would then be the
    // backend `wait_until_parked_on` sees, the grant would be moved before this
    // test's PATCH had done its pre-lock read, and the PATCH would read the
    // already-moved row and pass for the wrong reason. Observed once as a flake.
    let _fanout = exclusive(&f).await;
    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(f.compiled_policy_names().await, vec![name.clone()]);

    let (mut holder, holder_pid) = hold_profile_lock(&f, f.service_id).await;

    // Fire the patch: catalog_id → catalog_b, so its union is
    // {fixture profile, service_b} and it blocks on the fixture profile.
    let app = f.app();
    let patch_token = token.clone();
    let uri = format!("/perm/grants/{grant_id}");
    let body = grant_body(catalog_b, "analyst");
    let patching = tokio::spawn(async move {
        app.oneshot(req("PATCH", &uri, &patch_token, Some(body)))
            .await
            .unwrap()
    });

    wait_until_parked_on(&f, holder_pid).await;

    // Behind its back: move the grant to a catalog whose profile the parked
    // request did NOT lock. A plain UPDATE, so no recompile and no lock of its
    // own — this stands in for the concurrent PATCH that committed first.
    sqlx::query("UPDATE perm.grants SET catalog_id = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&grant_id).unwrap())
        .bind(catalog_c)
        .execute(&f.db.pool)
        .await
        .expect("move the grant under the parked request");
    sqlx::query("COMMIT")
        .execute(&mut holder)
        .await
        .expect("release the profile lock");

    let (status, got) = into_json(patching.await.expect("patch task")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the row moved out from under this request, so its union no longer covers the \
         profiles the grant affects — writing anyway would leave profile {service_c}'s \
         compiled policy behind forever; body: {got}. (If `services` in that body \
         names {service_c}, this test's PREMISE broke rather than the guard: the PATCH \
         read the row after it had already moved, so there was never a stale read to \
         catch.)"
    );
    let catalog_now: Uuid = sqlx::query_scalar("SELECT catalog_id FROM perm.grants WHERE id = $1")
        .bind(Uuid::parse_str(&grant_id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("read catalog_id");
    assert_eq!(
        catalog_now, catalog_c,
        "and the refused patch must not have overwritten the change that won"
    );
    assert_eq!(
        f.compiled_policy_names_under(service_b).await,
        Vec::<String>::new(),
        "nor compiled anything under the profile it was moving to"
    );
}

/// The rule half of the same window — `write_rule` has its own re-read, and an
/// untested guard is how the gate in I-1 shipped. A rule's version of "moved
/// outside the locked set" is a concurrent patch repointing its path selector at
/// a catalog only another profile reaches.
#[tokio::test]
async fn a_rule_patch_whose_row_moved_while_it_waited_is_a_409() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (_, catalog_c) = extra_profile(&f, "race-rule-c").await;
    // Same reason as the grant twin's — see its comment.
    let _fanout = exclusive(&f).await;
    let rule_name = unique("race-rule");
    let path_rule = |catalog: Uuid, mask: &str| {
        json!({
            "name": rule_name,
            "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
            "selector": { "type": "path", "catalog_id": catalog,
                          "schema": "sales", "table": "orders", "column": "email" },
            "exempt": { "include": [], "exclude": [] }
        })
    };
    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(path_rule(f.catalog_id, "MASK_HASH")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let rule_id = created["id"].as_str().expect("minted rule id").to_string();

    let (mut holder, holder_pid) = hold_profile_lock(&f, f.service_id).await;
    let app = f.app();
    let patch_token = token.clone();
    let uri = format!("/perm/rules/{rule_id}");
    let body = path_rule(f.catalog_id, "MASK_NULL");
    let patching = tokio::spawn(async move {
        app.oneshot(req("PATCH", &uri, &patch_token, Some(body)))
            .await
            .unwrap()
    });
    wait_until_parked_on(&f, holder_pid).await;

    sqlx::query("UPDATE perm.rules SET catalog_id = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&rule_id).unwrap())
        .bind(catalog_c)
        .execute(&f.db.pool)
        .await
        .expect("move the rule under the parked request");
    sqlx::query("COMMIT")
        .execute(&mut holder)
        .await
        .expect("release the profile lock");

    let (status, got) = into_json(patching.await.expect("patch task")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the rule moved out from under this request; body: {got}"
    );
    let catalog_now: Option<Uuid> =
        sqlx::query_scalar("SELECT catalog_id FROM perm.rules WHERE id = $1")
            .bind(Uuid::parse_str(&rule_id).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .expect("read catalog_id");
    assert_eq!(
        catalog_now,
        Some(catalog_c),
        "and the refused patch must not have overwritten the change that won"
    );
}

// ---------------------------------------------------------------------------
// preview: the dry run that cannot disagree with the write (SP4b D2)
// ---------------------------------------------------------------------------

/// A Trino profile with an engine bound to it and no catalogs at all — enough
/// to be compiler-targeted, which is all a tag rule's fan-out needs. Holds the
/// universe lock while it mints one; see [`TAG_FANOUT_LOCK`].
async fn extra_trino_profile(f: &Fixture, tag: &str) -> i64 {
    let _universe = advisory_lock(&f.dsn).await;
    egret_policy::seed_trino(&f.policy)
        .await
        .expect("seed trino service-def");
    let service = f
        .policy
        .create_service(&unique(tag), "trino", &json!({}))
        .await
        .expect("create trino service");
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{\"imageVersion\":\"483\"}'::jsonb,$2)",
    )
    .bind(unique(tag))
    .bind(service.id)
    .execute(&f.db.pool)
    .await
    .expect("engine");
    service.id
}

/// The profile ids a preview reported, ascending.
fn previewed_ids(body: &Value) -> Vec<i64> {
    let mut ids: Vec<i64> = body["affected"]
        .as_array()
        .unwrap_or_else(|| panic!("affected in {body}"))
        .iter()
        .map(|p| p["id"].as_i64().expect("affected[].id"))
        .collect();
    ids.sort_unstable();
    ids
}

/// The profile ids a write reported recompiling, ascending.
fn written_ids(services: &Value) -> Vec<i64> {
    let mut ids: Vec<i64> = services
        .as_array()
        .unwrap_or_else(|| panic!("services in {services}"))
        .iter()
        .map(|v| v.as_i64().expect("services[]"))
        .collect();
    ids.sort_unstable();
    ids
}

/// The preview names the profile the grant would reach, WITH its name (a
/// steward reads "Trino-prod", not a bare id) — and stores nothing: no policy,
/// no `perm` row, no `policy_version` bump.
#[tokio::test]
async fn a_preview_of_a_grant_reports_its_profile_and_stores_nothing() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let grants_before = f.grant_count().await;
    let version_before = f.policy_version().await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "grant": grant_body(f.catalog_id, "analyst") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        previewed_ids(&body),
        vec![f.service_id],
        "the grant names this fixture's catalog, which exactly one profile reaches; body: {body}"
    );
    let named = body["affected"][0]["name"]
        .as_str()
        .unwrap_or_else(|| panic!("affected[0].name in {body}"));
    assert!(
        !named.is_empty(),
        "the profile must be NAMED — the reason this endpoint returns more than ids; body: {body}"
    );
    assert_eq!(
        body["compile"]["created"], 1,
        "and it must report what the write would have written; body: {body}"
    );

    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "a preview must not leave a compiled policy behind"
    );
    assert_eq!(
        f.grant_count().await,
        grants_before,
        "a preview must not store the grant"
    );
    assert_eq!(
        f.policy_version().await,
        version_before,
        "a preview must not bump the profile's policy_version — an engine would poll \
         for a policy set that does not exist"
    );
}

/// The compile error the write would have returned, verbatim, and still nothing
/// stored. This is why the preview runs the real compiler: the refusal a steward
/// needs to see is produced by the same code that would refuse the write.
#[tokio::test]
async fn a_preview_of_an_uncompilable_grant_returns_the_error_and_stores_nothing() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let grants_before = f.grant_count().await;
    let version_before = f.policy_version().await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                // Ranger matches bare usernames, so an email can never compile.
                Some(json!({ "grant": grant_body(f.catalog_id, "analyst@egret.local") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("analyst@egret.local"),
        "the preview must return the write's own refusal, which names the offending \
         value; got {msg:?}"
    );
    assert_eq!(f.grant_count().await, grants_before);
    assert_eq!(f.policy_version().await, version_before);
    assert_eq!(f.compiled_policy_names().await, Vec::<String>::new());
}

/// **The test that protects the "cannot disagree" property.** The preview is
/// implemented as the REAL write rolled back precisely so no second,
/// read-only implementation of "which profiles does this touch" can drift from
/// the first — and without this assertion that design has no guard. A tag rule
/// is the case that matters: its affected set is the whole installation, so a
/// divergence would be wide and would be trusted.
///
/// Takes the universe lock for its whole body, so the two sets are taken over
/// the same set of compiler-targeted profiles; every helper that mints one
/// holds the same lock (see `TAG_FANOUT_LOCK`).
#[tokio::test]
async fn preview_and_write_agree_about_a_tag_rules_fan_out() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let rule = json!({
        "name": unique("agree"),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "tag", "key": f.tag_key },
        "exempt": { "include": [], "exclude": [] }
    });

    let (status, previewed) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "rule": rule })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {previewed}");

    // The SAME body, now for real — the preview rolled its own attempt back, so
    // the rule's UNIQUE name is free.
    let (status, written) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", &token, Some(rule)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {written}");

    assert_eq!(
        previewed_ids(&previewed),
        written_ids(&written["compile"]["services"]),
        "THE PREVIEW AND THE WRITE MUST AGREE. A preview that lies is worse than no \
         preview, because the steward would trust it — preview {previewed}, write {written}"
    );
    assert!(
        previewed_ids(&previewed).contains(&f.service_id),
        "and the agreed set must be a real one, containing this fixture's own \
         profile; preview {previewed}"
    );
    assert_eq!(
        previewed["compile"]["created"], written["compile"]["created"],
        "the preview's own compile summary must match the write's too; \
         preview {previewed}, write {written}"
    );
}

/// The installation-wide fan-out is the reason this endpoint exists, so it is
/// asserted rather than assumed: with two more Trino profiles in the database,
/// a tag rule's preview must name them. Scoped to the two ids this test minted
/// — a length assertion over the shared database would be racy.
#[tokio::test]
async fn a_preview_of_a_tag_rule_reports_every_profile_it_would_reach() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let trino_a = extra_trino_profile(&f, "preview-trino-a").await;
    let trino_b = extra_trino_profile(&f, "preview-trino-b").await;
    // A tag-rule preview LOCKS every compiler-targeted profile before it rolls
    // back, so it belongs under the same lock as the writes that fan out — it
    // can otherwise be found parked on another test's profile lock.
    let _lock = exclusive(&f).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "rule": {
                    "name": unique("fanout"),
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                }})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ids = previewed_ids(&body);
    for expected in [trino_a, trino_b, f.service_id] {
        assert!(
            ids.contains(&expected),
            "a tag rule is catalog-unscoped, so its preview must name profile \
             {expected} too — that blast radius is the whole point of this endpoint; \
             body: {body}"
        );
    }
    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "and it must still have stored nothing"
    );
}

/// Previewing a PATCH reports the UNION the patch would recompile — including
/// the profile the grant would be moved AWAY from, which is the profile whose
/// policy gets reaped and therefore the one a steward most needs named.
#[tokio::test]
async fn a_preview_of_a_patch_reports_the_old_profile_too() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (other_service, other_catalog) = extra_profile(&f, "preview-patch-move").await;
    let grant_id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "id": grant_id, "grant": grant_body(other_catalog, "analyst") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let mut expected = vec![f.service_id, other_service];
    expected.sort_unstable();
    assert_eq!(
        previewed_ids(&body),
        expected,
        "both the profile the grant leaves and the one it arrives at; body: {body}"
    );
    assert_eq!(body["compile"]["deleted"], 1, "body: {body}");
    assert_eq!(body["compile"]["created"], 1, "body: {body}");

    assert_eq!(
        f.compiled_policy_names().await,
        vec![name],
        "and the grant must still be compiled where it was — a preview writes nothing"
    );
    assert_eq!(
        f.compiled_policy_names_under(other_service).await,
        Vec::<String>::new()
    );
}

/// Previewing a patch of an id that does not exist is the write's own 404, and
/// a body naming neither a grant nor a rule is a 400 rather than a silent
/// "nothing would happen".
#[tokio::test]
async fn a_preview_of_nothing_or_of_an_unknown_id_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "id": Uuid::new_v4(), "grant": grant_body(f.catalog_id, "analyst") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    for payload in [
        json!({}),
        json!({
            "grant": grant_body(f.catalog_id, "analyst"),
            "rule": {
                "name": unique("both"),
                "kind": { "type": "row_filter", "filter_expr": "1=1" },
                "selector": { "type": "tag", "key": f.tag_key },
                "exempt": { "include": [], "exclude": [] }
            }
        }),
    ] {
        let (status, body) = into_json(
            f.app()
                .oneshot(req("POST", "/perm/preview", &token, Some(payload)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    }
}

// ---------------------------------------------------------------------------
// explain
// ---------------------------------------------------------------------------

/// The subject carries no `roles` — SP4c Task 7 removed the field from the
/// wire, and `explain` resolves the subject's roles server-side. See
/// `explain_ignores_a_caller_supplied_role_and_resolves_the_subjects_own`.
/// A `read` explain for `sales.orders` in `catalog_id`.
///
/// **`subject` carries only `user`** (SP4d-2a Task 5): `ExplainSubject` has no
/// `workspaces`, `roles` or `groups` field left, because none of the three can
/// change what any engine egret provisions decides. Tests that want to prove a
/// removed field is not honoured set the key on the returned value anyway —
/// nothing here sets `deny_unknown_fields`, so it is accepted and ignored,
/// which is the behaviour an older client sees.
fn explain_body(catalog_id: Uuid, user: &str) -> Value {
    json!({
        "subject": { "user": user },
        "action": "read",
        "target": { "catalog_id": catalog_id, "schema": "sales", "table": "orders", "column": null }
    })
}

/// The brief's test, and the package's whole point: `explain` must name the
/// reason CLASS, not just a verdict.
///
/// **Rewritten for SP4d-2a Task 5; the principle is unchanged and the
/// mechanism is not.** The class is no longer the model's `no_grant_matched`
/// but Ranger's own `NoPolicyMatched` — the case whose Access audit shows `-`,
/// which is exactly why naming it is the deliverable. A response carrying only
/// a boolean would reproduce the blindness this endpoint exists to remove.
#[tokio::test]
async fn explain_names_the_reason_class_not_just_a_verdict() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(verdict["decision"]["kind"], "denied", "body: {body}");
    assert_eq!(
        verdict["decision"]["reason"], "no_policy_matched",
        "Ranger's audit shows '-' for exactly this case; naming it is the point; body: {body}"
    );
}

/// An external engine declares the Ranger plugin version it runs. A version the
/// evaluator has not been read against is a refusal that NAMES it — never an
/// evaluation at a guessed one (SP4d-4 Task 7).
#[tokio::test]
async fn explain_refuses_an_external_engine_declaring_an_unmeasured_plugin_version() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let ask = || explain_body(f.catalog_id, "nobody");
    let declare = |version: &'static str| {
        let (pool, service) = (f.db.pool.clone(), f.service_id);
        async move {
            sqlx::query(
                "UPDATE engine_mgmt.engines \
                    SET origin = 'external', external_host = 'sr.example.com', \
                        external_port = 9030, ranger_plugin_version = $2, namespace = '', \
                        spec = '{}'::jsonb \
                  WHERE authz_service_id = $1",
            )
            .bind(service)
            .bind(version)
            .execute(&pool)
            .await
            .expect("declare the bound engine external")
            .rows_affected()
        }
    };

    assert_eq!(declare("2.6.0").await, 1);
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains("2.6.0"),
        "the refusal must name the declared version: {body}"
    );

    // The pair, not the version: StarRocks declaring 2.5.0 (the version only
    // Trino is modelled at) is refused too, and the sentence names the pair and
    // what is modelled (the migration gate refuses the other off-diagonal pair,
    // Trino@2.9.0, with the same derivation: `migrate_route.rs`).
    assert_eq!(declare("2.5.0").await, 1);
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("2.5.0") && error.contains("StarRocks@2.8.0"),
        "the refusal must name the pair: {body}"
    );

    // Accept cell: the same engine declaring the version StarRocks ships.
    assert_eq!(declare("2.8.0").await, 1);
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
}

/// Set the image of the provisioned StarRocks engine bound to `f.service_id`.
async fn pin_bound_starrocks_image(f: &Fixture, fe_image: &str) {
    let res = sqlx::query(
        "UPDATE engine_mgmt.engines \
            SET spec = jsonb_build_object('starrocksConfig', \
                                          jsonb_build_object('feImage', $2::text)) \
          WHERE authz_service_id = $1",
    )
    .bind(f.service_id)
    .bind(fe_image)
    .execute(&f.db.pool)
    .await
    .expect("pin the bound engine's image");
    assert_eq!(res.rows_affected(), 1);
}

async fn explain_status(f: &Fixture, token: &str) -> (StatusCode, Value) {
    into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                token,
                Some(explain_body(f.catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await
}

/// A provisioned engine's plugin comes from its EXACT image, not its type: the
/// same StarRocks type on a floating tag is refused with the fix, and on 4.1.4
/// (plugin 2.8.0) and on 4.1.6 (plugin 2.9.0, proved against the real jar in
/// engine-upgrade Task 4) is explained.
#[tokio::test]
async fn explain_resolves_a_provisioned_engines_plugin_through_its_exact_image() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    pin_bound_starrocks_image(&f, "starrocks/fe-ubuntu:4.1-latest").await;
    let (status, body) = explain_status(&f, &token).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("4.1-latest") && error.contains("pin"),
        "the refusal must name the image and the fix: {body}"
    );

    pin_bound_starrocks_image(&f, "starrocks/fe-ubuntu:4.1.6").await;
    let (status, body) = explain_status(&f, &token).await;
    assert_eq!(status, StatusCode::OK, "4.1.6 runs Ranger 2.9.0: {body}");

    pin_bound_starrocks_image(&f, "starrocks/fe-ubuntu:4.1.4").await;
    let (status, body) = explain_status(&f, &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
}

/// Two engines on one profile that resolve to different plugins cannot share
/// one verdict. The fixture's bound engine runs 2.8.0; a second one on 4.1.6
/// runs 2.9.0. Both are modelled, so the refusal is the disagreement itself.
#[tokio::test]
async fn a_profile_whose_engines_run_different_plugins_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret', \
                 '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.6\"}}'::jsonb,$2)",
    )
    .bind(unique("perm-routes-eng2"))
    .bind(f.service_id)
    .execute(&f.db.pool)
    .await
    .expect("second engine");
    let (status, body) = explain_status(&f, &token).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("different Ranger plugin versions"),
        "body: {body}"
    );
}

/// A policy that DENIED and nothing matching at all are different facts, and
/// must stay different on the wire. A response that collapsed both into a
/// boolean would pass the test above and still be useless.
///
/// **Rewritten for SP4d-2a Task 5.** Ranger keeps the same distinction the
/// model did — `DenyReason::DenyPolicyItem` against
/// `DenyReason::NoPolicyMatched` — so the rule survives the mechanism. This
/// test also asserted that the reason names WHICH grant, which makes it the
/// acceptance test for Task 3's name -> model-object mapping: the artifact
/// names a Ranger policy, and `attribution` has to turn that back into the
/// `perm` grant the operator wrote.
#[tokio::test]
async fn explain_distinguishes_an_explicit_deny_from_nothing_matching() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let mut deny = grant_body(f.catalog_id, "analyst");
    deny["effect"] = json!("deny");
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/grants", &token, Some(deny)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let grant_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(verdict["decision"]["kind"], "denied", "body: {body}");
    assert_eq!(
        verdict["decision"]["reason"], "deny_policy_item",
        "a deny item matched — NOT the same fact as nothing matching; body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["kind"], "grant",
        "and the verdict must map the Ranger policy name back to a `perm` grant; body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["grant_id"], grant_id,
        "which grant denied it; body: {body}"
    );

    // The other half of the distinction, against the same artifact: a subject
    // no policy mentions gets `no_policy_matched`, not this deny.
    let (status, other) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, &unique("unmentioned"))),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {other}");
    assert_eq!(
        access_for(profile_for(&other, "starrocks"), "select")["decision"]["reason"],
        "no_policy_matched",
        "body: {other}"
    );
}

/// `explain` is read-only. Asserted against a scoped population (grants naming
/// this fixture's catalog, and the profile's own `policy_version`) rather than
/// a whole-table count, which the shared test database makes racy.
#[tokio::test]
async fn explain_writes_nothing() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let grants_before = f.grant_count().await;
    let version_before = f.policy_version().await;

    for _ in 0..2 {
        let (status, _) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/explain",
                    &token,
                    Some(explain_body(f.catalog_id, "nobody")),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    assert_eq!(
        f.grant_count().await,
        grants_before,
        "explain wrote a grant"
    );
    assert_eq!(
        f.policy_version().await,
        version_before,
        "explain bumped the profile's policy_version — it compiled something"
    );
    assert_eq!(f.compiled_policy_names().await, Vec::<String>::new());
}

/// **A group grant reaches its members, and `explain` agrees with the engine
/// about both sides of it (J-F3 Task 1b, design R8).**
///
/// History, because this test inverted twice. Through SP4d-2a Task 5 the
/// compiler put a group into the Ranger `groups` bucket, which no engine egret
/// provisions resolves (D7: no usersync, no group provider) — so the model
/// allowed a member and every engine refused them, and this test pinned the
/// artifact's honest answer: a REFUSAL. R8 removed the divergence on the
/// compiler's side instead, exactly as SP4c did for roles: a non-`public` group
/// compiles to its members' stored usernames. So the same request is now an
/// ALLOW for a member and a refusal for a non-member, and the group name
/// appears nowhere in what the engine downloads.
///
/// The compiled body is read as ground truth first, so the verdicts below are
/// about what the engine can MATCH and not about a policy that was or was not
/// written. `explain` still receives no group for the subject (`ExplainSubject`
/// has no `groups` field), which is now exactly right: the engine does not
/// either, and the member is matched by username.
#[tokio::test]
async fn a_group_grant_reaches_its_members_in_the_artifact_and_in_explain() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let group = unique("contractors");
    let group_id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&group)
        .fetch_one(&f.db.pool)
        .await
        .expect("insert group");
    let member = known_user(&f, unique("groupmember")).await;
    let outsider = known_user(&f, unique("groupoutsider")).await;
    sqlx::query(
        "INSERT INTO user_groups (user_id, group_id) \
         SELECT id, $2 FROM users WHERE username = $1",
    )
    .bind(&member)
    .bind(group_id)
    .execute(&f.db.pool)
    .await
    .expect("membership");

    let mut grant = grant_body(f.catalog_id, "unused");
    grant["principals"] = json!({
        "include": [{ "kind": "group", "ref": group }],
        "exclude": []
    });
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/grants", &token, Some(grant)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let grant_id = body["id"].as_str().expect("grant id").to_string();

    // Ground truth: the policy the engine downloads names the MEMBER, by
    // username, and the group nowhere.
    let compiled = f
        .compiled_policy_body(&format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
        .await;
    assert_eq!(
        compiled["policyItems"][0]["users"],
        json!([member]),
        "a group compiles to its members' usernames: {compiled:#}"
    );
    assert_eq!(
        compiled["policyItems"][0]["groups"],
        json!([]),
        "and names no group — no engine egret provisions resolves one: {compiled:#}"
    );
    assert!(
        !compiled.to_string().contains(&group),
        "the group name appears nowhere in the artifact: {compiled:#}"
    );

    for (subject, kind) in [(&member, "allowed"), (&outsider, "denied")] {
        let (status, body) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/explain",
                    &token,
                    Some(explain_body(f.catalog_id, subject)),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        let verdict = access_for(profile_for(&body, "starrocks"), "select");
        assert_eq!(
            verdict["decision"]["kind"], kind,
            "{subject}: explain must answer what the engine enforces; body: {body}"
        );
    }
}

/// **J-F3 Task 1b, fix round 1 — Scenario 2: a grant on a group written while
/// a member is being removed.** No grant names group `G` yet. An admin's
/// `remove_member(G, U)` is parked right after computing its affected set — EMPTY,
/// because nothing names `G`. A steward then creates "allow `G`".
///
/// Without a per-group lock the grant compiled while `U` was still a member
/// (the remove had not committed), committed, and the remove then committed
/// with no recompile because its affected set was empty: `U` stays granted by a
/// group they have left — fail-OPEN. With the lock, `write_grant` takes `G`'s
/// lock before its profile locks, waits for the remove, and compiles a group of
/// nobody.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_on_a_group_written_during_a_member_remove_does_not_name_the_removed_member() {
    use pausing::{Pausing, Stage};
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let group = unique("race-grant");
    let group_id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&group)
        .fetch_one(&f.db.pool)
        .await
        .expect("insert group");
    let member = known_user(&f, unique("raceleaver")).await;
    let member_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(&member)
        .fetch_one(&f.db.pool)
        .await
        .expect("member id");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(member_id)
        .bind(group_id)
        .execute(&f.db.pool)
        .await
        .expect("membership");

    let (pausing, h) = Pausing::new(Stage::AfterAffected);
    let pausing = Arc::new(pausing);
    let remove = {
        let groups = egret_auth::groups::GroupRepo::new(f.db.clone());
        let pausing = pausing.clone();
        tokio::spawn(async move {
            groups
                .remove_member(group_id, egret_core::UserId(member_id), &*pausing)
                .await
        })
    };
    h.reached
        .await
        .expect("remove parked after its (empty) affected set");

    let mut body = grant_body(f.catalog_id, "unused");
    body["principals"] = json!({ "include": [{ "kind": "group", "ref": group }], "exclude": [] });
    let mut post = {
        let app = f.app();
        let token = token.clone();
        tokio::spawn(async move {
            into_json(
                app.oneshot(req("POST", "/perm/grants", &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        })
    };
    let early = tokio::time::timeout(std::time::Duration::from_millis(800), &mut post)
        .await
        .ok();
    let post_waited = early.is_none();
    let _ = h.go.send(());
    remove.await.expect("join").expect("remove");
    let (status, created) = match early {
        Some(done) => done,
        None => post.await,
    }
    .expect("join");
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let grant_id = created["id"].as_str().expect("grant id").to_string();

    let compiled: Option<Value> = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
    .fetch_optional(&f.db.pool)
    .await
    .expect("read");
    assert!(
        !compiled
            .as_ref()
            .map(|b| b.to_string().contains(&member))
            .unwrap_or(false),
        "{member} left the group before either write committed, so no policy may name them: \
         {compiled:?}"
    );
    assert!(
        post_waited,
        "the grant write must wait on the group's lock while the remove holds it"
    );
}

/// **2026-10-18 design R3 (spec E7): a grant on `G` written while `G` is being
/// deleted, then an IdP login claiming `G`.** The delete of local group `G` is
/// parked right after its reference check — nothing named `G` yet, so it is
/// allowed — holding `G`'s lock. A steward then creates "allow `G`".
///
/// Before R3 `write_grant` resolved its refs BEFORE taking the group locks: it
/// saw `G` (the delete had not committed), then waited on `G`'s lock, and once
/// the delete committed it stored a reference to a group that no longer
/// existed — the dangling name an IdP claim or a later group of that name
/// would inherit. With the lock first, the grant waits, sees the delete
/// committed, and is refused ("no group has this name"). The other order (the
/// grant first) is the delete's 409, pinned in `group_grants.rs`.
///
/// The third party: a login whose IdP claims `G` afterwards. Whatever it
/// creates, no stored grant may name `G` without a row of that name, and none
/// may reach the login's user.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_written_during_a_group_delete_never_stores_a_reference_to_the_deleted_group() {
    use pausing::{Pausing, Stage};
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let group = unique("race-delete");
    let group_id: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(&f.db.pool)
            .await
            .expect("insert group");

    let (pausing, h) = Pausing::new(Stage::AfterAffected);
    let pausing = Arc::new(pausing);
    let delete = {
        let groups = egret_auth::groups::GroupRepo::new(f.db.clone());
        let pausing = pausing.clone();
        tokio::spawn(async move {
            groups
                .delete_local(
                    group_id,
                    &*pausing,
                    egret_auth::groups::ReferenceDetail::Entries,
                )
                .await
        })
    };
    h.reached
        .await
        .expect("delete parked after its reference check, holding the group's lock");

    let mut body = grant_body(f.catalog_id, "unused");
    body["principals"] = json!({ "include": [{ "kind": "group", "ref": group }], "exclude": [] });
    let mut post = {
        let app = f.app();
        let token = token.clone();
        tokio::spawn(async move {
            into_json(
                app.oneshot(req("POST", "/perm/grants", &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        })
    };
    let early = tokio::time::timeout(std::time::Duration::from_millis(800), &mut post)
        .await
        .ok();
    let post_waited = early.is_none();
    let _ = h.go.send(());
    assert_eq!(delete.await.expect("join").expect("delete"), 1);
    let (status, created) = match early {
        Some(done) => done,
        None => post.await,
    }
    .expect("join");

    let dangling: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.grant_principals p \
          WHERE p.kind = 'group' AND p.ref = $1 \
            AND NOT EXISTS (SELECT 1 FROM groups g WHERE g.name = p.ref)",
    )
    .bind(&group)
    .fetch_one(&f.db.pool)
    .await
    .expect("read");
    assert_eq!(
        dangling, 0,
        "no grant may store a reference to the deleted group (grant answered {status}: {created})"
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {created}");
    assert!(
        created.to_string().contains("no group has this name"),
        "the grant is refused by validation: {created}"
    );
    assert!(
        post_waited,
        "the grant write must wait on the group's lock while the delete holds it"
    );

    // The third party: an IdP login claiming the freed name.
    let tag = unique("race-idp").replace('_', "-");
    let login_id = egret_auth::oidc::provision_and_sync(
        &f.users,
        &egret_auth::groups::GroupRepo::new(f.db.clone()),
        &egret_auth::oidc::OidcClaims {
            subject: format!("sub-{tag}"),
            email: format!("{tag}@idp.local"),
            email_verified: true,
            name: "IdP".into(),
            groups: vec![group.clone()],
        },
        &egret_perm_compile::PermGroupRecompiler,
    )
    .await
    .expect("login");
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(login_id.0)
        .fetch_one(&f.db.pool)
        .await
        .expect("username");
    if let Some(username) = username {
        let reached: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM policy.authz_policies \
              WHERE service_id = $1 AND policy_jsonb::text LIKE '%' || $2 || '%'",
        )
        .bind(f.service_id)
        .bind(&username)
        .fetch_one(&f.db.pool)
        .await
        .expect("read");
        assert_eq!(
            reached, 0,
            "no policy may reach the IdP's user through `{group}`"
        );
    }
}

/// How many transactions wait on the per-group membership lock of `group` —
/// the queue a lock-ordering test builds one entry at a time.
async fn group_lock_waiters(f: &Fixture, group: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks \
          WHERE locktype = 'advisory' AND NOT granted AND objsubid = 2 \
            AND classid::bigint = $1 \
            AND objid::bigint = (hashtext($2)::bigint & 4294967295)",
    )
    .bind(i64::from(egret_auth::groups::MEMBERSHIP_GROUP_LOCK_SPACE))
    .bind(group)
    .fetch_one(&f.db.pool)
    .await
    .expect("pg_locks")
}

async fn until_group_lock_waiters(f: &Fixture, group: &str, n: i64) {
    for _ in 0..200 {
        if group_lock_waiters(f, group).await >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("{n} writers never queued on {group:?}'s lock");
}

/// **Fix round 1, I-1: a PATCH must not keep a group ref on a stale read.** A
/// patch skips re-checking a ref the stored row already holds (design R6), and
/// it read that row BEFORE taking the group locks. Interleaving, made
/// deterministic by the advisory lock's FIFO queue behind a lock the test
/// holds on `G`:
///
/// 1. PATCH1 (drop `G` from grant `X`) queues on `G`;
/// 2. DELETE `G` queues behind it;
/// 3. PATCH2 (an edit that keeps `G`) reads `X` — still naming `G`, PATCH1 has
///    not committed — and queues last.
///
/// Released: PATCH1 commits `X` without `G`; the delete then finds nothing
/// naming `G` and deletes it; PATCH2 gets the lock. Before the fix it skipped
/// `G` as "already stored" on its stale copy and stored `X` naming a group
/// that no longer exists — the E7 outcome R3 closes for creates. With the
/// kept set re-read under the lock, `G` is checked, and the patch is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_patch_queued_behind_a_group_delete_does_not_keep_the_deleted_group() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let group = unique("race-patch");
    let group_id: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(&f.db.pool)
            .await
            .expect("insert group");
    let someone = known_user(&f, unique("racepatch")).await;
    let with_group = {
        let mut b = grant_body(f.catalog_id, &someone);
        b["principals"]["include"] = json!([
            { "kind": "group", "ref": group },
            { "kind": "user", "ref": someone }
        ]);
        b
    };
    let without_group = grant_body(f.catalog_id, &someone);
    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(with_group.clone()),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let grant_id = created["id"].as_str().expect("id").to_string();

    // Hold `G`'s lock so the three writers queue in a known order.
    let mut holder = f.db.pool.begin().await.expect("begin");
    egret_auth::groups::lock_group_names_via(&mut holder, std::slice::from_ref(&group))
        .await
        .expect("hold the group's lock");

    let patch = |body: Value| {
        let app = f.app();
        let token = token.clone();
        let uri = format!("/perm/grants/{grant_id}");
        tokio::spawn(async move {
            into_json(
                app.oneshot(req("PATCH", &uri, &token, Some(body)))
                    .await
                    .unwrap(),
            )
            .await
        })
    };
    let patch1 = patch(without_group);
    until_group_lock_waiters(&f, &group, 1).await;
    let delete = {
        let groups = egret_auth::groups::GroupRepo::new(f.db.clone());
        tokio::spawn(async move {
            groups
                .delete_local(
                    group_id,
                    &egret_perm_compile::PermGroupRecompiler,
                    egret_auth::groups::ReferenceDetail::Entries,
                )
                .await
        })
    };
    until_group_lock_waiters(&f, &group, 2).await;
    let patch2 = patch(with_group);
    until_group_lock_waiters(&f, &group, 3).await;
    holder.commit().await.expect("release");

    let (s1, b1) = patch1.await.expect("join");
    assert!(s1.is_success(), "PATCH1 drops the group: {s1} {b1}");
    assert_eq!(
        delete
            .await
            .expect("join")
            .expect("the delete finds nothing naming G"),
        1
    );
    let (s2, b2) = patch2.await.expect("join");

    let dangling: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.grant_principals p \
          WHERE p.kind = 'group' AND p.ref = $1 \
            AND NOT EXISTS (SELECT 1 FROM groups g WHERE g.name = p.ref)",
    )
    .bind(&group)
    .fetch_one(&f.db.pool)
    .await
    .expect("read");
    assert_eq!(
        dangling, 0,
        "no grant may name the deleted group (PATCH2 answered {s2}: {b2})"
    );
    assert_eq!(s2, StatusCode::BAD_REQUEST, "{b2}");
    assert!(b2.to_string().contains("no group has this name"), "{b2}");
}

/// `explain` resolves the target's tags itself — it never made the caller
/// supply them, because that would answer a question the caller had already
/// decided.
///
/// # ⚠ SP4d-2b Task 8 (F4/F5) changed what this field IS, because the browser
/// measured the old one lying
///
/// Through SP4d-2a Task 5 and SP4d-2b Task 7 this was `tag_keys_for_resource_via`
/// — `tags.key`, matched on `(schema, table, column)` by exact equality — while
/// the evaluator's set came from `resolved_tags_for_target`, which rolls the table
/// level up and returns `tags.name`. The argument for keeping them apart was that
/// they answer different questions. On a real Trino that argument failed twice in
/// one run: a request denied **by a tag policy through a tag attached to the
/// table above the column** rendered the deciding policy's name directly above
/// "Tags egret has recorded on this resource — None.", and a target reported
/// `Allowed` with "None." was one no query could read at all.
///
/// The field is now the evaluator's own set, read back out of it. This test
/// therefore asserts the NAME (not the key), and that `attached_at_column` tracks
/// the coordinate each assignment actually used — the half of the answer that
/// tells a steward which row to change.
///
/// # ⚠ SP4d debt Task 7 rebuilt the FIXTURE, because the assertion it replaced
/// could not have passed and asserting `[]` instead would have been worse
///
/// This test was ADDED by #611, the same squashed commit (`f5e2e84d`) in which the
/// H3 fix moved the set off the response and onto each profile, so it read
/// `body["tags"]` — `Null` — and had never run: it is DB-gated and the shared test
/// template was broken across branches. **So the F4/F5 property had no passing
/// automated assertion on `main` at all**; the three e2e tests that do pass read
/// decisions and decline reasons, and none of them reads `profiles[].tags`.
///
/// Re-pointing it at `profiles[0].tags` and expecting `[]` would have made it green
/// while exercising NOTHING. The old fixture is `setup()`'s single StarRocks
/// profile with no bound tag service, so `tag_policies_in_download` is `0` and
/// [`super`]'s `tags_for_profile` returns empty **by construction** — a population
/// in which neither name-not-key nor `attached_at_column` can be observed, which is
/// the exact shape this branch has been caught by repeatedly. The fixture below can
/// fail on each of the three things the field exists for:
///
/// 1. **The NAME, not the key.** Two tags, each with a key that differs from its
///    name by construction, and the keys are asserted ABSENT from the whole
///    response — so the happy path is not the only thing pinning the choice.
/// 2. **`attached_at_column`, in BOTH directions, from one request.** The column
///    row is `SELF` for this target and the table row is `ANCESTOR`, so a flag
///    written once — either way — pairs one tag with the wrong answer.
/// 3. **H1: non-empty for the profile that resolves it, empty for the one that does
///    not.** Migration 0176's `CHECK` makes a StarRocks profile carrying a
///    `tagService` unrepresentable, so "one profile resolves these tags and one
///    cannot" is the ONLY shape this pair can take — and it is exactly the state
///    whose single response-level list made the page claim, under both profiles,
///    that these tags decide this request.
///
/// It also pins H3 from the other side: there is no response-level `tags` key left
/// to read, so a tag list can only ever appear beside a verdict.
#[tokio::test]
async fn explain_reports_the_tag_set_the_evaluator_was_handed() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, _starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-tagset").await;
    // Two assignments at DIFFERENT coordinates on the same table. The resolver's
    // order is `ORDER BY is_self DESC, t.name`, so the column row comes first
    // whatever the two names sort as — the array below is a literal for that
    // reason, and because a set comparison would also pass with the two
    // `attached_at_column` flags swapped.
    let on_column = assign_tag(&f, catalog_id, "sales", "orders", "email").await;
    let on_table = assign_tag(&f, catalog_id, "sales", "orders", "").await;
    // One tag policy, on the TRINO profile only — so this profile's download
    // carries a tag block and the StarRocks one carries none.
    bind_tag_service_with_one_policy(&f, trino_id, &on_column.name).await;

    let mut ask = explain_body(catalog_id, "analyst");
    ask["target"]["column"] = json!("email");
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // ⚠ **The key-absence check leads deliberately**, so that each of the three
    // regressions this test exists for trips a DIFFERENT assertion: a list built
    // from `tags.key` dies here, a mis-derived `attached_at_column` dies on the
    // literal below, and a set handed to every profile dies on the StarRocks
    // block. Asserted against the whole serialised response rather than against
    // the one field, because "reports the name here" is not the same claim as "the
    // key is not on the wire" — it could also come back in an attribution or a
    // policy name.
    let rendered = body.to_string();
    for key in [&on_column.key, &on_table.key] {
        assert!(
            !rendered.contains(key.as_str()),
            "`tags.key` {key} must appear NOWHERE in the response — it is what \
             this list used to be built from, and live it differs from the name \
             the engine matches on; body: {body}"
        );
    }
    let trino = profile_for(&body, "trino");
    assert_eq!(
        trino["tags"],
        json!([
            { "name": on_column.name, "attached_at_column": true },
            { "name": on_table.name, "attached_at_column": false },
        ]),
        "the profile whose download carries a tag block must report the \
         evaluator's own set, by NAME, with `attached_at_column` tracking each \
         assignment's own coordinate: `sales.orders.email` is SELF of this target \
         and `sales.orders` is its ANCESTOR; body: {body}"
    );
    // The list sits beside verdicts, or it is a claim about nothing (H3).
    assert_eq!(
        access_for(trino, "select")["decision"]["kind"],
        "denied",
        "`analyst` holds no grant on this catalog, so there is a real verdict for \
         the tag list to sit beside; body: {body}"
    );
    assert_eq!(
        trino["tag_policies_in_download"], 1,
        "and the narrowing predicate's value has to be the one that let the list \
         through, or the assertion above is green for the wrong reason; body: \
         {body}"
    );

    // H1. Same catalog, same resolution, same request — and a profile that could
    // not have used it reports an empty list plus the reason it is empty.
    let starrocks = profile_for(&body, "starrocks");
    assert_eq!(
        starrocks["tags"],
        json!([]),
        "no tag policy of any kind is in this profile's download, so no tag can \
         reach any verdict here and saying these tags decide this request would \
         be false; body: {body}"
    );
    assert_eq!(
        starrocks["tag_policies_in_download"], 0,
        "and the page needs the reason the list is empty while the column IS \
         tagged; body: {body}"
    );
    // H3. There is nowhere to put a tag list except beside a verdict.
    assert_eq!(
        body["tags"],
        Value::Null,
        "the response-level `tags` field is gone, and this test read it for a \
         whole release; body: {body}"
    );
}

/// SP4a Task 15, D3 defect 2: `explain` and the compiled policy must agree
/// about an IMPLICIT (`workspace_members`) grant, for a member whose stored
/// `users.username` carries an `ensure_username` collision suffix (`-2`) —
/// the exact case a naive re-derivation from the email would get wrong (it
/// would name the FIRST colliding person for the SECOND). Two users share one
/// email local-part on purpose, so `collide` and `collide-2` are real,
/// minted, distinguishable identities before this test ever calls `explain`.
///
/// **SP4d-2a Task 5 keeps the fixture and the ground-truth read of
/// `policyItems[0].users`, and the surviving rule is the same one:** a
/// collision-suffixed identity is distinct from the bare one, and `explain`
/// must not conflate them. Measured against the artifact it becomes the
/// end-to-end test of Task 3's mapping over a real compiled body — the
/// verdict's Ranger policy name has to come back as this grant's uuid — and a
/// third, non-member identity sharing the same base slug is asked too, so
/// "allowed" cannot be coming from a prefix match.
#[tokio::test]
async fn explain_agrees_with_the_compiled_policy_for_a_collision_suffixed_member() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("explain-collide-ws"))
            .bind("Explain Collide WS")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");

    // Two users whose email local-part slugifies to the SAME base — the
    // second must mint `<base>-2`, never `<base>`, or the two people are
    // indistinguishable to any engine policy.
    let local = unique("collide");
    let user_a = f
        .users
        .insert_local(&format!("{local}@one.example.com"), "Collide A", "x")
        .await
        .expect("insert user a")
        .0;
    let user_b = f
        .users
        .insert_local(&format!("{local}@two.example.com"), "Collide B", "x")
        .await
        .expect("insert user b")
        .0;
    let mut conn = f.db.pool.acquire().await.expect("acquire conn");
    let username_a = egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(user_a))
        .await
        .expect("mint username a");
    let username_b = egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(user_b))
        .await
        .expect("mint username b");
    assert_eq!(
        username_a, local,
        "the first registrant keeps the bare slug"
    );
    assert_eq!(
        username_b,
        format!("{local}-2"),
        "the second registrant must carry the collision suffix, not overwrite the first"
    );

    for uid in [user_a, user_b] {
        let res = sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, r.id, 'workspace', $2 FROM roles r \
              WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
        )
        .bind(uid)
        .bind(ws_id)
        .execute(&f.db.pool)
        .await
        .expect("seed membership");
        assert_eq!(res.rows_affected(), 1, "the membership row must exist");
    }

    // `sync_assignment_grants` COMMITS the implicit grant, and the recompile that
    // materialises its policy is a second transaction. Between the two this
    // profile has desired state with no policy behind it — and a tag-rule
    // recompile reconciles EVERY profile, this one included. So both steps run
    // under the universe lock, like the helpers that mint a profile: see
    // `TAG_FANOUT_LOCK`. Released before `explain` is asked, which changes nothing.
    let grant_id: Uuid = {
        let _universe = exclusive(&f).await;
        egret_perm::sync_assignment_grants(&f.perm, f.catalog_id, &[ws_id])
            .await
            .expect("assign catalog to workspace");
        let grant_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment'",
        )
        .bind(f.catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("implicit grant");
        egret_perm_compile::recompile_service(&f.perm, &f.policy, f.service_id)
            .await
            .expect("recompile the profile");
        grant_id
    };

    // Ground truth: the compiled policy names BOTH usernames.
    let policy_jsonb: Value = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
    .fetch_one(&f.db.pool)
    .await
    .expect("compiled policy for the implicit grant");
    let mut compiled_users: Vec<String> = policy_jsonb["policyItems"][0]["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    compiled_users.sort();
    let mut expected_users = vec![username_a.clone(), username_b.clone()];
    expected_users.sort();
    assert_eq!(
        compiled_users, expected_users,
        "the compiled policy must name both real usernames, suffix included"
    );

    // `explain`, asked about the SUFFIXED user specifically, must agree —
    // the assertion this defect existed to fail. `target` names no schema, so
    // it matches the implicit grant's catalog-wide scope directly.
    let mut body = explain_body(f.catalog_id, &username_b);
    body["target"]["schema"] = Value::Null;
    body["target"]["table"] = Value::Null;
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "allowed",
        "explain must allow the collision-suffixed member the compiled policy \
         already serves; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["policy_name"],
        format!("{COMPILER_GRANT_PREFIX}{grant_id}"),
        "body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["grant_id"],
        grant_id.to_string(),
        "must name the SAME implicit grant the compiler compiled; body: {body}"
    );

    // And the FIRST colliding user (bare slug) must independently agree too —
    // proving explain distinguishes the two rather than matching on the
    // shared base string.
    let mut body_a = explain_body(f.catalog_id, &username_a);
    body_a["target"]["schema"] = Value::Null;
    body_a["target"]["table"] = Value::Null;
    let (status, body_a) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(body_a)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_a}");
    assert_eq!(
        access_for(profile_for(&body_a, "starrocks"), "select")["decision"]["kind"],
        "allowed",
        "body: {body_a}"
    );

    // A THIRD identity slugging from the same base is NOT a member and must be
    // refused: `{local}-3` would be minted by the next registrant, and an
    // `explain` matching on the shared base would allow it.
    let (status, body_c) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(json!({
                    "subject": { "user": format!("{local}-3") },
                    "action": "read",
                    "target": { "catalog_id": f.catalog_id, "schema": null,
                                "table": null, "column": null }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_c}");
    assert_eq!(
        access_for(profile_for(&body_c, "starrocks"), "select")["decision"]["kind"],
        "denied",
        "a username sharing only the base slug is a different person; body: {body_c}"
    );
}

/// **SP4c Task 7, kept by a stronger means (SP4d-2a Task 5).**
///
/// The surviving rule is that a caller may not assert a role the subject does
/// not hold. The SP4c live gate
/// (`docs/en/plans/2026-09-28-sp4c-live-result.md`) measured the old
/// `ExplainSubject::roles` wrong in both directions at once:
/// `{"user":"analyst","roles":["sp4c_cleared"]}` answered `allowed` for the
/// very user both engines refuse, and `{"user":"d3member"}` — the natural call
/// — answered `clearance_required` for the user the engine had just served the
/// classified column to.
///
/// Under D2 the rule is now kept by sending NO roles to the evaluator at all,
/// which is stronger than filtering a caller's list: there is nothing to
/// filter. The clearance itself has no Ranger analogue on the wire, so the
/// gating `tag` and its `required_roles` are reconstructed from the compiled
/// policy NAME (`perm-clearance-{tag_id}-{locator}`) via the `tags` row.
///
/// **This measures the STARROCKS path**, which is the fixture's engine and the
/// one where a clearance is a per-column resource DENY the evaluator handles
/// (D0b: StarRocks has no tag service). On Trino the same clearance lives in
/// the tag service, i.e. inside the `tagPolicies` block, which is tier (c) and
/// therefore `Undecidable` — see
/// `a_tag_policy_block_that_could_decide_makes_the_profile_undecidable`.
///
/// Both directions are asserted against ONE fixture, because a test that only
/// checked the lie is also passed by an `explain` that refuses everybody.
#[tokio::test]
async fn a_clearance_deny_in_the_artifact_refuses_the_uncleared_and_not_the_cleared() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let tag_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&f.tag_key)
        .fetch_one(&f.db.pool)
        .await
        .expect("the fixture's tag");

    let role_name = unique("sp4c-clearing-role");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&f.db.pool)
            .await
            .expect("clearing role");

    let cleared = f
        .users
        .insert_local(
            &format!("{}@example.com", unique("cleared")),
            "Cleared",
            "x",
        )
        .await
        .expect("insert cleared user")
        .0;
    let uncleared = f
        .users
        .insert_local(
            &format!("{}@example.com", unique("uncleared")),
            "Uncleared",
            "x",
        )
        .await
        .expect("insert uncleared user")
        .0;
    let mut conn = f.db.pool.acquire().await.expect("acquire conn");
    let cleared_name =
        egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(cleared))
            .await
            .expect("mint cleared username");
    let uncleared_name =
        egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(uncleared))
            .await
            .expect("mint uncleared username");
    drop(conn);

    // Only ONE of them actually holds the clearing role.
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(cleared)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("role grant");
    assert_eq!(res.rows_affected(), 1, "the role grant row must exist");

    // Gate the fixture's tag on that role, and give BOTH subjects a grant that
    // would allow the read on its own — so the clearance deny is the only thing
    // that can refuse either of them, and "allowed" is a real verdict rather
    // than "nothing matched" wearing a different name.
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}') \
         ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles",
    )
    .bind(tag_id)
    .bind(vec![role_name.clone()])
    .execute(&f.db.pool)
    .await
    .expect("gate the tag");
    for name in [&cleared_name, &uncleared_name] {
        let (status, body) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/grants",
                    &token,
                    Some(grant_body(f.catalog_id, name)),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {body}");
    }

    // 1. The lie. The uncleared subject asserts the clearing role it does not
    //    hold. `roles` is not even a field on `ExplainSubject` any more, so the
    //    key is ignored — and the artifact's clearance deny stands.
    let mut lying = explain_body(f.catalog_id, &uncleared_name);
    lying["target"]["column"] = json!("email");
    lying["subject"]["roles"] = json!([role_name]);
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(lying)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // View governance Task 12 (C2): StarRocks 4.1.6 checks no access on a
    // column it masks, so for this uncleared subject the clearance answers as
    // its `MASK_NULL`, not as the deny.
    let starrocks = profile_for(&body, "starrocks");
    assert_eq!(starrocks["masked_read_skips_access"], true, "body: {body}");
    let verdict = &starrocks["mask"];
    assert_eq!(
        verdict["decision"]["result"]["mask_type"], "MASK_NULL",
        "a caller-supplied role must not clear a subject who does not hold it — this is \
         the live gate's `{{\"user\":\"analyst\",\"roles\":[\"sp4c_cleared\"]}}` answering \
         `allowed` for the user both engines refuse; body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["kind"], "clearance",
        "the compiled policy NAME is where a clearance is recognisable at all; body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["tag_id"],
        tag_id.to_string(),
        "body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["tag"], f.tag_key,
        "the gating tag has to be reconstructed from that name — it is nowhere on \
         the wire the engine sees; body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["required_roles"],
        json!([role_name]),
        "and so do the roles that would clear it; body: {body}"
    );

    // 2. The other direction. The subject who DOES hold the role is cleared
    //    without naming it, because the COMPILER named them in the deny's
    //    `denyExceptions` — which is the only mechanism any engine honours.
    let mut truthful = explain_body(f.catalog_id, &cleared_name);
    truthful["target"]["column"] = json!("email");
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(truthful)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "allowed",
        "the role holder must be allowed WITHOUT naming the role in the request; body: {body}"
    );
    assert_eq!(
        verdict["attribution"]["kind"], "grant",
        "and be allowed by their GRANT, the clearance having excepted them; body: {body}"
    );

    // Lift the gate again. A clearance is installation-wide: left behind, this
    // tag would be compiled into every Trino profile in the shared test
    // database for the rest of the run, perturbing suites that have nothing to
    // do with it. Nothing above depends on this running, so a failure that
    // skips it costs correctness nowhere.
    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(tag_id)
        .execute(&f.db.pool)
        .await
        .expect("lift the gate");
}

/// View governance Task 3. Gates the fixture's tag on a fresh role, mints one
/// reader who holds it and one who does not, and gives both a read grant, so the
/// clearance is the only thing that can tell them apart. Returns
/// `(tag_id, role, cleared, uncleared)`. The caller lifts the gate.
async fn clearance_readers(f: &Fixture, token: &str) -> (Uuid, String, String, String) {
    let tag_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&f.tag_key)
        .fetch_one(&f.db.pool)
        .await
        .expect("the fixture's tag");
    let role_name = unique("viewgov-clearing-role");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&f.db.pool)
            .await
            .expect("clearing role");
    let mut names = Vec::new();
    for label in ["cleared", "uncleared"] {
        let id = f
            .users
            .insert_local(&format!("{}@example.com", unique(label)), label, "x")
            .await
            .expect("insert user")
            .0;
        let mut conn = f.db.pool.acquire().await.expect("acquire conn");
        let name = egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(id))
            .await
            .expect("mint username");
        drop(conn);
        if label == "cleared" {
            sqlx::query(
                "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, \
                 scope_id) VALUES ('user', $1, $2, 'platform', NULL)",
            )
            .bind(id)
            .bind(role_id)
            .execute(&f.db.pool)
            .await
            .expect("role grant");
        }
        names.push(name);
    }
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}') \
         ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles",
    )
    .bind(tag_id)
    .bind(vec![role_name.clone()])
    .execute(&f.db.pool)
    .await
    .expect("gate the tag");
    for name in &names {
        let (status, body) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/grants",
                    token,
                    Some(grant_body(f.catalog_id, name)),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {body}");
    }
    let uncleared = names.pop().unwrap();
    let cleared = names.pop().unwrap();
    (tag_id, role_name, cleared, uncleared)
}

async fn explain_column(f: &Fixture, token: &str, user: &str) -> Value {
    let mut body = explain_body(f.catalog_id, user);
    body["target"]["column"] = json!("email");
    let (status, out) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {out}");
    out
}

async fn lift_gate(f: &Fixture, tag_id: Uuid) {
    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(tag_id)
        .execute(&f.db.pool)
        .await
        .expect("lift the gate");
}

/// View governance Task 12 (live finding C2). StarRocks 4.1.6 asks no access
/// check for a column that a mask transforms (the FE's debug log asked
/// `policyType=0` only for the unmasked column), so an uncleared direct read
/// of a classified column returns `NULL` from the clearance's `MASK_NULL`,
/// not `ERROR 5204` from its deny. Explain mirrors that: no `select` verdict,
/// `masked_read_skips_access`, and the mask attributed to the clearance.
/// Trino checks access on a masked column, so its profile is unchanged.
#[tokio::test]
async fn explain_answers_an_uncleared_starrocks_read_of_a_masked_column_with_the_mask() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, role, _cleared, uncleared) = clearance_readers(&f, &token).await;

    let body = explain_column(&f, &token, &uncleared).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(profile["masked_read_skips_access"], true, "body: {body}");
    assert!(
        !access_types(profile).contains(&"select".to_string()),
        "the engine asks no `select` for a masked column, so explain gives none; body: {body}"
    );

    let mask = &profile["mask"];
    assert_eq!(mask["access_type"], "select", "body: {body}");
    assert_eq!(mask["decision"]["kind"], "masked", "body: {body}");
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_NULL",
        "body: {body}"
    );
    assert_eq!(mask["attribution"]["kind"], "clearance", "body: {body}");
    assert!(
        mask.get("replayed").is_none(),
        "MASK_NULL means not cleared: nothing to replay; body: {body}"
    );
    assert_eq!(
        mask["attribution"]["tag_id"],
        tag_id.to_string(),
        "body: {body}"
    );
    assert_eq!(
        mask["attribution"]["required_roles"],
        json!([role]),
        "body: {body}"
    );
    assert!(
        mask["decision"]["result"]["policy_name"]
            .as_str()
            .is_some_and(|n| n.contains("-mask-")),
        "the mask names the compiled mask policy, not the deny; body: {body}"
    );
    lift_gate(&f, tag_id).await;
}

/// Final review V-1: the same hand-written `{public}` `MASK_NULL` on the same
/// column in both profiles' downloads, for a subject with no grant. StarRocks
/// keys the skip on the deciding mask's TYPE, whichever policy decided it (live
/// §3.4: 2b's hand-written deny was not applied under the clearance `MASK_NULL`,
/// 2c's `MASK_HASH` column was checked), so the StarRocks profile skips the
/// column's `select` verdict. Trino checks access on a masked column, so its
/// profile keeps it.
#[tokio::test]
async fn a_hand_written_mask_null_skips_the_starrocks_access_check_but_not_trinos() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-masked").await;
    let catalog: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name");
    let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
    for (service_id, middle) in [(trino_id, "schema"), (starrocks_id, "database")] {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
             VALUES ($1, 'hand mask', $2, true, 'operator')",
        )
        .bind(service_id)
        .bind(json!({
            "name": "hand mask", "policyType": 1, "isEnabled": true, "policyPriority": 0,
            "resources": {"catalog": lvl(&catalog), middle: lvl("sales"),
                          "table": lvl("orders"), "column": lvl("email")},
            "dataMaskPolicyItems": [{"users": [], "groups": ["public"], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": "MASK_NULL"}}],
            "policyItems": [], "denyPolicyItems": [],
            "allowExceptions": [], "denyExceptions": []
        }))
        .execute(&f.db.pool)
        .await
        .expect("hand mask");
    }
    let mut ask = explain_body(catalog_id, "analyst");
    ask["target"]["column"] = json!("email");
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let starrocks = profile_for(&body, "starrocks");
    assert_eq!(starrocks["masked_read_skips_access"], true, "body: {body}");
    assert!(
        !access_types(starrocks).contains(&"select".to_string()),
        "StarRocks asks no `select` for a MASK_NULL column; body: {body}"
    );
    assert_eq!(
        starrocks["mask"]["decision"]["result"]["mask_type"], "MASK_NULL",
        "body: {body}"
    );
    assert_eq!(
        starrocks["mask"]["decision"]["result"]["policy_name"], "hand mask",
        "a hand-written mask decides here, not the clearance's; body: {body}"
    );
    let trino = profile_for(&body, "trino");
    assert!(
        access_types(trino).contains(&"select".to_string()),
        "Trino checks access on a masked column; body: {body}"
    );
    assert!(
        trino.get("masked_read_skips_access").is_none(),
        "body: {body}"
    );
}

/// A reader with no catalog grant at all, minted after the fixture (fix round 1).
async fn ungranted_reader(f: &Fixture) -> String {
    let id = f
        .users
        .insert_local(
            &format!("{}@example.com", unique("ungranted")),
            "ungranted",
            "x",
        )
        .await
        .expect("insert user")
        .0;
    let mut conn = f.db.pool.acquire().await.expect("acquire conn");
    egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(id))
        .await
        .expect("mint username")
}

/// Final review V-1 (live 2a): a reader with NO grant reads a classified
/// StarRocks column that carries the clearance mask. The engine refused that
/// reader (`5204`), but through ANOTHER column (`id`, or `___count___` for
/// `count(*)`), never through the masked one. So the masked column's own
/// `select` is not consulted: explain skips it and the mask is the answer.
/// Which other column refuses depends on the query, which explain does not see.
#[tokio::test]
async fn an_ungranted_reader_of_a_masked_classified_column_is_not_checked_on_that_column() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, _cleared, _uncleared) = clearance_readers(&f, &token).await;
    let nobody = ungranted_reader(&f).await;

    let body = explain_column(&f, &token, &nobody).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(
        profile["mask"]["decision"]["result"]["mask_type"], "MASK_NULL",
        "the clearance mask applies to this reader too; body: {body}"
    );
    assert_eq!(profile["masked_read_skips_access"], true, "body: {body}");
    assert!(
        !access_types(profile).contains(&"select".to_string()),
        "body: {body}"
    );
    lift_gate(&f, tag_id).await;
}

/// Final review V-1: a hand-written deny on the whole table (`column: *`) for a
/// reader who holds the grant. On the MASK_NULL column StarRocks consults no
/// access policy at all, so this deny is not applied there either: the skip
/// holds. (The query is still refused through any other column it reads.)
#[tokio::test]
async fn a_table_level_deny_does_not_reach_a_mask_null_column_on_starrocks() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    let body = explain_column(&f, &token, &uncleared).await;
    let service_id = profile_for(&body, "starrocks")["service_id"]
        .as_i64()
        .expect("service id");
    let catalog: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(f.catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name");
    let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, 'hand table deny', $2, true, 'operator')",
    )
    .bind(service_id)
    .bind(json!({
        "name": "hand table deny", "policyType": 0, "isEnabled": true, "policyPriority": 0,
        "resources": {"catalog": lvl(&catalog), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("*")},
        "policyItems": [],
        "denyPolicyItems": [{"users": [uncleared.clone()], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]}],
        "allowExceptions": [], "denyExceptions": []
    }))
    .execute(&f.db.pool)
    .await
    .expect("hand table deny");

    let body = explain_column(&f, &token, &uncleared).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(profile["masked_read_skips_access"], true, "body: {body}");
    assert!(
        !access_types(profile).contains(&"select".to_string()),
        "body: {body}"
    );
    lift_gate(&f, tag_id).await;
}

/// Inserts a hand-written column `select` deny for `user` on `sales.orders.email`
/// into the StarRocks profile `service_id` serves.
async fn hand_column_deny(f: &Fixture, service_id: i64, user: &str) {
    let catalog: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(f.catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name");
    let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, 'hand column deny', $2, true, 'operator')",
    )
    .bind(service_id)
    .bind(json!({
        "name": "hand column deny", "policyType": 0, "isEnabled": true, "policyPriority": 0,
        "resources": {"catalog": lvl(&catalog), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("email")},
        "policyItems": [],
        "denyPolicyItems": [{"users": [user], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]}],
        "allowExceptions": [], "denyExceptions": []
    }))
    .execute(&f.db.pool)
    .await
    .expect("hand column deny");
}

/// Final review V-1, live 2b (red on `1fe70ea8`): a hand-written column
/// `select` deny plus the clearance `MASK_NULL`, for a reader who holds the
/// grant. StarRocks 4.1.6 returned `NULL` for the column: the hand-written deny
/// was not applied, because a MASK_NULL column is not access-checked. The old
/// rule kept `select` denied here, which understated the exposure.
#[tokio::test]
async fn a_hand_written_column_deny_under_the_clearance_mask_null_is_skipped_on_starrocks() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    let body = explain_column(&f, &token, &uncleared).await;
    let service_id = profile_for(&body, "starrocks")["service_id"]
        .as_i64()
        .expect("service id");
    hand_column_deny(&f, service_id, &uncleared).await;

    let body = explain_column(&f, &token, &uncleared).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(
        profile["mask"]["decision"]["result"]["mask_type"], "MASK_NULL",
        "body: {body}"
    );
    assert_eq!(profile["masked_read_skips_access"], true, "body: {body}");
    assert!(
        !access_types(profile).contains(&"select".to_string()),
        "body: {body}"
    );
    lift_gate(&f, tag_id).await;
}

/// Spec C11: a verdict a rule decides names the rule, so the answer can say
/// "rule *Hash personal data*" without a second lookup by the client.
#[tokio::test]
async fn explain_names_the_rule_it_attributes() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let name = unique("vmask-names-the-rule");
    let rule = json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "path", "catalog_id": f.catalog_id,
                      "schema": "sales", "table": "orders", "column": "email" },
        "exempt": { "include": [], "exclude": [] }
    });
    let (status, written) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", &token, Some(rule)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {written}");

    let body = explain_column(&f, &token, "analyst").await;
    let mask = &profile_for(&body, "starrocks")["mask"];
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_HASH",
        "body: {body}"
    );
    assert_eq!(mask["attribution"]["kind"], "rule", "body: {body}");
    assert_eq!(
        mask["attribution"]["rule_id"], written["id"],
        "body: {body}"
    );
    assert_eq!(mask["attribution"]["name"], json!(name), "body: {body}");
}

/// Final review V-1, live 2c: a rule `MASK_HASH` folded into the clearance mask
/// for a cleared reader, plus a hand-written column `select` deny. StarRocks
/// refused that read (`5204`): a `MASK_HASH` column IS access-checked. So the
/// `select` verdict stays, denied, and nothing claims a skip. Keying the skip
/// on "any mask" instead of `MASK_NULL` turns this red.
#[tokio::test]
async fn a_mask_hash_column_keeps_its_starrocks_access_verdict() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, cleared, _uncleared) = clearance_readers(&f, &token).await;
    let rule = json!({
        "name": unique("v1-2c-hash"),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "path", "catalog_id": f.catalog_id,
                      "schema": "sales", "table": "orders", "column": "email" },
        "exempt": { "include": [], "exclude": [] }
    });
    let (status, written) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", &token, Some(rule)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {written}");
    let rule_id = written["id"].as_str().expect("rule id").to_string();
    let body = explain_column(&f, &token, &cleared).await;
    let service_id = profile_for(&body, "starrocks")["service_id"]
        .as_i64()
        .expect("service id");
    hand_column_deny(&f, service_id, &cleared).await;

    let body = explain_column(&f, &token, &cleared).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(
        profile["mask"]["decision"]["result"]["mask_type"], "MASK_HASH",
        "the cleared reader is folded the rule's MASK_HASH; body: {body}"
    );
    assert!(
        profile.get("masked_read_skips_access").is_none(),
        "body: {body}"
    );
    assert_eq!(
        access_for(profile, "select")["decision"]["kind"],
        "denied",
        "body: {body}"
    );
    let (status, _) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/perm/rules/{rule_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(status.is_success(), "delete the rule: {status}");
    lift_gate(&f, tag_id).await;
}

/// Spec §5.5 (D4): a cleared StarRocks reader's mask is the clearance's, which
/// replays the rule masks' outcome for them. Explain replays it without the
/// clearance so the answer credits both: the rule's `MASK_HASH`, or the rule's
/// exemption with who exempts them.
#[tokio::test]
async fn a_cleared_starrocks_reader_s_mask_replays_the_rule() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, cleared, _uncleared) = clearance_readers(&f, &token).await;
    let path = json!({ "type": "path", "catalog_id": f.catalog_id,
                       "schema": "sales", "table": "orders", "column": "email" });
    let rule_id = store_rule(
        &f,
        &token,
        mask_rule("vmask-replay", path.clone(), json!([])),
    )
    .await;

    let body = explain_column(&f, &token, &cleared).await;
    let mask = &profile_for(&body, "starrocks")["mask"];
    assert_eq!(mask["attribution"]["kind"], "clearance", "body: {body}");
    let replayed = &mask["replayed"];
    assert_eq!(
        replayed["decision"]["result"]["mask_type"], "MASK_HASH",
        "body: {body}"
    );
    assert_eq!(replayed["attribution"]["kind"], "rule", "body: {body}");
    assert_eq!(
        replayed["attribution"]["rule_id"],
        json!(rule_id),
        "body: {body}"
    );
    delete_rule(&f, &token, &rule_id).await;

    // Cleared AND exempt from the rule: the replay is the exempt item.
    let rule_id = store_rule(
        &f,
        &token,
        mask_rule(
            "vmask-replay-ex",
            path,
            json!([{"kind": "user", "ref": cleared}]),
        ),
    )
    .await;
    let body = explain_column(&f, &token, &cleared).await;
    let replayed = &profile_for(&body, "starrocks")["mask"]["replayed"];
    assert_eq!(
        replayed["decision"]["result"]["mask_type"], "MASK_NONE",
        "body: {body}"
    );
    assert_eq!(
        replayed["exemption"],
        json!({"via": [{"principal_kind": "user", "ref": cleared}], "more": 0}),
        "body: {body}"
    );
    delete_rule(&f, &token, &rule_id).await;
    lift_gate(&f, tag_id).await;
}

/// Review finding 2: a cleared reader under a rule that NULLs the column gets
/// `MASK_NULL` from the clearance mask too (the fold replays the rule). That
/// is the rule's outcome, not "not cleared": the replay names the rule, and the
/// uncleared reader still gets no replay.
#[tokio::test]
async fn a_cleared_reader_under_a_nulling_rule_is_told_the_rule_nulls_them() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, cleared, uncleared) = clearance_readers(&f, &token).await;
    let mut rule = mask_rule(
        "vmask-null-rule",
        json!({ "type": "path", "catalog_id": f.catalog_id,
                "schema": "sales", "table": "orders", "column": "email" }),
        json!([]),
    );
    rule["kind"]["mask_type"] = json!("MASK_NULL");
    let name = rule["name"].as_str().unwrap().to_string();
    let rule_id = store_rule(&f, &token, rule).await;

    let body = explain_column(&f, &token, &cleared).await;
    let profile = profile_for(&body, "starrocks");
    let mask = &profile["mask"];
    assert_eq!(mask["attribution"]["kind"], "clearance", "body: {body}");
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_NULL",
        "body: {body}"
    );
    assert_eq!(
        mask["replayed"]["attribution"]["kind"], "rule",
        "body: {body}"
    );
    assert_eq!(
        mask["replayed"]["attribution"]["rule_id"],
        json!(rule_id),
        "body: {body}"
    );
    let line = profile["effect_lines"][0]["text"].as_str().unwrap();
    assert!(
        line.starts_with(&format!(
            "{cleared} reads email as NULL by rule {name} (cleared for "
        )),
        "body: {body}"
    );

    let body = explain_column(&f, &token, &uncleared).await;
    assert!(
        profile_for(&body, "starrocks")["mask"]
            .get("replayed")
            .is_none(),
        "body: {body}"
    );
    delete_rule(&f, &token, &rule_id).await;
    lift_gate(&f, tag_id).await;
}

/// Review finding 4: a column carrying two gated classifications gets ONE
/// clearance mask (named after the lower tag id) whose cleared set is the
/// intersection, so a reader it clears is cleared for both, and the line names
/// both.
#[tokio::test]
async fn a_reader_cleared_for_two_classifications_is_told_both() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, role, cleared, _uncleared) = clearance_readers(&f, &token).await;
    let first: String = sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(tag_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("first tag name");
    let second = assign_tag(&f, f.catalog_id, "sales", "orders", "email").await;
    let second_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&second.key)
        .fetch_one(&f.db.pool)
        .await
        .expect("second tag id");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, ARRAY[$2], '{}')",
    )
    .bind(second_id)
    .bind(&role)
    .execute(&f.db.pool)
    .await
    .expect("gate the second tag");
    // A rule write recompiles the profile, folding both gates into one mask.
    let rule = mask_rule(
        "vmask-two-tags",
        json!({ "type": "path", "catalog_id": f.catalog_id,
                "schema": "sales", "table": "orders", "column": "email" }),
        json!([]),
    );
    let name = rule["name"].as_str().unwrap().to_string();
    let rule_id = store_rule(&f, &token, rule).await;

    let body = explain_column(&f, &token, &cleared).await;
    let mut tags = [first, second.name.clone()];
    tags.sort();
    assert_eq!(
        profile_for(&body, "starrocks")["effect_lines"][0]["text"],
        json!(format!(
            "{cleared} reads email as hashed by rule {name} (cleared for {} and {})",
            tags[0], tags[1]
        )),
        "body: {body}"
    );
    delete_rule(&f, &token, &rule_id).await;
    lift_gate(&f, second_id).await;
    lift_gate(&f, tag_id).await;
}

/// Live check defect 1: StarRocks asks no `select` for a column its clearance
/// nulls, but the query still needs `SELECT` on the other columns it reads.
/// A reader with no grant at all is refused by the engine (5204), so the line
/// must say so instead of "reads email as NULL". A granted, uncleared reader
/// does read NULL.
#[tokio::test]
async fn an_ungranted_starrocks_reader_of_a_nulled_column_is_refused_not_masked() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    let stranger = ungranted_reader(&f).await;

    let body = explain_column(&f, &token, &stranger).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(profile["masked_read_skips_access"], true, "body: {body}");
    let lines = profile["effect_lines"].as_array().expect("lines");
    assert_eq!(
        lines.len(),
        1,
        "a refused read has no rows line; body: {body}"
    );
    assert_eq!(lines[0]["outcome"], "refused", "body: {body}");
    assert_eq!(
        lines[0]["text"],
        json!(format!(
            "{stranger} is refused email (no policy allows it): StarRocks masks it to NULL, \
             and the query still needs SELECT on the other columns it reads"
        )),
        "body: {body}"
    );

    let body = explain_column(&f, &token, &uncleared).await;
    assert_eq!(
        profile_for(&body, "starrocks")["effect_lines"][0]["outcome"],
        "nullified",
        "body: {body}"
    );
    lift_gate(&f, tag_id).await;
}

async fn delete_rule(f: &Fixture, token: &str, rule_id: &str) {
    let (status, _) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/perm/rules/{rule_id}"),
                token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(status.is_success(), "delete the rule: {status}");
}

// ── D2 (2026-10-25): explain a whole-table clearance on StarRocks ──

/// `POST /perm/compile/{service}` as an admin: the recompile a write would do.
async fn compile_profile(f: &Fixture, token: &str) {
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                &format!("/perm/compile/{}", f.service_id),
                token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "compile: {body}");
}

/// The fixture catalog's warehouse, linked to a fresh stub Lakekeeper
/// warehouse: `(egret warehouse, lakekeeper warehouse)`.
async fn stub_catalog(f: &Fixture) -> (Uuid, Uuid) {
    let warehouse: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("warehouse");
    (
        warehouse,
        lakekeeper_stub::link_warehouse(&f.db, warehouse).await,
    )
}

async fn attach_whole_table(f: &Fixture, warehouse: Uuid, tag_id: Uuid) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales','orders','')",
    )
    .bind(tag_id)
    .bind(warehouse)
    .execute(&f.db.pool)
    .await
    .expect("whole-table assignment");
}

async fn tag_name_of(f: &Fixture, tag_id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(tag_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag name")
}

/// D2 + the live spike (StarRocks 4.1.4): on a table classified as a whole, an
/// uncleared reader holding a grant is refused a direct read (the `*` deny
/// matches the column StarRocks adds to the scan) and reads NULL through a view
/// (the column's NULL mask). Explain says both, attributed to the clearance on
/// the whole table, and no longer says the clearance is not enforced.
#[tokio::test]
async fn a_table_level_clearance_refuses_an_uncleared_reader_on_starrocks() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (warehouse, lk) = stub_catalog(&f).await;
    lakekeeper_stub::add_table(&f.db, lk, &["sales"], "orders", &["id", "email", "region"]).await;
    let (tag_id, _role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    attach_whole_table(&f, warehouse, tag_id).await;
    compile_profile(&f, &token).await;
    let tag = tag_name_of(&f, tag_id).await;

    let body = explain_column(&f, &token, &uncleared).await;
    let sr = profile_for(&body, "starrocks");
    lift_gate(&f, tag_id).await;
    assert!(sr.get("clearance_not_enforced").is_none(), "body: {body}");
    assert!(sr.get("clearance_masks_missing").is_none(), "body: {body}");
    assert_eq!(
        sr["effect_lines"][0]["text"],
        json!(format!(
            "{uncleared} is refused email (clearance {tag}, on the whole table); through a view \
             it reads NULL"
        )),
        "body: {body}"
    );
    assert_eq!(sr["effect_lines"][0]["outcome"], "refused", "body: {body}");
}

/// The cleared reader's line names every gated classification the column's
/// one clearance mask clears for: the one on the column AND the one on the
/// whole table.
#[tokio::test]
async fn a_reader_cleared_for_a_table_and_a_column_classification_is_told_both() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (_, lk) = stub_catalog(&f).await;
    lakekeeper_stub::add_table(&f.db, lk, &["sales"], "orders", &["id", "email"]).await;
    let (column_tag, role, cleared, _uncleared) = clearance_readers(&f, &token).await;
    let table = assign_tag(&f, f.catalog_id, "sales", "orders", "").await;
    let table_tag: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&table.key)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag id");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(table_tag)
    .bind(vec![role])
    .execute(&f.db.pool)
    .await
    .expect("gate");
    compile_profile(&f, &token).await;
    let column = tag_name_of(&f, column_tag).await;

    let body = explain_column(&f, &token, &cleared).await;
    lift_gate(&f, table_tag).await;
    lift_gate(&f, column_tag).await;
    let text = profile_for(&body, "starrocks")["effect_lines"][0]["text"]
        .as_str()
        .expect("line")
        .to_string();
    let (first, second) = if column < table.name {
        (column, table.name)
    } else {
        (table.name, column)
    };
    assert!(
        text.ends_with(&format!("(cleared for {first} and {second})")),
        "{text}"
    );
}

/// U4 and the live spike: a whole-table classification on a table the catalog
/// does not have compiles its deny and no mask, so a direct read is refused
/// and a view over the table would show the values. Explain says so.
#[tokio::test]
async fn an_unknown_table_says_its_masks_are_missing() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    stub_catalog(&f).await;
    let (fixture_tag, role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    lift_gate(&f, fixture_tag).await;
    let ghost = assign_tag(&f, f.catalog_id, "sales", "orders", "").await;
    let tag_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&ghost.key)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag id");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(tag_id)
    .bind(vec![role])
    .execute(&f.db.pool)
    .await
    .expect("gate");
    compile_profile(&f, &token).await;

    let body = explain_column(&f, &token, &uncleared).await;
    lift_gate(&f, tag_id).await;
    let sr = profile_for(&body, "starrocks");
    assert_eq!(
        sr["clearance_masks_missing"],
        json!([{"tag_id": tag_id, "tag_name": ghost.name, "reason": "table_not_in_catalog"}]),
        "body: {body}"
    );
    assert_eq!(
        sr["effect_lines"][0]["text"],
        json!(format!(
            "{uncleared} is refused email (clearance {0}, on the whole table). No NULL mask for \
             clearance {0} is served here: egret found no table sales.orders in the catalog, so \
             a view over it shows the values",
            ghost.name
        )),
        "body: {body}"
    );
}

/// A column added to the table since the last compile has no NULL mask yet:
/// content-derived (the served artifact has none for it, the catalog has the
/// table), not read from a timestamp.
#[tokio::test]
async fn a_column_added_since_the_compile_says_not_compiled() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (_, lk) = stub_catalog(&f).await;
    let table = lakekeeper_stub::add_table(&f.db, lk, &["sales"], "orders", &["id"]).await;
    let (fixture_tag, role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    lift_gate(&f, fixture_tag).await;
    let whole = assign_tag(&f, f.catalog_id, "sales", "orders", "").await;
    let tag_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&whole.key)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag id");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(tag_id)
    .bind(vec![role])
    .execute(&f.db.pool)
    .await
    .expect("gate");
    compile_profile(&f, &token).await;
    // `email` appears with no recompile.
    lakekeeper_stub::set_columns(&f.db, lk, table, 1, &["id", "email"]).await;

    let body = explain_column(&f, &token, &uncleared).await;
    lift_gate(&f, tag_id).await;
    let sr = profile_for(&body, "starrocks");
    assert_eq!(
        sr["clearance_masks_missing"][0]["reason"], "not_compiled",
        "body: {body}"
    );
    assert_eq!(
        sr["effect_lines"][0]["text"],
        json!(format!(
            "{uncleared} is refused email (clearance {0}, on the whole table). No NULL mask for \
             clearance {0} is served here: the profile has not been recompiled since this column \
             appeared, so a view over it shows the values",
            whole.name
        )),
        "body: {body}"
    );
}

/// Task 12 (C2), the other half: a classified column with a deny and NO mask
/// (for example a download from before the clearance's first compile, which
/// carried only the deny) is still checked, so explain still says denied.
#[tokio::test]
async fn explain_still_denies_a_classified_starrocks_column_with_no_mask() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, _cleared, uncleared) = clearance_readers(&f, &token).await;
    let body = explain_column(&f, &token, &uncleared).await;
    let service_id = profile_for(&body, "starrocks")["service_id"]
        .as_i64()
        .expect("service id");
    // Take the clearance's mask out of the served download, keeping its deny.
    let removed = sqlx::query(
        "DELETE FROM policy.authz_policies WHERE service_id = $1 \
         AND name LIKE 'perm-clearance-%' AND name LIKE '%-mask-%'",
    )
    .bind(service_id)
    .execute(&f.db.pool)
    .await
    .expect("remove the mask")
    .rows_affected();
    assert!(
        removed > 0,
        "the fixture compiled a clearance mask; body: {body}"
    );

    let body = explain_column(&f, &token, &uncleared).await;
    let profile = profile_for(&body, "starrocks");
    assert!(
        profile.get("masked_read_skips_access").is_none(),
        "body: {body}"
    );
    let deny = access_for(profile, "select");
    assert_eq!(deny["decision"]["kind"], "denied", "body: {body}");
    assert_eq!(
        deny["decision"]["reason"], "deny_policy_item",
        "body: {body}"
    );
    assert_eq!(deny["attribution"]["kind"], "clearance", "body: {body}");
    lift_gate(&f, tag_id).await;
}

/// Task 12 (live finding M-4, Task 10 deferred N2): explain no longer answers
/// `create materialized view`. It used to ask `default_catalog.<the target's
/// schema>`, a database nothing is created in, and showed a cleared reader a
/// red "denied" there (no policy matched). The A3 deny is engine-wide, so the
/// page states the rule instead of a per-database verdict.
#[tokio::test]
async fn explain_gives_no_materialized_view_row() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, cleared, uncleared) = clearance_readers(&f, &token).await;
    for reader in [&cleared, &uncleared] {
        let body = explain_column(&f, &token, reader).await;
        assert!(
            profile_for(&body, "starrocks")
                .get("materialized_view")
                .is_none(),
            "body: {body}"
        );
    }
    lift_gate(&f, tag_id).await;
}

/// The other reader kind: a CLEARED reader is allowed by their grant, and the
/// mask the clearance folded for them transforms nothing (`MASK_NONE`) — the
/// page must not tell them their column is nulled.
#[tokio::test]
async fn explain_shows_a_cleared_reader_the_clearance_mask_as_no_transform() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, _role, cleared, _uncleared) = clearance_readers(&f, &token).await;

    let body = explain_column(&f, &token, &cleared).await;
    let profile = profile_for(&body, "starrocks");
    assert_eq!(
        access_for(profile, "select")["decision"]["kind"],
        "allowed",
        "body: {body}"
    );
    let mask = &profile["mask"];
    assert_eq!(mask["decision"]["kind"], "masked", "body: {body}");
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_NONE",
        "body: {body}"
    );
    assert_eq!(mask["attribution"]["kind"], "clearance", "body: {body}");
    // Spec §5.5: the clearance's outcome replayed without it — no rule masks.
    assert_eq!(
        mask["replayed"]["decision"]["kind"], "no_mask",
        "body: {body}"
    );

    // A target that names no column has no mask to report.
    let (status, table_level) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, &cleared)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {table_level}");
    assert!(
        profile_for(&table_level, "starrocks").get("mask").is_none(),
        "a table-level explain carries no mask; body: {table_level}"
    );
    lift_gate(&f, tag_id).await;
}

// ---------------------------------------------------------------------------
// compile (the manual repair trigger)
// ---------------------------------------------------------------------------

/// D2 (U4): a whole-table classification on a table the catalog does not have
/// compiles its deny and no masks, and `POST /perm/compile`'s `skipped` list
/// says so, naming the table, while the call succeeds.
#[tokio::test]
async fn compile_reports_a_whole_table_clearance_on_an_unknown_table_on_the_wire() {
    let Some(f) = setup().await else { return };
    let warehouse_id: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("warehouse");
    let catalog: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(f.catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name");
    lakekeeper_stub::link_warehouse(&f.db, warehouse_id).await;
    let tag = assign_tag(&f, f.catalog_id, "sales", "ghost", "").await;
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let tag_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&tag.key)
        .fetch_one(&f.db.pool)
        .await
        .expect("tag id");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, ARRAY[$2], '{}')",
    )
    .bind(tag_id)
    .bind(unique("d2-role"))
    .execute(&f.db.pool)
    .await
    .expect("gate");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                &format!("/perm/compile/{}", f.service_id),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    lift_gate(&f, tag_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let skips: Vec<&Value> = body["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .filter(|s| s["id"] == json!(tag_id.to_string()))
        .collect();
    assert_eq!(skips.len(), 1, "body: {body}");
    assert_eq!(skips[0]["source_kind"], "clearance", "body: {body}");
    let reason = skips[0]["reason"].as_str().expect("reason");
    assert!(
        reason.contains(&format!("{catalog}.sales.ghost")),
        "{reason}"
    );
    assert!(reason.contains("not protected"), "{reason}");
}

/// Live check F1 (D2, U3): a rule write that would push a tag rule's StarRocks
/// expansion over its ceiling is refused as the caller's conflict, a coded
/// 409 that keeps the compiler's sentence; it was a 400. Nothing is stored.
#[tokio::test]
async fn a_rule_over_the_expansion_ceiling_is_a_coded_409() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let warehouse: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("warehouse");
    // The fixture tag is on `orders.email`; 500 more columns make 501.
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) SELECT t.id, $2, 'sales', 'wide', 'c' || gs::text \
         FROM tags t, generate_series(1, 500) gs WHERE t.key = $1",
    )
    .bind(&f.tag_key)
    .bind(warehouse)
    .execute(&f.db.pool)
    .await
    .expect("500 more columns");
    let name = unique("f1-hash");
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(json!({
                    "name": name,
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    sqlx::query(
        "DELETE FROM catalog_tag_assignments a USING tags t \
          WHERE t.id = a.tag_id AND t.key = $1 AND a.table_name = 'wide'",
    )
    .bind(&f.tag_key)
    .execute(&f.db.pool)
    .await
    .expect("clean up");
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(body["code"], "expansion_ceiling", "body: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("over the ceiling of 500"),
        "body: {body}"
    );
    assert_eq!(
        rules_named(&f, &name).await,
        0,
        "a refused rule is not stored"
    );
}

/// `POST /compile/{service_id}` brings a profile back in line with the model
/// after something outside the model made it drift. Proven by deleting the
/// compiled policy row directly — the one thing no endpoint does — and
/// watching the trigger restore it.
/// View governance A2, fix round 1: a StarRocks clearance whose mask cannot be
/// folded because the bound engine runs an unmodelled image is a SKIP, and the
/// skip reaches the wire — `POST /perm/compile`'s `skipped` list carries it as a
/// `clearance` item naming the engine and the remedy, while the call succeeds.
#[tokio::test]
async fn compile_reports_a_skipped_clearance_mask_on_the_wire() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let tag_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE key = $1")
        .bind(&f.tag_key)
        .fetch_one(&f.db.pool)
        .await
        .expect("fixture tag");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, ARRAY[$2], ARRAY[]::uuid[])",
    )
    .bind(tag_id)
    .bind(unique("viewgov-role"))
    .execute(&f.db.pool)
    .await
    .expect("gate the fixture tag");
    let engine_name: String = sqlx::query_scalar(
        "UPDATE engine_mgmt.engines \
            SET spec = '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:3.3.0\"}}'::jsonb \
          WHERE authz_service_id = $1 RETURNING name",
    )
    .bind(f.service_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("an unmodelled image");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                &format!("/perm/compile/{}", f.service_id),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let skips: Vec<&Value> = body["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .filter(|s| s["id"] == json!(tag_id.to_string()))
        .collect();
    assert_eq!(skips.len(), 1, "body: {body}");
    assert_eq!(
        skips[0]["source_kind"].as_str(),
        Some("clearance"),
        "body: {body}"
    );
    let reason = skips[0]["reason"].as_str().expect("reason");
    assert!(
        reason.contains("view protection (clearance mask) not active"),
        "{reason}"
    );
    assert!(
        reason.contains(&format!("engine {engine_name} ")),
        "{reason}"
    );
    assert!(
        reason.contains("docs/en/guides/engine-versions.md"),
        "{reason}"
    );
}

#[tokio::test]
async fn compile_restores_a_profile_that_drifted() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let name = format!(
        "{COMPILER_GRANT_PREFIX}{}",
        body["id"].as_str().expect("minted grant id")
    );
    assert_eq!(f.compiled_policy_names().await, vec![name.clone()]);

    sqlx::query("DELETE FROM policy.authz_policies WHERE service_id = $1 AND name = $2")
        .bind(f.service_id)
        .bind(&name)
        .execute(&f.db.pool)
        .await
        .expect("simulate drift");
    assert_eq!(f.compiled_policy_names().await, Vec::<String>::new());

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                &format!("/perm/compile/{}", f.service_id),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["created"], 1, "body: {body}");
    assert_eq!(f.compiled_policy_names().await, vec![name]);
}

#[tokio::test]
async fn compile_on_an_unknown_profile_is_a_404() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (status, _) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/compile/-1", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// listing, 404s, and the permission gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_grant_is_listed_with_its_principal_subtract() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let mut body = grant_body(f.catalog_id, "analyst");
    body["principals"]["exclude"] = json!([{ "kind": "user", "ref": "contractor" }]);
    let (status, created) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/grants", &token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let id = created["id"].as_str().unwrap().to_string();

    let (status, listed) = into_json(
        f.app()
            .oneshot(req("GET", "/perm/grants", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mine = listed
        .as_array()
        .expect("array")
        .iter()
        .find(|g| g["id"] == id)
        .unwrap_or_else(|| panic!("the created grant must be listed; got {listed}"));
    assert_eq!(mine["effect"], "allow");
    assert_eq!(mine["origin"]["type"], "explicit");
    assert_eq!(mine["principals"]["exclude"][0]["ref"], "contractor");
}

#[tokio::test]
async fn deleting_an_unknown_grant_or_rule_is_a_404() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let missing = Uuid::new_v4();
    for uri in [
        format!("/perm/grants/{missing}"),
        format!("/perm/rules/{missing}"),
    ] {
        let (status, _) = into_json(
            f.app()
                .oneshot(req("DELETE", &uri, &token, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

/// `policy:read` reaches the GETs and `explain`, and nothing else. The write
/// routes need `policy:update` — the same split the policy console already
/// uses, reused rather than given a new key.
#[tokio::test]
async fn policy_read_permits_the_reads_and_refuses_the_writes() {
    let Some(mut f) = setup().await else { return };
    let token = session_with_permission(&mut f, "policy:read").await;

    for (method, uri, body) in [
        ("GET", "/perm/grants".to_string(), None),
        ("GET", "/perm/rules".to_string(), None),
        (
            "POST",
            "/perm/explain".to_string(),
            Some(explain_body(f.catalog_id, "nobody")),
        ),
    ] {
        let (status, got) = into_json(
            f.app()
                .oneshot(req(method, &uri, &token, body))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{method} {uri} -> {got}");
    }

    for (method, uri, body) in [
        (
            "POST",
            "/perm/grants".to_string(),
            Some(grant_body(f.catalog_id, "analyst")),
        ),
        (
            "POST",
            "/perm/rules".to_string(),
            Some(json!({
                "name": unique("r"),
                "kind": { "type": "row_filter", "filter_expr": "1=1" },
                "selector": { "type": "tag", "key": f.tag_key },
                "exempt": { "include": [], "exclude": [] }
            })),
        ),
        ("DELETE", format!("/perm/grants/{}", Uuid::new_v4()), None),
        ("DELETE", format!("/perm/rules/{}", Uuid::new_v4()), None),
        ("POST", format!("/perm/compile/{}", f.service_id), None),
        (
            "PATCH",
            format!("/perm/grants/{}", Uuid::new_v4()),
            Some(grant_body(f.catalog_id, "analyst")),
        ),
        // The preview PREVIEWS A WRITE: it runs real compiler work under real
        // locks and is useless to someone who cannot write, so it gates on
        // `policy:update` like the mutations and not on `policy:read` like
        // `explain`.
        (
            "POST",
            "/perm/preview".to_string(),
            Some(json!({ "grant": grant_body(f.catalog_id, "analyst") })),
        ),
        // **This tuple was missing and the gate was therefore untested** (SP4b-1
        // review, I-1): downgrading `patch_rule`'s `require` to `policy:read`
        // passed all 34 tests, which means a `policy:read` holder could rewrite
        // a mask rule and drop a PII mask. Every route in this branch now
        // appears in BOTH directions of this gate — here, and in
        // `policy_update_alone_permits_a_write`, which is the direction a
        // typo'd permission key shows up in.
        (
            "PATCH",
            format!("/perm/rules/{}", Uuid::new_v4()),
            Some(json!({
                "name": unique("gate"),
                "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                "selector": { "type": "path", "catalog_id": f.catalog_id,
                              "schema": "sales", "table": "orders", "column": "email" },
                "exempt": { "include": [], "exclude": [] }
            })),
        ),
    ] {
        let (status, got) = into_json(
            f.app()
                .oneshot(req(method, &uri, &token, body))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "policy:read must not reach {method} {uri}; got {got}"
        );
    }
    assert_eq!(
        f.compiled_policy_names().await,
        Vec::<String>::new(),
        "a refused write must have compiled nothing"
    );
}

/// **The positive half of the gate.** Every other write test here
/// authenticates a `platform_admin`, who bypasses the PDP entirely — so a
/// handler that gated writes on a typo'd key would pass both that test and the
/// refusal test above, and the gate would be untested in the direction that
/// matters. This asserts a NON-admin holding exactly `policy:update` can write,
/// which fails if the key is anything other than `policy:update`.
#[tokio::test]
async fn policy_update_alone_permits_a_write() {
    let Some(mut f) = setup().await else { return };
    let token = session_with_permission(&mut f, "policy:update").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "policy:update alone must reach the write path; body: {body}"
    );
    let grant_id = body["id"].as_str().expect("minted grant id").to_string();
    assert_eq!(
        f.compiled_policy_names().await,
        vec![format!("{COMPILER_GRANT_PREFIX}{grant_id}")],
        "and the write must have compiled, same as it does for an admin"
    );

    // **Every route this branch added, in the direction a typo'd key shows up
    // in** (SP4b-1 review, I-1). The refusal test above cannot catch a typo:
    // a handler gated on `policy:updat` refuses a `policy:read` holder exactly
    // as it should and refuses everybody else too, so only a positive
    // assertion by a non-admin holding precisely `policy:update` pins the key.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{grant_id}"),
                &token,
                Some(grant_body(f.catalog_id, "auditor")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "policy:update alone must reach PATCH /perm/grants/{{id}}; body: {body}"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "grant": grant_body(f.catalog_id, "analyst") })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "policy:update alone must reach POST /perm/preview; body: {body}"
    );

    let rule_body = |name: &str, mask: &str| {
        json!({
            "name": name,
            "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
            "selector": { "type": "path", "catalog_id": f.catalog_id,
                          "schema": "sales", "table": "orders", "column": "email" },
            "exempt": { "include": [], "exclude": [] }
        })
    };
    let name = unique("gate-rule");
    let (status, created) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(rule_body(&name, "MASK_HASH")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let rule_id = created["id"].as_str().expect("minted rule id").to_string();
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/rules/{rule_id}"),
                &token,
                Some(rule_body(&name, "MASK_NULL")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "policy:update alone must reach PATCH /perm/rules/{{id}} — the gate the review \
         found untested in BOTH directions; body: {body}"
    );

    for uri in [
        format!("/perm/rules/{rule_id}"),
        format!("/perm/grants/{grant_id}"),
    ] {
        let (status, body) = into_json(
            f.app()
                .oneshot(req("DELETE", &uri, &token, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri} -> {body}");
    }
    assert_eq!(f.compiled_policy_names().await, Vec::<String>::new());
}

#[tokio::test]
async fn an_unauthenticated_request_is_refused() {
    let Some(f) = setup().await else { return };
    let (status, _) = into_json(
        f.app()
            .oneshot(req("GET", "/perm/grants", "not-a-session", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "got {status}"
    );
}

// ---------------------------------------------------------------------------
// the read surface resolves its own display names (SP4b-2 finding 1)
// ---------------------------------------------------------------------------

/// **THE DISCRIMINATING CASE.** A caller holding `policy:read` and NOTHING
/// else must get the catalog's NAME on every grant and the tag's display name
/// on every tag-selected rule.
///
/// Run as a `platform_admin` this assertion passes whether the name is on the
/// payload or resolved client-side from `GET /control-plane/catalogs`
/// (`catalog_registry:read`) and `GET /tags` (`tag:read`), because
/// `platform_admin` bypasses the PDP and therefore holds all three. That is
/// exactly how the browser run's admin pass reported D5's sentence as
/// working, while the same page showed a `policy_reader`
/// `role analyst cannot read 4a8fe34a-af58-4984-a1e7-12854fb9d496.sales.orders`.
/// The session below cannot reach either of those endpoints, so a name it sees
/// can only have come from this response.
///
/// It also pins the ADD, not replace: `scope.catalog_id` and the tag rule's
/// `selector.key` are still on the wire unchanged, because they are what a
/// write sends back.
#[tokio::test]
async fn the_read_surface_names_the_catalog_and_the_tag_for_a_policy_read_only_caller() {
    let Some(mut f) = setup().await else { return };

    let catalog_name: String =
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("read the fixture catalog's name");
    let tag_name: String = sqlx::query_scalar("SELECT name FROM tags WHERE key = $1")
        .bind(&f.tag_key)
        .fetch_one(&f.db.pool)
        .await
        .expect("read the fixture tag's name");
    assert_ne!(
        tag_name, f.tag_key,
        "the fixture's tag name must differ from its key, or `tag_name` \
         could be the key and this test could not tell"
    );

    // The grant goes through the endpoint — a catalog-scoped write, so no
    // installation-wide fan-out to serialize against.
    let admin = admin_session(&f).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &admin,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let grant_id = body["id"].as_str().expect("minted grant id").to_string();

    // The tag rule goes through the STORE, not the endpoint: creating one over
    // HTTP recompiles every compiler-targeted profile in the installation
    // (see `TAG_FANOUT_LOCK`), and nothing here is about the fan-out — this is
    // a test of what the READ hands back.
    //
    // **But the ROW is still installation-wide, and that made this test a
    // hazard to others (SP4c review).** Going through the store skips the
    // fan-out this test causes, not the fan-out it CAUSES FOR SOMEBODY ELSE: a
    // concurrent `preview_and_write_agree_about_a_tag_rules_fan_out` recompiles
    // every profile between its preview and its write, and a tag rule that
    // appeared in between makes the two summaries disagree — which is exactly
    // the flake that run produced. So this test takes the same lock, AFTER its
    // fixture setup (the helper's own warning: `setup` takes the lock itself),
    // and deletes the row before releasing it. Avoiding the defect by running
    // the suite differently would have left it for the next person.
    let _fanout = exclusive(&f).await;
    let mut tx = f.db.pool.begin().await.expect("begin");
    let rule_id = egret_perm::create_rule_in_tx(
        &mut tx,
        &egret_perm::Rule {
            id: Uuid::nil(),
            name: unique("finding1_rule"),
            kind: egret_perm::RuleKind::Mask {
                mask_type: "MASK".into(),
                value_expr: None,
            },
            selector: egret_perm::Selector::Tag {
                key: f.tag_key.clone(),
            },
            exempt: egret_perm::PrincipalSet::default(),
        },
    )
    .await
    .expect("insert the tag rule");
    tx.commit().await.expect("commit");

    let token = session_with_permission(&mut f, "policy:read").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req("GET", "/perm/grants", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let row = body
        .as_array()
        .expect("a grant array")
        .iter()
        .find(|g| g["id"] == json!(grant_id))
        .unwrap_or_else(|| panic!("the grant just created is missing from {body}"));
    assert_eq!(
        row["catalog_name"],
        json!(catalog_name),
        "a `policy:read`-only caller must be told the catalog's name — it \
         cannot ask the catalog registry; row: {row}"
    );
    assert_eq!(
        row["scope"]["catalog_id"],
        json!(f.catalog_id.to_string()),
        "and the id a write sends back must still be there, unchanged; row: {row}"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req("GET", "/perm/rules", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let row = body
        .as_array()
        .expect("a rule array")
        .iter()
        .find(|r| r["id"] == json!(rule_id.to_string()))
        .unwrap_or_else(|| panic!("the rule just created is missing from {body}"));
    assert_eq!(
        row["tag_name"],
        json!(tag_name),
        "a tag rule must carry the tag's DISPLAY name; `tag:read` is not \
         implied by `policy:read`; row: {row}"
    );
    assert_eq!(
        row["selector"]["key"],
        json!(f.tag_key),
        "and the stored key — what a write sends back — must be unchanged; row: {row}"
    );
    assert_eq!(
        row["catalog_name"],
        json!(null),
        "a tag selector names no catalog, so inventing one would be a lie; row: {row}"
    );

    // Remove the installation-wide row while the lock is still held, so no
    // other test's fan-out can ever see it. Not hygiene: this row is the one
    // that perturbed another test's assertion.
    sqlx::query("DELETE FROM perm.rules WHERE id = $1")
        .bind(rule_id)
        .execute(&f.db.pool)
        .await
        .expect("remove the installation-wide tag rule");
}

// ---------------------------------------------------------------------------
// explain over the SERVED ARTIFACT (SP4d-2a Task 5)
// ---------------------------------------------------------------------------
//
// These do not look at `perm.grants` and they do not look at
// `egret_perm::eval` (except the one agreement test that deliberately does).
// They look at what `explain` answers with now: the compiled policy set each
// engine downloads, evaluated by `egret-ranger-eval` once per profile and
// once per Ranger access type. A test that asserted only "allowed: true"
// would pass against the old model-only evaluator too, which is exactly the
// implementation this package replaces.

/// The profile block for `engine` — `"trino"` / `"starrocks"`, which is
/// `WireEngineType`'s `rename_all = "lowercase"` spelling and the same one
/// `egret_query_api::Engine` and `authz_services.type` use (SP4d-2a Task 5
/// review, I3). Panics naming the whole body, because a missing profile is
/// the failure mode every per-profile assertion here would otherwise skip
/// past.
fn profile_for<'a>(body: &'a Value, engine: &str) -> &'a Value {
    body["profiles"]
        .as_array()
        .unwrap_or_else(|| panic!("no `profiles` array in {body}"))
        .iter()
        .find(|p| p["engine"] == engine)
        .unwrap_or_else(|| panic!("no {engine} profile in {body}"))
}

/// One Ranger access type's verdict inside a profile block.
fn access_for<'a>(profile: &'a Value, access_type: &str) -> &'a Value {
    profile["accesses"]
        .as_array()
        .unwrap_or_else(|| panic!("no `accesses` array in {profile}"))
        .iter()
        .find(|a| a["access_type"] == access_type)
        .unwrap_or_else(|| panic!("no {access_type} verdict in {profile}"))
}

/// Every Ranger access type a profile block reports, sorted.
fn access_types(profile: &Value) -> Vec<String> {
    let mut out: Vec<String> = profile["accesses"]
        .as_array()
        .unwrap_or_else(|| panic!("no `accesses` array in {profile}"))
        .iter()
        .map(|a| a["access_type"].as_str().expect("access_type").to_string())
        .collect();
    out.sort();
    out
}

/// `registry.catalogs.name`, which a raw hand-written policy has to name in
/// its `resources` map the way the compiler's own output does.
async fn catalog_name(f: &Fixture, catalog_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name")
}

/// A raw, operator-shaped Ranger access policy body over
/// `catalog/{level}/table/column` with `*` below the catalog — the shape a
/// human writes in the policy console, with no `perm` model object behind it.
/// `items` is the `policyItems` array and `deny_items` the `denyPolicyItems`
/// one, verbatim.
fn raw_policy_body(
    name: &str,
    service_name: &str,
    service_type: &str,
    level: &str,
    catalog: &str,
    items: Value,
    deny_items: Value,
) -> Value {
    let any = json!({ "values": ["*"], "isExcludes": false, "isRecursive": false });
    let mut resources = serde_json::Map::new();
    resources.insert(
        "catalog".into(),
        json!({ "values": [catalog], "isExcludes": false, "isRecursive": false }),
    );
    // The middle level is `schema` on Trino and `database` on StarRocks — the
    // same position in the hierarchy under two declared names, which is why
    // the caller passes it in rather than this helper hard-coding Trino's.
    resources.insert(level.to_string(), any.clone());
    resources.insert("table".into(), any.clone());
    resources.insert("column".into(), any);
    json!({
        "name": name,
        "service": service_name,
        "serviceType": service_type,
        "policyType": 0,
        "isEnabled": true,
        "resources": Value::Object(resources),
        "policyItems": items,
        "denyPolicyItems": deny_items,
        "allowExceptions": [],
        "denyExceptions": [],
        "isDenyAllElse": false,
    })
}

/// Requirement 1. Two profiles serve one catalog, so `explain` owes two
/// answers — each naming the profile it is about and the engine whose plugin
/// would take it. One collapsed verdict cannot be right for both: the same
/// grant compiles into two different policy sets, and Trino and StarRocks do
/// not even share the resource-level vocabulary the request is built from.
#[tokio::test]
async fn explain_answers_once_per_profile_naming_the_profile_and_its_engine() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-two").await;

    let user = known_user(&f, unique("two-profile-reader")).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let grant_id = body["id"].as_str().expect("grant id").to_string();

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["outcome"], "per_profile", "body: {body}");
    assert_eq!(
        body["profiles"].as_array().expect("profiles").len(),
        2,
        "two profiles serve this catalog, so there are two answers; body: {body}"
    );

    for (engine, service_id) in [("trino", trino_id), ("starrocks", starrocks_id)] {
        let profile = profile_for(&body, engine);
        assert_eq!(
            profile["service_id"], service_id,
            "the {engine} block must name its own profile; body: {body}"
        );
        assert!(
            !profile["service_name"]
                .as_str()
                .expect("service_name")
                .is_empty(),
            "body: {body}"
        );
        let verdict = access_for(profile, "select");
        assert_eq!(
            verdict["decision"]["kind"], "allowed",
            "{engine} downloaded the compiled grant; body: {body}"
        );
        assert_eq!(
            verdict["decision"]["policy_name"],
            format!("{COMPILER_GRANT_PREFIX}{grant_id}"),
            "and the verdict must name the policy that decided it; body: {body}"
        );
        assert_eq!(
            verdict["attribution"]["kind"], "grant",
            "which maps back to a `perm` Grant; body: {body}"
        );
        assert_eq!(verdict["attribution"]["grant_id"], grant_id, "body: {body}");
    }
}

/// Requirement 2. **"Nothing is enforcing this" is not a refusal.** A catalog
/// no engine serves has no artifact to evaluate, and reporting that as
/// `Denied` would tell a steward the model is withholding access when in fact
/// no plugin would ever be consulted.
#[tokio::test]
async fn a_catalog_no_engine_serves_is_its_own_outcome_not_a_denial() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "explain-orphan").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(orphan, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["outcome"], "no_profile_serves_the_catalog",
        "no engine serves this catalog; body: {body}"
    );
    assert_eq!(
        body["profiles"],
        json!([]),
        "and there is therefore nothing to report per profile; body: {body}"
    );
}

/// Requirement 3. **Ranger has no `write`.** `AccessRequest.access_type` is
/// ONE string, and `write` is `insert,delete` on Trino and
/// `insert,update,delete` on StarRocks (`egret_perm::engine_accesses`), so a
/// `write` explain is several evaluations and must report them separately.
/// The StarRocks half is deliberately made to DISAGREE with itself — a raw
/// deny on `update` alone — because a handler that collapsed the three into
/// one verdict would still look right if all three agreed.
#[tokio::test]
async fn a_write_explain_reports_one_verdict_per_ranger_access_type() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (_trino_id, starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-write").await;
    let catalog = catalog_name(&f, catalog_id).await;

    let user = known_user(&f, unique("two-profile-writer")).await;

    // One raw deny, on ONE of the three StarRocks access types. Written BEFORE
    // the grant on purpose: the first recompile stamps the profile
    // `origin = 'generated'`, after which `create_policy` refuses by design
    // (`refuse_if_generated`) — and a recompile only ever touches
    // compiler-prefixed names, so this policy survives the one below.
    let starrocks = f
        .policy
        .get_service_by_id(starrocks_id)
        .await
        .expect("starrocks service row");
    let deny_name = unique("ops-no-update");
    f.policy
        .create_policy(
            starrocks_id,
            &deny_name,
            &raw_policy_body(
                &deny_name,
                &starrocks.name,
                "starrocks",
                "database",
                &catalog,
                json!([]),
                json!([{
                    "users": [user], "groups": [], "roles": [],
                    "accesses": [{ "type": "update", "isAllowed": true }],
                    "delegateAdmin": false,
                }]),
            ),
        )
        .await
        .expect("create raw deny policy");

    let mut grant = grant_body(catalog_id, &user);
    grant["actions"] = json!(["write"]);
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/grants", &token, Some(grant)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");

    let mut ask = explain_body(catalog_id, &user);
    ask["action"] = json!("write");
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let trino = profile_for(&body, "trino");
    assert_eq!(
        access_types(trino),
        vec!["delete".to_string(), "insert".to_string()],
        "Trino's service-def declares no `update`; body: {body}"
    );
    let starrocks = profile_for(&body, "starrocks");
    assert_eq!(
        access_types(starrocks),
        vec![
            "delete".to_string(),
            "insert".to_string(),
            "update".to_string()
        ],
        "StarRocks' does; body: {body}"
    );
    assert_eq!(
        access_for(starrocks, "insert")["decision"]["kind"],
        "allowed",
        "body: {body}"
    );
    assert_eq!(
        access_for(starrocks, "delete")["decision"]["kind"],
        "allowed",
        "body: {body}"
    );
    assert_eq!(
        access_for(starrocks, "update")["decision"]["kind"],
        "denied",
        "an operator who can insert but not update needs to SEE that, which a \
         collapsed `write` verdict cannot say; body: {body}"
    );
    assert_eq!(
        access_for(starrocks, "update")["decision"]["policy_name"],
        deny_name,
        "body: {body}"
    );
}

/// Requirement 4. **The blindness this package exists to remove.** Auto-bind
/// seeds `egret-admin` policies granting `*` on everything onto every
/// generated profile (`egret_policy::default_policies`), and the old,
/// model-only `explain` could not see a single one of them: an operator
/// asking "why can this person read that?" got `no_grant_matched` while the
/// engine allowed. Asserted directly, because it is the point.
#[tokio::test]
async fn a_seeded_egret_admin_policy_is_visible_to_explain() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let svc = f
        .policy
        .get_service_by_id(f.service_id)
        .await
        .expect("service row");
    let user = unique("seeded-admin-subject");

    let seeded = f
        .policy
        .seed_admin_all_access(
            f.service_id,
            &svc.name,
            &svc.r#type,
            std::slice::from_ref(&user),
        )
        .await
        .expect("seed egret-admin policies");
    assert!(seeded > 0, "the seeder must actually have written policies");
    assert!(
        f.compiled_policy_names().await.is_empty(),
        "and this subject holds NO `perm` grant — the old explain answered \
         `no_grant_matched` here while the engine allowed"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "allowed",
        "the seeded `egret-admin` policy allows, and explain must say so; body: {body}"
    );
    assert!(
        verdict["decision"]["policy_name"]
            .as_str()
            .expect("policy_name")
            .starts_with("egret-admin"),
        "and must NAME the seeded policy; body: {body}"
    );
    assert_eq!(
        verdict["attribution"],
        Value::Null,
        "a seeded policy has no `perm` model object, and inventing one would be \
         worse than reporting none; body: {body}"
    );
}

/// Requirement 5. A policy an operator wrote by hand has no model object
/// behind it. It must still be NAMED and must never be dropped — dropping
/// what the compiler does not own would rebuild, inside the new `explain`,
/// exactly the blindness the old one had.
#[tokio::test]
async fn a_raw_operator_written_policy_is_named_with_no_model_attribution() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let svc = f
        .policy
        .get_service_by_id(f.service_id)
        .await
        .expect("service row");
    let catalog = catalog_name(&f, f.catalog_id).await;
    let user = unique("raw-policy-subject");

    let name = unique("ops-hand-written-deny");
    f.policy
        .create_policy(
            f.service_id,
            &name,
            &raw_policy_body(
                &name,
                &svc.name,
                "starrocks",
                "database",
                &catalog,
                json!([]),
                json!([{
                    "users": [user], "groups": [], "roles": [],
                    "accesses": [{ "type": "select", "isAllowed": true }],
                    "delegateAdmin": false,
                }]),
            ),
        )
        .await
        .expect("create raw policy");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "the raw policy denies, and it is in the artifact the engine downloads; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "deny_policy_item",
        "and it denied through a deny item, not by nothing matching — the two are \
         different facts; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["policy_name"], name,
        "the policy must be named even though egret's compiler does not own it; body: {body}"
    );
    assert_eq!(
        verdict["attribution"],
        Value::Null,
        "with no model attribution, rather than being dropped; body: {body}"
    );
}

/// Requirement 6. **`roles` and `groups` reach the evaluator EMPTY.** egret
/// serves no `/service/roles/download` and runs no usersync, so a policy item
/// naming a role, or a group OTHER THAN `public`, matches nobody on any engine
/// egret provisions (D0/D7). Sending neither is also the strongest available
/// form of SP4c's rule that a caller may not assert a role the subject does
/// not hold: there is no role to assert.
///
/// **`public` is the exception, and it is not a hole in this test.**
/// `egret-ranger-eval` matches `GROUP_PUBLIC` by NAME, independently of
/// `request.groups` (`eval.rs`'s `match_user_group_and_owner`), because that
/// is what Ranger does — the group check needs only a non-null `userGroups`
/// and `setUserGroups` normalises `null` to an empty set. egret's own compiler
/// relies on it: every clearance deny and every mask and row-filter item is
/// emitted with `groups: ["public"]`. So a policy granting to `public` DOES
/// decide here, and "a group matches nobody" is true only of the groups a
/// caller could name. The group below is unique and is not `public`, which is
/// what makes it a measurement of the forwarding and not of `public`.
///
/// Both halves are made to fail OPEN if the handler forwards them — each raw
/// policy would allow, and the subject genuinely holds the role in
/// `access_grants` and genuinely names the group in the request body.
#[tokio::test]
async fn a_held_role_and_a_named_group_reach_the_evaluator_empty() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let svc = f
        .policy
        .get_service_by_id(f.service_id)
        .await
        .expect("service row");
    let catalog = catalog_name(&f, f.catalog_id).await;

    let role_name = unique("sp4d-role-that-grants");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&f.db.pool)
            .await
            .expect("insert role");
    let uid = new_user(&f, "sp4d_roleholder").await;
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(uid)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("assign role");
    assert_eq!(res.rows_affected(), 1, "the subject really holds the role");
    let mut conn = f.db.pool.acquire().await.expect("acquire conn");
    let username = egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(uid))
        .await
        .expect("mint username");
    drop(conn);

    let group_name = unique("sp4d-group-that-grants");
    for (suffix, item) in [
        (
            "by-role",
            json!({ "users": [], "groups": [], "roles": [role_name],
                    "accesses": [{ "type": "select", "isAllowed": true }], "delegateAdmin": false }),
        ),
        (
            "by-group",
            json!({ "users": [], "groups": [group_name], "roles": [],
                    "accesses": [{ "type": "select", "isAllowed": true }], "delegateAdmin": false }),
        ),
    ] {
        let name = unique(&format!("ops-allow-{suffix}"));
        f.policy
            .create_policy(
                f.service_id,
                &name,
                &raw_policy_body(
                    &name,
                    &svc.name,
                    "starrocks",
                    "database",
                    &catalog,
                    json!([item]),
                    json!([]),
                ),
            )
            .await
            .expect("create raw allow policy");
    }

    let mut ask = explain_body(f.catalog_id, &username);
    ask["subject"]["groups"] = json!([group_name]);
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "a policy granting through a role the subject HOLDS, or a group they NAMED, \
         matches nobody on any engine egret provisions — forwarding either would \
         report an access no engine grants; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "no_policy_matched",
        "and nothing matched at all, which is Ranger's own answer here; body: {body}"
    );
}

/// **A target that skips a level is REFUSED, not answered** (whole-branch
/// review, I-4 — fail-open).
///
/// `request_resource` stops at the first absent level, which is the shape an
/// engine request really has. Its comment used to cite `validate_grant_shape`
/// and the picker as the reason a gap cannot reach it; neither guards this
/// path. `validate_grant_shape` validates PRINCIPALS only, and the real
/// level-skip guard is a CHECK on the stored `perm.grants`/`perm.rules` rows
/// (`migrations/0154_perm_model.sql`) — an `explain` request body is never
/// stored, so any `policy:read` caller can post one.
///
/// The consequence is fail-open and silent: `{schema: null, table: "t"}`
/// builds the request resource `{catalog}` alone, a catalog-wide grant matches
/// it as `SELF_AND_ALL_DESCENDANTS` while a table-level clearance deny is only
/// a `DESCENDANT` match — which `eval.rs`'s allow test does not admit. So the
/// caller asks about the table, the evaluator answers about the catalog, and
/// the page says **Allowed** with full confidence. Refusing the body is the
/// only answer that cannot mislead: there is no engine request this shape
/// corresponds to.
#[tokio::test]
async fn a_target_that_skips_a_level_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    for (label, target) in [
        (
            "a table under no schema",
            json!({ "catalog_id": f.catalog_id, "schema": null, "table": "orders", "column": null }),
        ),
        (
            "a column under no table",
            json!({ "catalog_id": f.catalog_id, "schema": "sales", "table": null, "column": "amount" }),
        ),
        (
            "a column under no schema either",
            json!({ "catalog_id": f.catalog_id, "schema": null, "table": null, "column": "amount" }),
        ),
    ] {
        let mut ask = explain_body(f.catalog_id, "alice-doe");
        ask["target"] = target;
        let (status, body) = into_json(
            f.app()
                .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{label} names no resource any engine request could carry, and answering \
             it would answer about the nearest named ANCESTOR instead — silently, and \
             in the allow direction; body: {body}"
        );
    }

    // The shapes that DO name a resource keep working, so the guard cannot be
    // satisfied by refusing everything.
    for (label, target) in [
        (
            "catalog only",
            json!({ "catalog_id": f.catalog_id, "schema": null, "table": null, "column": null }),
        ),
        (
            "catalog and schema",
            json!({ "catalog_id": f.catalog_id, "schema": "sales", "table": null, "column": null }),
        ),
        (
            "down to the column",
            json!({ "catalog_id": f.catalog_id, "schema": "sales", "table": "orders", "column": "amount" }),
        ),
    ] {
        let mut ask = explain_body(f.catalog_id, "alice-doe");
        ask["target"] = target;
        let (status, body) = into_json(
            f.app()
                .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{label} is a resource an engine request can carry; body: {body}"
        );
    }
}

/// **A level present as `""` or `"*"` is refused, and this is the HTTP half.**
/// `validate_explain_target`'s own matrix lives in `src/routes.rs`; what this
/// adds is that the handler CALLS it — delete the call and every row below turns
/// 200 while the lib tests stay green.
///
/// Both forms are Ranger's `isAllValuesRequested` ("every value at this level"),
/// not a resource name, and the reasons they must not be answered differ:
///
/// * `""` is the one request shape where **egret's own evaluator is measured
///   disagreeing with the engine**. `egret_ranger_eval::resource::level_excluded_by_trie`
///   records it: on ranger 2.8.0 a request `{catalog: ""}` reaches a policy whose
///   only level is a literal and answers `Allowed`, while the trie candidacy
///   filter reports that policy absent. That gap is deliberately left open there
///   (the rule separating it from the shapes the trie gets right is not
///   established, and that function's job is to be a proof) — so the honest place
///   to close it is the surface that lets a caller choose the request.
///   ⚠ The same comment used to say an empty request level "is not something
///   egret sends", bounding the shape by AUTHORSHIP. `request_resource` copies
///   the body through verbatim, so it was false; this test is what makes it true.
/// * `"*"` the trie and the matcher agree about, and the answer is still about
///   another question: `isAllValuesRequested && !isMatchAny` short-circuits to
///   NONE without reading one policy value, so `{schema: "*"}` reports "nothing
///   matched" for every policy naming a real schema. `ScopePicker`'s Schema /
///   Table / Column are free text and `*` is exactly what a steward sees in a
///   compiled grant's resources, so this is a shape ORDINARY use produces — the
///   empty one it cannot, because the picker maps its own empty input to
///   `undefined`.
#[tokio::test]
async fn a_target_level_naming_every_value_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    for value in ["", "*"] {
        for (label, target) in [
            (
                "schema",
                json!({ "catalog_id": f.catalog_id, "schema": value, "table": null, "column": null }),
            ),
            (
                "table",
                json!({ "catalog_id": f.catalog_id, "schema": "sales", "table": value, "column": null }),
            ),
            (
                "column",
                json!({ "catalog_id": f.catalog_id, "schema": "sales", "table": "orders", "column": value }),
            ),
        ] {
            let mut ask = explain_body(f.catalog_id, "alice-doe");
            ask["target"] = target;
            let (status, body) = into_json(
                f.app()
                    .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{label} = {value:?} is Ranger's every-value form, not a resource name, \
                 and no engine request carries it; body: {body}"
            );
            assert!(
                body["error"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&format!("`{label}`")),
                "the refusal has to name WHICH level: {body}"
            );
        }
    }

    // A `*` INSIDE a name, and whitespace, are literal values the matcher
    // compares literally — so they stay answerable, and the guard cannot be
    // satisfied by refusing anything that merely contains a star.
    for target in [
        json!({ "catalog_id": f.catalog_id, "schema": "a*b", "table": null, "column": null }),
        json!({ "catalog_id": f.catalog_id, "schema": " ", "table": null, "column": null }),
    ] {
        let mut ask = explain_body(f.catalog_id, "alice-doe");
        ask["target"] = target.clone();
        let (status, body) = into_json(
            f.app()
                .oneshot(req("POST", "/perm/explain", &token, Some(ask)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{target} is a literal resource value, not the all-values form; body: {body}"
        );
    }
}

/// **A catalog id naming no row is refused — and that is NOT the same thing as a
/// catalog no profile serves.**
///
/// `services_for_catalog_via` joins on `ea.catalog_id = $1`, so a uuid naming
/// nothing returns the empty profile set, which the handler reported as
/// `no_profile_serves_the_catalog` — a steward reading that for a typo believes
/// they have learned something about enforcement.
///
/// ⚠ **The unserved outcome is deliberately kept.** "Nothing is enforcing here"
/// is a real answer a steward asks for on purpose
/// (`a_catalog_no_engine_serves_is_its_own_outcome_not_a_denial`), so both halves
/// are asserted in ONE test: a uuid naming nothing is a 400, and in the same
/// breath a catalog that EXISTS and that no engine serves still answers 200 with
/// that outcome. Anyone who later "tidies" the unserved case into a refusal has
/// to delete a line of this test to do it.
#[tokio::test]
async fn a_catalog_id_naming_no_catalog_is_refused_while_an_unserved_one_is_answered() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let nowhere = Uuid::new_v4();
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(nowhere, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a uuid no registered catalog has names no resource at all; body: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains(&nowhere.to_string()),
        "and the refusal names the id, so the steward can see it was the typo: {body}"
    );

    // The distinction this guard exists to make, asserted here so it cannot be
    // removed silently.
    let real_but_unserved = orphan_catalog(&f, "explain-exists-unserved").await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(real_but_unserved, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a catalog that EXISTS and that no profile serves is a legitimate question \
         and must keep its own outcome; body: {body}"
    );
    assert_eq!(
        body["outcome"], "no_profile_serves_the_catalog",
        "which is the answer, not an error; body: {body}"
    );
}

/// **The decision this task recorded: a platform steward may explain a catalog of
/// ANY workspace, and that is by construction, not by oversight.**
///
/// `explain` is mounted under `/api/control-plane` with the bare `Auth`
/// extractor, so no workspace id reaches it; `egret_core::Principal` has no
/// workspace field; `policy:read` is `Scope::Platform` in `egret_core::rbac` and
/// is gated on `Resource::Platform`, which `egret_auth::grants::grants_for` loads
/// with `ws = None` — admitting only `scope_type = 'platform'` rows. **A
/// workspace-confined caller holding `policy:read` is not representable.** And
/// `registry.catalogs` has no `workspace_id`: a workspace is *assigned* a
/// catalog, many to many, so "the catalog's workspace" is not a function.
///
/// So this asserts the positive: a `policy:read` holder — not a platform_admin,
/// whose PDP bypass would make the measurement vacuous — gets an answer about a
/// catalog assigned to a workspace it has nothing to do with.
#[tokio::test]
async fn a_platform_steward_may_explain_a_catalog_of_any_workspace() {
    let Some(mut f) = setup().await else { return };
    let other_ws = new_workspace(&f, "perm-task4-foreign").await;
    assign_catalog_to_workspace(&f, f.catalog_id, other_ws).await;
    let token = session_with_permission(&mut f, "policy:read").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a platform steward's remit is the installation: refusing this would break \
         the Explain access page for every catalog, since the caller has no \
         workspace to compare against; body: {body}"
    );
    assert_eq!(body["outcome"], "per_profile", "body: {body}");
}

/// **`confine_to_overlay_workspace`, called DIRECTLY, because no route reaches
/// it any more.**
///
/// ⚠ **No HTTP entry point can reach this function.** The mechanism, stated
/// instead of "nothing does this today" — a claim of that shape has been written
/// about this guard twice and falsified twice:
///
/// 1. Since `391c3c3a6` (2026-10-10) `SessionService::validate` re-evaluates
///    `egret_auth::impersonation::overlay_refused_by_platform_authority` on
///    **every request**, so a ws-scoped overlay over a target holding any
///    platform-scoped role grant is **inert**: the request is decided as the
///    admin and `Principal::impersonator` is `None`, which makes
///    `overlay_workspace_scope` here `None` and this guard a no-op.
/// 2. A ws-scoped overlay whose target holds NO platform-scoped role grant stays
///    live, but then it cannot pass `require("policy:read"/"policy:update")`
///    above this function: those keys are `Scope::Platform`, the PDP loads
///    `Resource::Platform` with `ws = None`, and
///    `egret_auth::grants::AccessGrantStore::grants_for` admits a `workspace`-
///    scoped row only for `scope_id = $ws`, which NULL never matches (pinned by
///    `grants_for_returns_platform_and_scoped_workspace_grants` in `egret-auth`).
///
/// Those two cases exhaust `workspace_scope = Some(_)`, so the guard is defence
/// in depth and nothing more. It is kept because it is keyed to a different
/// relation (`registry.catalog_workspaces`) than the inertness rule is, and
/// because it is the only half that would still hold if some future entry point
/// wrote a scoped overlay row without going through either.
///
/// **It WAS reachable, before `391c3c3a6`, through the stale-measurement
/// window.** `validate_impersonation_target` measured the target's platform roles
/// once, when the overlay row was written, while `validate` rebuilt the principal
/// from live tables for the overlay's whole 30-minute life — so a target who
/// gained a platform role AFTER the overlay began (an IdP group change replayed by
/// `sync_user_oidc_groups` on the next login, or a console grant) handed a
/// `ws_admin`'s own token a principal carrying `policy:update` with
/// `workspace_scope = Some(A)`, and this function was the thing standing between
/// it and another workspace's catalog.
///
/// **Why the test moved off the router.** Two route-level tests drove this guard
/// by writing the overlay row straight into the database
/// (`a_workspace_scoped_overlay_is_confined_to_that_workspaces_catalogs`,
/// `…_cannot_create_a_grant_on_another_workspaces_catalog`). After `391c3c3a6`
/// their ALLOW arm could not hold — the overlay is inert, so the principal is a
/// permissionless admin and every row is `403` — which is exactly how an
/// unreachable guard announces itself. A test whose fixture cannot hold the state
/// it guards is not a test, and a pair of rows that only ever observe `403` cannot
/// tell a correct guard from `return Err(Forbidden)`. The router now pins the
/// INERTNESS instead
/// (`a_ws_scoped_overlay_over_a_platform_role_target_is_decided_as_the_admin`),
/// at the layer the escalation actually lived on.
///
/// Four cells, because the predicate distinguishes allow from refuse and both
/// halves have to be observed:
/// 1. confined to the workspace the catalog IS assigned to → `Ok`;
/// 2. confined to a workspace the catalog is NOT assigned to → `Forbidden`;
/// 3. an UNSCOPED overlay (a `platform_admin` acting as someone) → `Ok` for that
///    same catalog, so the confinement is keyed to the SCOPE and not merely to
///    the presence of an overlay;
/// 4. no overlay at all → `Ok`, likewise.
///
/// ⚠ **Both catalogs carry a `registry.catalog_workspaces` row.** A "foreign"
/// catalog with no assignment at all let Task 4's M8 mutant survive — an `EXISTS`
/// that dropped `AND workspace_id = $2`, i.e. "is this catalog assigned to ANY
/// workspace", still produced the refusal — and that mutant is the actual leak.
#[tokio::test]
async fn confine_to_overlay_workspace_allows_its_own_workspace_and_refuses_another() {
    let Some(f) = setup().await else { return };
    let scope = new_workspace(&f, "perm-confine-scope").await;
    let own = orphan_catalog(&f, "perm-confine-own-cat").await;
    assign_catalog_to_workspace(&f, own, scope).await;
    let foreign_ws = new_workspace(&f, "perm-confine-foreign-ws").await;
    let foreign = orphan_catalog(&f, "perm-confine-foreign-cat").await;
    assign_catalog_to_workspace(&f, foreign, foreign_ws).await;

    let mut conn = f.db.pool.acquire().await.expect("acquire a connection");
    let confined = principal_under_overlay(Some(scope));

    // 1. The overlay's own workspace is assigned this catalog.
    let own_cell = confine_to_overlay_workspace(&mut conn, &confined, own).await;
    assert!(
        own_cell.is_ok(),
        "the overlay's own workspace is assigned this catalog, so the support flow \
         the overlay exists for must still be allowed — a blanket refusal would \
         pass cell 2 alone; got {own_cell:?}"
    );

    // 2. A catalog that workspace is not assigned.
    let foreign_cell = confine_to_overlay_workspace(&mut conn, &confined, foreign).await;
    assert!(
        matches!(foreign_cell, Err(PermHttpError::Forbidden)),
        "a ws-scoped overlay must not reach another workspace's catalog through a \
         platform route; got {foreign_cell:?}"
    );

    // 3. An UNSCOPED overlay is a platform_admin acting as someone: unrestricted.
    let unscoped = principal_under_overlay(None);
    let unscoped_cell = confine_to_overlay_workspace(&mut conn, &unscoped, foreign).await;
    assert!(
        unscoped_cell.is_ok(),
        "an unscoped overlay carries no workspace to confine to, so cell 2 must be \
         the SCOPE and not the presence of an overlay; got {unscoped_cell:?}"
    );

    // 4. No overlay at all.
    let plain = principal_without_overlay();
    let plain_cell = confine_to_overlay_workspace(&mut conn, &plain, foreign).await;
    assert!(
        plain_cell.is_ok(),
        "a platform steward with no overlay has the whole installation as a remit; \
         got {plain_cell:?}"
    );
}

/// [`confine_rule_selector`] directly, for the same reason and with the same
/// unreachability — read the ⚠ paragraphs on
/// `confine_to_overlay_workspace_allows_its_own_workspace_and_refuses_another`
/// before changing anything here.
///
/// A rule's target is a `Selector`, not a catalog, and the two arms fail
/// differently:
///
/// 1. `Path` on a catalog the overlay's workspace IS assigned → `Ok`;
/// 2. `Path` on a catalog it is not → `Forbidden` (the `Path` arm is just
///    `confine_to_overlay_workspace`, so this cell is what proves the delegation
///    is wired at all);
/// 3. `Tag` under a ws-scoped overlay → `Forbidden` **regardless of catalog**: a
///    tag selector names none, and `affected_services_for_rule` fans it out over
///    every profile in the installation, so there is no subset of it that stays
///    inside one workspace;
/// 4. `Tag` with no overlay → `Ok`, so cell 3 is the confinement and not a route
///    that refuses tag rules outright;
/// 5. `Path` on the foreign catalog with no overlay → `Ok`, the control for cell 2.
#[tokio::test]
async fn confine_rule_selector_refuses_a_foreign_path_and_every_tag() {
    let Some(f) = setup().await else { return };
    let scope = new_workspace(&f, "perm-rulesel-scope").await;
    let own = orphan_catalog(&f, "perm-rulesel-own-cat").await;
    assign_catalog_to_workspace(&f, own, scope).await;
    let foreign_ws = new_workspace(&f, "perm-rulesel-foreign-ws").await;
    let foreign = orphan_catalog(&f, "perm-rulesel-foreign-cat").await;
    assign_catalog_to_workspace(&f, foreign, foreign_ws).await;

    let path_on = |catalog_id: Uuid| {
        Selector::Path(egret_perm::model::Scope {
            catalog_id,
            schema: Some("sales".to_string()),
            table: Some("orders".to_string()),
            column: Some("email".to_string()),
        })
    };
    let tag = Selector::Tag {
        key: f.tag_key.clone(),
    };

    let mut conn = f.db.pool.acquire().await.expect("acquire a connection");
    let confined = principal_under_overlay(Some(scope));
    let plain = principal_without_overlay();

    // 1 and 2: the Path arm delegates to the catalog confinement.
    let own_cell = confine_rule_selector(&mut conn, &confined, &path_on(own)).await;
    assert!(
        own_cell.is_ok(),
        "a confined caller must still author rules over its own workspace's \
         catalogs; got {own_cell:?}"
    );
    let foreign_cell = confine_rule_selector(&mut conn, &confined, &path_on(foreign)).await;
    assert!(
        matches!(foreign_cell, Err(PermHttpError::Forbidden)),
        "a confined caller must not author a rule over another workspace's \
         catalog; got {foreign_cell:?}"
    );

    // 3: a tag selector has no catalog and fans out over the installation.
    let tag_cell = confine_rule_selector(&mut conn, &confined, &tag).await;
    assert!(
        matches!(tag_cell, Err(PermHttpError::Forbidden)),
        "a tag rule's affected set is every profile in the installation, so a \
         confined caller may not author one in any form; got {tag_cell:?}"
    );

    // 4 and 5: unconfined, both selectors are a platform steward's to author.
    let plain_tag = confine_rule_selector(&mut conn, &plain, &tag).await;
    assert!(
        plain_tag.is_ok(),
        "without an overlay a tag rule is a platform steward's to author, so cell \
         3 is the confinement and not a blanket refusal of tag rules; got \
         {plain_tag:?}"
    );
    let plain_path = confine_rule_selector(&mut conn, &plain, &path_on(foreign)).await;
    assert!(
        plain_path.is_ok(),
        "without an overlay the foreign catalog is in remit, so cell 2 is the \
         confinement and not a missing permission; got {plain_path:?}"
    );
}

/// **The escalation's regression test, at the HTTP layer it lived on.**
///
/// A `ws_admin` of workspace A holds an overlay on a member of A who holds the
/// platform `data_steward` role — which migration 0059 seeds with the whole
/// `policy:*` family. Before `391c3c3a6` (2026-10-10) that token reached
/// `POST /perm/grants` carrying the TARGET's `policy:update` and wrote a grant on
/// another workspace's catalog, compiled by the same request into the Ranger
/// artifact that workspace's engine downloads. egret IS the Ranger admin, so that
/// was a data-plane compromise, and the creation-time guard did not stop it: the
/// overlay began over a clean target and `validate` rebuilt the principal from
/// live tables for its whole 30-minute life, so an IdP group change replayed by
/// `sync_user_oidc_groups` — needing no permission from the admin — supplied the
/// role afterwards.
///
/// What holds now is INERTNESS, and it is a property of
/// `SessionService::validate`, not of any check on this router: the overlay is
/// re-judged every request and falls through to the admin principal. So the
/// assertions are about WHO the request is decided as.
///
/// Three observations, because "refused" alone cannot tell inertness from a route
/// that refuses everything:
/// 1. resolving the token yields the ADMIN — `impersonator` is `None` and the
///    target's platform roles are absent — which is the mechanism, read directly;
/// 2. the escalating write is `403` and **nothing is stored or compiled**,
///    asserted against `perm.grants` AND `policy.authz_policies`, because the 403
///    would be worth little if the row had already reached the artifact;
/// 3. giving the ADMIN the same `policy:update` makes the same token, with the
///    same overlay row over the same target, succeed — so row 2 is the missing
///    authority of an inert overlay and not a session with an overlay row being
///    refused wholesale.
///
/// The foreign catalog is the fixture's own `f.catalog_id` on purpose: it is
/// attached to an engine bound to `f.service_id`, so a grant on it compiles to a
/// real policy row and row 2's `policy.authz_policies` assertion could actually
/// have changed.
#[tokio::test]
async fn a_ws_scoped_overlay_over_a_platform_role_target_is_decided_as_the_admin() {
    let Some(mut f) = setup().await else { return };
    // The overlay's own workspace, with a catalog of its own so row 3 is askable.
    let scope = new_workspace(&f, "perm-inert-scope").await;
    let own = orphan_catalog(&f, "perm-inert-own-cat").await;
    assign_catalog_to_workspace(&f, own, scope).await;
    // The other workspace's catalog: served by `f.service_id`, and assigned.
    let foreign_ws = new_workspace(&f, "perm-inert-foreign-ws").await;
    assign_catalog_to_workspace(&f, f.catalog_id, foreign_ws).await;

    let grants_before = grants_on(&f, f.catalog_id).await;
    let policies_before = policies_on(&f, f.service_id).await;

    let (token, admin_uid) =
        ws_scoped_overlay_session_with(&mut f, scope, &["policy:update"]).await;

    // 1. The mechanism, read directly off `validate`.
    let p = f
        .sessions
        .validate(&token)
        .await
        .expect("validate the session")
        .expect("the session is live");
    assert_eq!(
        p.user_id.0, admin_uid,
        "a ws-scoped overlay over a platform-role-holding target must be INERT: \
         `validate` re-evaluates `overlay_refused_by_platform_authority` every \
         request and falls through to the admin principal"
    );
    assert!(
        p.impersonator.is_none(),
        "an inert overlay must leave no `Impersonator` behind — a principal \
         carrying `workspace_scope = Some(_)` on a platform route is the shape \
         nothing confines"
    );
    assert!(
        p.platform_roles.is_empty(),
        "the admin's own principal must be rebuilt, not the target's: the target's \
         platform roles are what the escalation borrowed"
    );

    // 2. The escalating write: refused, and nothing reaches the artifact.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the admin holds no platform permission of their own, so an inert overlay \
         must not lend them the target's `policy:update`; body: {body}"
    );
    assert_eq!(
        grants_on(&f, f.catalog_id).await,
        grants_before,
        "the refused grant must not have been stored"
    );
    assert_eq!(
        policies_on(&f, f.service_id).await,
        policies_before,
        "the refused grant must not have reached the artifact the engine downloads"
    );

    // 3. Same token, same overlay row, same target — but now the ADMIN holds the
    //    permission themselves. Accepted, so row 2 was the inert overlay's missing
    //    authority and not a blanket refusal.
    grant_platform_permission(&mut f, admin_uid, "policy:update").await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(own, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "an admin with their OWN `policy:update` still authors grants while an \
         inert overlay row sits on their session — row 2 must be the authority, \
         not the overlay row's existence; body: {body}"
    );
}

// ---------------------------------------------------------------------------
// SP4d debt Task 13: the compiled-output oracle the overlay tests above use
// ---------------------------------------------------------------------------

/// Compiled policies under the fixture's own profile — the table the engine's
/// Ranger plugin downloads. A refused *write* is only interesting if nothing
/// reached this, so the test asserts on it and not on `perm.grants` alone.
async fn policies_on(f: &Fixture, service_id: i64) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
        .bind(service_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count policies")
}

/// **SP4d-2b Task 7: the refusal lifts, and this is where it is pinned.**
///
/// SP4d-2a shipped `resolved_tags: None` unconditionally, so a profile whose tag
/// service carried any policy at all answered `Undecidable` for EVERY access on
/// EVERY catalog it serves. `explain` now resolves the tags, and here there are
/// none recorded in this catalog at all — so the set is `Some(empty)`, which is
/// Ranger's own measured narrowing (`getLikelyMatchPolicyEvaluators` returns
/// `Collections.EMPTY_LIST` at its `isNotEmpty(tags)` branch) and the profile
/// ANSWERS.
///
/// Three earlier versions of this test are worth knowing about, because each
/// change was a narrowing rather than a weakening. It first asserted the
/// evaluator refused the `tagPolicies` block on the ENVELOPE, naming its policy
/// COUNT; SP4d-2b Task 3 made the block EVALUATED, so the refusal that stood was
/// the phase's own `tags_unresolved`; Task 7 supplies the set, so what stands is
/// an answer. What has to stay true is that an answer is never GUESSED — the two
/// tests below pin the states that still refuse, and the empty set here is the
/// one state where answering is a measurement.
#[tokio::test]
async fn a_tag_policy_block_with_no_tags_recorded_answers_through_the_narrowing() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, _starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-tagblock").await;
    bind_tag_service_with_one_policy(&f, trino_id, "anything").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "trino"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "with no tag recorded on this catalog the tag phase is narrowed away and \
         the resource policies answer; body: {body}"
    );
    // Said explicitly, because "denied" alone would also be satisfied by a
    // verdict that reached the tag phase and found nothing: `tags_unresolved` is
    // the cause this task exists to stop producing on this state.
    assert!(
        verdict["decision"]["why"].is_null(),
        "a decided verdict carries no causes at all; body: {body}"
    );
    // And no decline reason, because nothing declined — the field is the page's
    // signal that `tags_unresolved` has a reason to render.
    assert!(
        body["tag_resolution_declined"].is_null(),
        "a resolved target reports no decline; body: {body}"
    );

    // The StarRocks profile alongside it has no tag service at all, and decides
    // as it always did — the change is scoped to the profile that had the block.
    assert_eq!(
        access_for(profile_for(&body, "starrocks"), "select")["decision"]["kind"],
        "denied",
        "body: {body}"
    );
}

/// **The fail-open the gate closes, end to end — and the half of it the DATABASE
/// now closes instead.**
///
/// Two assertions, in the order the fail-open is now blocked:
///
/// 1. **The empty-level row cannot be stored.** It used to be: `('sales','','')`
///    ships as a `RangerServiceResource` naming `{catalog, schema}` —
///    `ServiceTags::from_assignments` omits an empty level — which the engine's
///    enricher treats as an ANCESTOR of `sales.orders` and Ranger ADMITS for a tag
///    request, while `resolved_tags_for_resource_via`'s exact `table_name = $3`
///    can never return it. So egret's set was MISSING a tag the engine enforces,
///    and an empty set is the licence to skip the tag phase entirely. SP4d-2b
///    detected that at read time; migration 0177's
///    `catalog_tag_assignments_levels_are_present_and_literal` makes it
///    unstorable, which is why this test now asserts the INSERT is refused and
///    names the constraint that refused it. The remark this test used to carry —
///    "the row is storable today: `assign_resource_tag` validates nothing and
///    migration 0081 has no `CHECK`" — is exactly what stopped being true.
/// 2. **The class the schema must NOT take still declines, end to end.** A
///    character on which Postgres `lower()` and Java `equalsIgnoreCase` can
///    disagree is a legitimate identifier, so it stays storable and stays gated in
///    Rust. Before this task that class had NO end-to-end instrument: all three
///    e2e declines went through the empty-level row. Remove the gate call and this
///    half goes green-but-wrong in the fail-open direction.
#[tokio::test]
async fn the_schema_refuses_the_empty_level_and_the_fold_gate_still_declines() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, _starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-broadrow").await;
    bind_tag_service_with_one_policy(&f, trino_id, "anything").await;

    // 1. A SCHEMA-level assignment: `table_name = ''`, the level egret's storage
    //    has no convention for and its resolver cannot roll up. The DATABASE is
    //    what refuses it now, for every writer — this raw `sqlx::query`, the
    //    `PUT …/resource-tags` route, and `psql`.
    let e = try_assign_tag(&f, catalog_id, "sales", "", "")
        .await
        .expect_err("migration 0177 must refuse a row with an empty table_name");
    let named = match &e {
        sqlx::Error::Database(d) => {
            d.constraint() == Some("catalog_tag_assignments_levels_are_present_and_literal")
        }
        _ => false,
    };
    assert!(
        named,
        "the refusal must be migration 0177's CHECK, not a typo or a dropped \
         connection: {e}"
    );

    // 2. …and the surviving class, which no CHECK may take: the Turkish dotless i
    //    in a stored column name. `('sales','orders','emaıl')` is a row the
    //    matcher's per-character fold and the resolver's full-string `lower()` can
    //    disagree about, so neither direction of the comparison is safe and the
    //    whole resolution must decline.
    assign_tag(&f, catalog_id, "sales", "orders", "emaıl").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "trino"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "undecidable",
        "a stored assignment egret cannot reproduce must refuse, not be answered \
         from a set it is missing from; body: {body}"
    );
    assert!(
        verdict["decision"]["why"]
            .as_array()
            .expect("why")
            .iter()
            .any(|c| c["kind"] == "tags_unresolved"),
        "no tags_unresolved cause in {body}"
    );
    // **Fix round 3: the REASON reaches the wire, and names the row.** The cause
    // alone cannot say which state produced it — the page rendered one sentence for
    // every one of them, naming a stored row that did not exist — so the response
    // carries the reason and the coordinate beside it.
    assert_eq!(
        body["tag_resolution_declined"]["kind"], "stored_fold_divergent",
        "the decline's reason must reach the response; body: {body}"
    );
    assert_eq!(
        body["tag_resolution_declined"]["column"], "emaıl",
        "and the coordinate must be the offending one, so the page can name it; body: {body}"
    );
}

/// **The other half of the lift: a real tag, a real tag policy, a real answer.**
/// A TABLE-level assignment is `SELF` for a request naming that table, the tag
/// policy's `resources.tag.values` names that tag, and its deny item names the
/// subject — so the engine's tag phase denies and so does `explain`. Before Task
/// 7 this was `tags_unresolved`, which is the whole cost the task removes: the
/// steward could not see that a tag policy was the thing deciding.
#[tokio::test]
async fn a_resolved_tag_lets_the_tag_policy_itself_decide() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, _starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-tagdecides").await;
    let tag = assign_tag(&f, catalog_id, "sales", "orders", "").await;
    bind_tag_service_with_one_policy(&f, trino_id, &tag.name).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "trino"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "the tag policy is what decides, and it must be able to; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "deny_policy_item",
        "and it decides through its deny item, not by nothing matching; body: {body}"
    );
}

/// Both halves of one fixture tag row, and they are DIFFERENT strings by
/// construction (`t7key_…` vs `t7tag_…`).
///
/// # ⚠ The `key` is carried so a test can assert it is ABSENT
///
/// `tags.name` is what the tag WIRE carries as the tag's `type`, and therefore
/// what a tag policy's `resources.tag.values` must name and what `explain`'s tag
/// list must report. `tags.key` is what that list used to report, and live the two
/// differ — the fail-open D3 names, and the F4/F5 defect the browser measured.
///
/// An assertion that only names the expected NAME cannot distinguish "reports the
/// name" from "reports the key of a row whose two columns happen to agree", so
/// name-not-key would be pinned by the happy path alone. Holding both strings lets
/// the test say the key appears NOWHERE in the response, which is a statement with
/// content only because `unique()` guarantees they differ.
struct AssignedTag {
    key: String,
    name: String,
}

/// One `tags` row plus one `catalog_tag_assignments` row on this catalog's
/// warehouse, returning both of the row's identifiers — see [`AssignedTag`] for
/// why the key comes back too.
async fn assign_tag(
    f: &Fixture,
    catalog_id: Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> AssignedTag {
    let warehouse_id: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("catalog warehouse");
    let key = unique("t7key");
    let name = unique("t7tag");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(&key)
            .bind(&name)
            .fetch_one(&f.db.pool)
            .await
            .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .execute(&f.db.pool)
    .await
    .expect("tag assignment");
    AssignedTag { key, name }
}

/// [`assign_tag`] without the `expect`, for the one test whose subject is the
/// INSERT being REFUSED. Separate rather than making `assign_tag` return a
/// `Result`: every other call site is asserting the row lands, and an ignored
/// `Result` at any of them would be a silently missing fixture.
async fn try_assign_tag(
    f: &Fixture,
    catalog_id: Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<(), sqlx::Error> {
    let warehouse_id: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("catalog warehouse");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(unique("t3key"))
            .bind(unique("t3tag"))
            .fetch_one(&f.db.pool)
            .await
            .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .execute(&f.db.pool)
    .await
    .map(|_| ())
}

/// The tag service the Trino profile's plugin embeds as `tagPolicies`, carrying
/// ONE ACCESS policy on `tag_value` that denies `nobody` — the shape
/// `download_policies` actually serves.
async fn bind_tag_service_with_one_policy(f: &Fixture, trino_id: i64, tag_value: &str) {
    egret_policy::seed_tag(&f.policy)
        .await
        .expect("seed tag def");
    let trino = f
        .policy
        .get_service_by_id(trino_id)
        .await
        .expect("trino service row");
    let tag_service_name = format!("{}_tag", trino.name);
    let tag_service = f
        .policy
        .create_service(&tag_service_name, "tag", &json!({}))
        .await
        .expect("create tag service");
    let tag_policy_name = unique("ops-tag-policy");
    f.policy
        .create_policy(
            tag_service.id,
            &tag_policy_name,
            &json!({
                "name": tag_policy_name,
                "service": tag_service_name,
                "serviceType": "tag",
                "policyType": 0,
                "isEnabled": true,
                "resources": { "tag": { "values": [tag_value], "isExcludes": false, "isRecursive": false } },
                "policyItems": [],
                "denyPolicyItems": [{
                    "users": ["nobody"],
                    "accesses": [{"type": "select", "isAllowed": true}],
                }],
                "allowExceptions": [],
                "denyExceptions": [],
                "isDenyAllElse": false,
            }),
        )
        .await
        .expect("create tag policy");
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(trino_id)
        .bind(json!({ "tagService": tag_service_name }))
        .execute(&f.db.pool)
        .await
        .expect("bind the tag service");
}

/// **The drift alarm, restored.** Rerouting `explain` through the artifact
/// removes the three `explain_and_the_compiled_policy_agree_*` tests' reason
/// for existing — `explain` no longer answers with `egret_perm::eval`. This
/// puts the alarm back from the other side: on the happy path the artifact's
/// answer and the model oracle must still agree, for a granted subject and
/// for a stranger. A disagreement here is the model and the engine parting
/// company, which is the class of defect the whole package exists to make
/// impossible to ship silently.
///
/// **The second target differs from the grant only in case** (design R6):
/// the grant names `sales.orders`, the target `sales.Orders`. The engine folds
/// every resource level (`ignoreCase: true`) and the artifact evaluator folds
/// with it, so a model oracle comparing exactly would deny the granted subject
/// here while the artifact allows them. `egret_perm::eval::resource_level_matches`
/// is what makes the two agree.
#[tokio::test]
async fn the_artifact_answer_and_the_model_oracle_agree_on_the_happy_path() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let granted = known_user(&f, unique("oracle-granted")).await;
    let stranger = unique("oracle-stranger");
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, &granted)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");

    let grants = f.perm.list_grants().await.expect("grants");
    let grants: Vec<_> = grants
        .into_iter()
        .filter(|g| g.scope.catalog_id == f.catalog_id)
        .collect();
    for (table, user, expected) in [
        ("orders", &granted, true),
        ("orders", &stranger, false),
        ("Orders", &granted, true),
        ("Orders", &stranger, false),
    ] {
        let target = egret_perm::Scope {
            catalog_id: f.catalog_id,
            schema: Some("sales".into()),
            table: Some(table.into()),
            column: None,
        };
        let oracle = egret_perm::eval::evaluate(
            &egret_perm::Subject {
                user: user.clone(),
                groups: vec![],
                roles: vec![],
                workspaces: vec![],
            },
            egret_perm::Action::Read,
            &target,
            &grants,
            &[],
            &[],
            &[],
        );
        assert_eq!(
            oracle.allowed, expected,
            "the model oracle must {expected} {user} on sales.{table}: {oracle:?}"
        );

        let (status, body) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/explain",
                    &token,
                    Some(json!({
                        "subject": { "user": user },
                        "action": "read",
                        "target": {
                            "catalog_id": f.catalog_id,
                            "schema": "sales",
                            "table": table,
                            "column": null,
                        }
                    })),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        let kind = access_for(profile_for(&body, "starrocks"), "select")["decision"]["kind"]
            .as_str()
            .expect("decision kind")
            .to_string();
        assert_eq!(
            kind == "allowed",
            oracle.allowed,
            "the artifact and the model oracle disagree about {user} on sales.{table}: \
             oracle {oracle:?}, artifact {kind}; body: {body}"
        );
    }
}

/// `policy.authz_policies.id` for one compiled policy under a given profile —
/// the DB-assigned id `enabled_policies_for_service` injects into the download
/// body, and the only thing on the wire that can prove WHICH assembler
/// `explain` used.
async fn compiled_policy_id(f: &Fixture, service_id: i64, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(name)
    .fetch_one(&f.db.pool)
    .await
    .unwrap_or_else(|e| panic!("read policy id for {name:?}: {e}"))
}

/// **The assembler `explain` loads a profile's policies with is the same one
/// the download route uses, and this is what makes that falsifiable** (SP4d-2a
/// Task 5 review, M4).
///
/// `enabled_policies_for_service` does two things nothing else does: it injects
/// the DB-assigned `id` (and `version`/`isEnabled`) into each body, because the
/// Ranger policy engine NPEs on a policy whose `id` is null, and it returns
/// only rows with `enabled = true`. The stored `policy_jsonb` carries neither —
/// the compiler's rendered body has no `id` field at all. So each half below
/// pins one of them:
///
/// - the verdict's `policy_id` equals the ROW id, which an assembler that
///   skipped the injection would report as `null`;
/// - a disabled policy does not decide, which an assembler that read every row
///   would let decide (the stored body says `"isEnabled": true`; only the
///   column says otherwise).
///
/// Without these, "the same assembler the download route uses" is maintained by
/// prose and by nobody having changed it — `policy_id` appears in no other
/// assertion in this file, and no other test disables a policy.
#[tokio::test]
async fn explain_reads_the_profile_through_the_download_routes_own_assembler() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let user = known_user(&f, unique("assembler-subject")).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let grant_id = body["id"].as_str().expect("grant id").to_string();
    let policy_name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    let row_id = compiled_policy_id(&f, f.service_id, &policy_name).await;

    // Half 1: the injected id reaches the verdict.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(verdict["decision"]["kind"], "allowed", "body: {body}");
    assert_eq!(
        verdict["decision"]["policy_id"], row_id,
        "the deciding policy must carry the DB-assigned id that ONLY \
         `enabled_policies_for_service` injects — the stored `policy_jsonb` has no \
         `id` field, so an assembler that skipped the injection reports null here; \
         body: {body}"
    );

    // Half 2: a disabled policy is not in the download, so it cannot decide.
    // The stored body still says `"isEnabled": true`; only the COLUMN says
    // otherwise, which is exactly the difference the assembler applies.
    let stored: Value =
        sqlx::query_scalar("SELECT policy_jsonb FROM policy.authz_policies WHERE id = $1")
            .bind(row_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("read stored body");
    assert_eq!(
        stored["isEnabled"], true,
        "the stored body must still claim enabled, or half 2 proves nothing: {stored:#}"
    );
    let res = sqlx::query("UPDATE policy.authz_policies SET enabled = false WHERE id = $1")
        .bind(row_id)
        .execute(&f.db.pool)
        .await
        .expect("disable the policy");
    assert_eq!(res.rows_affected(), 1, "the disable must have landed");

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain_body(f.catalog_id, &user)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "a disabled policy is not served to any plugin, so it cannot decide here \
         either; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "no_policy_matched",
        "body: {body}"
    );
}

/// **SP4d debt Task 5, at the route: a tag-addressed MASK is refused when the
/// tag is already attached to a whole TABLE.**
///
/// Measured live on Trino 470 (SP4d-2b's live gate): that pair makes the engine
/// type-check the one mask against EVERY column of the scanned table, so
/// `SELECT item`, `SELECT id` and even `count(*)` fail — the table is
/// unreadable, not masked. The row filter was measured and ruled out as the
/// cause in the same run.
///
/// **This binary is where the 400 lives**, not
/// `egret-policy/tests/unreadable_table_mask_db.rs`: that one pins the rule's
/// SHAPE against the database, and a route handler's guard is not covered by it
/// — nothing there can tell a refusal that is a 400 naming the remedy from one
/// that is an uncaught error, and nothing there can tell a refusal that rolled
/// the row back from one that stored it.
///
/// Three assertions, and the third is the one a shape-only test cannot make:
/// the refusal is a 400, the message carries the two rule-side remedies (Task 16
/// added a third, edit-the-policy one to the shared sentence), and the rule is NOT
/// in `perm.rules` afterwards.
///
/// The fixture's tag is ALREADY attached to `sales.orders.email` at column
/// level, and this adds a TABLE-level attachment on a different table — so the
/// test also shows that an existing, harmless column attachment does not
/// excuse the table one.
#[tokio::test]
async fn a_tag_mask_rule_is_refused_when_its_tag_is_attached_to_a_whole_table() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    attach_fixture_tag_to_whole_table(&f, "orders_t5a").await;

    let name = unique("t5-tag-mask");
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &token,
                Some(json!({
                    "name": name,
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("sales.orders_t5a"),
        "the refusal must name the table that would become unreadable: {msg}"
    );
    assert!(
        msg.contains("to the columns that need masking instead of to the table"),
        "the refusal must name the attach-to-column remedy: {msg}"
    );
    assert!(
        msg.contains("by catalog/schema/table/column path instead of by tag"),
        "the refusal must name the resource-addressed remedy: {msg}"
    );
    assert_eq!(
        rules_named(&f, &name).await,
        0,
        "the refused rule must not have been stored"
    );
}

/// **The remedy has to keep working, and `preview` has to say so first.**
///
/// The refusal above names two remedies; this drives the second one through the
/// route that applies it. A tag-addressed mask is created while nothing is
/// attached at table level (legal), the tag is then attached to a whole table
/// behind the route's back — which is the state the guard exists to prevent and
/// which `psql` can still reach, so it is also the honest fixture for "a
/// deployment that already holds the pair" — and the PATCH that makes the rule
/// resource-addressed is then asserted to SUCCEED.
///
/// Without this control the obvious over-refusal (gate the guard on "the rule
/// is a tag rule", old or new) would be green: it would refuse the very edit
/// the error message asks the steward to make, and the steward would have no
/// way out of the state at all.
///
/// `preview` is asserted on the same body first, because it funnels through the
/// same `write_rule` and is what the dialog calls before enabling Save — a
/// remedy that previewed as a 400 and then saved fine would be unreachable from
/// the UI.
///
/// Takes [`exclusive`]: creating and patching a TAG-selected rule recompiles
/// every compiler-targeted profile in the database, other tests' included.
#[tokio::test]
async fn the_resource_addressed_remedy_is_accepted_while_the_tag_is_on_a_whole_table() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;

    // 1. A tag-addressed mask, authored while nothing is attached at table level.
    let name = unique("t5-remedy");
    let tag_rule = json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "tag", "key": f.tag_key },
        "exempt": { "include": [], "exclude": [] }
    });
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", &token, Some(tag_rule.clone())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let rule_id = body["id"].as_str().expect("id").to_string();

    // 2. The other half arrives from outside the three guarded routes.
    attach_fixture_tag_to_whole_table(&f, "orders_t5b").await;

    // 3. The dialog previews the remedy...
    let path_rule = json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "path", "catalog_id": f.catalog_id,
                      "schema": "sales", "table": "orders", "column": "email" },
        "exempt": { "include": [], "exclude": [] }
    });
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "id": rule_id, "rule": path_rule })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the preview of the remedy must not be refused; body: {body}"
    );

    // 4. ...and saves it.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/rules/{rule_id}"),
                &token,
                Some(path_rule),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the remedy the refusal names must be applicable; body: {body}"
    );

    // And the rule really is resource-addressed now — the assertion that makes
    // the 200 above mean something.
    let selector: String =
        sqlx::query_scalar("SELECT selector_kind::text FROM perm.rules WHERE id = $1::uuid")
            .bind(&rule_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("read back selector");
    assert_eq!(selector, "path");
}

/// Attach the fixture's tag to the WHOLE table `sales.{table}` in the fixture's
/// own warehouse — migration 0081's `column_name = ''` sentinel, which 0177
/// deliberately keeps legal.
///
/// Raw SQL on purpose: this is the half `egret_bi::CatalogTagRepo::assign_catalog`
/// now refuses from the other direction, and a fixture that went through that
/// route could not set up the state the rule-side guard is being tested on.
async fn attach_fixture_tag_to_whole_table(f: &Fixture, table: &str) {
    let warehouse_id: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("fixture warehouse");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         SELECT id, $1, 'sales', $2, '' FROM tags WHERE key = $3",
    )
    .bind(warehouse_id)
    .bind(table)
    .bind(&f.tag_key)
    .execute(&f.db.pool)
    .await
    .expect("table-level attachment");
}

/// **`explain` evaluates the `tagPolicies` block against the `tag` service-def,
/// whatever the tag service's own `type` is, so it agrees with the engine.**
///
/// The live defect (Trino 483, plugin 2.5.0): a tag service made through the
/// admin API is a `trino` profile, and `download_policies` used to serve the
/// block with THAT type's def. `artifact_via` mirrors the download, so it
/// assembled the same wrong def. Two tags here, each on its own table, and one
/// deny policy on the second: the policy must decide for the table carrying its
/// tag and must NOT decide for the table carrying only the other one.
#[tokio::test]
async fn explain_evaluates_the_tag_block_against_the_tag_def_with_two_tags() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (trino_id, _starrocks_id, catalog_id) = two_profile_catalog(&f, "explain-tagdef").await;
    let key = assign_tag(&f, catalog_id, "sales", "orders", "").await;
    let secret = assign_tag(&f, catalog_id, "sales", "secrets", "").await;
    assert_ne!(key.name, secret.name);

    egret_policy::seed_tag(&f.policy)
        .await
        .expect("seed tag def");
    let trino = f
        .policy
        .get_service_by_id(trino_id)
        .await
        .expect("trino service row");
    // The shape the admin API produces: the tag service is a `trino` profile.
    let tag_service_name = format!("{}_tag", trino.name);
    let tag_service = f
        .policy
        .create_service(&tag_service_name, "trino", &json!({}))
        .await
        .expect("create trino-typed tag service");
    let tag_policy_name = unique("deny-secret");
    f.policy
        .create_policy(
            tag_service.id,
            &tag_policy_name,
            &json!({
                "name": tag_policy_name,
                "service": tag_service_name,
                "serviceType": "tag",
                "policyType": 0,
                "isEnabled": true,
                "resources": { "tag": { "values": [secret.name], "isExcludes": false, "isRecursive": false } },
                "policyItems": [],
                "denyPolicyItems": [{
                    "users": ["nobody"],
                    "accesses": [{"type": "select", "isAllowed": true}],
                }],
                "allowExceptions": [],
                "denyExceptions": [],
                "isDenyAllElse": false,
            }),
        )
        .await
        .expect("create tag policy");
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(trino_id)
        .bind(json!({ "tagService": tag_service_name }))
        .execute(&f.db.pool)
        .await
        .expect("bind the tag service");

    let explain = |table: &str| {
        json!({
            "subject": { "user": "nobody" },
            "action": "read",
            "target": { "catalog_id": catalog_id, "schema": "sales", "table": table, "column": null }
        })
    };

    // The table carrying the policy's tag: the tag policy decides.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain("secrets")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "trino"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "the tag policy must decide for the table carrying its tag; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "deny_policy_item",
        "through its deny item; body: {body}"
    );

    // The table carrying only the OTHER tag: the policy must not decide.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &token,
                Some(explain("orders")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let verdict = access_for(profile_for(&body, "trino"), "select");
    assert_ne!(verdict["decision"]["kind"], "undecidable", "body: {body}");
    assert_ne!(
        verdict["decision"]["reason"], "deny_policy_item",
        "a policy on one tag must not decide for a table carrying only the other; \
         body: {body}"
    );
    assert_ne!(
        verdict["decision"]["policy_name"],
        tag_policy_name.as_str(),
        "body: {body}"
    );
}

// ---------------------------------------------------------------------------
// J-F3: every principal ref must name someone egret knows
// ---------------------------------------------------------------------------
//
// A ref that matches nobody fails CLOSED on an allow include (the grant grants
// nobody) but OPEN on a deny include or on any exclude: the deny or the subtract
// evaporates and the compiled policy allows more than the model states. Every
// write path (create, patch, the rolled-back preview, and so the migration) goes
// through `write_grant`/`write_rule`, so these drive the HTTP routes and assert
// both the refusal and that nothing was stored.

/// A fresh user WITH a minted `users.username`, as `(id, email, username)`.
async fn named_user(f: &Fixture, tag: &str) -> (Uuid, String, String) {
    let id = new_user(f, tag).await;
    let email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&f.db.pool)
        .await
        .expect("read email");
    let username = f
        .users
        .ensure_username(egret_core::UserId(id))
        .await
        .expect("mint username");
    (id, email, username)
}

/// One POST/PATCH through the router.
async fn send(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    into_json(
        f.app()
            .oneshot(req(method, uri, token, Some(body)))
            .await
            .unwrap(),
    )
    .await
}

fn grant_with(catalog_id: Uuid, effect: &str, include: Value, exclude: Value) -> Value {
    json!({
        "effect": effect,
        "actions": ["read"],
        "scope": { "catalog_id": catalog_id, "schema": "sales", "table": "orders", "column": null },
        "principals": { "include": include, "exclude": exclude }
    })
}

/// The stored refs of one grant, in `mode` then `ref` order.
async fn stored_grant_refs(f: &Fixture, grant_id: &str) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT kind::text, ref, mode::text FROM perm.grant_principals \
          WHERE grant_id = $1::uuid ORDER BY mode, ref",
    )
    .bind(grant_id)
    .fetch_all(&f.db.pool)
    .await
    .expect("read grant principals")
}

/// **R2, the finding itself.** `POST /perm/grants` accepted a user's UUID as the
/// ref and compiled it as the Ranger user name, which no engine principal
/// carries. It is refused, and the refusal names the username to use instead —
/// the id resolves to exactly one person, so saying who is the useful answer.
#[tokio::test]
async fn a_user_ref_that_is_a_user_id_is_refused_naming_the_username() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (id, _, username) = named_user(&f, "jf3_by_id").await;
    let before = f.grant_count().await;

    let (status, body) = send(
        &f,
        "POST",
        "/perm/grants",
        &token,
        grant_body(f.catalog_id, &id.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(msg.contains(&id.to_string()), "names the ref; got {msg:?}");
    assert!(
        msg.contains(&format!("{username:?}")),
        "names the username to use instead; got {msg:?}"
    );
    assert_eq!(f.grant_count().await, before, "nothing stored");
}

/// The email twin of the id case: refused, naming the username. (Before J-F3
/// the compiler's `UnusableUserRef` caught an email too, but could not say whose
/// username to use — it has no store handle.)
#[tokio::test]
async fn a_user_ref_that_is_an_email_is_refused_naming_the_username() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (_, email, username) = named_user(&f, "jf3_by_email").await;
    let before = f.grant_count().await;

    // Upper-cased: `users.email` is citext, so the lookup must not depend on case.
    let (status, body) = send(
        &f,
        "POST",
        "/perm/grants",
        &token,
        grant_body(f.catalog_id, &email.to_uppercase()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&format!("{username:?}")),
        "names the username to use instead; got {msg:?}"
    );
    assert_eq!(f.grant_count().await, before, "nothing stored");
}

/// R1, R3, R4: a ref of each kind that names nothing egret knows is a 400 naming
/// the kind and the ref, and nothing is stored.
#[tokio::test]
async fn an_unknown_user_group_role_or_workspace_ref_is_refused_by_name() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let before = f.grant_count().await;

    for (kind, r#ref) in [
        ("user", unique("jf3_nobody")),
        ("group", unique("jf3_no_group")),
        ("role", unique("jf3_no_role")),
        ("workspace_members", Uuid::new_v4().to_string()),
        ("workspace_members", "not-a-uuid".to_string()),
    ] {
        let (status, body) = send(
            &f,
            "POST",
            "/perm/grants",
            &token,
            grant_with(
                f.catalog_id,
                "allow",
                json!([{ "kind": kind, "ref": r#ref }]),
                json!([]),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{kind} {ref:?}: body: {body}"
        );
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains(&format!("{kind} {:?}", r#ref)),
            "{kind}: the refusal names the kind and the ref; got {msg:?}"
        );
        assert!(
            msg.contains("principals.include[0]"),
            "{kind}: and its position; got {msg:?}"
        );
    }
    assert_eq!(f.grant_count().await, before, "nothing stored");
}

/// **The positions that fail OPEN**, refused exactly like the include of an
/// allow — and EVERY unknown ref is named, not just the first, so an author
/// fixing a long principal list does not discover them one round trip at a time.
#[tokio::test]
async fn a_deny_include_and_every_exclude_are_refused_and_every_unknown_ref_is_named() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let before = f.grant_count().await;
    let (a, b, c) = (
        unique("jf3_ghost_a"),
        unique("jf3_ghost_b"),
        unique("jf3_ghost_c"),
    );

    let (status, body) = send(
        &f,
        "POST",
        "/perm/grants",
        &token,
        grant_with(
            f.catalog_id,
            "deny",
            json!([{ "kind": "user", "ref": "analyst" }, { "kind": "user", "ref": a }]),
            json!([{ "kind": "group", "ref": b }, { "kind": "user", "ref": c }]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    for needle in [
        format!("user {a:?} (principals.include[1])"),
        format!("group {b:?} (principals.exclude[0])"),
        format!("user {c:?} (principals.exclude[1])"),
    ] {
        assert!(msg.contains(&needle), "missing {needle:?} in {msg:?}");
    }
    assert!(
        !msg.contains("\"analyst\""),
        "a ref that resolves is not named; got {msg:?}"
    );

    // An ALLOW grant whose only unknown is in the exclude: the subtract would
    // evaporate, so it is refused too.
    let (status, body) = send(
        &f,
        "POST",
        "/perm/grants",
        &token,
        grant_with(
            f.catalog_id,
            "allow",
            json!([{ "kind": "user", "ref": "analyst" }]),
            json!([{ "kind": "user", "ref": c }]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(f.grant_count().await, before, "nothing stored");
}

/// A rule's exemption is a `PrincipalSet` too, and an unknown ref there masks
/// someone the author meant to exempt. Refused on the rule route the same way.
#[tokio::test]
async fn a_rule_exemption_naming_nobody_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let name = unique("jf3_rule");
    let ghost = unique("jf3_ghost");

    let (status, body) = send(
        &f,
        "POST",
        "/perm/rules",
        &token,
        json!({
            "name": name,
            "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
            "selector": { "type": "path", "catalog_id": f.catalog_id,
                          "schema": "sales", "table": "orders", "column": "email" },
            "exempt": { "include": [{ "kind": "user", "ref": ghost }], "exclude": [] }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&format!("user {ghost:?} (exempt.include[0])")),
        "got {msg:?}"
    );
    assert_eq!(rules_named(&f, &name).await, 0, "nothing stored");
}

/// R2's other half: `users.username` is citext, so a username in another case
/// names the same person — and the STORED ref is the stored spelling, because
/// the compiler emits the ref byte-for-byte and the engine's user is the
/// canonical one. Two spellings of one person collapse to one row (the table's
/// primary key would otherwise refuse the write with a 500).
#[tokio::test]
async fn a_username_in_another_case_is_stored_in_its_canonical_spelling() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (_, _, username) = named_user(&f, "jf3_canon").await;

    let id = create_grant_returning_id(
        &f,
        &token,
        grant_with(
            f.catalog_id,
            "allow",
            json!([
                { "kind": "user", "ref": username.to_uppercase() },
                { "kind": "user", "ref": username },
            ]),
            json!([]),
        ),
    )
    .await;
    assert_eq!(
        stored_grant_refs(&f, &id).await,
        vec![("user".to_string(), username.clone(), "include".to_string())]
    );
}

/// The patch and the preview run the same `write_grant`, so they refuse the same
/// refs — and a refused patch leaves the stored row as it was.
#[tokio::test]
async fn a_patch_and_a_preview_naming_nobody_are_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let ghost = unique("jf3_ghost");
    let id = create_grant_returning_id(&f, &token, grant_body(f.catalog_id, "analyst")).await;

    let (status, body) = send(
        &f,
        "PATCH",
        &format!("/perm/grants/{id}"),
        &token,
        grant_with(
            f.catalog_id,
            "allow",
            json!([{ "kind": "user", "ref": "analyst" }]),
            json!([{ "kind": "user", "ref": ghost }]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains(&ghost),
        "body: {body}"
    );
    assert_eq!(
        stored_grant_refs(&f, &id).await,
        vec![(
            "user".to_string(),
            "analyst".to_string(),
            "include".to_string()
        )],
        "the refused patch changed nothing"
    );

    let (status, body) = send(
        &f,
        "POST",
        "/perm/preview",
        &token,
        json!({ "grant": grant_body(f.catalog_id, &ghost) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains(&ghost),
        "body: {body}"
    );
}

/// The refs that DO resolve are accepted: `public` (no `groups` row, and every
/// engine resolves it), a group in `groups`, a role in `roles`, and a username.
/// A `workspace_members` ref naming a REAL workspace is past this check and
/// refused by the shape check instead (no authoring path may write one), which
/// is what tells R4's "unknown workspace" refusal apart from that one.
#[tokio::test]
async fn refs_that_resolve_are_accepted() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let group = unique("jf3_group");
    sqlx::query("INSERT INTO groups (name) VALUES ($1)")
        .bind(&group)
        .execute(&f.db.pool)
        .await
        .expect("insert group");
    let role = unique("jf3_role");
    sqlx::query("INSERT INTO roles (scope, name) VALUES ('workspace', $1)")
        .bind(&role)
        .execute(&f.db.pool)
        .await
        .expect("insert role");

    // In the exclude of a deny — the position that fails open, so the one
    // where a false refusal would be most tempting to "fix" by loosening.
    for exclude in [
        json!([{ "kind": "group", "ref": "public" }]),
        json!([{ "kind": "group", "ref": group }]),
        json!([{ "kind": "role", "ref": role }]),
    ] {
        create_grant_returning_id(
            &f,
            &token,
            grant_with(
                f.catalog_id,
                "deny",
                json!([{ "kind": "user", "ref": "analyst" }]),
                exclude,
            ),
        )
        .await;
    }

    let ws = new_workspace(&f, "jf3_ws").await;
    let (status, body) = send(
        &f,
        "POST",
        "/perm/grants",
        &token,
        grant_with(
            f.catalog_id,
            "allow",
            json!([{ "kind": "workspace_members", "ref": ws }]),
            json!([]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        !msg.contains("principals.include[0]"),
        "a real workspace is not an unknown ref; got {msg:?}"
    );
}

/// **R6 on the patch path** (J-F3 fix round 1). PATCH takes the whole body, so
/// every edit re-sends every principal. A grant that validly excluded a user who
/// was deleted afterwards must stay editable: an actions-only patch is a 200.
/// A NEW unknown ref added by the same patch is still refused — the skip covers
/// what the stored row already holds, nothing else.
#[tokio::test]
async fn a_patch_keeping_a_since_deleted_principal_is_accepted() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let gone = known_user(&f, unique("jf3_gone")).await;
    let body = grant_with(
        f.catalog_id,
        "allow",
        json!([{ "kind": "user", "ref": "analyst" }]),
        json!([{ "kind": "user", "ref": gone }]),
    );
    let id = create_grant_returning_id(&f, &token, body.clone()).await;
    sqlx::query("DELETE FROM users WHERE username = $1")
        .bind(&gone)
        .execute(&f.db.pool)
        .await
        .expect("delete user");

    let mut actions_only = body.clone();
    actions_only["actions"] = json!(["read", "write"]);
    let (status, resp) = send(
        &f,
        "PATCH",
        &format!("/perm/grants/{id}"),
        &token,
        actions_only.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {resp}");

    // The same patch adding a fresh unknown ref is refused, naming only that one.
    let ghost = unique("jf3_ghost");
    let mut adds_ghost = actions_only;
    adds_ghost["principals"]["exclude"] =
        json!([{ "kind": "user", "ref": gone }, { "kind": "user", "ref": ghost }]);
    let (status, resp) = send(
        &f,
        "PATCH",
        &format!("/perm/grants/{id}"),
        &token,
        adds_ghost,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {resp}");
    let msg = resp["error"].as_str().unwrap_or_default();
    assert!(msg.contains(&ghost), "got {msg:?}");
    assert!(
        !msg.contains(&gone),
        "the stored ref is not re-checked; got {msg:?}"
    );

    // Moving the deleted ref to the OTHER list is a new position, so it is checked.
    let mut moved = body;
    moved["principals"]["include"] =
        json!([{ "kind": "user", "ref": "analyst" }, { "kind": "user", "ref": gone }]);
    moved["principals"]["exclude"] = json!([]);
    let (status, resp) = send(&f, "PATCH", &format!("/perm/grants/{id}"), &token, moved).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {resp}");
}

/// **The R6 skip holds only while the effect is unchanged** (J-F3 final review
/// M-a). A legacy allow grant whose include names a user by UUID fails closed
/// (it grants nobody); PATCHing it to `deny` while keeping that ref would store
/// a deny that denies nobody — fail OPEN. That patch re-checks every principal
/// and is refused; an actions-only patch of the same row still passes.
#[tokio::test]
async fn a_patch_flipping_allow_to_deny_rechecks_a_stored_bad_ref() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let target = known_user(&f, unique("jf3_flip")).await;
    let body = grant_with(
        f.catalog_id,
        "allow",
        json!([{ "kind": "user", "ref": target }]),
        json!([]),
    );
    let id = create_grant_returning_id(&f, &token, body).await;
    // The legacy state: the ref stored as the user's id, behind the write path.
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(&target)
        .fetch_one(&f.db.pool)
        .await
        .expect("user id");
    sqlx::query("UPDATE perm.grant_principals SET ref = $1 WHERE grant_id = $2::uuid")
        .bind(uid.to_string())
        .bind(&id)
        .execute(&f.db.pool)
        .await
        .expect("seed legacy ref");
    let legacy = grant_with(
        f.catalog_id,
        "allow",
        json!([{ "kind": "user", "ref": uid.to_string() }]),
        json!([]),
    );

    let mut to_deny = legacy.clone();
    to_deny["effect"] = json!("deny");
    let (status, resp) = send(&f, "PATCH", &format!("/perm/grants/{id}"), &token, to_deny).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {resp}");
    let msg = resp["error"].as_str().unwrap_or_default();
    assert!(msg.contains(&uid.to_string()), "names the ref; got {msg:?}");

    let mut actions_only = legacy;
    actions_only["actions"] = json!(["read", "write"]);
    let (status, resp) = send(
        &f,
        "PATCH",
        &format!("/perm/grants/{id}"),
        &token,
        actions_only,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {resp}");
}

/// The rule twin: an exemption naming a since-deleted user does not block an
/// edit that leaves it alone.
#[tokio::test]
async fn a_rule_patch_keeping_a_since_deleted_exemption_is_accepted() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let gone = known_user(&f, unique("jf3_gone")).await;
    let body = json!({
        "name": unique("jf3_rule_patch"),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "path", "catalog_id": f.catalog_id,
                      "schema": "sales", "table": "orders", "column": "email" },
        "exempt": { "include": [{ "kind": "user", "ref": gone }], "exclude": [] }
    });
    let (status, created) = send(&f, "POST", "/perm/rules", &token, body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let id = created["id"].as_str().expect("rule id").to_string();
    sqlx::query("DELETE FROM users WHERE username = $1")
        .bind(&gone)
        .execute(&f.db.pool)
        .await
        .expect("delete user");

    let mut renamed = body;
    renamed["name"] = json!(unique("jf3_rule_renamed"));
    let (status, resp) = send(&f, "PATCH", &format!("/perm/rules/{id}"), &token, renamed).await;
    assert_eq!(status, StatusCode::OK, "body: {resp}");
}

/// Spec C5: a classification mask over a classification that sits on a whole
/// table is refused with a stable code and the ids the UI needs to offer
/// "Remove from table" — on preview, before anything is created.
///
/// Also pins `moreTables` end to end and which attachment is reported: one
/// attachment → `false`; another table → `true`, still naming the first in
/// `(schema, table, warehouse)` order; the SAME `schema.table` in a second
/// warehouse is a second attachment → `true`, and the warehouse tiebreaker
/// makes the reported warehouse the smaller id rather than whichever row
/// Postgres returns first.
#[tokio::test]
async fn a_tag_mask_over_a_table_level_attachment_answers_table_tag_mask_conflict() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, wh): (Uuid, Uuid) = sqlx::query_as(
        "SELECT a.tag_id, a.warehouse_id FROM catalog_tag_assignments a JOIN tags t ON t.id = a.tag_id \
          WHERE t.key = $1 LIMIT 1",
    )
    .bind(&f.tag_key)
    .fetch_one(&f.db.pool)
    .await
    .expect("the fixture's assignment");
    let attach = |warehouse: Uuid, table: &'static str| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
                 VALUES ($1, $2, 'sales', $3, '')",
            )
            .bind(tag_id)
            .bind(warehouse)
            .bind(table)
            .execute(&pool)
            .await
            .expect("table-level attachment");
        }
    };
    let preview = || {
        let rule = json!({
            "name": unique("mask-on-table-tag"),
            "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
            "selector": { "type": "tag", "key": f.tag_key },
            "exempt": { "include": [], "exclude": [] }
        });
        let app = f.app();
        let token = token.clone();
        async move {
            into_json(
                app.oneshot(req(
                    "POST",
                    "/perm/preview",
                    &token,
                    Some(json!({ "rule": rule })),
                ))
                .await
                .unwrap(),
            )
            .await
        }
    };

    attach(wh, "orders").await;
    let (status, one) = preview().await;
    // A table that sorts AFTER `orders`, so the reported one stays `orders`.
    attach(wh, "zz_orders").await;
    let (_, two_tables) = preview().await;
    sqlx::query(
        "DELETE FROM catalog_tag_assignments WHERE tag_id = $1 AND column_name = '' \
           AND table_name = 'zz_orders'",
    )
    .bind(tag_id)
    .execute(&f.db.pool)
    .await
    .expect("drop the second table");
    let other_wh = Uuid::new_v4();
    attach(other_wh, "orders").await;
    let (_, two_warehouses) = preview().await;

    sqlx::query("DELETE FROM catalog_tag_assignments WHERE tag_id = $1 AND column_name = ''")
        .bind(tag_id)
        .execute(&f.db.pool)
        .await
        .expect("clean up");

    assert_eq!(status, StatusCode::BAD_REQUEST, "{one}");
    assert_eq!(one["code"], "table_tag_mask_conflict");
    assert_eq!(one["tagId"], tag_id.to_string());
    assert_eq!(one["warehouseId"], wh.to_string());
    assert_eq!(
        (one["schema"].as_str(), one["table"].as_str()),
        (Some("sales"), Some("orders"))
    );
    assert_eq!(one["maskOrigin"], "rule");
    assert_eq!(one["moreTables"], false, "one attachment: {one}");
    assert!(
        one["error"].as_str().unwrap().starts_with("tag \""),
        "the sentence is kept: {one}"
    );

    assert_eq!(
        two_tables["code"], "table_tag_mask_conflict",
        "{two_tables}"
    );
    assert_eq!(two_tables["moreTables"], true, "two tables: {two_tables}");
    assert_eq!(
        two_tables["table"], "orders",
        "the first in order: {two_tables}"
    );

    assert_eq!(
        two_warehouses["code"], "table_tag_mask_conflict",
        "{two_warehouses}"
    );
    assert_eq!(
        two_warehouses["moreTables"], true,
        "one table in two warehouses is two attachments: {two_warehouses}"
    );
    assert_eq!(
        two_warehouses["warehouseId"],
        wh.min(other_wh).to_string(),
        "the warehouse tiebreaker reports the smaller id: {two_warehouses}"
    );
}

// ---------------------------------------------------------------------------
// Rule preview `effect` (classification UX spec C6, C7)
// ---------------------------------------------------------------------------

async fn preview_rule(f: &Fixture, token: &str, rule: Value) -> (StatusCode, Value) {
    into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                token,
                Some(json!({ "rule": rule })),
            ))
            .await
            .unwrap(),
    )
    .await
}

fn mask_rule(name: &str, selector: Value, exempt_include: Value) -> Value {
    json!({
        "name": unique(name),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": selector,
        "exempt": { "include": exempt_include, "exclude": [] }
    })
}

/// Spec C7 (a)+(d): the fixture's catalog has one engine bound to a profile, and
/// its tag sits on one column, so a tag mask reaches 1 table and 1 column and is
/// enforced by 1 engine.
#[tokio::test]
async fn a_tag_mask_preview_states_its_reach_and_its_engines() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule(
            "eff-tag",
            json!({"type": "tag", "key": f.tag_key}),
            json!([]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["effect"];
    assert_eq!(e["kind"], "mask");
    assert_eq!(
        (e["tableCount"].as_u64(), e["columnCount"].as_u64()),
        (Some(1), Some(1)),
        "{e}"
    );
    assert_eq!(e["enforcedBy"], 1, "{e}");
    assert!(e["notEnforced"].is_null(), "{e}");
    assert_eq!(e["truncated"], false, "{e}");
    let cat = &e["catalogs"][0];
    assert_eq!(cat["catalogId"], f.catalog_id.to_string());
    assert!(cat["notEnforced"].is_null(), "{cat}");
    assert_eq!(cat["engines"].as_array().unwrap().len(), 1);
    assert_eq!(cat["engines"][0]["engineType"], "starrocks", "{cat}");
}

/// Spec C7 (b): a catalog no engine serves is reported as not enforced.
#[tokio::test]
async fn a_path_mask_on_a_catalog_without_an_engine_is_not_enforced() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-orphan").await;
    let selector = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-orphan", selector, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["effect"];
    assert_eq!(e["enforcedBy"], 0, "{e}");
    assert_eq!(e["catalogs"][0]["notEnforced"], "no_engine", "{e}");
    assert_eq!(e["notEnforced"], "no_engine", "{e}");
    assert_eq!(
        (e["tableCount"].as_u64(), e["columnCount"].as_u64()),
        (Some(1), Some(1))
    );
    // A path with no column masks every column: the count is not a number.
    let every = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t"});
    let (status, body) = preview_rule(&f, &token, mask_rule("eff-every", every, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["tableCount"], 1, "{body}");
    assert!(body["effect"]["columnCount"].is_null(), "{body}");
    // A pattern level has no count either.
    let pattern = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "ord*", "column": "email"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-pattern", pattern, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["kind"], "mask", "{body}");
    assert!(body["effect"]["tableCount"].is_null(), "{body}");
    assert!(body["effect"]["columnCount"].is_null(), "{body}");
}

/// Spec C7 (b), second reason: engines attached, none bound to a profile.
#[tokio::test]
async fn a_catalog_whose_engines_have_no_profile_is_not_enforced() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-noprof").await;
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1,'trino','egret','{}'::jsonb) RETURNING id",
    )
    .bind(unique("eff-unbound"))
    .fetch_one(&f.db.pool)
    .await
    .expect("unbound engine");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(orphan)
    .execute(&f.db.pool)
    .await
    .expect("attach");
    let selector = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-noprof", selector, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["enforcedBy"], 0, "{body}");
    assert_eq!(
        body["effect"]["catalogs"][0]["notEnforced"], "no_profile",
        "{body}"
    );
    assert_eq!(
        body["effect"]["catalogs"][0]["engines"],
        json!([]),
        "{body}"
    );
}

/// Spec C7 (c): exemptions count distinct people — a group of two plus one of
/// them named directly is two.
#[tokio::test]
async fn exempt_members_counts_distinct_people() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let a = known_user(&f, unique("effa")).await;
    let b = known_user(&f, unique("effb")).await;
    let group = unique("eff_group");
    let gid: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&group)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    for u in [&a, &b] {
        sqlx::query(
            "INSERT INTO user_groups (user_id, group_id) SELECT id, $2 FROM users WHERE username = $1",
        )
        .bind(u)
        .bind(gid)
        .execute(&f.db.pool)
        .await
        .unwrap();
    }
    let include = json!([{"kind": "group", "ref": group}, {"kind": "user", "ref": a}]);
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule(
            "eff-exempt",
            json!({"type": "tag", "key": f.tag_key}),
            include,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["exemptMembers"], 2, "{body}");
    assert_eq!(body["effect"]["exemptEveryone"], false);
}

/// Spec C6: on a catalog no engine serves (nothing compiles, so the compiler's
/// overlap check sees nothing), a second mask on one column is still reported.
#[tokio::test]
async fn a_second_mask_on_a_column_is_reported_without_any_engine() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-overlap").await;
    let path = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"});
    let first = mask_rule("eff-first", path.clone(), json!([]));
    let (status, created) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", &token, Some(first.clone())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let first_id = created["id"].as_str().unwrap().to_string();

    // The same column spelled in another case: resource levels match without regard to case.
    let same_column = json!({"type": "path", "catalog_id": orphan, "schema": "S", "table": "T", "column": "EMAIL"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-second", same_column, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = body["effect"]["alreadyMasked"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{body}");
    assert_eq!(hits[0]["ruleId"], first_id);
    assert_eq!(hits[0]["maskType"], "MASK_HASH");

    // A different column of the same table is not "already masked".
    let other_column = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "phone"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-other", other_column, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["alreadyMasked"], json!([]), "{body}");

    // Previewing a PATCH of the first rule does not report the rule against itself.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "id": first_id, "rule": first })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["alreadyMasked"], json!([]), "{body}");
}

/// Bounds: at most `ALREADY_MASKED_CAP` (20) overlapping masks are listed.
#[tokio::test]
async fn already_masked_lists_at_most_twenty() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-cap").await;
    let path = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"});
    for _ in 0..21 {
        let (status, created) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/rules",
                    &token,
                    Some(mask_rule("eff-cap", path.clone(), json!([]))),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
    }
    let (status, body) = preview_rule(&f, &token, mask_rule("eff-cap-new", path, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["effect"]["alreadyMasked"].as_array().unwrap().len(),
        20,
        "{body}"
    );
    assert_eq!(body["effect"]["truncated"], true, "{body}");
}

/// Bounds: a tag's reach is COUNTED in SQL, not truncated by the cap on the
/// coordinates the overlap check examines (60 columns > 50).
#[tokio::test]
async fn a_tag_on_more_columns_than_the_scan_cap_is_still_counted_exactly() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-wide").await;
    let wh: Uuid = sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
        .bind(orphan)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let key = unique("effwide");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(&key)
            .bind(unique("EffWide"))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    // Two columns per table, thirty tables: `t{n}.c` and `t{n}.d`.
    for i in 0..30 {
        for column in ["c", "d"] {
            sqlx::query(
                "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
                 VALUES ($1,$2,'s',$3,$4)",
            )
            .bind(tag_id)
            .bind(wh)
            .bind(format!("t{i}"))
            .bind(column)
            .execute(&f.db.pool)
            .await
            .unwrap();
        }
    }
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule("eff-wide", json!({"type": "tag", "key": key}), json!([])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["effect"];
    assert_eq!(
        (e["tableCount"].as_u64(), e["columnCount"].as_u64()),
        (Some(30), Some(60)),
        "{e}"
    );
    // Sixty columns, fifty examined: the overlap answer says it may be partial.
    assert_eq!(e["truncated"], true, "{e}");
    assert_eq!(e["catalogs"][0]["notEnforced"], "no_engine", "{e}");
}

/// A grant preview carries no effect.
#[tokio::test]
async fn a_grant_preview_has_no_effect_block() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let user = known_user(&f, unique("effg")).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "grant": grant_body(f.catalog_id, &user) })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("effect").is_none_or(|e| e.is_null()), "{body}");
}

// --- Review round 1: tag-name overlap (spec C6), whole-table and pattern
// columns, `truncated`, `no_assignment`.

/// Stores a rule over HTTP and returns its id.
async fn store_rule(f: &Fixture, token: &str, rule: Value) -> String {
    let (status, created) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", token, Some(rule)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().unwrap().to_string()
}

/// A classification `(key, name)` with the given name, assigned nowhere yet.
async fn classification_named(f: &Fixture, name: &str) -> (String, Uuid) {
    let key = unique("effkey");
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(&key)
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    (key, id)
}

async fn warehouse_of(f: &Fixture, catalog_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
        .bind(catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
}

/// Assigns `tag_id` to `s.<table>.<column>` (`""` = the whole table).
async fn assign(f: &Fixture, tag_id: Uuid, wh: Uuid, table: &str, column: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'s',$3,$4)",
    )
    .bind(tag_id)
    .bind(wh)
    .bind(table)
    .bind(column)
    .execute(&f.db.pool)
    .await
    .unwrap();
}

fn hit_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body["effect"]["alreadyMasked"]
        .as_array()
        .unwrap_or_else(|| panic!("no alreadyMasked: {body}"))
        .iter()
        .map(|h| h["ruleId"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

/// Spec C6's own scenario, on a catalog no engine serves: a path mask on a
/// column that a TAG mask already reaches names the tag rule. Controls: another
/// classification's mask on another column of the table is not named, and a
/// classification on the whole TABLE reaches every column of it.
#[tokio::test]
async fn a_path_mask_on_a_column_a_tag_mask_reaches_names_the_tag_rule() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-c6").await;
    let wh = warehouse_of(&f, orphan).await;

    let (key_a, tag_a) = classification_named(&f, &unique("EffCls")).await;
    let (key_b, tag_b) = classification_named(&f, &unique("EffOther")).await;
    let (key_c, tag_c) = classification_named(&f, &unique("EffWhole")).await;
    let none = json!([]);
    let tag_sel = |k: &str| json!({"type": "tag", "key": k});
    // Stored before any table-level assignment exists (a tag mask over one is refused).
    let mask_a = store_rule(
        &f,
        &token,
        mask_rule("eff-c6-a", tag_sel(&key_a), none.clone()),
    )
    .await;
    let mask_b = store_rule(
        &f,
        &token,
        mask_rule("eff-c6-b", tag_sel(&key_b), none.clone()),
    )
    .await;
    let mask_c = store_rule(
        &f,
        &token,
        mask_rule("eff-c6-c", tag_sel(&key_c), none.clone()),
    )
    .await;
    assign(&f, tag_a, wh, "t", "email").await;
    assign(&f, tag_b, wh, "t", "phone").await;
    assign(&f, tag_c, wh, "t2", "").await;

    let path = |table: &str, column: &str| json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": table, "column": column});
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule("eff-c6-p", path("t", "email"), none.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        hit_ids(&body),
        vec![mask_a.clone()],
        "only the tag rule on `email`, not the one on `phone`: {body}"
    );
    assert_ne!(mask_a, mask_b);
    let hit = &body["effect"]["alreadyMasked"][0];
    assert!(
        hit["coordinate"].as_str().unwrap().ends_with(".s.t.email"),
        "{hit}"
    );

    // The table-level classification reaches `t2.email`.
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule("eff-c6-q", path("t2", "email"), none.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(hit_ids(&body), vec![mask_c], "{body}");
}

/// The reverse of C6: a TAG mask previewed against a stored PATH mask on a
/// column the classification sits on.
#[tokio::test]
async fn a_tag_mask_on_a_column_a_path_mask_reaches_names_the_path_rule() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-c6r").await;
    let wh = warehouse_of(&f, orphan).await;
    let (key, tag) = classification_named(&f, &unique("EffRev")).await;
    assign(&f, tag, wh, "t", "email").await;
    let stored = store_rule(
        &f,
        &token,
        mask_rule(
            "eff-c6r-p",
            json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"}),
            json!([]),
        ),
    )
    .await;
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule("eff-c6r-t", json!({"type": "tag", "key": key}), json!([])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(hit_ids(&body), vec![stored], "{body}");
}

/// A literal table with no column, or a column pattern, still finds the masks
/// already on that table's columns — path masks and tag masks — and nothing
/// on another table.
#[tokio::test]
async fn a_whole_table_or_pattern_mask_finds_the_masks_on_its_columns() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-wt").await;
    let wh = warehouse_of(&f, orphan).await;
    let none = json!([]);
    let column_mask = store_rule(
        &f,
        &token,
        mask_rule(
            "eff-wt-col",
            json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"}),
            none.clone(),
        ),
    )
    .await;
    let (key, tag) = classification_named(&f, &unique("EffWt")).await;
    let tag_mask = store_rule(
        &f,
        &token,
        mask_rule(
            "eff-wt-tag",
            json!({"type": "tag", "key": key}),
            none.clone(),
        ),
    )
    .await;
    assign(&f, tag, wh, "t", "phone").await;

    let mut both = vec![column_mask.clone(), tag_mask.clone()];
    both.sort();

    // The whole table: both.
    let whole = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-wt-whole", whole, none.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(hit_ids(&body), both, "{body}");

    // `e*` meets `email` but not the classification on `phone`.
    let pattern =
        json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "e*"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-wt-pat", pattern, none.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(hit_ids(&body), vec![column_mask], "{body}");

    // `p*` meets the classification on `phone` only.
    let pattern =
        json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "p*"});
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-wt-pat2", pattern, none.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(hit_ids(&body), vec![tag_mask], "{body}");

    // Another table: nothing.
    let other = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "other"});
    let (status, body) = preview_rule(&f, &token, mask_rule("eff-wt-other", other, none)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["alreadyMasked"], json!([]), "{body}");
}

/// A classification assigned nowhere reaches nothing, and says so.
#[tokio::test]
async fn a_tag_assigned_nowhere_is_not_enforced_for_no_assignment() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (key, _) = classification_named(&f, &unique("EffNowhere")).await;
    let (status, body) = preview_rule(
        &f,
        &token,
        mask_rule("eff-nowhere", json!({"type": "tag", "key": key}), json!([])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["effect"];
    assert_eq!(e["notEnforced"], "no_assignment", "{e}");
    assert_eq!(e["catalogs"], json!([]), "{e}");
    assert_eq!(e["enforcedBy"], 0, "{e}");
    assert_eq!(e["tableCount"], 0, "{e}");
    assert_eq!(e["truncated"], false, "{e}");
}

// ---------------------------------------------------------------------------
// A STORED rule's effect (classification UX Task 13: "Not enforced yet" after
// Create). `GET /perm/rules/{id}/effect` and `GET /perm/rules/enforcement`.
// ---------------------------------------------------------------------------

async fn get_json(f: &Fixture, token: &str, uri: &str) -> (StatusCode, Value) {
    into_json(f.app().oneshot(req("GET", uri, token, None)).await.unwrap()).await
}

/// The stored rule's effect is the SAME computation the preview runs: the
/// effect a preview of the stored body (as a patch of that id) returns, field
/// for field — including the reason it is not enforced.
#[tokio::test]
async fn a_stored_rules_effect_is_what_its_preview_says() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-stored").await;
    let body = mask_rule(
        "eff-stored",
        json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"}),
        json!([]),
    );
    let id = store_rule(&f, &token, body.clone()).await;

    let (status, got) = get_json(&f, &token, &format!("/perm/rules/{id}/effect")).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["enforcedBy"], 0, "{got}");
    assert_eq!(got["notEnforced"], "no_engine", "{got}");
    assert_eq!(got["tableCount"], 1, "{got}");

    let (status, preview) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/preview",
                &token,
                Some(json!({ "id": id, "rule": body })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(got, preview["effect"], "one computation, one answer");
}

/// An enforced stored rule says so; an unknown id is a 404; `policy:read`
/// alone reaches it and a caller without it is refused.
#[tokio::test]
async fn a_stored_rules_effect_is_a_policy_read_and_404s_an_unknown_id() {
    let Some(mut f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let admin = admin_session(&f).await;
    // The fixture's catalog has an engine bound to a profile. A unique table
    // keeps this mask from meeting any other test's.
    let table = unique("eff_enf");
    let id = store_rule(
        &f,
        &admin,
        mask_rule(
            "eff-enf",
            json!({"type": "path", "catalog_id": f.catalog_id, "schema": "s", "table": table, "column": "email"}),
            json!([]),
        ),
    )
    .await;

    let reader = session_with_permission(&mut f, "policy:read").await;
    let (status, got) = get_json(&f, &reader, &format!("/perm/rules/{id}/effect")).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["enforcedBy"], 1, "{got}");
    assert!(got["notEnforced"].is_null(), "{got}");

    let (status, got) = get_json(
        &f,
        &reader,
        &format!("/perm/rules/{}/effect", Uuid::new_v4()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{got}");

    let outsider = session_with_permission(&mut f, "tag:read").await;
    let (status, got) = get_json(&f, &outsider, &format!("/perm/rules/{id}/effect")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{got}");
}

/// The compact list form: one entry per KNOWN id, in the order asked, with the
/// same `enforcedBy`/`notEnforced` the single read gives; unknown ids are left
/// out, a repeated id is answered once.
#[tokio::test]
async fn rule_enforcement_answers_each_known_id_like_the_single_read() {
    let Some(mut f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let admin = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-batch").await;
    let not_enforced = store_rule(
        &f,
        &admin,
        mask_rule(
            "eff-batch-no",
            json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"}),
            json!([]),
        ),
    )
    .await;
    let enforced = store_rule(
        &f,
        &admin,
        mask_rule(
            "eff-batch-yes",
            json!({"type": "path", "catalog_id": f.catalog_id, "schema": "s", "table": unique("eff_b"), "column": "email"}),
            json!([]),
        ),
    )
    .await;
    let (key, _) = classification_named(&f, &unique("EffBatchNowhere")).await;
    let nowhere = store_rule(
        &f,
        &admin,
        mask_rule(
            "eff-batch-tag",
            json!({"type": "tag", "key": key}),
            json!([]),
        ),
    )
    .await;
    let unknown = Uuid::new_v4();

    let reader = session_with_permission(&mut f, "policy:read").await;
    let uri = format!(
        "/perm/rules/enforcement?ids={not_enforced},{unknown},{enforced},{nowhere},{not_enforced}"
    );
    let (status, got) = get_json(&f, &reader, &uri).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    // The per-catalog half is compared with the single read below.
    let rule_wide: Vec<Value> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let mut e = e.clone();
            e.as_object_mut().unwrap().remove("catalogs");
            e
        })
        .collect();
    assert_eq!(
        json!(rule_wide),
        json!([
            {"ruleId": not_enforced, "enforcedBy": 0, "notEnforced": "no_engine"},
            {"ruleId": enforced, "enforcedBy": 1, "notEnforced": null},
            {"ruleId": nowhere, "enforcedBy": 0, "notEnforced": "no_assignment"},
        ]),
        "{got}"
    );

    // Each entry agrees with the single read.
    for (id, entry) in [&not_enforced, &enforced, &nowhere]
        .into_iter()
        .zip(got.as_array().unwrap())
    {
        let (status, single) = get_json(&f, &reader, &format!("/perm/rules/{id}/effect")).await;
        assert_eq!(status, StatusCode::OK, "{single}");
        assert_eq!(single["enforcedBy"], entry["enforcedBy"], "{id}");
        assert_eq!(single["notEnforced"], entry["notEnforced"], "{id}");
        assert_eq!(single["catalogs"], entry["catalogs"], "{id}");
    }

    let outsider = session_with_permission(&mut f, "tag:read").await;
    let (status, got) = get_json(&f, &outsider, &uri).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{got}");
}

/// Per catalog as well as rule-wide: a classification rule enforced in one
/// catalog (the fixture's, engine bound) is NOT enforced in another it reaches
/// that has no engine, and the list says so for each catalog — the answer a
/// Data page shows for the catalog it is on.
#[tokio::test]
async fn rule_enforcement_answers_each_catalog_the_rule_reaches() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let admin = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-percat").await;
    let (key, tag_id) = classification_named(&f, &unique("EffPerCatalog")).await;
    let table = unique("eff_pc");
    assign(
        &f,
        tag_id,
        warehouse_of(&f, f.catalog_id).await,
        &table,
        "email",
    )
    .await;
    assign(&f, tag_id, warehouse_of(&f, orphan).await, &table, "email").await;
    let id = store_rule(
        &f,
        &admin,
        mask_rule("eff-percat", json!({"type": "tag", "key": key}), json!([])),
    )
    .await;

    let (status, got) = get_json(&f, &admin, &format!("/perm/rules/enforcement?ids={id}")).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let entry = &got[0];
    assert_eq!(entry["enforcedBy"], 1, "enforced somewhere: {got}");
    assert!(entry["notEnforced"].is_null(), "{got}");
    let catalogs = entry["catalogs"].as_array().expect("catalogs");
    assert_eq!(catalogs.len(), 2, "{got}");
    let of = |c: Uuid| {
        catalogs
            .iter()
            .find(|e| e["catalogId"] == json!(c))
            .unwrap_or_else(|| panic!("catalog {c} missing: {got}"))
    };
    let here = of(f.catalog_id);
    assert!(here["notEnforced"].is_null(), "{got}");
    assert_eq!(here["engines"].as_array().unwrap().len(), 1, "{got}");
    let there = of(orphan);
    assert_eq!(there["notEnforced"], "no_engine", "{got}");
    assert_eq!(there["engines"], json!([]), "{got}");

    // The single read's catalogs are the same computation.
    let (_, single) = get_json(&f, &admin, &format!("/perm/rules/{id}/effect")).await;
    assert_eq!(single["catalogs"], entry["catalogs"], "one computation");
}

/// A Trino profile (named `name`, `config_jsonb` = `config`, inserted raw so a
/// dangling `tagService` survives) with an engine bound to it, attached to a
/// new catalog in a new warehouse. Returns the catalog id.
async fn trino_catalog_with_config(f: &Fixture, tag: &str, config: Value) -> Uuid {
    let _universe = advisory_lock(&f.dsn).await;
    egret_policy::seed_trino(&f.policy)
        .await
        .expect("seed trino service-def");
    let service_id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) VALUES ($1,'trino',$2) RETURNING id",
    )
    .bind(unique(tag))
    .bind(config)
    .fetch_one(&f.db.pool)
    .await
    .expect("trino profile");
    let catalog_id = orphan_catalog(f, tag).await;
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{\"imageVersion\":\"483\"}'::jsonb,$2) RETURNING id",
    )
    .bind(unique(tag))
    .bind(service_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("engine");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach catalog");
    catalog_id
}

/// A Trino profile whose `tagService` names a real, empty tag service, bound to
/// an engine on a new catalog (spec Task 3). Returns the catalog and the TAG
/// service's id, which a test writes hand policies into.
async fn trino_tag_profile(f: &Fixture, label: &str) -> (Uuid, i64) {
    egret_policy::seed_tag(&f.policy)
        .await
        .expect("seed tag def");
    let tag_service = f
        .policy
        .create_service(&unique(&format!("{label}_tag")), "tag", &json!({}))
        .await
        .expect("tag service");
    let catalog_id =
        trino_catalog_with_config(f, label, json!({ "tagService": tag_service.name })).await;
    (catalog_id, tag_service.id)
}

/// A hand-written tag MASK policy for `{public}` on `tag_name`, in the tag
/// service `tag_service_id`.
async fn hand_tag_mask(f: &Fixture, tag_service_id: i64, tag_name: &str, mask_type: &str) {
    let tag_service = f
        .policy
        .get_service_by_id(tag_service_id)
        .await
        .expect("tag service row");
    let name = unique("hand-tag-mask");
    f.policy
        .create_policy(
            tag_service_id,
            &name,
            &json!({
                "name": name, "service": tag_service.name, "serviceType": "tag",
                "policyType": 1, "isEnabled": true, "policyPriority": 0,
                "resources": { "tag": { "values": [tag_name], "isExcludes": false, "isRecursive": false } },
                "dataMaskPolicyItems": [{"users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": mask_type}}],
                "policyItems": [], "denyPolicyItems": [],
                "allowExceptions": [], "denyExceptions": []
            }),
        )
        .await
        .expect("hand tag mask");
}

/// `POST /perm/explain` for `user` reading `sales.orders.email` in `catalog_id`.
async fn explain_email(f: &Fixture, token: &str, catalog_id: Uuid, user: &str) -> Value {
    let mut ask = explain_body(catalog_id, user);
    ask["target"]["column"] = json!("email");
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", token, Some(ask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    body
}

/// Spec §5.2 (C2): a classification mask on Trino is asked and answered, with
/// the resolved tags, and attributed to the rule.
#[tokio::test]
async fn explain_reports_a_trino_classification_mask() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (catalog_id, _tag_service) = trino_tag_profile(&f, "vmask-trino-mask").await;
    let tag = assign_tag(&f, catalog_id, "sales", "orders", "email").await;
    let reader = known_user(&f, unique("vmask-reader")).await;
    // After the fixtures: `trino_catalog_with_config` takes the lock itself.
    let _lock = exclusive(&f).await;
    let (status, out) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(catalog_id, &reader)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {out}");
    let rule_id = store_rule(
        &f,
        &token,
        mask_rule(
            "vmask-trino-hash",
            json!({"type": "tag", "key": tag.key}),
            json!([]),
        ),
    )
    .await;

    let body = explain_email(&f, &token, catalog_id, &reader).await;
    let trino = profile_for(&body, "trino");
    assert_eq!(
        access_for(trino, "select")["decision"]["kind"],
        "allowed",
        "body: {body}"
    );
    let mask = &trino["mask"];
    assert_eq!(mask["decision"]["kind"], "masked", "body: {body}");
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_HASH",
        "body: {body}"
    );
    assert_eq!(mask["attribution"]["kind"], "rule", "body: {body}");
    assert_eq!(
        mask["attribution"]["rule_id"],
        json!(rule_id),
        "body: {body}"
    );
    assert!(
        trino.get("masked_read_skips_access").is_none(),
        "Trino checks access on a masked column; body: {body}"
    );
    // Spec §5.1: the answer in one sentence, naming the rule.
    let rule_name: String = sqlx::query_scalar("SELECT name FROM perm.rules WHERE id = $1::uuid")
        .bind(&rule_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("rule name");
    assert_eq!(
        trino["effect_lines"][0]["text"],
        json!(format!("{reader} reads email as hashed (rule {rule_name})")),
        "body: {body}"
    );
    assert_eq!(
        trino["effect_lines"][0]["outcome"], "masked",
        "body: {body}"
    );
}

/// The same question when the classification is on the TABLE only: the tag
/// reaches the column request as `ANCESTOR`, and a tag mask applies there.
#[tokio::test]
async fn explain_reports_a_trino_mask_from_a_table_classification() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (catalog_id, tag_service) = trino_tag_profile(&f, "vmask-trino-anc").await;
    let tag = assign_tag(&f, catalog_id, "sales", "orders", "").await;
    hand_tag_mask(&f, tag_service, &tag.name, "MASK_HASH").await;

    let body = explain_email(&f, &token, catalog_id, "analyst").await;
    let mask = &profile_for(&body, "trino")["mask"];
    assert_eq!(mask["decision"]["kind"], "masked", "body: {body}");
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_HASH",
        "body: {body}"
    );
}

/// A resolution explain declines (a fold-divergent stored column) leaves the
/// mask undecidable with `tags_unresolved`, never answered from a narrower set.
#[tokio::test]
async fn a_declined_resolution_leaves_the_trino_mask_undecidable() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (catalog_id, tag_service) = trino_tag_profile(&f, "vmask-trino-decl").await;
    let tag = assign_tag(&f, catalog_id, "sales", "orders", "email").await;
    assign_tag(&f, catalog_id, "sales", "orders", "emaıl").await;
    hand_tag_mask(&f, tag_service, &tag.name, "MASK_HASH").await;

    let body = explain_email(&f, &token, catalog_id, "analyst").await;
    let mask = &profile_for(&body, "trino")["mask"];
    assert_eq!(mask["decision"]["kind"], "undecidable", "body: {body}");
    assert!(
        mask["decision"]["why"]
            .as_array()
            .expect("why")
            .iter()
            .any(|c| c["kind"] == "tags_unresolved"),
        "body: {body}"
    );
}

/// `POST /perm/explain` with `ask` as the body.
async fn explain_with(f: &Fixture, token: &str, ask: Value) -> Value {
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", token, Some(ask)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    body
}

fn row_filter_rule(name: &str, selector: Value, filter_expr: &str) -> Value {
    json!({
        "name": unique(name),
        "kind": { "type": "row_filter", "filter_expr": filter_expr },
        "selector": selector,
        "exempt": { "include": [], "exclude": [] }
    })
}

/// Spec §5.3 (C3): the row-filter decision joins the answer, asked against the
/// TABLE; a column target shows its table's filter.
#[tokio::test]
async fn explain_reports_a_path_row_filter_on_starrocks() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let rule_id = store_rule(
        &f,
        &token,
        row_filter_rule(
            "vmask-path-rf",
            json!({"type": "path", "catalog_id": f.catalog_id,
                   "schema": "sales", "table": "orders", "column": null}),
            "region = 'us'",
        ),
    )
    .await;

    let table = explain_with(&f, &token, explain_body(f.catalog_id, "analyst")).await;
    let column = explain_column(&f, &token, "analyst").await;
    for body in [&table, &column] {
        let rf = &profile_for(body, "starrocks")["row_filter"];
        assert_eq!(rf["access_type"], "select", "body: {body}");
        assert_eq!(rf["decision"]["kind"], "filtered", "body: {body}");
        assert_eq!(
            rf["decision"]["result"]["filter_expr"], "region = 'us'",
            "body: {body}"
        );
        assert_eq!(rf["attribution"]["kind"], "rule", "body: {body}");
        assert_eq!(rf["attribution"]["rule_id"], json!(rule_id), "body: {body}");
    }
}

/// A Trino classification row filter on a TABLE-level classification filters.
#[tokio::test]
async fn explain_reports_a_trino_classification_row_filter_on_a_tagged_table() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (catalog_id, _tag_service) = trino_tag_profile(&f, "vmask-trino-rft").await;
    let tag = assign_tag(&f, catalog_id, "sales", "orders", "").await;
    let _lock = exclusive(&f).await;
    store_rule(
        &f,
        &token,
        row_filter_rule(
            "vmask-trino-rft",
            json!({"type": "tag", "key": tag.key}),
            "region = 'us'",
        ),
    )
    .await;

    let mut ask = explain_body(catalog_id, "analyst");
    ask["target"]["column"] = json!("email");
    let body = explain_with(&f, &token, ask).await;
    let rf = &profile_for(&body, "trino")["row_filter"];
    assert_eq!(rf["decision"]["kind"], "filtered", "body: {body}");
    assert_eq!(
        rf["decision"]["result"]["filter_expr"], "region = 'us'",
        "body: {body}"
    );
}

/// E14: on Trino a classification on a COLUMN reaches the table request as a
/// descendant, so its row filter filters nothing. The table request is asked
/// with the table-level resolution, so explain says so.
#[tokio::test]
async fn a_trino_row_filter_on_a_column_classification_filters_nothing() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (catalog_id, _tag_service) = trino_tag_profile(&f, "vmask-trino-rfc").await;
    let tag = assign_tag(&f, catalog_id, "sales", "orders", "email").await;
    let _lock = exclusive(&f).await;
    // A grant, so the read is allowed and the answer has a rows line.
    let (status, out) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {out}");
    let rule = row_filter_rule(
        "vmask-trino-rfc",
        json!({"type": "tag", "key": tag.key}),
        "region = 'us'",
    );
    let rule_name = rule["name"].as_str().expect("name").to_string();
    let rule_id = store_rule(&f, &token, rule).await;

    let mut ask = explain_body(catalog_id, "analyst");
    ask["target"]["column"] = json!("email");
    let body = explain_with(&f, &token, ask).await;
    let trino = profile_for(&body, "trino");
    assert_eq!(
        trino["row_filter"]["decision"]["kind"], "no_filter",
        "body: {body}"
    );
    // D3 (spec §5.8): the gap is listed and the rows line says it.
    assert_eq!(
        trino["row_filter_not_applied"],
        json!([{"rule_id": rule_id, "rule_name": rule_name, "tag_name": tag.name}]),
        "body: {body}"
    );
    assert_eq!(
        trino["effect_lines"][1]["text"],
        json!(format!(
            "analyst sees every row: rule {rule_name} does not filter on Trino here, because {} \
             is on a column, not on the table",
            tag.name
        )),
        "body: {body}"
    );
}

/// A table with no row filter answers `no_filter`; a `write` has no row filter.
#[tokio::test]
async fn a_read_without_a_filter_is_no_filter_and_a_write_has_none() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (_trino, _starrocks, catalog_id) = two_profile_catalog(&f, "vmask-rf-none").await;

    let read = explain_with(&f, &token, explain_body(catalog_id, "analyst")).await;
    for engine in ["trino", "starrocks"] {
        assert_eq!(
            profile_for(&read, engine)["row_filter"]["decision"]["kind"],
            "no_filter",
            "body: {read}"
        );
    }
    let mut ask = explain_body(catalog_id, "analyst");
    ask["action"] = json!("write");
    let write = explain_with(&f, &token, ask).await;
    for engine in ["trino", "starrocks"] {
        assert!(
            profile_for(&write, engine).get("row_filter").is_none(),
            "body: {write}"
        );
    }
}

/// C5 (spec §5.7): a catalog nothing serves says so in one line, naming the
/// catalog and which of the two remedies applies.
#[tokio::test]
async fn no_profile_explain_says_not_enforced_with_the_catalog() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let catalog_id = orphan_catalog(&f, "vmask-orphan").await;
    let name: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name");

    let body = explain_with(&f, &token, explain_body(catalog_id, "analyst")).await;
    assert_eq!(
        body["outcome"], "no_profile_serves_the_catalog",
        "body: {body}"
    );
    assert_eq!(
        body["effect_line"]["text"],
        json!(format!("Not enforced: {name} has no engine.")),
        "body: {body}"
    );
    assert_eq!(
        body["effect_line"]["outcome"], "not_enforced",
        "body: {body}"
    );

    // An engine attached, bound to no policy profile.
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1,'trino','egret','{}'::jsonb) RETURNING id",
    )
    .bind(unique("vmask-unbound"))
    .fetch_one(&f.db.pool)
    .await
    .expect("engine");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("attach");
    let body = explain_with(&f, &token, explain_body(catalog_id, "analyst")).await;
    assert_eq!(
        body["effect_line"]["text"],
        json!(format!(
            "Not enforced: {name}'s engines have no policy profile."
        )),
        "body: {body}"
    );

    // A served catalog has no such line.
    let body = explain_column(&f, &token, "analyst").await;
    assert!(body.get("effect_line").is_none(), "body: {body}");
}

/// Spec §5.8 (D3, C9): a classification row filter's preview counts the
/// tables where the classification is on columns only, in catalogs a Trino
/// engine enforces it: there it filters nothing. A table carrying it as a
/// whole is not counted; a mask, and a catalog only StarRocks enforces, get
/// `null`. The stored rule's effect says the same.
#[tokio::test]
async fn a_classification_row_filter_preview_counts_trino_column_only_tables() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (catalog_id, _tag_service) = trino_tag_profile(&f, "vmask-d3-count").await;
    let name: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("catalog name");
    let _lock = exclusive(&f).await;
    let (key, tag_id) = classification_named(&f, &unique("VmaskD3")).await;
    let wh = warehouse_of(&f, catalog_id).await;
    assign(&f, tag_id, wh, "orders", "email").await;
    assign(&f, tag_id, wh, "orders", "phone").await;
    // On the whole table AND a column: the table attachment filters, so it
    // is not counted.
    assign(&f, tag_id, wh, "customers", "").await;
    assign(&f, tag_id, wh, "customers", "id").await;
    let selector = json!({"type": "tag", "key": key});

    let rule = row_filter_rule("vmask-d3", selector.clone(), "region = 'us'");
    let (status, body) = preview_rule(&f, &token, rule.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["effect"]["trinoColumnOnly"],
        json!({"catalogs": [name], "tables": 1}),
        "{body}"
    );
    let id = store_rule(&f, &token, rule).await;
    let (status, stored) = get_json(&f, &token, &format!("/perm/rules/{id}/effect")).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(
        stored["trinoColumnOnly"], body["effect"]["trinoColumnOnly"],
        "{stored}"
    );

    let (_, body) = preview_rule(&f, &token, mask_rule("vmask-d3-mask", selector, json!([]))).await;
    assert!(body["effect"]["trinoColumnOnly"].is_null(), "{body}");

    // The fixture's StarRocks-only catalog, its classification on a column.
    let (_, body) = preview_rule(
        &f,
        &token,
        row_filter_rule(
            "vmask-d3-sr",
            json!({"type": "tag", "key": f.tag_key}),
            "1=1",
        ),
    )
    .await;
    assert!(body["effect"]["trinoColumnOnly"].is_null(), "{body}");
}

/// Review finding 1: a rule whose OWN mask is `MASK_NONE` ("Unmasked"), or a
/// row filter whose own filter is empty, decides through its `{public}` item.
/// That is the rule's outcome for everyone, not an exemption, so explain must
/// not report one (nor call the compiled policy stale).
#[tokio::test]
async fn a_rule_whose_own_outcome_is_no_transform_is_not_an_exemption() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let path = |column: Value| {
        json!({"type": "path", "catalog_id": f.catalog_id,
               "schema": "sales", "table": "orders", "column": column})
    };
    let mut unmasked = mask_rule("vmask-unmasked", path(json!("email")), json!([]));
    unmasked["kind"]["mask_type"] = json!("MASK_NONE");
    let name = unmasked["name"].as_str().unwrap().to_string();
    store_rule(&f, &token, unmasked).await;
    store_rule(
        &f,
        &token,
        row_filter_rule("vmask-nofilter", path(Value::Null), ""),
    )
    .await;
    let (status, out) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {out}");

    let body = explain_column(&f, &token, "analyst").await;
    let starrocks = profile_for(&body, "starrocks");
    let mask = &starrocks["mask"];
    assert_eq!(
        mask["decision"]["result"]["mask_type"], "MASK_NONE",
        "body: {body}"
    );
    assert_eq!(mask["attribution"]["kind"], "rule", "body: {body}");
    assert!(
        mask.get("exemption").is_none(),
        "not an exemption; body: {body}"
    );
    let rows = &starrocks["row_filter"];
    assert_eq!(
        rows["decision"]["result"]["filter_expr"], "",
        "body: {body}"
    );
    assert!(
        rows.get("exemption").is_none(),
        "not an exemption; body: {body}"
    );
    assert_eq!(
        starrocks["effect_lines"][0]["text"],
        json!(format!(
            "analyst reads email as the real value (rule {name})"
        )),
        "body: {body}"
    );
}

/// Live check finding 2, the mirror of D3, fixed by the D2 StarRocks package
/// (spec 2026-10-25, C8): a classification row filter on a table-level
/// classification compiles on StarRocks, Explain evaluates it there and lists
/// no gap, and the rule preview no longer counts StarRocks tables it misses.
#[tokio::test]
async fn a_starrocks_row_filter_on_a_table_classification_filters_rows() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let tag = assign_tag(&f, f.catalog_id, "sales", "orders", "").await;
    let _lock = exclusive(&f).await;
    let (status, out) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &token,
                Some(grant_body(f.catalog_id, "analyst")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {out}");
    let rule = row_filter_rule(
        "vmask-sr-table",
        json!({"type": "tag", "key": tag.key}),
        "region = 'us'",
    );
    let (status, preview) = preview_rule(&f, &token, rule.clone()).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(
        preview["effect"].get("starrocksTableOnly").is_none(),
        "{preview}"
    );
    let rule_name = rule["name"].as_str().unwrap().to_string();
    let rule_id = store_rule(&f, &token, rule).await;

    let body = explain_with(&f, &token, explain_body(f.catalog_id, "analyst")).await;
    let sr = profile_for(&body, "starrocks");
    assert_eq!(
        sr["row_filter"]["decision"]["kind"], "filtered",
        "body: {body}"
    );
    assert_eq!(
        sr["row_filter"]["attribution"]["rule_id"],
        json!(rule_id),
        "body: {body}"
    );
    assert!(sr.get("row_filter_not_applied").is_none(), "body: {body}");
    assert_eq!(
        sr["effect_lines"][1]["text"],
        json!(format!(
            "analyst sees rows where region = 'us' (rule {rule_name})"
        )),
        "body: {body}"
    );
}

/// A classification rule is compiled for a Trino profile only through the
/// profile's `tagService`, and skipped (`MissingTagService`) when it names no
/// existing profile. So a Trino engine on such a profile does NOT enforce a
/// classification rule: `no_tag_service` per catalog and rule-wide. A path
/// rule on the same catalog IS enforced there (it needs no tag service), and a
/// Trino profile whose `tagService` resolves enforces the classification rule.
#[tokio::test]
async fn a_trino_engine_without_a_usable_tag_service_does_not_enforce_a_classification_rule() {
    let Some(f) = setup().await else { return };
    let admin = admin_session(&f).await;
    egret_policy::seed_tag(&f.policy).await.expect("seed tag");
    let tag_service = f
        .policy
        .create_service(&unique("eff_tagsvc"), "tag", &json!({}))
        .await
        .expect("tag service");
    let unset = trino_catalog_with_config(&f, "eff-nots", json!({})).await;
    let dangling =
        trino_catalog_with_config(&f, "eff-dangl", json!({"tagService": unique("nobody")})).await;
    let wired =
        trino_catalog_with_config(&f, "eff-wired", json!({"tagService": tag_service.name})).await;
    // After the fixtures: `trino_catalog_with_config` takes the lock itself.
    let _lock = exclusive(&f).await;
    let table = unique("eff_ts");
    let mut rules = Vec::new();
    for (catalog, label) in [(unset, "unset"), (dangling, "dangling"), (wired, "wired")] {
        let (key, tag_id) = classification_named(&f, &unique("EffTagSvc")).await;
        assign(&f, tag_id, warehouse_of(&f, catalog).await, &table, "email").await;
        let id = store_rule(
            &f,
            &admin,
            mask_rule(
                &format!("eff-ts-{label}"),
                json!({"type": "tag", "key": key}),
                json!([]),
            ),
        )
        .await;
        rules.push((catalog, id));
    }
    let path_rule = store_rule(
        &f,
        &admin,
        mask_rule(
            "eff-ts-path",
            json!({"type": "path", "catalog_id": unset, "schema": "s", "table": table, "column": "email"}),
            json!([]),
        ),
    )
    .await;

    let ids = rules
        .iter()
        .map(|(_, id)| id.clone())
        .chain([path_rule.clone()])
        .collect::<Vec<_>>()
        .join(",");
    let (status, got) = get_json(&f, &admin, &format!("/perm/rules/enforcement?ids={ids}")).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let entry = |id: &str| {
        got.as_array()
            .unwrap()
            .iter()
            .find(|e| e["ruleId"] == json!(id))
            .unwrap_or_else(|| panic!("{id} missing: {got}"))
            .clone()
    };
    for (catalog, id) in &rules[..2] {
        let e = entry(id);
        assert_eq!(e["enforcedBy"], 0, "{e}");
        assert_eq!(e["notEnforced"], "no_tag_service", "{e}");
        assert_eq!(e["catalogs"][0]["catalogId"], json!(catalog), "{e}");
        assert_eq!(e["catalogs"][0]["notEnforced"], "no_tag_service", "{e}");
        assert_eq!(e["catalogs"][0]["engines"], json!([]), "{e}");
        // The single read says the same.
        let (_, single) = get_json(&f, &admin, &format!("/perm/rules/{id}/effect")).await;
        assert_eq!(single["notEnforced"], "no_tag_service", "{single}");
    }
    let e = entry(&rules[2].1);
    assert_eq!(e["enforcedBy"], 1, "a resolving tagService enforces: {e}");
    assert!(e["notEnforced"].is_null(), "{e}");
    let e = entry(&path_rule);
    assert_eq!(e["enforcedBy"], 1, "a path rule needs no tag service: {e}");
    assert!(e["notEnforced"].is_null(), "{e}");
}

/// Bounded: more than `RULE_ENFORCEMENT_CAP` (100) distinct ids, or an id that
/// is not a UUID, is refused before anything is read; no ids is an empty list.
#[tokio::test]
async fn rule_enforcement_is_capped_at_one_hundred_ids() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    assert_eq!(egret_perm_compile::effect::RULE_ENFORCEMENT_CAP, 100);

    let ids = |n: usize| {
        (0..n)
            .map(|_| Uuid::new_v4().to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let (status, got) = get_json(
        &f,
        &token,
        &format!("/perm/rules/enforcement?ids={}", ids(100)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "100 is allowed: {got}");
    assert_eq!(got, json!([]), "unknown ids are left out");

    let (status, got) = get_json(
        &f,
        &token,
        &format!("/perm/rules/enforcement?ids={}", ids(101)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{got}");

    let (status, got) = get_json(&f, &token, "/perm/rules/enforcement?ids=not-a-uuid").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{got}");

    // The cap is checked while parsing: the 101st distinct id fails the
    // request before the rest of the list is read, so a bad id after it is
    // never reached and the refusal names the cap.
    let (status, got) = get_json(
        &f,
        &token,
        &format!("/perm/rules/enforcement?ids={},not-a-uuid", ids(101)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{got}");
    assert!(
        got.to_string().contains("at most 100 rule ids"),
        "the cap refuses first: {got}"
    );

    // Repeats are not distinct: 100 ids each asked twice is within the cap.
    let hundred = ids(100);
    let (status, got) = get_json(
        &f,
        &token,
        &format!("/perm/rules/enforcement?ids={hundred},{hundred}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "repeats count once: {got}");

    let (status, got) = get_json(&f, &token, "/perm/rules/enforcement?ids=").await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got, json!([]));
}

// ---------------------------------------------------------------------------
// grants-follow spec D10: writes wait their turn behind a held rename
// ---------------------------------------------------------------------------

/// Give the fixture's warehouse a Lakekeeper id, and hold the rename hold's
/// name lock on `sales.{table}` from a connection of the test's own, as a
/// held rename would. Dropping the connection releases it.
async fn hold_name(f: &Fixture, table: &str) -> sqlx::PgConnection {
    use sqlx::Connection as _;
    let warehouse: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("warehouse");
    let prefix: Option<String> =
        sqlx::query_scalar("SELECT lakekeeper_warehouse_id FROM catalog.warehouses WHERE id = $1")
            .bind(warehouse)
            .fetch_one(&f.db.pool)
            .await
            .expect("prefix");
    let prefix = match prefix {
        Some(p) => p,
        None => lakekeeper_stub::link_warehouse(&f.db, warehouse)
            .await
            .to_string(),
    };
    let mut conn = sqlx::PgConnection::connect(&f.dsn).await.expect("connect");
    let key = egret_perm_compile::view_tags::name_lock_key(&prefix, "sales", table);
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
        .bind(key)
        .fetch_one(&mut conn)
        .await
        .expect("lock");
    assert!(got, "the test's own lock");
    conn
}

fn deny_body(catalog_id: Uuid, table: &str) -> Value {
    json!({
        "effect": "deny",
        "actions": ["read"],
        "scope": { "catalog_id": catalog_id, "schema": "sales", "table": table, "column": null },
        "principals": { "include": [{ "kind": "user", "ref": "contractor" }], "exclude": [] }
    })
}

async fn send_json(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    into_json(
        f.app()
            .oneshot(req(method, uri, token, body))
            .await
            .unwrap(),
    )
    .await
}

fn assert_held(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "table_change_held", "{body}");
}

#[tokio::test]
async fn a_grant_write_on_a_held_name_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let _held = hold_name(&f, "orders").await;
    let (count, version) = (f.grant_count().await, f.policy_version().await);

    let (status, body) = send_json(
        &f,
        "POST",
        "/perm/grants",
        &token,
        Some(deny_body(f.catalog_id, "Orders")),
    )
    .await;
    assert_held(status, &body);
    assert_eq!(f.grant_count().await, count, "nothing written");
    assert_eq!(f.policy_version().await, version, "no recompile");
}

#[tokio::test]
async fn a_patch_moving_a_grant_off_a_held_name_is_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (status, body) = send_json(
        &f,
        "POST",
        "/perm/grants",
        &token,
        Some(deny_body(f.catalog_id, "orders")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    let _held = hold_name(&f, "orders").await;

    let (status, body) = send_json(
        &f,
        "PATCH",
        &format!("/perm/grants/{id}"),
        &token,
        Some(deny_body(f.catalog_id, "other")),
    )
    .await;
    assert_held(status, &body);
    let table: String =
        sqlx::query_scalar("SELECT table_name FROM perm.grants WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&f.db.pool)
            .await
            .expect("grant");
    assert_eq!(table, "orders");
}

#[tokio::test]
async fn a_rule_write_and_deletes_on_a_held_name_are_refused() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let (status, body) = send_json(
        &f,
        "POST",
        "/perm/grants",
        &token,
        Some(deny_body(f.catalog_id, "orders")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let grant = body["id"].as_str().expect("id").to_string();
    let mask = |name: String| {
        json!({
            "name": name,
            "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
            "selector": { "type": "path", "catalog_id": f.catalog_id,
                          "schema": "sales", "table": "orders", "column": "phone" },
            "exempt": { "include": [], "exclude": [] }
        })
    };
    let (status, body) =
        send_json(&f, "POST", "/perm/rules", &token, Some(mask(unique("m")))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let rule = body["id"].as_str().expect("id").to_string();
    let _held = hold_name(&f, "orders").await;

    let (status, body) =
        send_json(&f, "POST", "/perm/rules", &token, Some(mask(unique("m2")))).await;
    assert_held(status, &body);
    let (status, body) =
        send_json(&f, "DELETE", &format!("/perm/grants/{grant}"), &token, None).await;
    assert_held(status, &body);
    let (status, body) =
        send_json(&f, "DELETE", &format!("/perm/rules/{rule}"), &token, None).await;
    assert_held(status, &body);
}

#[tokio::test]
async fn a_pattern_write_takes_no_name_lock() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let _held = hold_name(&f, "orders").await;
    let (status, body) = send_json(
        &f,
        "POST",
        "/perm/grants",
        &token,
        Some(deny_body(f.catalog_id, "ord*")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn a_grant_on_a_catalog_without_an_embedded_warehouse_is_not_locked() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let _held = hold_name(&f, "orders").await;
    let other: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'iceberg', '{}'::jsonb) \
         RETURNING id",
    )
    .bind(unique("perm_routes_hive"))
    .fetch_one(&f.db.pool)
    .await
    .expect("catalog");
    let (status, body) = send_json(
        &f,
        "POST",
        "/perm/grants",
        &token,
        Some(deny_body(other, "orders")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}
