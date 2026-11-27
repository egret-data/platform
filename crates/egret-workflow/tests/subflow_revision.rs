//! Package #12 Task 2: a `Subflow`'s `revision` spawns that published
//! revision of the target (Kestra `Subflow.revision`); absent = the latest.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5442/egret_test
//!   cargo test -p egret-workflow --test subflow_revision

use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

mod common;

fn child(task: &str) -> String {
    format!(
        "id: child\nnamespace: t\ntasks:\n  - id: {task}\n    \
         type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

fn parent(extra: &str) -> String {
    format!(
        "id: parent\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n    \
         wait: false\n{extra}"
    )
}

/// File `child` with its draft (the latest) holding task "three" and
/// published revisions 1 and 2 holding "one" and "two".
async fn seed(db: &Db) -> Uuid {
    let (ws, user) = common::seed_workspace(db).await;
    let latest = child("three");
    let flow = egret_workflow::spec::parse(&latest).expect("parses");
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let row = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: flow.namespace.clone(),
                flow_id: flow.id.clone(),
                source: latest,
                plan: serde_json::to_value(&flow).expect("serialises"),
                folder_id: None,
            },
        )
        .await
        .expect("file the child");
    for task in ["one", "two"] {
        repo.insert_version(
            row.id,
            user,
            &json!({ "source": child(task), "plan": {}, "warnings": [] }),
            "",
        )
        .await
        .expect("insert version");
    }
    ws
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    egret_workflow::worker::start_execution_with_trigger_labels(
        pool,
        ws,
        None,
        source,
        json!({}),
        None,
        vec![],
    )
    .await
    .expect("start the parent")
}

/// The spawned child's source, if one was spawned.
async fn child_source(pool: &sqlx::PgPool, parent: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT source FROM workflow.executions WHERE trigger_execution_id = $1")
        .bind(parent)
        .fetch_optional(pool)
        .await
        .expect("read the child")
}

#[tokio::test]
async fn a_pinned_revision_runs_that_revisions_source() {
    let Some(db) = common::db().await else { return };
    let ws = seed(&db).await;
    for (rev, task) in [(1, "id: one"), (2, "id: two")] {
        let p = start(&db.pool, ws, &parent(&format!("    revision: {rev}\n"))).await;
        let src = child_source(&db.pool, p).await.expect("a child");
        assert!(src.contains(task), "revision {rev}, got {src}");
    }
}

#[tokio::test]
async fn no_revision_runs_the_latest() {
    let Some(db) = common::db().await else { return };
    let ws = seed(&db).await;
    let p = start(&db.pool, ws, &parent("")).await;
    let src = child_source(&db.pool, p).await.expect("a child");
    assert!(src.contains("id: three"), "the latest, got {src}");
}

#[tokio::test]
async fn a_missing_revision_spawns_nothing_and_fails_the_parent() {
    let Some(db) = common::db().await else { return };
    let ws = seed(&db).await;
    let p = start(&db.pool, ws, &parent("    revision: 9\n")).await;
    assert!(
        child_source(&db.pool, p).await.is_none(),
        "no child for a revision that does not exist"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(p)
        .fetch_one(&db.pool)
        .await
        .expect("parent state");
    assert_eq!(state, "FAILED");
}
