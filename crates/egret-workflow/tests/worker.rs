//! The four durability properties the differential oracle can never see.
//!
//! `tests/executor.rs` compares execution traces against Kestra's own corpus.
//! It has no opinion about whether two workers claimed the same run, whether a
//! crash left half a transaction applied, or whether a lease expired on time —
//! so this file is the **only** instrument on any of that, and every property
//! here was written failing first for that reason.
//!
//! # Why `#[sqlx::test]` and not `common::db()`
//!
//! The rest of this crate's DB tests share one database per test process
//! (`egret-test-db`). The claim is **global** — a worker serves every workspace,
//! which is the whole point of `SKIP LOCKED` — so two of these tests running in
//! the same database would claim each other's runs and run each other's
//! handlers. `#[sqlx::test]` gives a scratch database per test, which is the
//! isolation this file actually needs, and it is what
//! `egret-automation`'s own scheduler tests use for the same statement.
//!
//! **`#[sqlx::test]` resolves `DATABASE_URL` through dotenvy unconditionally**,
//! so an unexported variable does not skip: it silently falls back to `.env`,
//! i.e. the *dev* database. Every test here calls
//! `egret_test_db::assert_database_url_is_exported()` first, which is that
//! crate's own guard for exactly this.

use std::sync::Mutex;

use egret_metastore::Db;
use egret_workflow::worker::{
    apply_outcome, claim, reap, start_execution, Outcome, Registry, TaskInput,
};
use uuid::Uuid;

mod common;

/// A one-task flow. `core.log.Log` is a `RunnableTask`, so its run is what a
/// worker claims; nothing here implements the handler (W3b Task 3 owns it) and
/// [`Recorder`] answers for it.
fn one_task(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

/// Two tasks in sequence: finishing `a` is what makes `advance()` create `b`,
/// so `b`'s row is "the successor" every atomicity assertion here is about.
fn two_tasks(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n  - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: bye\n"
    )
}

/// A [`Registry`] that records every task it was asked to run.
///
/// The recording is the instrument: three of the four properties assert on
/// **which runs were handed to a handler**, not on the absence of an error.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<String>>,
}

impl Recorder {
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Registry for Recorder {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.seen.lock().unwrap().push(task.task_id.clone());
        Ok(Outcome::success())
    }
}

async fn seed(pool: &sqlx::PgPool) -> (Db, Uuid) {
    let db = Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    (db, ws)
}

async fn run_state(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_optional(pool)
    .await
    .expect("read run state")
}

async fn execution_history(pool: &sqlx::PgPool, execution: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read execution history")
}

async fn lease_of(
    pool: &sqlx::PgPool,
    execution: Uuid,
    task_id: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar(
        "SELECT claimed_at FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read lease")
}

/// **Property 1: two concurrent claims never take the same run.**
///
/// Asserted on the **claimed ids** — disjoint, and their union is every
/// runnable run — rather than on the absence of an error, because a test that
/// only checks "no error" passes with `SKIP LOCKED` deleted. The 500 ms timeout
/// is the other half: plain `FOR UPDATE` also ends up returning the right rows
/// here, by BLOCKING until the first claimer commits, so an untimed assertion
/// proves only that the marker works. `egret-automation`'s
/// `two_concurrent_claimers_never_take_the_same_row` records the same pair of
/// reasons for the same statement.
#[sqlx::test(migrations = "../../migrations")]
async fn two_concurrent_claims_partition_the_runnable_set(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    let mut all = Vec::new();
    for i in 0..4 {
        let ex = start_execution(&pool, ws, None, &one_task(&format!("f{i}")))
            .await
            .expect("start execution");
        all.push(ex);
    }

    // The first claimer holds its transaction open, so its locks are still live
    // when the second runs — exactly the overlap two replicas produce.
    let mut tx = pool.begin().await.unwrap();
    let first = claim(&mut *tx, 2).await.expect("first claim");
    let first_ids: Vec<Uuid> = first.iter().map(|c| c.id).collect();
    assert_eq!(
        first_ids.len(),
        2,
        "the first claimer must take exactly its limit of 2 runnable runs, took {first_ids:?}"
    );

    let second = tokio::time::timeout(std::time::Duration::from_millis(500), claim(&pool, 10))
        .await
        .expect("a locked row must be SKIPPED immediately, not waited on")
        .expect("second claim");
    let second_ids: Vec<Uuid> = second.iter().map(|c| c.id).collect();
    assert_eq!(
        second_ids.len(),
        2,
        "the second claimer must take the other 2 runs, took {second_ids:?}"
    );

    let overlap: Vec<Uuid> = first_ids
        .iter()
        .copied()
        .filter(|id| second_ids.contains(id))
        .collect();
    assert!(
        overlap.is_empty(),
        "two claimers took the same run(s): {overlap:?}"
    );

    let mut union: Vec<Uuid> = first
        .iter()
        .chain(second.iter())
        .map(|c| c.execution_id)
        .collect();
    union.sort();
    union.dedup();
    let mut seeded = all.clone();
    seeded.sort();
    assert_eq!(
        union,
        seeded,
        "the two claims must partition every seeded execution's run: {} + {} claimed, \
         {} distinct, {} seeded",
        first_ids.len(),
        second_ids.len(),
        union.len(),
        seeded.len()
    );

    tx.commit().await.unwrap();

    // And once committed the marker keeps them out: a third claimer, holding no
    // lock and racing nobody, must find nothing.
    let third = claim(&pool, 10).await.expect("third claim");
    assert!(
        third.is_empty(),
        "a leased run must not be claimed again, got {:?}",
        third.iter().map(|c| c.task_id.clone()).collect::<Vec<_>>()
    );
}

/// **Property 2a: a tick killed between the handler and the commit applies
/// nothing.**
///
/// Driven by dropping the transaction, which is what a killed process does to
/// it. The interesting assertion is the second one — *the execution has not
/// moved* — so the test carries its own positive control: the SAME fixture,
/// committed, must move it. Without that control the "has not moved"
/// assertions pass against an `apply_outcome` that writes nothing at all.
///
/// It also pins the cost of the two-commit design that
/// `worker`'s module doc states: the run is **not** immediately claimable
/// again, because its lease is durable, and [`reap`] is what brings it back.
/// Both directions are asserted.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tick_killed_between_the_handler_and_the_commit_applies_nothing(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    // --- positive control: the same steps, committed. ---
    //
    // It runs FIRST and is driven to completion, so it leaves nothing
    // claimable behind. That is not tidiness: the claim is global, and the
    // first draft of this test left the control's successor claimable, so the
    // kill below claimed the CONTROL's run and every assertion about the
    // killed execution passed by never touching it. Each claim below asserts
    // WHICH execution it took for the same reason.
    let good = start_execution(&pool, ws, None, &two_tasks("good"))
        .await
        .expect("start good");
    let claimed = claim(&pool, 1).await.expect("claim good");
    assert_eq!(
        claimed.len(),
        1,
        "the one runnable run of `good` is claimable"
    );
    assert_eq!(
        claimed[0].execution_id, good,
        "the control claimed its own run"
    );
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &claimed[0], &Outcome::success())
        .await
        .expect("apply good");
    tx.commit().await.unwrap();
    assert_eq!(
        run_state(&pool, good, "a").await.as_deref(),
        Some("SUCCESS"),
        "the control must record the handler's result"
    );
    assert_eq!(
        run_state(&pool, good, "b").await.as_deref(),
        Some("RUNNING"),
        "the control must create and start the successor in the same commit"
    );
    // Drain the control's successor so the claim below can only find the
    // killed execution's run.
    let claimed = claim(&pool, 1).await.expect("claim good's successor");
    assert_eq!(claimed.len(), 1, "the control's successor is claimable");
    assert_eq!(
        claimed[0].task_id, "b",
        "only the control's successor is left to claim; a finished run must not \
         come back"
    );
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &claimed[0], &Outcome::success())
        .await
        .expect("apply good's successor");
    tx.commit().await.unwrap();
    assert!(
        claim(&pool, 10).await.expect("drained").is_empty(),
        "the control leaves nothing claimable"
    );

    // --- the kill. ---
    let dead = start_execution(&pool, ws, None, &two_tasks("dead"))
        .await
        .expect("start dead");
    let before = execution_history(&pool, dead).await;
    let claimed = claim(&pool, 1).await.expect("claim dead");
    assert_eq!(
        claimed.len(),
        1,
        "the one runnable run of `dead` is claimable"
    );
    assert_eq!(
        claimed[0].execution_id, dead,
        "the kill claimed the killed execution's run"
    );
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &claimed[0], &Outcome::success())
        .await
        .expect("apply dead");
    drop(tx); // the process dies here

    assert_eq!(
        run_state(&pool, dead, "a").await.as_deref(),
        Some("RUNNING"),
        "a dropped transaction must not record the handler's result"
    );
    assert_eq!(
        run_state(&pool, dead, "b").await,
        None,
        "a dropped transaction must not create the successor — this is the half-applied \
         execution the one-transaction rule exists to prevent"
    );
    assert_eq!(
        execution_history(&pool, dead).await,
        before,
        "a dropped transaction must not move the execution"
    );

    // The cost of committing the claim separately: the lease survives the kill,
    // so the run is NOT claimable again on its own …
    let leftover = claim(&pool, 10).await.expect("claim after the kill");
    assert!(
        leftover.is_empty(),
        "a leased run must stay leased after a killed tick, got {:?}",
        leftover
            .iter()
            .map(|c| c.task_id.clone())
            .collect::<Vec<_>>()
    );
    // … and `reap` is what makes it claimable again.
    let reclaimed = reap(&pool, std::time::Duration::ZERO)
        .await
        .expect("reap after the kill");
    assert_eq!(
        reclaimed, 1,
        "reap must take back the run the killed tick left leased"
    );
    let again = claim(&pool, 10).await.expect("claim after reap");
    assert_eq!(
        again.len(),
        1,
        "after reap the run must be claimable again, got {:?}",
        again.iter().map(|c| c.task_id.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        again[0].task_id, "a",
        "and it is the run the kill left behind"
    );
    assert_eq!(again[0].execution_id, dead);
}

/// **Property 2b: a failure partway through `apply_outcome` leaves the run
/// unfinished.**
///
/// 2a drops the transaction from outside `tick`. This one drives the real
/// [`tick`](egret_workflow::worker::tick) and makes the **successor's insert**
/// fail, which is the only way to observe that `tick` wraps the whole of
/// `apply_outcome` rather than letting its statements autocommit. With the
/// writes on autocommit the run is left `SUCCESS` while the successor does not
/// exist — a task finished with the execution not moved, which is exactly the
/// window the plan's Global Constraint forbids.
///
/// The fault is a CHECK constraint rather than a mock: it fails the one
/// statement that inserts `b`, at the real call site, with the real error path.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failure_partway_through_a_tick_leaves_nothing_half_applied(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &two_tasks("halt"))
        .await
        .expect("start execution");

    sqlx::query(
        "ALTER TABLE workflow.task_runs ADD CONSTRAINT wf_test_no_successor CHECK (task_id <> 'b')",
    )
    .execute(&pool)
    .await
    .expect("arm the fault");

    let reg = Recorder::default();
    let err = egret_workflow::worker::tick(&pool, 10, &reg).await;
    assert!(
        err.is_err(),
        "a tick whose successor cannot be written must fail, not report success"
    );
    assert_eq!(reg.seen(), vec!["a".to_string()], "the handler ran once");

    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("RUNNING"),
        "the run must not be left finished when its successor could not be written"
    );
    assert!(
        lease_of(&pool, ex, "a").await.is_some(),
        "a rolled-back tick must leave the lease in place for the reaper"
    );
}

/// **Property 3: `reap` reclaims exactly what the timeout says.**
///
/// Both directions in one test, because a one-directional test passes with the
/// predicate widened to everything: one lease past the timeout is reclaimed,
/// one inside it is left alone, and the second assertion is the one that fails
/// under that mutation.
#[sqlx::test(migrations = "../../migrations")]
async fn reap_reclaims_the_expired_lease_and_leaves_the_live_one(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let stale = start_execution(&pool, ws, None, &one_task("stale"))
        .await
        .expect("start stale");
    let fresh = start_execution(&pool, ws, None, &one_task("fresh"))
        .await
        .expect("start fresh");

    let claimed = claim(&pool, 10).await.expect("claim both");
    assert_eq!(claimed.len(), 2, "both runs are claimable");

    // Backdate one lease past the timeout. The other keeps the instant the
    // claim wrote.
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = now() - interval '2 hours' \
         WHERE execution_id = $1",
    )
    .bind(stale)
    .execute(&pool)
    .await
    .expect("backdate");

    let n = reap(&pool, std::time::Duration::from_secs(3600))
        .await
        .expect("reap");
    assert_eq!(n, 1, "exactly the one lease past the timeout is reclaimed");
    assert!(
        lease_of(&pool, stale, "a").await.is_none(),
        "the expired lease must be cleared"
    );
    assert!(
        lease_of(&pool, fresh, "a").await.is_some(),
        "a lease INSIDE the timeout must be left alone — this is the direction a \
         widened predicate breaks, and it is why a worker mid-handler is not \
         reclaimed out from under itself"
    );

    let after = claim(&pool, 10).await.expect("claim after reap");
    assert_eq!(
        after.iter().map(|c| c.execution_id).collect::<Vec<_>>(),
        vec![stale],
        "only the reclaimed run becomes claimable again"
    );
}

/// **Property 4 (found while writing the other three): a run whose result has
/// committed is never claimed again.**
///
/// The claim keys on `state = 'RUNNING'` as well as on the lease, and it has to:
/// a successful commit **clears** the lease, so `claimed_at IS NULL` is true of
/// a finished run and the lease alone would readmit it. A re-claim means the
/// handler runs a second time for one attempt — at-least-once turning into
/// duplicate execution — and the ratchet cannot see it, because a re-run that
/// produces the same terminal state produces an identical trace.
///
/// Asserted on the **handler's call log**, which is the only place a duplicate
/// run is visible at all.
#[sqlx::test(migrations = "../../migrations")]
async fn a_run_whose_result_committed_is_never_claimed_again(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &two_tasks("once"))
        .await
        .expect("start execution");

    let reg = Recorder::default();
    assert_eq!(
        egret_workflow::worker::tick(&pool, 10, &reg)
            .await
            .expect("tick 1"),
        1,
        "the first tick runs `a`"
    );
    assert_eq!(
        egret_workflow::worker::tick(&pool, 10, &reg)
            .await
            .expect("tick 2"),
        1,
        "the second tick runs the successor `b`"
    );
    assert_eq!(
        egret_workflow::worker::tick(&pool, 10, &reg)
            .await
            .expect("tick 3"),
        0,
        "nothing is left to claim"
    );

    assert_eq!(
        reg.seen(),
        vec!["a".to_string(), "b".to_string()],
        "each run's handler must be called exactly once"
    );
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("SUCCESS"));
    assert_eq!(run_state(&pool, ex, "b").await.as_deref(), Some("SUCCESS"));
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        "the execution reaches SUCCESS through the same states advance() emits"
    );

    // And the reaper cannot resurrect them either: a committed result holds no
    // lease, so there is nothing to reclaim however old the timeout is.
    assert_eq!(
        reap(&pool, std::time::Duration::ZERO).await.expect("reap"),
        0,
        "reap must not resurrect a run whose result committed"
    );
    let none = claim(&pool, 10).await.expect("claim after reap");
    assert!(
        none.is_empty(),
        "nothing is claimable once every run is terminal"
    );
}

/// Non-durability, but it is the assertion that makes every "the execution has
/// not moved" above mean something: a run and its successor, and the execution
/// state that goes with them, all land in the SAME commit.
#[sqlx::test(migrations = "../../migrations")]
async fn the_result_the_successor_and_the_execution_land_together(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &two_tasks("atomic"))
        .await
        .expect("start execution");

    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("RUNNING"),
        "start_execution creates and starts the first run"
    );
    assert_eq!(run_state(&pool, ex, "b").await, None, "and only the first");

    let claimed = claim(&pool, 1).await.expect("claim");
    assert_eq!(
        claimed.len(),
        1,
        "the one runnable run of `atomic` is claimable"
    );
    assert_eq!(claimed[0].type_id, "io.kestra.plugin.core.log.Log");
    assert_eq!(
        claimed[0].workspace_id, ws,
        "the claim carries the workspace a handler must be scoped to"
    );
    assert_eq!(claimed[0].attempt, 1, "the first attempt");

    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &claimed[0], &Outcome::failed())
        .await
        .expect("apply");
    tx.commit().await.unwrap();

    // A failed `a` means no `b` and a failed execution — the successor is
    // whatever `advance()` says it is, not "the next task in the list".
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("FAILED"),
        "the handler's result is what the run records"
    );
    assert_eq!(
        run_state(&pool, ex, "b").await,
        None,
        "a failed predecessor has no successor"
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"])
    );
    assert!(
        lease_of(&pool, ex, "a").await.is_none(),
        "committing the result releases the lease"
    );
}
