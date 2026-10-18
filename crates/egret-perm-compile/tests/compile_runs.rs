//! `perm.compile_runs` (DG #3, spec §3.3): the last compile of each profile.
//! Success is written by the compile funnel on the compile's own transaction;
//! failure only by `recompile_service`, after its transaction rolled back, and
//! never by a write the failure refuses.
//!
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set.
//! Every assertion reads only the fixture's own profile.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, recompile_service, recompile_services_in_tx, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    app: axum::Router,
    token: String,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
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
    sqlx::query(
        "INSERT INTO users (provider, email, username) \
         VALUES ('local', 'analyst@perm-compile-runs.egret.local', 'analyst') \
         ON CONFLICT DO NOTHING",
    )
    .execute(&db.pool)
    .await
    .expect("fixture username");
    let uid = UserRepo::new(db.clone())
        .insert_local(
            &format!("runs_{}@egret.local", Uuid::new_v4().simple()),
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
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let app = perm_router().with_state(PermState {
        perm: perm.clone(),
        policy: policy.clone(),
        sessions,
        pdp,
    });
    Some(Env {
        db,
        perm,
        policy,
        app,
        token,
    })
}

/// A StarRocks profile with one engine bound and one catalog attached.
struct Profile {
    service_id: i64,
    catalog_id: Uuid,
}

async fn profile(e: &Env) -> Profile {
    let pool = &e.db.pool;
    let svc = e
        .policy
        .create_service(&unique("runs_sr"), "starrocks", &json!({}))
        .await
        .expect("profile");
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'Runs') RETURNING id")
            .bind(unique("runs-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-runs','AKIATEST') RETURNING id",
    )
    .bind(unique("runs-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("runs_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret', \
                 '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) \
         RETURNING id",
    )
    .bind(unique("runs-eng"))
    .bind(svc.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("runs_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    Profile {
        service_id: svc.id,
        catalog_id,
    }
}

async fn send(e: &Env, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={}", e.token));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = e.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn grant(p: &Profile, table: &str) -> Value {
    json!({
        "effect": "allow",
        "actions": ["read"],
        "scope": { "catalog_id": p.catalog_id, "schema": "sales", "table": table, "column": null },
        "principals": { "include": [{ "kind": "user", "ref": "analyst" }], "exclude": [] }
    })
}

fn path_mask(name: &str, p: &Profile, column: &str, mask: &str) -> Value {
    json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
        "selector": { "type": "path", "catalog_id": p.catalog_id,
                      "schema": "sales", "table": "orders", "column": column },
        "exempt": { "include": [], "exclude": [] }
    })
}

/// Two mask rules whose MODEL rows overlap on `orders.email` while the stored
/// download still holds them apart: the next recompile of the profile would
/// create the pair, so every recompile of it is refused (409). Returns the id
/// of the rule that was moved, so a test can move it back.
async fn overlapping_model(e: &Env, p: &Profile) -> Uuid {
    let (status, body) = send(
        e,
        "POST",
        "/perm/rules",
        Some(path_mask(&unique("runs a"), p, "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = send(
        e,
        "POST",
        "/perm/rules",
        Some(path_mask(&unique("runs b"), p, "phone", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let b: Uuid = body["id"].as_str().expect("id").parse().unwrap();
    sqlx::query("UPDATE perm.rules SET column_name = 'email' WHERE id = $1")
        .bind(b)
        .execute(&e.db.pool)
        .await
        .expect("move the model row only");
    b
}

#[derive(Debug, sqlx::FromRow)]
struct Run {
    outcome: String,
    attempted_at: DateTime<Utc>,
    error: Option<String>,
    last_ok_at: Option<DateTime<Utc>>,
    created: i32,
    updated: i32,
    deleted: i32,
    unchanged: i32,
    skipped: i32,
}

async fn run_of(e: &Env, service_id: i64) -> Option<Run> {
    sqlx::query_as(
        "SELECT outcome, attempted_at, error, last_ok_at, created, updated, deleted, unchanged, \
                skipped \
           FROM perm.compile_runs WHERE service_id = $1",
    )
    .bind(service_id)
    .fetch_optional(&e.db.pool)
    .await
    .expect("read compile run")
}

#[tokio::test]
async fn a_grant_write_records_an_ok_run_and_an_unchanged_recompile_refreshes_it() {
    let Some(e) = env().await else { return };
    let p = profile(&e).await;
    let (status, body) = send(&e, "POST", "/perm/grants", Some(grant(&p, "orders"))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let first = run_of(&e, p.service_id).await.expect("an ok row");
    assert_eq!(first.outcome, "ok");
    assert!(first.created >= 1, "{first:?}");
    assert_eq!(first.error, None);
    assert_eq!(first.last_ok_at, Some(first.attempted_at));

    recompile_service(&e.perm, &e.policy, p.service_id)
        .await
        .expect("recompile unchanged");
    let second = run_of(&e, p.service_id).await.expect("still a row");
    assert_eq!(second.outcome, "ok");
    assert_eq!(second.created, 0, "{second:?}");
    assert_eq!((second.updated, second.deleted), (0, 0), "{second:?}");
    assert!(second.unchanged >= 1, "{second:?}");
    assert_eq!(second.skipped, 0);
    assert!(second.attempted_at > first.attempted_at);
}

#[tokio::test]
async fn a_recompile_on_a_rolled_back_transaction_leaves_the_row_as_it_was() {
    let Some(e) = env().await else { return };
    let p = profile(&e).await;

    let mut tx = e.db.pool.begin().await.unwrap();
    recompile_services_in_tx(&mut tx, &[p.service_id])
        .await
        .expect("recompile");
    tx.rollback().await.unwrap();
    assert!(
        run_of(&e, p.service_id).await.is_none(),
        "a rolled-back compile changed nothing on the engine"
    );

    recompile_service(&e.perm, &e.policy, p.service_id)
        .await
        .expect("committed recompile");
    let before = run_of(&e, p.service_id).await.expect("a row");
    let mut tx = e.db.pool.begin().await.unwrap();
    recompile_services_in_tx(&mut tx, &[p.service_id])
        .await
        .expect("recompile");
    tx.rollback().await.unwrap();
    let after = run_of(&e, p.service_id).await.expect("a row");
    assert_eq!(after.attempted_at, before.attempted_at);
}

#[tokio::test]
async fn a_profile_the_compiler_does_not_target_gets_no_row() {
    let Some(e) = env().await else { return };
    let svc = e
        .policy
        .create_service(&unique("runs_tag"), "tag", &json!({}))
        .await
        .expect("tag service");
    recompile_service(&e.perm, &e.policy, svc.id)
        .await
        .expect("an untargeted recompile is a no-op");
    assert!(run_of(&e, svc.id).await.is_none());
}

#[tokio::test]
async fn a_failed_recompile_records_its_error_and_keeps_the_last_success() {
    let Some(e) = env().await else { return };
    let p = profile(&e).await;
    let (status, body) = send(&e, "POST", "/perm/grants", Some(grant(&p, "orders"))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let moved = overlapping_model(&e, &p).await;
    let ok = run_of(&e, p.service_id).await.expect("an ok row");
    assert_eq!(ok.outcome, "ok");

    let err = recompile_service(&e.perm, &e.policy, p.service_id)
        .await
        .expect_err("the model now holds an overlapping pair");
    let failed = run_of(&e, p.service_id).await.expect("a row");
    assert_eq!(failed.outcome, "error");
    assert_eq!(failed.error.as_deref(), Some(err.to_string().as_str()));
    assert_eq!(failed.last_ok_at, ok.last_ok_at, "the last success is kept");
    assert_eq!(
        (
            failed.created,
            failed.updated,
            failed.deleted,
            failed.unchanged
        ),
        (ok.created, ok.updated, ok.deleted, ok.unchanged),
        "the last success's counts are kept"
    );
    assert!(failed.attempted_at > ok.attempted_at);

    sqlx::query("UPDATE perm.rules SET column_name = 'phone' WHERE id = $1")
        .bind(moved)
        .execute(&e.db.pool)
        .await
        .unwrap();
    recompile_service(&e.perm, &e.policy, p.service_id)
        .await
        .expect("the overlap is gone");
    let healed = run_of(&e, p.service_id).await.expect("a row");
    assert_eq!(healed.outcome, "ok");
    assert_eq!(healed.error, None);
    assert!(healed.last_ok_at > ok.last_ok_at);
}

#[tokio::test]
async fn a_write_refused_by_its_recompile_records_no_failure() {
    let Some(e) = env().await else { return };
    let p = profile(&e).await;
    overlapping_model(&e, &p).await;
    let before = run_of(&e, p.service_id).await.expect("the rules' ok row");

    let (status, body) = send(&e, "POST", "/perm/grants", Some(grant(&p, "items"))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let after = run_of(&e, p.service_id).await.expect("a row");
    assert_eq!(
        after.outcome, "ok",
        "the refused write's caller saw the error"
    );
    assert_eq!(after.attempted_at, before.attempted_at);
}

#[tokio::test]
async fn a_failure_older_than_the_stored_run_changes_nothing() {
    let Some(e) = env().await else { return };
    let p = profile(&e).await;
    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, last_ok_at, created) \
         VALUES ($1, 'ok', now() + interval '1 hour', now() + interval '1 hour', 3)",
    )
    .bind(p.service_id)
    .execute(&e.db.pool)
    .await
    .unwrap();
    egret_perm_compile::compile_runs::record_compile_failure(&e.db.pool, p.service_id, "boom")
        .await
        .expect("record");
    let run = run_of(&e, p.service_id).await.expect("a row");
    assert_eq!(run.outcome, "ok");
    assert_eq!(run.error, None);
    assert_eq!(run.created, 3);
}

#[tokio::test]
async fn a_failure_text_is_capped_and_its_profile_listed() {
    let Some(e) = env().await else { return };
    let p = profile(&e).await;
    let long = "é".repeat(3000);
    egret_perm_compile::compile_runs::record_compile_failure(&e.db.pool, p.service_id, &long)
        .await
        .expect("record");
    let run = run_of(&e, p.service_id).await.expect("a row");
    assert_eq!(run.outcome, "error");
    assert_eq!(run.error.as_deref().map(|s| s.chars().count()), Some(2000));
    assert_eq!(run.last_ok_at, None);

    let mut conn = e.db.pool.acquire().await.unwrap();
    let failed = egret_perm_compile::compile_runs::failed_compile_runs_via(&mut conn)
        .await
        .expect("list");
    let mine = failed
        .iter()
        .find(|r| r.service_id == p.service_id)
        .expect("listed");
    assert_eq!(mine.outcome, "error");
    assert!(mine.service_name.starts_with("runs_sr_"));
    assert_eq!(mine.error.as_deref().map(|s| s.chars().count()), Some(2000));

    // A profile that does not exist is not recorded (and does not error).
    egret_perm_compile::compile_runs::record_compile_failure(&e.db.pool, i64::MAX, "gone")
        .await
        .expect("no row, no error");
}
