//! DB-gated: the backfill's `tag_version` bump is part of the attachment's own
//! transaction, so an interrupted boot leaves BOTH or NEITHER (SP3
//! catalog-registry review F-14). Self-skips without DATABASE_URL ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Why this matters more than the usual atomicity argument: step 2 selects only
//! legacy rows that are NOT yet in `registry.legacy_engine_catalogs`. Once the
//! attachment and its marker commit, the row is never looked at again — so a
//! bump left for a later pass and lost to a kill is lost permanently, and the
//! plugin 304s forever on a catalog the export has already started emitting.
//!
//! Its own test binary — and therefore its own database — because
//! `run_backfill` is global and the fault injected here (a trigger that fails
//! every `tag_version` update) would reach any other test's services.
//!
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test download_tags_backfill_atomicity

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

/// A workspace + storage connection + one warehouse. Returns `(id, name)`.
async fn seed_warehouse(pool: &sqlx::PgPool, ws_id: Uuid, sc_id: Uuid) -> (Uuid, String) {
    let name = format!("atwh_{}", Uuid::new_v4().simple());
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(&name)
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    (id, name)
}

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

/// F-14: a boot whose `tag_version` bump cannot complete must commit no
/// attachment either — the row stays unmapped, is reported, and the NEXT boot
/// migrates it whole, ending with the plugin getting its 200. The alternative
/// (bump after the attachment's transaction) commits the attachment, marks the
/// row migrated, and loses the bump for good.
#[tokio::test]
async fn an_interrupted_bump_takes_the_attachment_with_it_and_the_next_boot_delivers() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db.clone());
    seed_trino(&store).await.expect("seed trino def");
    seed_tag(&store).await.expect("seed tag def");

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
    let last_known = tag_row.tag_version;

    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("atws-{base}"))
            .bind("Atomic Backfill WS")
            .fetch_one(&pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, 'us-east-1', 'egret-at-test', 'AKIATEST') RETURNING id",
    )
    .bind(format!("atsc-{base}"))
    .fetch_one(&pool)
    .await
    .expect("insert storage connection");
    let (wh1, wh1_name) = seed_warehouse(&pool, ws_id, sc_id).await;
    let (wh2, wh2_name) = seed_warehouse(&pool, ws_id, sc_id).await;
    let (engine_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec, authz_service_id) \
         VALUES ($1,'trino','egret',$2,'{}'::jsonb,$3) RETURNING id",
    )
    .bind(format!("ateng-{base}"))
    .bind(wh1)
    .bind(trino_row.id)
    .fetch_one(&pool)
    .await
    .expect("insert engine");
    seed_legacy_row(&pool, engine_id, "egret", &wh1_name, true).await;
    seed_legacy_row(&pool, engine_id, "secondary", &wh2_name, false).await;

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

    let registry = egret_catalog_registry::RegistryStore::new(db.clone());
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );

    // The interruption, injected where it is deterministic: the bump statement
    // itself fails, exactly as a killed process's uncommitted bump never
    // happens. Anything the attachment's transaction did must go with it.
    sqlx::query(
        "CREATE OR REPLACE FUNCTION pg_temp_block_tag_bump() RETURNS trigger \
         LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'tag bump interrupted'; END $$",
    )
    .execute(&pool)
    .await
    .expect("create trigger fn");
    sqlx::query(
        "CREATE TRIGGER block_tag_bump BEFORE UPDATE OF tag_version ON policy.authz_services \
         FOR EACH ROW EXECUTE FUNCTION pg_temp_block_tag_bump()",
    )
    .execute(&pool)
    .await
    .expect("create trigger");

    let interrupted = egret_catalog_registry::run_backfill(&registry, &secrets)
        .await
        .expect("the run itself survives a failing row");
    assert!(
        interrupted.failed_rows > 0,
        "the interrupted rows must be reported, not silently skipped"
    );

    // NEITHER: no attachment, and the legacy rows are still unmapped, so the
    // next boot retries them whole.
    let attachments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(engine_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        attachments, 0,
        "an attachment whose bump could not commit must not be committed either"
    );
    let unmapped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM engine_mgmt.engine_catalogs c \
          WHERE c.engine_id = $1 \
            AND NOT EXISTS (SELECT 1 FROM registry.legacy_engine_catalogs m \
                             WHERE m.engine_catalog_id = c.id)",
    )
    .bind(engine_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unmapped, 2, "both rows are left for the next boot");
    let stalled: i64 =
        sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(tag_row.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stalled, last_known, "nothing was bumped");
    let r = app(store.clone())
        .oneshot(get(&format!(
            "/tags/download/{tag_svc}?lastKnownVersion={last_known}"
        )))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_MODIFIED,
        "nothing reached the plugin yet, and nothing claimed it had"
    );

    // The boot after the interruption.
    sqlx::query("DROP TRIGGER block_tag_bump ON policy.authz_services")
        .execute(&pool)
        .await
        .expect("drop trigger");
    let resumed = egret_catalog_registry::run_backfill(&registry, &secrets)
        .await
        .expect("resumed backfill");
    assert_eq!(resumed.failed_rows, 0, "the retry migrates both rows");

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
            .unwrap();
    assert!(
        bumped > last_known,
        "the resumed boot must bump ({bumped} vs {last_known})"
    );

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
}
