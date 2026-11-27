//! Package #12 Task 1: a `Subflow`'s `labels` + `inheritLabels` on the child
//! execution (Kestra `ExecutableUtils.subflowExecution` `:204-207`).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5442/egret_test
//!   cargo test -p egret-workflow --test subflow_labels

use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const CHILD: &str = "id: child\nnamespace: t\nlabels:\n  tier: gold\n  region: us\ntasks:\n  \
     - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

fn parent(extra: &str) -> String {
    format!(
        "id: parent\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n    \
         wait: false\n    labels:\n      team: x\n      bad: \"{{{{ nope.x }}}}\"\n{extra}"
    )
}

async fn file_flow(db: &Db, ws: Uuid, by: egret_core::UserId, source: &str) {
    let flow = egret_workflow::spec::parse(source).expect("the fixture flow parses");
    let input = egret_workflow::CreateFlowInput {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        source: source.to_string(),
        plan: serde_json::to_value(&flow).expect("a parsed flow serialises"),
        folder_id: None,
    };
    egret_workflow::FlowRepo::new(db.clone())
        .create(ws, by, &input)
        .await
        .expect("file the flow");
}

/// The child's stored own labels and its visible list, flow's labels first.
async fn child_labels(pool: &sqlx::PgPool, source: &str) -> (Option<Value>, Vec<(String, String)>) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(&db, ws, user, CHILD).await;
    let labels = vec![
        json!({"key": "env", "value": "prod"}),
        json!({"key": "region", "value": "eu"}),
        json!({"key": "system.from", "value": "api"}),
    ];
    let parent = egret_workflow::worker::start_execution_with_trigger_labels(
        pool,
        ws,
        None,
        source,
        json!({}),
        None,
        labels,
    )
    .await
    .expect("start the parent");
    let stored: Option<Value> = sqlx::query_scalar(
        "SELECT labels FROM workflow.executions WHERE trigger_execution_id = $1",
    )
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("one subflow child");
    // The child's stored list is its OWN; readers put the child flow's
    // `labels:` (tier, region) in front of it.
    let visible = stored
        .iter()
        .flat_map(|v| v.as_array().cloned().unwrap_or_default())
        .map(|l| {
            (
                l["key"].as_str().unwrap().to_string(),
                l["value"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    (stored, visible)
}

fn kv(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn inherit_labels_true_carries_the_parents_minus_the_childs_keys(pool: sqlx::PgPool) {
    // Act
    let (_, visible) = child_labels(&pool, &parent("    inheritLabels: true\n")).await;

    // Assert: the parent's (region is the child flow's, so dropped), then the
    // task's own; the unrenderable `bad` is omitted.
    assert_eq!(
        visible,
        kv(&[("env", "prod"), ("system.from", "api"), ("team", "x"),])
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn inherit_labels_false_carries_only_the_parents_system_labels(pool: sqlx::PgPool) {
    // Act: the default (absent) is false.
    let (stored, visible) = child_labels(&pool, &parent("")).await;

    // Assert
    assert_eq!(visible, kv(&[("system.from", "api"), ("team", "x")]));
    assert!(stored.is_some());
}
