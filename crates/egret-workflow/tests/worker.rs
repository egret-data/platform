//! The properties the differential oracle can never see: five durability ones,
//! one scheduling one, and one **ruling** — W4 Task 4's, that a handler result
//! is applied one per transaction. The seventh is the odd one out twice over:
//! the oracle CAN see its consequence (four entries that differ on the
//! happens-before relation alone), and it is the only test in the crate that
//! can tell the ruling from its alternative — measured, under a prototype that
//! applies a whole claim batch in one transaction at `driver::BATCH = 1`, it is
//! the sole failure and every other test in the crate passes, `tests/executor.rs`
//! included. *(That was **707** when W4 Task 4 measured it, from a per-suite sum
//! that is one larger now: Task 6 added a derivation test to `tests/executor.rs`.
//! The claim is "the only test that can tell the two disciplines apart", which
//! the total does not carry — so the total is not restated here, because a
//! figure nobody re-derives is a figure that goes stale, and this one already
//! had.)* See
//! [`one_result_per_transaction_is_the_ruling_and_this_is_the_step_it_costs`].
//!
//! `tests/executor.rs` compares execution traces against Kestra's own corpus.
//! It has no opinion about whether two workers claimed the same run, whether a
//! crash left half a transaction applied, or whether a lease expired on time,
//! or whether a worker whose lease was reaped can still commit — so this file
//! is the **only** instrument on any of that, and every property here was
//! written failing first for that reason.
//!
//! The fifth was added by the whole-branch review, which found that the first
//! four could not see it: the reap property reaps a run whose handler never
//! ran, and nothing called [`apply_outcome`] twice with one `Claimed`.
//!
//! **The sixth is not a durability property at all** — it is the `RETRYING`
//! waker, W4 Task 1, and the oracle cannot see it for a different reason than
//! the other five. It is not that the trace does not carry the fact; it is
//! that `oracle-exec/driver.rs`'s `pass_time` *rewinds*
//! `executions.created_at` to fake the delay, so no retry deadline ever
//! elapses in wall-clock time there and the waker's predicate is never
//! satisfied. Measured: instrumented to print every execution it selects, the
//! waker fires **zero** times across the whole 294-entry oracle run, with this
//! file as the positive control for the instrument. See
//! [`a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`].
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
    apply_outcome, claim, due_retries, reap, start_execution, start_execution_in, wake,
    NewExecution, Outcome, Registry, TaskInput,
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
    /// Nothing here has a budget, so nothing here can be killed. A `panic!`
    /// rather than `Ok(())`: this registry's whole instrument is *which runs
    /// were handed to a handler*, and a kill it silently absorbed would be a
    /// kill nobody could see.
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing in these fixtures declares a `timeout:`, so the kill \
                path must never reach {}",
            task.task_id
        );
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

async fn run_outputs(
    pool: &sqlx::PgPool,
    execution: Uuid,
    task_id: &str,
) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read run outputs")
}

async fn execution_history(pool: &sqlx::PgPool, execution: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read execution history")
}

/// The happens-before right endpoint migration 0136 adds — read here because
/// the fence's whole point is that a stale worker cannot wipe it.
async fn ended_seq(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT ended_seq FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read ended_seq")
}

/// The happens-before LEFT endpoint, read for the same reason `ended_seq` is:
/// [`one_result_per_transaction_is_the_ruling_and_this_is_the_step_it_costs`]
/// compares the two across two runs, and a helper per column keeps the query
/// that reads them from being the thing that drifts.
async fn created_seq(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT created_seq FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read created_seq")
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
        let ex = start_execution(
            &pool,
            ws,
            None,
            &one_task(&format!("f{i}")),
            serde_json::json!({}),
        )
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
    let good = start_execution(&pool, ws, None, &two_tasks("good"), serde_json::json!({}))
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
    let dead = start_execution(&pool, ws, None, &two_tasks("dead"), serde_json::json!({}))
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
    let ex = start_execution(&pool, ws, None, &two_tasks("halt"), serde_json::json!({}))
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
    let stale = start_execution(&pool, ws, None, &one_task("stale"), serde_json::json!({}))
        .await
        .expect("start stale");
    let fresh = start_execution(&pool, ws, None, &one_task("fresh"), serde_json::json!({}))
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
    let ex = start_execution(&pool, ws, None, &two_tasks("once"), serde_json::json!({}))
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

/// **Property 5: a worker whose lease was reaped must not be able to commit.**
///
/// This is the one the other four could not see, and the whole-branch review
/// found it by driving shipped code with no configuration at all.
///
/// [`reap`] clears `claimed_at` and leaves `state = 'RUNNING'`, which is
/// exactly the claim predicate, so the run is re-claimable **while the first
/// handler is still in flight** — `DEFAULT_WORKING_TIMEOUT` is an hour and
/// `core.flow.Sleep` takes an author-chosen duration, so `duration: PT2H` opens
/// the window with nothing switched on, and nothing can tell the first handler
/// to stop — W4 Task 3's kill path fires only on a task-level budget, inside
/// the worker that owns the run — so both handlers reach [`apply_outcome`].
/// `a_killed_handler_cannot_commit_over_the_worker_that_replaced_it` is the
/// same property driven through the shipping `tick`.
///
/// Measured on the shipped code before the fence existed, with the second
/// worker committing `SUCCESS` and the first then committing `FAILED`:
///
/// ```text
/// after B: state=SUCCESS  ended_seq=Some(4)  successor b = RUNNING
/// A's late apply_outcome returned: Ok
/// after A: state=FAILED   ended_seq=None     successor b still RUNNING
/// execution history: ["CREATED","RUNNING","FAILED"]
/// ```
///
/// Two terminals on one attempt; `ended_seq` — the happens-before endpoint —
/// wiped from `Some(4)` to NULL; and the execution FAILED while a successor is
/// `RUNNING` and claimable, which is the dual of the plan's Global Constraint:
/// *the execution moved and the task is not finished*.
///
/// **The lease is the fencing token.** `claimed_at` is `now()` at the instant
/// of the claim, it is returned on [`Claimed`], and every write
/// `apply_outcome` makes carries `AND claimed_at = $n`. A stale claimer
/// therefore matches no row, and **0 rows affected is an error rather than a
/// silent success** — the two are indistinguishable to `execute()`, which is
/// how a fence that is written but not read passes every test.
///
/// Both directions are asserted: the reaped worker is refused **and** the live
/// worker's own commit still works, because a fence that refused everybody
/// would pass the first half.
#[sqlx::test(migrations = "../../migrations")]
async fn a_worker_whose_lease_was_reaped_cannot_commit(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &two_tasks("fenced"), serde_json::json!({}))
        .await
        .expect("start execution");

    // Worker A claims `a` and is now "in its handler".
    let a = claim(&pool, 1).await.expect("claim A");
    assert_eq!(a.len(), 1, "the one runnable run is claimable");
    assert_eq!(a[0].task_id, "a", "worker A claimed `a`");

    // Its lease expires — `reap` clears `claimed_at` and leaves the run
    // RUNNING, which is the claim predicate again.
    assert_eq!(
        reap(&pool, std::time::Duration::ZERO)
            .await
            .expect("reap A's lease"),
        1,
        "the expired lease is reclaimed while A is still in its handler"
    );

    // Worker B claims the SAME run and commits first.
    let b = claim(&pool, 1).await.expect("claim B");
    assert_eq!(b.len(), 1, "the reaped run is claimable again");
    assert_eq!(b[0].id, a[0].id, "B took the run A is still running");
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &b[0], &Outcome::success())
        .await
        .expect("B's commit is the LIVE lease and must succeed");
    tx.commit().await.unwrap();

    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("SUCCESS"),
        "B's result is the one on the row"
    );
    let ended = ended_seq(&pool, ex, "a").await;
    assert!(
        ended.is_some(),
        "B's commit stamps the happens-before right endpoint"
    );
    assert_eq!(
        run_state(&pool, ex, "b").await.as_deref(),
        Some("RUNNING"),
        "and creates the successor"
    );

    // A now finishes its handler and tries to commit a DIFFERENT terminal,
    // carrying an output so "A's write did not land" is a fact about a value
    // and not about two NULLs.
    //
    // **It commits if `apply_outcome` lets it**, exactly as `tick` does. A test
    // that dropped the transaction unconditionally would satisfy every
    // post-state assertion below by the ROLLBACK, and would go on passing with
    // the fence deleted — the shape this package has found nine times.
    let late_outcome = Outcome {
        state: egret_workflow::executor::RunState::Failed,
        outputs: Some(serde_json::json!({ "from": "the reaped worker" })),
        message: None,
        files: Vec::new(),
    };
    let mut tx = pool.begin().await.unwrap();
    let late = apply_outcome(&mut tx, &a[0], &late_outcome).await;
    match &late {
        Ok(()) => tx.commit().await.expect("commit A"),
        Err(_) => drop(tx),
    }

    // Nothing A tried to write survived, in every field the review found moving.
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("SUCCESS"),
        "A's FAILED must not overwrite B's terminal"
    );
    assert_eq!(
        ended_seq(&pool, ex, "a").await,
        ended,
        "and `ended_seq` must not be wiped — the column 0136 exists to hold, \
         and the endpoint the oracle's happens-before relation is read from"
    );
    assert_eq!(
        run_outputs(&pool, ex, "a").await,
        None,
        "nor may A's outputs land: the `outputs` UPDATE is the FIRST write \
         `apply_outcome` makes, so a fence only on the state update would let \
         this one through and still pass every assertion above"
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING"]),
        "the execution must not reach FAILED while its successor is RUNNING — \
         that is the dual of the Global Constraint, and it is what shipped"
    );
    assert_eq!(
        run_state(&pool, ex, "b").await.as_deref(),
        Some("RUNNING"),
        "the successor B created is untouched"
    );

    // And it was refused rather than absorbed. The assertions above hold as
    // long as no ROW moved; this one is what says the worker was TOLD, which
    // is the difference between a fence that is read and one that is merely
    // written — 0 rows affected and 1 are the same value to `execute()`.
    let err = late.expect_err(
        "a worker whose lease was reaped must not be able to commit: it holds a \
         `Claimed` from a lease that no longer exists, and nothing else in this \
         module can tell it so",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("lease") && msg.contains(&a[0].id.to_string()),
        "the error must name the lost lease and the run it was on, so a \
         production log says which run was double-run rather than reporting a \
         row count: {msg}"
    );

    // The live half, so the fence is not simply refusing everybody: the
    // successor's own claim and commit still work.
    let c = claim(&pool, 1).await.expect("claim the successor");
    assert_eq!(c.len(), 1, "`b` is claimable");
    assert_eq!(c[0].task_id, "b");
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &c[0], &Outcome::success())
        .await
        .expect("a live lease commits");
    tx.commit().await.unwrap();
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        "and the execution finishes through the live worker"
    );
}

/// Non-durability, but it is the assertion that makes every "the execution has
/// not moved" above mean something: a run and its successor, and the execution
/// state that goes with them, all land in the SAME commit.
#[sqlx::test(migrations = "../../migrations")]
async fn the_result_the_successor_and_the_execution_land_together(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &two_tasks("atomic"), serde_json::json!({}))
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

// --------------------------------------------------------------------------
// W3b Task 3: the four handlers, driven through the real claim loop.
//
// `src/worker/handlers/*` carries the per-class unit tests -- the output shape
// of each handler, against the citations. These four are the OTHER half: that
// the registry is actually reachable from `tick`, that a handler's properties
// arrive from the execution's own frozen `source`, and that the two refusals
// this worker makes (no handler, no render) are refusals rather than states.
// --------------------------------------------------------------------------

use egret_workflow::worker::Handlers;

/// Drive `tick` until it claims nothing, and say how many runs it finished.
async fn drain(pool: &sqlx::PgPool, reg: &Handlers) -> usize {
    let mut total = 0;
    // Bounded: an unbounded loop over a worker that never converges is a hang,
    // and a hang in a test file reads as an infrastructure problem.
    for _ in 0..64 {
        let n = egret_workflow::worker::tick(pool, 8, reg)
            .await
            .expect("tick");
        if n == 0 {
            return total;
        }
        total += n;
    }
    panic!("tick never stopped claiming; {total} runs finished");
}

/// All four handlers, in one flow, through the real registry.
///
/// The assertion is the **execution's** history, not "no error": a `Fail` with
/// a falsy condition SUCCEEDS (`Fail.java:137`), so a flow that ends `SUCCESS`
/// is only possible if every one of the four ran and produced the right
/// terminal. A registry that answered `Outcome::success()` for everything
/// would pass this — which is why the truthy-condition test below is its pair.
#[sqlx::test(migrations = "../../migrations")]
async fn the_real_registry_runs_all_four_handlers_end_to_end(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: four\nnamespace: t\ntasks:\n\
         \x20 - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
         \x20 - id: r\n    type: io.kestra.plugin.core.debug.Return\n    format: hello\n\
         \x20 - id: f\n    type: io.kestra.plugin.core.execution.Fail\n    condition: \"false\"\n\
         \x20 - id: s\n    type: io.kestra.plugin.core.flow.Sleep\n    duration: PT0.001S\n";
    let ex = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution");

    let finished = drain(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(finished, 4, "one tick's work per task, four tasks");

    for id in ["l", "r", "f", "s"] {
        assert_eq!(
            run_state(&pool, ex, id).await.as_deref(),
            Some("SUCCESS"),
            "{id} must have succeeded"
        );
    }
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"])
    );
}

/// The pair to the test above, and what stops it passing against a registry
/// that says `SUCCESS` to everything: the SAME `Fail` type, with a truthy
/// condition, fails the execution.
#[sqlx::test(migrations = "../../migrations")]
async fn a_truthy_fail_fails_the_execution_through_the_real_handler(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: boom\nnamespace: t\ntasks:\n\
         \x20 - id: f\n    type: io.kestra.plugin.core.execution.Fail\n    condition: \"true\"\n\
         \x20 - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: never\n";
    let ex = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution");

    assert_eq!(
        drain(
            &pool,
            &Handlers::with_core_tasks(egret_workflow::storage::current())
        )
        .await,
        1
    );
    assert_eq!(run_state(&pool, ex, "f").await.as_deref(), Some("FAILED"));
    assert_eq!(
        run_state(&pool, ex, "after").await,
        None,
        "a failed predecessor has no successor"
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"])
    );
}

/// **The render surface, end to end through a real tick, and what it still
/// refuses.**
///
/// W3b Task 3 left this test asserting the opposite — *"with no render context,
/// `{{ inputs.who }}` cannot render"* — and said in as many words that the day
/// Task 5 filled the context it should fail and be deleted. It did not fail,
/// and that is worth recording: the flow it used declares no `inputs:` and
/// posts none, so `who` is undefined whether or not a context exists. **It
/// would have passed for the wrong reason** — an assertion whose output does
/// not depend on the thing it names — so it is replaced here rather than
/// deleted, by one that posts the input it reads.
///
/// Three things at once, deliberately, because each is the control for the
/// others:
///
/// 1. a **posted input** renders, and the value reaches the handler — asserted
///    on `Return`'s persisted `outputs`, which only a `Return` has, so this
///    cannot pass by reaching a different handler;
/// 2. a **task output** renders in a later task, which is the whole of what
///    `task_runs.outputs` is for;
/// 3. a name this worker does not carry — `trigger` — still FAILS the run. That
///    is the refusal that keeps the widening honest: a context that answered
///    everything would be an over-acceptance.
#[sqlx::test(migrations = "../../migrations")]
async fn the_render_surface_reaches_inputs_and_outputs_and_still_refuses_labels(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: surface\nnamespace: t\ninputs:\n  - id: who\n    type: STRING\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ inputs.who }}\"\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ outputs.a.value }}!\"\n";
    let ex = start_execution(&pool, ws, None, source, serde_json::json!({"who": "world"}))
        .await
        .expect("start execution");

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        (
            run_state(&pool, ex, "a").await,
            run_outputs(&pool, ex, "a").await
        ),
        (
            Some("SUCCESS".to_string()),
            Some(serde_json::json!({"value": "world"}))
        ),
        "the posted input reached the handler and its output was persisted"
    );
    assert_eq!(
        (
            run_state(&pool, ex, "b").await,
            run_outputs(&pool, ex, "b").await
        ),
        (
            Some("SUCCESS".to_string()),
            Some(serde_json::json!({"value": "world!"}))
        ),
        "`{{{{ outputs.a.value }}}}` resolved against the row `a` wrote in the \
         SAME transaction that finished it -- there is no other place it could \
         have come from"
    );

    // The refusal. `trigger` is a name four corpus flows write and this worker
    // does not carry, and an unrenderable property FAILS the run rather than
    // rendering to nothing. (`labels` was this test's refusal until the render
    // surface started carrying it -- which is exactly why the refusal has to be
    // a name the surface really lacks, and why this half is asserted at all.)
    let refused = "id: refused\nnamespace: t\ntasks:\n\
         \x20 - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: \"{{ trigger.executionId }}\"\n";
    let r = start_execution(&pool, ws, None, refused, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        run_state(&pool, r, "l").await.as_deref(),
        Some("FAILED"),
        "`trigger` is not carried, and a refusal is the safe direction -- the \
         alternative is a run that succeeded on a value Kestra never produced"
    );
}

/// **"No output" is `{}` and never an absent key, through the durable path.**
///
/// The W3b Task 3 brief said the opposite — that a `Log`, a `Sleep` and a falsy
/// `Fail` `return null` in Kestra so `outputs.<id>` would be *absent* — and
/// called the distinction load-bearing for exactly this module. Task 3
/// measured it on the artifact and it is the other way round: `{{ outputs.a }}`
/// printed `{}` for all three and for a format-less `Return`, `{{ outputs }}`
/// over two such tasks printed `{"a":{},"b":{}}`, and `{{ outputs.a.value }}`
/// FAILED the run in all four.
///
/// Both halves are asserted, because only the pair distinguishes the rule from
/// its two neighbours: omitting the key fails the first, and inventing a
/// `value` passes the second when it must not.
#[sqlx::test(migrations = "../../migrations")]
async fn a_task_that_produced_no_output_renders_as_an_empty_map_not_an_absent_key(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ok = "id: empty-ok\nnamespace: t\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ outputs.a }}\"\n";
    let e1 = start_execution(&pool, ws, None, ok, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        (
            run_state(&pool, e1, "b").await,
            run_outputs(&pool, e1, "b").await
        ),
        (
            Some("SUCCESS".to_string()),
            Some(serde_json::json!({"value": "{}"}))
        ),
        "a `Log` produces no Output object, and `{{{{ outputs.a }}}}` still \
         renders -- as the empty map, exactly as the artifact printed it"
    );

    let bad = "id: empty-bad\nnamespace: t\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ outputs.a.value }}\"\n";
    let e2 = start_execution(&pool, ws, None, bad, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        run_state(&pool, e2, "b").await.as_deref(),
        Some("FAILED"),
        "and `{{{{ outputs.a.value }}}}` FAILS -- the empty map has no `value`, \
         which is what stops the rule above from being 'answer anything'"
    );
}

/// A type nothing serves **aborts the tick** and leaves the run where it was.
///
/// Neither alternative is acceptable: a silent `SUCCESS` is the
/// over-acceptance shape spec §7 forbids, and a `FAILED` would report the
/// author's flow as wrong when it is egret that has no handler. `OutputValues`
/// is not a hypothetical — Task 1 measured it on 23 corpus entries, and 6 of
/// the 11 "at risk" entries are its.
#[sqlx::test(migrations = "../../migrations")]
async fn a_type_with_no_handler_aborts_the_tick(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: unhandled\nnamespace: t\ntasks:\n\
         \x20 - id: o\n    type: io.kestra.plugin.core.output.OutputValues\n    values:\n      k: v\n";
    let ex = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution");

    let err = egret_workflow::worker::tick(
        &pool,
        8,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await
    .expect_err("an unhandled type must not report a finished run");
    assert!(
        err.to_string()
            .contains("io.kestra.plugin.core.output.OutputValues"),
        "the refusal names the type, got {err}"
    );
    assert_eq!(
        run_state(&pool, ex, "o").await.as_deref(),
        Some("RUNNING"),
        "the execution has not moved"
    );
}

// ---------------------------------------------------------------------------
// W3b Task 4: `Command::SpawnExecution`.
// ---------------------------------------------------------------------------

/// A loop flow, parameterised so one shape serves every property below.
///
/// `body` is the loop's `tasks:` list, already indented six spaces.
fn loop_flow(
    id: &str,
    limit: Option<i32>,
    transmit: Option<bool>,
    values: &str,
    body: &str,
) -> String {
    let mut s = format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: loop\n    \
         type: io.kestra.plugin.core.flow.Loop\n    values: '{values}'\n"
    );
    if let Some(l) = limit {
        s.push_str(&format!("    concurrencyLimit: {l}\n"));
    }
    if let Some(t) = transmit {
        s.push_str(&format!("    transmitFailed: {t}\n"));
    }
    s.push_str("    tasks:\n");
    s.push_str(body);
    s
}

/// One `Log` as a loop body.
const LOG_BODY: &str =
    "      - id: item\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";
/// One always-failing `Fail` as a loop body.
const FAIL_BODY: &str = "      - id: item\n        type: io.kestra.plugin.core.execution.Fail\n";

/// One loop iteration as this schema stores it.
#[derive(Debug, PartialEq, Eq)]
struct SubExecution {
    index: i32,
    value: String,
    state: String,
}

/// Every sub-execution of one loop run, **ordered by `loop_index`**.
///
/// Deliberately not ordered by `created_at`: the whole point of the index is
/// that the instant cannot order these. `spawn_instants` below is what asserts
/// that it cannot.
async fn sub_executions(pool: &sqlx::PgPool, parent: Uuid, run_ix: i32) -> Vec<SubExecution> {
    sqlx::query_as::<_, (i32, String, String)>(
        "SELECT loop_index, loop_value, state FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix = $2 ORDER BY loop_index",
    )
    .bind(parent)
    .bind(run_ix)
    .fetch_all(pool)
    .await
    .expect("read sub-executions")
    .into_iter()
    .map(|(index, value, state)| SubExecution {
        index,
        value,
        state,
    })
    .collect()
}

/// The creation instants of one loop run's sub-executions.
async fn spawn_instants(
    pool: &sqlx::PgPool,
    parent: Uuid,
    run_ix: i32,
) -> Vec<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar(
        "SELECT created_at FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix = $2 ORDER BY loop_index",
    )
    .bind(parent)
    .bind(run_ix)
    .fetch_all(pool)
    .await
    .expect("read spawn instants")
}

/// Run every claimable task until nothing moves.
///
/// **W3b Task 5 deleted this function's other half.** Until Task 5 the worker
/// rendered nothing, so a `Loop` waited at `CREATED` and this file carried a
/// `render_pending_loops` that wrote `TaskRun::rendered` itself and called
/// `resume`. `worker::render` is now inside `settle`, so the rendering happens
/// in the same transaction as the round that needs it and a caller has nothing
/// to fill.
async fn settle_everything(pool: &sqlx::PgPool, reg: &Handlers) {
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

/// **A sub-execution inherits its parent's `inputs`, and the corpus can tell.**
///
/// The whole-branch review found this at a seam: W3b Task 4 wrote `spawn_one`'s
/// INSERT and W3b Task 5 added `executions.inputs` (migration 0136), so the
/// INSERT does not name the column and 0136's `DEFAULT '{}'` applies.
/// `render::Surface::load` reads `inputs` for the **current** execution only,
/// unlike `outputs`, which walks the `parent_execution_id` chain — so a loop
/// body reading `{{ inputs.x }}` rendered against nothing and branched the
/// other way.
///
/// It is corpus-reachable rather than hypothetical: `valids/finally-loop`
/// declares `failed: BOOL` defaulting `false`, `oracle-exec/corpus.json` posts
/// `true`, and the loop body is an `If` on `{{ inputs.failed == false }}`. It
/// could not be seen here because every fixture in this file builds its flows
/// with no `inputs:` at all.
///
/// **The `else` branch is the assertion.** A test whose body took the `then`
/// would pass with the column missing, since `{}` and a `false` input agree on
/// that branch — the same reason W3a's first circularity demonstration proved
/// nothing. So the posted input is `true`, the declared default is `false`, and
/// the body reaches the `else` only if the sub-execution got the POSTED value
/// rather than the default or nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_sub_execution_inherits_the_posted_inputs_of_its_parent(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    let source = "id: loop-inputs\nnamespace: t\n\
         inputs:\n  - id: failed\n    type: BOOL\n    defaults: false\n\
         tasks:\n  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    \
         values: '[\"1\"]'\n    concurrencyLimit: 0\n    tasks:\n      \
         - id: branch\n        type: io.kestra.plugin.core.flow.If\n        \
         condition: \"{{ inputs.failed == false }}\"\n        then:\n          \
         - id: took_then\n            type: io.kestra.plugin.core.log.Log\n            \
         message: then\n        else:\n          \
         - id: took_else\n            type: io.kestra.plugin.core.log.Log\n            \
         message: else\n";

    let parent = start_execution(&pool, ws, None, source, serde_json::json!({"failed": true}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let subs = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT inputs FROM workflow.executions WHERE parent_execution_id = $1",
    )
    .bind(parent)
    .fetch_all(&pool)
    .await
    .expect("read the sub-executions' inputs");
    assert_eq!(
        subs,
        vec![serde_json::json!({"failed": true})],
        "the one sub-execution carries the inputs POSTED to its parent, not \
         0136's `DEFAULT '{{}}'`"
    );

    let sub = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .expect("the sub-execution");
    assert_eq!(
        run_state(&pool, sub, "took_else").await.as_deref(),
        Some("SUCCESS"),
        "`{{{{ inputs.failed == false }}}}` is FALSE for the posted `true`, so \
         the body takes the `else`. With the inputs missing it renders against \
         `{{}}`, the declared default `false` applies, and the `then` runs \
         instead -- which is `valids/finally-loop` reproduced"
    );
    assert_eq!(
        run_state(&pool, sub, "took_then").await,
        None,
        "and the `then` branch is never created -- asserted so the test cannot \
         pass by running BOTH"
    );
}

/// **The brief's property: a `Loop` over two values spawns two executions,
/// ordered by `loopRun.index` and NOT by the creation instant.**
///
/// The second half is asserted directly rather than implied. All the
/// sub-executions of one loop run are written in **one transaction**, so
/// Postgres's `now()` is the same for every one of them and the instant cannot
/// order them at all — which is exactly what the artifact does: measured over
/// `oracle-exec/traces.json`, all 670 loop sub-executions carry their PARENT
/// execution's `startDate`, and all 220 multi-sibling groups share one instant
/// between them. `oracle-exec/normalise.py` records the consequence — ordering
/// by `startDate` made nine corpus flows fail the five-round stability check.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_over_two_values_spawns_two_sub_executions_ordered_by_index(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow("two", Some(0), None, r#"["a", "b"]"#, LOG_BODY);
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    // **The render happens inside `start_execution`'s own `settle`.** Before
    // W3b Task 5 this read `Vec::new()` and the test had to call a caller-side
    // renderer to get any further; `worker::render` is now a step of
    // `claim::drive`, between two rounds of `advance()`, so a literal `values:`
    // is resolved and its first batch spawned in the transaction that started
    // the execution.
    let spawned = sub_executions(&pool, ex, 0).await;
    assert_eq!(
        spawned
            .iter()
            .map(|s| (s.index, s.value.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "a"), (1, "b")],
        "two values, two sub-executions, indexed 0 and 1 in `values:` order"
    );

    let instants = spawn_instants(&pool, ex, 0).await;
    assert_eq!(instants.len(), 2);
    assert_eq!(
        instants[0], instants[1],
        "both iterations are written in ONE transaction, so the creation \
         instant is identical and cannot order them -- ordering `spawned` by \
         the instant is not unstable, it is undefined. `loop_index` is the \
         only thing that orders these, which is why \
         `workflow_executions_loop_run_idx` makes two iterations sharing one \
         index unrepresentable. Migration 0135 created that index; migration \
         0138 owns it now -- same name, plus `NULLS NOT DISTINCT`."
    );

    // And each sub-execution really runs the loop's body, in its own right.
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    let done = sub_executions(&pool, ex, 0).await;
    assert_eq!(
        done.iter().map(|s| s.state.as_str()).collect::<Vec<_>>(),
        vec!["SUCCESS", "SUCCESS"]
    );
    assert_eq!(
        run_state(&pool, ex, "loop").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"])
    );
}

/// **`concurrencyLimit` defaults to 1, and 1 is full serialisation.**
///
/// `advance()` emits the first batch and cannot emit a second, so every
/// iteration after the first is this module's. The measurement that settles
/// the default is Kestra's own traces rather than either schema:
/// `valids/loop-serial` has three values, no `concurrencyLimit:`, and the
/// artifact ran its three iterations one at a time.
#[sqlx::test(migrations = "../../migrations")]
async fn the_default_concurrency_limit_runs_one_iteration_at_a_time(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow("serial", None, None, r#"["a", "b", "c"]"#, LOG_BODY);
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    let first = sub_executions(&pool, ex, 0).await;
    assert_eq!(
        first.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0],
        "the default limit is 1, so the first batch is ONE iteration -- a \
         default of 0 (unlimited) would have spawned all three here"
    );

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        sub_executions(&pool, ex, 0).await,
        vec![
            SubExecution {
                index: 0,
                value: "a".into(),
                state: "SUCCESS".into()
            },
            SubExecution {
                index: 1,
                value: "b".into(),
                state: "SUCCESS".into()
            },
            SubExecution {
                index: 2,
                value: "c".into(),
                state: "SUCCESS".into()
            },
        ],
        "and the other two are started one at a time as each terminates"
    );
    assert_eq!(
        run_state(&pool, ex, "loop").await.as_deref(),
        Some("SUCCESS")
    );
}

/// **`concurrencyLimit: 0` is unlimited, not zero.**
///
/// The control for the test above, and the reason it is worth its own test:
/// `0` read as a cap of zero spawns nothing and hangs every unlimited loop in
/// the corpus, and that mistake is invisible in a flow whose limit is
/// defaulted.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unlimited_concurrency_limit_spawns_every_value_at_once(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow("unlimited", Some(0), None, r#"["a", "b", "c"]"#, LOG_BODY);
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    assert_eq!(
        sub_executions(&pool, ex, 0).await.len(),
        3,
        "`concurrencyLimit: 0` means no limit, so the first batch is every value"
    );
}

/// **`transmitFailed` (default true): the loop ends on the FIRST failure.**
///
/// `valids/loop-failed` measured on the artifact: three values, a body that
/// always fails, and Kestra ran **one** sub-execution — the loop terminated
/// before the second was started. That is the one branch of the continuation a
/// count of sub-executions can see, and it is why "fewer iterations than
/// values" is evidence about `transmitFailed` rather than about the limit.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failing_iteration_ends_a_transmit_failed_loop_at_once(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow("boom", None, None, r#"["a", "b", "c"]"#, FAIL_BODY);
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        sub_executions(&pool, ex, 0).await,
        vec![SubExecution {
            index: 0,
            value: "a".into(),
            state: "FAILED".into()
        }],
        "one iteration failed and `transmitFailed` defaults to true, so the \
         other two values are never started"
    );
    assert_eq!(
        run_state(&pool, ex, "loop").await.as_deref(),
        Some("FAILED")
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"])
    );
}

/// **`transmitFailed: false`: every iteration runs and the loop SUCCEEDS.**
///
/// The pair to the test above, and what makes it mean something: the same
/// body, the same values, the same default limit, one flag different, and the
/// artifact's answer is three FAILED sub-executions under a **SUCCESSFUL**
/// loop run (`valids/loop-failed-no-transmit`). A spawner that stopped at the
/// first failure whatever the flag said passes the test above and fails this
/// one.
#[sqlx::test(migrations = "../../migrations")]
async fn transmit_failed_false_runs_every_iteration_and_the_loop_succeeds(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow(
        "no-transmit",
        None,
        Some(false),
        r#"["a", "b", "c"]"#,
        FAIL_BODY,
    );
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        sub_executions(&pool, ex, 0).await,
        vec![
            SubExecution {
                index: 0,
                value: "a".into(),
                state: "FAILED".into()
            },
            SubExecution {
                index: 1,
                value: "b".into(),
                state: "FAILED".into()
            },
            SubExecution {
                index: 2,
                value: "c".into(),
                state: "FAILED".into()
            },
        ],
    );
    assert_eq!(
        run_state(&pool, ex, "loop").await.as_deref(),
        Some("SUCCESS"),
        "Kestra's loop terminates SUCCESS once every iteration has terminated, \
         whatever they terminated AS, unless `transmitFailed` short-circuited it"
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"])
    );
}

/// **A loop body that is itself a `Loop` spawns from inside a sub-execution.**
///
/// `valids/loop-nested` is this shape and it is the reason the sub-execution's
/// terminal has to walk **up**: the innermost iteration finishing is what
/// finishes the inner loop run, which finishes its sub-execution, which
/// finishes the outer loop run. Two levels, 2x2 iterations, and the assertion
/// is on the whole tree rather than on the count.
#[sqlx::test(migrations = "../../migrations")]
async fn a_nested_loop_spawns_from_inside_a_sub_execution(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let inner = "      - id: inner\n        type: io.kestra.plugin.core.flow.Loop\n        \
                 values: '[\"x\", \"y\"]'\n        concurrencyLimit: 0\n        tasks:\n\
                 \x20         - id: item\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n";
    let source = loop_flow("nested", Some(0), None, r#"["a", "b"]"#, inner);
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let outer = sub_executions(&pool, ex, 0).await;
    assert_eq!(
        outer,
        vec![
            SubExecution {
                index: 0,
                value: "a".into(),
                state: "SUCCESS".into()
            },
            SubExecution {
                index: 1,
                value: "b".into(),
                state: "SUCCESS".into()
            },
        ],
    );
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 ORDER BY loop_index",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .expect("read outer sub-executions");
    for id in &ids {
        assert_eq!(
            sub_executions(&pool, *id, 0).await,
            vec![
                SubExecution {
                    index: 0,
                    value: "x".into(),
                    state: "SUCCESS".into()
                },
                SubExecution {
                    index: 1,
                    value: "y".into(),
                    state: "SUCCESS".into()
                },
            ],
            "each outer iteration runs the inner loop's own two iterations"
        );
    }
    assert_eq!(
        run_state(&pool, ex, "loop").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"])
    );
}

/// **A sub-execution is a task run of nobody: the loop body never gets a run
/// in the parent.**
///
/// The control for every count above. Measured over the whole golden: across
/// the 39 comparable entries holding a `Loop`, not one of the 82 tasks inside
/// a loop body has a run in its parent — on either side. A spawner that ALSO
/// created the body's runs in the parent would pass every assertion above and
/// produce a trace Kestra never produces.
#[sqlx::test(migrations = "../../migrations")]
async fn no_run_for_a_loop_body_task_is_ever_created_in_the_parent(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow("bodyless", Some(0), None, r#"["a", "b"]"#, LOG_BODY);
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let parent_runs: Vec<String> = sqlx::query_scalar(
        "SELECT task_id FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .expect("read parent runs");
    assert_eq!(
        parent_runs,
        vec!["loop".to_string()],
        "the parent holds the `Loop`'s own run and nothing else"
    );

    // The positive control: the body task DID run, twice, in the two
    // sub-executions. Without this the assertion above passes against a
    // spawner that created no sub-execution at all.
    let body_runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs r \
           JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE e.parent_execution_id = $1 AND r.task_id = 'item' AND r.state = 'SUCCESS'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("read body runs");
    assert_eq!(
        body_runs, 2,
        "the body ran once per iteration, in its own execution"
    );
}

/// **A `transmitFailed` loop with several iterations in flight terminates
/// ONCE.**
///
/// `advance()` emits three spawns at once here, so all three fail and all
/// three report their terminal to the same loop run. The first ends it; the
/// other two must find it already ended and do nothing. Without that check
/// each of them applies a second `FAILED` and the run's history grows a
/// duplicate — which is a state Kestra never records and which
/// `oracle-exec/golden.json` compares element by element.
///
/// No corpus entry has this shape (`valids/loop-failed` is limit 1, so only
/// one iteration ever exists), which is exactly why it needs a test rather
/// than an entry.
#[sqlx::test(migrations = "../../migrations")]
async fn a_transmit_failed_loop_with_several_iterations_in_flight_terminates_once(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = loop_flow(
        "boom-parallel",
        Some(0),
        None,
        r#"["a", "b", "c"]"#,
        FAIL_BODY,
    );
    let ex = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");

    assert_eq!(
        sub_executions(&pool, ex, 0).await.len(),
        3,
        "unlimited: all three iterations are in flight before any of them fails"
    );

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    let history: serde_json::Value = sqlx::query_scalar(
        "SELECT history FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'loop'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("read the loop run's history");
    assert_eq!(
        history,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
        "three iterations reported a failure to one loop run; it must carry ONE \
         FAILED, not one per sibling"
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"])
    );
}

// --------------------------------------------------------------------------
// W3b Task 7 fix round: the claim's order
// --------------------------------------------------------------------------

/// Replace an execution's runs with `n` claimable siblings sharing ONE
/// `created_at`, which is what a real `settle` writes: `now()` in Postgres is
/// TRANSACTION time, so every row one round creates carries the same instant.
async fn siblings_sharing_one_instant(pool: &sqlx::PgPool, execution: Uuid, n: i32) -> Vec<Uuid> {
    sqlx::query("DELETE FROM workflow.task_runs WHERE execution_id = $1")
        .bind(execution)
        .execute(pool)
        .await
        .expect("clear the seeded runs");
    let mut ids = Vec::new();
    for run_ix in 0..n {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO workflow.task_runs \
               (execution_id, run_ix, task_ix, task_id, type_id, state, history, \
                created_at, updated_at) \
             VALUES ($1, $2, 0, $3, 'io.kestra.plugin.core.log.Log', 'RUNNING', \
                     '[\"CREATED\",\"SUBMITTED\",\"RUNNING\"]'::jsonb, \
                     TIMESTAMPTZ '2026-01-01 00:00:00+00', \
                     TIMESTAMPTZ '2026-01-01 00:00:00+00') \
             RETURNING id",
        )
        .bind(execution)
        .bind(run_ix)
        .bind(format!("t{run_ix}"))
        .fetch_one(pool)
        .await
        .expect("insert a sibling run");
        ids.push(id);
    }
    ids
}

/// Move rows to the end of the heap, in the order given.
///
/// An `UPDATE` writes a NEW tuple version and leaves the old one dead, so the
/// physical order a sequential scan returns follows the order rows were last
/// updated. This is not a contrivance: `apply_outcome`, the render pass and
/// `reap` all update these rows, so in production the physical order drifts
/// away from the creation order as a matter of course.
async fn touch_in_order(pool: &sqlx::PgPool, ids: &[Uuid]) {
    for id in ids {
        sqlx::query("UPDATE workflow.task_runs SET updated_at = updated_at WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("rewrite the row");
    }
}

/// **The claim's order must not depend on where Postgres happens to keep a
/// row** — W3b Task 7's fix round.
///
/// `ORDER BY created_at` alone is not an order over sibling runs: one `settle`
/// writes them all in one transaction, so they share `now()` to the microsecond
/// and the sort has nothing left to compare. What came back was then whatever
/// the scan produced, and since W3b Task 5 that choice is **observable**: the
/// round in which a run terminates is its `ended_seq`, which is one endpoint of
/// the happens-before relation the oracle compares. W3b Task 2 recorded the
/// missing tiebreak as benign; it was benign until Task 5 made termination
/// order part of the answer.
///
/// Measured before the fix: `executor_matches_golden` failed about **one run in
/// eight** under load, in *both* directions — `valids/parallel-nested`
/// regressing on five ordered pairs and `valids/parallel-fail-with-flowable`
/// matching its way out of the baseline.
///
/// **Both directions are driven**, because a claim that simply reversed would
/// pass a one-direction test: one execution's rows are pushed to the end of the
/// heap in descending `run_ix` and the other's in ascending, and both must come
/// back ascending. The descending arm is the one that fails without the
/// tiebreak; the ascending arm is what stops the test passing against a claim
/// that sorts backwards.
#[sqlx::test(migrations = "../../migrations")]
async fn the_claim_takes_runs_in_run_ix_order_whatever_their_physical_order(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    let a = start_execution(
        &pool,
        ws,
        None,
        &one_task("shuffled"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution a");
    let b = start_execution(
        &pool,
        ws,
        None,
        &one_task("in-order"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution b");

    let a_ids = siblings_sharing_one_instant(&pool, a, 6).await;
    let b_ids = siblings_sharing_one_instant(&pool, b, 6).await;

    let mut descending: Vec<Uuid> = a_ids.clone();
    descending.reverse();
    touch_in_order(&pool, &descending).await;
    touch_in_order(&pool, &b_ids).await;

    let taken = claim(&pool, 100).await.expect("claim every runnable run");
    assert_eq!(
        taken.len(),
        12,
        "both executions' runs must be claimable, or this test is measuring the \
         predicate rather than the order"
    );

    let order_of = |execution: Uuid| -> Vec<i32> {
        taken
            .iter()
            .filter(|c| c.execution_id == execution)
            .map(|c| c.run_ix)
            .collect()
    };
    assert_eq!(
        order_of(a),
        vec![0, 1, 2, 3, 4, 5],
        "the run rewritten LAST must not be claimed FIRST: sibling runs share one \
         `created_at`, so without a tiebreak the claim returns whatever the scan \
         produced. `run_ix` is the executor's own creation order and the order \
         the oracle compares"
    );
    assert_eq!(
        order_of(b),
        vec![0, 1, 2, 3, 4, 5],
        "and the control, whose physical order already matches: a claim that \
         sorted backwards would satisfy the assertion above and fail this one"
    );

    // The other half, and the one the oracle's driver actually takes: at
    // `LIMIT 1` the question is not what order the batch comes back in but
    // WHICH single row is taken, and that is decided by the subquery. Both
    // executions' rows share one `created_at` here, so the answer is the lowest
    // `(execution_id, run_ix)` — derived from the ids this test inserted rather
    // than assumed, because `execution_id` is a random uuid and naming one
    // would make the assertion a coin toss.
    sqlx::query("UPDATE workflow.task_runs SET claimed_at = NULL")
        .execute(&pool)
        .await
        .expect("release the leases taken above");
    touch_in_order(&pool, &descending).await;

    let first_execution = std::cmp::min(a, b);
    let mut one_at_a_time = Vec::new();
    for _ in 0..3 {
        let c = claim(&pool, 1).await.expect("claim one");
        assert_eq!(c.len(), 1, "one run per claim");
        one_at_a_time.push((c[0].execution_id, c[0].run_ix));
    }
    assert_eq!(
        one_at_a_time,
        vec![
            (first_execution, 0),
            (first_execution, 1),
            (first_execution, 2)
        ],
        "at `LIMIT 1` the subquery's order IS the answer, and the oracle's driver \
         ticks one run at a time (`driver::BATCH == 1`). Without the tiebreak \
         this is the choice that made `executor_matches_golden` return a \
         different verdict on the same tree"
    );
}

/// A one-task flow whose only task carries a `retry:` with a non-zero
/// `interval:` — and **no sibling**, which is the condition that makes the
/// stall reachable: with a second live task, a `settle` for that task would
/// carry a fresh `now` past the deadline and `retry::reopen_due` would fire
/// inside it by accident.
fn one_task_with_retry(id: &str, interval: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    retry:\n      type: constant\n      interval: {interval}\n\
         \x20     maxAttempts: 2\n"
    )
}

/// Fails every run's **first** attempt and succeeds on every later one.
///
/// The failure is what makes `advance()` schedule the retry; the success on
/// attempt 2 is what makes "the run moved" observable as a state rather than
/// as the absence of an error.
#[derive(Default)]
struct FailsOnce {
    seen: Mutex<Vec<(String, usize)>>,
}

impl FailsOnce {
    fn seen(&self) -> Vec<(String, usize)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Registry for FailsOnce {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.seen
            .lock()
            .unwrap()
            .push((task.task_id.clone(), task.attempt));
        match task.attempt {
            1 => Ok(Outcome::failed_with("the first attempt always fails")),
            _ => Ok(Outcome::success()),
        }
    }
    /// A handler that FAILED on its own is not an expiry, so the retry
    /// property must never see a kill. Same `panic!` as [`Recorder`]'s, and it
    /// is what says a `Failed` outcome is not mistaken for a timed-out one.
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "a handler that returned its own failure must not be killed: {}",
            task.task_id
        );
    }
}

/// How many attempts a run has recorded.
async fn attempts_of(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read attempts")
}

/// The deadline a `Command::ScheduleRetry` wrote, in `Tick` milliseconds.
async fn not_before_of(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT not_before_ms FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read not_before_ms")
}

async fn run_id_of(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(execution)
        .bind(task_id)
        .fetch_one(pool)
        .await
        .expect("read run id")
}

/// Everything about a stuck run, in one string, so an assertion can say **what**
/// is stuck rather than that a count was zero.
async fn stuck(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> String {
    format!(
        "run {} (execution {execution}, task `{task_id}`) is {:?} with {} attempt(s) \
         and not_before_ms {:?}; the execution is {:?}",
        run_id_of(pool, execution, task_id).await,
        run_state(pool, execution, task_id).await,
        attempts_of(pool, execution, task_id).await,
        not_before_of(pool, execution, task_id).await,
        execution_history(pool, execution)
            .await
            .as_array()
            .and_then(|h| h.last().cloned()),
    )
}

/// Drive `tick` until it claims nothing.
async fn drain_with(pool: &sqlx::PgPool, reg: &dyn Registry) -> usize {
    let mut total = 0;
    for _ in 0..64 {
        let n = egret_workflow::worker::tick(pool, 8, reg)
            .await
            .expect("tick");
        if n == 0 {
            return total;
        }
        total += n;
    }
    panic!("tick never stopped claiming; {total} runs finished");
}

/// **Property 6: a retry whose delay has elapsed is woken, and one whose delay
/// has not is left alone.**
///
/// This is the hole W3b handed to W4, and it is a *scheduling* property rather
/// than a durability one: a run in `RETRYING` with no concurrently-live sibling
/// had nothing in this worker that would ever look at its deadline again. The
/// claim takes only `RUNNING`, [`reap`] takes only `RUNNING`,
/// `retry::reopen_due` fires only inside a `settle` whose `now` is fixed for
/// the transaction, and `resume`'s only caller in the tree was the differential
/// oracle's driver — which fakes the delay by rewinding
/// `executions.created_at`.
///
/// **Nothing here rewinds anything.** The delay elapses in real time, which is
/// what makes this the shipping worker's problem rather than the driver's.
///
/// **It asserts what is stuck, not that nothing happened**, because "no rows
/// claimed" is also what a correct idle loop returns: a test that asserted a
/// tick returned zero would pass with the bug present *and* with it fixed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = FailsOnce::default();

    let due = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_retry("due", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start due");
    let waiting = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_retry("waiting", "PT30S"),
        serde_json::json!({}),
    )
    .await
    .expect("start waiting");

    // Both first attempts fail, so both runs take `RETRYING` and a deadline.
    assert_eq!(
        drain_with(&pool, &reg).await,
        2,
        "both first attempts must have run: {:?}",
        reg.seen()
    );
    for (label, execution, interval_ms) in [("due", due, 1_000), ("waiting", waiting, 30_000)] {
        assert_eq!(
            run_state(&pool, execution, "a").await.as_deref(),
            Some("RETRYING"),
            "the {label} run must be RETRYING before anything is measured: {}",
            stuck(&pool, execution, "a").await
        );
        let deadline = not_before_of(&pool, execution, "a")
            .await
            .expect("a retry was scheduled");
        assert!(
            deadline >= interval_ms,
            "the {label} run's deadline must be at least its own interval past \
             the execution's origin: {}",
            stuck(&pool, execution, "a").await
        );
    }

    // The delay passes in REAL time. `executions.created_at` is untouched.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    // The waker names the execution whose deadline has passed **by id**, and
    // no other. A count would not say which one moved, and an entry that moved
    // for an unattributed reason is a coincidence rather than a wake.
    let woken = wake(&pool, 8).await.expect("wake");
    assert_eq!(
        woken,
        vec![due],
        "the waker must name the execution whose retry came due and no other. \
         due={due} (interval PT1S, elapsed); waiting={waiting} (interval PT30S, \
         not elapsed). It named {woken:?}"
    );

    let finished = drain_with(&pool, &reg).await;

    assert_eq!(
        run_state(&pool, due, "a").await.as_deref(),
        Some("SUCCESS"),
        "the retry's delay has elapsed and nothing else is alive in this \
         execution, so the worker itself has to reopen it. Instead: {}\n\
         handler calls: {:?}",
        stuck(&pool, due, "a").await,
        reg.seen()
    );
    assert_eq!(
        attempts_of(&pool, due, "a").await,
        2,
        "the woken run must open a SECOND attempt rather than reuse the first: {}",
        stuck(&pool, due, "a").await
    );
    assert_eq!(
        finished,
        1,
        "exactly one run's result committed after the wake — the due retry's \
         second attempt: {:?}",
        reg.seen()
    );

    // The other direction, which is what a widened predicate breaks: a retry
    // whose interval has NOT elapsed must still be sitting there.
    assert_eq!(
        run_state(&pool, waiting, "a").await.as_deref(),
        Some("RETRYING"),
        "a retry 30 seconds away must not be started 1.3 seconds in: {}",
        stuck(&pool, waiting, "a").await
    );
    assert_eq!(
        attempts_of(&pool, waiting, "a").await,
        1,
        "and it must not have opened a second attempt: {}",
        stuck(&pool, waiting, "a").await
    );

    // **The `RETRYING` term's own assertion, and the one that names it.**
    // `advance()` never clears a `not_before` — `executor::apply`'s
    // `ScheduleRetry` arm writes one and nothing ever writes `None` back — so
    // `due`'s run STILL carries the deadline its retry wrote and that deadline
    // is still in the past. The state term is the only thing between the waker
    // and a finished execution it would relock and re-settle on every tick for
    // ever.
    let again = wake(&pool, 8).await.expect("wake again");
    assert!(
        again.is_empty(),
        "the waker named {again:?} when nothing is RETRYING. Due side: {}. \
         Waiting side: {}",
        stuck(&pool, due, "a").await,
        stuck(&pool, waiting, "a").await
    );
}

/// A one-task flow whose only task declares a `timeout:`.
///
/// The task id is a parameter because the whole point of this property is
/// **which** run the budget belonged to, and three flows whose task is called
/// `a` cannot tell that.
fn one_task_with_timeout(id: &str, task: &str, timeout: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: {task}\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    timeout: {timeout}\n"
    )
}

/// How long the [`Slow`] handler takes. Longer than the tight budget below and
/// far shorter than the generous one, so the two arms cannot both be explained
/// by the same accident.
const SLOW_HANDLER: std::time::Duration = std::time::Duration::from_millis(2_000);

/// A [`Registry`] that takes [`SLOW_HANDLER`] to answer, and records the budget
/// it was handed.
///
/// **Two lists, not one.** `started` records the budget each run arrived with
/// — the "reaches the handler" half — and `completed` records the runs whose
/// future was allowed to finish. A wrapper that awaited the handler and then
/// rewrote the outcome would satisfy every state assertion in this test and
/// leave `completed` holding the timed-out run, so the second list is what
/// says the handler was actually cut off.
#[derive(Default)]
struct Slow {
    started: Mutex<Vec<(String, Option<std::time::Duration>)>>,
    completed: Mutex<Vec<String>>,
    /// The runs this registry was asked to STOP — W4 Task 3's kill path.
    ///
    /// A third list rather than a flag on the first two, for their own reason:
    /// "was handed a budget", "was allowed to finish" and "was told to stop"
    /// are three different facts, and a kill path wired to the wrong branch
    /// (every run, or none) is only visible by comparing this list with
    /// `completed`.
    killed: Mutex<Vec<String>>,
}

impl Slow {
    fn started(&self) -> Vec<(String, Option<std::time::Duration>)> {
        let mut v = self.started.lock().unwrap().clone();
        v.sort();
        v
    }
    fn completed(&self) -> Vec<String> {
        let mut v = self.completed.lock().unwrap().clone();
        v.sort();
        v
    }
    fn killed(&self) -> Vec<String> {
        let mut v = self.killed.lock().unwrap().clone();
        v.sort();
        v
    }
}

#[async_trait::async_trait]
impl Registry for Slow {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.started
            .lock()
            .unwrap()
            .push((task.task_id.clone(), task.timeout));
        tokio::time::sleep(SLOW_HANDLER).await;
        self.completed.lock().unwrap().push(task.task_id.clone());
        Ok(Outcome::success())
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        self.killed.lock().unwrap().push(task.task_id.clone());
        Ok(())
    }
}

/// The message a run failed with — `task_runs.message`, migration 0137.
async fn run_message(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read run message")
}

/// **Property 7: a task-level `timeout:` reaches the handler and is enforced by
/// the worker.**
///
/// W3b measured that a `timeout:` was stripped before a handler saw it —
/// `models::Task`'s deserializer takes it out of a leaf's `spec` — so
/// `egret.sql` ran under a 300-second constant unrelated to what the author
/// wrote, and every other handler ran unbounded. This is the property that
/// says otherwise, in the three shapes the artifact distinguishes.
///
/// # Why it is written this way
///
/// **With the old constant in place a `timeout: PT1S` test PASSES**, because
/// the handler finishes and the run succeeds: a test that asserted "the flow
/// ran" or "an error occurred somewhere" would record only that time passed.
/// So every arm here asserts the run's **terminal state and its message**, and
/// the message is what separates a budget that fired from a handler that
/// failed for an unrelated reason — the two are the same `FAILED` row
/// otherwise.
///
/// **The generous arm is the positive control.** `timeout: PT30S` over the same
/// two-second handler must reach `SUCCESS` with no message at all, at the same
/// moment the tight arm has failed. A worker that simply failed everything
/// carrying a `timeout:` passes the first arm and fails this one — and that is
/// the artifact's own answer, measured: the same flow at `PT30S` ran to
/// SUCCESS in 3.1 s.
///
/// # The values are the artifact's
///
/// `Timeout after 00:00:01.000` is `TimeoutExceededException(Duration)` — probed
/// by reflection inside `kestra/kestra@sha256:28f59bf1…` and seen live in that
/// server's own ERROR log for a `Sleep` cut off by `timeout: PT1S`, whose run
/// went `['CREATED','SUBMITTED','RUNNING','FAILED']`. `timeout: must be a
/// positive duration` is the artifact's own sentence for `PT0S`. See
/// `worker::timeout`'s module doc for both, and for the third case (an
/// unparseable duration) where egret's wording is deliberately its own.
#[sqlx::test(migrations = "../../migrations")]
async fn a_task_level_timeout_reaches_the_handler_and_the_worker_enforces_it(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = Slow::default();

    let tight = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_timeout("tight", "tight", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start tight");
    let generous = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_timeout("generous", "generous", "PT30S"),
        serde_json::json!({}),
    )
    .await
    .expect("start generous");
    let zero = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_timeout("zero", "zero", "PT0S"),
        serde_json::json!({}),
    )
    .await
    .expect("start zero");

    let finished = drain_with(&pool, &reg).await;
    assert_eq!(
        finished,
        3,
        "all three runs must reach a committed result; a timeout is a task \
         failure, not a worker failure that aborts the tick. started: {:?}",
        reg.started()
    );

    // ---- the budget reaches the handler ----------------------------------
    assert_eq!(
        reg.started(),
        vec![
            (
                "generous".to_string(),
                Some(std::time::Duration::from_secs(30))
            ),
            ("tight".to_string(), Some(std::time::Duration::from_secs(1))),
        ],
        "the handler must be handed the author's OWN budget, per run, and the \
         run whose `timeout:` cannot be used must never be started at all. \
         `zero` appearing here means a refusal ran a handler; a `None` beside \
         a name means the seam W3b measured is still open and the value was \
         stripped before the handler saw it"
    );

    // ---- the worker enforces it ------------------------------------------
    assert_eq!(
        run_state(&pool, tight, "tight").await.as_deref(),
        Some("FAILED"),
        "a two-second handler under `timeout: PT1S` must fail, and FAILED is \
         the artifact's answer rather than KILLED (`callWithTimeout` calls \
         `kill(null)` so `exceptionHandler` falls through to FAILED): {}",
        stuck(&pool, tight, "tight").await
    );
    assert_eq!(
        run_message(&pool, tight, "tight").await.as_deref(),
        Some("Timeout after 00:00:01.000"),
        "and it must carry Kestra's own sentence for the budget the author \
         wrote, byte for byte: {}",
        stuck(&pool, tight, "tight").await
    );
    assert_eq!(
        reg.killed(),
        vec!["tight".to_string()],
        "the kill path fires on the budget's expiry and on nothing else: \
         `generous` answered inside its budget and has already stopped, and \
         `zero` was refused before a handler ran at all, so a kill reaching \
         either would be reaching a task that is not running"
    );
    assert_eq!(
        reg.completed(),
        vec!["generous".to_string()],
        "the timed-out handler must be DROPPED at its budget. A run in this \
         list finished its two seconds, which means the budget was applied to \
         its RESULT rather than to its execution -- indistinguishable from a \
         real timeout by state and message alone"
    );

    // ---- the positive control --------------------------------------------
    assert_eq!(
        run_state(&pool, generous, "generous").await.as_deref(),
        Some("SUCCESS"),
        "the same handler under `timeout: PT30S` must be left alone; without \
         this arm, a worker that failed every task carrying a `timeout:` would \
         pass everything above: {}",
        stuck(&pool, generous, "generous").await
    );
    assert_eq!(
        run_message(&pool, generous, "generous").await,
        None,
        "and a success carries no message: {}",
        stuck(&pool, generous, "generous").await
    );

    // ---- a budget that cannot be used ------------------------------------
    assert_eq!(
        run_state(&pool, zero, "zero").await.as_deref(),
        Some("FAILED"),
        "`timeout: PT0S` is refused on the artifact -- `@DurationMin(millis = \
         1)`, measured at both sides of the boundary -- and the run fails \
         without the task body ever running: {}",
        stuck(&pool, zero, "zero").await
    );
    assert_eq!(
        run_message(&pool, zero, "zero").await.as_deref(),
        Some("timeout: must be a positive duration"),
        "with the artifact's own sentence, not a duration-parse complaint: {}",
        stuck(&pool, zero, "zero").await
    );
}

/// A two-task flow whose FIRST task carries a `timeout:`.
///
/// Two tasks rather than one because the properties below are about what a
/// stale writer could do to the *rest* of the execution: `b`'s row is the
/// successor whose existence, and whose being left alone, is the dual of the
/// plan's Global Constraint.
fn two_tasks_first_with_timeout(id: &str, timeout: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    timeout: {timeout}\n  - id: b\n    \
         type: io.kestra.plugin.core.log.Log\n    message: bye\n"
    )
}

/// A [`Registry`] whose handler never finishes and whose `cancel` **refuses**,
/// which is `handlers::sql`'s shape without needing an engine.
#[derive(Default)]
struct Stubborn {
    killed: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Registry for Stubborn {
    async fn run(&self, _task: &TaskInput) -> egret_core::Result<Outcome> {
        tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
        unreachable!("this handler is never allowed to finish")
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        self.killed.lock().unwrap().push(task.task_id.clone());
        Err(egret_core::Error::Internal(anyhow::anyhow!(
            "the statement is still running on the engine"
        )))
    }
}

/// **Property 8: a handler that cannot be stopped still fails on its budget,
/// and the refusal changes nothing the author sees.**
///
/// This is the `egret.sql` shape driven end to end. W3b made that handler's
/// `cancel` refuse deliberately — *"when W4 wires the kill path this fails
/// loudly at the one place that has to change"* — and the loudness has to be
/// bounded: a refusal is an operator's fact about egret, not the reason the
/// author's task failed, and it must not abort the tick or leave the run
/// uncommitted.
///
/// **Measured on the pinned artifact rather than reasoned**, by `javap -c` over
/// classes extracted from `/app/kestra` in
/// `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`:
/// `WorkerTaskCallable.kill(State.Type)` calls `task.kill()` inside a `try`
/// whose exception table catches `java.lang.Exception`, logs
/// `"Error while killing task: '{}'"` at WARN, and then calls
/// `super.kill(state)` on every path. The terminal is still
/// `exceptionHandler(TimeoutExceededException)`'s — `FAILED`, carrying
/// `Timeout after HH:mm:ss.SSS`. So a kill that fails is *logged and forgotten*
/// there, and here.
///
/// Three assertions, and the third is the one a wrong implementation passes the
/// first two without: the tick's own **count**. A version that propagated the
/// refusal as `Err` would leave the run leased and untouched, and the state
/// assertion alone cannot tell that apart from a run that failed on its budget.
#[sqlx::test(migrations = "../../migrations")]
async fn a_handler_that_cannot_be_stopped_still_fails_on_its_budget_and_says_so(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = Stubborn::default();
    let ex = start_execution(
        &pool,
        ws,
        None,
        &two_tasks_first_with_timeout("stubborn", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");

    let finished = egret_workflow::worker::tick(&pool, 8, &reg)
        .await
        .expect("a refused kill must NOT be a worker failure -- it would abort the tick");
    assert_eq!(
        finished, 1,
        "the run's result must still commit; a refusal that propagated would \
         leave it leased and RUNNING until the reaper took it back"
    );

    assert_eq!(
        reg.killed.lock().unwrap().clone(),
        vec!["a".to_string()],
        "the handler must have been ASKED to stop -- this is the whole of the \
         kill path, and without it the refusal below is unreachable"
    );
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("FAILED"),
        "FAILED and not KILLED, and not left RUNNING by a refusal: {}",
        stuck(&pool, ex, "a").await
    );
    assert_eq!(
        run_message(&pool, ex, "a").await.as_deref(),
        Some("Timeout after 00:00:01.000"),
        "the author-facing reason is the budget's, byte for byte. A kill that \
         could not be delivered is an operator's fact and goes to `tracing`, \
         exactly as `WorkerTaskCallable.kill` sends it to slf4j and leaves \
         `exceptionHandler`'s state alone: {}",
        stuck(&pool, ex, "a").await
    );
}

/// A [`Registry`] that reaps its own lease and lets a rival worker commit,
/// **from inside its handler**, and then outruns its budget.
///
/// The interleaving is deterministic rather than raced: everything the rival
/// does happens before this handler's sleep, so by the time the budget expires
/// the run is already somebody else's and this worker is the stale one.
struct ReapedFromUnderIt {
    pool: sqlx::PgPool,
    killed: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Registry for ReapedFromUnderIt {
    async fn run(&self, _task: &TaskInput) -> egret_core::Result<Outcome> {
        // The lease expires while this handler is in flight -- `reap` clears
        // `claimed_at` and leaves `state = 'RUNNING'`, which is the claim
        // predicate again.
        assert_eq!(
            reap(&self.pool, std::time::Duration::ZERO)
                .await
                .expect("reap"),
            1,
            "the run this handler is executing must be reclaimable"
        );
        // A rival claims the same run and commits first.
        let rival = claim(&self.pool, 1).await.expect("rival claim");
        assert_eq!(rival.len(), 1, "the reaped run is claimable again");
        let mut tx = self.pool.begin().await.unwrap();
        apply_outcome(&mut tx, &rival[0], &Outcome::success())
            .await
            .expect("the rival holds the LIVE lease and must be able to commit");
        tx.commit().await.unwrap();
        // ... and only now does this worker outrun its budget.
        tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
        unreachable!("this handler is never allowed to finish")
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        self.killed.lock().unwrap().push(task.task_id.clone());
        Ok(())
    }
}

/// **Property 9: a killed handler cannot commit over the worker that replaced
/// it.**
///
/// W3b's Critical was a *reaped* worker committing over its replacement, and
/// `a_worker_whose_lease_was_reaped_cannot_commit` drives that by calling
/// `apply_outcome` by hand. Cancellation is the same seam from the other side
/// and this drives the **shipping `tick`**: the run is killed on its budget,
/// the kill path runs, and the worker then tries to write a terminal for a run
/// that is no longer its.
///
/// **A cancel is not what protects this seam, and that is the finding.**
/// [`reap`] is an `UPDATE` run by whatever process calls it — in production not
/// the one holding the run — and a lease is a `timestamptz` in a row, not a
/// handle to a future, so nothing can reach across and cancel the abandoned
/// handler. The abandoned worker finds out only when its write is refused. The
/// fence is therefore the whole of the protection here, and this test exists to
/// see it refuse rather than to assume it does. `worker::cancel`'s module doc
/// carries the same statement.
///
/// **Every assertion is one the pre-fence code failed.** W3b measured that
/// shape directly: `state=FAILED`, `ended_seq` wiped from `Some(4)` to NULL,
/// the execution FAILED with its successor still live. All four are read back
/// here, plus the kill path's own footprint — because a test that only checked
/// the error would pass against an `apply_outcome` that errored *after*
/// writing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_killed_handler_cannot_commit_over_the_worker_that_replaced_it(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = ReapedFromUnderIt {
        pool: pool.clone(),
        killed: Mutex::new(Vec::new()),
    };
    let ex = start_execution(
        &pool,
        ws,
        None,
        &two_tasks_first_with_timeout("fenced-kill", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");

    let err = egret_workflow::worker::tick(&pool, 8, &reg)
        .await
        .expect_err("a worker whose lease is gone must not report its run finished");
    assert!(
        err.to_string().contains("lease that no longer exists"),
        "and it must say WHY, so a production log names the reclaim rather than \
         a generic write failure: {err}"
    );

    assert_eq!(
        reg.killed.lock().unwrap().clone(),
        vec!["a".to_string()],
        "the kill path still runs for a run this worker has already lost. It is \
         stopping its OWN in-flight future, which nothing else can reach -- and \
         that is the whole of the claim. It is NOT that `cancel` may act on the \
         run: by now the run belongs to the rival, and a handler that named an \
         external resource after `run_id` would be reaching the rival's. \
         `TaskHandler::cancel`'s doc carries that hazard for W4 Task 5, and it \
         is why the fence below is what protects the row rather than the kill"
    );

    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("SUCCESS"),
        "the rival's terminal stands; a second terminal on one attempt is W3b's \
         Critical: {}",
        stuck(&pool, ex, "a").await
    );
    assert_eq!(
        run_message(&pool, ex, "a").await,
        None,
        "and not even the message may land -- `apply_outcome` writes it before \
         the state, so a fence only on the state would leave `Timeout after \
         00:00:01.000` beside somebody else's SUCCESS: {}",
        stuck(&pool, ex, "a").await
    );
    assert!(
        ended_seq(&pool, ex, "a").await.is_some(),
        "the happens-before right endpoint must survive: the pre-fence \
         measurement wiped it from Some(4) to NULL"
    );
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING"]),
        "the execution must NOT have reached a terminal; FAILED here is the \
         dual of the plan's Global Constraint -- the execution moved and the \
         task is not finished"
    );
    assert_eq!(
        run_state(&pool, ex, "b").await.as_deref(),
        Some("RUNNING"),
        "and the successor the rival created is live and claimable -- RUNNING \
         with no lease IS the claim's predicate, which is what makes an \
         execution that had reached FAILED beside it the Critical it was"
    );
}

/// Two `Sequential` branches of one `Parallel`, four leaves. The shape of
/// `valids/parallel-disabled-tasks` without the `disabled:`, which is the
/// entry W4 Task 4's ruling is about.
fn parallel_of_two_sequences(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: s1\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: task1\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n          - id: task2\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n      - id: s2\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: task3\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n          - id: task4\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n"
    )
}

/// **The seventh property, and it is a RULING rather than a durability one:
/// one handler result per transaction.**
///
/// W3b handed W4 the question of whether the tick should stay this shape, and
/// W4 Task 4 ruled that it does. The ruling is invisible to every other test in
/// this file and only *half*-visible to the oracle — `oracle-exec` sees it as
/// four entries that reproduce every run, attempt and execution state Kestra
/// recorded and differ on the happens-before relation alone — so it is stated
/// here as the thing a future change would have to break on purpose.
///
/// # What is asserted, and why it is these three runs
///
/// `task1` and `task3` are the first leaves of two `Sequential` branches of one
/// `Parallel`, so both are claimable at once and ONE tick finishes both. The
/// question the ruling answers is what `advance()` sees in between:
///
/// - egret applies `task1`'s result, drives `advance()` to a fixed point (which
///   creates `task2`) and commits, then does the same for `task3`. So
///   `ended(task1) < created(task2) < ended(task3)`.
/// - Kestra's worker pool finished both siblings before either successor
///   existed, so on its side `created(task2)` is AFTER `ended(task3)`.
///
/// The middle inequality is therefore the whole of the divergence, and it is
/// the assertion below. Measured, on this flow, both ways: shipping gives
/// `ended(task1)=8, created(task2)=9, ended(task3)=11`; the widened tick gives
/// `ended(task1)=8, created(task2)=9, ended(task3)=8` — both siblings on step
/// 8 and both successors on step 9, which is Kestra's shape exactly.
///
/// # This is not about the batch size, and that was measured
///
/// The tick claims **eight** runs here and finishes two of them, so "one run
/// per tick" is the wrong name for it: the discipline is one result per
/// TRANSACTION. Setting `oracle-exec/driver.rs`'s `BATCH` to 32 with this
/// discipline unchanged left the ratchet at 143 baselined and every test in
/// this file green — the claim orders by `created_at, execution_id, run_ix`, so
/// a batch of n is indistinguishable from n successive claims of 1.
///
/// # The cost of the ruling, measured rather than argued
///
/// Applying a whole batch's results in one transaction before calling
/// `advance()` banks **`valids/dag`, `valids/full` and
/// `valids/parallel-disabled-tasks`** (143 mismatching → 140, no regression,
/// `valids/if` still differing on `order` alone — so the benefit column is a
/// LOWER bound while that entry's cause is unknown). It was refused for what it
/// costs, and the cost it turns on is not an ordering one:
///
/// **Widening would ship `a_worker_whose_lease_was_reaped_cannot_commit` and
/// `a_tick_killed_between_the_handler_and_the_commit_applies_nothing` green
/// over a path they do not touch.** Both drive `apply_outcome` with a single
/// `Claimed`; a widened tick releases `n` leases per transaction. Measured:
/// under the widened prototype, deleting the `outputs` fence outright AND
/// making `persist` fence only the group's first lease — a defect that is
/// byte-equivalent at `n == 1` and lets a reaped worker commit over its
/// replacement at `n > 1` — leaves **the whole crate green** but for this test.
/// `worker::claim::tick`'s doc carries all four costs in order and
/// `oracle-exec/w4t4-widened-tick.txt` carries every run behind them, including
/// the five reap configurations that show the *shipping* tick starving under
/// the mirror-image adversary — which is why the lease-loss cost is ranked
/// fourth here and not first, and why the first version of this doc was wrong
/// to call it a live-lock of the widening's.
///
/// See `oracle-exec/README.md`'s *One result per transaction is a ruling* for
/// W3a's deleted guard, which stays unreachable under the widened tick —
/// measured, not predicted — and for the savepoint design that is the named way
/// back in.
#[sqlx::test(migrations = "../../migrations")]
async fn one_result_per_transaction_is_the_ruling_and_this_is_the_step_it_costs(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(
        &pool,
        ws,
        None,
        &parallel_of_two_sequences("ruling"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");

    let reg = Recorder::default();
    let finished = egret_workflow::worker::tick(&pool, 8, &reg)
        .await
        .expect("one tick, eight claimable slots");
    assert_eq!(
        finished, 2,
        "both first leaves are claimable at once and ONE tick finishes both, so          the ruling is not about how many runs a tick takes: {:?}",
        reg.seen()
    );
    assert_eq!(
        reg.seen(),
        vec!["task1".to_string(), "task3".to_string()],
        "and it takes them in `run_ix` order, which is what the claim's          tiebreak is for"
    );

    let ended1 = ended_seq(&pool, ex, "task1")
        .await
        .expect("task1 terminated in this tick");
    let created2 = created_seq(&pool, ex, "task2")
        .await
        .expect("task1's successor exists");
    let ended3 = ended_seq(&pool, ex, "task3")
        .await
        .expect("task3 terminated in this tick");

    assert!(
        ended1 < created2,
        "a successor is created strictly after the run that caused it ends          (task1 {ended1}, task2 {created2}, task3 {ended3})"
    );
    assert!(
        created2 < ended3,
        "THE RULING. `task1`'s result, `advance()` and `task1`'s successor are          one transaction, so `task2` exists before `task3`'s result is applied          at all. Kestra's worker pool finished both siblings first and its          `created(task2)` is after `ended(task3)`; that divergence is          `valids/dag`, `valids/full` and `valids/parallel-disabled-tasks`, and          W4 Task 4 measured that widening the tick banks all three and refused          it anyway -- see this test's doc for the price. TWO things reach this          failure and they want different answers. Either the tick has been          WIDENED, in which case the ruling has been reversed and the three          baseline lines have to go in the same commit; or `task1` and `task3`          were finished CONCURRENTLY by two workers, which produces the same          equality with nothing edited at all and means this test has lost its          isolation -- it drives one `tick` itself, on its own `#[sqlx::test]`          database, and nothing else may be claiming from it          (task1 {ended1}, task2 {created2}, task3 {ended3})"
    );
    assert_ne!(
        ended1, ended3,
        "two siblings finished in one tick must not share a step: the step IS          the happens-before endpoint, and a shared one is exactly what a batched          transaction produces"
    );
}

// --------------------------------------------------------------------------
// W6 Task 2: the three kinds of child execution, and the CHECKs that make
// every other combination unrepresentable
// --------------------------------------------------------------------------

/// Every column of `workflow.executions` that can link one execution to
/// another, as one value, so a test can name a combination instead of writing
/// an INSERT for it.
///
/// `Default` is a TOP-LEVEL execution -- no link of either kind -- and each
/// test sets only the fields whose combination it is about.
#[derive(Default, Clone, Copy)]
struct Link {
    /// Supplied only by the self-link cases, which need to know the id before
    /// the row exists. `None` lets the column default.
    id: Option<Uuid>,
    parent: Option<Uuid>,
    parent_run_ix: Option<i32>,
    loop_index: Option<i32>,
    loop_value: Option<&'static str>,
    trigger: Option<Uuid>,
    trigger_run_ix: Option<i32>,
}

/// Insert one execution with exactly `link`'s columns set, returning the
/// database's own answer.
///
/// **Deliberately not `start_execution`.** The point of these tests is what the
/// SCHEMA refuses, so the statement has to be able to express the illegal
/// combinations -- a writer that cannot spell one cannot prove the database
/// refuses it.
async fn insert_linked(pool: &sqlx::PgPool, ws: Uuid, link: Link) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (id, workspace_id, namespace, flow_key, source, state, history, \
            parent_execution_id, parent_run_ix, loop_index, loop_value, \
            trigger_execution_id, trigger_run_ix) \
         VALUES (COALESCE($1, gen_random_uuid()), $2, 't', 'f', 'src', \
                 'CREATED', '[\"CREATED\"]'::jsonb, $3, $4, $5, $6, $7, $8) \
         RETURNING id",
    )
    .bind(link.id)
    .bind(ws)
    .bind(link.parent)
    .bind(link.parent_run_ix)
    .bind(link.loop_index)
    .bind(link.loop_value)
    .bind(link.trigger)
    .bind(link.trigger_run_ix)
    .fetch_one(pool)
    .await
}

/// The name of the constraint that refused a write.
///
/// **The name is the assertion, not the fact that something failed.** A test
/// that asserted only `is_err()` would keep passing if the CHECK were dropped
/// and a NOT NULL, a foreign key or a unique index happened to refuse the same
/// row for a different reason -- and it would keep passing if the constraint
/// were renamed out from under it, which is the case migration 0138 itself
/// creates by renaming 0135's.
fn refusing_constraint(e: &sqlx::Error) -> String {
    let db = e
        .as_database_error()
        .unwrap_or_else(|| panic!("expected a database error, got {e}"));
    db.constraint()
        .unwrap_or_else(|| panic!("the database refused the row without naming a constraint: {e}"))
        .to_string()
}

/// Seed a workspace and one top-level execution to hang children off.
async fn seed_root(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let (_db, ws) = seed(pool).await;
    let root = insert_linked(pool, ws, Link::default())
        .await
        .expect("a top-level execution has no link of either kind");
    (ws, root)
}

/// **All three kinds of child execution are representable, and this is the
/// positive control for the test below.**
///
/// Measured on the pinned artifact (`oracle-exec/traces.json`), Kestra makes
/// three distinct things and links them two different ways:
///
/// | kind | `parentId` | loop index | the link it is found by |
/// |---|---|---|---|
/// | loop iteration | set | set | `parent_execution_id` |
/// | `CREATE_NEW_EXECUTION` replay | set | **absent** | `parent_execution_id` |
/// | subflow child | **NULL** | absent | `trigger_execution_id` |
///
/// Before migration 0138 the middle row was **unrepresentable** -- 0135's
/// `workflow_executions_loop_run_all_or_nothing` required all four loop
/// columns or none -- and the bottom row had no column to be written into at
/// all. A test that only listed refusals could not tell "the CHECK is right"
/// from "the CHECK forbids everything", so this one exists to be the other
/// half of that pair.
///
/// The fourth row is W6 Task 6's `Flow` trigger, which is caused by an
/// execution reaching a state and by no particular taskrun: it is here because
/// the schema must already admit it, not because anything writes it yet.
#[sqlx::test(migrations = "../../migrations")]
async fn all_three_kinds_of_child_execution_are_representable(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (ws, root) = seed_root(&pool).await;

    for (kind, link) in [
        (
            "loop iteration",
            Link {
                parent: Some(root),
                parent_run_ix: Some(0),
                loop_index: Some(0),
                loop_value: Some("a"),
                ..Link::default()
            },
        ),
        (
            "CREATE_NEW_EXECUTION replay",
            Link {
                parent: Some(root),
                ..Link::default()
            },
        ),
        (
            "subflow child",
            Link {
                trigger: Some(root),
                trigger_run_ix: Some(0),
                ..Link::default()
            },
        ),
        (
            "flow-trigger-fired execution (W6 Task 6)",
            Link {
                trigger: Some(root),
                ..Link::default()
            },
        ),
    ] {
        insert_linked(&pool, ws, link)
            .await
            .unwrap_or_else(|e| panic!("{kind} must be representable, and was refused: {e}"));
    }
}

/// **Every wrong combination is refused, and by the constraint that is named
/// here.**
///
/// This programme's rule is *unrepresentable beats unforgettable*: a writer
/// that forgets one of these returns 200, it does not fail. Migration 0135's
/// own comment says the same thing about the constraint 0138 replaces.
///
/// **The row that matters most is `both links at once`.**
/// `oracle-exec/driver.rs` builds the oracle's `spawned` field with
/// `WHERE parent_execution_id = $1`, and all 41 golden `spawned`-carrying
/// entries have **zero** `Subflow` children. A subflow child that also carried
/// a `parent_execution_id` would appear in egret's trace and turn the entries
/// W6 exists to bank into `spawned` mismatches --
/// `a_subflow_child_is_invisible_to_the_query_that_builds_spawned` in
/// `oracle-exec/driver.rs` is the other end of that guarantee.
#[sqlx::test(migrations = "../../migrations")]
async fn every_wrong_child_link_is_refused_by_a_named_constraint(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (ws, root) = seed_root(&pool).await;
    // Chosen by this test rather than by the database, because a self-link has
    // to be written into the same row it names.
    let mine = Uuid::new_v4();

    let cases: Vec<(&str, Link, &str)> = vec![
        (
            "a loop index with no parent",
            Link {
                parent_run_ix: Some(0),
                loop_index: Some(0),
                loop_value: Some("a"),
                ..Link::default()
            },
            "workflow_executions_loop_run_whole_or_absent",
        ),
        (
            "a parent and a run, but no index -- 0135's real target, still refused",
            Link {
                parent: Some(root),
                parent_run_ix: Some(0),
                ..Link::default()
            },
            "workflow_executions_loop_run_whole_or_absent",
        ),
        (
            "a loop iteration with no value",
            Link {
                parent: Some(root),
                parent_run_ix: Some(0),
                loop_index: Some(0),
                ..Link::default()
            },
            "workflow_executions_loop_run_whole_or_absent",
        ),
        (
            "a loop iteration that does not say which run spawned it",
            Link {
                parent: Some(root),
                loop_index: Some(0),
                loop_value: Some("a"),
                ..Link::default()
            },
            "workflow_executions_loop_run_whole_or_absent",
        ),
        (
            "BOTH links at once -- a subflow child that would appear in `spawned`",
            Link {
                parent: Some(root),
                trigger: Some(root),
                ..Link::default()
            },
            "workflow_executions_one_link_kind",
        ),
        (
            "a trigger run ordinal into an execution that is not named",
            Link {
                trigger_run_ix: Some(0),
                ..Link::default()
            },
            "workflow_executions_trigger_run_needs_its_execution",
        ),
        (
            "an execution that is its own parent",
            Link {
                id: Some(mine),
                parent: Some(mine),
                parent_run_ix: Some(0),
                loop_index: Some(0),
                loop_value: Some("a"),
                ..Link::default()
            },
            "workflow_executions_no_self_link",
        ),
        (
            "an execution that triggered itself",
            Link {
                id: Some(mine),
                trigger: Some(mine),
                trigger_run_ix: Some(0),
                ..Link::default()
            },
            "workflow_executions_no_self_link",
        ),
    ];

    for (what, link, constraint) in cases {
        let err = insert_linked(&pool, ws, link)
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("the database ACCEPTED {what}, which must be unrepresentable")
            });
        assert_eq!(
            refusing_constraint(&err),
            constraint,
            "{what} must be refused by {constraint}"
        );
    }
}

/// **One replay child per parent, and the collision is unrepresentable rather
/// than merely undetected.**
///
/// 0135's argument, restated for a key that can now hold NULLs: two children of
/// one parent that sort equal make the `spawned` order undetermined, which is
/// what `oracle-exec/normalise.py` has to BAIL on, because the creation instant
/// carries no information at all -- every sub-execution of one batch is written
/// in one transaction and shares `now()`. A replay child's `parent_run_ix` and
/// `loop_index` are both NULL, so under Postgres's default NULLS DISTINCT rule
/// any number of them could hang off one parent and tie under
/// `ORDER BY parent_run_ix, loop_index`. 0138 adds `NULLS NOT DISTINCT`.
///
/// **One per parent is the artifact's behaviour, measured.** Over the 710
/// descendants in `oracle-exec/traces.json`, 40 carry a NULL `loopIndex` (the
/// four `valids/retry-new-execution-*` flows, two deep, five rounds) and no
/// parent has more than one of them: Kestra CHAINS a `CREATE_NEW_EXECUTION`
/// retry rather than fanning it out.
///
/// **Both positive controls are here because a unique index is exactly the kind
/// of guard that is easy to make too strong.** Two loop iterations of one run
/// are legal, and so are two subflow children of ONE run -- a `Subflow` task may
/// carry its own `retry:`, and each attempt starts a new child
/// (`valids/subflow-parent-retry` declares two of them at `maxAttempts: 3`),
/// which is why `workflow_executions_trigger_idx` is NOT unique.
#[sqlx::test(migrations = "../../migrations")]
async fn a_parent_may_have_one_replay_child_but_never_two(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (ws, root) = seed_root(&pool).await;

    let replay = Link {
        parent: Some(root),
        ..Link::default()
    };
    insert_linked(&pool, ws, replay)
        .await
        .expect("the first replay child");
    let err = insert_linked(&pool, ws, replay)
        .await
        .expect_err("a SECOND replay child of the same parent must be unrepresentable");
    assert_eq!(
        refusing_constraint(&err),
        "workflow_executions_loop_run_idx",
        "two replay children of one parent tie under `ORDER BY parent_run_ix, \
         loop_index` and would be ordered by nothing"
    );

    for index in 0..2 {
        insert_linked(
            &pool,
            ws,
            Link {
                parent: Some(root),
                parent_run_ix: Some(0),
                loop_index: Some(index),
                loop_value: Some("a"),
                ..Link::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("loop iteration {index} of one run must stay legal: {e}"));
    }

    for attempt in 0..2 {
        insert_linked(
            &pool,
            ws,
            Link {
                trigger: Some(root),
                trigger_run_ix: Some(0),
                ..Link::default()
            },
        )
        .await
        .unwrap_or_else(|e| {
            panic!("attempt {attempt} of ONE retried `Subflow` run must stay legal: {e}")
        });
    }
}

/// **A subflow child is not part of its parent's record, and the foreign key
/// says so.**
///
/// 0135 made `parent_execution_id` `ON DELETE CASCADE` because a loop
/// sub-execution *is* a part of its parent -- it has no flow of its own and no
/// meaning without the iteration that made it. A subflow child is the opposite:
/// a full execution of a DIFFERENT flow, with its own row in the executions
/// list. So `trigger_execution_id` takes no `ON DELETE` action at all, and this
/// test is what that decision costs and what it buys.
///
/// **The cost is a refusal, and the refusal is deliberate.** Nothing in `src/`
/// deletes an execution row today, so NO ACTION refuses a case that does not
/// arise rather than guessing at it; `ON DELETE SET NULL` was not available as
/// an alternative, because it would null the execution and leave
/// `trigger_run_ix` set, which
/// `workflow_executions_trigger_run_needs_its_execution` forbids -- the delete
/// would then fail anyway, with a constraint error naming the wrong thing.
///
/// **The workspace cascade is the half that had to be measured**, because a NO
/// ACTION self-reference could have made deleting a workspace impossible.
/// It does not: parent and child go in one statement, and the referential check
/// runs after it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_childs_parent_cannot_be_deleted_alone_but_the_workspace_cascade_takes_both(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (ws, root) = seed_root(&pool).await;
    let child = insert_linked(
        &pool,
        ws,
        Link {
            trigger: Some(root),
            trigger_run_ix: Some(0),
            ..Link::default()
        },
    )
    .await
    .expect("a subflow child");

    let err = sqlx::query("DELETE FROM workflow.executions WHERE id = $1")
        .bind(root)
        .execute(&pool)
        .await
        .expect_err("deleting an execution a subflow child still names must be refused");
    assert_eq!(
        refusing_constraint(&err),
        "workflow_executions_trigger_same_workspace",
        "the child must outlive nothing here silently: the refusal has to name \
         the link, not some later CHECK"
    );
    let alive: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE id = $1")
        .bind(child)
        .fetch_one(&pool)
        .await
        .expect("count the child");
    assert_eq!(alive, 1, "and nothing was deleted");

    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .expect("deleting the workspace must still cascade through both links");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&pool)
        .await
        .expect("count what is left");
    assert_eq!(left, 0, "the cascade removed the parent AND the child");
}

/// **Neither link may cross a workspace boundary.**
///
/// `parent_execution_id` (0135) and `trigger_execution_id` are id columns whose
/// single-column foreign key checked only that the row EXISTS -- never whose it
/// was. This repository has already paid for that shape once: five columns of
/// exactly this kind, one of which (`chart_id`) allowed a cross-workspace read,
/// patched only after it shipped.
///
/// 0138 replaces both foreign keys with composite ones carrying `workspace_id`,
/// so a child can only name a row in its OWN workspace. **Both columns, not
/// just the new one** -- closing it for `trigger_execution_id` alone would leave
/// a guard that reads as complete and is not, the same reason
/// `workflow_executions_no_self_link` closes 0135's hole as well as this
/// migration's.
///
/// The same-workspace inserts are the positive control: a composite key that
/// refused everything would look identical here to one that refuses the right
/// thing. A nonexistent parent is checked too, because replacing a foreign key
/// is a chance to lose plain referential integrity while gaining the scoping.
#[sqlx::test(migrations = "../../migrations")]
async fn neither_child_link_may_cross_a_workspace_boundary(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (ws, root) = seed_root(&pool).await;
    let (_db, other) = seed(&pool).await;

    for (kind, link) in [
        (
            "a loop iteration in the parent's own workspace",
            Link {
                parent: Some(root),
                parent_run_ix: Some(0),
                loop_index: Some(0),
                loop_value: Some("a"),
                ..Link::default()
            },
        ),
        (
            "a subflow child in the trigger's own workspace",
            Link {
                trigger: Some(root),
                trigger_run_ix: Some(0),
                ..Link::default()
            },
        ),
    ] {
        insert_linked(&pool, ws, link)
            .await
            .unwrap_or_else(|e| panic!("{kind} must stay legal, and was refused: {e}"));
    }

    let cases: Vec<(&str, Link, &str)> = vec![
        (
            "a loop iteration whose parent is in ANOTHER workspace",
            Link {
                parent: Some(root),
                parent_run_ix: Some(0),
                loop_index: Some(1),
                loop_value: Some("a"),
                ..Link::default()
            },
            "workflow_executions_parent_same_workspace",
        ),
        (
            "a replay whose parent is in ANOTHER workspace",
            Link {
                parent: Some(root),
                ..Link::default()
            },
            "workflow_executions_parent_same_workspace",
        ),
        (
            "a subflow child whose trigger is in ANOTHER workspace",
            Link {
                trigger: Some(root),
                trigger_run_ix: Some(1),
                ..Link::default()
            },
            "workflow_executions_trigger_same_workspace",
        ),
        (
            "a parent that does not exist at all -- the scoping must not have \
             cost plain referential integrity",
            Link {
                parent: Some(Uuid::new_v4()),
                parent_run_ix: Some(0),
                loop_index: Some(2),
                loop_value: Some("a"),
                ..Link::default()
            },
            "workflow_executions_parent_same_workspace",
        ),
    ];

    for (what, link, constraint) in cases {
        // `other` is the child's workspace; `root` lives in `ws`.
        let err = insert_linked(&pool, other, link)
            .await
            .expect_err("a cross-workspace link must be unrepresentable");
        assert_eq!(
            refusing_constraint(&err),
            constraint,
            "{what} must be refused by {constraint}"
        );
    }
}

/// **A replay does not inherit the outputs of the execution it replays, and a
/// loop iteration still does.**
///
/// `render::Surface::load` walks the `parent_execution_id` chain and copies
/// every ancestor's terminated outputs into `inherited`. Until migration 0138
/// that was unconditionally right: 0135's CHECK made a row with a parent and no
/// loop index unrepresentable, so every parent link was a loop iteration's.
/// 0138 admits the `CREATE_NEW_EXECUTION` replay, and a replay is a NEW
/// execution of the same flow -- it must not read `{{ outputs.<task> }}` off the
/// attempt it is replacing.
///
/// **The asymmetry is the reason this is guarded rather than merely written
/// down.** The other consumer of the same widened invariant,
/// `oracle-exec/driver.rs`'s `spawned` decode, would PANIC on a replay row and
/// announce itself. This one would have rendered a value from a failed previous
/// run and taken the other branch, in silence.
///
/// **Both halves hang off ONE parent execution with ONE terminated `before`
/// run**, so the only difference between them is the kind of link -- a control
/// that a second fixture could not give.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_does_not_inherit_outputs_but_a_loop_iteration_still_does(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    let parent = r#"id: inherit-parent
namespace: t
tasks:
  - id: before
    type: io.kestra.plugin.core.debug.Return
    format: parent-output
  - id: loop
    type: io.kestra.plugin.core.flow.Loop
    values: '["x"]'
    tasks:
      - id: item
        type: io.kestra.plugin.core.debug.Return
        format: "{{ outputs.before.value }}"
"#;
    let ex = start_execution(&pool, ws, None, parent, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let sub: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 \
           AND loop_index IS NOT NULL",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("the loop spawned exactly one sub-execution");
    assert_eq!(
        (
            run_state(&pool, sub, "item").await,
            run_outputs(&pool, sub, "item").await
        ),
        (
            Some("SUCCESS".to_string()),
            Some(serde_json::json!({"value": "parent-output"}))
        ),
        "the positive control: a LOOP iteration reads its parent's outputs, \
         which is `valids/loop-expression-context`'s whole shape and is what \
         `inherited` exists for"
    );

    // The replay. Two tasks, because `start_execution` settles once itself and
    // the run that reads the parent must not exist until the link is set:
    // `reads` is created only when `gate` finishes, which is after the UPDATE.
    let replayed = r#"id: inherit-replay
namespace: t
tasks:
  - id: gate
    type: io.kestra.plugin.core.log.Log
    message: hi
  - id: reads
    type: io.kestra.plugin.core.debug.Return
    format: "{{ outputs.before.value }}"
"#;
    let child = start_execution(&pool, ws, None, replayed, serde_json::json!({}))
        .await
        .expect("start execution");
    assert!(
        run_state(&pool, child, "reads").await.is_none(),
        "the run that reads the parent must not exist yet, or it was rendered \
         before the link was set and this test would pass for the wrong reason"
    );
    // Written by hand because nothing spawns a replay yet -- W6 Task 4 does.
    sqlx::query("UPDATE workflow.executions SET parent_execution_id = $1 WHERE id = $2")
        .bind(ex)
        .bind(child)
        .execute(&pool)
        .await
        .expect("link the child as a replay: a parent, and no loop columns");

    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        (
            run_state(&pool, child, "reads").await,
            run_outputs(&pool, child, "reads").await
        ),
        (Some("FAILED".to_string()), None),
        "a replay must NOT see `before`'s output. The same expression, over the \
         same parent, SUCCEEDED for the loop iteration above -- so a failure \
         here is the link kind and nothing else. Rendering it would have \
         produced a value from the attempt this execution replaces, with no \
         error anywhere"
    );
}

// ---------------------------------------------------------------------------
// W6 Task 3: `Subflow`
//
// The oracle compares the PARENT's trace and nothing else -- a subflow child is
// linked by `trigger_execution_id` and is deliberately invisible to the
// `spawned` query that builds it (`driver::read_trace_inner`, migration 0138).
// So everything about the child, the link and the failure paths is this file's
// to assert or it is asserted nowhere.
// ---------------------------------------------------------------------------

/// File one flow into `workflow.flows` through the product's own create path,
/// so a `(namespace, flowId)` pair has something to resolve against.
///
/// `FlowRepo::create` and not an INSERT, for `driver::publish_corpus`'s reason:
/// the resolver reads the same table the product writes, and a fixture that
/// wrote the row itself could keep passing after that stopped being true.
///
/// It is deliberately NOT published to a live version, and **the reason this
/// doc used to give for that has been overturned**. It read: *"W6 Task 1
/// measured why that has to work: only 16 of the corpus's 345 flows reach one,
/// so a resolver reading `flow_versions` would resolve 2 of the 20 `Subflow`
/// targets."* Both figures are gone — W10 Task 6 fix round 1 put the four core
/// Kestra FQCNs into `spec::registry::TASK_TYPES`, and the census now measures
/// **248 of 345** flows reaching a live version and **18 of the 20** targets
/// live (`driver::LIVE`, `driver::SUBFLOW_TARGETS_LIVE`).
///
/// **The fixture is unchanged and so is the decision it mirrors**, because the
/// binding reason was never the corpus's shape: `worker::subflow::resolve`
/// reads the DRAFT row so that a parent and its child cannot disagree about
/// which version of the world they are in. This fixture files a draft because
/// that is what the resolver reads — not because a version would be rare.
async fn file_flow(db: &Db, ws: Uuid, by: egret_core::UserId, source: &str) -> Uuid {
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
        .expect("file the flow")
        .id
}

/// One subflow child as [`children_by_trigger`] reads it:
/// `(id, trigger_run_ix, parent_execution_id, flow_key, state)`.
///
/// A named alias rather than the tuple inline, because the tuple is five wide
/// and clippy's `type_complexity` is right that a reader cannot tell the two
/// `Option`s apart at the call site. The two that matter are the middle pair —
/// the link this task is about, present and absent.
type SubflowChild = (Uuid, Option<i32>, Option<Uuid>, String, String);

/// One row of the cycle guard's chain walk:
/// `(execution id, flow_key, execution state, run state, run message)`.
type ChainRow = (Uuid, String, String, Option<String>, Option<String>);

/// Every execution linked to `parent` as a **subflow child**, with the columns
/// that say which kind of child it is.
async fn children_by_trigger(pool: &sqlx::PgPool, parent: Uuid) -> Vec<SubflowChild> {
    sqlx::query_as(
        "SELECT id, trigger_run_ix, parent_execution_id, flow_key, state \
           FROM workflow.executions \
          WHERE trigger_execution_id = $1 \
          ORDER BY created_at, id",
    )
    .bind(parent)
    .fetch_all(pool)
    .await
    .expect("read subflow children")
}

async fn run_history(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT history FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read run history")
}

async fn attempt_histories(
    pool: &sqlx::PgPool,
    execution: Uuid,
    task_id: &str,
) -> Vec<serde_json::Value> {
    let attempts: serde_json::Value = sqlx::query_scalar(
        "SELECT attempts FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("read attempts");
    attempts
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.get("history").cloned().unwrap_or(serde_json::Value::Null))
                .collect()
        })
        .unwrap_or_default()
}

fn subflow_parent(id: &str, target: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
         flowId: {target}\n{extra}  - id: after\n    \
         type: io.kestra.plugin.core.log.Log\n    message: after\n"
    )
}

/// **The link is `trigger_execution_id`, the parent run never passes through
/// `SUBMITTED`, and the child is invisible to the query that builds `spawned`.**
///
/// All three at once, because they are one decision seen from three sides, and
/// because each alone would pass against a different wrong implementation: a
/// child linked by `parent_execution_id` would still make the parent SUCCEED,
/// and a `Subflow` planned an opaque `Runnable` would still create a child if
/// its handler did.
///
/// The artifact's own answer, measured on the pinned digest with
/// `io.kestra.tests/subflow-parent`: the parent run is
/// `CREATED RUNNING SUCCESS`, the child's `parentId` is **null**, its `trigger`
/// is `{"type": "io.kestra.plugin.core.flow.Subflow", "variables":
/// {"executionId": …, "taskRunId": …}}`, and
/// `SELECT id FROM executions WHERE parent_id = <parent>` returns nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_is_linked_by_the_trigger_columns_and_never_by_parent_execution_id(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(&db, ws, user, &one_task("child")).await;

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    // **The load-bearing assertion, and it is first on purpose.** It is asked
    // WITHOUT going through the trigger columns -- the child is found by the
    // flow it runs -- so a child linked the wrong way is still found here and
    // this assertion is the one that fails, rather than a lookup that quietly
    // returns nothing three lines earlier.
    let ran: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM workflow.executions \
          WHERE flow_key = 'child' AND id <> $1 ORDER BY created_at, id",
    )
    .bind(parent)
    .fetch_all(&pool)
    .await
    .expect("find the child by the flow it runs");
    assert_eq!(
        ran.len(),
        1,
        "exactly one execution of the target flow, however it is linked: {ran:?}"
    );
    let child = ran[0].0;

    // The query `driver::read_trace_inner` builds `spawned` from, verbatim.
    // All 41 golden `spawned`-carrying entries have zero `Subflow` children, so
    // a child this finds turns every entry W6 banks into a `spawned` mismatch.
    let spawned: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 \
          ORDER BY parent_run_ix, loop_index",
    )
    .bind(parent)
    .fetch_all(&pool)
    .await
    .expect("read the spawned query");
    assert_eq!(
        spawned,
        Vec::<Uuid>::new(),
        "the child is {child}, and the `spawned` query must not see it: all 41 \
         golden `spawned`-carrying entries have ZERO Subflow children, so a \
         child reachable this way turns every entry W6 Task 3 banks into a \
         `spawned` mismatch"
    );

    let kids = children_by_trigger(&pool, parent).await;
    assert_eq!(kids.len(), 1, "exactly one child, and it exists: {kids:?}");
    let (by_trigger, run_ix, parent_link, flow_key, child_state) = kids[0].clone();
    assert_eq!(
        (
            by_trigger,
            run_ix,
            parent_link,
            flow_key.as_str(),
            child_state.as_str()
        ),
        (child, Some(0), None, "child", "SUCCESS"),
        "the child names the parent's run 0 through `trigger_run_ix`, carries NO \
         `parent_execution_id` (migration 0138's \
         `workflow_executions_one_link_kind` makes carrying both \
         unrepresentable), runs the TARGET flow rather than the parent's, and \
         reaches a terminal of its own"
    );

    assert_eq!(
        run_history(&pool, parent, "sub").await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        "no SUBMITTED. That is the measurement W6 opened src/executor/ for -- \
         `no_subflow_run_in_the_golden_passes_through_submitted` derives the \
         Kestra side of it over all 24 executed `Subflow` runs in the golden"
    );
    assert_eq!(
        run_outputs(&pool, parent, "sub").await,
        Some(serde_json::json!({
            "executionId": child.to_string(),
            "state": "SUCCESS",
            "outputs": {},
        })),
        "what the parent sees of its child. Measured on the artifact with \
         `egret.probe/probe-outputs-parent`, which printed `eid=… state=SUCCESS \
         o1=hello`; `outputs` is the empty map here because nothing in egret \
         evaluates a flow-level `outputs:` block"
    );
    assert_eq!(
        run_state(&pool, parent, "after").await,
        Some("SUCCESS".to_string()),
        "and the successor runs, so the parent really did wait rather than \
         merely record a terminal"
    );
}

/// **A target that does not resolve fails the RUN, not the flow, and creates no
/// child.**
///
/// Measured on the artifact (`egret.probe/probe-unresolvable`): Kestra
/// published the flow with **200**, executing it left the `Subflow` run FAILED,
/// the successor never ran, the execution FAILED, and no child existed.
///
/// **The decision is what agrees; one shape does not, and it is unreachable.**
/// Kestra's run reads `["CREATED","FAILED"]` with an attempt of `["FAILED"]` --
/// no `RUNNING`, because it never gets as far as building the
/// `SubflowExecution`. egret emits `StartRun` in the same round as the spawn
/// request (`Command::SpawnSubflow` says why it cannot be emitted alone), so it
/// reads `["CREATED","RUNNING","FAILED"]`. Asserted here rather than left as
/// prose, so the divergence is a thing a reader can see and a change to it
/// fails a test. `the_only_unresolvable_subflow_target_in_the_corpus_is_skipped`
/// is why no golden entry can tell.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_target_that_does_not_resolve_fails_the_run_and_creates_no_child(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    // Nothing is filed. The flow still STARTS -- a `build_plan` that refused it
    // would turn `valids/task-runif` from `runs` into `refused`.
    let source = subflow_parent("parent", "nobody-holds-this", "");
    assert!(
        egret_workflow::executor::build_plan(&source).is_ok(),
        "the plan must not refuse a flow for an unresolvable target: nothing at \
         plan time can know what `workflow.flows` holds, and Kestra published \
         the same flow with 200"
    );
    let parent = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        children_by_trigger(&pool, parent).await.len(),
        0,
        "no child may be created for a target that does not resolve"
    );
    assert_eq!(
        (
            run_state(&pool, parent, "sub").await,
            run_message(&pool, parent, "sub").await,
            run_state(&pool, parent, "after").await,
            execution_history(&pool, parent).await,
        ),
        (
            Some("FAILED".to_string()),
            Some("unable to find flow 't'.'nobody-holds-this'".to_string()),
            None,
            serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
        ),
        "the run fails carrying the reason, the successor is never created, and \
         the execution takes the failure -- which is what the artifact did"
    );
    assert_eq!(
        (
            run_history(&pool, parent, "sub").await,
            attempt_histories(&pool, parent, "sub").await,
        ),
        (
            serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
            vec![serde_json::json!(["CREATED", "RUNNING", "FAILED"])],
        ),
        "THE KNOWN DIVERGENCE, pinned rather than described: the artifact reads \
         `[CREATED, FAILED]` with an attempt of `[FAILED]`. egret has already \
         started the run by the time the lookup happens. No golden entry \
         reaches this"
    );
}

/// **A `Subflow` target in ANOTHER workspace does not resolve.**
///
/// `FlowRepo::find_by_name` -- what `publish` resolves a `RefKind::Subflow`
/// through -- is deliberately unscoped, and its own doc records why it cannot
/// be: `WorkflowRefPort::workspace_of` carries no caller-workspace parameter.
/// **This caller has one**, on `ExecutionCtx`, so `worker::subflow::resolve`
/// uses it.
///
/// The test is written so that the ONLY difference from the passing case is the
/// workspace: the same namespace, the same flow id, the same source.
///
/// **This test IS the defence, because there is no other one.** The doc here
/// used to add *"and migration 0138's composite foreign key would then refuse
/// the child row"* — the same false sentence `worker::subflow::resolve` carried,
/// found by the same sweep and corrected in the same commit. It would not:
/// `executions_flow_id_fkey` is one column with no workspace half, and dropping
/// the `workspace_id = $1` predicate makes this test report
/// `(1, Some("SUCCESS"), None)` — the other tenant's flow read AND executed,
/// refused by nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_target_in_another_workspace_does_not_resolve(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (theirs, their_user) = common::seed_workspace(&db).await;
    let (ours, _our_user) = common::seed_workspace(&db).await;
    file_flow(&db, theirs, their_user, &one_task("child")).await;

    let parent = start_execution(
        &pool,
        ours,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        (
            children_by_trigger(&pool, parent).await.len(),
            run_state(&pool, parent, "sub").await,
            run_message(&pool, parent, "sub").await,
        ),
        (
            0,
            Some("FAILED".to_string()),
            Some("unable to find flow 't'.'child'".to_string()),
        ),
        "a flow that exists in another workspace must be as invisible as one \
         that does not exist at all. The SAME source, filed into THIS workspace, \
         is what `a_subflow_child_is_linked_by_the_trigger_columns_and_never_by_parent_execution_id` \
         runs, so a failure here is the scoping and nothing else"
    );
}

/// **A `Subflow` target switched off by its own YAML does not run.** W10 Task 5
/// fix round 2's Critical.
///
/// A flow can be switched off two ways and this path honoured only one. The
/// failure was concrete: an operator writes `disabled: true`, the API's
/// `POST .../flows/{id}/executions` answers 409, and a sibling flow naming that
/// flow as a `Subflow` target **ran it anyway**. `worker::subflow::resolve`
/// tests `state <> 'disabled'` -- the column -- and never parses the YAML;
/// `.disabled` occurred **0** times under `src/worker/` (positive control:
/// `.namespace`, 11 occurrences across 4 files).
///
/// **The control is the same source with the line removed**, so a failure here
/// is `disabled:` and not the fixture: one flow runs, the other does not, and
/// nothing else differs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_target_disabled_by_its_own_yaml_does_not_run(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(
        &db,
        ws,
        user,
        &format!("{}disabled: true\n", one_task("child")),
    )
    .await;

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        (
            children_by_trigger(&pool, parent).await.len(),
            run_state(&pool, parent, "sub").await,
            run_message(&pool, parent, "sub").await,
        ),
        (
            0,
            Some("FAILED".to_string()),
            Some("unable to find flow 't'.'child'".to_string()),
        ),
        "a flow whose own YAML says `disabled: true` must not be reachable as a \
         Subflow target -- and it answers exactly what the column door answers, \
         because one switch must give one answer"
    );

    // The control: the SAME fixture without the `disabled:` line spawns a child
    // and reaches SUCCESS. Without it, a spawn broken for any other reason would
    // pass the assertion above.
    let (ws2, user2) = common::seed_workspace(&db).await;
    file_flow(&db, ws2, user2, &one_task("child")).await;
    let ok = start_execution(
        &pool,
        ws2,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        (
            children_by_trigger(&pool, ok).await.len(),
            run_state(&pool, ok, "sub").await,
        ),
        (1, Some("SUCCESS".to_string())),
        "the control: the identical flow without `disabled:` runs"
    );
}

/// **A `Subflow` target switched off by the `state` COLUMN does not run.**
///
/// `resolve`'s `state <> 'disabled'` predicate. It had **no test**: W10 Task 5
/// fix round 2 measured that deleting the predicate left `--test worker` at
/// **59 passed / 0 failed**, while the positive control on the same statement
/// (dropping `workspace_id = $1`) failed one test -- so the instrument reached
/// that SQL and the predicate was simply undefended.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_target_disabled_by_the_state_column_does_not_run(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let child = file_flow(&db, ws, user, &one_task("child")).await;
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .expect("disable the flow by column");

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        (
            children_by_trigger(&pool, parent).await.len(),
            run_state(&pool, parent, "sub").await,
            run_message(&pool, parent, "sub").await,
        ),
        (
            0,
            Some("FAILED".to_string()),
            Some("unable to find flow 't'.'child'".to_string()),
        ),
        "the state column is the management UI's switch and it must reach this \
         path too"
    );
}

/// **A TRASHED `Subflow` target does not run.**
///
/// `resolve`'s `deleted_at IS NULL` predicate, and it had no test either --
/// removing it as well still left `--test worker` at **59 passed / 0 failed**.
/// `FlowRepo`'s delete is soft, so without this predicate a flow an author put
/// in the trash stays executable by name for ever.
#[sqlx::test(migrations = "../../migrations")]
async fn a_trashed_subflow_target_does_not_run(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let child = file_flow(&db, ws, user, &one_task("child")).await;
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .expect("trash the flow");

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        (
            children_by_trigger(&pool, parent).await.len(),
            run_state(&pool, parent, "sub").await,
            run_message(&pool, parent, "sub").await,
        ),
        (
            0,
            Some("FAILED".to_string()),
            Some("unable to find flow 't'.'child'".to_string()),
        ),
        "a trashed flow is as invisible to a Subflow target as one that never \
         existed"
    );
}

/// **`wait: false` does not wait, and what the parent sees is the child's state
/// at SPAWN time.**
///
/// Measured on the artifact: `egret.probe/probe-nowait-parent`'s successor
/// printed `raw={"state":"CREATED","executionId":"61J0wKdkpWSS8MUTSfOPd8"}` --
/// no `outputs` key, and `CREATED` rather than the `SUCCESS` the child reached a
/// moment later. `valids/subflow-parent-no-wait` is the golden entry, and it is
/// one of the 12 W6 Task 3 banks.
///
/// **The child still runs**, which is the half a `wait: false` implemented as
/// "do nothing" would also pass. So both are asserted.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_that_does_not_wait_records_the_childs_state_at_spawn_and_still_runs_it(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    // A child that FAILS, so "did not wait" and "waited and transmitted" cannot
    // both produce SUCCESS: a parent that waited would take the failure.
    let child_src = "id: child\nnamespace: t\ntasks:\n  - id: boom\n    \
                     type: io.kestra.plugin.core.execution.Fail\n";
    file_flow(&db, ws, user, child_src).await;

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "child", "    wait: false\n"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let kids = children_by_trigger(&pool, parent).await;
    assert_eq!(kids.len(), 1, "the child is still created and driven");
    let (child, _, _, _, child_state) = kids[0].clone();
    assert_eq!(
        (
            child_state.as_str(),
            run_state(&pool, parent, "sub").await,
            run_history(&pool, parent, "sub").await,
            run_outputs(&pool, parent, "sub").await,
            run_state(&pool, parent, "after").await,
        ),
        (
            "FAILED",
            Some("SUCCESS".to_string()),
            serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
            Some(serde_json::json!({
                "executionId": child.to_string(),
                "state": "CREATED",
            })),
            Some("SUCCESS".to_string()),
        ),
        "the child really ran and really FAILED; the parent's run SUCCEEDED \
         anyway, records `CREATED` for it, carries NO `outputs` key, and its \
         successor ran. A `wait: false` that reported the child's terminal would \
         fail this on three of the five"
    );

    // ---- the positive control for the `!sub.wait` guard itself -------------
    //
    // **The case above does not need that guard**, and that was measured
    // rather than assumed: mutation M5a (delete the `!sub.wait` early return in
    // `on_child_terminal`) leaves it GREEN, because the child above needs a
    // worker tick to fail, by which time the parent's run is already terminal
    // and the *already-decided* guard one line below catches it instead. Two
    // guards, one observable behaviour — the shape W4 warned reads identical to
    // a weak guard in a mutation table.
    //
    // This child terminates INSIDE `spawn_one`'s own settle: its only task has
    // an unrenderable `runIf:`, which `render::render_run_if` fails without a
    // handler (the shape `valids/task-runif`'s `willfailedtheflow` has —
    // `[CREATED, FAILED]` with no attempt). So `on_child_terminal` fires while
    // the parent's run is still RUNNING, the already-decided guard cannot
    // apply, and `!sub.wait` is the only thing between a `wait: false` parent
    // and its child's FAILED.
    let instant = "id: child-instant\nnamespace: t\ntasks:\n  - id: x\n    \
                   type: io.kestra.plugin.core.log.Log\n    \
                   runIf: \"{{ outputs.notexists == 2 }}\"\n    message: hi\n";
    file_flow(&db, ws, user, instant).await;
    let fast = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent-fast", "child-instant", "    wait: false\n"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let fast_kids = children_by_trigger(&pool, fast).await;
    assert_eq!(
        fast_kids.len(),
        1,
        "the instant child exists: {fast_kids:?}"
    );
    assert_eq!(
        (
            fast_kids[0].4.as_str(),
            run_history(&pool, fast, "sub").await,
            run_state(&pool, fast, "after").await,
        ),
        (
            "FAILED",
            serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
            Some("SUCCESS".to_string()),
        ),
        "the child FAILED before `spawn_one` returned, and the `wait: false` \
         parent must still SUCCEED. Without the `!sub.wait` guard the child's \
         FAILED arrives while this run is RUNNING and `guess_state` turns it \
         into a FAILED parent"
    );
}

/// **`transmitFailed` decides what a failing child does to its parent, and the
/// two directions are asserted as a PAIR.**
///
/// One direction alone cannot tell a working implementation from a constant:
/// with `transmitFailed` hard-coded true, the second case fails; hard-coded
/// false, the first does. The measurements are
/// `valids/task-flow`'s golden (`transmitFailed: true`, child FAILED, parent
/// run `CREATED RUNNING FAILED`) and `egret.probe/probe-notransmit-parent` on
/// the artifact (`transmitFailed: false`, same child, parent run
/// `CREATED RUNNING FAILED SUCCESS`).
///
/// **The second is the two-step `ExecutableUtils.guessState` produces**, and
/// egret's run history reproduces it. Its ATTEMPT does not:
/// `executor::state::apply` rule 1 stops a terminated attempt taking further
/// state, and that rule's own doc records this as the second measured shape it
/// is wrong for. Unreachable from the corpus --
/// `no_corpus_subflow_turns_transmit_failed_off` derives that -- and asserted
/// here so the boundary is visible rather than remembered.
#[sqlx::test(migrations = "../../migrations")]
async fn transmit_failed_decides_what_a_failing_child_does_to_its_parent(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let child_src = "id: child\nnamespace: t\ntasks:\n  - id: boom\n    \
                     type: io.kestra.plugin.core.execution.Fail\n";
    file_flow(&db, ws, user, child_src).await;

    let on = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("p-on", "child", "    transmitFailed: true\n"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    let off = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("p-off", "child", "    transmitFailed: false\n"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        (
            run_history(&pool, on, "sub").await,
            run_state(&pool, on, "after").await,
            execution_history(&pool, on).await,
        ),
        (
            serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
            None,
            serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
        ),
        "transmitFailed ON: the child's FAILED reaches the parent run, the \
         successor is never created and the execution fails -- `valids/task-flow`"
    );
    assert_eq!(
        (
            run_history(&pool, off, "sub").await,
            attempt_histories(&pool, off, "sub").await,
            run_state(&pool, off, "after").await,
            execution_history(&pool, off).await,
        ),
        (
            serde_json::json!(["CREATED", "RUNNING", "FAILED", "SUCCESS"]),
            vec![serde_json::json!([
                "CREATED", "RUNNING", "FAILED", "SUCCESS"
            ])],
            Some("SUCCESS".to_string()),
            serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        ),
        "transmitFailed OFF: the run takes the child's FAILED first and \
         `guessState`'s SUCCESS after it -- the artifact's two-step, and the \
         ATTEMPT now carries BOTH. **This assertion read \
         `[CREATED, RUNNING, FAILED]` until W8 Task 5**, with a note saying the \
         artifact's carries the SUCCESS too and that `state::apply` rule 1 made \
         it unreachable from the corpus. Both halves were true when written and \
         the second stopped being: `valids/task-allow-warning-executable-flow` \
         is the corpus reaching exactly this shape, through `allowWarning:` \
         rather than through `transmitFailed: false`, so \
         `Command::CompleteRun` now writes the second state onto the closed \
         attempt and `egret.probe/probe-notransmit-parent`'s measurement is \
         reproduced rather than registered as a divergence"
    );
}

/// **A `Subflow` inside a flowable's `errors:` is reached, spawned and run.**
///
/// Measured on the artifact with `egret.probe/probe-errors-subflow`: the
/// `handler` run reads `CREATED RUNNING SUCCESS` and a child execution exists.
///
/// It is the shape `spec::validate` and `publish::resolve_task` both miss --
/// they walk `children()` only -- and `worker::subflow::subflow_of` walks
/// `error_children()` for exactly this. No corpus flow reaches it, which is why
/// `driver::subflow_targets` could measure its own wider walk as redundant;
/// here it is not, and this is the test that says so.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_inside_an_errors_list_is_reached_and_spawned(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(&db, ws, user, &one_task("child")).await;

    let source = "id: errors-subflow\nnamespace: t\ntasks:\n  - id: seq\n    \
                  type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      \
                  - id: boom\n        type: io.kestra.plugin.core.execution.Fail\n    \
                  errors:\n      - id: handler\n        \
                  type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n        \
                  flowId: child\n";
    let parent = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    assert_eq!(
        (
            children_by_trigger(&pool, parent).await.len(),
            run_history(&pool, parent, "handler").await,
        ),
        (1, serde_json::json!(["CREATED", "RUNNING", "SUCCESS"])),
        "a walk that stopped at `children()` would answer `None` here and report \
         a corruption for a flow the artifact merely finds unusual"
    );
}

/// **A cycle of `Subflow`s stops instead of taking the worker down.**
///
/// The positive control for `worker::subflow`'s cycle guard, which is the one
/// answer that module gives that is **not** the artifact's. `spec::validate`'s
/// `self_dependency` refuses a `Subflow` naming its own flow —
/// `invalids/recursive-flow` is that flow, and Kestra refuses it too — but
/// **neither side refuses A → B → A**, and a resolver that looks its target up
/// by name cannot see a cycle from one flow. Kestra would create executions for
/// ever, one message at a time.
///
/// egret cannot: `spawn_one` drives the child's first settle inside its
/// caller's transaction, so a cycle is unbounded async recursion. **Measured
/// before the guard was written**: this exact pair overflowed the test thread's
/// stack and aborted the process with SIGABRT, and it did so with a depth bound
/// of 32 AND with one of 16 — which is why the guard is a cycle check rather
/// than a depth bound. A number large enough to be worth having is one this
/// recursion cannot survive.
///
/// **Both halves are asserted**, because a guard that stopped the recursion and
/// left the run RUNNING would look identical here to one that worked.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cycle_of_subflows_stops_instead_of_crashing(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    // A → B → A. Neither names itself, so `self_dependency` passes on both and
    // both publish; the cycle only exists across the pair.
    let a = "id: cyc-a\nnamespace: t\ntasks:\n  - id: sub\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: cyc-b\n";
    let b = "id: cyc-b\nnamespace: t\ntasks:\n  - id: sub\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: cyc-a\n";
    file_flow(&db, ws, user, a).await;
    file_flow(&db, ws, user, b).await;

    let root = start_execution(&pool, ws, None, a, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    // **Ordered by the CHAIN, not by `created_at`.** The first draft of this
    // ordered by `(created_at, id)` and passed alone and failed in the full
    // suite: `now()` in Postgres is TRANSACTION time, so a parent and the child
    // its own settle creates share the instant to the microsecond and the tie
    // fell to a random uuid. That is the same fact `worker::claim`'s
    // `ORDER BY created_at, execution_id, run_ix` exists for, met one table over.
    let rows: Vec<ChainRow> = sqlx::query_as(
        "WITH RECURSIVE down AS ( \
             SELECT id, 0 AS depth FROM workflow.executions WHERE id = $1 \
           UNION ALL \
             SELECT e.id, down.depth + 1 FROM workflow.executions e \
               JOIN down ON e.trigger_execution_id = down.id \
         ) \
         SELECT e.id, e.flow_key, e.state, r.state, r.message \
           FROM down JOIN workflow.executions e ON e.id = down.id \
                     JOIN workflow.task_runs r ON r.execution_id = e.id \
          ORDER BY down.depth",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .expect("walk the chain from the root");
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&pool)
        .await
        .expect("count executions");
    assert_eq!(
        (rows.len(), all),
        (2, 2),
        "the root and ONE child, on the chain and in the table: {rows:?}"
    );
    assert_eq!(rows[0].0, root, "the chain starts at the root");
    assert_eq!(
        (
            rows[1].1.as_str(),
            rows[1].2.as_str(),
            rows[1].3.as_deref(),
            rows[1].4.as_deref()
        ),
        (
            "cyc-b",
            "FAILED",
            Some("FAILED"),
            Some(
                "subflow 't'.'cyc-a' is already running higher up this execution's \
                 own chain; starting it would loop"
            )
        ),
        "the child that would have closed the cycle must FAIL and say why. \
         Leaving it RUNNING would stop the recursion and look identical in a count"
    );
    assert_eq!(
        rows[0].2.as_str(),
        "FAILED",
        "and `transmitFailed` carries it back to the root, which is what makes \
         the refusal visible to whoever started the flow"
    );
}

/// **A cycle whose every hop crosses a `Loop` boundary.**
///
/// The shape the review reproduced with the first cycle guard shipped and
/// enabled: two flows, each a one-value `Loop` whose body is a `Subflow` naming
/// the other. It aborted the process with `SIGABRT`.
///
/// **Why the first guard missed it, and why the miss was structural.** That
/// guard walked `trigger_execution_id` only. A `Loop` sub-execution is linked
/// by `parent_execution_id` and its `trigger_execution_id` is NULL — not by
/// accident but by `workflow_executions_one_link_kind`, the constraint W6 Task
/// 2 added to make carrying both unrepresentable. So the walk **truncated at
/// every loop boundary**, and no single walked segment ever contained a repeat.
/// The guard's premise was that a causal chain is walkable by one link kind;
/// migration 0138 guarantees it is not.
///
/// The control that makes the diagnosis certain is the sibling test
/// [`a_cycle_of_subflows_stops_instead_of_crashing`], where neither hop crosses
/// a loop and the first guard caught it — so what distinguishes them is the
/// link kind and nothing else.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cycle_whose_every_hop_crosses_a_loop_boundary_stops_too(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let a = "id: cyl-a\nnamespace: t\ntasks:\n  - id: each\n    \
             type: io.kestra.plugin.core.flow.Loop\n    values: '[\"1\"]'\n    tasks:\n      \
             - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        \
             namespace: t\n        flowId: cyl-b\n";
    let b = "id: cyl-b\nnamespace: t\ntasks:\n  - id: each\n    \
             type: io.kestra.plugin.core.flow.Loop\n    values: '[\"1\"]'\n    tasks:\n      \
             - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        \
             namespace: t\n        flowId: cyl-a\n";
    file_flow(&db, ws, user, a).await;
    file_flow(&db, ws, user, b).await;

    let root = start_execution(&pool, ws, None, a, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    // **The chain by NAME and in order, not a bare count**, because the count
    // alone would not say the walk crossed the loop links at all. It is read
    // through the same `COALESCE(trigger_execution_id, parent_execution_id)`
    // the guard walks, so a chain this query can order is a chain the guard
    // could see.
    let chain: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE down AS ( \
             SELECT id, flow_key, trigger_execution_id IS NOT NULL AS by_trigger, \
                    0 AS depth \
               FROM workflow.executions WHERE id = $1 \
           UNION ALL \
             SELECT e.id, e.flow_key, e.trigger_execution_id IS NOT NULL, down.depth + 1 \
               FROM workflow.executions e \
               JOIN down ON down.id = COALESCE(e.trigger_execution_id, e.parent_execution_id) \
         ) \
         SELECT flow_key || (CASE WHEN by_trigger THEN ' (subflow)' ELSE ' (loop)' END) \
           FROM down ORDER BY depth",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .expect("walk the chain");
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&pool)
        .await
        .expect("count executions");
    let refused: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT r.state, r.message FROM workflow.task_runs r \
          WHERE r.message IS NOT NULL ORDER BY r.created_at, r.execution_id",
    )
    .fetch_all(&pool)
    .await
    .expect("read the refusals");
    assert_eq!(
        (chain, all, refused.len()),
        (
            vec![
                "cyl-a (loop)".to_string(),
                "cyl-a (loop)".to_string(),
                "cyl-b (subflow)".to_string(),
                "cyl-b (loop)".to_string(),
            ],
            4,
            1
        ),
        "the whole chain: the root, its LOOP child, the `cyl-b` SUBFLOW child, \
         and ITS loop child -- and then the spawn back to `cyl-a` is refused. \
         Note the first row's label is the root's own (it carries neither link); \
         what matters is that two of the three links are loop links, which is \
         exactly what the first guard could not see. Four executions and no more"
    );
    assert_eq!(
        (refused[0].0.as_str(), refused[0].1.as_deref()),
        (
            "FAILED",
            Some(
                "subflow 't'.'cyl-a' is already running higher up this execution's \
                 own chain; starting it would loop"
            )
        ),
        "and the run that would have closed it FAILS carrying the reason"
    );
    assert_eq!(
        run_state(&pool, root, "each").await,
        Some("FAILED".to_string()),
        "the refusal reaches the root through the loop's own `transmitFailed`"
    );
}

/// Run `body` on a thread whose stack is **2 MiB, tokio's worker default**
/// (`tokio::runtime::Builder::thread_stack_size`), on its own current-thread
/// runtime and its own pool to the same database.
///
/// The two subflow-chain tests below are the stack's tripwire (W9a final
/// review): `settle` re-enters itself down a `Subflow` chain, and what they
/// prove is that `MAX_SPAWN_CHAIN` levels fit in a production worker's stack.
/// Run on the test harness's own thread they would prove it for whatever
/// `RUST_MIN_STACK` the runner happened to export (the harness sizes its
/// threads from it); pinned here, a future that grows past 2 MiB at the bound
/// aborts this binary on every machine.
async fn on_a_tokio_worker_stack<F, Fut>(pool: sqlx::PgPool, body: F)
where
    F: FnOnce(sqlx::PgPool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    const TOKIO_WORKER_STACK: usize = 2 * 1024 * 1024;
    let options = (*pool.connect_options()).clone();
    let thread = std::thread::Builder::new()
        .name("tokio-worker-stack".into())
        .stack_size(TOKIO_WORKER_STACK)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(async move {
                    let pool = sqlx::postgres::PgPoolOptions::new()
                        .max_connections(4)
                        .connect_with(options)
                        .await
                        .expect("connect to the test database");
                    body(pool.clone()).await;
                    pool.close().await;
                })
        })
        .expect("spawn the 2 MiB thread");
    let joined = tokio::task::spawn_blocking(move || thread.join())
        .await
        .expect("join");
    if let Err(panic) = joined {
        std::panic::resume_unwind(panic);
    }
}

/// **A chain deeper than `MAX_SPAWN_CHAIN` is refused rather than overflowing a
/// stack.**
///
/// The positive control for the backstop beside the cycle guard, and the reason
/// the SIGABRT reproduction above can live in this suite at all: with a bound in
/// place a regression of the cycle guard fails an assertion instead of aborting
/// the test binary, which takes every other test in it with it.
///
/// **This is the class fix round 1 said was left open**, and the report's
/// earlier sentence — *"a depth bound large enough to be worth having is not one
/// this recursion can survive"* — was a measurement about a bound used as THE
/// guard generalised to one used as a backstop. As a backstop it only has to sit
/// between what is real (the corpus's deepest chain is **2 links**) and what
/// kills a stack (**16 was measured to abort**). 8 is inside both.
///
/// The chain is `dep-0 → dep-1 → … → dep-11`, twelve DISTINCT flows, so the
/// cycle guard has nothing to say about it and only the bound can stop it.
///
/// **This covers the DESCENT only, and that is a limit of it rather than a
/// property of the bound.** Its chain never completes — it runs straight down
/// through nested `spawn_one`s and hits the bound before any child terminates —
/// so it could not see fix round 2's Critical, where the counter was reset on
/// every re-entrant load and a *completing* chain walked past the bound into a
/// stack overflow. The ascent is
/// [`a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter`], and
/// the two are a pair: mutation M10 (the re-entrant loads say `Stack::Fresh`
/// again) kills that one and leaves this one green.
#[sqlx::test(migrations = "../../migrations")]
async fn a_chain_of_subflows_deeper_than_the_bound_is_refused_rather_than_overflowing(
    pool: sqlx::PgPool,
) {
    on_a_tokio_worker_stack(
        pool,
        a_chain_of_subflows_deeper_than_the_bound_is_refused_rather_than_overflowing_body,
    )
    .await;
}

/// [`a_chain_of_subflows_deeper_than_the_bound_is_refused_rather_than_overflowing`]'s body, run by [`on_a_tokio_worker_stack`].
async fn a_chain_of_subflows_deeper_than_the_bound_is_refused_rather_than_overflowing_body(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let deeper_than_the_bound = egret_workflow::worker::MAX_SPAWN_CHAIN + 4;
    for i in 0..deeper_than_the_bound {
        let src = format!(
            "id: dep-{i}\nnamespace: t\ntasks:\n  - id: sub\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
             flowId: dep-{}\n",
            i + 1
        );
        file_flow(&db, ws, user, &src).await;
    }
    // The last one in the chain names a flow that does not exist, so if the
    // bound ever stopped firing this test would fail on the count rather than
    // run away -- and the message would be the unresolvable one, which is a
    // different assertion failure and not the same as silence.
    let root_src = "id: dep-0\nnamespace: t\ntasks:\n  - id: sub\n    \
                    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
                    flowId: dep-1\n";

    let root = start_execution(&pool, ws, None, root_src, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&pool)
        .await
        .expect("count executions");
    assert_eq!(
        all,
        egret_workflow::worker::MAX_SPAWN_CHAIN as i64 + 1,
        "the root plus exactly MAX_SPAWN_CHAIN children, and then it stops"
    );

    let refused: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT r.state, r.message FROM workflow.task_runs r WHERE r.message IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("read the refusals");
    assert_eq!(
        refused.len(),
        1,
        "exactly one run is refused -- the one at the bound: {refused:?}"
    );
    assert_eq!(
        (refused[0].0.as_str(), refused[0].1.as_deref()),
        (
            "FAILED",
            Some(
                "this execution is 8 child execution(s) down a chain and 8 is as \
                 deep as this worker will build in one transaction; 't'.'dep-9' \
                 is not started"
            )
        ),
        "and it FAILS carrying the bound and the target it did not start. A \
         backstop that stopped the recursion and left the run RUNNING would look \
         identical in the count above"
    );
    assert_eq!(
        run_state(&pool, root, "sub").await,
        Some("FAILED".to_string()),
        "and `transmitFailed` carries the refusal all the way back to the root"
    );
}

/// **The same counter, on the ASCENT — where a completing chain climbs the
/// stack and the counter used to be reset to zero on every link.**
///
/// # Why the sibling test above cannot see this, in the reviewer's own words
///
/// *"`a_chain_of_subflows_deeper_than_the_bound…` cannot see this defect
/// because its chain never completes."* It runs straight down through nested
/// `spawn_one`s, where the `caller + 1` arithmetic was always right, and hits
/// the bound before any child terminates. **A test that exercises only the
/// descent cannot see a defect in the ascent** — and the ascent is where a
/// `wait: true` subflow actually spends its life, because that is how it
/// resumes its parent.
///
/// # The defect this pins
///
/// `claim::load_with` wrote `depth: 0` for every caller, under a comment
/// asserting that no caller that loads an execution is inside another
/// `settle`. Two are: `subflow::complete` and `spawn::on_execution_terminal`
/// are both reached from `claim::settle`. So a completing chain reset the
/// counter once per link — the reviewer instrumented it and watched it cycle
/// `0,1,2,0,1,2,…` — and **nine flows filed in one workspace, plain
/// `wait: true`, aborted the worker process with
/// `fatal runtime error: stack overflow`**. `claim::Stack` is the fix: a load
/// must SAY which of the two it is.
///
/// # The shape, and why each part of it is needed
///
/// Each `asc-i` runs two subflows in sequence:
///
/// * `leaf` → `asc-leaf`, a flow whose only task carries an unrenderable
///   `runIf:` so it FAILS **inside `spawn_one`'s own settle**, with no worker
///   tick in between. That is what makes the ascent happen inside the descent's
///   frames rather than on a fresh stack — the same fixture the `wait: false`
///   positive control uses, and for the same reason.
/// * `transmitFailed: false` on it, so `guess_state` turns that FAILED into a
///   SUCCESS and the execution carries on to the second task instead of ending
///   there. Without it the ascent stops at the first leaf and the flow never
///   descends again.
/// * `next` → `asc-{i+1}`, which is the link that must be refused.
///
/// So one level costs three frames — spawn the leaf, re-enter through
/// `complete`, spawn the next — and `MAX_SPAWN_CHAIN` is reached partway down
/// the third `asc-`. **`asc-4` is deliberately NOT filed**, so a counter that
/// never fires runs to a *different* refusal (`unable to find flow 't'.'asc-4'`)
/// and one more execution rather than running away: the mutation that reverts
/// the threading fails this as an ASSERTION with the process intact, which is
/// the whole reason the numbers here are small.
#[sqlx::test(migrations = "../../migrations")]
async fn a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter(pool: sqlx::PgPool) {
    on_a_tokio_worker_stack(
        pool,
        a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter_body,
    )
    .await;
}

/// [`a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter`]'s body, run by [`on_a_tokio_worker_stack`].
async fn a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter_body(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;

    // Terminates inside its own first settle: `render::render_run_if` fails
    // with no handler, so there is no worker tick between the spawn and the
    // callback that walks back up.
    file_flow(
        &db,
        ws,
        user,
        "id: asc-leaf\nnamespace: t\ntasks:\n  - id: x\n    \
         type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ outputs.notexists == 2 }}\"\n    message: hi\n",
    )
    .await;
    // Four levels. `asc-4` is never filed -- see the doc.
    for i in 0..4 {
        let src = format!(
            "id: asc-{i}\nnamespace: t\ntasks:\n  - id: leaf\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
             flowId: asc-leaf\n    transmitFailed: false\n  - id: next\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
             flowId: asc-{}\n",
            i + 1
        );
        file_flow(&db, ws, user, &src).await;
    }

    let root_src = "id: asc-0\nnamespace: t\ntasks:\n  - id: leaf\n    \
                    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
                    flowId: asc-leaf\n    transmitFailed: false\n  - id: next\n    \
                    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
                    flowId: asc-1\n";
    let root = start_execution(&pool, ws, None, root_src, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    // By NAME and in order, not by count: a count alone cannot say the chain
    // stopped at the right link, and `asc-3` appearing here is exactly what the
    // unthreaded counter allowed.
    //
    // **Ordered by a walk of the chain, never by `created_at`.** All six rows
    // are written in ONE transaction and `now()` in Postgres is transaction
    // time, so they share the instant to the microsecond and the tie falls to a
    // random uuid -- Concern 13 of this task's report, met a second time. The
    // walk orders by `(depth, trigger_run_ix)`: `leaf` is run 0 and `next` is
    // run 1 at every level, so the sequence below is a fact about the flow.
    let names: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE down AS ( \
             SELECT id, 0 AS depth, -1 AS ix FROM workflow.executions WHERE id = $1 \
           UNION ALL \
             SELECT e.id, down.depth + 1, e.trigger_run_ix \
               FROM workflow.executions e JOIN down ON e.trigger_execution_id = down.id \
         ) \
         SELECT e.flow_key FROM down JOIN workflow.executions e ON e.id = down.id \
          ORDER BY down.depth, down.ix",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .expect("walk the chain from the root");
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&pool)
        .await
        .expect("count executions");
    assert_eq!(
        all as usize,
        names.len(),
        "the walk reaches every execution there is, so the list below is the \
         whole of what ran and not a subgraph of it"
    );
    assert_eq!(
        names,
        vec!["asc-0", "asc-leaf", "asc-1", "asc-leaf", "asc-2", "asc-leaf",],
        "three levels and their three leaves, and then it stops. `asc-3` in \
         this list is the defect: it means the counter was reset on the way \
         back up and the stack kept climbing"
    );

    let refused: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT r.state, r.message FROM workflow.task_runs r WHERE r.message IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("read the refusals");
    assert_eq!(
        refused.len(),
        1,
        "exactly one run is refused -- the one at the bound: {refused:?}"
    );
    assert_eq!(
        (refused[0].0.as_str(), refused[0].1.as_deref()),
        (
            "FAILED",
            Some(
                "this execution is 8 child execution(s) down a chain and 8 is as \
                 deep as this worker will build in one transaction; 't'.'asc-3' \
                 is not started"
            )
        ),
        "and it is the DEPTH refusal naming `asc-3`. `unable to find flow \
         't'.'asc-4'` here would mean the chain ran one level further than the \
         bound allows and stopped only because the fixture ran out of flows"
    );
    assert_eq!(
        run_state(&pool, root, "next").await,
        Some("FAILED".to_string()),
        "and `transmitFailed` carries the refusal back through every level to \
         the root"
    );
}

/// **The other two re-entrant sites: a `Loop`'s next iteration and a
/// `wait: false` subflow's own completion are frames too.**
///
/// # Why this test exists, and it is a mutation result rather than a hunch
///
/// The threading fix touched **three** call sites, and the ascent test above
/// covers exactly one of them. Measured, per site:
///
/// | mutation | site | ascent test | descent test |
/// |---|---|---|---|
/// | M10a | `subflow::on_child_terminal` (`wait: true`) | KILLED | passes |
/// | M10b | `subflow::complete` from `spawn_one` (`wait: false`, and every refusal) | **SURVIVED** | passes |
/// | M10c | `spawn::on_execution_terminal` (a `Loop`'s next iteration) | **SURVIVED** | passes |
///
/// Two threaded sites with nothing watching them is the shape the programme's
/// own rule is about, and the `wait: false` one is not hypothetical — the
/// reviewer's `wait: false` probe aborted the worker at **eight** levels, two
/// fewer than the `wait: true` one. This test is the positive control for both.
///
/// # The frame arithmetic, which is what the fixture is built from
///
/// Every hop below happens inside `start_execution`'s single transaction,
/// because nothing here ever needs a worker: the loop body and the subflow
/// target both carry an unrenderable `runIf:` and fail inside their own first
/// settle.
///
/// ```text
/// settle(root, 0)
///   each  -> iteration 0 at 1 -> FAILED -> on_execution_terminal(caller 1)
///                             -> load root at 2 -> start iteration 1 at 3 -> FAILED
///                             -> on_execution_terminal(caller 3)
///                             -> load root at 4 -> start iteration 2 at 5 -> FAILED
///                             -> on_execution_terminal(caller 5)
///                             -> load root at 6 -> every value terminated -> settle(root, 6)
///   fire1 -> child at 7 -> FAILED, but `wait: false` -> complete(Nested(6)) -> settle(root, 7)
///   fire2 -> child at 8 -> FAILED, but `wait: false` -> complete(Nested(7)) -> settle(root, 8)
///   deep  -> 8 + 1 > MAX_SPAWN_CHAIN  ->  REFUSED
/// ```
///
/// `transmitFailed: false` on the `Loop` is what lets the flow reach `fire1` at
/// all — every iteration FAILS, and a `transmitFailed` loop would end on the
/// first one (`valids/loop-failed` against `valids/loop-failed-no-transmit`).
///
/// **Each surviving mutation moves the refusal off `deep` in a different
/// direction**, which is why one test kills both: M10b flattens the two
/// `wait: false` hops (`deep` reaches `spawn_one` at 2 instead of 8) and M10c
/// flattens the three loop hops (`deep` reaches it at 2), and in both cases the
/// child is created and there is no refusal at all.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_ascent_and_a_wait_false_subflow_are_frames_too(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;

    // Fails inside its own first settle, with no worker tick -- the same
    // fixture the `wait: false` positive control and the ascent test use.
    let instant = "id: asc-leaf\nnamespace: t\ntasks:\n  - id: x\n    \
                   type: io.kestra.plugin.core.log.Log\n    \
                   runIf: \"{{ outputs.notexists == 2 }}\"\n    message: hi\n";
    file_flow(&db, ws, user, instant).await;

    let root_src = "id: mix\nnamespace: t\ntasks:\n  \
                    - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    \
                      values: [\"1\", \"2\", \"3\"]\n    transmitFailed: false\n    \
                      tasks:\n      - id: body\n        \
                        type: io.kestra.plugin.core.log.Log\n        \
                        runIf: \"{{ outputs.notexists == 2 }}\"\n        message: hi\n  \
                    - id: fire1\n    type: io.kestra.plugin.core.flow.Subflow\n    \
                      namespace: t\n    flowId: asc-leaf\n    wait: false\n  \
                    - id: fire2\n    type: io.kestra.plugin.core.flow.Subflow\n    \
                      namespace: t\n    flowId: asc-leaf\n    wait: false\n  \
                    - id: deep\n    type: io.kestra.plugin.core.flow.Subflow\n    \
                      namespace: t\n    flowId: asc-leaf\n";
    let root = start_execution(&pool, ws, None, root_src, serde_json::json!({}))
        .await
        .expect("start execution");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let refused: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT r.state, r.message FROM workflow.task_runs r WHERE r.message IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("read the refusals");
    assert_eq!(
        (
            run_state(&pool, root, "each").await,
            run_state(&pool, root, "fire1").await,
            run_state(&pool, root, "fire2").await,
            run_state(&pool, root, "deep").await,
            refused.len(),
        ),
        (
            Some("SUCCESS".to_string()),
            Some("SUCCESS".to_string()),
            Some("SUCCESS".to_string()),
            Some("FAILED".to_string()),
            1,
        ),
        "the loop and both `wait: false` subflows run; only `deep` is refused. \
         `deep` SUCCEEDing here means its frames were not counted -- which is \
         what both surviving mutations produce: {refused:?}"
    );
    assert_eq!(
        refused[0].1.as_deref(),
        Some(
            "this execution is 8 child execution(s) down a chain and 8 is as \
             deep as this worker will build in one transaction; 't'.'asc-leaf' \
             is not started"
        ),
        "and the refusal is the DEPTH one, at exactly 8 -- three loop hops, two \
         `wait: false` hops, and the two frames each of those costs. A different \
         number here means the arithmetic in this test's doc is wrong, not that \
         the bound is"
    );
    // Three loop iterations and two `wait: false` children, and NOT a sixth:
    // the refusal must happen before `deep`'s child row is written.
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&pool)
        .await
        .expect("count executions");
    assert_eq!(
        all, 6,
        "the root, three iterations, and two `wait: false` children"
    );
}

/// The test-only effect log [`the_walk_runs_each_started_settle_where_the_inline_call_ran`]
/// reads: one row per effect, numbered by a sequence, so the numbering is the
/// order the walk wrote them in, across every execution of one transaction.
///
/// Triggers, because nothing egret writes is globally ordered except the
/// Flow-trigger outbox's `id`: `executions.created_at` is transaction time
/// (one instant for the whole walk), `seq` is per execution, and ids are
/// random. `#[sqlx::test]` gives this test its own database, so the triggers
/// are seen by nothing else.
async fn record_effects(pool: &sqlx::PgPool) {
    for ddl in [
        "CREATE TABLE public.w9c_effects (n bigserial PRIMARY KEY, execution uuid NOT NULL, what text NOT NULL)",
        "CREATE FUNCTION public.w9c_execution_effect() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF TG_OP = 'INSERT' THEN \
             INSERT INTO public.w9c_effects (execution, what) VALUES (NEW.id, 'created'); \
           ELSIF OLD.state IS DISTINCT FROM NEW.state THEN \
             INSERT INTO public.w9c_effects (execution, what) VALUES (NEW.id, NEW.state); \
           END IF; \
           RETURN NEW; \
         END $$",
        "CREATE FUNCTION public.w9c_run_effect() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF TG_OP = 'INSERT' OR OLD.state IS DISTINCT FROM NEW.state THEN \
             INSERT INTO public.w9c_effects (execution, what) \
               VALUES (NEW.execution_id, NEW.task_id || ' ' || NEW.state); \
           END IF; \
           RETURN NEW; \
         END $$",
        "CREATE FUNCTION public.w9c_event_effect() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           INSERT INTO public.w9c_effects (execution, what) \
             VALUES (NEW.source_execution_id, 'event ' || NEW.state); \
           RETURN NEW; \
         END $$",
        "CREATE TRIGGER w9c_execution_effect AFTER INSERT OR UPDATE ON workflow.executions \
         FOR EACH ROW EXECUTE FUNCTION public.w9c_execution_effect()",
        "CREATE TRIGGER w9c_run_effect AFTER INSERT OR UPDATE ON workflow.task_runs \
         FOR EACH ROW EXECUTE FUNCTION public.w9c_run_effect()",
        "CREATE TRIGGER w9c_event_effect AFTER INSERT ON workflow.flow_trigger_events \
         FOR EACH ROW EXECUTE FUNCTION public.w9c_event_effect()",
    ] {
        sqlx::query(ddl)
            .execute(pool)
            .await
            .unwrap_or_else(|e| panic!("{ddl}: {e}"));
    }
}

/// **The order of effects across one walk** (W9c Task 9 re-review 3, N1).
///
/// Fix round 3 turned every settle a head starts -- a `Loop` batch's first
/// settles, a `Subflow` child's first settle, a refused `Subflow`'s
/// completion -- into a `claim::Job` the walk runs, and argued the ORDER is
/// the inline calls' order by construction: a frame runs its queued jobs,
/// each to the end of its own tail, before its next phase. The golden cannot
/// see this axis. This pins it on one `Parallel` holding a `Loop` (both
/// iterations in one batch), a refused `Subflow` and a `Subflow` whose child
/// ends in its first settle, all inside the start's one transaction:
///
/// - the loop's iterations settle -- and the loop ends -- BEFORE the
///   `Subflow` phase starts;
/// - the refused `Subflow`'s completion lands BEFORE its sibling's child is
///   created;
/// - the child's whole first settle and its completion land before the
///   parent's own outbox.
///
/// **The Flow-trigger ids alone cannot see a reordering of the queue.** Every
/// event is written by an `Outbox` phase, and a frame whose jobs were
/// deferred to its `Outbox` phase (mutant O1) still runs them, in the same
/// order, before that phase's outbox -- so the event sequence is identical
/// under O1, measured. What moves is where a head's own writes (a child's
/// row) fall relative to the jobs, so the whole effect log is asserted:
/// creations, state changes and events.
#[sqlx::test(migrations = "../../migrations")]
async fn the_walk_runs_each_started_settle_where_the_inline_call_ran(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(
        &db,
        ws,
        user,
        &listener(
            "ord-hears",
            "    states: [RUNNING, SUCCESS, FAILED, WARNING]\n",
        ),
    )
    .await;
    // Ends inside its own first settle: an `If` that is false with no `else`.
    let leaf = "id: ord-leaf\nnamespace: t\ntasks:\n  - id: gate\n    \
                type: io.kestra.plugin.core.flow.If\n    condition: \"{{ false }}\"\n    \
                then:\n      - id: never\n        type: io.kestra.plugin.core.log.Log\n        \
                message: no\n";
    file_flow(&db, ws, user, leaf).await;
    record_effects(&pool).await;

    let root_src = "id: ord\nnamespace: t\ntasks:\n  \
                    - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
                      - id: each\n        type: io.kestra.plugin.core.flow.Loop\n        \
                        values: [\"1\", \"2\"]\n        concurrencyLimit: 0\n        tasks:\n          \
                          - id: body\n            type: io.kestra.plugin.core.flow.If\n            \
                            condition: \"{{ false }}\"\n            then:\n              \
                              - id: never\n                type: io.kestra.plugin.core.log.Log\n                \
                                message: no\n      \
                      - id: refused\n        type: io.kestra.plugin.core.flow.Subflow\n        \
                        namespace: t\n        flowId: missing\n      \
                      - id: call\n        type: io.kestra.plugin.core.flow.Subflow\n        \
                        namespace: t\n        flowId: ord-leaf\n";
    let root = start_execution(&pool, ws, None, root_src, serde_json::json!({}))
        .await
        .expect("start execution");

    // Name every execution: the root, the iterations by index, the child.
    let names: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, CASE WHEN id = $1 THEN 'root' \
                         WHEN loop_index IS NOT NULL THEN 'it' || loop_index \
                         ELSE flow_key END \
           FROM workflow.executions",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .expect("name the executions");
    let name = |id: Uuid| {
        names
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let log: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT execution, what FROM public.w9c_effects ORDER BY n")
            .fetch_all(&pool)
            .await
            .expect("read the effect log");
    let trace: Vec<String> = log
        .into_iter()
        .map(|(e, what)| format!("{}: {what}", name(e)))
        .collect();
    assert_eq!(
        trace,
        [
            // The root's `Begin`: persist, the `Parallel` starts all three.
            "root: created",
            "root: par RUNNING",
            "root: each RUNNING",
            "root: refused RUNNING",
            "root: call RUNNING",
            "root: RUNNING",
            // `Loops`: the batch's rows first (fix round 2), then each
            // iteration's whole settle, and the loop ends in the second's tail.
            "it0: created",
            "it1: created",
            "it0: body SUCCESS",
            "it0: SUCCESS",
            "it0: event RUNNING",
            "it0: event SUCCESS",
            "it1: body SUCCESS",
            "it1: SUCCESS",
            "it1: event RUNNING",
            "it1: event SUCCESS",
            "root: each SUCCESS",
            // `Subflows`, request 1: the refusal's completion -- a nested
            // settle of the root, which fails the `Parallel` and the root --
            // lands BEFORE request 2 runs at all.
            "root: par FAILED",
            "root: refused FAILED",
            "root: call FAILED",
            "root: FAILED",
            "root: event FAILED",
            // Request 2: the child is still created (the top frame's request
            // list is fixed at its `Begin`) and its whole first settle runs.
            "ord-leaf: created",
            "ord-leaf: gate SUCCESS",
            "ord-leaf: SUCCESS",
            "ord-leaf: event RUNNING",
            "ord-leaf: event SUCCESS",
            // The root's own `Outbox` last: it appends `RUNNING` relative to
            // its own `before`, after the nested settle's `FAILED`.
            "root: event RUNNING",
        ],
        "the walk must run every settle a head started where the inline call \
         ran it: a frame's queued jobs before its next phase. Under mutant O1 \
         (jobs deferred to the `Outbox` phase) `ord-leaf: created` moves up \
         before `it0: body SUCCESS`; the events alone do not move"
    );
}

// ---------------------------------------------------------------------------
// W6 Task 4: the `CREATE_NEW_EXECUTION` replay, at the seams the oracle cannot
// reach
// ---------------------------------------------------------------------------

/// A flow whose only task always fails, carrying a `CREATE_NEW_EXECUTION`
/// retry.
///
/// `maxAttempts` is a parameter because the two things worth driving here are
/// *the chain stops* and *the chain continues*, and one flow that did both
/// would be two tests wearing one fixture.
fn one_task_replaying(id: &str, interval: &str, max_attempts: u32) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
         \x20     type: constant\n      interval: {interval}\n      maxAttempts: {max_attempts}\n"
    )
}

/// The same policy on a task inside a one-value `Loop`.
///
/// The body is what fails, so the retry is decided inside the **sub-execution**
/// — which is the row `worker::replay::spawn_due` refuses, because the artifact
/// refuses it too.
fn loop_body_replaying(id: &str, interval: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: [\"v1\"]\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.log.Log\n\
         \x20       message: hi\n        retry:\n          behavior: CREATE_NEW_EXECUTION\n\
         \x20         type: constant\n          interval: {interval}\n          maxAttempts: 3\n"
    )
}

/// A [`Registry`] that fails every attempt of every run.
///
/// `FailsOnce` cannot be reused: a `CREATE_NEW_EXECUTION` chain gives every
/// execution a FRESH run whose `attempt` is 1, so "fail the first attempt"
/// would fail every one of them anyway and the name would be a lie about what
/// the fixture does.
#[derive(Default)]
struct AlwaysFails {
    seen: Mutex<Vec<String>>,
}

impl AlwaysFails {
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Registry for AlwaysFails {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.seen.lock().unwrap().push(task.task_id.clone());
        Ok(Outcome::failed_with("this fixture always fails"))
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "a handler that returned its own failure must not be killed: {}",
            task.task_id
        );
    }
}

/// The replay hanging off an execution, if there is one: a child linked by
/// `parent_execution_id` with **no** loop index.
async fn replay_of(pool: &sqlx::PgPool, execution: Uuid) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(execution)
    .fetch_optional(pool)
    .await
    .expect("read the replay")
}

/// `(attempt_number, origin_offset_ms)` — Kestra's `ExecutionMetadata`,
/// migration 0139.
async fn metadata_of(pool: &sqlx::PgPool, execution: Uuid) -> (i32, i64) {
    sqlx::query_as("SELECT attempt_number, origin_offset_ms FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read the metadata")
}

/// **The waker's `RETRIED` arm: a due replay is woken, one still waiting is
/// left alone, and once the replacement exists the waker stops naming it.**
///
/// This is the shipping worker's half of the replay and **the oracle cannot see
/// any of it**: `oracle-exec/driver.rs` makes the delay elapse by rewinding
/// `executions.created_at` and calling `worker::resume` itself, so a
/// `due_retries` that named nobody would leave the ratchet at **127** (131 when
/// this sentence was written, before Task 4's own four banks) and this suite is
/// the only instrument that would say why.
///
/// **Nothing here rewinds anything** — the delay elapses in real time, exactly
/// as `a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`
/// does for the other behaviour.
///
/// Three directions, because a predicate that named everything would pass the
/// first alone:
///
/// 1. the execution whose delay has elapsed is named, **by id**;
/// 2. the one whose `PT30S` interval has not is not;
/// 3. and after the replay exists, the first is named no more — which is the
///    term that keeps a finished chain from being relocked and re-settled on
///    every tick for ever, since nothing ever clears a `not_before`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_due_replay_is_woken_and_the_waker_stops_naming_it_once_it_exists(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = AlwaysFails::default();

    let due = start_execution(
        &pool,
        ws,
        None,
        &one_task_replaying("due", "PT1S", 3),
        serde_json::json!({}),
    )
    .await
    .expect("start due");
    let waiting = start_execution(
        &pool,
        ws,
        None,
        &one_task_replaying("waiting", "PT30S", 3),
        serde_json::json!({}),
    )
    .await
    .expect("start waiting");

    assert_eq!(
        drain_with(&pool, &reg).await,
        2,
        "both first attempts must have run: {:?}",
        reg.seen()
    );
    for (label, execution, interval_ms) in [("due", due, 1_000), ("waiting", waiting, 30_000)] {
        assert_eq!(
            run_state(&pool, execution, "a").await.as_deref(),
            Some("RETRIED"),
            "a CREATE_NEW_EXECUTION retry marks the run RETRIED, not RETRYING \
             -- the {label} run: {}",
            stuck(&pool, execution, "a").await
        );
        let deadline = not_before_of(&pool, execution, "a")
            .await
            .expect("the replay's ExecutionDelay was scheduled");
        assert!(
            deadline >= interval_ms,
            "the {label} run's deadline must be at least its own interval past \
             the execution's origin: {}",
            stuck(&pool, execution, "a").await
        );
        assert_eq!(
            replay_of(&pool, execution).await,
            None,
            "no replay may exist before its delay has elapsed ({label})"
        );
    }

    // The delay passes in REAL time.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    let woken = wake(&pool, 8).await.expect("wake");
    assert_eq!(
        woken,
        vec![due],
        "the waker must name the execution whose replay came due and no other. \
         due={due} (interval PT1S, elapsed); waiting={waiting} (interval PT30S, \
         not elapsed). It named {woken:?}"
    );
    let child = replay_of(&pool, due)
        .await
        .expect("the wake must have created the replacement execution");
    assert_eq!(
        replay_of(&pool, waiting).await,
        None,
        "and the execution the waker did not name must have no replay"
    );
    assert_eq!(
        metadata_of(&pool, child).await.0,
        2,
        "the replacement is attempt 2 of the chain"
    );

    // Direction 3: the deadline is still there and still past, and the child
    // now exists.
    let again = wake(&pool, 8).await.expect("wake again");
    assert!(
        !again.contains(&due),
        "the waker named {due} again after its replay existed. Nothing clears \
         a `not_before`, so without the `NOT EXISTS` term this execution is \
         relocked and re-settled on every tick for ever. It named {again:?}"
    );
}

/// **A replay is refused inside a `Loop` sub-execution and taken at the top
/// level** — the same flow shape either side of the one difference.
///
/// The refusal is the artifact's own behaviour, measured on the pinned digest
/// (`egret.probe4/replay-in-loop2`): Kestra throws `Can't find taskrun with
/// taskrunId <the LOOP run's>` out of `markWithTaskRunAs(…, markParents =
/// true)`, whose walk leaves the sub-execution, and the iteration ends FAILED
/// with no child. Creating one here would be over-acceptance the oracle cannot
/// see — no corpus flow puts a `CREATE_NEW_EXECUTION` retry inside a `Loop`.
///
/// **Both directions, in one test, because a `spawn_due` that refused
/// everything would pass either alone.** The control is the top-level
/// execution: same policy, same interval, same handler, and it must get its
/// replacement.
///
/// It also asserts the waker's own term: the loop sub-execution is one
/// `due_retries` must never name, because no child will ever appear to stop it.
///
/// # It runs in BOTH directions, and the second one was added by a fix round
///
/// Everything the first version asserted was on the way **down** — that the
/// iteration's run is `RETRIED`, that no replay is created, that the waker does
/// not name it. The defect was on the way **up**: the loop run read a `RETRIED`
/// iteration as one that had terminated without error and the whole flow
/// reported `SUCCESS` where Kestra reports `FAILED`. A test that builds exactly
/// the right fixture and only ever walks one direction through it is the third
/// instance of that shape in this package. So this one now asserts the loop
/// run, the root's history, **and** the one thing that is still divergent (the
/// iteration's own terminal), which is what keeps the remaining gap from
/// drifting unrecorded.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_is_refused_inside_a_loop_sub_execution_and_taken_at_the_top_level(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = AlwaysFails::default();

    let looped = start_execution(
        &pool,
        ws,
        None,
        &loop_body_replaying("looped", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start the loop flow");
    let top = start_execution(
        &pool,
        ws,
        None,
        &one_task_replaying("top", "PT1S", 3),
        serde_json::json!({}),
    )
    .await
    .expect("start the top-level flow");
    drain_with(&pool, &reg).await;

    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix IS NOT NULL",
    )
    .bind(looped)
    .fetch_one(&pool)
    .await
    .expect("the loop spawned its one iteration");
    assert_eq!(
        run_state(&pool, iteration, "a").await.as_deref(),
        Some("RETRIED"),
        "the body run inside the iteration is RETRIED, so the only thing \
         between it and a replay is `spawn_due`'s refusal: {}",
        stuck(&pool, iteration, "a").await
    );

    // **The ASCENT, and it is the half a fix round had to add.** Everything
    // above this walks DOWN into the sub-execution; the defect was on the way
    // back up, where the loop run reads what the iteration terminated as. Egret
    // reported SUCCESS here — `spawn::decide` fell through to
    // `terminated == total` because `RETRIED` is not `terminated_in_error` —
    // while Kestra reports FAILED on both rows, measured on the pinned digest:
    //
    // ```text
    // exec 4hsg…  parentId=None   state=FAILED  run l     = FAILED
    // exec 1EM1…  parentId=4hsg…  state=FAILED  run inner = FAILED
    // ```
    assert_eq!(
        run_state(&pool, looped, "l").await.as_deref(),
        Some("FAILED"),
        "the loop run whose one iteration was retried into a replay Kestra \
         cannot create must FAIL. It read SUCCESS until `as_the_loop_reads_it` \
         -- a silent FAILED->SUCCESS inversion on a publishable flow that the \
         oracle cannot see, because no corpus flow puts a CREATE_NEW_EXECUTION \
         retry inside a Loop: {}",
        stuck(&pool, looped, "l").await
    );
    assert_eq!(
        execution_history(&pool, looped).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
        "and the root execution with it -- Kestra's root ends FAILED. This is \
         the assertion the first round stopped one line short of"
    );
    // The half that is NOT fixed, asserted so it cannot drift unrecorded: the
    // iteration's OWN terminal is still `RETRIED` where Kestra's is `FAILED`,
    // because `advance()` is pure over `(plan, state, now)` and cannot see
    // which link kind carried the execution it is driving.
    assert_eq!(
        execution_history(&pool, iteration).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED", "RETRIED"]),
        "the iteration execution itself is the remaining divergence: it FAILS \
         and is then marked RETRIED, where Kestra's stops at FAILED. It is recorded rather than repaired: fixing it needs a third \
         metadata field on `ExecutionState` for a case no corpus flow reaches, \
         and it is invisible to the parent now that the loop reads it as a \
         failure"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    // The waker's term first: it must not name the iteration at all.
    let woken = wake(&pool, 8).await.expect("wake");
    assert!(
        !woken.contains(&iteration),
        "the waker named the loop iteration {iteration}. Its replay is refused \
         permanently, so no child will ever appear to stop it being named -- \
         this is the arm that would relock and re-settle it for ever. Named: \
         {woken:?}"
    );

    // And directly, which is the path the oracle's driver takes and the waker's
    // predicate never sees.
    egret_workflow::worker::resume(&pool, iteration)
        .await
        .expect("resume the iteration");
    assert_eq!(
        replay_of(&pool, iteration).await,
        None,
        "a `Loop` iteration must not be replayed even when `resume` is called \
         on it directly: the artifact cannot do it and the child would be an \
         orphan -- `spawn::on_execution_terminal` reports a terminal only \
         through a `parent_run_ix`, which a replay does not have"
    );

    // The control: the same policy, not in a loop.
    assert!(
        woken.contains(&top),
        "the top-level execution's replay is due and the waker must name it, \
         or the refusal above is being made by something that refuses \
         everything. Named: {woken:?}"
    );
    let child = replay_of(&pool, top)
        .await
        .expect("the top-level execution must get its replacement");
    assert_eq!(
        metadata_of(&pool, child).await.0,
        2,
        "and it is attempt 2 of that chain"
    );
}

/// **The metadata accumulates up a chain, and `maxAttempts` is what ends it.**
///
/// `ExecutionMetadata.nextAttempt()` increments `attemptNumber` and leaves
/// `originalCreatedDate` alone; migration 0139 stores the second as an offset,
/// so "leaves it alone" is "grows by the elapsed time of the execution that
/// spawned it". Both are asserted because they answer different questions —
/// which attempt this is, and how much of the chain's clock has run — and the
/// oracle can only see their consequence.
///
/// **`maxAttempts: 3`, so the increment is asserted TWICE.** A two-long chain
/// was the first version and a mutation is why it is not: replacing
/// `attempt + 1` with `attempt.max(2)` writes the right number for the first
/// replay and the wrong one for every later replay, and a chain that ends AT
/// the first replay cannot tell those apart. That mutation SURVIVED the whole
/// of `--lib` and `--test worker` until this test grew a third link.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_chain_carries_its_attempt_number_and_its_origin_offset(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = AlwaysFails::default();

    let root = start_execution(
        &pool,
        ws,
        None,
        &one_task_replaying("chain", "PT1S", 3),
        serde_json::json!({}),
    )
    .await
    .expect("start the chain");
    drain_with(&pool, &reg).await;
    assert_eq!(
        metadata_of(&pool, root).await,
        (1, 0),
        "the original execution is attempt 1 and its own creation is the origin"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(
        wake(&pool, 8).await.expect("wake"),
        vec![root],
        "the root's replay is due"
    );
    let second = replay_of(&pool, root)
        .await
        .expect("the replacement exists");
    let (attempt, offset) = metadata_of(&pool, second).await;
    assert_eq!(attempt, 2, "the replacement is attempt 2");
    assert!(
        offset >= 1_000,
        "the offset is how much of the ORIGINAL's clock had run when this one \
         was created, and the retry was a whole PT1S away, so it cannot be \
         less than 1000 ms. It is {offset}"
    );

    // **The SECOND increment, which is the one a saturating `max(2)` gets
    // wrong**: it writes the right number for the first replay and the wrong
    // one for every later replay, so a two-long chain cannot see it.
    drain_with(&pool, &reg).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(
        wake(&pool, 8).await.expect("wake"),
        vec![second],
        "attempt 2 of 3 is not the last, so its own replay comes due"
    );
    let third = replay_of(&pool, second)
        .await
        .expect("the second replacement");
    let (third_attempt, third_offset) = metadata_of(&pool, third).await;
    assert_eq!(
        third_attempt, 3,
        "the second replacement is attempt 3. An increment that saturates at 2 \
         writes 2 here, `2 >= 3` is false for ever, and the chain never ends -- \
         which the ratchet sees minutes later as the driver's step bound"
    );
    assert!(
        third_offset >= offset + 1_000,
        "and the origin offset accumulates: {third_offset} must be at least a \
         second past the second execution's {offset}"
    );

    // The chain ends here: `attemptNumber >= maxAttempts` is 3 >= 3.
    drain_with(&pool, &reg).await;
    assert_eq!(
        execution_history(&pool, third)
            .await
            .as_array()
            .and_then(|h| h.last().cloned()),
        Some(serde_json::json!("FAILED")),
        "attempt 3 of 3 exhausts the policy, so the third execution FAILS \
         rather than being RETRIED"
    );
    assert_eq!(
        not_before_of(&pool, third, "a").await,
        None,
        "and no fourth delay is scheduled: `next_retry_at` returned None, so \
         `scan` emitted nothing at all for this run"
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert!(
        wake(&pool, 8).await.expect("wake").is_empty(),
        "nothing is due anywhere: the chain is three executions long"
    );
    assert_eq!(
        replay_of(&pool, third).await,
        None,
        "and the third execution has no replacement"
    );
}

/// **A replay is never started from a nested `settle`, and the very next
/// `resume` starts it** — the third of `worker::replay`'s refusals, with the
/// shape that reaches it.
///
/// # Why this bound exists, and why it is not a number
///
/// `subflow::spawn_one` drives a child's first settle **inside its caller's
/// transaction**, so a chain of executions is a chain of stack frames — the
/// recursion `subflow::MAX_SPAWN_CHAIN` was measured to abort the process
/// under. A replay reached from inside such a settle would extend that
/// recursion with a chain nothing else bounds: `interval: PT0S` makes the
/// deadline due at the instant it is written, and a flow that reaches `FAILED`
/// **without a worker** — an unresolvable `Subflow` target is the reachable
/// example, measured on the artifact — closes the loop entirely inside one
/// transaction.
///
/// The answer is structural rather than numeric, and it is the artifact's own:
/// its replay comes off the **execution-delay loop**, a different loop from the
/// executor cycle (`DefaultExecutor.java:508`), one execution per delivery.
/// egret's equivalent of "a different loop" is `claim::Stack::Fresh` — a settle
/// with nothing of this crate's below it — so `spawn_due` refuses at any depth
/// above 0 and leaves the deadline where it is.
///
/// # Both directions, from one execution
///
/// The refusal and the taking are asserted on the **same** execution `mid`,
/// seconds apart, with nothing between them but who called `settle`: first
/// nested inside its parent's spawn, then from `worker::resume` at the bottom
/// of a fresh stack. A test that only drove the second could not tell this
/// bound from no bound at all.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_is_not_started_from_a_nested_settle_and_the_next_resume_starts_it(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;

    // `mid` fails inside its own first settle -- its `Subflow` target does not
    // resolve, which `a_subflow_target_that_does_not_resolve_fails_the_run_and_
    // creates_no_child` pins -- and its flow-level retry is due at once.
    let mid = "id: mid\nnamespace: t\nretry:\n  behavior: CREATE_NEW_EXECUTION\n  \
               type: constant\n  interval: PT0S\n  maxAttempts: 3\ntasks:\n  - id: sub\n    \
               type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
               flowId: nobody-holds-this\n";
    file_flow(&db, ws, user, mid).await;

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "mid", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start the parent");

    let child = children_by_trigger(&pool, parent).await;
    assert_eq!(
        child.len(),
        1,
        "the parent's `Subflow` resolved and started `mid`: {child:?}"
    );
    let mid_id = child[0].0;
    assert_eq!(
        execution_history(&pool, mid_id)
            .await
            .as_array()
            .and_then(|h| h.last().cloned()),
        Some(serde_json::json!("RETRIED")),
        "`mid` reached FAILED inside its parent's transaction and its \
         `CREATE_NEW_EXECUTION` retry took it to RETRIED there -- if it did \
         not, this test is no longer driving the nested case at all"
    );
    let deadline = not_before_of(&pool, mid_id, "sub")
        .await
        .expect("the replay's delay was scheduled");
    assert_eq!(
        deadline, 0,
        "`interval: PT0S` at the instant of the failure: the deadline is due \
         the moment it is written, so nothing but the depth refuses this spawn"
    );

    // Direction 1: refused, inside the parent's settle.
    assert_eq!(
        replay_of(&pool, mid_id).await,
        None,
        "a replay was started from a NESTED settle. `spawn_due` runs at the end \
         of every settle and this one is inside `subflow::spawn_one`'s -- the \
         recursion `subflow::MAX_SPAWN_CHAIN` exists to bound"
    );

    // Direction 2: the same execution, the same due deadline, from a fresh
    // stack.
    egret_workflow::worker::resume(&pool, mid_id)
        .await
        .expect("resume mid");
    let replay = replay_of(&pool, mid_id)
        .await
        .expect("the resume must start the replacement: nothing else changed");
    assert_eq!(
        metadata_of(&pool, replay).await.0,
        2,
        "and it is attempt 2 of the chain"
    );
    // One per resume, not a chain built in this transaction: the replay's own
    // settle is nested inside this one and refuses in turn.
    assert_eq!(
        replay_of(&pool, replay).await,
        None,
        "the replacement's OWN replay must wait for its own fresh stack, or the \
         bound is not bounding: with `PT0S` and `maxAttempts: 3` an unbounded \
         version builds the whole chain inside one transaction"
    );
}

/// **A replay inherits the `correlationId` of the execution it replaces**, and
/// its own `{{ execution.id }}` is still its own.
///
/// `render::Surface::load` walks `parent_execution_id` to the top of the chain
/// for `root`, which feeds `labels.system.correlationId`, and it does **not**
/// stop that walk where the `inheriting` flag stops the outputs walk. W6 Task 3
/// left that difference open for Task 4 to measure rather than choose, and the
/// measurement is on the pinned artifact — `egret.probe4/replay-correlation`, a
/// flow-level `CREATE_NEW_EXECUTION` retry over a `Log` of
/// `{{ labels.system.correlationId }}`:
///
/// ```text
/// root   cid=3nY5p1H9MmhpWnq4hZsVOD eid=3nY5p1H9MmhpWnq4hZsVOD
/// replay cid=3nY5p1H9MmhpWnq4hZsVOD eid=5cNdx41IqqhyGSExZsuGLu
/// ```
///
/// `ExecutionService.replay` copies the parent's labels, and
/// `Execution.java:213` fills a `correlationId` in only when one is ABSENT — so
/// the label survives the whole chain. **No golden entry pairs a replay with a
/// `correlationId` read**, so the ratchet is silent about this and the probe
/// plus this test are the only instruments.
///
/// The condition of an `If` is the observation point because `advance()` stores
/// the render in `task_runs.rendered`, which is a column a test can read
/// without a handler in the middle.
///
/// # `maxAttempts: 3`, and the two-link chain is the whole point
///
/// **This test claimed to keep `root` from drifting and did not** — W6 Task 6's
/// Important, carried to Task 8 and fixed here. Its fixture was `maxAttempts:
/// 2`, i.e. **one** link, where the chain root and the immediate parent are the
/// same row: replacing `root = id` inside `Surface::load`'s walk with
/// `if root == ctx.id { root = id; }` — *take the nearest ancestor, not the
/// top* — answered the same id, passed this test, passed the rest of the crate
/// and passed the ratchet. The exact shape of
/// [`a_replay_chain_carries_its_attempt_number_and_its_origin_offset`]'s
/// `attempt.max(2)`, one module over: **a fixture answers the question you
/// built it for and stays silent about the one you then generalise to.**
///
/// So the chain is three executions long and the assertion that matters is on
/// the **second** replay, whose parent is the first replay and whose root is
/// the original. Measured: with that mutation applied, the second replay
/// renders the FIRST replay's id and this test fails naming both.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_inherits_the_correlation_id_of_the_execution_it_replaces(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = AlwaysFails::default();

    let source = "id: cid\nnamespace: t\nretry:\n  behavior: CREATE_NEW_EXECUTION\n  \
                  type: constant\n  interval: PT1S\n  maxAttempts: 3\ntasks:\n  - id: cond\n    \
                  type: io.kestra.plugin.core.flow.If\n    \
                  condition: \"{{ labels.system.correlationId }}\"\n    then:\n      - id: a\n        \
                  type: io.kestra.plugin.core.log.Log\n        message: hi\n";
    let root = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start");
    drain_with(&pool, &reg).await;

    async fn rendered(pool: &sqlx::PgPool, execution: Uuid) -> Option<String> {
        sqlx::query_scalar(
            "SELECT rendered FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'cond'",
        )
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read the rendered condition")
    }
    assert_eq!(
        rendered(&pool, root).await,
        Some(root.to_string()),
        "a top-level execution IS its own correlation root"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(wake(&pool, 8).await.expect("wake"), vec![root]);
    let replay = replay_of(&pool, root)
        .await
        .expect("the replacement exists");
    drain_with(&pool, &reg).await;

    assert_ne!(replay, root, "the replay is a different execution");
    assert_eq!(
        rendered(&pool, replay).await,
        Some(root.to_string()),
        "the replay's `labels.system.correlationId` must be the ORIGINAL \
         execution's id -- `Surface::load`'s `root` walks the whole \
         `parent_execution_id` chain, deliberately, and a sticky version of \
         that walk (the one `inheriting` uses for outputs) would answer {replay} \
         here and diverge from the artifact"
    );

    // **The SECOND link, which is the one a nearest-ancestor `root` gets
    // wrong.** Everything above is one link deep, where the chain root IS the
    // immediate parent, so it cannot tell the two apart -- and until this
    // block existed, a `root` that stopped at the nearest ancestor passed the
    // whole crate and the ratchet.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(
        wake(&pool, 8).await.expect("wake"),
        vec![replay],
        "attempt 2 of 3 is not the last, so the first replay's own replacement \
         comes due -- if this is empty the chain is one link long again and the \
         assertion below is measuring nothing"
    );
    let second = replay_of(&pool, replay)
        .await
        .expect("the second replacement exists");
    drain_with(&pool, &reg).await;
    assert_ne!(second, replay, "the second replay is its own execution");
    assert_eq!(
        parent_of(&pool, second).await,
        Some(replay),
        "and it hangs off the FIRST replay, not off the root: the two-link \
         chain this assertion needs does not exist otherwise"
    );
    assert_eq!(
        rendered(&pool, second).await,
        Some(root.to_string()),
        "two links up, and the label is still the ORIGINAL execution's id. A \
         `root` that took the nearest ancestor instead of the top of the chain \
         answers {replay} here -- it answers the same thing as the correct walk \
         one link up, which is why the one-link fixture this test used to carry \
         could not see it"
    );
}

/// `parent_execution_id` of one execution, for the tests that need to assert
/// the SHAPE of a chain rather than only its rendered output.
async fn parent_of(pool: &sqlx::PgPool, execution: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT parent_execution_id FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read the parent link")
}

/// **A second `resume` on an execution whose replay already exists starts no
/// second one**, and the database would refuse it if it tried.
///
/// Two mechanisms sit on this and they are not the same fence, which is why
/// both are driven here:
///
/// - `replay::spawn_due` asks whether a replay exists and answers "already
///   done" rather than inserting. Without it the insert is attempted, and
/// - migration 0138's `workflow_executions_loop_run_idx` — `(parent_execution_id,
///   parent_run_ix, loop_index)` with `NULLS NOT DISTINCT` — makes the second
///   row **unrepresentable**, so the attempt aborts the whole transaction
///   rather than producing a fan-out.
///
/// The first is what keeps an ordinary second `resume` from being an error, and
/// an ordinary second `resume` is not hypothetical: nothing ever clears a
/// `not_before`, `worker::wake` filters on the child's absence but the oracle's
/// driver resumes by deadline alone, and any caller may resume any execution at
/// any time.
///
/// One replay per parent is the artifact's behaviour, measured: over the 710
/// descendants in `traces.json`, 40 carry a NULL `loopIndex` and no parent
/// carries two.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_resume_does_not_start_a_second_replay(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = AlwaysFails::default();

    let root = start_execution(
        &pool,
        ws,
        None,
        &one_task_replaying("twice", "PT1S", 3),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    drain_with(&pool, &reg).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    egret_workflow::worker::resume(&pool, root)
        .await
        .expect("the first resume");
    let first = replay_of(&pool, root).await.expect("one replay");

    egret_workflow::worker::resume(&pool, root)
        .await
        .expect("the second resume must SUCCEED, not abort on a unique index");
    let all: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix IS NULL ORDER BY id",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .expect("read the replays");
    assert_eq!(
        all,
        vec![first],
        "the deadline is still past and nothing cleared it, so the second \
         resume asked the same question and must answer `already done`"
    );
}

/// **A `Subflow` child IS replayed, and egret cannot report the replacement's
/// terminal to the parent: the `sub` RUN diverges always, and the parent
/// EXECUTION diverges whenever a successor outlives the replay** — W6 Task 4,
/// fix rounds 2-4.
///
/// # This test asserted the opposite of the artifact, and that was worse than
/// asserting nothing
///
/// Fix round 1 wrote it with the message *"which is what Kestra does too —
/// measured"* on the one line where Kestra does something else. A wrong claim
/// does not merely fail to catch a divergence: it **defends** it, and the next
/// reader takes the assertion for the measurement. Both halves are now measured
/// here, on the pinned digest, twice, with this file's own fixture shape
/// (compose project `t4r2kestra`, namespace `t4r2.probe`, torn down `-v`).
///
/// ```text
/// parent      6Mma…  SUCCESS   ["CREATED","RUNNING","SUCCESS"]
///   run sub          FAILED    ["CREATED","RUNNING","RETRIED","SUCCESS","FAILED"]
///   run after        SUCCESS
/// child       3vcz…  RETRIED   trigger.taskRunId = the parent's `sub` run
/// replacement kPXx…  FAILED    parentId = the child,
///                              trigger  = BYTE-IDENTICAL to the child's
/// ```
///
/// Two facts the round-1 version got wrong:
///
/// 1. **The replacement is NOT untriggered.** `Execution.childExecution()`
///    copies `this.trigger` verbatim (`Execution.java:405-431`, the field at
///    :422), so the replacement carries the *parent's* `executionId` and the
///    *parent's* `sub` `taskRunId`, exactly as the child does.
/// 2. **The parent's `sub` run therefore ends `FAILED`**, stamped **8.5-33.3
///    ms** after the replacement's own `FAILED` — a range over the 16 stamps
///    recorded to a decimal (4 in fix round 3, 12 in re-review 3), not a
///    characteristic figure. An earlier version of this doc said `~13-33 ms`,
///    which the next round's runs falsified at the low end.
///
/// # What the FIXTURE was holding fixed, and the third correction it hid
///
/// Rounds 1 and 2 read the trace above as *"the stamp lands after the parent
/// has finished, so it changes nothing"*. **That framing was an artifact of
/// this fixture's trivial successor, not of Kestra's design.** Change one thing
/// the fixture holds fixed — the successor, from `Log` to `Sleep PT8S`, still a
/// legal publishable flow — and measure again. Both orderings, measured twice
/// each on the pinned digest in fix round 3:
///
/// ```text
/// successor `Log`    parent EXEC SUCCESS in 16/16   stamp +407 to +1627.6 ms AFTER the parent ended (8 obs)
/// successor `Sleep`  parent EXEC FAILED  in 21/22   stamp  -17.4 to -45.9 ms BEFORE it, parent still RUNNING (8 obs)
/// ```
///
/// It is **one rule** — report the replacement's terminal through the copied
/// trigger — whose observable consequence depends on whether the parent is
/// still alive to receive it. At the second ordering the write is ordinary, the
/// parent is live, and **the flow's own verdict differs**: Kestra `FAILED`,
/// egret `SUCCESS`. (Kestra's `after` then completed `SUCCESS` 6.1-7.8 s
/// *after* the execution had been declared `FAILED`, over 7 observations.)
///
/// **It is NOT established that the rule always fires, and an earlier version
/// of this doc said "one deterministic rule, not a race".** That is stronger
/// than 22 ordering-B runs support: **1 of the 22 delivered no stamp at all**
/// on a live parent — `sub` stopped at the same four states egret produces and
/// the parent ended `SUCCESS` — and `Execution.hasTaskRunJoinable`
/// (`Execution.java:844`) is a live path that discards a subflow result whose
/// taskrun carries fewer histories than the execution row now holds. That run
/// was not reproduced in 17 further attempts; it is a **calibrated
/// counterexample, not a falsification**, and it is recorded because a property
/// asserted from runs nobody had made yet is this task's recurring defect.
///
/// **egret's answer is the same at both orderings, by construction**, and the
/// assertion that says so is the `(trigger, trigger_run) == (None, None)` one
/// below: with no trigger link on the replacement, `subflow::on_child_terminal`
/// is never invoked for it, so there is no stamp to be early or late. That is
/// why this test does not need a second fixture to cover the second ordering —
/// but it is also why the divergence is bigger than rounds 1-2 said.
///
/// # What agrees, and what egret cannot represent
///
/// The child (`RETRIED`), the replacement (`FAILED`) and the fact that a replay
/// happens at all agree. Concern 3's verdict stands — replaying here is the
/// artifact's behaviour and refusing it would be under-acceptance. **What
/// diverges is the parent: its `sub` run always, and its EXECUTION whenever a
/// successor outlives the replay.**
///
/// **It is not chosen — but it is less structural than this doc used to say,
/// and fix round 4 corrected both halves of the claim:**
///
/// * ~~the CHECK makes it impossible~~. The replay must carry
///   `parent_execution_id` — that is what puts it in the oracle's `spawned`,
///   which is what all four banked entries compare — and 0138's
///   `workflow_executions_one_link_kind` makes carrying a trigger link beside
///   it unrepresentable. **But the address does not have to be stored.** This
///   row's `parent_execution_id` names the CHILD, whose own
///   `(trigger_execution_id, trigger_run_ix)` is the parent execution and its
///   `sub` run — which is exactly the pair `Execution.childExecution()` copied,
///   by construction. One hop up resolves it, `subflow::chain_holds` already
///   walks `COALESCE(trigger_execution_id, parent_execution_id)`, and `spawned`
///   never moves because `read_trace_inner` reads a column the walk does not
///   touch. **So this cost does not exist**, and it was the stronger of the two
///   the ruling stood on;
/// * what actually drops the report is `subflow::on_child_terminal` returning
///   early on a parent run that is already terminal — and it *is* terminal in
///   both orderings before the replacement exists, which the
///   `run_history(parent, "sub")` assertion below pins. Kestra has no such
///   guard: `Execution.hasTaskRunJoinable` blocks terminal→non-terminal
///   (`:838`) and an outdated-history join (`:844`) but permits
///   terminal→terminal when the history grows. **Nothing in this tree observes
///   that guard** — mutating it so it can never fire left this suite's 53
///   tests, the executor suite's 45 and the ratchet's 294/127/127/0 unmoved —
///   and its stated grounds are wrong in one half: this doc said
///   *"`transmitFailed` and `wait: false`"*, while the guard's own doc names a
///   `Subflow` task's `retry:` and `wait: false`, and the `wait: false` half is
///   redundant behind the `!sub.wait` return that sits after it.
///
/// **A third cost was claimed in round 2 and is withdrawn.** It read *"egret
/// would need a path that writes to a terminal execution"*. At the ordering
/// that matters the parent is **live**, so the write is an ordinary one and
/// W3b's durability property is not in play. The fidelity rule does not excuse
/// egret here; an unmeasured guard and an empty corpus population are what it
/// ships on.
///
/// For a shape **no corpus flow reaches** — derived, not remembered: over
/// `oracle-exec/corpus/`, 345 flows, 4 writing `CREATE_NEW_EXECUTION` and none
/// of those declaring a `Loop`; 25 declaring a `Subflow` across 20 literal
/// `flowId` targets; intersection with the four is empty.
///
/// # A person decided this, and the decision is registered
///
/// **Ruled by the controller (W6 Task 4, fix rounds 2 and 3): the divergence
/// stands, deliberately** — on 0138's CHECK and the empty corpus population.
/// **Fix round 4 removed the first of those two grounds**: the CHECK is
/// avoidable, so what the ruling now rests on is the empty corpus population
/// plus one guard whose load-bearingness nobody has measured. The reasoning,
/// the corrected size, the ranges with their run counts and the costs as they
/// now stand are on the record in
/// `oracle-exec/README.md`'s coverage statement, **item 16**, which is where
/// this package registers divergences the ratchet cannot hold.
///
/// **It is parked, not hidden.** The third derived shape in
/// `tests/executor.rs`'s `STILL_OVER_ACCEPTED_AT_RUNTIME` — a run Kestra
/// stopped that egret completed — is exactly this divergence's shape, so the
/// moment any corpus flow reaches it the suite fails as an *unregistered*
/// runtime over-acceptance rather than passing quietly.
///
/// So a reader who reaches the assertions below is not looking at a test that
/// lapsed. The values are egret's, they are correct, and each message carries
/// Kestra's value beside them.
///
/// **It is the same FAILED→SUCCESS direction as this task's first Critical, and
/// at the second ordering it is the same SIZE too** — the flow's own verdict.
/// Rounds 1 and 2 said otherwise; that sentence is corrected here rather than
/// left standing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_is_replayed_and_egret_cannot_report_it_to_the_parent(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    // The target flow fails for real -- `core.execution.Fail` has a handler in
    // `with_core_tasks` -- and carries the flow's retry, so the retry is
    // decided inside the CHILD execution.
    let child_source = "\
id: child
namespace: t
tasks:
  - id: boom
    type: io.kestra.plugin.core.execution.Fail
    retry:
      behavior: CREATE_NEW_EXECUTION
      type: constant
      interval: PT1S
      maxAttempts: 2
";
    file_flow(&db, ws, user, child_source).await;

    let reg = Handlers::with_core_tasks(egret_workflow::storage::current());
    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "child", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start the parent");
    settle_everything(&pool, &reg).await;

    let kids = children_by_trigger(&pool, parent).await;
    assert_eq!(kids.len(), 1, "exactly one subflow child: {kids:?}");
    let (child, _, parent_link, _, child_state) = kids[0].clone();
    assert_eq!(
        (parent_link, child_state.as_str()),
        (None, "RETRIED"),
        "the child is linked by the trigger columns only, and its own terminal \
         is RETRIED -- the state a CREATE_NEW_EXECUTION retry ends an execution \
         in. Kestra's child ends RETRIED too (measured)"
    );
    // **THE DIVERGENCE, asserted as one.** Kestra's `sub` run ends `FAILED`
    // (`["CREATED","RUNNING","RETRIED","SUCCESS","FAILED"]`, measured in 38
    // runs across four compose projects on the pinned digest). egret's ends
    // `SUCCESS` and stops there. Round 1 asserted this value with the words
    // "which is what Kestra does too -- measured", which was false and defended
    // the gap.
    //
    // **Why egret stops is one thing, not two, and fix round 4 corrected it.**
    // The replacement carries no trigger link (0138), but the address is
    // derivable one hop up `parent_execution_id` without storing it -- so what
    // actually drops the report is `on_child_terminal`'s already-terminal
    // guard, and the assertion below pins that the run IS terminal here, before
    // the replacement exists.
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("SUCCESS"),
        "egret's `sub` run ends SUCCESS. **Kestra's ends FAILED** -- it stamps \
         the replacement's failure onto this run through the trigger block \
         `Execution.childExecution()` copied, 8.5-33.3 ms after the \
         replacement failed (16 stamps recorded to a decimal; an earlier \
         version of this message said 13-33 ms and the next round's runs \
         falsified the low end). WHEN it lands relative to the parent's own \
         terminal depends on the successor and nothing else -- after it here, \
         BEFORE it with a `Sleep PT8S`, where Kestra's parent then ends \
         FAILED in 21 of 22 runs. This is a recorded DIVERGENCE, not \
         agreement, and a RULED one: `oracle-exec/README.md`'s coverage \
         statement, item 16, carries the measurement with its run counts, what \
         following Kestra would actually cost -- ONE guard, which no test in \
         this tree observes -- and the controller's ruling that it stands"
    );
    assert_eq!(
        run_history(&pool, parent, "sub").await,
        serde_json::json!(["CREATED", "RUNNING", "RETRIED", "SUCCESS"]),
        "and the whole history, not just its last state, so the divergence is \
         pinned at the exact point it starts: egret reaches Kestra's fourth \
         entry and stops, and Kestra appends a fifth, FAILED. WHEN that fifth \
         lands relative to the execution holding this run is the successor's \
         doing rather than a property of the shape: after that execution had \
         terminated with THIS fixture's `Log`, before it with a `Sleep PT8S`. \
         **SUCCESS here is also already terminal**, which is the guard that \
         would drop the report even with the address in hand -- item 16's \
         second cost, and the only one that survives"
    );
    assert_eq!(
        run_state(&pool, parent, "after").await.as_deref(),
        Some("SUCCESS"),
        "so the successor runs and the parent really did proceed"
    );
    assert_eq!(
        replay_of(&pool, child).await,
        None,
        "no replacement before the delay has elapsed"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    let woken = wake(&pool, 8).await.expect("wake");
    assert_eq!(
        woken,
        vec![child],
        "the waker names the subflow child, and nothing else. A `spawn_due` \
         keyed on `has a parent` rather than on `parent_run_ix` would refuse \
         this replay, which is UNDER-acceptance: Kestra replays here, because \
         the retried run's parents are all local to the child execution and \
         nothing throws. Named: {woken:?}"
    );
    let replacement = replay_of(&pool, child)
        .await
        .expect("the subflow child gets its replacement");
    assert_eq!(
        metadata_of(&pool, replacement).await.0,
        2,
        "attempt 2 of the chain"
    );
    let (trigger, trigger_run): (Option<Uuid>, Option<i32>) = sqlx::query_as(
        "SELECT trigger_execution_id, trigger_run_ix FROM workflow.executions WHERE id = $1",
    )
    .bind(replacement)
    .fetch_one(&pool)
    .await
    .expect("read the replacement\'s trigger columns");
    assert_eq!(
        (trigger, trigger_run),
        (None, None),
        "egret's replacement carries NO trigger link, so \
         `subflow::on_child_terminal` -- which reads `trigger_execution_id` -- \
         can never reach the parent with it. **Kestra's replacement carries the \
         child's trigger block verbatim** (`Execution.childExecution()` copies \
         `this.trigger`, `Execution.java:422`), naming the parent execution and \
         the parent's `sub` taskrun. egret STORES no such link: 0138's \
         `workflow_executions_one_link_kind` makes `parent_execution_id` and \
         `trigger_execution_id` mutually exclusive, and the replay needs the \
         first, because that is the column the oracle's `spawned` is built from \
         and all four banked entries compare. **`cannot` would be too strong, \
         and fix round 4 corrected it**: `parent_execution_id` here names the \
         CHILD, whose own trigger columns hold the very pair Kestra copied, so \
         the address is one hop away and derivable without storing it. The \
         value here is egret's, and the divergence is the point of the message"
    );

    // The replacement runs and ends, and the parent must not move.
    settle_everything(&pool, &reg).await;
    assert_eq!(
        execution_history(&pool, replacement).await,
        serde_json::json!(["CREATED", "RUNNING", "FAILED"]),
        "attempt 2 of 2 has no third attempt to schedule, so the chain ends \
         FAILED"
    );
    // **This message claimed parity and was WRONG -- fix round 3, and it is the
    // SECOND time this task shipped that construction in an assertion.** It
    // read: "which is what Kestra does too, measured: its parent's histories
    // end at SUCCESS". That is true only of THIS fixture, whose successor is a
    // `Log` that finishes before the replacement does. Change the successor to
    // a `Sleep PT8S` -- a legal publishable flow -- and Kestra's parent
    // EXECUTION ends **FAILED** in 21 of 22 runs across three compose
    // projects: the `sub` stamp lands 17.4 to 45.9 ms BEFORE the parent's own
    // terminal (8 observations), i.e. while it is still RUNNING. So the
    // divergence can be the FLOW'S OWN VERDICT, not a run state after the flow
    // is over. The 22nd run delivered no stamp at all and ended SUCCESS; it is
    // recorded in item 16 as a counterexample to "the rule always fires",
    // never as a reason to soften the sentence above.
    assert_eq!(
        execution_history(&pool, parent).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        "egret's parent EXECUTION ends SUCCESS. **So does Kestra's ONLY \
         because this fixture's successor is trivial.** With a successor that \
         outlives the replay (`Sleep PT8S`, 22 runs on the pinned digest) the \
         stamp lands while the parent is still RUNNING and Kestra's parent \
         ends **FAILED** in 21 of them -- egret still answers SUCCESS, \
         because its replacement carries no trigger link at all and so no \
         stamp is ever delivered, at any ordering. That makes this a \
         divergence in the FLOW'S OWN VERDICT, the same class as this task's \
         first Critical, and it is registered and ruled: \
         `oracle-exec/README.md` coverage statement, item 16"
    );
}

/// **Migration 0139's CHECK: the replay metadata is unrepresentable on a row
/// that is not a replay.**
///
/// A guard nobody has seen fail is a guard nobody has seen, and this one cannot
/// be reached through `src/` at all: `worker::replay::spawn_one` is the only
/// writer of a non-default `attempt_number`, and it writes the replay's own
/// shape. So it is driven directly, at the level it lives at — a statement the
/// database must refuse.
///
/// **Three rows, because one refusal proves nothing about which clause did it.**
/// The permitted row is asserted first so a CHECK that refused everything would
/// fail here rather than pass three times.
#[sqlx::test(migrations = "../../migrations")]
async fn the_replay_metadata_is_unrepresentable_on_anything_but_a_replay(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    async fn insert(
        pool: &sqlx::PgPool,
        ws: Uuid,
        parent: Option<Uuid>,
        loop_ix: Option<i32>,
        attempt: i32,
        offset: i64,
    ) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO workflow.executions \
               (workspace_id, namespace, flow_key, source, state, history, \
                parent_execution_id, parent_run_ix, loop_index, loop_value, \
                attempt_number, origin_offset_ms) \
             VALUES ($1,'t','f','src','CREATED','[\"CREATED\"]'::jsonb, \
                     $2,$3,$3,$4,$5,$6) RETURNING id",
        )
        .bind(ws)
        .bind(parent)
        .bind(loop_ix)
        .bind(loop_ix.map(|_| "v"))
        .bind(attempt)
        .bind(offset)
        .fetch_one(pool)
        .await
    }

    let root = insert(&pool, ws, None, None, 1, 0)
        .await
        .expect("a top-level execution with the default metadata is legal");
    let replay = insert(&pool, ws, Some(root), None, 2, 1_234)
        .await
        .expect("a replay -- a parent, no loop index, attempt 2 -- is legal");
    assert_ne!(replay, root);

    let cases = [
        (
            "a top-level execution claiming to be attempt 2",
            None,
            None,
            2,
            0,
        ),
        (
            "a top-level execution carrying an origin offset",
            None,
            None,
            1,
            5,
        ),
        (
            "a LOOP iteration claiming to be attempt 2",
            Some(root),
            Some(0),
            2,
            0,
        ),
        (
            "a replay claiming to be attempt 1 while carrying an offset",
            Some(root),
            None,
            1,
            9,
        ),
    ];
    for (what, parent, loop_ix, attempt, offset) in cases {
        let refused = insert(&pool, ws, parent, loop_ix, attempt, offset).await;
        let Err(e) = refused else {
            panic!("{what} was ACCEPTED; migration 0139's CHECK did not refuse it");
        };
        assert!(
            e.to_string()
                .contains("workflow_executions_metadata_belongs_to_a_replay")
                || e.to_string()
                    .contains("workflow_executions_origin_offset_non_negative"),
            "{what} was refused by the wrong constraint, which means this case \
             is not driving the CHECK it is written about: {e}"
        );
    }
}

/// **A `maxDuration` chain stops when the ORIGINAL execution's clock runs out**
/// — the second of the two facts migration 0139 stores, driven on its own.
///
/// `a_replay_chain_carries_its_attempt_number_and_its_origin_offset` ends its
/// chain on `maxAttempts`, so it cannot tell a working origin from a broken
/// one: a policy with no attempt cap and a `maxDuration` is the only shape that
/// reads `origin` at all.
///
/// `PT2S` over `PT1S` intervals gives the shortest chain with both an end and a
/// middle: the first execution's next retry is one second in, which is inside
/// the budget, and the second's is **two** seconds into the ORIGINAL's clock,
/// which is not. An origin that reset per execution — or one whose sign was
/// wrong — measures the second retry as one second again and the chain never
/// ends.
///
/// The margin is about a second on both sides and does not depend on how fast
/// this machine is: the offset is at least the interval by construction
/// (the replay is created no earlier than its deadline), and the failure that
/// schedules each retry happens milliseconds into its own execution.
#[sqlx::test(migrations = "../../migrations")]
async fn a_max_duration_chain_stops_when_the_original_clock_runs_out(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = AlwaysFails::default();

    let source = "id: dur\nnamespace: t\ntasks:\n  - id: a\n    \
                  type: io.kestra.plugin.core.log.Log\n    message: hi\n    retry:\n      \
                  behavior: CREATE_NEW_EXECUTION\n      type: constant\n      \
                  interval: PT1S\n      maxDuration: PT2S\n";
    let root = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start");
    drain_with(&pool, &reg).await;
    assert_eq!(
        run_state(&pool, root, "a").await.as_deref(),
        Some("RETRIED"),
        "the first retry is one second in, well inside PT2S: {}",
        stuck(&pool, root, "a").await
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(wake(&pool, 8).await.expect("wake"), vec![root]);
    let second = replay_of(&pool, root)
        .await
        .expect("the replacement exists");
    drain_with(&pool, &reg).await;

    assert_eq!(
        run_state(&pool, second, "a").await.as_deref(),
        Some("FAILED"),
        "the second execution's next retry would be TWO seconds into the \
         original's clock, which is not inside PT2S -- so it fails rather than \
         being retried. An origin measured from its OWN creation, or one with \
         the sign flipped, reads one second and retries: {}",
        stuck(&pool, second, "a").await
    );
    assert_eq!(
        not_before_of(&pool, second, "a").await,
        None,
        "and no third delay is scheduled at all"
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert!(
        wake(&pool, 8).await.expect("wake").is_empty(),
        "nothing more is due"
    );
    assert_eq!(
        replay_of(&pool, second).await,
        None,
        "and no third execution"
    );
}

/// **A subflow grandchild's `labels.system.correlationId` is the ROOT's id, and
/// it inherits no `item` from the loop that spawned its parent.**
///
/// The whole-branch review of W6 found `Surface::load`'s `root` walking
/// `parent_execution_id` **alone**, which is `subflow::chain_holds`'s W6 Task 3
/// defect one module over and never swept for: migration `0138`'s
/// `workflow_executions_one_link_kind` makes a subflow child carry
/// `trigger_execution_id` with `parent_execution_id` **NULL**, so the walk
/// stopped dead at the first subflow link and `correlationId` was the child's
/// **own** id.
///
/// # The artifact half, measured rather than argued
///
/// `t8.probe/corr-parent -> corr-child -> corr-grand-child` on the pinned digest
/// — the corpus's own `subflow-parent -> subflow-child -> subflow-grand-child`
/// shape, renamed — read out of Kestra's Postgres:
///
/// ```text
/// flow              id                      parentId  trigger.…executionId     system.correlationId
/// corr-parent       2cvzjgi7mY6lDfaQxiNa0x  null      (started by hand)        2cvzjgi7mY6lDfaQxiNa0x
/// corr-child        8yWnkcC1oAvHsunjmTG1Q   null      2cvzjgi7mY6lDfaQxiNa0x   2cvzjgi7mY6lDfaQxiNa0x
/// corr-grand-child  2Cr8T9lsAM5icJ7KT5NEna  null      8yWnkcC1oAvHsunjmTG1Q    2cvzjgi7mY6lDfaQxiNa0x
/// ```
///
/// The label is the **root's** id two links down, `parentId` is NULL at every
/// hop, and the cause is `trigger.variables.executionId`.
///
/// # Why this test needs four parts
///
/// A one-execution fixture cannot see the defect: **every** implementation of
/// `root` returns the same value for a top-level execution, which is exactly
/// how `the_seeded_correlation_id_divergence_has_no_corpus_population` came to assert
/// that the two corpus readers "agree with the artifact" while comparing them
/// only as roots. So:
///
/// 1. **standalone** — the instrument's own control: the label renders at all,
///    and a root IS its own correlation id;
/// 2. **two subflow links** — the assertion this test exists for. Two, not one:
///    a walk that took the immediate cause instead of the top of the chain
///    agrees with the correct one at a single link;
/// 3. **a loop child** — the `parent_execution_id` half, which must keep
///    working: a fix that swapped one column for the other instead of taking
///    whichever is set would break this row and nothing else;
/// 4. **`item` does not cross a subflow link** — the guard the widened walk
///    needs. The walk reaches `loop -> subflow child` now, and without the gate
///    that child would resolve `{{ item.value }}` off its caller's iteration.
///
/// The corpus reaches part 2 and the ratchet cannot: `valids/subflow-grand-child`
/// logs `{{labels.system.correlationId}}` and is the target of
/// `valids/subflow-child`, one of the twelve entries W6 banked — and the golden
/// records states, so a `Log` renders either value without failing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let reg = Handlers::with_core_tasks(egret_workflow::storage::current());

    // An `If`'s condition is the observation point: `advance()` stores the
    // render in `task_runs.rendered`, so no handler stands between the surface
    // and the assertion.
    fn reader(id: &str, expr: &str) -> String {
        format!(
            "id: {id}\nnamespace: t\ntasks:\n  - id: cond\n    \
             type: io.kestra.plugin.core.flow.If\n    condition: \"{expr}\"\n    then:\n      \
             - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n"
        )
    }
    async fn rendered_of(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT rendered FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(execution)
        .bind(task)
        .fetch_optional(pool)
        .await
        .expect("read the rendered condition")
        .flatten()
    }

    let gc = reader("gc", "{{ labels.system.correlationId }}");

    // 1. The standalone control.
    let solo = start_execution(&pool, ws, None, &gc, serde_json::json!({}))
        .await
        .expect("start the standalone");
    settle_everything(&pool, &reg).await;
    assert_eq!(
        rendered_of(&pool, solo, "cond").await,
        Some(solo.to_string()),
        "the control: a top-level execution IS its own correlation root, and the \
         label renders at all. Every implementation of the walk agrees here, \
         which is why the rest of this test exists"
    );

    // 2. Two subflow links: parent -> mid -> gc.
    file_flow(&db, ws, user, &gc).await;
    file_flow(&db, ws, user, &subflow_parent("mid", "gc", "")).await;
    let root = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("parent", "mid", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start the chain");
    settle_everything(&pool, &reg).await;
    let mid = children_by_trigger(&pool, root);
    let mid = mid.await;
    assert_eq!(
        mid.len(),
        1,
        "the parent spawned exactly one child, or the chain this test needs does \
         not exist: {mid:?}"
    );
    let mid_id = mid[0].0;
    let grand = children_by_trigger(&pool, mid_id).await;
    assert_eq!(
        grand.len(),
        1,
        "and the child spawned the grandchild -- TWO links, which is the whole \
         point: {grand:?}"
    );
    let grand_id = grand[0].0;
    assert_eq!(
        (grand[0].2, grand[0].3.as_str()),
        (None, "gc"),
        "the grandchild carries no `parent_execution_id` (0138's \
         `one_link_kind`) and runs the target flow, so the walk that has to \
         reach the root has two `trigger_execution_id` hops to make"
    );
    assert_eq!(
        rendered_of(&pool, grand_id, "cond").await,
        Some(root.to_string()),
        "**the assertion this test exists for**: two subflow links down, the \
         label is still the ROOT's id -- measured on the artifact, where \
         `corr-grand-child` carries `corr-parent`'s id. A walk keyed on \
         `parent_execution_id` alone answers {grand_id} (its own id, because \
         that column is NULL); a walk that took the immediate cause answers \
         {mid_id}"
    );
    assert_eq!(
        rendered_of(&pool, mid_id, "cond").await,
        None,
        "and `mid` has no `cond` run at all -- it is the `subflow_parent` \
         fixture, not the reader. If this ever renders, the ids above are not \
         the executions this test thinks they are"
    );

    // 3. The loop control: the `parent_execution_id` half still works.
    let loopy = "id: loopy\nnamespace: t\ntasks:\n  - id: each\n    \
                 type: io.kestra.plugin.core.flow.Loop\n    values: '[\"v1\"]'\n    tasks:\n      \
                 - id: cond\n        type: io.kestra.plugin.core.flow.If\n        \
                 condition: \"{{ labels.system.correlationId }}\"\n        then:\n          \
                 - id: a\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n";
    let lroot = start_execution(&pool, ws, None, loopy, serde_json::json!({}))
        .await
        .expect("start the loop");
    settle_everything(&pool, &reg).await;
    let kids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE parent_execution_id = $1")
            .bind(lroot)
            .fetch_all(&pool)
            .await
            .expect("read the loop's sub-executions");
    assert_eq!(kids.len(), 1, "one value, one sub-execution: {kids:?}");
    assert_eq!(
        rendered_of(&pool, kids[0], "cond").await,
        Some(lroot.to_string()),
        "the OTHER link kind, and it must keep working: a fix that swapped \
         `parent_execution_id` for `trigger_execution_id` rather than taking \
         whichever is set would break this row alone"
    );

    // 4. `item` does not cross a subflow link.
    file_flow(&db, ws, user, &reader("gcitem", "{{ item.value }}")).await;
    let li = "id: li\nnamespace: t\ntasks:\n  - id: each\n    \
              type: io.kestra.plugin.core.flow.Loop\n    values: '[\"LOOPVAL\"]'\n    tasks:\n      \
              - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        \
              namespace: t\n        flowId: gcitem\n";
    start_execution(&pool, ws, None, li, serde_json::json!({}))
        .await
        .expect("start the loop-over-subflow");
    settle_everything(&pool, &reg).await;
    let leaked: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM workflow.executions WHERE flow_key = 'gcitem' \
          ORDER BY created_at, id",
    )
    .fetch_all(&pool)
    .await
    .expect("read the item-reading children");
    assert_eq!(
        leaked.len(),
        1,
        "one iteration, one subflow child: {leaked:?}"
    );
    assert_eq!(
        (
            rendered_of(&pool, leaked[0].0, "cond").await,
            leaked[0].1.as_str()
        ),
        (None, "FAILED"),
        "a subflow child has no `item`: the render fails and nothing is stored. \
         Without the `inheriting` gate on the `levels` push, the widened walk \
         would hand this child `LOOPVAL` off the iteration that spawned its \
         CALLER -- a value from another execution's scope, rendered silently. \
         (egret's side is what is asserted here; the artifact's answer to a \
         subflow child reading `{{ item.value }}` is unmeasured, and the \
         alternative is certainly wrong either way, since a subflow child's \
         values arrive as `inputs:`)"
    );
}

/// **The eighth property, and the one the type system could not hold: a
/// `PAUSED` row that names no resumer cannot be stored.**
///
/// W8 Task 4 fix round 2 removed a silent stall by making it unrepresentable in
/// Rust — `Command::PauseRun` cannot be built without a `Resume`, so
/// `executor::flowable::resume_paused`'s `Waiting`-on-`None` arm is unreachable
/// through `advance()`. **That argument covers one of the two ways a `TaskRun`
/// is built.** The other is `claim::state_of`, which reads the row back out of
/// Postgres, and migration 0141 left `resume_with` nullable behind two CHECKs
/// that are *both* predicated on `resume_with` itself:
///
/// ```sql
/// resume_with IS NULL OR resume_with IN (...)           -- the vocabulary
/// resume_with IS NULL OR (... not_before_ms ...)        -- the pairing
/// ```
///
/// Both are vacuously true when the column is NULL, so
/// `state = 'PAUSED' AND resume_with IS NULL` was **accepted**, `parse_resume`
/// mapped it to `None`, and the run stalled for ever — byte for byte the defect
/// fix round 2 deleted, relocated from the type system to storage. Migration
/// **0142** closes it there.
///
/// **Nothing else in this crate can see this.** `tests/executor.rs` never
/// touches Postgres, and the executor's own
/// `every_paused_run_names_its_resumer` proves only that *this* executor does
/// not write such a row — not that the column refuses one. The oracle is blind
/// for the ordinary reason instead: it drives `advance()`, which cannot build a
/// `Command::PauseRun` without a `Resume`, so the row this CHECK refuses is one
/// no execution it runs can produce. **That sentence read "no merged code has
/// ever written a `PAUSED` row at all", which W8 Task 3 had already falsified
/// in the commit before it** — `flowable::pause` emits `PauseRun`,
/// `claim::persist` writes `history.last().wire()`, and eight golden entries
/// store `'PAUSED'` on the way through. What is true is narrower and is the
/// half that matters: nothing writes one with a NULL `resume_with`.
///
/// Both directions, because a one-sided test passes against a CHECK that
/// refuses everything.
#[sqlx::test(migrations = "../../migrations")]
async fn a_paused_row_that_names_no_resumer_cannot_be_stored(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &one_task("a"), serde_json::json!({}))
        .await
        .expect("start execution");

    // The hole, exactly as a truncated `settle` or a future `Command` would
    // leave it: the state moves to `PAUSED` and the discriminant stays NULL.
    let refused = sqlx::query(
        "UPDATE workflow.task_runs SET state = 'PAUSED' WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .execute(&pool)
    .await;
    let err = refused.expect_err(
        "a PAUSED run with no `resume_with` was STORED — `resume_paused` answers \
         `Waiting` to this row for ever and no trace in `golden.json` can show it",
    );
    // Named, not merely "some error": a NOT NULL, a typo'd column or a dropped
    // table would all be `Err` here, and each would make this test pass while
    // testing nothing.
    assert!(
        err.to_string()
            .contains("workflow_task_runs_paused_names_its_resumer"),
        "refused by the wrong thing: {err}"
    );

    // **The other direction**, and it is not decoration: the identical UPDATE
    // that DOES name a resumer must still be accepted, or the constraint has
    // simply made pausing impossible.
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'PAUSED', resume_with = 'NEVER' \
           WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("an unbounded pause names `NEVER` and must be stored");
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("PAUSED"),
        "the legal pause landed"
    );

    // And the converse migration 0142 deliberately does NOT refuse: a bounded
    // pause that reached its terminal keeps its discriminant, because
    // `Command::SetRunState` does not clear `resume_with` (only
    // `Command::RestartLoop` does). This is `valids/pause-delay`'s resting
    // shape, and the biconditional form of the CHECK refuses it — see MD1.
    sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'SUCCESS', resume_with = 'TERMINAL:SUCCESS', not_before_ms = 1000 \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("a terminated bounded pause keeps its discriminant and must be stored");

    // **Migration 0143's value, exercised** -- W8 Task 5 fix round 3. `0143`
    // widened 0141's `workflow_task_runs_resume_with_known` CHECK with
    // `TERMINAL:WARNING`, for a `Pause` whose `timeout:` fires under
    // `allowFailure: true` (`valids/pause-timeout-allow-failure`). It was
    // **unexercised**: the literal appeared nowhere but the migration, and
    // deleting 0143 failed nothing -- the executor writes the row through
    // `Resume::wire`, and no test drove that path against Postgres.
    //
    // The row below is refused without 0143 and accepted with it, which is the
    // whole of what the migration changes. Its control is the `TERMINAL:SUCCESS`
    // write above: if the CHECK were dropped altogether BOTH would pass, so the
    // pair is what makes this an assertion about the vocabulary rather than
    // about Postgres accepting an UPDATE.
    sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'WARNING', resume_with = 'TERMINAL:WARNING', not_before_ms = 1000 \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect(
        "migration 0143 admits TERMINAL:WARNING -- a `Pause` whose `timeout:` \
         fires under `allowFailure: true`",
    );
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("WARNING"),
        "and the row really landed, so the write above is not a no-op UPDATE \
         matching zero rows"
    );

    // The negative half, so the CHECK is shown to still REFUSE something: a
    // discriminant 0143 did not add stays out.
    let refused = sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'CANCELLED', resume_with = 'TERMINAL:CANCELLED', not_before_ms = 1000 \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .execute(&pool)
    .await;
    assert!(
        refused.is_err(),
        "`TERMINAL:CANCELLED` is not in the vocabulary -- 0143 added one \
         discriminant, not an escape hatch. **This message read \"`CANCELLED` is \
         the state `RunState` still does not hold\"**, which W8 Task 6 falsified \
         two tasks later: `RunState::Cancelled` is slot 12 of `RunState::ALL` \
         and `is_terminal` lists it. The row is still refused, and the reason is \
         now the narrower one -- no `Pause` bound resolves to `CANCELLED`, \
         because `pause::deadline` produces SUCCESS, FAILED and (under \
         `allowFailure:`) WARNING and nothing else. A `sla:` CANCEL cancels the \
         EXECUTION and never a pause's bound"
    );
}

/// A `Pause` and a successor behind it, with or without a `pauseDuration:`.
///
/// The bound is an `Option` rather than two fixtures because the whole point of
/// [`a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken`]
/// is that the two flows differ in **exactly** that line: an unbounded `Pause`
/// must be invisible to the waker for a structural reason, not because its
/// fixture happened to be built differently. The successor is there so "the
/// wait ended" is observable as a task that ran rather than only as a state.
fn a_pause(id: &str, bound: Option<&str>) -> String {
    let duration = match bound {
        Some(d) => format!("\x20   pauseDuration: {d}\n"),
        None => String::new(),
    };
    format!(
        "id: {id}\n\
         namespace: t\n\
         tasks:\n\
         \x20 - id: pause\n\
         \x20   type: io.kestra.plugin.core.flow.Pause\n\
         {duration}\
         \x20 - id: last\n\
         \x20   type: io.kestra.plugin.core.log.Log\n\
         \x20   message: after\n"
    )
}

/// **W8 Task 7, Step 1: a bounded pause whose wait has elapsed is woken, one
/// whose wait has not is left alone, and an unbounded one is never woken at
/// all.**
///
/// `claim::due_retries` was gated to `'RETRYING'`/`'RETRIED'` from W4 Task 1
/// until this test. A bounded `Pause` parks its run `'PAUSED'` with a
/// `not_before_ms`, which matched neither arm, so **in the product** the wait
/// never ended however correct `advance()` was. W8 Task 3 gave `'PAUSED'` a
/// producer and Task 4 a second one, so this is a live defect rather than the
/// prediction it was when Task 2 wrote it down.
///
/// # The ratchet is green either way and this test is the only instrument
///
/// `oracle-exec/driver.rs`'s `pass_time` selects on `not_before_ms` with **no
/// state term at all** and then calls `worker::resume` itself, so the oracle
/// reaches these same runs through a different door. All eight pause entries
/// banked in W8 Task 3 with the gate narrow and stay banked with it wide:
/// `baseline.txt` cannot move for this fix and cannot move if it is reverted.
/// That is W4 item 15's shape — a property whose only instrument is an
/// integration test, because the oracle reaches the same runs through a door
/// that cannot fail — and it is why the assertion has to be here.
///
/// **The gloss item 15 used for that shape — "a path no deployment takes" — is
/// no longer true of this one, and this doc said it for seventeen commits.**
/// Since W10 Task 2 `egret-bin` runs the claim loop
/// (`egret_workflow::runtime::spawn`), so `runtime` → `tick` → `wake` →
/// `claim::due_retries` is a production path. `due_retries`' own doc was
/// corrected in that task and **this copy was not**; W10 Task 7's sweep found
/// it, and only by widening the pattern from the proposition item 15 states to
/// the *gloss* it states it in — the two share no words. The ratchet's
/// blindness is unchanged; what changed is that it is now blindness about
/// **shipping** behaviour rather than about a driver's, which makes this
/// assertion more load-bearing than it was, not less.
///
/// # Nothing rewinds a clock here
///
/// `executions.created_at` is untouched and the second elapses in real time,
/// exactly as in
/// [`a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`],
/// whose shape this follows. The three executions are driven together so the
/// waker has to **choose**: a predicate that named everything due would pass a
/// test that held only the due one.
///
/// **The unbounded arm is the over-acceptance guard.** Seven corpus flows write
/// a `Pause` with no bound and the artifact leaves every one of them `PAUSED`
/// for the full 90 s harvest, so a waker that resumed one would ship a wrong
/// product behind a green ratchet. It is refused **in the schema**: migration
/// 0141's `workflow_task_runs_resume_bound_matches` makes `resume_with =
/// 'NEVER'` beside a non-NULL `not_before_ms` unrepresentable, so the
/// pre-existing `not_before_ms IS NOT NULL` term cannot reach one. This asserts
/// that end of it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = Recorder::default();

    let due = start_execution(
        &pool,
        ws,
        None,
        &a_pause("pause-due", Some("PT1S")),
        serde_json::json!({}),
    )
    .await
    .expect("start due");
    let waiting = start_execution(
        &pool,
        ws,
        None,
        &a_pause("pause-waiting", Some("PT30S")),
        serde_json::json!({}),
    )
    .await
    .expect("start waiting");
    let unbounded = start_execution(
        &pool,
        ws,
        None,
        &a_pause("pause-unbounded", None),
        serde_json::json!({}),
    )
    .await
    .expect("start unbounded");

    // All three park in the same state. What separates them is the deadline,
    // which is the column this predicate reads.
    for (label, execution, floor) in [
        ("due", due, Some(1_000)),
        ("waiting", waiting, Some(30_000)),
        ("unbounded", unbounded, None),
    ] {
        assert_eq!(
            run_state(&pool, execution, "pause").await.as_deref(),
            Some("PAUSED"),
            "the {label} pause must be PAUSED before anything is measured: {}",
            stuck(&pool, execution, "pause").await
        );
        match floor {
            Some(floor) => {
                let deadline = not_before_of(&pool, execution, "pause")
                    .await
                    .expect("a bounded pause writes its deadline");
                assert!(
                    deadline >= floor,
                    "the {label} pause's deadline must be at least its own \
                     `pauseDuration:` past the execution's origin: {}",
                    stuck(&pool, execution, "pause").await
                );
            }
            None => assert_eq!(
                not_before_of(&pool, execution, "pause").await,
                None,
                "an unbounded pause must write NO deadline — `Resume::Never` is \
                 what keeps the environment's clock from being asked to move, \
                 and migration 0141 is what makes the pairing structural: {}",
                stuck(&pool, execution, "pause").await
            ),
        }
        assert!(
            run_state(&pool, execution, "last").await.is_none(),
            "the successor must not exist while the {label} pause is waiting: {}",
            stuck(&pool, execution, "pause").await
        );
    }

    // The `pauseDuration:` passes in REAL time. `executions.created_at` is
    // untouched — a rewind is `driver::pass_time`'s trick and it is exactly why
    // this hole survived a package.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    let woken = wake(&pool, 8).await.expect("wake");
    assert_eq!(
        woken,
        vec![due],
        "the waker must name the execution whose pause came due and no other. \
         due={due} (pauseDuration PT1S, elapsed); waiting={waiting} (PT30S, not \
         elapsed); unbounded={unbounded} (no bound, no deadline, never due). \
         It named {woken:?}. With the `'PAUSED'` arm removed from \
         `claim::due_retries` this is `[]` and every baselined figure is \
         unchanged"
    );

    let finished = drain_with(&pool, &reg).await;

    assert_eq!(
        run_state(&pool, due, "pause").await.as_deref(),
        Some("SUCCESS"),
        "a `pauseDuration:` that elapses ends its run SUCCESS — that terminal \
         was chosen by `pause::deadline` at pause time and carried on the \
         `Resume`. Instead: {}",
        stuck(&pool, due, "pause").await
    );
    assert_eq!(
        run_state(&pool, due, "last").await.as_deref(),
        Some("SUCCESS"),
        "the wait ending is what creates the successor, and nothing else in \
         this execution could: {}",
        stuck(&pool, due, "pause").await
    );
    assert_eq!(
        reg.seen(),
        vec!["last".to_string()],
        "exactly one task reached a handler — the successor. A `Pause` is a \
         flowable and never does"
    );
    assert_eq!(
        finished,
        1,
        "exactly one run's result committed after the wake: {:?}",
        reg.seen()
    );

    // The other two directions, which are what a widened predicate breaks.
    for (label, execution) in [("waiting", waiting), ("unbounded", unbounded)] {
        assert_eq!(
            run_state(&pool, execution, "pause").await.as_deref(),
            Some("PAUSED"),
            "the {label} pause must still be waiting 1.3 s in: {}",
            stuck(&pool, execution, "pause").await
        );
        assert!(
            run_state(&pool, execution, "last").await.is_none(),
            "and its successor must still not exist: {}",
            stuck(&pool, execution, "pause").await
        );
    }

    // **The state term's own assertion, and the one that names it.**
    // `Command::SetRunState` moves a bounded pause to its terminal WITHOUT
    // clearing `not_before_ms` — only `Command::RestartLoop` clears one — so
    // `due`'s run still carries a deadline that is still in the past. The state
    // term is the only thing between the waker and a finished execution it
    // would relock and re-settle on every tick for ever.
    let again = wake(&pool, 8).await.expect("wake again");
    assert!(
        again.is_empty(),
        "the waker named {again:?} when nothing is waiting on an elapsed \
         deadline. Due side: {}. Waiting side: {}. Unbounded side: {}",
        stuck(&pool, due, "pause").await,
        stuck(&pool, waiting, "pause").await,
        stuck(&pool, unbounded, "pause").await
    );
}

/// **W8 Task 7, Step 1's other half: the waker's state vocabulary is a
/// PARTITION of `RunState::ALL`, and a fourteenth variant cannot be answered by
/// omission.**
///
/// W8's first predicate class: *adding a variant to a state enum silently
/// falsifies every predicate that enumerates a SUBSET of it, and the compiler
/// cannot find them, because they are defined by an OMISSION.* `claim::due_retries`'s
/// `WHERE` is exactly such a subset, written in SQL where not even an
/// exhaustive `match` could help — `Warning` and `Cancelled` were added to this
/// enum in W8 Tasks 5 and 6 and nothing asked this predicate about either.
///
/// So the question is asked here for **every** member of `RunState::ALL`, with
/// a verdict each, in the shape `neither_paused_nor_restarted_is_terminal` uses
/// for `is_terminal`: the expectation table's key set is asserted equal to
/// `ALL`'s, so a variant added tomorrow **fails this test** rather than
/// silently joining the unnamed majority.
///
/// # What it measures, and why it is `due_retries` rather than `wake`
///
/// The SELECT, not the settle. `claim::persist` recomputes `task_runs.state`
/// from `history.last()`, so a fixture that hand-writes a state and then goes
/// through `wake` would have the very column under test overwritten before it
/// could be read back. `due_retries` is `pub` for this.
///
/// The rows are hand-written for the same reason: no `advance()` can produce a
/// `KILLED` run holding a live deadline, and that is the point — the predicate
/// must be safe against a row the schema admits, not only against rows this
/// executor writes today. `resume_with` is set on all of them because migration
/// 0142 refuses a `'PAUSED'` row without one, and 0141 refuses a non-`'NEVER'`
/// discriminant without a `not_before_ms`.
///
/// # Three verdicts, and each has a named reason
///
/// | state | named | why |
/// |---|---|---|
/// | `RETRYING` | yes | `retry::reopen_due` puts it back to `CREATED` |
/// | `PAUSED` | yes | `flowable::resume_paused` ends the wait — W8 Task 7 |
/// | `RETRIED` | yes | `replay::spawn_due` starts the new execution |
/// | the other ten | no | nothing moves a run in them, so a named one would be relocked and re-settled on every tick for ever |
///
/// **`RESTARTED` is the one to look at twice**, because it is the other
/// non-terminal state W8 Task 2 added and "non-terminal" is not the rule here.
/// Nothing pushes it onto a *run* — `pause_mark` writes it to the
/// **execution**'s history, 0 of the golden's 840 run histories hold it against
/// 12 execution histories that do — and `Command::RestartLoop`, the one command
/// that ends a `LoopUntil`'s wait, clears `not_before` and `resume_with` in the
/// same arm.
///
/// # The clock control
///
/// A fourteenth execution is `'PAUSED'` with a deadline far in the future. It
/// is what fails if the clock term is dropped while the state terms are kept,
/// which a state-only table could not see.
#[sqlx::test(migrations = "../../migrations")]
async fn the_waker_names_exactly_the_states_something_will_move(pool: sqlx::PgPool) {
    use egret_workflow::executor::RunState;
    use std::collections::{BTreeMap, BTreeSet};

    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    // The verdict for every member of `RunState::ALL`, spelled out. The key set
    // is checked against `ALL` below, so this table cannot go stale by omission.
    let expected: BTreeMap<&str, bool> = BTreeMap::from([
        ("CREATED", false),
        ("SUBMITTED", false),
        ("RUNNING", false),
        ("SUCCESS", false),
        ("FAILED", false),
        ("RETRYING", true),
        ("RETRIED", true),
        ("KILLED", false),
        ("SKIPPED", false),
        ("PAUSED", true),
        ("RESTARTED", false),
        ("WARNING", false),
        ("CANCELLED", false),
    ]);

    let vocabulary: BTreeSet<&str> = RunState::ALL.iter().map(|s| s.wire()).collect();
    let tabled: BTreeSet<&str> = expected.keys().copied().collect();
    assert_eq!(
        tabled, vocabulary,
        "this table must answer for EVERY `RunState`. A variant in `ALL` and \
         not here is the omission that made `Warning` and `Cancelled` silently \
         un-asked; one here and not in `ALL` is a state this predicate can \
         never see"
    );

    // One execution per state, each holding a run whose deadline has passed.
    let mut execution_of: BTreeMap<&str, Uuid> = BTreeMap::new();
    for state in expected.keys().copied() {
        let ex = start_execution(
            &pool,
            ws,
            None,
            &one_task(&format!("vocab-{}", state.to_lowercase())),
            serde_json::json!({}),
        )
        .await
        .expect("start execution");
        sqlx::query(
            "UPDATE workflow.task_runs \
                SET state = $2, not_before_ms = 0, resume_with = 'TERMINAL:SUCCESS' \
              WHERE execution_id = $1 AND task_id = 'a'",
        )
        .bind(ex)
        .bind(state)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("store a due run in {state}: {e}"));
        execution_of.insert(state, ex);
    }

    // The clock control: `PAUSED`, which the state terms DO name, with a
    // deadline an hour out.
    let not_yet = start_execution(
        &pool,
        ws,
        None,
        &one_task("vocab-not-yet"),
        serde_json::json!({}),
    )
    .await
    .expect("start not-yet");
    sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'PAUSED', not_before_ms = 3600000, resume_with = 'TERMINAL:SUCCESS' \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(not_yet)
    .execute(&pool)
    .await
    .expect("store a PAUSED run whose deadline has not arrived");

    let named: BTreeSet<Uuid> = due_retries(&pool, 64)
        .await
        .expect("due_retries")
        .into_iter()
        .collect();

    // Positive control first: a `due_retries` that returned nothing at all —
    // a broken fixture, a CHECK that silently refused every UPDATE — would
    // satisfy every `false` row below on its own.
    assert!(
        !named.is_empty(),
        "the waker named NOBODY out of {} due runs. Every negative row below is \
         unfalsifiable until this holds",
        expected.len()
    );

    let mut wrong: Vec<String> = Vec::new();
    for (state, want) in &expected {
        let ex = execution_of[state];
        let got = named.contains(&ex);
        if got != *want {
            wrong.push(format!(
                "{state}: expected named={want}, got named={got} (execution {ex})"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the waker's state vocabulary has moved:\n  {}\n\
         Named: {:?}. Removing the `'PAUSED'` arm shows up here as \
         `PAUSED: expected named=true, got named=false`",
        wrong.join("\n  "),
        named
    );

    assert!(
        !named.contains(&not_yet),
        "the waker named a `PAUSED` run whose deadline is an hour away \
         (execution {not_yet}) — the state terms are not the only terms"
    );
}

/// **W8 Task 7 final round: a `PAUSED` run inside a TERMINAL execution is never
/// named, because `advance()` would write nothing and the waker would name it
/// again on the next tick, for ever.**
///
/// The first version of the `'PAUSED'` arm had no execution-terminality term,
/// and the justification for not needing one was **false**: it said
/// `flowable::resume_paused` moves a due run on the very settle this waker's
/// `resume` produces, so the stale deadline lands in a state the predicate does
/// not name. `advance()` returns on its **first line** when the execution is
/// terminal —
///
/// ```ignore
/// if state.history.last().is_some_and(|s| s.is_terminal()) { return Vec::new(); }
/// ```
///
/// — and `RunState::Paused` is **not** terminal, so `claim::sla_terminal` will
/// `CANCEL` an execution that is still holding a parked bounded `Pause`. The
/// run keeps `state = 'PAUSED'` and its `not_before_ms` for ever, every tick
/// names it, and every tick relocks the execution and settles it into writing
/// nothing but a new `executions.seq`. **That is the endless case the
/// `'RETRIED'` arm needed two extra terms to avoid, reached by a third road.**
///
/// # Both directions, and the control is the whole test
///
/// One `assert!(named.is_empty())` would pass against a broken fixture, a
/// refused `UPDATE`, or a waker that named nobody at all. So the two rows are
/// **the same row twice**, differing only in `executions.state`: one `CANCELLED`
/// (terminal), one `RUNNING` (not). The second must be named. A predicate that
/// dropped the new term fails on the first; one that dropped the `'PAUSED'` arm
/// entirely fails on the second.
///
/// # Why the row is written by hand
///
/// **No merged code produces this pairing**, which is the point: it is latent,
/// not shipped. No corpus flow declares both an `sla:` and a `Pause`, so the
/// oracle has never built one and cannot. The row the SCHEMA admits is what the
/// predicate has to be safe against — the same argument migration 0142 rests
/// on, and the same one
/// `the_waker_names_exactly_the_states_something_will_move` rests on.
///
/// The terminal list in the SQL is **derived from `RunState::ALL`** by
/// `is_terminal()`, so this test also fixes the *direction* a fourteenth
/// terminal variant takes: it joins the exclusion automatically rather than
/// waiting to be remembered.
#[sqlx::test(migrations = "../../migrations")]
async fn a_paused_run_in_a_terminal_execution_is_never_named(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    let mut ids: Vec<(&str, Uuid)> = Vec::new();
    for (label, execution_state) in [("cancelled", "CANCELLED"), ("running", "RUNNING")] {
        let ex = start_execution(
            &pool,
            ws,
            None,
            &one_task(&format!("terminal-{label}")),
            serde_json::json!({}),
        )
        .await
        .expect("start execution");
        // A bounded pause, parked and due. `resume_with` is required by 0142
        // and its non-`NEVER` value is required by 0141 to sit beside a
        // deadline, so this is the shape `Command::PauseRun` actually writes.
        sqlx::query(
            "UPDATE workflow.task_runs \
                SET state = 'PAUSED', not_before_ms = 0, resume_with = 'TERMINAL:SUCCESS' \
              WHERE execution_id = $1 AND task_id = 'a'",
        )
        .bind(ex)
        .execute(&pool)
        .await
        .expect("park a due bounded pause");
        sqlx::query("UPDATE workflow.executions SET state = $2 WHERE id = $1")
            .bind(ex)
            .bind(execution_state)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("set the execution {execution_state}: {e}"));
        ids.push((label, ex));
    }

    let named: std::collections::BTreeSet<Uuid> = due_retries(&pool, 64)
        .await
        .expect("due_retries")
        .into_iter()
        .collect();

    let cancelled = ids[0].1;
    let running = ids[1].1;

    // The control first: without it every assertion below is satisfied by a
    // waker that named nobody.
    assert!(
        named.contains(&running),
        "the waker did not name the RUNNING execution ({running}) holding a due \
         bounded pause, so this test cannot say anything about the CANCELLED \
         one. It named {named:?}"
    );
    assert!(
        !named.contains(&cancelled),
        "the waker named execution {cancelled}, whose own state is CANCELLED. \
         `advance()` returns an empty Vec on its first line for a terminal \
         execution, so `resume` would write nothing but a new \
         `executions.seq` — and the run stays PAUSED with its deadline, so the \
         next tick names it again, for ever. It named {named:?}"
    );
}

/// **W8 Task 7 final round part 2: one execution-terminality term over every
/// arm, and the ONE arm that is legitimately exempt.**
///
/// The term the round before this added sat on the `'PAUSED'` arm alone, and
/// the `'RETRYING'` twin — the identical endless case, reached by an `sla:`
/// `CANCEL` over a waiting retry instead of a parked pause — was *recorded as a
/// finding*. **A recorded divergence is not a closed one**, and fixing one
/// member of a family while its sibling stands is this branch's named failure
/// mode in its structural form rather than its documentary one. The term is
/// hoisted now:
///
/// ```sql
/// AND (r.state = ANY($2) OR NOT (e.state = ANY($2)))
/// ```
///
/// # The 3×2, and both halves of the term are pinned by it
///
/// | run state | in a TERMINAL execution | in a RUNNING one |
/// |---|---|---|
/// | `RETRYING` | **not named** — `retry::reopen_due` is inside `advance()`, which returns `Vec::new()` on its first line | named |
/// | `PAUSED` | **not named** — `flowable::resume_paused` likewise | named |
/// | `RETRIED` | **NAMED** — the exemption | named |
///
/// The left column pins `NOT (e.state = ANY($2))`; the right column is the
/// control without which every "not named" row is satisfied by a waker that
/// named nobody; and the `RETRIED`/terminal cell pins `r.state = ANY($2)`,
/// which a test of the guard alone would have deleted without noticing.
///
/// # The exemption is measured, not defensive
///
/// A `CREATE_NEW_EXECUTION` retry ends **this execution** `RETRIED`, so the
/// whole population of that arm lives in a terminal execution **by
/// construction** — the golden says so in as many words: every
/// `valids/retry-new-execution-*` link reads execution
/// `["CREATED","RUNNING","FAILED","RETRIED"]` beside run
/// `["CREATED","SUBMITTED","RUNNING","FAILED","RETRIED"]`. The naive hoist —
/// the same term without `r.state = ANY($2)` — is **mutation MX6**, and it kills
/// this test AND two of W6 Task 4's, 3 runs of 3:
/// `a_due_replay_is_woken_and_the_waker_stops_naming_it_once_it_exists` and
/// `a_replay_is_refused_inside_a_loop_sub_execution_and_taken_at_the_top_level`
/// — assertions that **predate** the term and therefore prove it was not
/// written to fit them. **The ratchet does NOT move: 109 under MX6, 3 runs of
/// 3.** (I predicted 109 → 113 and was wrong, for this waker's own reason — the
/// oracle reaches a due replay through `driver::pass_time`, never through this
/// query.) So the cell below is not a weakening of the guard; it is the arm
/// whose consumer,
/// `worker::replay::spawn_due`, runs in the settle **outside** `advance()` and
/// is therefore untouched by the early return the guard is about.
///
/// **This test postdates both defects and is not evidence that anything caught
/// either of them before.** It exists because a review read the guard and found
/// the `RETRYING` half unguarded and the `RETRIED` half unmeasured.
#[sqlx::test(migrations = "../../migrations")]
async fn a_terminal_execution_hides_every_arm_except_the_replay(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    // (run state, execution state, must the waker name it?)
    //
    // `CANCELLED` is the terminal used throughout because it is the one an
    // `sla:` actually produces over a live execution -- the reachable form of
    // this row rather than an arbitrary terminal.
    let cases: [(&str, &str, bool); 6] = [
        ("RETRYING", "CANCELLED", false),
        ("PAUSED", "CANCELLED", false),
        ("RETRIED", "RETRIED", true),
        ("RETRYING", "RUNNING", true),
        ("PAUSED", "RUNNING", true),
        ("RETRIED", "RUNNING", true),
    ];

    let mut planted: Vec<(&str, &str, bool, Uuid)> = Vec::new();
    for (run_state, execution_state, want) in cases {
        let ex = start_execution(
            &pool,
            ws,
            None,
            &one_task(&format!(
                "hoist-{}-{}",
                run_state.to_lowercase(),
                execution_state.to_lowercase()
            )),
            serde_json::json!({}),
        )
        .await
        .expect("start execution");
        // 0142 requires a `resume_with` for a `PAUSED` row and 0141 requires a
        // deadline beside any non-`NEVER` discriminant, so this is the shape
        // `Command::PauseRun` really writes -- and it is harmless on the other
        // two, whose own CHECK is one-way (see 0142's "the converse is FALSE").
        sqlx::query(
            "UPDATE workflow.task_runs \
                SET state = $2, not_before_ms = 0, resume_with = 'TERMINAL:SUCCESS' \
              WHERE execution_id = $1 AND task_id = 'a'",
        )
        .bind(ex)
        .bind(run_state)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("plant a due {run_state} run: {e}"));
        sqlx::query("UPDATE workflow.executions SET state = $2 WHERE id = $1")
            .bind(ex)
            .bind(execution_state)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("set the execution {execution_state}: {e}"));
        planted.push((run_state, execution_state, want, ex));
    }

    let named: std::collections::BTreeSet<Uuid> = due_retries(&pool, 64)
        .await
        .expect("due_retries")
        .into_iter()
        .collect();

    // The positive control before any negative row is believed.
    assert!(
        !named.is_empty(),
        "the waker named NOBODY out of {} planted rows; every `false` row below \
         is unfalsifiable until this holds",
        planted.len()
    );

    let mut wrong: Vec<String> = Vec::new();
    for (run_state, execution_state, want, ex) in &planted {
        let got = named.contains(ex);
        if got != *want {
            wrong.push(format!(
                "run {run_state} in a {execution_state} execution: expected \
                 named={want}, got named={got} (execution {ex})"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the execution-terminality term has moved:\n  {}\n\
         Making it vacuous flips the TWO non-terminal-run rows -- \
         `RETRYING`/`CANCELLED` and `PAUSED`/`CANCELLED` -- to named=true, and \
         leaves the `RETRIED` row alone, which is already exempt; dropping its \
         `r.state = ANY($2)` exemption flips `RETRIED in a RETRIED execution` \
         to named=false and also kills two of W6 Task 4's replay tests. \
         Neither moves the ratchet.\nNamed: {named:?}",
        wrong.join("\n  ")
    );
}

/// **The reason `start_execution_in` takes `conn` and not `pool`**: W12 Task
/// 4's scheduler creates the fired execution inside the SAME transaction that
/// advances the `Schedule` trigger's `next_evaluation_date`, so a crash
/// between the two can never leave one without the other. This is the
/// property that makes that safe to rely on -- a caller that rolls back its
/// own transaction leaves no execution behind, and a caller that commits
/// leaves exactly one.
#[sqlx::test(migrations = "../../migrations")]
async fn start_execution_in_commits_nothing_when_the_caller_rolls_back(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (db, ws) = seed(&pool).await;
    let source = one_task("rollback-me");

    let mut tx = pool.begin().await.expect("begin");
    start_execution_in(
        &mut tx,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &source,
            inputs: serde_json::json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
        },
    )
    .await
    .expect("start_execution_in");
    // The rollback: dropped, never committed.
    drop(tx);

    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "a transaction the caller never committed must leave no row behind"
    );

    let mut tx = pool.begin().await.expect("begin");
    start_execution_in(
        &mut tx,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &source,
            inputs: serde_json::json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
        },
    )
    .await
    .expect("start_execution_in");
    tx.commit().await.expect("commit");

    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "and the same call, committed, leaves exactly one"
    );
}

/// 0156's `workflow_executions_scheduled_date_iff_schedule` CHECK, from the
/// caller's side: a `scheduled_date` with no `Schedule` trigger record must be
/// refused as an error, not accepted and not a panic.
#[sqlx::test(migrations = "../../migrations")]
async fn scheduled_date_is_refused_without_a_schedule_trigger(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = one_task("scheduled-no-trigger");

    let mut tx = pool.begin().await.expect("begin");
    let err = start_execution_in(
        &mut tx,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &source,
            inputs: serde_json::json!({}),
            trigger: None,
            scheduled_date: Some(chrono::Utc::now()),
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
        },
    )
    .await
    .expect_err(
        "scheduled_date with no execution_trigger must violate the CHECK, \
         not succeed",
    );
    // Named, not merely "some error": a NOT NULL, a typo'd column or a
    // dropped constraint would all be `Err` here too, and each would make
    // this test pass while testing nothing.
    assert!(
        err.to_string()
            .contains("workflow_executions_scheduled_date_iff_schedule"),
        "refused by the wrong thing: {err}"
    );
}

// ===========================================================================
// W13a Task 3: the link and depth on `NewExecution`, the source's render
// context, and the Flow-trigger outbox hook in `settle` (W13 spec §4.3-§4.5).
// ===========================================================================

/// A flow that declares one `Flow` trigger, `on`, with the given `states:`
/// line (empty for Kestra's defaults). Filed through `FlowRepo`, whose writers
/// call `scheduler::sync`, so the `workflow.flow_listeners` row is the one
/// production writes -- never a hand-inserted row that could disagree with it.
fn listener(id: &str, states: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntriggers:\n  - id: on\n    \
         type: io.kestra.plugin.core.trigger.Flow\n{states}tasks:\n  \
         - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: heard\n"
    )
}

/// The outbox events `source` produced, in the order they were written.
async fn events_of(pool: &sqlx::PgPool, source: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as(
        "SELECT workspace_id, state FROM workflow.flow_trigger_events \
          WHERE source_execution_id = $1 ORDER BY id",
    )
    .bind(source)
    .fetch_all(pool)
    .await
    .expect("read outbox events")
}

async fn all_events(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.flow_trigger_events")
        .fetch_one(pool)
        .await
        .expect("count outbox events")
}

/// **`start_execution_in` writes the three new columns it is handed.** The
/// depth is `7`, not `1`, so a writer that ignored the field and let the
/// column default (`0`) apply -- or wrote a constant -- cannot pass; the event
/// id is likewise a value no default produces.
#[sqlx::test(migrations = "../../migrations")]
async fn a_start_writes_trigger_execution_id_and_depth(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = start_execution(&pool, ws, None, &one_task("source"), serde_json::json!({}))
        .await
        .expect("start the source");

    let mut tx = pool.begin().await.expect("begin");
    let fired = start_execution_in(
        &mut tx,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &one_task("fired"),
            inputs: serde_json::json!({}),
            trigger: Some(serde_json::json!({
                "id": "on",
                "type": "io.kestra.plugin.core.trigger.Flow",
                "variables": {"executionId": source.to_string(), "state": "SUCCESS"},
            })),
            scheduled_date: None,
            trigger_execution_id: Some(source),
            execution_depth: 7,
            flow_trigger_event_id: Some(4242),
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
        },
    )
    .await
    .expect("start the fired execution");
    tx.commit().await.expect("commit");

    let row: (Option<Uuid>, i32, Option<i64>, Option<i32>) = sqlx::query_as(
        "SELECT trigger_execution_id, execution_depth, flow_trigger_event_id, trigger_run_ix \
           FROM workflow.executions WHERE id = $1",
    )
    .bind(fired)
    .fetch_one(&pool)
    .await
    .expect("read the fired row");
    assert_eq!(
        row,
        (Some(source), 7, Some(4242), None),
        "(trigger_execution_id, execution_depth, flow_trigger_event_id, \
         trigger_run_ix): the link and the depth are what the caller passed, and \
         there is no run ordinal -- which is what keeps a flow-fired execution's \
         terminal from being reported to its source as a subflow child's would be"
    );
}

/// **0157's `workflow_executions_flow_trigger_event_iff_flow`, both halves, by
/// hand-inserted rows.** An event id on a row whose trigger is not a `Flow` is
/// refused (the `Schedule` record here is otherwise legal, as 0156's own CHECK
/// is satisfied), a `Flow` record without one is refused, and the pair
/// together is accepted -- the control that shows the refusals are about the
/// pairing and not about the fixture.
#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_trigger_event_id_is_refused_on_anything_but_a_flow_fire(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let insert = |trigger: serde_json::Value, scheduled: bool, event: Option<i64>| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO workflow.executions \
                   (workspace_id, namespace, flow_key, source, state, history, \
                    execution_trigger, scheduled_date, flow_trigger_event_id) \
                 VALUES ($1, 't', 'f', 'id: f', 'CREATED', '[\"CREATED\"]'::jsonb, $2, \
                         CASE WHEN $3 THEN now() END, $4)",
            )
            .bind(ws)
            .bind(trigger)
            .bind(scheduled)
            .bind(event)
            .execute(&pool)
            .await
        }
    };
    let schedule = serde_json::json!({
        "id": "s", "type": "io.kestra.plugin.core.trigger.Schedule", "variables": {}
    });
    let flow = serde_json::json!({
        "id": "on", "type": "io.kestra.plugin.core.trigger.Flow", "variables": {}
    });

    let err = insert(schedule, true, Some(1))
        .await
        .expect_err("an event id on a Schedule fire must be refused");
    assert!(
        err.to_string()
            .contains("workflow_executions_flow_trigger_event_iff_flow"),
        "refused by the wrong thing: {err}"
    );
    let err = insert(flow.clone(), false, None)
        .await
        .expect_err("a Flow fire with no event id must be refused");
    assert!(
        err.to_string()
            .contains("workflow_executions_flow_trigger_event_iff_flow"),
        "refused by the wrong thing: {err}"
    );
    insert(flow, false, Some(1))
        .await
        .expect("control: a Flow fire carrying its event id is accepted");
}

/// **The backstop keys on the EVENT, so re-entering a state fires again** --
/// the controller's ruling on Task 2's design finding. The same listener
/// (flow + trigger id) fired twice for one event is refused by
/// `workflow_executions_one_per_flow_trigger_event`; fired for a SECOND event
/// from the same source and the same state -- a source that went PAUSED,
/// RUNNING, PAUSED -- it is accepted. The old key,
/// `(source, flow, trigger id, variables.state)`, refused that second fire.
#[sqlx::test(migrations = "../../migrations")]
async fn one_event_fires_a_listener_once_and_a_second_event_in_the_same_state_fires_again(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let target = listener("target", "");
    let flow = file_flow(&db, ws, user, &target).await;
    let source = start_execution(&pool, ws, None, &one_task("src"), serde_json::json!({}))
        .await
        .expect("start the source");

    let fire = |event: i64| {
        let pool = pool.clone();
        let target = target.clone();
        async move {
            let mut tx = pool.begin().await.expect("begin");
            let r = start_execution_in(
                &mut tx,
                NewExecution {
                    workspace_id: ws,
                    flow_id: Some(flow),
                    source: &target,
                    inputs: serde_json::json!({}),
                    trigger: Some(serde_json::json!({
                        "id": "on",
                        "type": "io.kestra.plugin.core.trigger.Flow",
                        "variables": {"executionId": source.to_string(), "state": "PAUSED"},
                    })),
                    scheduled_date: None,
                    trigger_execution_id: Some(source),
                    execution_depth: 1,
                    flow_trigger_event_id: Some(event),
                    born_failed: false,
                    replayed_from_execution_id: None,
                    id: None,
                    input_files: Vec::new(),
                },
            )
            .await;
            if r.is_ok() {
                tx.commit().await.expect("commit");
            }
            r
        }
    };

    fire(10).await.expect("the first fire for event 10");
    let err = fire(10)
        .await
        .expect_err("a second fire of the same listener for the same event");
    assert!(
        err.to_string()
            .contains("workflow_executions_one_per_flow_trigger_event"),
        "refused by the wrong thing: {err}"
    );
    fire(11)
        .await
        .expect("a NEW event from the same source in the same state fires again");
}

/// **A transition a listener can hear writes exactly one event, and one it
/// cannot hear writes none.** The listener hears `SUCCESS` only, so the
/// `RUNNING` the start produces is the in-workspace negative -- the `$3 =
/// ANY (l.states)` half of the `EXISTS` -- and the drive to `SUCCESS` is the
/// positive.
#[sqlx::test(migrations = "../../migrations")]
async fn a_transition_writes_one_event_when_a_listener_can_hear_it(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(
        &db,
        ws,
        user,
        &listener("hears-success", "    states: [SUCCESS]\n"),
    )
    .await;

    let source = start_execution(&pool, ws, None, &one_task("src"), serde_json::json!({}))
        .await
        .expect("start the source");
    assert_eq!(
        execution_history(&pool, source).await,
        serde_json::json!(["CREATED", "RUNNING"]),
        "control: the start's own settle ended RUNNING"
    );
    assert_eq!(
        events_of(&pool, source).await,
        vec![],
        "RUNNING is not in the listener's states, so it writes no event"
    );

    drain_with(&pool, &Recorder::default()).await;
    assert_eq!(
        execution_history(&pool, source).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        "control: the source finished"
    );
    assert_eq!(
        events_of(&pool, source).await,
        vec![(ws, "SUCCESS".to_string())],
        "exactly one event, for the SUCCESS transition, in the source's workspace"
    );
}

/// **`PAUSED` writes an event, and it is not terminal.** The listener declares
/// no `states:`, so it hears Kestra's defaults -- the terminated states plus
/// `PAUSED`. A hook placed inside `notifies_parent`'s terminal-only branch
/// would write nothing here, which is the mutation this test is for.
///
/// The start's one settle appends `RUNNING` and `PAUSED`; the defaults do not
/// include `RUNNING`, so this start writes ONE event, `PAUSED`. The
/// per-state `EXISTS` is what drops the `RUNNING`, and
/// `one_settle_that_appends_two_listenable_states_writes_both_in_order` is the
/// same start heard by a listener that includes it.
#[sqlx::test(migrations = "../../migrations")]
async fn paused_writes_an_event_for_default_states(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(&db, ws, user, &listener("hears-defaults", "")).await;

    let source = start_execution(
        &pool,
        ws,
        None,
        &a_pause("src", Some("PT30S")),
        serde_json::json!({}),
    )
    .await
    .expect("start the source");
    assert_eq!(
        execution_history(&pool, source).await,
        serde_json::json!(["CREATED", "RUNNING", "PAUSED"]),
        "control: the source is paused, not terminal"
    );
    assert_eq!(
        events_of(&pool, source).await,
        vec![(ws, "PAUSED".to_string())]
    );
}

/// **No listener in the source's workspace, no event** -- and the listener
/// that exists, in ANOTHER workspace, is live: a source there, driven the same
/// way, does write one. Without that control the negative would pass against a
/// hook that wrote nothing at all.
#[sqlx::test(migrations = "../../migrations")]
async fn no_listener_in_the_workspace_writes_no_event(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (quiet, _) = common::seed_workspace(&db).await;
    let (other, other_user) = common::seed_workspace(&db).await;
    file_flow(
        &db,
        other,
        other_user,
        &listener("elsewhere", "    states: [SUCCESS]\n"),
    )
    .await;

    let unheard = start_execution(
        &pool,
        quiet,
        None,
        &one_task("unheard"),
        serde_json::json!({}),
    )
    .await
    .expect("start in the quiet workspace");
    let heard = start_execution(
        &pool,
        other,
        None,
        &one_task("heard"),
        serde_json::json!({}),
    )
    .await
    .expect("start in the listening workspace");
    drain_with(&pool, &Recorder::default()).await;

    for (label, execution) in [("unheard", unheard), ("heard", heard)] {
        assert_eq!(
            execution_history(&pool, execution).await,
            serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
            "control: the {label} source finished"
        );
    }
    assert_eq!(
        events_of(&pool, heard).await,
        vec![(other, "SUCCESS".to_string())],
        "control: the other workspace's listener is live and hears its own source"
    );
    assert_eq!(
        events_of(&pool, unheard).await,
        vec![],
        "a listener in another workspace must not make this workspace's \
         transition an event"
    );
    assert_eq!(all_events(&pool).await, 1, "and nothing else was written");
}

/// **Atomic with the transition**: the event is written on the caller's
/// connection, so a transaction that rolls back leaves no event, and the same
/// start committed leaves exactly one. The listener hears `RUNNING`, which the
/// start's own settle reaches -- so the event is written inside
/// `start_execution_in`'s caller's transaction, and is visible there before
/// the rollback, which shows the negative is a rollback and not an absence.
#[sqlx::test(migrations = "../../migrations")]
async fn a_rolled_back_transition_leaves_no_event(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(
        &db,
        ws,
        user,
        &listener("hears-running", "    states: [RUNNING]\n"),
    )
    .await;
    let source = one_task("src");
    let new = || NewExecution {
        workspace_id: ws,
        flow_id: None,
        source: &source,
        inputs: serde_json::json!({}),
        trigger: None,
        scheduled_date: None,
        trigger_execution_id: None,
        execution_depth: 0,
        flow_trigger_event_id: None,
        born_failed: false,
        replayed_from_execution_id: None,
        id: None,
        input_files: Vec::new(),
    };

    let mut tx = pool.begin().await.expect("begin");
    start_execution_in(&mut tx, new()).await.expect("start");
    let inside: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.flow_trigger_events")
        .fetch_one(&mut *tx)
        .await
        .expect("count inside the transaction");
    assert_eq!(
        inside, 1,
        "control: the RUNNING event exists inside the transaction"
    );
    drop(tx);
    assert_eq!(
        all_events(&pool).await,
        0,
        "rolled back with its transition"
    );

    let mut tx = pool.begin().await.expect("begin");
    let committed = start_execution_in(&mut tx, new()).await.expect("start");
    tx.commit().await.expect("commit");
    assert_eq!(
        events_of(&pool, committed).await,
        vec![(ws, "RUNNING".to_string())],
        "and committed with it"
    );
}

/// **`execution_render_context` is the finished SOURCE's context**: its posted
/// `inputs` and its tasks' `outputs` render, and `trigger` does NOT -- exactly
/// as W13a Task 1 measured Kestra failing `{{ trigger.executionId }}` in a
/// Flow trigger's `inputs` ("Unable to find `trigger`"). Task 4 turns that
/// failure into an execution born FAILED (Kestra's behaviour), so the render
/// failure itself is asserted here where the context is built.
#[sqlx::test(migrations = "../../migrations")]
async fn execution_render_context_renders_the_sources_inputs_and_outputs(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: src\nnamespace: t\ninputs:\n  - id: x\n    type: STRING\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ inputs.x }}-out\"\n";
    let ex = start_execution(&pool, ws, None, source, serde_json::json!({"x": "posted"}))
        .await
        .expect("start the source");
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    assert_eq!(
        execution_history(&pool, ex).await,
        serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
        "control: the source finished"
    );

    let mut conn = pool.acquire().await.expect("acquire");
    let cx = egret_workflow::worker::execution_render_context(&mut conn, ex)
        .await
        .expect("the source's render context");
    assert_eq!(
        egret_workflow::render_str("{{ inputs.x }}", &cx).expect("inputs render"),
        "posted"
    );
    assert_eq!(
        egret_workflow::render_str("{{ outputs.a.value }}", &cx).expect("outputs render"),
        "posted-out",
        "the task's persisted output, not the input it was rendered from"
    );
    let refused = egret_workflow::render_str("{{ trigger.executionId }}", &cx);
    assert!(
        refused.is_err(),
        "`trigger` is not in the source's context, as on Kestra; got {refused:?}"
    );
}

/// **A settle that appends two listenable states writes two events, in
/// order** -- the fix to Task 3's concern (1). A bounded `Pause` start goes
/// `CREATED -> RUNNING -> PAUSED` inside ONE settle; Kestra's
/// `processFlowTriggers` runs once per transition, so a listener on
/// `[RUNNING, PAUSED]` hears both. A hook that compared only the last state
/// before and after the settle wrote `PAUSED` alone.
#[sqlx::test(migrations = "../../migrations")]
async fn one_settle_that_appends_two_listenable_states_writes_both_in_order(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(
        &db,
        ws,
        user,
        &listener("hears-both", "    states: [RUNNING, PAUSED]\n"),
    )
    .await;

    let source = start_execution(
        &pool,
        ws,
        None,
        &a_pause("src", Some("PT30S")),
        serde_json::json!({}),
    )
    .await
    .expect("start the source");
    assert_eq!(
        execution_history(&pool, source).await,
        serde_json::json!(["CREATED", "RUNNING", "PAUSED"]),
        "control: RUNNING and PAUSED were both appended by the start's one settle"
    );
    assert_eq!(
        events_of(&pool, source).await,
        vec![(ws, "RUNNING".to_string()), (ws, "PAUSED".to_string())],
        "one event per appended state, in the order they were appended"
    );
}

/// **PAUSED, resumed, PAUSED again writes two `PAUSED` events** -- the
/// re-entry case `workflow_executions_one_per_flow_trigger_event` is keyed on
/// the event for. Two bounded pauses in sequence: the first comes due in real
/// time, the waker resumes it, and the settle that ends it enters the second.
/// That settle starts and ends at `PAUSED`, so a hook gated on "the last state
/// changed" writes nothing for it.
#[sqlx::test(migrations = "../../migrations")]
async fn paused_resumed_and_paused_again_writes_two_paused_events(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    file_flow(&db, ws, user, &listener("hears-defaults", "")).await;
    let two_pauses = "id: src\nnamespace: t\ntasks:\n\
         \x20 - id: first\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n\
         \x20 - id: second\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT30S\n\
         \x20 - id: last\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";

    let source = start_execution(&pool, ws, None, two_pauses, serde_json::json!({}))
        .await
        .expect("start the source");
    assert_eq!(
        events_of(&pool, source).await,
        vec![(ws, "PAUSED".to_string())],
        "control: the first pause wrote one event"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(wake(&pool, 8).await.expect("wake"), vec![source]);
    drain_with(&pool, &Recorder::default()).await;

    assert_eq!(
        (
            run_state(&pool, source, "first").await.as_deref(),
            run_state(&pool, source, "second").await.as_deref(),
        ),
        (Some("SUCCESS"), Some("PAUSED")),
        "control: the first pause ended and the second one is waiting: {}",
        execution_history(&pool, source).await
    );
    let history = execution_history(&pool, source).await;
    assert_eq!(
        history.as_array().and_then(|h| h.last()),
        Some(&serde_json::json!("PAUSED")),
        "control: the source is paused again: {history}"
    );
    assert_eq!(
        events_of(&pool, source).await,
        vec![(ws, "PAUSED".to_string()), (ws, "PAUSED".to_string())],
        "re-entering PAUSED is a new transition and a new event; history {history}"
    );
}

// ---------------------------------------------------------------------------
// W13a final review I1: `execution_depth` through Subflow, Loop and replay.
// ---------------------------------------------------------------------------

/// A flow whose `Subflow` (or `Loop`) comes AFTER a `Log`, so the test can set
/// the execution's depth between its first settle (which only opens `pre`) and
/// the settle that spawns.
fn after_a_log(id: &str, then: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: pre\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\n{then}"
    )
}

const SUBFLOW_TO_CHILD: &str = "  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    \
                                namespace: t\n    flowId: depth-child\n";

async fn depth_of(pool: &sqlx::PgPool, execution: Uuid) -> i32 {
    sqlx::query_scalar("SELECT execution_depth FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read execution_depth")
}

async fn set_depth(pool: &sqlx::PgPool, execution: Uuid, depth: i32) {
    sqlx::query("UPDATE workflow.executions SET execution_depth = $2 WHERE id = $1")
        .bind(execution)
        .bind(depth)
        .execute(pool)
        .await
        .expect("set execution_depth");
}

/// `(id, execution_depth)` of the `Subflow` children of `parent`.
async fn subflow_children(pool: &sqlx::PgPool, parent: Uuid) -> Vec<(Uuid, i32)> {
    sqlx::query_as(
        "SELECT id, execution_depth FROM workflow.executions \
          WHERE trigger_execution_id = $1 AND trigger_run_ix IS NOT NULL ORDER BY id",
    )
    .bind(parent)
    .fetch_all(pool)
    .await
    .expect("read the subflow children")
}

/// Kestra, `ExecutableUtils.subflowExecution` on the pinned artifact (javap,
/// offsets 703-792 and 1092-1100): the child's `metadata.executionDepth` is
/// `executionDepthOrZero() + 1`, and a child that would exceed `maxDepth`
/// (100) is not created -- the task fails with "Cannot execute flow '%s'.'%s':
/// the execution chain exceeded the maximum depth of %d". A parent at 7 gives
/// a child at 8; a parent at 99 still gives a child, at 100; a parent at 100
/// gives none and a FAILED `sub` run that says why.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_is_one_deeper_and_refused_past_depth_100(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let child = "id: depth-child\nnamespace: t\ntasks:\n  - id: a\n    \
                 type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    file_flow(&db, ws, user, child).await;
    let parent = after_a_log("depth-parent", SUBFLOW_TO_CHILD);
    file_flow(&db, ws, user, &parent).await;

    let mut roots = Vec::new();
    for depth in [7, 99, 100] {
        let root = start_execution(&pool, ws, None, &parent, serde_json::json!({}))
            .await
            .expect("start the parent");
        set_depth(&pool, root, depth).await;
        roots.push(root);
    }
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let depths: Vec<Vec<i32>> = {
        let mut out = Vec::new();
        for root in &roots {
            out.push(
                subflow_children(&pool, *root)
                    .await
                    .into_iter()
                    .map(|(_, d)| d)
                    .collect(),
            );
        }
        out
    };
    assert_eq!(
        depths,
        vec![vec![8], vec![100], vec![]],
        "parent 7 -> child 8; parent 99 -> child 100; parent 100 -> no child"
    );
    let message: Option<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'sub'",
    )
    .bind(roots[2])
    .fetch_one(&pool)
    .await
    .expect("the refused run");
    assert_eq!(
        (
            run_state(&pool, roots[2], "sub").await.as_deref(),
            message.as_deref()
        ),
        (
            Some("FAILED"),
            Some(
                "cannot execute flow 't'.'depth-child': the execution chain exceeded \
                 the maximum depth of 100"
            )
        )
    );
    assert_eq!(
        run_state(&pool, roots[1], "sub").await.as_deref(),
        Some("SUCCESS"),
        "control: the depth-100 child ran"
    );
}

/// Kestra, `Execution.loopExecution` on the pinned artifact (javap): an
/// iteration is built with the parent's own `metadata` -- so its
/// `executionDepth` is the parent's, not one more -- and `ExecutionKind.LOOP`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_keeps_its_parents_depth(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = after_a_log(
        "depth-loop",
        "  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"x\"]'\n    \
         tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        \
         message: hi\n",
    );
    let root = start_execution(&pool, ws, None, &source, serde_json::json!({}))
        .await
        .expect("start");
    set_depth(&pool, root, 7).await;
    settle_everything(
        &pool,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;

    let iterations: Vec<i32> = sqlx::query_scalar(
        "SELECT execution_depth FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(iterations, vec![7], "one iteration, at its parent's depth");
    assert_eq!(depth_of(&pool, root).await, 7, "control");
}

/// Kestra, `ExecutionService.replay`: the replacement keeps the metadata
/// (`metadata.nextAttempt()`, a `toBuilder` copy with the attempt bumped), so
/// its `executionDepth` is the original's.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_keeps_its_depth(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: depth-replay\nnamespace: t\nretry:\n  behavior: CREATE_NEW_EXECUTION\n  \
                  type: constant\n  interval: PT1S\n  maxAttempts: 2\ntasks:\n  - id: a\n    \
                  type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let root = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start");
    set_depth(&pool, root, 7).await;
    let reg = AlwaysFails::default();
    drain_with(&pool, &reg).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(wake(&pool, 8).await.expect("wake"), vec![root]);
    let replay = replay_of(&pool, root)
        .await
        .expect("the replacement exists");
    assert_eq!(depth_of(&pool, replay).await, 7, "the original's depth");
}

// ===========================================================================
// W11 Task 3: the attempt's log, written through `run_one`
// ===========================================================================

use egret_workflow::logs::{LogLevel, LogLimits, LogLine};
use egret_workflow::worker::handlers::LogSink;

/// W11 Task 1's measured fixture, read for its typed `egret_expected`.
fn w11_expected(id: &str) -> serde_json::Value {
    let w: serde_json::Value =
        serde_json::from_str(include_str!("../oracle-exec/w11-logs.json")).expect("parses");
    w["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap_or_else(|| panic!("no finding {id}"))["egret_expected"]
        .clone()
}

fn w11_level(v: &serde_json::Value) -> LogLevel {
    LogLevel::parse(v.as_str().expect("a level name")).expect("one of the five")
}

/// A recording [`LogSink`]; `handlers::testing::Recorder` is `pub(crate)`.
#[derive(Debug, Default)]
struct Lines(Mutex<Vec<LogLine>>);

#[async_trait::async_trait]
impl LogSink for Lines {
    async fn write(&self, line: LogLine) {
        self.0.lock().unwrap().push(line);
    }
}

impl Lines {
    /// `(level, message)` of every line about `execution`, in write order.
    fn of(&self, execution: Uuid) -> Vec<(LogLevel, String)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.execution_id == execution)
            .map(|l| (l.level, l.message.clone()))
            .collect()
    }
}

/// A one-task flow; the task id tells [`Scripted`] what to do.
fn scripted_flow(id: &str, task: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: {task}\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

/// A [`Registry`] whose handler writes `lines` INFO lines through `task.log`
/// and then answers by task id: `err` is a worker `Err`, `slow` sleeps past
/// any tight budget, `boom` fails with a message, `nomsg` fails without one,
/// anything else succeeds. Its log goes to `sink` under `limits`.
struct Scripted {
    sink: std::sync::Arc<Lines>,
    limits: LogLimits,
    lines: usize,
    started: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(max_lines: u64, lines: usize) -> Self {
        Self {
            sink: std::sync::Arc::new(Lines::default()),
            limits: LogLimits {
                run_max_lines: max_lines,
                ..LogLimits::default()
            },
            lines,
            started: Mutex::new(Vec::new()),
        }
    }

    /// The cap summary for `n` withheld lines of `bytes`.
    fn summary(&self, n: usize, bytes: usize) -> String {
        format!(
            "{n} lines ({bytes} bytes) of this attempt were not stored: over the per-run log \
             cap ({} lines / {} bytes).",
            self.limits.run_max_lines, self.limits.run_max_bytes
        )
    }
}

#[async_trait::async_trait]
impl Registry for Scripted {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.started.lock().unwrap().push(task.task_id.clone());
        for i in 1..=self.lines {
            task.log.info(&format!("{} line {i}", task.task_id)).await;
        }
        match task.task_id.as_str() {
            "err" => Err(egret_core::Error::Internal(anyhow::anyhow!(
                "an egret outage, not the task's"
            ))),
            "slow" => {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(Outcome::success())
            }
            "boom" => Ok(Outcome::failed_with("boom")),
            "nomsg" => Ok(Outcome::failed()),
            _ => Ok(Outcome::success()),
        }
    }
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
    fn log_sink(&self) -> (std::sync::Arc<dyn LogSink>, LogLimits) {
        (self.sink.clone(), self.limits)
    }
}

/// **`finish()` runs on every path out of the handler**: success (the
/// control), a worker `Err`, and a budget that expired (the kill path). In
/// all three the handler logged past a 1-line cap, so the summary must be
/// there.
#[sqlx::test(migrations = "../../migrations")]
async fn run_one_finishes_the_log_on_error_timeout_and_cancel(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let reg = Scripted::new(1, 3);
    let start = |src: String| {
        let pool = pool.clone();
        async move {
            start_execution(&pool, ws, None, &src, serde_json::json!({}))
                .await
                .expect("start")
        }
    };
    let ok = start(scripted_flow("fin-ok", "ok")).await;
    let err = start(scripted_flow("fin-err", "err")).await;
    let slow = start(one_task_with_timeout("fin-slow", "slow", "PT0.001S")).await;

    let r = egret_workflow::worker::tick(&pool, 8, &reg).await;
    assert!(r.is_err(), "the `err` handler's Err reaches the tick");
    let mut started = reg.started.lock().unwrap().clone();
    started.sort();
    assert_eq!(started, ["err", "ok", "slow"]);

    let two = |t: &str| format!("{t} line 2").len() + format!("{t} line 3").len();
    assert_eq!(
        reg.sink.of(ok),
        vec![
            (LogLevel::Info, "ok line 1".to_string()),
            (LogLevel::Warn, reg.summary(2, two("ok"))),
        ]
    );
    assert_eq!(
        reg.sink.of(err),
        vec![
            (LogLevel::Info, "err line 1".to_string()),
            (LogLevel::Warn, reg.summary(2, two("err"))),
        ],
        "the Err path finishes the log too"
    );
    assert_eq!(
        reg.sink.of(slow),
        vec![
            (LogLevel::Info, "slow line 1".to_string()),
            (LogLevel::Error, "Timeout after 00:00:00.001".to_string()),
            (LogLevel::Warn, reg.summary(2, two("slow"))),
        ],
        "an expired budget finishes the log too"
    );
}

/// **The failed-attempt line**: once, at ERROR, after the handler's lines and
/// before the summary, and outside the cap — Task 1's `failed_attempt_line`.
/// A `Failed` outcome with no message writes none.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_attempt_logs_its_reason_once_even_over_the_cap(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let e = w11_expected("script_runner_diagnostics")["failed_attempt_line"].clone();
    assert_eq!(w11_level(&e["level"]), LogLevel::Error);
    assert_eq!(e["count"], 1);
    assert_eq!(e["bypasses_run_cap"], true);
    assert_eq!(
        e["order"],
        serde_json::json!(["handler_lines", "failed_attempt_line", "cap_summary"])
    );
    let path = |p: &str| {
        e["paths"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["path"] == p)
            .unwrap_or_else(|| panic!("no path {p}"))["written"]
            .as_bool()
            .unwrap()
    };
    assert!(path("handler_ok_failed"));
    assert!(!path("failed_without_message"));

    let (_db, ws) = seed(&pool).await;
    let reg = Scripted::new(1, 3);
    let boom = start_execution(
        &pool,
        ws,
        None,
        &scripted_flow("fa-boom", "boom"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    let nomsg = start_execution(
        &pool,
        ws,
        None,
        &scripted_flow("fa-nomsg", "nomsg"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    assert_eq!(drain_with(&pool, &reg).await, 2);
    assert_eq!(
        run_message(&pool, boom, "boom").await.as_deref(),
        Some("boom")
    );

    let got = reg.sink.of(boom);
    assert_eq!(
        got,
        vec![
            (LogLevel::Info, "boom line 1".to_string()),
            (w11_level(&e["level"]), "boom".to_string()),
            (
                LogLevel::Warn,
                reg.summary(2, "boom line 2".len() + "boom line 3".len())
            ),
        ]
    );
    assert_eq!(got.iter().filter(|l| l.0 == LogLevel::Error).count(), 1);
    assert!(
        reg.sink.of(nomsg).iter().all(|l| l.0 != LogLevel::Error),
        "a Failed outcome with no message writes no failure line: {:?}",
        reg.sink.of(nomsg)
    );
}

/// **A refused budget logs its reason**: one ERROR line, the refusal, and the
/// handler never ran.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refused_budget_logs_its_reason(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let e = w11_expected("script_runner_diagnostics")["failed_attempt_line"].clone();
    assert!(e["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["path"] == "budget_refused" && x["written"] == true));
    let (_db, ws) = seed(&pool).await;
    let reg = Scripted::new(1, 3);
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_timeout("refused", "refused", "abc"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    assert_eq!(drain_with(&pool, &reg).await, 1);
    assert!(
        reg.started.lock().unwrap().is_empty(),
        "the handler never ran"
    );
    let message = run_message(&pool, ex, "refused").await.expect("a message");
    assert!(message.contains("abc"), "{message}");
    assert_eq!(reg.sink.of(ex), vec![(LogLevel::Error, message)]);
}

/// **An expired budget logs `Timeout after …`**: one ERROR line, Kestra's own
/// text.
#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_budget_logs_timeout_after(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let e = w11_expected("script_runner_diagnostics")["failed_attempt_line"].clone();
    assert!(e["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["path"] == "timeout_expired"
            && x["written"] == true
            && x["text"] == "Timeout after {HH}:{mm}:{ss}.{SSS}"));
    let (_db, ws) = seed(&pool).await;
    let reg = Scripted::new(10, 0);
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_timeout("expired", "slow", "PT0.001S"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    assert_eq!(drain_with(&pool, &reg).await, 1);
    assert_eq!(
        reg.sink.of(ex),
        vec![(LogLevel::Error, "Timeout after 00:00:00.001".to_string())]
    );
}

/// **A handler `Err` writes no failure line** — it is egret's outage and this
/// tick records no outcome — but the summary is still written.
#[sqlx::test(migrations = "../../migrations")]
async fn a_handler_err_logs_no_failure_line(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let e = w11_expected("script_runner_diagnostics")["failed_attempt_line"].clone();
    assert!(e["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["path"] == "handler_err" && x["written"] == false));
    let (_db, ws) = seed(&pool).await;
    let reg = Scripted::new(1, 3);
    let ex = start_execution(
        &pool,
        ws,
        None,
        &scripted_flow("herr", "err"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    assert!(egret_workflow::worker::tick(&pool, 8, &reg).await.is_err());
    let got = reg.sink.of(ex);
    assert_eq!(
        got,
        vec![
            (LogLevel::Info, "err line 1".to_string()),
            (
                LogLevel::Warn,
                reg.summary(2, "err line 2".len() + "err line 3".len())
            ),
        ]
    );
    assert!(got.iter().all(|l| l.0 != LogLevel::Error));
}

/// **`Fail` stores Kestra's lines**: the unconditional path one ERROR line
/// (written by `run_one`), the conditional path two (the handler's, then
/// `run_one`'s), a falsy condition none — Task 1's `fail_task` table, typed,
/// through the shipping registry. A successful `Log` stores no ERROR line.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unconditional_fail_logs_one_line_and_a_conditional_fail_two(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let sink = std::sync::Arc::new(Lines::default());
    let reg = Handlers::with_core_tasks(egret_workflow::storage::current())
        .with_log_sink(sink.clone(), LogLimits::default());

    let cases = w11_expected("script_runner_diagnostics")["failed_attempt_line"]["fail_task"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(cases.len(), 3);
    let mut runs = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let mut task = format!(
            "  - id: f\n    type: io.kestra.plugin.core.execution.Fail\n    errorMessage: {}\n",
            case["error_message"]
        );
        match &case["condition"] {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(b) => task.push_str(&format!("    condition: \"{b}\"\n")),
            other => panic!("condition {other}"),
        }
        let src = format!("id: fail-{i}\nnamespace: t\ntasks:\n{task}");
        let ex = start_execution(&pool, ws, None, &src, serde_json::json!({}))
            .await
            .expect("start");
        runs.push((ex, case.clone()));
    }
    let ok = start_execution(&pool, ws, None, &one_task("fail-ok"), serde_json::json!({}))
        .await
        .expect("start");
    drain(&pool, &reg).await;

    for (ex, case) in &runs {
        let want: Vec<(LogLevel, String)> = case["stored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (w11_level(&p[0]), p[1].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(sink.of(*ex), want, "{case}");
    }
    assert_eq!(sink.of(ok), vec![(LogLevel::Info, "hi".to_string())]);
}

// ===========================================================================
// W5a Task 3: the worker renews its lease while a handler runs
// ===========================================================================

use egret_workflow::worker::{tick_with, WorkerTiming};

/// A [`WorkerTiming`] with a short lease and the product's script default.
fn lease_of_ms(working_timeout_ms: u64) -> WorkerTiming {
    WorkerTiming {
        working_timeout: std::time::Duration::from_millis(working_timeout_ms),
        ..WorkerTiming::default()
    }
}

/// A [`Registry`] whose handler signals that it has started and then sleeps
/// for `sleep`; `completed` counts the handlers that were allowed to finish.
struct Sleeper {
    sleep: std::time::Duration,
    started: std::sync::Arc<tokio::sync::Notify>,
    calls: std::sync::atomic::AtomicUsize,
    completed: std::sync::atomic::AtomicUsize,
}

impl Sleeper {
    fn new(sleep: std::time::Duration) -> Self {
        Self {
            sleep,
            started: std::sync::Arc::new(tokio::sync::Notify::new()),
            calls: Default::default(),
            completed: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl Registry for Sleeper {
    async fn run(&self, _task: &TaskInput) -> egret_core::Result<Outcome> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.notify_one();
        tokio::time::sleep(self.sleep).await;
        self.completed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Outcome::success())
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!("nothing here declares a `timeout:`: {}", task.task_id);
    }
}

/// **A run whose worker stopped renewing is reaped — and not before.**
///
/// The worker dies mid-handler: its tick future is aborted, which is what a
/// killed process looks like to the database (nothing more is written). Two
/// reaper passes bracket the death, and they must DISAGREE: while the worker
/// lived its lease was younger than `working_timeout` however long the handler
/// had run (zero reclaimed, after more than one `working_timeout` of
/// handler), and one `working_timeout` after it died the same pass reclaims
/// it (one). A reaper that reclaimed nothing ever, and a worker that never
/// renewed, are each caught by one of the two numbers.
#[sqlx::test(migrations = "../../migrations")]
async fn a_run_whose_worker_stopped_renewing_is_reaped(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &one_task("dies"), serde_json::json!({}))
        .await
        .expect("start execution");
    let timeout = std::time::Duration::from_millis(900);
    let reg = std::sync::Arc::new(Sleeper::new(std::time::Duration::from_secs(60)));

    let worker = {
        let (pool, reg) = (pool.clone(), reg.clone());
        tokio::spawn(async move { tick_with(&pool, 1, &*reg, lease_of_ms(900)).await })
    };
    reg.started.notified().await;
    let claimed_at = lease_of(&pool, ex, "a").await.expect("leased");

    // Longer than one working_timeout inside the handler.
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let renewed = lease_of(&pool, ex, "a").await.expect("still leased");
    assert!(
        renewed > claimed_at,
        "the live worker renewed its lease: claimed {claimed_at}, now {renewed}"
    );
    assert_eq!(
        reap(&pool, timeout).await.expect("reap"),
        0,
        "a run whose worker is alive is never reclaimed, however long its handler"
    );

    // The worker dies.
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    let last = lease_of(&pool, ex, "a").await.expect("still leased");

    tokio::time::sleep(timeout + std::time::Duration::from_millis(300)).await;
    assert_eq!(
        lease_of(&pool, ex, "a").await,
        Some(last),
        "a dead worker renews nothing"
    );
    assert_eq!(
        reap(&pool, timeout).await.expect("reap"),
        1,
        "one working_timeout after its worker died, the run is reclaimed"
    );
    assert_eq!(lease_of(&pool, ex, "a").await, None);
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("RUNNING"));
    assert_eq!(reg.completed.load(std::sync::atomic::Ordering::SeqCst), 0);

    // And the reclaimed run is claimable and finishes once, under a new worker.
    let next = Sleeper::new(std::time::Duration::ZERO);
    assert_eq!(
        tick_with(&pool, 1, &next, lease_of_ms(900))
            .await
            .expect("tick"),
        1
    );
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("SUCCESS"));
    assert_eq!(attempts_of(&pool, ex, "a").await, 1);
}

/// A [`Registry`] whose handler writes three lines, then lets a rival take its
/// run exactly as production would (the reaper clears the lease, another
/// worker claims it), then sleeps `sleep` and answers FAILED with a message.
///
/// **The rival runs on its own task** (W9a Task 6 fix round 2). The reap is
/// what moves the fence, so the stale worker's next renewal (every 100 ms
/// under `lease_of_ms(300)`) finds it moved and DROPS this handler -- which
/// is the behaviour under test. Awaited inline, the reap and the rival's
/// claim were part of the handler future and died with it whenever the reap
/// committed less than one renewal before the claim returned; a reap that
/// took ~90 ms (measured: 6-90 ms on the dev Postgres) left no rival at all
/// and the test failed on its own harness ("the rival claimed"). A rival
/// worker is never inside another worker's handler in production; spawned,
/// it outlives the drop as a real one would.
struct FenceMover {
    pool: sqlx::PgPool,
    sink: std::sync::Arc<Lines>,
    sleep: std::time::Duration,
    rival: std::sync::Arc<Mutex<Option<egret_workflow::worker::Claimed>>>,
    completed: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl Registry for FenceMover {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        for i in 1..=3 {
            task.log.info(&format!("line {i}")).await;
        }
        let (pool, slot) = (self.pool.clone(), self.rival.clone());
        let rival = tokio::spawn(async move {
            assert_eq!(
                reap(&pool, std::time::Duration::ZERO).await.expect("reap"),
                1
            );
            let rival = claim(&pool, 1).await.expect("rival claim");
            assert_eq!(rival.len(), 1, "the reaped run is claimable again");
            *slot.lock().unwrap() = rival.into_iter().next();
        });
        // Dropped here (or in the sleep) when the renewal finds the fence
        // moved; the rival's task runs on regardless.
        rival.await.expect("the rival's reap and claim");
        tokio::time::sleep(self.sleep).await;
        self.completed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Outcome::failed_with("the stale worker's outcome"))
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!("nothing here declares a `timeout:`: {}", task.task_id);
    }
    fn log_sink(&self) -> (std::sync::Arc<dyn LogSink>, egret_workflow::logs::LogLimits) {
        (
            self.sink.clone(),
            LogLimits {
                run_max_lines: 1,
                ..LogLimits::default()
            },
        )
    }
}

/// **A worker that finds its fence moved records no outcome, and stops.**
///
/// The rival holds the run now. The stale worker's renewal finds
/// `claimed_at` moved, drops its handler (which never finishes), writes no
/// failed-attempt line and applies no outcome — the tick returns `Ok(0)`,
/// not the `LeaseLost` error a worker that ran its handler to the end and
/// then hit `apply_outcome`'s fence returns. Its already-written lines stay,
/// and `finish()` still runs: the cap is one line, so the cap summary (WARN,
/// two withheld) is the evidence that the lost path finished the log.
#[sqlx::test(migrations = "../../migrations")]
async fn a_worker_that_loses_its_fence_records_no_outcome(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &one_task("fence"), serde_json::json!({}))
        .await
        .expect("start execution");
    let reg = FenceMover {
        pool: pool.clone(),
        sink: std::sync::Arc::new(Lines::default()),
        sleep: std::time::Duration::from_secs(3),
        rival: std::sync::Arc::new(Mutex::new(None)),
        completed: std::sync::atomic::AtomicBool::new(false),
    };

    let started = std::time::Instant::now();
    let committed = tick_with(&pool, 1, &reg, lease_of_ms(300))
        .await
        .expect("a lost fence is not an error: the new holder owns the run");
    assert_eq!(committed, 0, "no result committed");
    assert!(
        !reg.completed.load(std::sync::atomic::Ordering::SeqCst),
        "the handler was dropped, not run to the end"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the worker stopped following the task at its next renewal: {:?}",
        started.elapsed()
    );

    // The rival's task may still be finishing its claim when the tick
    // returns: wait for it (bounded), never assume it.
    let rival = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(r) = reg.rival.lock().unwrap().clone() {
                break r;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the rival claimed within 10 s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };
    assert_eq!(
        lease_of(&pool, ex, "a").await,
        Some(rival.claimed_at),
        "the rival's lease is untouched"
    );
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("RUNNING"));
    assert_eq!(run_message(&pool, ex, "a").await, None);
    assert_eq!(run_outputs(&pool, ex, "a").await, None);
    assert_eq!(attempts_of(&pool, ex, "a").await, 1);
    assert_eq!(
        reg.sink.of(ex),
        vec![
            (LogLevel::Info, "line 1".to_string()),
            (
                LogLevel::Warn,
                "2 lines (12 bytes) of this attempt were not stored: over the per-run log \
                 cap (1 lines / 10485760 bytes)."
                    .to_string()
            ),
        ],
        "the written line stays, finish() ran, and no failed-attempt line"
    );

    // The rival, holding the live lease, can still commit.
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, &rival, &Outcome::success())
        .await
        .expect("the rival commits");
    tx.commit().await.unwrap();
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("SUCCESS"));
}

/// A [`Registry`] that records each run's `(task_id, timeout)` and sleeps
/// `slow` for the task named `slow`.
#[derive(Default)]
struct Budgets {
    seen: Mutex<Vec<(String, Option<std::time::Duration>)>>,
    killed: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Registry for Budgets {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.seen
            .lock()
            .unwrap()
            .push((task.task_id.clone(), task.timeout));
        if task.task_id == "slow" {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        Ok(Outcome::success())
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        self.killed.lock().unwrap().push(task.task_id.clone());
        Ok(())
    }
}

fn one_typed_task(id: &str, task: &str, type_id: &str, extra: &str) -> String {
    let body = if type_id.starts_with("io.kestra.plugin.core") {
        "    message: hi\n".to_string()
    } else if type_id.starts_with("kestra.") {
        "    url: jdbc:postgresql://pg/db\n    sql: SELECT 1\n".to_string()
    } else {
        "    commands: [echo hi]\n".to_string()
    };
    format!("id: {id}\nnamespace: t\ntasks:\n  - id: {task}\n    type: {type_id}\n{body}{extra}")
}

/// **A script or container task with no `timeout:` runs under the configured
/// default; an explicit `timeout:` wins; every other type stays unbounded.**
///
/// The default is set to 777 s, a value no other code path produces, so the
/// handler seeing it is the configured value arriving and not a constant. The
/// control rows are a `core.log.Log` with no `timeout:` (unbounded, as in
/// Kestra) and a script task with `timeout: PT5S` (the author's). The last
/// part enforces it: with a 1 s default, a script task whose handler takes 3 s
/// is FAILED with the artifact's own timeout message.
#[sqlx::test(migrations = "../../migrations")]
async fn a_script_task_without_timeout_gets_the_default(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let default = std::time::Duration::from_secs(777);
    let cases = [
        (
            "shell",
            "io.kestra.plugin.scripts.shell.Commands",
            "",
            Some(default),
        ),
        (
            "python",
            "io.kestra.plugin.scripts.python.Commands",
            "",
            Some(default),
        ),
        ("alias", "egret.container", "", Some(default)),
        (
            "plugin",
            "kestra.io.kestra.plugin.jdbc.postgresql.Query",
            "",
            Some(default),
        ),
        (
            "explicit",
            "io.kestra.plugin.scripts.shell.Commands",
            "    timeout: PT5S\n",
            Some(std::time::Duration::from_secs(5)),
        ),
        ("log", "io.kestra.plugin.core.log.Log", "", None),
    ];
    for (task, type_id, extra, _) in &cases {
        start_execution(
            &pool,
            ws,
            None,
            &one_typed_task(task, task, type_id, extra),
            serde_json::json!({}),
        )
        .await
        .unwrap_or_else(|e| panic!("start {task}: {e}"));
    }
    let reg = Budgets::default();
    let timing = WorkerTiming {
        container_default_timeout: default,
        ..WorkerTiming::default()
    };
    assert_eq!(
        tick_with(&pool, 8, &reg, timing).await.expect("tick"),
        cases.len()
    );
    let mut seen = reg.seen.lock().unwrap().clone();
    seen.sort();
    let mut want: Vec<_> = cases
        .iter()
        .map(|(task, _, _, budget)| (task.to_string(), *budget))
        .collect();
    want.sort();
    assert_eq!(seen, want);

    // Enforced, not only handed over.
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_typed_task(
            "slow",
            "slow",
            "io.kestra.plugin.scripts.shell.Commands",
            "",
        ),
        serde_json::json!({}),
    )
    .await
    .expect("start slow");
    let timing = WorkerTiming {
        container_default_timeout: std::time::Duration::from_secs(1),
        ..WorkerTiming::default()
    };
    tick_with(&pool, 1, &reg, timing).await.expect("tick");
    assert_eq!(
        run_state(&pool, ex, "slow").await.as_deref(),
        Some("FAILED")
    );
    assert_eq!(
        run_message(&pool, ex, "slow").await.as_deref(),
        Some("Timeout after 00:00:01.000")
    );
    assert_eq!(reg.killed.lock().unwrap().clone(), vec!["slow".to_string()]);
}

// ---------------------------------------------------------------------------
// W9a Task 6: a run's files are indexed with its outcome (spec §4.3)
// ---------------------------------------------------------------------------

/// The URI a task run's file `name` gets: its own execution and task run, as
/// `uri::task_prefix` builds them for the claim.
fn file_of(execution: Uuid, task_id: &str, run: Uuid, flow: &str, name: &str) -> String {
    format!(
        "{}{name}",
        egret_workflow::storage::uri::task_prefix("t", flow, execution, task_id, run).unwrap()
    )
}

/// The index rows of `execution`: `(uri, task_run_id, size)`.
async fn indexed(pool: &sqlx::PgPool, execution: Uuid) -> Vec<(String, Option<Uuid>, i64)> {
    sqlx::query_as(
        "SELECT uri, task_run_id, size FROM workflow.execution_files \
          WHERE execution_id = $1 ORDER BY uri",
    )
    .bind(execution)
    .fetch_all(pool)
    .await
    .expect("read the index")
}

/// A handler that "wrote" one file per run, `out.txt`, of `attempt * 10`
/// bytes -- and, while it runs (before any commit), saw no row for it.
#[derive(Default)]
struct WritesFiles {
    pool: Mutex<Option<sqlx::PgPool>>,
    flow: String,
    /// `(run id, attempt, rows seen for its URI while the handler ran)`.
    seen: Mutex<Vec<(Uuid, usize, i64)>>,
    fail_first: bool,
}

#[async_trait::async_trait]
impl Registry for WritesFiles {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let uri = file_of(
            task.execution_id,
            &task.task_id,
            task.run_id,
            &self.flow,
            "out.txt",
        );
        let pool = self.pool.lock().unwrap().clone().expect("a pool");
        let before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE uri = $1")
                .bind(&uri)
                .fetch_one(&pool)
                .await
                .expect("count");
        self.seen
            .lock()
            .unwrap()
            .push((task.run_id, task.attempt, before));
        let files = vec![egret_workflow::storage::StoredFile {
            uri: uri.clone(),
            size: task.attempt as u64 * 10,
        }];
        let outputs = Some(serde_json::json!({ "outputFiles": { "out.txt": uri } }));
        if self.fail_first && task.attempt == 1 {
            return Ok(Outcome {
                state: egret_workflow::executor::RunState::Failed,
                outputs,
                message: Some("the first attempt fails, having written its file".into()),
                files,
            });
        }
        Ok(Outcome {
            state: egret_workflow::executor::RunState::Success,
            outputs,
            message: None,
            files,
        })
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!("nothing here is killed: {}", task.task_id);
    }
}

/// **Files are indexed in the outcome's transaction, and not before** (spec
/// §4.3): while the handler runs there is no row; while the outcome's
/// transaction is open no other connection sees one; after the commit the
/// row is there, carrying the task run.
#[sqlx::test(migrations = "../../migrations")]
async fn files_are_indexed_in_the_outcomes_transaction_and_not_before(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;

    // Through the worker: a registry whose handler returns a file.
    let ex = start_execution(&pool, ws, None, &one_task("indexed"), serde_json::json!({}))
        .await
        .expect("start");
    let reg = WritesFiles {
        pool: Mutex::new(Some(pool.clone())),
        flow: "indexed".into(),
        ..WritesFiles::default()
    };
    assert_eq!(drain_with(&pool, &reg).await, 1);
    let seen = reg.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let (run, _, before) = seen[0];
    assert_eq!(before, 0, "no row while the handler ran");
    assert_eq!(
        indexed(&pool, ex).await,
        vec![(file_of(ex, "a", run, "indexed", "out.txt"), Some(run), 10)],
        "after the commit: one row, the task run's"
    );

    // And the transaction itself: apply_outcome writes the row, nobody
    // outside sees it until the commit.
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task("indexed2"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    let claimed = claim(&pool, 1).await.expect("claim");
    assert_eq!(claimed.len(), 1);
    let c = &claimed[0];
    let uri = file_of(ex, &c.task_id, c.id, "indexed2", "a.csv");
    let outcome = Outcome {
        files: vec![egret_workflow::storage::StoredFile {
            uri: uri.clone(),
            size: 7,
        }],
        ..Outcome::success()
    };
    let mut tx = pool.begin().await.unwrap();
    apply_outcome(&mut tx, c, &outcome)
        .await
        .expect("the live lease commits");
    let inside: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE uri = $1")
            .bind(&uri)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(inside, 1, "the transaction holds the row");
    assert!(
        indexed(&pool, ex).await.is_empty(),
        "no other connection sees it before the commit"
    );
    tx.commit().await.unwrap();
    assert_eq!(indexed(&pool, ex).await, vec![(uri, Some(c.id), 7)]);
}

/// **A fenced outcome indexes nothing**: a worker whose lease moved returns
/// `LeaseLost` before any row is written, so its uploads stay orphans.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fenced_outcome_indexes_nothing(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task("fenced-files"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    let claimed = claim(&pool, 1).await.expect("claim");
    let c = &claimed[0];
    // Another holder renewed the lease: the fencing token moved.
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = claimed_at + interval '1 second' \
          WHERE id = $1",
    )
    .bind(c.id)
    .execute(&pool)
    .await
    .unwrap();
    let outcome = Outcome {
        files: vec![egret_workflow::storage::StoredFile {
            uri: file_of(ex, &c.task_id, c.id, "fenced-files", "a.csv"),
            size: 7,
        }],
        ..Outcome::success()
    };
    let mut tx = pool.begin().await.unwrap();
    let late = apply_outcome(&mut tx, c, &outcome).await;
    // Not even inside the stale worker's own transaction: the fence is checked
    // BEFORE indexing (spec §4.3), so a lost lease writes no row that only
    // the rollback would remove (and takes no lock on the live holder's URIs,
    // which are the same deterministic names). Counted in `tx`, before the
    // rollback below can hide an index-then-fence order.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.execution_files WHERE execution_id = $1"
        )
        .bind(ex)
        .fetch_one(&mut *tx)
        .await
        .unwrap(),
        0,
        "the fenced outcome wrote no index row, even uncommitted"
    );
    // Committed if `apply_outcome` let it through, as `tick` would.
    match &late {
        Ok(()) => tx.commit().await.expect("commit"),
        Err(_) => drop(tx),
    }
    let err = late.expect_err("the lease moved");
    assert!(err.to_string().contains("lease"), "{err}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.execution_files WHERE execution_id = $1"
        )
        .bind(ex)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "nothing indexed for the run"
    );
}

/// **A retry attempt overwrites its own URIs** without a unique violation:
/// the task run keeps its id across attempts, so its names are the same
/// (spec §4.3), and the row ends holding the second attempt's size.
#[sqlx::test(migrations = "../../migrations")]
async fn a_retry_attempt_overwrites_its_own_uris_without_a_unique_violation(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_retry("retried-files", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start");
    let reg = WritesFiles {
        pool: Mutex::new(Some(pool.clone())),
        flow: "retried-files".into(),
        fail_first: true,
        ..WritesFiles::default()
    };
    assert_eq!(drain_with(&pool, &reg).await, 1, "the first attempt");
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("RETRYING"));
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(wake(&pool, 8).await.expect("wake"), vec![ex]);
    assert_eq!(drain_with(&pool, &reg).await, 1, "the second attempt");
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("SUCCESS"));
    let seen = reg.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, seen[1].0, "one task run, two attempts");
    assert_eq!((seen[0].1, seen[1].1), (1, 2));
    assert_eq!(
        seen[1].2, 1,
        "the first attempt's row was there when the second ran"
    );
    assert_eq!(
        indexed(&pool, ex).await,
        vec![(
            file_of(ex, "a", seen[0].0, "retried-files", "out.txt"),
            Some(seen[0].0),
            20
        )],
        "one row, overwritten by the second attempt"
    );
}

/// **The 2026-10-11 ruling, on a subflow call.** A `Subflow` whose target is a
/// flow with a `Pause` declaring `behavior:` (one of the four properties
/// egret does not run) creates no child: the target does not plan, so the
/// `Subflow` run FAILS with `cannot execute an invalid flow` naming the
/// property, and the parent ends `FAILED` instead of waiting on a child that
/// would stall in `RUNNING` for ever. The control is the same call to a plain
/// bounded `Pause`, which does create its child.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_to_a_pause_egret_does_not_run_creates_no_child_and_fails(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let pause = |id: &str, extra: &str| {
        format!(
            "id: {id}\nnamespace: t\ntasks:\n  - id: gate\n    \
             type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1H\n{extra}"
        )
    };
    file_flow(&db, ws, user, &pause("stalls", "    behavior: RESUME\n")).await;
    file_flow(&db, ws, user, &pause("plain", "")).await;
    let reg = Handlers::with_core_tasks(egret_workflow::storage::current());

    let parent = start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("caller", "stalls", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(&pool, &reg).await;

    let children: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_key = 'stalls'")
            .fetch_one(&pool)
            .await
            .expect("count children");
    assert_eq!(children, 0, "no child of a flow that does not plan");
    let (state, message): (String, Option<String>) = sqlx::query_as(
        "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'sub'",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .expect("read the sub run");
    assert_eq!(state, "FAILED");
    let message = message.unwrap_or_default();
    assert!(
        message.contains("cannot execute an invalid flow 't'.'stalls'")
            && message.contains("unsupported_pause_property")
            && message.contains("tasks.gate.behavior"),
        "{message}"
    );
    let parent_state: String =
        sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
            .bind(parent)
            .fetch_one(&pool)
            .await
            .expect("read parent");
    assert_eq!(parent_state, "FAILED");

    // The control: the same call to a plain Pause creates its child.
    start_execution(
        &pool,
        ws,
        None,
        &subflow_parent("caller2", "plain", ""),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");
    settle_everything(&pool, &reg).await;
    let children: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_key = 'plain'")
            .fetch_one(&pool)
            .await
            .expect("count children");
    assert_eq!(children, 1, "a plain Pause target is started");
}
