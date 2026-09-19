//! DB-gated Ranger `download_tags` tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
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

/// A bare workspace + storage connection + warehouse, no engine attached.
/// Returns the warehouse id.
async fn seed_warehouse(pool: &sqlx::PgPool) -> Uuid {
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
    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("tagwh_{}", Uuid::new_v4().simple()))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    wh_id
}

/// A Trino engine bound to `service_id`, whose PRIMARY warehouse is
/// `warehouse_id` (`engines.warehouse_id` — the legacy join's key).
async fn seed_engine(pool: &sqlx::PgPool, service_id: i64, warehouse_id: Uuid) -> Uuid {
    let (eng_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec, authz_service_id) \
         VALUES ($1,'trino','egret',$2,'{}'::jsonb,$3) RETURNING id",
    )
    .bind(format!("tageng-{}", Uuid::new_v4().simple()))
    .bind(warehouse_id)
    .bind(service_id)
    .fetch_one(pool)
    .await
    .expect("insert engine");
    eng_id
}

/// A registry catalog for `warehouse_id`, named `{prefix}_<uuid>`. Returns
/// `(catalog_id, catalog_name)`.
async fn seed_registry_catalog(
    pool: &sqlx::PgPool,
    warehouse_id: Uuid,
    prefix: &str,
) -> (Uuid, String) {
    let catalog_name = format!("{prefix}_{}", Uuid::new_v4().simple());
    let (catalog_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&catalog_name)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("insert registry catalog");
    (catalog_id, catalog_name)
}

/// Attach `catalog_id` to `engine_id` (`registry.engine_catalog_attachments`
/// — the registry branch's key).
async fn attach_catalog(pool: &sqlx::PgPool, engine_id: Uuid, catalog_id: Uuid) {
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
}

/// The engine's legacy `engine_mgmt.engine_catalogs` PRIMARY row, named
/// `name` — the pre-SP3 shape the backfill has not (or not yet) migrated away
/// from, and the legacy branch's key.
async fn seed_legacy_primary(pool: &sqlx::PgPool, engine_id: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, $2, 'egret-warehouse', '{}'::jsonb, true)",
    )
    .bind(engine_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert legacy primary catalog");
}

/// One PII tag, assigned to `sales.orders.email` on `warehouse_id`.
async fn seed_tag_assignment(pool: &sqlx::PgPool, warehouse_id: Uuid) {
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("pii-{}", Uuid::new_v4().simple()))
            .bind("PII")
            .fetch_one(pool)
            .await
            .expect("insert tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .execute(pool)
    .await
    .expect("insert assignment");
}

/// Seed a workspace + storage connection + warehouse + a Trino engine bound to
/// `service_id`, with a registry catalog attached (the modern, fully-migrated
/// shape). Returns (warehouse_id, the registry catalog name the engine
/// reaches it by).
async fn seed_bound_warehouse(pool: &sqlx::PgPool, service_id: i64) -> (Uuid, String) {
    let wh_id = seed_warehouse(pool).await;
    let eng_id = seed_engine(pool, service_id, wh_id).await;
    // The engine reaches the warehouse through its registry catalog; the tag
    // export must emit THAT catalog's name (the SQL name Trino sees).
    let (catalog_id, catalog_name) = seed_registry_catalog(pool, wh_id, "tagcat").await;
    attach_catalog(pool, eng_id, catalog_id).await;
    (wh_id, catalog_name)
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

    let (wh_id, catalog_name) = seed_bound_warehouse(&pool, svc_row.id).await;

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
    // registry catalog name, NOT the warehouse name.
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(srs.len(), 1, "one resource, got {srs:?}");
    assert_eq!(srs[0]["isEnabled"], true);
    let els = &srs[0]["resourceElements"];
    assert_eq!(els["catalog"]["values"][0], json!(catalog_name));
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
    let (wh_id, catalog_name) = seed_bound_warehouse(&pool, trino_row.id).await;
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
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(catalog_name)
    );
    assert_eq!(srs[0]["resourceElements"]["column"]["values"][0], "email");
}

/// C1: an engine the backfill has not migrated (no attachment at all, only its
/// legacy `engine_catalogs` primary row) must still export tags — under the
/// LEGACY name, since that's the only name it actually serves.
#[tokio::test]
async fn download_tags_legacy_only_engine_exports_legacy_name() {
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

    let wh_id = seed_warehouse(&pool).await;
    let eng_id = seed_engine(&pool, svc_row.id, wh_id).await;
    let legacy_name = format!("legacy_{}", Uuid::new_v4().simple());
    seed_legacy_primary(&pool, eng_id, &legacy_name).await;
    // No registry catalog, no attachment: this engine is exactly what the
    // backfill leaves behind on a `backfill_failed` row or a run that errored
    // out entirely (SP3 catalog-registry review, C1).
    seed_tag_assignment(&pool, wh_id).await;

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(
        srs.len(),
        1,
        "un-migrated engine must still export under its legacy name, got {srs:?}"
    );
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(legacy_name)
    );
}

/// C1: an engine mid rename-window — its registry catalog was renamed by the
/// backfill (or an edit) but the engine has not been re-applied yet — must
/// export tags under BOTH names: the OLD one it is still actually serving,
/// and the NEW one it will serve after its next apply. Dropping either one is
/// the fail-open this union closes; emitting an extra name that matches
/// nothing live costs nothing.
#[tokio::test]
async fn download_tags_rename_window_exports_both_names() {
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

    let wh_id = seed_warehouse(&pool).await;
    let eng_id = seed_engine(&pool, svc_row.id, wh_id).await;
    let old_name = format!("old_{}", Uuid::new_v4().simple());
    seed_legacy_primary(&pool, eng_id, &old_name).await;
    let (catalog_id, new_name) = seed_registry_catalog(&pool, wh_id, "new").await;
    attach_catalog(&pool, eng_id, catalog_id).await;
    assert_ne!(old_name, new_name);
    seed_tag_assignment(&pool, wh_id).await;

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(
        srs.len(),
        2,
        "rename window must export under BOTH the old and the new name, got {srs:?}"
    );
    let catalogs: std::collections::BTreeSet<String> = srs
        .iter()
        .map(|r| {
            r["resourceElements"]["catalog"]["values"][0]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        catalogs,
        std::collections::BTreeSet::from([old_name, new_name])
    );
    // Both resources link to the SAME tag (one tag row, one warehouse_id).
    let tags = st["tags"].as_object().unwrap();
    assert_eq!(tags.len(), 1, "one underlying tag, got {tags:?}");
}

/// I3: a SECONDARY attachment — a warehouse that is NOT the attaching engine's
/// own `engines.warehouse_id` primary — is exported under its registry
/// catalog name; an engine bound to the SAME service that does NOT attach
/// that warehouse's catalog sees nothing for it, even though another engine
/// (bound to the same service) does. This pins the attachment join: a mutant
/// that drops it and falls back to `e.warehouse_id = a.warehouse_id` would
/// pass neither half of this test (it would show W's tags to the
/// non-attaching engine's service via a `warehouse_id` match it doesn't have,
/// or fail to show them to the attaching one).
#[tokio::test]
async fn download_tags_secondary_attachment_scoped_to_attaching_engine() {
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

    // Two engines bound to the SAME service, each with its own (untagged,
    // unattached-to-W) primary warehouse.
    let wh_a = seed_warehouse(&pool).await;
    let eng_a = seed_engine(&pool, svc_row.id, wh_a).await;
    let wh_b = seed_warehouse(&pool).await;
    let eng_b = seed_engine(&pool, svc_row.id, wh_b).await;

    // A THIRD warehouse W, tagged, whose catalog only engine B attaches — a
    // secondary attachment (W is not B's primary warehouse either).
    let wh_w = seed_warehouse(&pool).await;
    let (catalog_w, catalog_name_w) = seed_registry_catalog(&pool, wh_w, "secondary").await;
    attach_catalog(&pool, eng_b, catalog_w).await;
    seed_tag_assignment(&pool, wh_w).await;

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(
        srs.len(),
        1,
        "only the secondary attachment's tag must be exported, got {srs:?}"
    );
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(catalog_name_w)
    );
    let _ = (eng_a, wh_b);
}

/// I3: an engine bound to a DIFFERENT service exports nothing for a warehouse
/// it attaches — confirming the service scope (`e.authz_service_id =
/// COALESCE(tp.id, s.id)`) still holds on both branches of the union.
#[tokio::test]
async fn download_tags_unrelated_service_exports_nothing() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");

    // The service under test binds NO engine at all.
    let empty_svc = format!("trino_{}", Uuid::new_v4().simple());
    store
        .create_service(&empty_svc, "trino", &json!({}))
        .await
        .expect("create empty service");

    // A DIFFERENT service, with a fully-migrated engine + tagged warehouse —
    // proves the fixture actually has tags to (not) leak.
    let other_svc = format!("trino_{}", Uuid::new_v4().simple());
    let other_row = store
        .create_service(&other_svc, "trino", &json!({}))
        .await
        .expect("create other service");
    let (wh_id, _catalog_name) = seed_bound_warehouse(&pool, other_row.id).await;
    seed_tag_assignment(&pool, wh_id).await;

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{empty_svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert!(
        srs.is_empty(),
        "a service with no bound engine must export nothing, got {srs:?}"
    );
}
