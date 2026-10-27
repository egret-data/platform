//! **Which `Exit` shapes leave a run live for good** (package #2b, B5 fix
//! round 1, measured 2026-10-27), and what `workflow.execution_is_idle`
//! answers for them.
//!
//! A non-`KILLED` `Exit` ends the execution and leaves its siblings alone
//! (D5, `executor/exit.rs`). From then on `advance()` drives only the
//! `afterExecution:` phase (`executor/mod.rs`), and before package #2 it
//! drove nothing (`drives_forward`). A sibling LEAF already started is
//! finished by its worker. But a main-list run that needs the executor to
//! move stays as it is for good: a `CREATED` leaf is never started, and a
//! flowable is never closed even once every child has ended. Its execution
//! is never idle, so the sweeps and the pruner keep its files and
//! containers, and the logs route never says `complete`. This test pins the
//! measured population; a fix that closes such runs flips it on purpose.
//!
//! Deferred debt (B5 report, "Fix round 1"): the leak, not a loss.

use serde_json::json;
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";
const EXIT: &str = "io.kestra.plugin.core.execution.Exit";

fn shapes() -> Vec<(&'static str, String)> {
    vec![
        (
            "A parallel: leaf sibling + Exit",
            format!(
                "id: a\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: leaf\n        type: {LOG}\n        message: hi\n      - id: exit\n        type: {EXIT}\n        state: SUCCESS\n"
            ),
        ),
        (
            "B parallel: Sequential[b1,b2] + Exit",
            format!(
                "id: b\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: seq\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: b1\n            type: {LOG}\n            message: one\n          - id: b2\n            type: {LOG}\n            message: two\n      - id: exit\n        type: {EXIT}\n        state: SUCCESS\n"
            ),
        ),
        (
            "C Sequential[exit, c]",
            format!(
                "id: c\nnamespace: t\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: exit\n        type: {EXIT}\n        state: SUCCESS\n      - id: c\n        type: {LOG}\n        message: c\n"
            ),
        ),
        (
            "D If(true)[exit] then d",
            format!(
                "id: d\nnamespace: t\ntasks:\n  - id: if\n    type: io.kestra.plugin.core.flow.If\n    condition: \"true\"\n    then:\n      - id: exit\n        type: {EXIT}\n        state: SUCCESS\n  - id: d\n    type: {LOG}\n    message: d\n"
            ),
        ),
        (
            "E parallel: Sequential[b1,b2] + Sequential[x, exit]",
            format!(
                "id: e\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: seq\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: b1\n            type: {LOG}\n            message: one\n          - id: b2\n            type: {LOG}\n            message: two\n      - id: seq2\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: x\n            type: {LOG}\n            message: x\n          - id: exit\n            type: {EXIT}\n            state: FAILED\n"
            ),
        ),
        (
            "F parallel: Sequential[b1] + Exit",
            format!(
                "id: f\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: seq\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: b1\n            type: {LOG}\n            message: one\n      - id: exit\n        type: {EXIT}\n        state: WARNING\n"
            ),
        ),
    ]
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_exit_leaves_a_flowable_sibling_live_for_good_and_its_execution_never_idle(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let mut ids: Vec<(&str, Uuid)> = Vec::new();
    for (name, src) in shapes() {
        let ex = egret_workflow::worker::start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        ids.push((name, ex));
    }
    let reg = common::Answering(true);
    for _ in 0..40 {
        egret_workflow::worker::tick(&pool, 64, &reg)
            .await
            .expect("tick");
    }
    let mut measured = Vec::new();
    for (name, ex) in ids {
        let (state,): (String,) =
            sqlx::query_as("SELECT state FROM workflow.executions WHERE id = $1")
                .bind(ex)
                .fetch_one(&pool)
                .await
                .unwrap();
        let idle: Option<bool> = sqlx::query_scalar("SELECT workflow.execution_is_idle($1)")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
        let runs: Vec<(String, String)> = sqlx::query_as(
            "SELECT task_id, state FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
        )
        .bind(ex)
        .fetch_all(&pool)
        .await
        .unwrap();
        eprintln!("MEASURE {name}: execution {state}, idle {idle:?}, runs {runs:?}");
        let live: Vec<(String, String)> = runs
            .into_iter()
            .filter(|(_, s)| {
                ![
                    "SUCCESS",
                    "WARNING",
                    "FAILED",
                    "KILLED",
                    "CANCELLED",
                    "SKIPPED",
                ]
                .contains(&s.as_str())
            })
            .collect();
        measured.push((name, state, idle, live));
    }
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        measured,
        vec![
            (
                "A parallel: leaf sibling + Exit",
                "SUCCESS".into(),
                Some(true),
                vec![]
            ),
            (
                "B parallel: Sequential[b1,b2] + Exit",
                "SUCCESS".into(),
                Some(false),
                vec![s("seq", "RUNNING"), s("b1", "CREATED")]
            ),
            (
                "C Sequential[exit, c]",
                "SUCCESS".into(),
                Some(true),
                vec![]
            ),
            (
                "D If(true)[exit] then d",
                "SUCCESS".into(),
                Some(true),
                vec![]
            ),
            (
                "E parallel: Sequential[b1,b2] + Sequential[x, exit]",
                "FAILED".into(),
                Some(false),
                vec![s("seq", "RUNNING")]
            ),
            (
                "F parallel: Sequential[b1] + Exit",
                "WARNING".into(),
                Some(false),
                vec![s("seq", "RUNNING"), s("b1", "CREATED")]
            ),
        ]
    );
}
