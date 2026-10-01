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
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
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
    tag_key: String,
    /// The same tag's `tags.name`, which is a DIFFERENT string on purpose: the
    /// wire reports the name (what the engine matches tag policies on) and the
    /// rule selectors take the key, so a fixture where they coincide cannot tell
    /// a key/name confusion from a correct answer.
    tag_name: String,
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
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
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
        tag_name,
    })
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

/// A session token whose `Principal` is a platform `policy:read` holder reached
/// **through a `ws_admin`'s workspace-scoped impersonation overlay** — the only
/// way a workspace id reaches a `/api/control-plane` handler at all.
///
/// Built the way production builds it: the overlay row hangs off the
/// IMPERSONATOR's own session (`SessionService::validate` consults
/// `active_for_session(row.session_id)`), and resolving that session yields the
/// target's principal plus `Impersonator { workspace_scope: Some(scope) }`.
///
/// ⚠ **This principal is no longer reachable from the real
/// `POST /api/ws/{workspace}/impersonate` flow, and this comment used to say it
/// was.** When Task 4 wrote it that was true:
/// `egret_auth::impersonation::validate_impersonation_target` refused a
/// `platform_admin` target and nothing else, and migration 0059 seeds the platform
/// `data_steward` role with the whole `policy:*` family, so a `ws_admin` could
/// start exactly this overlay. SP4d debt **Task 13 closed that at the source**:
/// the validator now refuses a target holding ANY platform-scoped role grant, so
/// no entry point will mint a scoped overlay over a `policy:*` holder.
///
/// The helper writes the overlay row directly (`ImpersonationRepo::start`), which
/// is deliberate and is the only way these tests can still reach the state they
/// guard. What they now hold is the SECOND line: the containment that would still
/// stand if a future entry point wrote a scoped overlay row without going through
/// that validator. `a_ws_admin_may_not_impersonate_a_platform_permission_holder`
/// in `egret-auth` holds the first line, and
/// `the_ws_impersonate_route_refuses_a_platform_role_holding_member` in
/// `egret-workspace` holds it over the real HTTP route.
async fn ws_scoped_overlay_session(f: &mut Fixture, scope: Uuid) -> String {
    ws_scoped_overlay_session_with(f, scope, &["policy:read"]).await
}

/// [`ws_scoped_overlay_session`] with the target's platform permission set spelled
/// out — `policy:update` for the write routes, which is the permission migration
/// 0059's `data_steward` carries alongside `policy:read`.
async fn ws_scoped_overlay_session_with(
    f: &mut Fixture,
    scope: Uuid,
    permissions: &[&str],
) -> String {
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
    issued.token
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
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
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
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique(&format!("{tag}_trino_eng")))
    .bind(trino_service.id)
    .fetch_one(&f.db.pool)
    .await
    .expect("trino engine");
    let starrocks_engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
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
/// `0167`'s CHECK would refuse the INSERT whatever happens in Rust, and on a
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
    // with nothing named beneath it, which is what `0167` must not refuse.
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
/// refuse this input and the test has to be pointed at one. `0168`'s CHECK would
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
         VALUES ($1,'trino','egret','{}'::jsonb,$2)",
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

/// **The meaning inverts, and that is the point (SP4d-2a Task 5).**
///
/// A grant reachable only through a GROUP is allowed by the model and refused
/// by every engine egret provisions: D7 measured no usersync and no group
/// provider, so a Ranger item naming a group matches nobody. While `explain`
/// answered with the model it had to report that allow under its own reason
/// class (`allowed_by_group_only`) so a caller parsing JSON could see the
/// warning the page printed. Measured against the artifact the answer is
/// simply a REFUSAL — so the reason class disappears because the defect it
/// existed to flag disappears with it.
///
/// The compiled body is read as ground truth first, because the refusal has to
/// come from the engine's inability to SEE a group and not from a policy that
/// was never written: the compiler does emit `groups`, and this asserts it.
#[tokio::test]
async fn a_group_only_grant_is_refused_by_the_artifact_the_engine_downloads() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let group = unique("contractors");
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

    // Ground truth: the policy the engine downloads really does name the group,
    // and names no user at all.
    let compiled = f
        .compiled_policy_body(&format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
        .await;
    assert_eq!(
        compiled["policyItems"][0]["groups"],
        json!([group]),
        "the compiler emits `groups`; the refusal below must be about what the \
         engine can MATCH, not about a missing policy: {compiled:#}"
    );
    assert_eq!(
        compiled["policyItems"][0]["users"],
        json!([]),
        "and no user: {compiled:#}"
    );

    let subject = unique("grouponly");
    let mut ask = explain_body(f.catalog_id, &subject);
    // Named anyway, to prove naming it changes nothing — `ExplainSubject` has
    // no `groups` field left and the evaluator receives an empty set.
    ask["subject"]["groups"] = json!([group]);
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
        "the model allows this and no engine will, so the ARTIFACT's answer is a \
         refusal — there is no longer an allow to warn about; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "no_policy_matched",
        "the group item matched nobody, which is `-` in Ranger's own audit; body: {body}"
    );
    assert_eq!(
        verdict["attribution"],
        Value::Null,
        "nothing decided, so nothing to attribute; body: {body}"
    );
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
/// therefore asserts the NAME (not the key), and that a column-level assignment
/// reports `attached_at_column: true` — the half of the answer that tells a
/// steward which row to change.
#[tokio::test]
async fn explain_reports_the_tag_set_the_evaluator_was_handed() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;
    let mut body = explain_body(f.catalog_id, "analyst");
    body["target"]["column"] = json!("email");

    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/explain", &token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["tags"],
        json!([{ "name": f.tag_name, "attached_at_column": true }]),
        "sales.orders.email carries this fixture's tag, reported by NAME and as a \
         column-level attachment; the KEY is {} and must not appear; body: {body}",
        f.tag_key
    );
    // And the tag did not become an evaluator input: this profile's download
    // carries no `tagPolicies` block, so the verdict is a decision rather than
    // the refusal a block that could decide would produce.
    assert_eq!(
        access_for(profile_for(&body, "starrocks"), "select")["decision"]["kind"],
        "denied",
        "body: {body}"
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
    let verdict = access_for(profile_for(&body, "starrocks"), "select");
    assert_eq!(
        verdict["decision"]["kind"], "denied",
        "a caller-supplied role must not clear a subject who does not hold it — this is \
         the live gate's `{{\"user\":\"analyst\",\"roles\":[\"sp4c_cleared\"]}}` answering \
         `allowed` for the user both engines refuse; body: {body}"
    );
    assert_eq!(
        verdict["decision"]["reason"], "deny_policy_item",
        "and the refusal must be the clearance's own deny, not a grant miss; body: {body}"
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

// ---------------------------------------------------------------------------
// compile (the manual repair trigger)
// ---------------------------------------------------------------------------

/// `POST /compile/{service_id}` brings a profile back in line with the model
/// after something outside the model made it drift. Proven by deleting the
/// compiled policy row directly — the one thing no endpoint does — and
/// watching the trigger restore it.
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

    let user = unique("two-profile-reader");
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

    let user = unique("two-profile-writer");

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

/// **The workspace boundary that DOES reach this handler, and it arrives by
/// impersonation.**
///
/// `egret_workspace::context::WsContext` confines a scoped overlay, but only on a
/// route that resolves a `{workspace}` path segment — the only thing that builds a
/// `WsContext`. No `/api/control-plane` route resolves one. (Its comment used to
/// call that containment "airtight … no route can reach another workspace under a
/// scoped overlay"; Task 13 corrected it at the site.)
///
/// ⚠ **The path this test was written against is now closed at its source, so
/// what this test holds is the SECOND line, not the live gate.** When Task 4 wrote
/// it, `validate_impersonation_target` refused only a `platform_admin` target, so a
/// `ws_admin` of workspace A could impersonate a member of A holding the platform
/// `data_steward` role (migration 0059 seeds it with the whole `policy:*` family)
/// and ask about a catalog of workspace B. Task 13 made that validator refuse ANY
/// platform-role-holding target; see `ws_scoped_overlay_session`, which writes the
/// overlay row directly for exactly that reason.
///
/// Three rows, because a guard that refused everything under an overlay would
/// pass the first alone:
/// 1. the overlay's OWN workspace's catalog is answered;
/// 2. a catalog that workspace is not assigned is `403`;
/// 3. the same catalog, same steward, with NO overlay, is answered — so the
///    refusal is the containment and not the permission.
#[tokio::test]
async fn a_workspace_scoped_overlay_is_confined_to_that_workspaces_catalogs() {
    let Some(mut f) = setup().await else { return };
    let scope = new_workspace(&f, "perm-task4-scope").await;
    assign_catalog_to_workspace(&f, f.catalog_id, scope).await;
    // A second catalog the overlay's workspace is NOT assigned — and it is
    // deliberately assigned to a DIFFERENT workspace rather than to none.
    //
    // ⚠ **It used to have no assignment at all, and a mutation proved that
    // worthless.** With `registry.catalog_workspaces` empty for it, an `EXISTS`
    // that dropped `AND workspace_id = $2` — i.e. "is this catalog assigned to
    // ANY workspace" — still produced the 403 and the test passed. That mutant is
    // the real leak: a catalog of workspace B IS assigned to B, so a containment
    // keyed to the wrong half of the relation would answer it for an overlay
    // scoped to A. The row below is what makes the two distinguishable.
    let foreign = orphan_catalog(&f, "perm-task4-foreign-cat").await;
    let foreign_ws = new_workspace(&f, "perm-task4-foreign-ws").await;
    assign_catalog_to_workspace(&f, foreign, foreign_ws).await;

    let overlay = ws_scoped_overlay_session(&mut f, scope).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &overlay,
                Some(explain_body(f.catalog_id, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the overlay's own workspace is assigned this catalog, so the support flow \
         the overlay exists for still works; body: {body}"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &overlay,
                Some(explain_body(foreign, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a ws-scoped overlay must not be able to read another workspace's \
         enforcement through a platform route; body: {body}"
    );

    // Same target, same steward permission, no overlay: answered. So row 2 is the
    // containment and not a missing permission — and a guard keyed to the
    // PERMISSION instead of the overlay would fail here.
    let plain = session_with_permission(&mut f, "policy:read").await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/explain",
                &plain,
                Some(explain_body(foreign, "nobody")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "without an overlay the same question is a platform steward's to ask; body: {body}"
    );
}

// ---------------------------------------------------------------------------
// SP4d debt Task 13: the WRITE half of the same containment
// ---------------------------------------------------------------------------

/// Compiled policies under the fixture's own profile — the table the engine's
/// Ranger plugin downloads. A cross-workspace *write* is only interesting if it
/// reaches this, so the test asserts on it rather than on `perm.grants` alone.
async fn policies_on(f: &Fixture, service_id: i64) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
        .bind(service_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count policies")
}

/// **The write half Task 4 named open, measured on a route that mutates.**
///
/// Task 4 confined `explain` only, and recorded that the same router's write
/// routes were open: a `ws_admin` of workspace A, impersonating a member of A who
/// holds the platform `data_steward` role, could `POST /perm/grants` naming a
/// catalog of workspace B — a grant that is COMPILED by the same request into the
/// Ranger artifact B's engine downloads. That is strictly worse than reading an
/// explain, and it is what this test refuses.
///
/// Three rows, in the shape Task 4's mutation round taught:
/// 1. the foreign catalog is `403` and **nothing is written** — asserted against
///    `perm.grants` AND `policy.authz_policies`, because the 403 would be worth
///    little if the row had already been compiled;
/// 2. the overlay's OWN workspace's catalog is still `201`, so this is not a
///    blanket refusal of every write under an overlay;
/// 3. the same foreign body with the same permission and NO overlay is `201`, so
///    row 1 is the containment and not a missing permission.
///
/// The foreign catalog is the fixture's own `f.catalog_id` on purpose: it is
/// attached to an engine bound to `f.service_id`, so a grant on it compiles to a
/// real policy row. An orphan catalog would have made row 1 pass against a
/// `policy.authz_policies` assertion that could never have changed.
///
/// ⚠ **Both catalogs carry a `registry.catalog_workspaces` row.** A fixture whose
/// "foreign" catalog had no assignment at all let Task 4's M8 mutant survive — an
/// `EXISTS` that dropped `AND workspace_id = $2` still produced a 403 — while that
/// mutant is the actual leak.
#[tokio::test]
async fn a_workspace_scoped_overlay_cannot_create_a_grant_on_another_workspaces_catalog() {
    let Some(mut f) = setup().await else { return };
    // The overlay's own workspace, with a catalog of its own so row 2 is askable.
    let scope = new_workspace(&f, "perm-task13-scope").await;
    let own = orphan_catalog(&f, "perm-task13-own-cat").await;
    assign_catalog_to_workspace(&f, own, scope).await;
    // The other workspace's catalog: served by `f.service_id`, and assigned.
    let foreign_ws = new_workspace(&f, "perm-task13-foreign-ws").await;
    assign_catalog_to_workspace(&f, f.catalog_id, foreign_ws).await;

    let grants_before = grants_on(&f, f.catalog_id).await;
    let policies_before = policies_on(&f, f.service_id).await;

    let overlay = ws_scoped_overlay_session_with(&mut f, scope, &["policy:update"]).await;

    // 1. The foreign catalog.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &overlay,
                Some(grant_body(f.catalog_id, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a ws-scoped overlay must not be able to WRITE a permission grant on \
         another workspace's catalog; body: {body}"
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

    // 2. The overlay's own workspace's catalog: still writable.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &overlay,
                Some(grant_body(own, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the overlay's own workspace is assigned this catalog, so authoring there \
         must still work — a blanket refusal would pass row 1 alone; body: {body}"
    );

    // 3. The same foreign body, same permission, no overlay.
    let plain = session_with_permission(&mut f, "policy:update").await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/grants",
                &plain,
                Some(grant_body(f.catalog_id, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "without an overlay this is a platform steward's write to make, so row 1 \
         is the containment and not the permission; body: {body}"
    );
}

/// The three write shapes a single catalog check would miss, each a separate
/// refusal in `write_grant` / `write_rule` / the delete handlers.
///
/// 1. **A PATCH that moves a grant OUT of the overlay's workspace** — caught by
///    confining the body's catalog.
/// 2. **A PATCH of a FOREIGN grant into the overlay's own workspace** — caught
///    only by confining the STORED row's catalog. Confining the body alone accepts
///    this, and it is a write to another workspace's model (the foreign profile is
///    recompiled and the grant leaves it).
/// 3. **A DELETE of a foreign grant** — no body exists to confine, so the
///    containment has to run on the stored row.
///
/// Plus the rule surface, which has a selector rather than a scope:
///
/// 4. a **tag rule**, which names no catalog at all and whose fan-out is the whole
///    installation (`affected_services_for_rule` walks every profile), so a
///    confined caller may not author one in any form;
/// 5. a **path-selector rule** on the foreign catalog — the rule twin of
///    `create_grant`, and the only leg that reaches `confine_rule_selector`'s
///    `Path` arm;
/// 6. a **DELETE of a foreign path rule**, the rule half of row 3.
///
/// The last row is a control: the same tag rule with the same permission and NO
/// overlay is created, so none of the six refusals above can be satisfied by a
/// route that refuses everything.
#[tokio::test]
async fn a_workspace_scoped_overlay_cannot_patch_delete_or_fan_out_across_the_boundary() {
    let Some(mut f) = setup().await else { return };
    let scope = new_workspace(&f, "perm-task13b-scope").await;
    let own = orphan_catalog(&f, "perm-task13b-own-cat").await;
    assign_catalog_to_workspace(&f, own, scope).await;
    let foreign_ws = new_workspace(&f, "perm-task13b-foreign-ws").await;
    assign_catalog_to_workspace(&f, f.catalog_id, foreign_ws).await;

    // Two grants, both written WITHOUT an overlay (a platform steward's writes),
    // so the overlay below is only ever asked to change existing state.
    let plain = session_with_permission(&mut f, "policy:update").await;
    let own_grant = created_id(&f, &plain, grant_body(own, "alice")).await;
    let foreign_grant = created_id(&f, &plain, grant_body(f.catalog_id, "alice")).await;

    let overlay = ws_scoped_overlay_session_with(&mut f, scope, &["policy:update"]).await;

    // 1. Move the overlay's own grant OUT, to the foreign catalog.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{own_grant}"),
                &overlay,
                Some(grant_body(f.catalog_id, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a confined caller must not move a grant onto another workspace's \
         catalog; body: {body}"
    );

    // 2. Drag the FOREIGN grant into the overlay's own workspace. Only the
    //    stored-row confinement refuses this one — the body names `own`, which is
    //    assigned to the overlay's workspace, so the body check passes.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &format!("/perm/grants/{foreign_grant}"),
                &overlay,
                Some(grant_body(own, "alice")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a confined caller must not edit another workspace's grant, even to point \
         it at its own catalog — the foreign profile is recompiled and the grant \
         leaves it; body: {body}"
    );

    // 3. Delete the foreign grant.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/perm/grants/{foreign_grant}"),
                &overlay,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a confined caller must not delete another workspace's grant; body: {body}"
    );
    assert_eq!(
        grants_on(&f, f.catalog_id).await,
        1,
        "the foreign grant must still be there after all three refusals"
    );

    // 4. A tag rule names no catalog and fans out over the installation.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &overlay,
                Some(json!({
                    "name": unique("task13-fanout"),
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a tag rule's affected set is every profile in the installation, so a \
         confined caller may not author one; body: {body}"
    );

    // 5. A PATH-selector rule naming the foreign catalog: the rule twin of row 1,
    //    and the only leg that exercises `confine_rule_selector`'s `Path` arm.
    let foreign_path_rule = json!({
        "name": unique("task13-path"),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "path", "catalog_id": f.catalog_id,
                      "schema": "sales", "table": "orders", "column": "email" },
        "exempt": { "include": [], "exclude": [] }
    });
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &overlay,
                Some(foreign_path_rule.clone()),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a confined caller must not author a masking rule over another \
         workspace's catalog; body: {body}"
    );

    // 6. And must not DELETE one, which is the rule half of row 3.
    let (status, created) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/rules", &plain, Some(foreign_path_rule)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "setup rule write failed: {created}"
    );
    let foreign_rule_id = created["id"].as_str().expect("minted rule id").to_string();
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/perm/rules/{foreign_rule_id}"),
                &overlay,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a confined caller must not delete another workspace's rule; body: {body}"
    );

    // Control: the same tag rule, same permission, no overlay. Without this the
    // refusals above would all pass against a route that refuses everything.
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                "/perm/rules",
                &plain,
                Some(json!({
                    "name": unique("task13-fanout-ok"),
                    "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
                    "selector": { "type": "tag", "key": f.tag_key },
                    "exempt": { "include": [], "exclude": [] }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a platform steward with no overlay authors tag rules; body: {body}"
    );
}

/// `POST /perm/grants` with `body`, returning the minted id. Panics unless the
/// write was accepted, so a setup failure cannot be mistaken for the behaviour
/// under test.
async fn created_id(f: &Fixture, token: &str, body: Value) -> Uuid {
    let (status, json) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/grants", token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "setup write failed: {json}");
    json["id"]
        .as_str()
        .expect("minted id")
        .parse()
        .expect("uuid")
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
///    detected that at read time; migration 0166's
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
        .expect_err("migration 0166 must refuse a row with an empty table_name");
    let named = match &e {
        sqlx::Error::Database(d) => {
            d.constraint() == Some("catalog_tag_assignments_levels_are_present_and_literal")
        }
        _ => false,
    };
    assert!(
        named,
        "the refusal must be migration 0166's CHECK, not a typo or a dropped \
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
    let tag_name = assign_tag(&f, catalog_id, "sales", "orders", "").await;
    bind_tag_service_with_one_policy(&f, trino_id, &tag_name).await;

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

/// One `tags` row plus one `catalog_tag_assignments` row on this catalog's
/// warehouse, returning `tags.name` — which is what the tag WIRE carries as the
/// tag's `type` and therefore what a tag policy's `resources.tag.values` must
/// name. Never `tags.key`: live the two differ, and that is the fail-open D3
/// names.
async fn assign_tag(
    f: &Fixture,
    catalog_id: Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> String {
    let warehouse_id: Uuid =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("catalog warehouse");
    let name = unique("t7tag");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(unique("t7key"))
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
    name
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
#[tokio::test]
async fn the_artifact_answer_and_the_model_oracle_agree_on_the_happy_path() {
    let Some(f) = setup().await else { return };
    let token = admin_session(&f).await;

    let granted = unique("oracle-granted");
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
    let target = egret_perm::Scope {
        catalog_id: f.catalog_id,
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: None,
    };

    for (user, expected) in [(&granted, true), (&stranger, false)] {
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
            "the model oracle must {expected} {user}: {oracle:?}"
        );

        let (status, body) = into_json(
            f.app()
                .oneshot(req(
                    "POST",
                    "/perm/explain",
                    &token,
                    Some(explain_body(f.catalog_id, user)),
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
            "the artifact and the model oracle disagree about {user}: oracle {oracle:?}, \
             artifact {kind}; body: {body}"
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

    let user = unique("assembler-subject");
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
