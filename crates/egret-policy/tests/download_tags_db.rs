//! DB-gated Ranger `download_tags` tests. Self-skip without DATABASE_URL.
//!
//! Exercises the real tag-download path: 404 unknown service, 304 on matching
//! `tagVersion`, and 200 serving a seeded `catalog_tag_assignments` row for the
//! service's warehouse (resolved via the engine binding — see
//! `PolicyStore::tag_assignments_for_service`).
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

/// Seed a workspace + storage connection + warehouse + a Trino engine bound to
/// `service_id`. Returns (warehouse_id, warehouse_name).
async fn seed_bound_warehouse(pool: &sqlx::PgPool, service_id: i64) -> (Uuid, String) {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("tagws-{}", Uuid::new_v4().simple()))
            .bind("Tag Test WS")
            .fetch_one(pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("tagsc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-tag-test")
    .bind("AKIATEST")
    .fetch_one(pool)
    .await
    .expect("insert storage connection");
    let wh_name = format!("tagwh_{}", Uuid::new_v4().simple());
    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(&wh_name)
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    let (eng_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec, authz_service_id) \
         VALUES ($1,'trino','egret',$2,'{}'::jsonb,$3) RETURNING id",
    )
    .bind(format!("tageng-{}", Uuid::new_v4().simple()))
    .bind(wh_id)
    .bind(service_id)
    .fetch_one(pool)
    .await
    .expect("insert engine");
    // The engine's primary Trino catalog is named `egret` (a fixed name at
    // provision time), NOT the warehouse name — `tag_assignments_for_service`
    // emits THIS as the `catalog` so downloaded tags match the query's resource.
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1,'egret','egret-warehouse','{}'::jsonb,true)",
    )
    .bind(eng_id)
    .execute(pool)
    .await
    .expect("insert engine catalog");
    (wh_id, wh_name)
}

#[tokio::test]
async fn download_tags_unknown_service_404() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let r = app(store)
        .oneshot(get("/tags/download/nope_xyz_unknown"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_tags_matching_tag_version_304() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", Uuid::new_v4().simple());
    let row = store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");
    // Freshly-created services default to tag_version = 1.
    assert_eq!(row.tag_version, 1);
    let r = app(store)
        .oneshot(get(&format!(
            "/tags/download/{svc}?lastKnownVersion={}",
            row.tag_version
        )))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn download_tags_serves_seeded_assignment() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", Uuid::new_v4().simple());
    let svc_row = store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");

    let (wh_id, _wh_name) = seed_bound_warehouse(&pool, svc_row.id).await;

    // One classification tag, assigned to a column of a table in this warehouse.
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("pii-{}", Uuid::new_v4().simple()))
            .bind("PII")
            .fetch_one(&pool)
            .await
            .expect("insert tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(wh_id)
    .execute(&pool)
    .await
    .expect("insert assignment");

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(st["op"], "add");
    assert_eq!(st["serviceName"], svc.as_str());
    assert_eq!(st["tagVersion"], 1);
    // One tag def named PII.
    let tags = st["tags"].as_object().unwrap();
    assert_eq!(tags.len(), 1, "one tag, got {tags:?}");
    assert_eq!(st["tags"]["1"]["type"], "PII");
    // One resource. `resourceElements` sits directly on the entry (Ranger
    // `RangerServiceResource` shape), and `catalog` resolves to the engine's
    // Trino catalog name (`egret`), NOT the warehouse name.
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(srs.len(), 1, "one resource, got {srs:?}");
    assert_eq!(srs[0]["isEnabled"], true);
    let els = &srs[0]["resourceElements"];
    assert_eq!(els["catalog"]["values"][0], "egret");
    assert_eq!(els["schema"]["values"][0], "sales");
    assert_eq!(els["table"]["values"][0], "orders");
    assert_eq!(els["column"]["values"][0], "email");
    // Resource → tag linkage.
    assert_eq!(st["resourceToTagIds"]["1"], json!([1]));
}

/// The Trino tag plugin polls the TAG service (`<profile>_tag`), but engines bind
/// to the TRINO profile. `download_tags` must resolve the owning trino profile
/// (via its `tagService` config) and serve ITS engine's assignments — otherwise
/// tag masking silently sees zero resources.
#[tokio::test]
async fn download_tags_via_tag_service_resolves_owning_trino_profile() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino def");
    seed_tag(&store).await.expect("seed tag def");
    let base = Uuid::new_v4().simple().to_string();
    let trino = format!("trino_{base}");
    let tag_svc = format!("{trino}_tag");
    // Trino profile carries `tagService` pointing at its tag service; engine binds
    // to the TRINO profile (not the tag service).
    let trino_row = store
        .create_service(&trino, "trino", &json!({ "tagService": tag_svc }))
        .await
        .expect("create trino service");
    store
        .create_service(&tag_svc, "tag", &json!({}))
        .await
        .expect("create tag service");
    let (wh_id, _wh) = seed_bound_warehouse(&pool, trino_row.id).await;
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
    .bind(wh_id)
    .execute(&pool)
    .await
    .expect("insert assignment");

    // Poll the TAG service, as the Trino plugin does.
    let r = app(store)
        .oneshot(get(&format!("/tags/download/{tag_svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(
        srs.len(),
        1,
        "polling the tag service must resolve the owning trino profile's assignments, got {srs:?}"
    );
    assert_eq!(srs[0]["resourceElements"]["catalog"]["values"][0], "egret");
    assert_eq!(srs[0]["resourceElements"]["column"]["values"][0], "email");
}
