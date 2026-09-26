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
/// two tests that care (`creating_a_tag_rule_over_http_compiles_its_policy`,
/// which causes the fan-out, and `compile_restores_a_profile_that_drifted`,
/// which is the one damaged by it) take this lock so neither has to weaken its
/// assertion to a "either the endpoint repaired it or somebody else did" shape.
///
/// A dedicated, NON-pooled connection: a session advisory lock is released
/// when the session ends, and a pooled connection returned to the pool keeps
/// its session — and its lock — alive.
async fn exclusive(f: &Fixture) -> sqlx::PgConnection {
    use sqlx::Connection as _;
    let mut conn = sqlx::PgConnection::connect(&f.dsn)
        .await
        .expect("lock conn");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(TAG_FANOUT_LOCK)
        .execute(&mut conn)
        .await
        .expect("acquire advisory lock");
    conn
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
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM policy.authz_policies \
             WHERE service_id = $1 AND (name LIKE $2 || '%' OR name LIKE $3 || '%') \
             ORDER BY name",
        )
        .bind(self.service_id)
        .bind(COMPILER_GRANT_PREFIX)
        .bind(COMPILER_RULE_PREFIX)
        .fetch_all(&self.db.pool)
        .await
        .expect("read compiled policies")
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
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(&tag_key)
            .bind(unique("PII"))
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

/// A NON-admin session holding exactly one fine-grained permission key. The
/// PDP snapshots the role catalog at build time, so the role is seeded and
/// then the fixture's PDP is rebuilt.
async fn session_with_permission(f: &mut Fixture, permission: &str) -> String {
    let uid = new_user(f, "perm_scoped").await;
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
    f.sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
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

    // A registry catalog attached to no engine at all, under its own
    // warehouse (`registry.catalogs` is UNIQUE on `warehouse_id`).
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("perm-orphan-ws"))
            .bind("Perm Orphan WS")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-perm-orphan-test','AKIATEST') RETURNING id",
    )
    .bind(unique("perm-orphan-sc"))
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
    let orphan: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("perm_routes_orphan"))
    .bind(orphan_wh)
    .fetch_one(&f.db.pool)
    .await
    .expect("orphan catalog");

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
// explain
// ---------------------------------------------------------------------------

fn explain_body(catalog_id: Uuid, user: &str) -> Value {
    json!({
        "subject": { "user": user, "groups": [], "roles": [] },
        "action": "read",
        "target": { "catalog_id": catalog_id, "schema": "sales", "table": "orders", "column": null }
    })
}

/// The brief's test: `explain` must name the reason CLASS, not just a verdict.
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
    assert_eq!(body["allowed"], false);
    assert_eq!(
        body["reasons"][0]["kind"], "no_grant_matched",
        "Ranger's audit shows '-' for exactly this case; naming it is the point"
    );
}

/// `no_grant_matched` and `explicit_deny` are different facts about the model
/// and must stay different on the wire. A response that collapsed both into
/// `allowed: false` would pass the test above and still be useless.
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
    assert_eq!(body["allowed"], false);
    assert_eq!(body["reasons"][0]["kind"], "explicit_deny");
    assert_eq!(
        body["reasons"][0]["grant_id"], grant_id,
        "the reason must name WHICH grant denied it; body: {body}"
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

/// `explain` resolves the target's tags itself. If it made the caller supply
/// them it would answer a question the caller had already decided.
#[tokio::test]
async fn explain_resolves_the_targets_tags_itself() {
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
        json!([f.tag_key]),
        "sales.orders.email carries this fixture's tag; body: {body}"
    );
}

/// SP4a Task 15, D3 defect 2: `explain` and the compiled policy must agree
/// about an IMPLICIT (`workspace_members`) grant, for a member whose stored
/// `users.username` carries an `ensure_username` collision suffix (`-2`) —
/// the exact case a naive re-derivation from the email would get wrong (it
/// would name the FIRST colliding person for the SECOND). Two users share one
/// email local-part on purpose, so `collide` and `collide-2` are real,
/// minted, distinguishable identities before this test ever calls `explain`.
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
    assert_eq!(
        body["allowed"], true,
        "explain must allow the collision-suffixed member the compiled policy \
         already serves; body: {body}"
    );
    assert_eq!(
        body["reasons"][0]["kind"], "allowed_by_grant",
        "body: {body}"
    );
    assert_eq!(
        body["reasons"][0]["grant_id"],
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
    assert_eq!(body_a["allowed"], true, "body: {body_a}");
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
