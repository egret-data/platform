//! SP3 catalog-registry review N1 (Important): the Trino Ranger tag plugin
//! polls the TAG service (`<profile>_tag`), and `download_tags`
//! (`egret_policy::ranger::api::download`) gates its `304 Not Modified` on
//! **that polled service's own** `tag_version` — not the trino profile's.
//! Before this fix, `CatalogTagRepo::bump_bound_tag_versions`
//! (`egret-bi/src/catalog_tags.rs`) only ever bumped the row reached through
//! `engines.authz_service_id` (the trino profile), so the tag service's
//! `tag_version` never moved and the plugin 304'd forever after a real tag
//! change — the C1/I1 union closed the WHICH-catalog gate but not this
//! WHICH-service-gets-bumped gate.
//!
//! This lives in `egret-server` (not `egret-bi` or `egret-policy`) because it
//! is the one crate that already depends on BOTH `egret-bi` (the write side,
//! `CatalogTagRepo::assign_catalog`) and `egret-policy` (the read side,
//! `ranger_router`'s `download_tags`) as real (non-dev) dependencies — proving
//! the fix requires driving both.
//!
//! DB-gated: self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-server --test tag_bump_reaches_polled_tag_service

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::TagRepo;
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
        mode: egret_policy::ranger::AuthMode::Required,
        verifier: None,
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

/// A workspace + storage connection + warehouse + a Trino engine bound to
/// `trino_profile_id`, with the warehouse's registry catalog attached (the
/// only channel left, since SP3c-2 Task 6, for an engine to reach a
/// warehouse). Returns the warehouse id.
async fn seed_warehouse_and_engine(pool: &sqlx::PgPool, trino_profile_id: i64) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("tagbumpws-{}", Uuid::new_v4().simple()))
            .bind("Tag Bump WS")
            .fetch_one(pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("tagbumpsc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-tag-bump-test")
    .bind("AKIATEST")
    .fetch_one(pool)
    .await
    .expect("insert storage connection");
    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("tagbumpwh_{}", Uuid::new_v4().simple()))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    let (eng_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(format!("tagbumpeng-{}", Uuid::new_v4().simple()))
    .bind(trino_profile_id)
    .fetch_one(pool)
    .await
    .expect("insert engine");
    // `tag_assignments_for_service` (egret-policy) needs a registry catalog
    // attached to the engine to resolve the warehouse through — without one it
    // exports nothing regardless of the bump fix under test.
    let (catalog_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(format!("tagbumpcat_{}", Uuid::new_v4().simple()))
    .bind(wh_id)
    .fetch_one(pool)
    .await
    .expect("insert registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(eng_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
    wh_id
}

/// A tag change on a warehouse bumps the TAG SERVICE the Trino plugin actually
/// polls (its trino profile's `config.tagService`), not just the trino
/// profile itself, and `download_tags` on the tag service's name answers 200
/// with the new payload — not a 304 — once the plugin's cached version is
/// stale.
#[tokio::test]
async fn tag_change_bumps_the_polled_tag_service_not_just_the_trino_profile() {
    let Some(db) = db().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db.clone());
    seed_trino(&store).await.expect("seed trino def");
    seed_tag(&store).await.expect("seed tag def");

    let base = Uuid::new_v4().simple().to_string();
    let trino_name = format!("trino_{base}");
    let tag_svc_name = format!("{trino_name}_tag");
    // The trino profile's `tagService` config is EXACTLY the field
    // `crates/egret-engine-mgmt/src/engines.rs`'s auto-bind writes when it
    // creates the paired tag service (`config: { "tagService":
    // format!("{name}_tag") }`), and the SAME field
    // `tag_assignments_for_service`'s `LEFT JOIN ... tp.config_jsonb->>
    // 'tagService' = s.name` reads on the serving side.
    let tag_row = store
        .create_service(&tag_svc_name, "tag", &json!({}))
        .await
        .expect("create tag service");
    let trino_row = store
        .create_service(&trino_name, "trino", &json!({ "tagService": tag_svc_name }))
        .await
        .expect("create trino profile");
    assert_eq!(trino_row.tag_version, 1);
    assert_eq!(
        tag_row.tag_version, 1,
        "fresh tag service starts at tag_version 1"
    );

    // An engine bound to the TRINO PROFILE (not the tag service) — this is how
    // every engine binds, per `tag_assignments_for_service`'s own comment.
    let wh_id = seed_warehouse_and_engine(&pool, trino_row.id).await;

    let by: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("tagbump_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .expect("insert user");
    let tags = TagRepo::new(db.clone());
    let repo = CatalogTagRepo::new(db.clone());
    let tag = tags
        .create(
            &format!("pii-{base}"),
            "PII",
            "Personal data",
            "#ff0000",
            Some(by),
        )
        .await
        .expect("create tag");

    // The Trino plugin has already cached tagVersion=1 for the TAG service —
    // simulate that by polling once, before the tag change.
    let app_router = app(store.clone());
    let r = app_router
        .clone()
        .oneshot(get(&format!(
            "/tags/download/{tag_svc_name}?lastKnownVersion=1"
        )))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_MODIFIED,
        "the plugin's cached version 1 must match before any tag exists"
    );

    // The tag change.
    repo.assign_catalog(tag.id, wh_id, "sales", "orders", "email", Some(by))
        .await
        .expect("assign");

    // The TAG SERVICE's own tag_version moved — this is the row `download_tags`
    // actually gates its 304 on when the plugin polls by tag-service name.
    let tag_svc_after = store
        .get_service(&tag_svc_name)
        .await
        .expect("get tag service");
    assert_eq!(
        tag_svc_after.tag_version, 2,
        "the polled TAG service's tag_version must move on a tag change, not just the trino profile's"
    );

    // Polling the tag service with the STALE cached version now gets 200 with
    // the new payload, not another 304.
    let r = app_router
        .oneshot(get(&format!(
            "/tags/download/{tag_svc_name}?lastKnownVersion=1"
        )))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "a stale cached version must now get 200, not 304, after the tag change"
    );
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(st["tagVersion"], 2);
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(srs.len(), 1, "one resource, got {srs:?}");
    assert_eq!(srs[0]["resourceElements"]["column"]["values"][0], "email");
}
