//! DB-gated test for the `tag_version` bump on catalog-tag assign/unassign (Access P3.2,
//! Task 36 Part B). Self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and
//! otherwise FAILS.
//!
//! When a catalog tag is (un)assigned, the bound policy service's
//! `tag_version` must bump so the Ranger tag-download plugin re-polls
//! `download_tags` (otherwise it 304s forever and never sees the change). The
//! binding is `engine_mgmt.engines.authz_service_id` -> the warehouse's
//! engine(s), reached through a registry catalog attachment
//! (`registry.engine_catalog_attachments`, SP3 D5) — the same join
//! `PolicyStore::tag_assignments_for_service` serves under. There is no
//! `engines.warehouse_id` column any more (SP3c-2 Task 6) for a second,
//! attachment-free path.

use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::TagRepo;
use egret_metastore::Db;
use uuid::Uuid;

/// Seed a workspace + storage connection + warehouse + a Trino engine bound to
/// `service_id`, with the warehouse's registry catalog attached to that engine.
/// Returns the warehouse id.
///
/// The attachment is the binding. Until SP3c-2 Task 6 this function bound the
/// two through `engines.warehouse_id` instead, and when that column went the
/// INSERT simply dropped it — leaving an engine joined to the warehouse by
/// nothing, so `bump_bound_tag_versions` matched no service and the assertion
/// below failed. That failure was briefly labelled pre-existing; it was not.
/// A fixture that no longer expresses the binding it is named for makes its
/// test measure nothing, and this test guards the bump path that has already
/// failed open once in this programme.
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
    let (eng_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(format!("tveng-{}", Uuid::new_v4().simple()))
    .bind(service_id)
    .fetch_one(pool)
    .await
    .expect("insert engine");
    let catalog_id = seed_registry_catalog(pool, wh_id).await;
    attach_catalog(pool, eng_id, catalog_id).await;
    wh_id
}

/// A bare workspace + storage connection + warehouse, no engine attached.
/// Returns the warehouse id.
async fn seed_warehouse(pool: &sqlx::PgPool) -> Uuid {
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
    wh_id
}

/// A Trino engine bound to `service_id`. Returns the engine id.
async fn seed_engine(pool: &sqlx::PgPool, service_id: i64) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(format!("tveng-{}", Uuid::new_v4().simple()))
    .bind(service_id)
    .fetch_one(pool)
    .await
    .expect("insert engine")
}

/// A registry catalog for `warehouse_id`. Returns the catalog id.
async fn seed_registry_catalog(pool: &sqlx::PgPool, warehouse_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(format!("tvcat_{}", Uuid::new_v4().simple()))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("insert registry catalog")
}

/// Attach `catalog_id` to `engine_id` (`registry.engine_catalog_attachments`).
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

async fn create_service(db: &Db) -> i64 {
    // The real Trino service-def, seeded the way the server does at startup, so
    // the profile is of the engine's own type. An engine bound to a profile of
    // another type is refused at compile (d-i), so a fake type would not compile.
    egret_policy::seed_trino(&egret_policy::PolicyStore::new(db.clone()))
        .await
        .expect("seed trino service-def");
    sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) VALUES ($1,'trino','{}'::jsonb) RETURNING id",
    )
    .bind(format!("trino_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
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

    let service_id = create_service(&db).await;
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
            &format!("PII {}", Uuid::new_v4().simple()),
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

/// SP3 catalog-registry review I1: a tag change on a warehouse attached to an
/// engine as a SECONDARY catalog (not its primary attachment) must
/// still bump that engine's bound service `tag_version` — otherwise
/// `download_tags` keeps 304-ing and the new mask/tag never reaches the
/// Ranger plugin. Pins `bump_bound_tag_versions`'s registry-attachment branch.
#[tokio::test]
async fn secondary_attachment_bump_reaches_engines_bound_service() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();

    let service_id = create_service(&db).await;
    let eng_b = seed_engine(&pool, service_id).await;

    // Warehouse W is not attached at all yet; B attaches W's registry catalog.
    let wh_w = seed_warehouse(&pool).await;
    let catalog_w = seed_registry_catalog(&pool, wh_w).await;
    attach_catalog(&pool, eng_b, catalog_w).await;

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
            &format!("PII {}", Uuid::new_v4().simple()),
            "Personal data",
            "#ff0000",
            Some(by),
        )
        .await
        .expect("create tag");

    // Tag a column on W — NOT B's primary warehouse.
    repo.assign_catalog(tag.id, wh_w, "sales", "orders", "email", Some(by))
        .await
        .expect("assign");
    assert_eq!(
        tag_version(&pool, service_id).await,
        2,
        "a tag change on a SECONDARY attachment must bump the attaching engine's bound service"
    );
}
