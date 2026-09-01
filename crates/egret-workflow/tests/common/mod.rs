//! Shared helpers for `egret-workflow`'s DB-gated integration tests.
//!
//! Follows `crates/egret-query/tests/registry_repo.rs` (`repo_with_db`,
//! `seed_workspace`) rather than inventing a new shape.
#![allow(dead_code)]

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

/// Connect to the database named by `DATABASE_URL`, migrated, and ready.
///
/// `None` (self-skip) only when `egret_test_db::url()` says so -- no
/// `DATABASE_URL` and `EGRET_ALLOW_DB_SKIP` is set. Any other absence PANICS
/// (that crate's contract), so this suite cannot silently report `ok` having
/// asserted nothing.
pub async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// Seed a fresh user + workspace, returning `(workspace_id, user_id)` --
/// enough for every `FlowRepo` call in this crate's tests, which all take a
/// workspace and a `by: UserId`.
pub async fn seed_workspace(db: &Db) -> (Uuid, UserId) {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed user");

    let wid: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1, 't', $2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("seed workspace");

    (wid, UserId(uid))
}

/// Parse `source` and create a draft through `FlowRepo` -- shared by
/// `tests/publish.rs`, which needs a draft row to publish in every test.
///
/// `namespace`/`flow_id` come from the parsed flow itself (the same fields
/// `FlowRepo::create`'s caller is expected to supply, per `repo.rs`'s own
/// doc), not hard-coded here, so a test's YAML is the single source of truth
/// for what gets stored.
pub async fn draft(
    repo: &egret_workflow::FlowRepo,
    ws: Uuid,
    user: UserId,
    source: &str,
) -> egret_workflow::FlowRow {
    let flow = egret_workflow::spec::parse(source).expect("parse draft source");
    let plan = serde_json::to_value(&flow).expect("serialize draft plan");
    repo.create(
        ws,
        user,
        &egret_workflow::CreateFlowInput {
            namespace: flow.namespace.clone(),
            flow_id: flow.id.clone(),
            source: source.to_string(),
            plan,
            folder_id: None,
        },
    )
    .await
    .expect("create draft")
}
