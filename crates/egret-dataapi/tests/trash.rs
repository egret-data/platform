//! Migration 0126: `dataapi.endpoints.deleted_at` plus the CHECK that makes a
//! trashed-but-live endpoint unrepresentable in the schema.
//!
//! The serving path resolves an endpoint through exactly one query
//! (`serving/router.rs::live_snapshot`), which filters `state = 'live'`. That
//! predicate can be forgotten by whoever writes the next query — the Alerts
//! programme shipped exactly that bug, a soft-deleted schedule that kept
//! firing. A CHECK constraint cannot be forgotten the way a WHERE clause can,
//! so this test proves the guarantee holds against a writer that goes
//! straight to SQL and never reads this plan, let alone a route handler.
//!
//! DB-gated. Self-skips (passes) when DATABASE_URL is unset, like
//! `bookmarks.rs` — so check the duration, not just the `ok`.

use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    ws_id: Uuid,
    admin: Uuid,
    database_id: Uuid,
}

async fn insert_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn insert_workspace(db: &Db, owner: Uuid, tag: &str) -> Uuid {
    let slug = format!("{tag}-{}", Uuid::new_v4().simple());
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Data API Trash Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace")
}

async fn insert_database(db: &Db, ws: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

async fn fixture() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let admin = insert_user(&db, "dataapi_trash_admin").await;
    let ws_id = insert_workspace(&db, admin, "dataapi-trash").await;
    let database_id = insert_database(&db, ws_id, admin).await;
    Some(Fixture {
        db,
        ws_id,
        admin,
        database_id,
    })
}

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

impl Fixture {
    /// An endpoint written straight to the table with the given `state`, so a
    /// test can put it in `'live'` without going through the publish route.
    async fn create_endpoint(&self, name: &str, path: &str, state: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, sql, state, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, 'SELECT 1', $5, $6) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(name)
        .bind(path)
        .bind(self.database_id)
        .bind(state)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint")
    }
}

/// The CHECK constraint added in migration 0126 must refuse a direct SQL
/// UPDATE that would soft-delete a `live` endpoint — bypassing every route,
/// because the guarantee has to hold against a writer that never read this
/// plan, not merely against the handlers this crate happens to expose today.
#[tokio::test]
async fn trashing_a_live_endpoint_is_refused_by_the_database() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue", "live").await;

    let err = sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&f.db.pool)
        .await
        .expect_err("the database must refuse to trash a live endpoint");

    let msg = err.to_string();
    assert!(
        msg.contains("dataapi_endpoints_trashed_not_live"),
        "expected the CHECK constraint to be the reason for the refusal, got: {msg}"
    );

    let deleted_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("select endpoint");
    assert_eq!(
        deleted_at, None,
        "a refused UPDATE must leave the row untouched, not half-applied"
    );
}

/// The same UPDATE against a non-`live` endpoint (here: `draft`) is exactly
/// what soft-delete needs to allow — this pins the constraint to `state =
/// 'live'` specifically, not to trashing in general.
#[tokio::test]
async fn trashing_a_draft_endpoint_is_allowed() {
    let f = skip_without_db!();
    let id = f.create_endpoint("cost", "/reports/cost", "draft").await;

    sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&f.db.pool)
        .await
        .expect("a draft endpoint must be trashable");

    let deleted_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("select endpoint");
    assert!(deleted_at.is_some(), "deleted_at must have been written");
}
