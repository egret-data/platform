//! **The def cross-check `explain` arms, and why it needs its own binary**
//! (SP4d-2a Task 5 review, I2).
//!
//! `explain` evaluates every request under
//! `EngineConfig { engine: Some(…), ..EngineConfig::without_super_users() }`.
//! The `engine` field is not decoration: `egret_ranger_eval::eval`'s preamble
//! only pushes `UndecidableCause::EngineMismatch` `if let Some(declared) =
//! engine.engine`, so leaving it `None` disarms the check entirely — and the
//! failure it catches is SILENT. Trino's middle resource level is `schema` and
//! StarRocks' is `database`, so a request built for one engine against the
//! other's service-def matches no hierarchy at all and comes back "nothing
//! matched" with full confidence, which is indistinguishable from "no policy
//! applies". The review mutated the handler to `engine: None` and the whole
//! `routes` binary stayed green, which is how an unpinned decision gets deleted
//! as redundant.
//!
//! **Why not in `tests/routes.rs`.** Provoking the mismatch means making a
//! service-def row's own `def_jsonb.name` disagree with the
//! `policy.authz_services.type` that selects it, and `authz_service_defs` has
//! ONE row per def name for the whole database. `egret-test-db` gives a private
//! database per test PROCESS, not per test, so inside the `routes` binary that
//! mutation would be visible to every other `explain` test running on one of
//! the other three threads. Its own binary makes the corruption safe by
//! construction: nothing else runs in this process.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test explain_engine_mismatch

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    db: Db,
    app: axum::Router,
    token: String,
    catalog_id: Uuid,
}

/// A StarRocks profile with a bound engine and one attached registry catalog,
/// plus a `platform_admin` session. No tags and no grants: this file is about
/// the cross-check, and a profile with no policies at all still has to reach
/// the evaluator to be told the def is the wrong one.
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
        .create_service(&unique("sr_mismatch"), "starrocks", &json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("mismatch-ws"))
            .bind("Engine Mismatch WS")
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-mismatch-test','AKIATEST') RETURNING id",
    )
    .bind(unique("mismatch-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("mismatch_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("mismatch-eng"))
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
    .bind(unique("mismatch_cat"))
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

    let users = UserRepo::new(db.clone());
    let uid = users
        .insert_local(
            &format!("mismatch_{}@egret.local", Uuid::new_v4().simple()),
            "Test",
            "x",
        )
        .await
        .expect("insert user")
        .0;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let app = perm_router().with_state(PermState {
        perm,
        policy,
        sessions,
        pdp,
    });

    Some(Fixture {
        db,
        app,
        token,
        catalog_id,
    })
}

async fn explain(f: &Fixture) -> (StatusCode, Value) {
    let body = json!({
        "subject": { "user": "nobody" },
        "action": "read",
        "target": { "catalog_id": f.catalog_id, "schema": "sales",
                    "table": "orders", "column": null }
    });
    let request = HttpRequest::builder()
        .method("POST")
        .uri("/perm/explain")
        .header("Cookie", format!("egret_session={}", f.token))
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = f.app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

/// The `select` verdict of the one profile in the response.
fn only_verdict(body: &Value) -> &Value {
    let profiles = body["profiles"]
        .as_array()
        .unwrap_or_else(|| panic!("no `profiles` array in {body}"));
    assert_eq!(profiles.len(), 1, "one profile serves this catalog: {body}");
    profiles[0]["accesses"]
        .as_array()
        .unwrap_or_else(|| panic!("no `accesses` array in {body}"))
        .iter()
        .find(|a| a["access_type"] == "select")
        .unwrap_or_else(|| panic!("no select verdict in {body}"))
}

#[tokio::test]
async fn a_service_def_that_is_not_this_engines_makes_the_decision_undecidable() {
    let Some(f) = setup().await else { return };

    // Control. With the real def, the profile DECIDES — it has no policies, so
    // the answer is Ranger's own "nothing matched". Without this the assertion
    // below would also pass against an `explain` that refused everything.
    let (status, body) = explain(&f).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        only_verdict(&body)["decision"]["kind"],
        "denied",
        "body: {body}"
    );
    assert_eq!(
        only_verdict(&body)["decision"]["reason"],
        "no_policy_matched",
        "body: {body}"
    );

    // Now the def this StarRocks profile is served says it is Trino's. This is
    // the shape that is otherwise SILENT: the resource levels no longer line up
    // (`database` vs `schema`), every policy matches nothing, and the honest
    // answer and the wrong one are the same string.
    let res = sqlx::query(
        "UPDATE policy.authz_service_defs \
            SET def_jsonb = jsonb_set(def_jsonb, '{name}', '\"trino\"') \
          WHERE name = 'starrocks'",
    )
    .execute(&f.db.pool)
    .await
    .expect("corrupt the def name");
    assert_eq!(res.rows_affected(), 1, "the def row must have been updated");
    let seen: String = sqlx::query_scalar(
        "SELECT def_jsonb->>'name' FROM policy.authz_service_defs WHERE name = 'starrocks'",
    )
    .fetch_one(&f.db.pool)
    .await
    .expect("read back the def name");
    assert_eq!(
        seen, "trino",
        "the substitution must have landed before anything is concluded from it"
    );

    let (status, body) = explain(&f).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let decision = &only_verdict(&body)["decision"];
    assert_eq!(
        decision["kind"], "undecidable",
        "a def that is not this engine's must REFUSE, not answer `nothing matched` \
         with full confidence — that is exactly what `engine: None` would do, and it \
         reads identically to a correct denial; body: {body}"
    );
    let cause = decision["why"]
        .as_array()
        .unwrap_or_else(|| panic!("no `why` array in {body}"))
        .iter()
        .find(|c| c["kind"] == "engine_mismatch")
        .unwrap_or_else(|| panic!("no engine_mismatch cause in {body}"));
    assert_eq!(
        cause["declared"], "starrocks",
        "the cause must name the engine egret declared; body: {body}"
    );
    assert_eq!(
        cause["service_def_name"], "trino",
        "and the def it was actually handed; body: {body}"
    );
}
