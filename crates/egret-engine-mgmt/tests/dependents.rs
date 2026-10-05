//! The engine-delete preflight (`egret_engine_mgmt::dependents`) against a real
//! schema: the FK tree it classifies, what it counts, and the locked row delete.
//!
//! DB-gated: without `DATABASE_URL` this FAILS unless `EGRET_ALLOW_DB_SKIP` is
//! set, in which case every test prints SKIPPED and asserts nothing — check the
//! duration, not only the `ok`.

use egret_engine_mgmt::dependents::{
    ChildOnDelete, DeleteEngineOutcome, EngineDependentKind, QUERY_DATABASES_CHILD_FKS,
};
use egret_engine_mgmt::EngineMgmtStore;
use egret_metastore::Db;
use std::collections::BTreeSet;
use uuid::Uuid;

/// Every single-column FK pointing at `$1`, as (child table, column, confdeltype, NOT NULL).
const FK_CHILDREN_SQL: &str = "\
SELECT n.nspname || '.' || cl.relname, a.attname::text, c.confdeltype::text, a.attnotnull \
  FROM pg_constraint c \
  JOIN pg_class cl ON cl.oid = c.conrelid \
  JOIN pg_namespace n ON n.oid = cl.relnamespace \
  JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1] \
 WHERE c.contype = 'f' AND c.confrelid = $1::regclass";

/// CHECK constraints on table `$1` that involve column `$2`.
const CHECKS_ON_COLUMN_SQL: &str = "\
SELECT c.conname::text FROM pg_constraint c \
 WHERE c.contype = 'c' AND c.conrelid = $1::regclass \
   AND EXISTS (SELECT 1 FROM pg_attribute a \
                WHERE a.attrelid = c.conrelid AND a.attnum = ANY(c.conkey) AND a.attname = $2) \
 ORDER BY 1";

type FkRow = (String, String, String, bool);

async fn harness() -> Option<(Db, EngineMgmtStore)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = EngineMgmtStore::new(db.clone());
    Some((db, store))
}

macro_rules! harness_or_skip {
    () => {
        match harness().await {
            Some(h) => h,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

async fn fk_children(db: &Db, parent: &str) -> BTreeSet<FkRow> {
    sqlx::query_as::<_, FkRow>(FK_CHILDREN_SQL)
        .bind(parent)
        .fetch_all(&db.pool)
        .await
        .expect("read pg_constraint")
        .into_iter()
        .collect()
}

async fn seed_workspace(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("dep-{}", Uuid::new_v4().simple()))
    .bind(name)
    .fetch_one(&db.pool)
    .await
    .expect("seed workspace")
}

async fn seed_engine(db: &Db, store: &EngineMgmtStore) -> Uuid {
    let catalog = egret_catalog_engine::CatalogStore::new(db.clone());
    let owner = seed_workspace(db, "Warehouse owner").await;
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("dep-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed storage connection");
    let _wh = catalog
        .create_warehouse(&format!("dep-wh-{}", Uuid::new_v4().simple()), owner, sc)
        .await
        .expect("create warehouse");
    store
        .create_engine(
            &format!("dep-eng-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .expect("seed engine")
        .id
}

/// A materialized workspace connection for `engine_id`, as `assign` would write it.
async fn seed_connection(db: &Db, ws: Uuid, engine_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, engine_id) \
         VALUES ($1, $2, 'trino', 'coordinator', 8443, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("conn-{}", Uuid::new_v4().simple()))
    .bind(engine_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed connection")
}

async fn seed_endpoint(db: &Db, ws: Uuid, database_id: Uuid, name: &str, trashed: bool) {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints (workspace_id, name, path, method, database_id, statements, state) \
         VALUES ($1, $2, $3, 'GET', $4, \
                 jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), 'draft') \
         RETURNING id",
    )
    .bind(ws)
    .bind(name)
    .bind(format!("/{name}"))
    .bind(database_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed endpoint");
    if trashed {
        sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&db.pool)
            .await
            .expect("trash endpoint");
    }
}

async fn seed_alert(db: &Db, ws: Uuid, database_id: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO automation.schedules (workspace_id, type, name, crontab, database_id, sql, validator_type) \
         VALUES ($1, 'alert', $2, '0 9 * * *', $3, 'SELECT 1', 'not_null')",
    )
    .bind(ws)
    .bind(name)
    .bind(database_id)
    .execute(&db.pool)
    .await
    .expect("seed alert");
}

async fn seed_dataset(db: &Db, ws: Uuid, database_id: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
         VALUES ($1, $2, $3, 't')",
    )
    .bind(ws)
    .bind(database_id)
    .bind(name)
    .execute(&db.pool)
    .await
    .expect("seed dataset");
}

/// The guard the spec asks for: a new child of `query.databases` — or a changed
/// action on an existing one — fails here until `QUERY_DATABASES_CHILD_FKS` (and
/// with it the preflight) classifies it. A RESTRICT child the preflight does not
/// count is exactly the silent 500 this plan exists to remove.
#[tokio::test]
async fn every_fk_child_of_query_databases_is_classified_by_the_preflight() {
    let (db, _store) = harness_or_skip!();
    let live = fk_children(&db, "query.databases").await;
    let classified: BTreeSet<FkRow> = QUERY_DATABASES_CHILD_FKS
        .iter()
        .map(|c| {
            (
                c.table.to_string(),
                c.column.to_string(),
                c.on_delete.to_string(),
                c.not_null,
            )
        })
        .collect();
    assert_eq!(
        live.len(),
        5,
        "positive control — the query found the known children: {live:?}"
    );
    assert_eq!(
        live, classified,
        "the FK children of query.databases changed; classify the new/changed child in \
         dependents::QUERY_DATABASES_CHILD_FKS and, if it blocks, count it in DEPENDENTS_SQL"
    );
}

/// `SetsNull` is only safe while no CHECK needs the column. `automation.schedules`
/// is the counter-example, and is classified `Blocks` because of it.
#[tokio::test]
async fn set_null_children_have_no_check_that_needs_the_column() {
    let (db, _store) = harness_or_skip!();
    for fk in QUERY_DATABASES_CHILD_FKS {
        let checks: Vec<String> = sqlx::query_scalar(CHECKS_ON_COLUMN_SQL)
            .bind(fk.table)
            .bind(fk.column)
            .fetch_all(&db.pool)
            .await
            .expect("read checks");
        match fk.handling {
            ChildOnDelete::SetsNull => assert!(
                checks.is_empty(),
                "{}.{} is SET NULL but CHECKs {checks:?} involve it — it must block instead",
                fk.table,
                fk.column
            ),
            ChildOnDelete::Blocks(EngineDependentKind::Schedule) => assert!(
                checks.iter().any(|c| c == "schedule_alert_needs_query"),
                "positive control: the CHECK that makes schedules block: {checks:?}"
            ),
            _ => {}
        }
    }
}

/// Deleting the engine row cascades these three; nothing else may reference it.
#[tokio::test]
async fn every_fk_child_of_engines_cascades() {
    let (db, _store) = harness_or_skip!();
    let expected: BTreeSet<FkRow> = [
        ("engine_mgmt.engine_workspaces", "engine_id", "c", true),
        ("query.databases", "engine_id", "c", false),
        (
            "registry.engine_catalog_attachments",
            "engine_id",
            "c",
            true,
        ),
    ]
    .into_iter()
    .map(|(t, c, a, n)| (t.to_string(), c.to_string(), a.to_string(), n))
    .collect();
    assert_eq!(fk_children(&db, "engine_mgmt.engines").await, expected);
}

#[tokio::test]
async fn engine_dependents_groups_by_workspace_and_kind_and_caps_the_sample() {
    let (db, store) = harness_or_skip!();
    let engine = seed_engine(&db, &store).await;
    let other_engine = seed_engine(&db, &store).await;
    let ws_a = seed_workspace(&db, "Alpha").await;
    let ws_b = seed_workspace(&db, "Beta").await;
    let conn_a = seed_connection(&db, ws_a, engine).await;
    let conn_b = seed_connection(&db, ws_b, engine).await;
    // Same workspace, ANOTHER engine's connection: must not count.
    let foreign = seed_connection(&db, ws_a, other_engine).await;

    for n in ["g", "f", "e", "d", "c", "b", "a"] {
        seed_endpoint(&db, ws_a, conn_a, n, false).await;
    }
    seed_endpoint(&db, ws_a, conn_a, "binned", true).await;
    seed_endpoint(&db, ws_a, foreign, "not-mine", false).await;
    seed_alert(&db, ws_b, conn_b, "nightly").await;
    seed_dataset(&db, ws_b, conn_b, "orders").await;

    let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let summary = |deps: Vec<egret_engine_mgmt::dependents::EngineDependentDto>| {
        deps.into_iter()
            .map(|d| (d.workspace_id, d.kind, d.count, d.sample))
            .collect::<Vec<_>>()
    };

    let all = store
        .engine_dependents(engine, None)
        .await
        .expect("dependents");
    assert_eq!(all[0].workspace_name, "Alpha");
    assert!(all[0].workspace_slug.starts_with("dep-"), "{:?}", all[0]);
    assert_eq!(
        summary(all),
        vec![
            (
                ws_a,
                EngineDependentKind::DataapiEndpoint,
                7,
                strings(&["a", "b", "c", "d", "e"])
            ),
            (
                ws_a,
                EngineDependentKind::DataapiEndpointTrashed,
                1,
                strings(&["binned"])
            ),
            (ws_b, EngineDependentKind::Dataset, 1, strings(&["orders"])),
            (
                ws_b,
                EngineDependentKind::Schedule,
                1,
                strings(&["nightly"])
            ),
        ]
    );

    let only_b = store
        .engine_dependents(engine, Some(&[ws_b]))
        .await
        .expect("dependents restricted to one workspace");
    assert_eq!(
        summary(only_b),
        vec![
            (ws_b, EngineDependentKind::Dataset, 1, strings(&["orders"])),
            (
                ws_b,
                EngineDependentKind::Schedule,
                1,
                strings(&["nightly"])
            ),
        ]
    );
    assert!(store
        .engine_dependents(engine, Some(&[]))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn delete_engine_unless_blocked_keeps_the_row_while_a_dependent_exists() {
    let (db, store) = harness_or_skip!();
    let engine = seed_engine(&db, &store).await;
    let ws = seed_workspace(&db, "Gamma").await;
    let conn = seed_connection(&db, ws, engine).await;
    seed_alert(&db, ws, conn, "blocks-the-delete").await;

    match store
        .delete_engine_unless_blocked(engine)
        .await
        .expect("outcome")
    {
        DeleteEngineOutcome::Blocked(d) => {
            assert_eq!(d.len(), 1, "{d:?}");
            assert_eq!(d[0].kind, EngineDependentKind::Schedule);
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert!(
        store.get_engine(engine).await.unwrap().is_some(),
        "the row must survive"
    );

    sqlx::query("DELETE FROM automation.schedules WHERE database_id = $1")
        .bind(conn)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        store.delete_engine_unless_blocked(engine).await.unwrap(),
        DeleteEngineOutcome::Deleted
    ));
    assert!(store.get_engine(engine).await.unwrap().is_none());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM query.databases WHERE id = $1")
        .bind(conn)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        left, 0,
        "the materialized connection cascades with the engine"
    );
    assert!(matches!(
        store.delete_engine_unless_blocked(engine).await.unwrap(),
        DeleteEngineOutcome::NotFound
    ));
}
