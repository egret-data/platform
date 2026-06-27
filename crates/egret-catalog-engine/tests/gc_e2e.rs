//! DB + MinIO gated END-TO-END proof of CE-6: a table dropped WITH purge is
//! actually reclaimed by lakekeeper's built-in GC task-queue workers running
//! inside the embedded engine.
//!
//! This is the CE-6 headline test. The earlier `gc_workers` test only proves
//! `build_state` *registers* the 3 built-in queues; this one proves the workers
//! we spawn actually *run* and complete a real purge:
//!
//!   1. build the engine state + bootstrap + provision a per-workspace warehouse
//!      (live MinIO), exactly like `front_auth.rs`,
//!   2. create a real namespace + Iceberg table through the mounted router (its
//!      metadata JSON lands in MinIO under the warehouse's key prefix),
//!   3. `DELETE .../tables/{t}?purgeRequested=true` → 204, and loading the table
//!      afterwards is 404 (the warehouse's default delete profile is HARD, so
//!      the drop schedules a `tabular_purge` task immediately — no soft-delete
//!      expiration delay),
//!   4. spawn the GC workers against the SAME engine_state/registry the drop ran
//!      through (so they poll the same DB/queue where the purge was enqueued),
//!      registered with a SHORT 1s poll so the test doesn't wait the prod 10s
//!      per tick (`build_state_with_gc_poll`, test-utils-gated — `build_state`'s
//!      public signature is unchanged),
//!   5. poll for the purge reaching its terminal SUCCESS state.
//!
//! ## GC-completion signal asserted (the less-flaky one)
//!
//! On success lakekeeper MOVES a task out of `lakekeeper.task` into
//! `lakekeeper.task_log` with `status = 'success'` (a `task_final_status` enum;
//! the other terminal values are `failed` / `cancelled`). We poll for a
//! `lakekeeper.task_log` row with `queue_name = 'tabular_purge'` and
//! `status::text = 'success'`. This is a single authoritative DB read of a
//! terminal state — strictly more robust than listing MinIO objects (which is
//! eventually-consistent and needs an S3 client), and it fails LOUDLY (never
//! silently passes) if the purge errors: a `failed` row would never satisfy the
//! `= 'success'` predicate, so the test would time out rather than green.
//!
//! Needs MinIO reachable at `localhost:9000` with a writable `warehouse` bucket
//! (the egret dev stack ships it). Self-skips when `DATABASE_URL` is unset.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test gc_e2e

use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    build_state_with_gc_poll, ensure_bootstrapped, mount_catalog_engine,
    provision_workspace_warehouse, run_lakekeeper_migrations, spawn_gc_workers, CancellationToken,
};
use egret_metastore::Db;
use egret_storage::StorageConfig;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

/// Dev MinIO config matching the egret dev stack (bucket `warehouse`,
/// egret/egretsecret, S3-compat at localhost:9000).
fn dev_storage_config() -> StorageConfig {
    StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    }
}

/// Collect a response into `(status, json)`, surfacing the raw body on failure.
async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// POST a create-table body, tolerating ONE transient MinIO write flake on the
/// create (retry once). Returns `(status, json)` of the (final) attempt. Only
/// the create path retries; the GC assertion below is never weakened.
async fn create_table_once(
    app: &axum::Router,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    into_json(resp).await
}

#[tokio::test]
async fn dropped_with_purge_is_reclaimed_by_gc_workers() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping gc_e2e test");
        return;
    };

    // --- 1. Engine + egret schema setup (mirrors front_auth.rs) --------------
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");

    // Build the engine state AND register the built-in GC queues with a SHORT 1s
    // poll (not the prod 10s) into the returned registry. The workers we spawn in
    // step 4 use THIS registry, and it shares the same isolated `lakekeeper`-schema
    // pools as the state we drop the table through — so the worker polls exactly
    // the DB/queue where the purge gets enqueued.
    let (engine_state, registry) =
        build_state_with_gc_poll(&dsn, SYSTEM_USER, Duration::from_secs(1))
            .await
            .expect("build engine state with 1s GC poll");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();

    // Seed a workspace + its real lakekeeper warehouse (live MinIO validate_access).
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("gce2e-{suffix}@egret.test"), "GC E2E Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("gce2e{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "GC E2E Test", owner)
        .await
        .expect("seed workspace row");
    let workspace_id: Uuid = ws.id.0;

    let warehouse = provision_workspace_warehouse(
        engine_state.clone(),
        &store,
        workspace_id,
        &slug,
        &storage_cfg,
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");
    let prefix = warehouse.to_string();

    // Full-scope rw token for this workspace, used for all catalog ops below.
    let (_row, token) = store
        .create_token(
            workspace_id,
            "gce2e-token",
            &["catalog:read".to_string(), "catalog:write".to_string()],
            &[],
        )
        .await
        .expect("mint full-scope rw token");

    let app = mount_catalog_engine(engine_state.clone(), store.clone());

    // --- 2. Create a real namespace + Iceberg table --------------------------
    let ns_name = format!("gc_{}", Uuid::now_v7().simple());
    // Unique per run so the create-retry below always creates a DISTINCT table:
    // if lakekeeper does NOT roll back its DB row on a failed S3 write, a
    // same-name 2nd attempt would 409 — the unique name sidesteps that entirely.
    let table_name = format!("doomed_{}", Uuid::now_v7().simple());

    let ns_body = serde_json::json!({ "namespace": [ns_name], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix}/namespaces"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&ns_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, ns) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create namespace; body={ns}");

    let create_table = serde_json::json!({
        "name": table_name,
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [ { "id": 1, "name": "id", "required": true, "type": "long" } ]
        }
    });
    let table_path = format!("/{prefix}/namespaces/{ns_name}/tables");
    // Tolerate ONE transient MinIO write flake on the create (retry once).
    let (status, created) = {
        let (s, body) = create_table_once(&app, &token, &table_path, &create_table).await;
        if s == StatusCode::OK {
            (s, body)
        } else {
            eprintln!("create table first attempt non-OK ({s}); retrying once. body={body}");
            create_table_once(&app, &token, &table_path, &create_table).await
        }
    };
    assert_eq!(status, StatusCode::OK, "create table; body={created}");
    let metadata_location = created["metadata-location"]
        .as_str()
        .unwrap_or_else(|| panic!("created table has a metadata-location; body={created}"))
        .to_string();
    assert!(
        metadata_location.contains(&slug),
        "table metadata lives under the workspace prefix; loc={metadata_location}"
    );

    // --- 3. Drop WITH purge → 204; loading the table afterwards is 404 -------
    let drop_uri =
        format!("/{prefix}/namespaces/{ns_name}/tables/{table_name}?purgeRequested=true");
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&drop_uri)
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "drop-with-purge returns 204 NO_CONTENT"
    );

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{prefix}/namespaces/{ns_name}/tables/{table_name}"
                ))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "the dropped table no longer loads (404)"
    );

    // --- 4. Spawn the GC workers against THIS registry/engine_state ----------
    // CancellationToken does NOT cancel on drop, so a panic between spawn and the
    // final `cancel.cancel()` would leak the workers for the rest of the test
    // binary (spurious DB traffic in a shared CI runtime). A drop guard cancels
    // them on ANY scope exit, panic included. Held for the whole test.
    let cancel = CancellationToken::new();
    let _gc_guard = cancel.clone().drop_guard();
    spawn_gc_workers(registry, cancel.clone());

    // --- 5. Poll for the `tabular_purge` task reaching terminal SUCCESS ------
    // A separate, search_path-pinned pool to read lakekeeper's task_log directly.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(|conn, _| {
            Box::pin(async move {
                use sqlx::Executor;
                conn.execute("SET search_path = lakekeeper, public").await?;
                Ok(())
            })
        })
        .connect(&dsn)
        .await
        .expect("connect for GC task-log assertion");

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut succeeded = false;
    while std::time::Instant::now() < deadline {
        // On success the purge task is moved into task_log with status 'success'
        // (task_final_status enum). We restrict to OUR warehouse so a stray task
        // from another run can't satisfy the assertion.
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT count(*) FROM task_log
               WHERE queue_name = 'tabular_purge'
                 AND warehouse_id = $1
                 AND status::text = 'success'"#,
        )
        .bind(warehouse)
        .fetch_one(&pool)
        .await
        .expect("query lakekeeper.task_log for the purge outcome");
        if count > 0 {
            succeeded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    if !succeeded {
        // Surface what the queue actually looks like so a failure is diagnosable
        // rather than a bare timeout: any terminal purge rows, and any still-live
        // task rows for our warehouse.
        let logged: Vec<(String, String)> = sqlx::query_as(
            r#"SELECT queue_name, status::text FROM task_log
               WHERE warehouse_id = $1"#,
        )
        .bind(warehouse)
        .fetch_all(&pool)
        .await
        .unwrap_or_default();
        let live: Vec<(String, String)> = sqlx::query_as(
            r#"SELECT queue_name, status::text FROM task
               WHERE warehouse_id = $1"#,
        )
        .bind(warehouse)
        .fetch_all(&pool)
        .await
        .unwrap_or_default();
        cancel.cancel();
        pool.close().await;
        panic!(
            "tabular_purge did not reach terminal 'success' within 20s. \
             task_log rows for warehouse={warehouse:?}: {logged:?}; \
             live task rows: {live:?}"
        );
    }

    cancel.cancel();
    pool.close().await;
}
