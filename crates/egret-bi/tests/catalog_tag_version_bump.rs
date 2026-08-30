//! DB-gated test for the `tag_version` bump on catalog-tag assign/unassign (Access P3.2,
//! Task 36 Part B). Self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and
//! otherwise FAILS.
//!
//! When a catalog tag is (un)assigned, the bound policy service's
//! `tag_version` must bump so the Ranger tag-download plugin re-polls
//! `download_tags` (otherwise it 304s forever and never sees the change). The
//! binding is `engine_mgmt.engines.authz_service_id` -> the warehouse's engine
//! (same join `PolicyStore::tag_assignments_for_service` documents).

use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::TagRepo;
use egret_metastore::Db;
use uuid::Uuid;

/// Seed a workspace + storage connection + warehouse + a Trino engine bound to
/// `service_id`. Returns the warehouse id.
async fn seed_bound_warehouse(pool: &sqlx::PgPool, service_id: i64) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("tvws-{}", Uuid::new_v4().simple()))
            .bind("Tag Version WS")
            .fetch_one(pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("tvsc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-tv-test")
    .bind("AKIATEST")
    .fetch_one(pool)
    .await
    .expect("insert storage connection");
    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("tvwh_{}", Uuid::new_v4().simple()))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec, authz_service_id) \
         VALUES ($1,'trino','egret',$2,'{}'::jsonb,$3)",
    )
    .bind(format!("tveng-{}", Uuid::new_v4().simple()))
    .bind(wh_id)
    .bind(service_id)
    .execute(pool)
    .await
    .expect("insert engine");
    wh_id
}

async fn create_service(pool: &sqlx::PgPool) -> i64 {
    // Seed a minimal service-def + service directly (no full policy seam needed).
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1,$2,'{}'::jsonb) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(950950_i64)
    .bind("trino_tv")
    .execute(pool)
    .await
    .expect("insert service-def");
    // `authz_services.type` is a FOREIGN KEY onto `authz_service_defs(name)`, so the service must
    // name the def this function just created. It said 'trino', which no migration seeds — the row
    // only ever existed because some OTHER test had called `upsert_service_def` against the same
    // database first. That made this test pass on a developer's accumulated database and on a
    // `--workspace` run where the other binary happened to go first, and fail on a clean one.
    sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) VALUES ($1,'trino_tv','{}'::jsonb) RETURNING id",
    )
    .bind(format!("trino_tv_{}", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .expect("insert service")
}

async fn tag_version(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("read tag_version")
}

#[tokio::test]
async fn assign_and_unassign_bump_bound_service_tag_version() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();

    let service_id = create_service(&pool).await;
    let wh = seed_bound_warehouse(&pool, service_id).await;
    // Freshly-created services default to tag_version = 1.
    assert_eq!(tag_version(&pool, service_id).await, 1);

    let by: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("tv_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .expect("insert user");
    let tags = TagRepo::new(db.clone());
    let repo = CatalogTagRepo::new(db.clone());
    let tag = tags
        .create(
            &format!("pii-{}", Uuid::new_v4().simple()),
            "PII",
            "Personal data",
            "#ff0000",
            Some(by),
        )
        .await
        .expect("create tag");

    // assign -> bound service's tag_version bumps.
    repo.assign_catalog(tag.id, wh, "sales", "orders", "email", Some(by))
        .await
        .expect("assign");
    assert_eq!(
        tag_version(&pool, service_id).await,
        2,
        "assign_catalog must bump the bound service's tag_version"
    );

    // unassign -> bumps again.
    repo.unassign_catalog(tag.id, wh, "sales", "orders", "email")
        .await
        .expect("unassign");
    assert_eq!(
        tag_version(&pool, service_id).await,
        3,
        "unassign_catalog must bump the bound service's tag_version"
    );
}
