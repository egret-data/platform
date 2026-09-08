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
    apply_outcome, claim, reap, start_execution, wake, Outcome, Registry, TaskInput,
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

    let finished = drain(&pool, &Handlers::with_core_tasks()).await;
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

    assert_eq!(drain(&pool, &Handlers::with_core_tasks()).await, 1);
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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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

    let err = egret_workflow::worker::tick(&pool, 8, &Handlers::with_core_tasks())
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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;

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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;

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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;

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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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
    settle_everything(&pool, &Handlers::with_core_tasks()).await;

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

    settle_everything(&pool, &Handlers::with_core_tasks()).await;
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
