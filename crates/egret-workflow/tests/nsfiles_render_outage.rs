//! W9b Task 4: a database outage during a namespace lookup at render is
//! egret's, not the author's (W9b spec §11). Requires DATABASE_URL.
//!
//! Its own test binary because the namespace-file connect options are
//! process-wide (`nsfiles::install_db`, set once): this binary installs
//! options that reach a closed port, while the worker's own pool -- claims,
//! outcomes, the execution -- uses the real database. So every namespace
//! lookup here fails to connect, and nothing else does.
//!
//! # It touches only what it created (fix round 1, I2 and M10)
//!
//! Each test process already gets a private database (`egret-test-db`,
//! cloned from the migrated template and dropped at exit), so what this test
//! leaves cannot reach another binary's `reap(ZERO) == 1` -- measured: run
//! together with `storage_render`, both pass. It is still written not to
//! depend on that:
//!
//! - **Its ticks claim only its own runs.** A claim is global over the
//!   database (`claim`'s `FOR UPDATE SKIP LOCKED`), so while it ticks another
//!   transaction holds a row lock on every OTHER claimable run, and
//!   `SKIP LOCKED` passes over them; that transaction is rolled back, so
//!   nothing of theirs is written.
//! - **It deletes what it made**, by its own execution ids, whatever the
//!   assertions answered, and never by `claimed_at = NULL` (which would hand a
//!   run to another worker).

mod common;

use egret_metastore::Db;
use egret_workflow::storage;
use egret_workflow::storage::nsfiles;
use egret_workflow::worker::{start_execution, tick, Handlers};
use serde_json::json;
use uuid::Uuid;

async fn attempts(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("attempts")
}

async fn state_of(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// A port nothing listens on: bound, then released.
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// One `tick` that can claim only runs of `mine`: every other claimable run
/// is row-locked by a transaction held across the tick (and rolled back, so
/// nothing of theirs changes). Returns the tick's answer and how many other
/// runs were held aside.
async fn tick_only(db: &Db, mine: &[Uuid]) -> (egret_core::Result<usize>, usize) {
    let mut aside = db.pool.begin().await.unwrap();
    let held: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs \
          WHERE type_id IS NOT NULL AND state = 'RUNNING' AND claimed_at IS NULL \
            AND execution_id <> ALL($1) \
          FOR UPDATE",
    )
    .bind(mine)
    .fetch_all(&mut *aside)
    .await
    .unwrap();
    let reg = Handlers::with_core_tasks(storage::current());
    // A guard, not a measurement: a tick that waited on a row held aside
    // (another execution's settle, through `wake`) would otherwise hang.
    let ticked = tokio::time::timeout(std::time::Duration::from_secs(60), tick(&db.pool, 16, &reg))
        .await
        .expect("the tick waited 60 s on a run held aside");
    aside.rollback().await.unwrap();
    (ticked, held.len())
}

/// The handler path: `r`'s `fileExists` cannot reach the database. Not
/// `false` (which a missing file answers) and not a failed task: the tick is
/// an `Err`, the run keeps its lease and records nothing, no attempt is
/// spent. Then the settle path: `b`'s `runIf` renders inside the transaction
/// that finishes `a`, and the outage rolls that transaction back whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_outage_during_a_namespace_read_re_claims_with_no_attempt_spent() {
    let Some(db) = common::db().await else {
        return;
    };
    let unreachable = (*db.pool.connect_options())
        .clone()
        .host("127.0.0.1")
        .port(closed_port());
    nsfiles::install_db(unreachable).expect("the first install in this binary");
    storage::install_mem_for_tests();
    let (ws, _) = common::seed_workspace(&db).await;

    let handler = "id: outage\nnamespace: w9b.outage\ntasks:\n\
         \x20 - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{ fileExists('cfg/x.txt') }}\"\n";
    let settle = "id: outage_settle\nnamespace: w9b.outage\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: a\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ fileExists('cfg/x.txt') }}\"\n    message: b\n";
    let ex1 = start_execution(&db.pool, ws, None, handler, json!({}))
        .await
        .unwrap();
    let ex2 = start_execution(&db.pool, ws, None, settle, json!({}))
        .await
        .unwrap();
    let mine = vec![ex1, ex2];

    // The assertions run as a task, so a failed one still reaches the cleanup.
    let checks = {
        let db = Db {
            pool: db.pool.clone(),
        };
        let mine = mine.clone();
        tokio::spawn(async move {
            // ex2's `a` is claimable too: hold it aside for the handler half.
            let before = attempts(&db.pool, ex1, "r").await;
            let (ticked, _) = tick_only(&db, &[ex1]).await;
            let err = ticked.expect_err("an outage is the worker's error, not the task's");
            assert!(
                err.to_string().contains("'/cfg/x.txt'") && err.to_string().contains("database"),
                "{err}"
            );
            let (state, outputs, message): (String, Option<serde_json::Value>, Option<String>) =
                sqlx::query_as(
                    "SELECT state, outputs, message FROM workflow.task_runs \
                      WHERE execution_id = $1 AND task_id = 'r'",
                )
                .bind(ex1)
                .fetch_one(&db.pool)
                .await
                .unwrap();
            assert_eq!((state.as_str(), outputs, message), ("RUNNING", None, None));
            assert_eq!(
                attempts(&db.pool, ex1, "r").await,
                before,
                "no attempt spent"
            );

            let before = attempts(&db.pool, ex2, "a").await;
            let (ticked, _) = tick_only(&db, &mine).await;
            let err = ticked.expect_err("an outage in settle is the worker's error");
            assert!(err.to_string().contains("'/cfg/x.txt'"), "{err}");
            assert_eq!(
                state_of(&db.pool, ex2, "a").await.as_deref(),
                Some("RUNNING"),
                "a's outcome rolled back"
            );
            assert_eq!(
                state_of(&db.pool, ex2, "b").await,
                None,
                "b was never created"
            );
            assert_eq!(
                attempts(&db.pool, ex2, "a").await,
                before,
                "no attempt spent"
            );
        })
    };
    let outcome = checks.await;

    // Cleanup, by this test's own execution ids: its two leased runs (`r`,
    // `a`) and nothing else. Never `claimed_at = NULL`, which would hand them
    // to another binary's worker.
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = ANY($1)")
            .bind(&mine)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let deleted = sqlx::query("DELETE FROM workflow.executions WHERE id = ANY($1)")
        .bind(&mine)
        .execute(&db.pool)
        .await
        .unwrap()
        .rows_affected();
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = ANY($1)")
            .bind(&mine)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
    assert_eq!(
        (runs, deleted, left),
        (2, 2, 0),
        "the cleanup removed exactly r and a"
    );
}
