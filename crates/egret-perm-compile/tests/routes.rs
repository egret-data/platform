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
