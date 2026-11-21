//! KI-316: a warehouse whose storage overlaps one the catalog already holds is
//! refused however its storage connection spells the store. lakekeeper's own
//! check compares the region and the endpoint as written.
//!
//! The held warehouse is an in-memory one given an S3 storage profile in the
//! database (lakekeeper never touches its storage here). The new warehouse
//! goes through egret's provisioning path, `create_lakekeeper_warehouse`; an
//! overlap is refused before lakekeeper is asked, and anything else reaches
//! lakekeeper, whose storage validation fails, since no store listens at the
//! endpoint.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_catalog_engine::{
    build_state, create_lakekeeper_warehouse, create_memory_warehouse, ensure_bootstrapped,
    run_lakekeeper_migrations, CatalogEngineError, S3StorageConfig,
};
use serde_json::json;

const SYSTEM_USER: &str = "egret-system";

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

type State = egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>;

/// A bootstrapped engine state, and a pool on the same database.
async fn engine() -> Option<(State, sqlx::PgPool)> {
    let dsn = egret_test_db::url().await?;
    let db = egret_metastore::Db::connect(&dsn, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    LAKEKEEPER
        .get_or_init(|| async {
            run_lakekeeper_migrations(&dsn)
                .await
                .expect("lakekeeper migrations");
            let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
    let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    Some((state, db.pool))
}

/// Give the in-memory warehouse `id` an S3 storage profile on `bucket` at
/// `http://rustfs:9000`, region `us-east-1`, key prefix `wh`, as lakekeeper
/// stores it once it has normalised it.
async fn give_s3_profile(pool: &sqlx::PgPool, id: &str, bucket: &str) {
    let profile = json!({
        "type": "s3",
        "bucket": bucket,
        "region": "us-east-1",
        "endpoint": "http://rustfs:9000/",
        "path-style-access": true,
        "flavor": "s3-compat",
        "sts-enabled": true,
        "remote-signing-enabled": false,
        "key-prefix": "wh",
    });
    sqlx::query(
        "UPDATE lakekeeper.warehouse SET storage_profile = $2 WHERE warehouse_id = $1::uuid",
    )
    .bind(id)
    .bind(&profile)
    .execute(pool)
    .await
    .expect("storage profile");
}

/// The engine state, and the name of a held warehouse on `bucket`.
async fn held_warehouse(bucket: &str) -> Option<(State, String)> {
    let (state, pool) = engine().await?;
    let name = unique("held");
    let id = create_memory_warehouse(state.clone(), SYSTEM_USER, &name)
        .await
        .expect("held warehouse");
    give_s3_profile(&pool, &id, bucket).await;
    Some((state, name))
}

fn s3(bucket: &str, endpoint: &str, region: &str, key_prefix: Option<&str>) -> S3StorageConfig {
    S3StorageConfig {
        bucket: bucket.to_string(),
        region: region.to_string(),
        endpoint: Some(endpoint.to_string()),
        path_style: true,
        access_key: "AK".into(),
        secret_key: "SK".into(),
        key_prefix: key_prefix.map(str::to_string),
        sts_token_validity_seconds: 900,
    }
}

#[tokio::test]
async fn an_overlapping_warehouse_is_refused_however_its_store_is_written() {
    let bucket = unique("lake");
    let Some((state, held)) = held_warehouse(&bucket).await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    for (what, cfg) in [
        (
            "another region",
            s3(&bucket, "http://rustfs:9000", "eu-west-1", Some("wh")),
        ),
        (
            "upper case, explicit path and a nested prefix",
            s3(
                &bucket,
                "HTTP://RUSTFS:9000/",
                "us-east-1",
                Some("/wh/sub/"),
            ),
        ),
        (
            "the whole bucket, the bucket in the path",
            s3(
                &bucket,
                &format!("http://rustfs:9000/{bucket}"),
                "us-east-1",
                None,
            ),
        ),
        (
            "https on the same host and port",
            s3(&bucket, "https://rustfs:9000", "local", Some("wh")),
        ),
    ] {
        let got =
            create_lakekeeper_warehouse(state.clone(), SYSTEM_USER, &unique("new"), &cfg).await;
        match got {
            Err(CatalogEngineError::Conflict(m)) => {
                assert!(m.contains(&held), "{what}: names the held warehouse: {m}")
            }
            other => panic!("{what}: an overlap is a conflict, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn storage_that_does_not_overlap_reaches_the_catalog() {
    let bucket = unique("lake");
    let Some((state, _)) = held_warehouse(&bucket).await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    for (what, cfg) in [
        (
            "a sibling prefix",
            s3(&bucket, "http://rustfs:9000", "us-east-1", Some("wh2")),
        ),
        (
            "another bucket",
            s3(
                &unique("lake"),
                "http://rustfs:9000",
                "us-east-1",
                Some("wh"),
            ),
        ),
        (
            "another store",
            s3(&bucket, "http://127.0.0.1:1", "us-east-1", Some("wh")),
        ),
        (
            "a host written as a path",
            s3(
                &bucket,
                "http://127.0.0.1:1/rustfs:9000",
                "us-east-1",
                Some("wh"),
            ),
        ),
    ] {
        let got =
            create_lakekeeper_warehouse(state.clone(), SYSTEM_USER, &unique("new"), &cfg).await;
        assert!(
            !matches!(got, Err(CatalogEngineError::Conflict(_)) | Ok(_)),
            "{what}: not refused as an overlap, and lakekeeper cannot reach a store: {got:?}"
        );
    }
}

/// Two warehouses created at the same moment are checked against each other:
/// a create holds the warehouse-creation lock from its overlap check to its
/// insert, so it waits for a create in flight and then sees its warehouse.
///
/// Deterministic, with no race to win: the test holds the lock as a create in
/// flight would, starts a create, shows it waits, then lets the "other create"
/// land an overlapping warehouse and release the lock. The waiting create must
/// then refuse.
#[tokio::test]
async fn a_create_waits_for_one_in_flight_and_then_sees_its_warehouse() {
    let Some((state, pool)) = engine().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let bucket = unique("lake");
    let in_flight = unique("inflight");
    let id = create_memory_warehouse(state.clone(), SYSTEM_USER, &in_flight)
        .await
        .expect("the warehouse the create in flight lands");

    let mut lock = pool.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(egret_catalog_engine::WAREHOUSE_CREATE_LOCK)
        .execute(&mut *lock)
        .await
        .expect("take the warehouse-creation lock");

    let cfg = s3(&bucket, "http://rustfs:9000", "us-east-1", Some("wh"));
    let waiting = tokio::spawn({
        let state = state.clone();
        async move { create_lakekeeper_warehouse(state, SYSTEM_USER, &unique("new"), &cfg).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !waiting.is_finished(),
        "a create must wait while another holds the warehouse-creation lock"
    );

    give_s3_profile(&pool, &id, &bucket).await;
    lock.commit().await.expect("release the lock");

    match waiting.await.expect("join") {
        Err(CatalogEngineError::Conflict(m)) => {
            assert!(
                m.contains(&in_flight),
                "names the warehouse that landed: {m}"
            )
        }
        other => panic!("the waiting create must see the overlap, got {other:?}"),
    }
}
