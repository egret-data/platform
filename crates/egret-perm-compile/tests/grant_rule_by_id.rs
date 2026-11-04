//! The grant and rule by-id write path, driven through the real router:
//!
//! * **#789** — a PATCH takes its old affected set from the row re-read under
//!   its locks, not from the read it made before them, so a competing PATCH that
//!   committed in between is accounted for;
//! * **#801** — a grant or rule id outside a confined caller's workspace answers
//!   exactly like an id that names nothing.
//!
//! Its own binary on purpose, so its own database (`egret_test_db`: one per test
//! process). `tests/routes.rs` serialises every profile it creates on one
//! advisory lock (`TAG_FANOUT_LOCK`) whose waiters give up after 60 s, and it
//! already holds that lock for most of its run; nothing here creates a tag rule,
//! so nothing here needs it.
//!
//! DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`,
//! and otherwise FAILS.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermHttpError, PermState};
use egret_policy::{PolicyStore, COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

#[path = "support/lakekeeper_stub.rs"]
mod lakekeeper_stub;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    sessions: SessionService,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    /// A StarRocks profile with one engine bound to it and one registry
    /// catalog attached to that engine.
    service_id: i64,
    catalog_id: Uuid,
    /// A username that resolves, for grant bodies (J-F3 refuses one that does
    /// not).
    analyst: String,
}

impl Fixture {
    fn app(&self) -> axum::Router {
        perm_router().with_state(PermState {
            opa_cache: None,
            perm: self.perm.clone(),
            policy: self.policy.clone(),
            sessions: self.sessions.clone(),
            pdp: self.pdp.clone(),
        })
    }

    /// The compiler-owned policy names under `service_id` — the compiled output
    /// that profile's engine downloads.
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
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    // The embedded catalog's tables the StarRocks column source reads (D2).
    lakekeeper_stub::install(&db).await;
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let (service_id, catalog_id) = profile(&db, &policy, "by-id").await;
    let analyst = unique("analyst");
    sqlx::query("INSERT INTO users (provider, email, username) VALUES ('local', $1, $2)")
        .bind(format!("{analyst}@perm-by-id.egret.local"))
        .bind(&analyst)
        .execute(&db.pool)
        .await
        .expect("analyst");
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    Some(Fixture {
        perm: PermStore::new(db.pool.clone()),
        policy,
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        pdp,
        db,
        service_id,
        catalog_id,
        analyst,
    })
}

/// A StarRocks profile with an engine bound to it and one catalog attached to
/// that engine: `(service_id, catalog_id)`.
async fn profile(db: &Db, policy: &PolicyStore, tag: &str) -> (i64, Uuid) {
    let pool = &db.pool;
    let service = policy
        .create_service(&unique(tag), "starrocks", &json!({}))
        .await
        .expect("create service");
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique(tag))
            .bind("Perm By-Id WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-perm-by-id-test','AKIATEST') RETURNING id",
    )
    .bind(unique(tag))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("perm_by_id_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique(tag))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("perm_by_id_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
    (service.id, catalog_id)
}

/// A catalog assigned to a fresh workspace, served by no engine.
async fn catalog_of_new_workspace(f: &Fixture, tag: &str) -> (Uuid, Uuid) {
    let (_, catalog_id) = profile(&f.db, &f.policy, tag).await;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique(tag))
            .bind("Perm By-Id Scope")
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1,$2)",
    )
    .bind(catalog_id)
    .bind(ws)
    .execute(&f.db.pool)
    .await
    .expect("assign catalog to workspace");
    (ws, catalog_id)
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

async fn session_for(f: &Fixture, role: Option<&str>) -> String {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!(
        "{}@perm-by-id.egret.local",
        Uuid::new_v4().simple()
    ))
    .fetch_one(&f.db.pool)
    .await
    .expect("user");
    if let Some(role) = role {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name=$2",
        )
        .bind(uid)
        .bind(role)
        .execute(&f.db.pool)
        .await
        .expect("grant role");
    }
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

fn rule_body(name: &str, catalog_id: Uuid) -> Value {
    json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "path", "catalog_id": catalog_id,
                      "schema": "sales", "table": "orders", "column": "email" },
        "exempt": { "include": [], "exclude": [] }
    })
}

async fn create(f: &Fixture, token: &str, kind: &str, body: Value) -> Uuid {
    let (status, created) = into_json(
        f.app()
            .oneshot(req("POST", &format!("/perm/{kind}"), token, Some(body)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create {kind}; body: {created}"
    );
    Uuid::parse_str(created["id"].as_str().expect("minted id")).unwrap()
}

/// A synthesized `Principal` inside a ws-scoped impersonation overlay — the only
/// shape the containment confines. Synthesized because `SessionService::validate`
/// never builds one that carries platform authority (see
/// `confine_to_overlay_workspace`'s ⚠ paragraph).
fn principal_confined_to(ws: Uuid) -> egret_core::Principal {
    egret_core::Principal {
        impersonator: Some(egret_core::context::Impersonator {
            user_id: egret_core::UserId(Uuid::new_v4()),
            email: "admin@egret.local".to_string(),
            workspace_scope: Some(egret_core::WorkspaceId(ws)),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(30),
        }),
        ..principal_unconfined()
    }
}

fn principal_unconfined() -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(Uuid::new_v4()),
        email: "steward@egret.local".to_string(),
        platform_roles: vec!["data_steward".to_string()],
        permissions: vec!["policy:update".to_string()],
        group_ids: Vec::new(),
        impersonator: None,
    }
}

// ---------------------------------------------------------------------------
// #789: the old affected set comes from the row as it stands under the locks
// ---------------------------------------------------------------------------

/// **#789.** A PATCH reads the stored grant before it takes any lock, so a
/// concurrent PATCH of the same grant can commit in between. Its old affected
/// set must come from the row re-read under the locks, not from that first
/// read. Otherwise it misses the profile the concurrent patch moved the grant
/// to, and the re-check after the profile locks answers a 409 for a sound write
/// (without that re-check, the moved-to profile would keep a policy nothing
/// produces).
///
/// Deterministic through `egret_perm_compile::write_seam`: our PATCH (to
/// catalog C) stops right after its first read; a second PATCH (to catalog B)
/// runs to commit; ours resumes. Before the fix: 409. After: 200, B's policy
/// reaped, C's compiled.
#[tokio::test]
async fn a_patch_racing_a_committed_patch_derives_its_old_set_from_the_fresh_row() {
    let Some(f) = setup().await else { return };
    let token = session_for(&f, Some("platform_admin")).await;
    let (service_b, catalog_b) = profile(&f.db, &f.policy, "fresh-b").await;
    let (service_c, catalog_c) = profile(&f.db, &f.policy, "fresh-c").await;
    let grant_id = create(&f, &token, "grants", grant_body(f.catalog_id, &f.analyst)).await;
    let name = format!("{COMPILER_GRANT_PREFIX}{grant_id}");
    assert_eq!(
        f.compiled_policy_names_under(f.service_id).await,
        vec![name.clone()]
    );

    // Ours: parked between its first read and its lock wait.
    let (point, pause) = egret_perm_compile::write_seam::PausePoint::new();
    let uri = format!("/perm/grants/{grant_id}");
    let ours = tokio::spawn(egret_perm_compile::write_seam::run_with(point, {
        let (app, uri, token) = (f.app(), uri.clone(), token.clone());
        let body = grant_body(catalog_c, &f.analyst);
        async move {
            app.oneshot(req("PATCH", &uri, &token, Some(body)))
                .await
                .unwrap()
        }
    }));
    pause
        .reached
        .await
        .expect("our PATCH reached the pause point");

    // Theirs: commits while ours waits.
    let (status, theirs) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &uri,
                &token,
                Some(grant_body(catalog_b, &f.analyst)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the competing patch; body: {theirs}"
    );
    assert_eq!(
        f.compiled_policy_names_under(service_b).await,
        vec![name.clone()]
    );

    pause.go.send(()).expect("resume our PATCH");
    let (status, got) = into_json(ours.await.expect("our PATCH task")).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "the row re-read under the locks says the grant is on B, so B belongs in the \
         old set and nothing escaped it; body: {got}"
    );
    assert_eq!(
        f.compiled_policy_names_under(service_b).await,
        Vec::<String>::new(),
        "the profile the competing patch moved the grant to must be reaped; body: {got}"
    );
    assert_eq!(f.compiled_policy_names_under(service_c).await, vec![name]);
    let services = got["services"].as_array().expect("services");
    assert!(
        services.iter().any(|v| v.as_i64() == Some(service_b)),
        "B is in the recompiled set; body: {got}"
    );
}

/// The rule twin of the test above: `write_rule` derives its old set from the
/// rule re-read under the group locks too.
#[tokio::test]
async fn a_rule_patch_racing_a_committed_patch_derives_its_old_set_from_the_fresh_row() {
    let Some(f) = setup().await else { return };
    let token = session_for(&f, Some("platform_admin")).await;
    let (service_b, catalog_b) = profile(&f.db, &f.policy, "fresh-rule-b").await;
    let (_, catalog_c) = profile(&f.db, &f.policy, "fresh-rule-c").await;
    let rule_name = unique("fresh_rule");
    let rule_id = create(&f, &token, "rules", rule_body(&rule_name, f.catalog_id)).await;
    let uri = format!("/perm/rules/{rule_id}");

    let (point, pause) = egret_perm_compile::write_seam::PausePoint::new();
    let ours = tokio::spawn(egret_perm_compile::write_seam::run_with(point, {
        let (app, uri, token) = (f.app(), uri.clone(), token.clone());
        let body = rule_body(&rule_name, catalog_c);
        async move {
            app.oneshot(req("PATCH", &uri, &token, Some(body)))
                .await
                .unwrap()
        }
    }));
    pause
        .reached
        .await
        .expect("our PATCH reached the pause point");
    let (status, theirs) = into_json(
        f.app()
            .oneshot(req(
                "PATCH",
                &uri,
                &token,
                Some(rule_body(&rule_name, catalog_b)),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the competing patch; body: {theirs}"
    );
    pause.go.send(()).expect("resume our PATCH");
    let (status, got) = into_json(ours.await.expect("our PATCH task")).await;

    assert_eq!(status, StatusCode::OK, "body: {got}");
    let services = got["services"].as_array().expect("services");
    assert!(
        services.iter().any(|v| v.as_i64() == Some(service_b)),
        "the profile the competing patch moved the rule to is in the old set; body: {got}"
    );
    let catalog_now: Option<Uuid> =
        sqlx::query_scalar("SELECT catalog_id FROM perm.rules WHERE id = $1")
            .bind(rule_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("read catalog_id");
    assert_eq!(catalog_now, Some(catalog_c));
}

// ---------------------------------------------------------------------------
// #801: a grant or rule id outside the caller's workspace answers like a
// missing one
// ---------------------------------------------------------------------------

/// **#801, the stored-row reads every by-id route uses.** A caller confined to
/// workspace A (a ws-scoped overlay) asking for a grant or rule on workspace
/// B's catalog must get exactly what it gets for an id that names no row:
/// `NotFound`. A `Forbidden` there tells it the id exists elsewhere.
///
/// Tested directly, as `confine_to_overlay_workspace` is in `tests/routes.rs`
/// and for the same reason: no session builds a confined principal that passes
/// the `policy:*` gate in front of these routes. The controls: the confined
/// caller still reaches its own workspace's rows, and an unconfined caller
/// reaches B's.
#[tokio::test]
async fn grant_and_rule_ids_answer_404_alike_outside_the_callers_workspace() {
    let Some(f) = setup().await else { return };
    let token = session_for(&f, Some("platform_admin")).await;
    let (ws_a, own) = catalog_of_new_workspace(&f, "oracle-a").await;
    let (_, foreign) = catalog_of_new_workspace(&f, "oracle-b").await;
    let own_grant = create(&f, &token, "grants", grant_body(own, &f.analyst)).await;
    let foreign_grant = create(&f, &token, "grants", grant_body(foreign, &f.analyst)).await;
    let own_rule = create(&f, &token, "rules", rule_body(&unique("own"), own)).await;
    let foreign_rule = create(&f, &token, "rules", rule_body(&unique("foreign"), foreign)).await;
    let missing = Uuid::new_v4();
    let mut conn = f.db.pool.acquire().await.expect("acquire a connection");
    let confined = principal_confined_to(ws_a);
    let plain = principal_unconfined();
    use egret_perm_compile::routes::{grant_in_reach, rule_in_reach};

    // The oracle: foreign and missing answer the same.
    for id in [foreign_grant, missing] {
        let got = grant_in_reach(&mut conn, &confined, id).await.map(|g| g.id);
        assert!(
            matches!(got, Err(PermHttpError::NotFound)),
            "grant {id} (foreign: {}) must answer like a missing one; got {got:?}",
            id == foreign_grant
        );
    }
    for id in [foreign_rule, missing] {
        let got = rule_in_reach(&mut conn, &confined, id).await.map(|r| r.id);
        assert!(
            matches!(got, Err(PermHttpError::NotFound)),
            "rule {id} (foreign: {}) must answer like a missing one; got {got:?}",
            id == foreign_rule
        );
    }

    // Controls: its own rows, and an unconfined caller's view of B's.
    let got = grant_in_reach(&mut conn, &confined, own_grant)
        .await
        .map(|g| g.id);
    assert!(matches!(got, Ok(id) if id == own_grant), "{got:?}");
    let got = rule_in_reach(&mut conn, &confined, own_rule)
        .await
        .map(|r| r.id);
    assert!(matches!(got, Ok(id) if id == own_rule), "{got:?}");
    let got = grant_in_reach(&mut conn, &plain, foreign_grant)
        .await
        .map(|g| g.id);
    assert!(matches!(got, Ok(id) if id == foreign_grant), "{got:?}");
    let got = rule_in_reach(&mut conn, &plain, foreign_rule)
        .await
        .map(|r| r.id);
    assert!(matches!(got, Ok(id) if id == foreign_rule), "{got:?}");
}

/// #801 over HTTP, as a caller who can actually reach these routes: a user
/// with no `policy:*` permission gets the same status and body for an existing
/// grant or rule id as for an id that names nothing, on every by-id route.
#[tokio::test]
async fn a_caller_probing_existing_ids_gets_the_answer_a_random_id_gets() {
    let Some(f) = setup().await else { return };
    let admin = session_for(&f, Some("platform_admin")).await;
    let grant_id = create(&f, &admin, "grants", grant_body(f.catalog_id, &f.analyst))
        .await
        .to_string();
    let rule = rule_body(&unique("probe"), f.catalog_id);
    let rule_id = create(&f, &admin, "rules", rule.clone()).await.to_string();
    let caller = session_for(&f, None).await;
    let missing = Uuid::new_v4().to_string();

    let cases = [
        (
            "PATCH",
            format!("/perm/grants/{grant_id}"),
            Some(grant_body(f.catalog_id, &f.analyst)),
        ),
        ("DELETE", format!("/perm/grants/{grant_id}"), None),
        ("PATCH", format!("/perm/rules/{rule_id}"), Some(rule)),
        ("DELETE", format!("/perm/rules/{rule_id}"), None),
        ("GET", format!("/perm/rules/{rule_id}/effect"), None),
    ];
    for (method, uri, body) in cases {
        let random = uri.replace(&grant_id, &missing).replace(&rule_id, &missing);
        let real = into_json(
            f.app()
                .oneshot(req(method, &uri, &caller, body.clone()))
                .await
                .unwrap(),
        )
        .await;
        let fake = into_json(
            f.app()
                .oneshot(req(method, &random, &caller, body))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(real, fake, "{method} {uri} vs {random}");
    }
}
