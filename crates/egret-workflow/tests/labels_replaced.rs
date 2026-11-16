//! Every reader and every copy of an execution's labels honours
//! `labels_replaced` (workflow package 7d, spec D6; plan corrections S6, S10).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test labels_replaced

use egret_metastore::Db;
use egret_workflow::worker::{Handlers, Registry};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}

async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId, common::App) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    (db, ws, user, app)
}

/// The flow row `source` is a draft of, created on first use (the
/// workspace's creator owns it).
async fn flow_of(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let parsed = egret_workflow::spec::parse(source).expect("parse");
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.flows WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3",
    )
    .bind(ws)
    .bind(&parsed.namespace)
    .bind(&parsed.id)
    .fetch_optional(pool)
    .await
    .expect("look up the flow");
    if let Some(id) = found {
        return id;
    }
    let owner: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .expect("workspace owner");
    let db = Db { pool: pool.clone() };
    common::seed_flow(&db, ws, egret_core::UserId(owner), source, "draft").await
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let flow = flow_of(pool, ws, source).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), source, json!({}))
        .await
        .expect("start execution")
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution state")
}

async fn run_state(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_optional(pool)
    .await
    .expect("run state")
}

async fn outputs_of(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("outputs")
}

fn handlers() -> Handlers {
    Handlers::with_core_tasks(egret_workflow::storage::current())
}

async fn settle_everything(pool: &sqlx::PgPool, reg: &dyn Registry) {
    for _ in 0..64 {
        let ran = egret_workflow::worker::tick(pool, 16, reg)
            .await
            .expect("tick");
        if ran == 0 {
            return;
        }
    }
    panic!("the loop never settled in 64 rounds");
}

const LABELLED: &str = "id: labelled\nnamespace: t\nlabels:\n  team: data\n  env: prod\ntasks:\n  \
     - id: r\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ labels.env }}\"\n";

/// `team` is the flow's; after a replace that dropped it, reading it fails the run.
const READS_TEAM: &str = "id: reads-team\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
     - id: r\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ labels.team }}\"\n";

/// What `POST …/labels` writes, written by hand before the first tick.
async fn replace(pool: &sqlx::PgPool, ex: Uuid, labels: Value) {
    sqlx::query("UPDATE workflow.executions SET labels = $2, labels_replaced = true WHERE id = $1")
        .bind(ex)
        .bind(labels)
        .execute(pool)
        .await
        .expect("replace the labels");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replaced_list_reaches_the_render_and_a_removed_flow_label_is_gone(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _user, _app) = setup(&pool).await;
    let env = start(&pool, ws, LABELLED).await;
    let team = start(&pool, ws, READS_TEAM).await;
    let control = start(&pool, ws, READS_TEAM).await;
    replace(&pool, env, json!([{"key": "env", "value": "dev"}])).await;
    replace(&pool, team, json!([{"key": "other", "value": "x"}])).await;

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(
        outputs_of(&pool, env, "r").await,
        Some(json!({"value": "dev"}))
    );
    assert_eq!(run_state(&pool, team, "r").await.as_deref(), Some("FAILED"));
    assert_eq!(
        outputs_of(&pool, control, "r").await,
        Some(json!({"value": "data"}))
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_detail_shows_the_replaced_list(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, LABELLED).await;
    replace(&pool, ex, json!([{"key": "env", "value": "dev"}])).await;
    let body = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{ex}"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(
        body["effective_labels"],
        json!([{"key": "env", "value": "dev"}])
    );
    assert_eq!(body["labels"], json!([{"key": "env", "value": "dev"}]));
}

/// A `Labels` task after an edit merges into the visible list and keeps
/// the removal (D6, S10): `team` does not come back, the flag stays.
#[sqlx::test(migrations = "../../migrations")]
async fn a_labels_task_after_an_edit_keeps_the_removal(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let src = "id: lt\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
               - id: first\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n  \
               - id: labels\n    type: io.kestra.plugin.core.execution.Labels\n    labels:\n      x: \"1\"\n";
    let ex = start(&pool, ws, src).await;
    replace(&pool, ex, json!([{"key": "env", "value": "dev"}])).await;
    settle_everything(&pool, &handlers()).await;
    let (labels, replaced): (Value, bool) =
        sqlx::query_as("SELECT labels, labels_replaced FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    assert_eq!(
        labels,
        json!([{"key": "env", "value": "dev"}, {"key": "x", "value": "1"}])
    );
    assert!(replaced);
}

/// Control for the test above: without an edit the task's write keeps the
/// flag false and the flow's label is in the stored list, as before 7d.
#[sqlx::test(migrations = "../../migrations")]
async fn a_labels_task_without_an_edit_is_unchanged(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let src = "id: lt\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
               - id: first\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n  \
               - id: labels\n    type: io.kestra.plugin.core.execution.Labels\n    labels:\n      x: \"1\"\n";
    let ex = start(&pool, ws, src).await;
    settle_everything(&pool, &handlers()).await;
    let (labels, replaced): (Value, bool) =
        sqlx::query_as("SELECT labels, labels_replaced FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    assert_eq!(
        labels,
        json!([{"key": "team", "value": "data"}, {"key": "x", "value": "1"}])
    );
    assert!(!replaced);
}

/// S6: a loop iteration of an edited root copies the flag.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_copies_the_flag(pool: sqlx::PgPool) {
    // Arrange: a `Log` first, so the loop is settled after the edit rather than
    // inside `start`; a loop whose body reads `labels.env`, which only the edit has.
    let (_db, ws, _user, _app) = setup(&pool).await;
    let src = "id: loopy-l\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
               - id: first\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n  \
               - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [\"1\", \"2\"]\n    \
               concurrencyLimit: 1\n    tasks:\n      \
               - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: \"{{ labels.env }}\"\n";
    let root = start(&pool, ws, src).await;
    replace(&pool, root, json!([{"key": "env", "value": "dev"}])).await;

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(
        exec_state(&pool, root).await,
        "SUCCESS",
        "the body rendered `labels.env`"
    );
    let iterations: Vec<(Value, bool)> = sqlx::query_as(
        "SELECT labels, labels_replaced FROM workflow.executions WHERE parent_execution_id = $1",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(iterations.len(), 2);
    for it in iterations {
        assert_eq!(it, (json!([{"key": "env", "value": "dev"}]), true));
    }
}

/// S6: a replay of an edited execution copies the flag (worker API).
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_copies_the_flag(pool: sqlx::PgPool) {
    let (_db, ws, user, _app) = setup(&pool).await;
    let ex = start(&pool, ws, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    replace(&pool, ex, json!([{"key": "env", "value": "dev"}])).await;
    let new = Uuid::new_v4();
    let replayed = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: ex,
            source: LABELLED,
            kind: egret_workflow::worker::ReplayKind::Whole,
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: user.0,
            id: new,
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");
    settle_everything(&pool, &handlers()).await;
    let replaced: bool =
        sqlx::query_scalar("SELECT labels_replaced FROM workflow.executions WHERE id = $1")
            .bind(replayed)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(replaced);
    assert_eq!(
        outputs_of(&pool, replayed, "r").await,
        Some(json!({"value": "dev"}))
    );
}
