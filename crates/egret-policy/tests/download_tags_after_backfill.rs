//! DB-gated: the boot backfill must make its new attachments reachable by a
//! Ranger tag plugin that is ALREADY RUNNING (SP3 catalog-registry review F-1).
//! Self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and
//! otherwise FAILS.
//!
//! Its own test binary — and therefore its own database — because
//! `run_backfill` is global: it migrates every unmigrated legacy row in the
//! database, so it cannot share a process with tests that seed legacy rows of
//! their own.
//!
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test download_tags_after_backfill

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_policy::{ranger_router, seed_tag, seed_trino, PolicyStore, RangerState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "devtoken";

async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn app(store: PolicyStore) -> axum::Router {
    ranger_router(RangerState {
        store,
        auth_token: Arc::from(TOKEN),
    })
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

/// A workspace + storage connection + two warehouses in it. Returns
/// `(warehouse_id, warehouse_name)` for each.
async fn seed_two_warehouses(pool: &sqlx::PgPool) -> ((Uuid, String), (Uuid, String)) {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("bfws-{}", Uuid::new_v4().simple()))
            .bind("Backfill Tag WS")
            .fetch_one(pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, 'us-east-1', 'egret-bf-test', 'AKIATEST') RETURNING id",
    )
    .bind(format!("bfsc-{}", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .expect("insert storage connection");
    let mut out = Vec::new();
    for _ in 0..2 {
        let name = format!("bfwh_{}", Uuid::new_v4().simple());
        let (wh_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
             VALUES ($1,$2,$3) RETURNING id",
        )
        .bind(&name)
        .bind(ws_id)
        .bind(sc_id)
        .fetch_one(pool)
        .await
        .expect("insert warehouse");
        out.push((wh_id, name));
    }
    (out[0].clone(), out[1].clone())
}

/// One legacy `engine_mgmt.engine_catalogs` row, the pre-SP3 shape the backfill
/// migrates: an `egret-warehouse` catalog carrying its Catalog Client
/// credential in plaintext.
async fn seed_legacy_row(
    pool: &sqlx::PgPool,
    engine_id: Uuid,
    name: &str,
    warehouse_name: &str,
    is_primary: bool,
) {
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, $2, 'egret-warehouse', \
                 jsonb_build_object('warehouse', $3::text, 'credential', $4::text), $5)",
    )
    .bind(engine_id)
    .bind(name)
    .bind(warehouse_name)
    .bind(format!("client-{}:secret", Uuid::new_v4().simple()))
    .bind(is_primary)
    .execute(pool)
    .await
    .expect("insert legacy engine catalog");
}

/// F-1: a pre-existing engine that reaches a warehouse through a SECONDARY
/// `egret-warehouse` row — exactly the population the export's registry∪legacy
/// union exists for, since the legacy branch filters `is_primary`. After the
/// backfill migrates it, a plugin holding the pre-migration `lastKnownVersion`
/// must get 200 with the new catalog's tags, not the 304 it would get forever
/// if nothing moved `tag_version`.
#[tokio::test]
async fn backfill_bumps_tag_version_so_a_running_plugin_sees_the_new_catalog() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db.clone());
    seed_trino(&store).await.expect("seed trino def");
    seed_tag(&store).await.expect("seed tag def");

    // The profile the engine binds to, and the tag service the plugin polls.
    let base = Uuid::new_v4().simple().to_string();
    let trino = format!("trino_{base}");
    let tag_svc = format!("{trino}_tag");
    let trino_row = store
        .create_service(&trino, "trino", &json!({ "tagService": tag_svc }))
        .await
        .expect("create trino service");
    let tag_row = store
        .create_service(&tag_svc, "tag", &json!({}))
        .await
        .expect("create tag service");

    // A pre-SP3 engine: primary warehouse W1, plus a SECOND `egret-warehouse`
    // row for W2. No registry catalogs, no attachments — the state migration
    // 0147 leaves behind before the first backfill runs.
    let ((wh1, wh1_name), (wh2, wh2_name)) = seed_two_warehouses(&pool).await;
    let (engine_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec, authz_service_id) \
         VALUES ($1,'trino','egret',$2,'{}'::jsonb,$3) RETURNING id",
    )
    .bind(format!("bfeng-{}", Uuid::new_v4().simple()))
    .bind(wh1)
    .bind(trino_row.id)
    .fetch_one(&pool)
    .await
    .expect("insert engine");
    seed_legacy_row(&pool, engine_id, "egret", &wh1_name, true).await;
    seed_legacy_row(&pool, engine_id, "secondary", &wh2_name, false).await;

    // W2 carries the tag the plugin needs.
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("pii-{base}"))
            .bind("PII")
            .fetch_one(&pool)
            .await
            .expect("insert tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(wh2)
    .execute(&pool)
    .await
    .expect("insert assignment");

    // The running plugin's steady state before the migration: it holds this
    // version and is told nothing changed.
    let last_known = tag_row.tag_version;
    let r = app(store.clone())
        .oneshot(get(&format!(
            "/tags/download/{tag_svc}?lastKnownVersion={last_known}"
        )))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_MODIFIED,
        "before the backfill the plugin is up to date"
    );

    // Boot.
    let registry = egret_catalog_registry::RegistryStore::new(db.clone());
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let summary = egret_catalog_registry::run_backfill(&registry, &secrets)
        .await
        .expect("backfill");
    assert_eq!(summary.failed_rows, 0, "every legacy row must migrate");

    // W2 is now a registry catalog the engine attaches.
    let catalog_name: String =
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE warehouse_id = $1")
            .bind(wh2)
            .fetch_one(&pool)
            .await
            .expect("W2's registry catalog");

    let bumped: i64 =
        sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(tag_row.id)
            .fetch_one(&pool)
            .await
            .expect("tag service");
    assert!(
        bumped > last_known,
        "the polled tag service's tag_version must move ({bumped} vs {last_known})"
    );

    // Same plugin, same held version: now it is told to re-download, and the
    // payload carries W2's tags under the registry catalog name.
    let r = app(store)
        .oneshot(get(&format!(
            "/tags/download/{tag_svc}?lastKnownVersion={last_known}"
        )))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "the plugin must re-download");
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(srs.len(), 1, "one tagged resource, got {srs:?}");
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(catalog_name),
        "exported under the migrated catalog's name"
    );
    assert_eq!(srs[0]["resourceElements"]["column"]["values"][0], "email");
}
