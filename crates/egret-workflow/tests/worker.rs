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
    let ex = start_execution(&pool, ws, None, source)
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
    let ex = start_execution(&pool, ws, None, source)
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

/// **The hole this task leaves, pinned so it cannot close silently.**
///
/// `claim::render_context_for` supplies an EMPTY render context — W3b Task 5
/// owns the render surface (spec §4.4) — so a property that references any
/// variable cannot render and the run goes `FAILED` where Kestra's would
/// succeed. That is a divergence in the safe direction (a refusal, never an
/// over-acceptance), and the day Task 5 fills the context this test fails and
/// says so.
#[sqlx::test(migrations = "../../migrations")]
async fn a_task_whose_property_needs_a_render_fails_rather_than_succeeding(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let (_db, ws) = seed(&pool).await;
    let source = "id: needs-render\nnamespace: t\ntasks:\n\
         \x20 - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: \"{{ inputs.who }}\"\n";
    let ex = start_execution(&pool, ws, None, source)
        .await
        .expect("start execution");

    assert_eq!(drain(&pool, &Handlers::with_core_tasks()).await, 1);
    assert_eq!(
        run_state(&pool, ex, "l").await.as_deref(),
        Some("FAILED"),
        "with no render context, `{{{{ inputs.who }}}}` cannot render — W3b \
         Task 5 is what makes this SUCCESS, and when it does, delete this test"
    );

    // The control, on the SAME handler and the same tick path: a literal
    // message needs no context and succeeds. Without it, this test would pass
    // against a `Log` handler that failed on everything.
    let lit = start_execution(&pool, ws, None, &one_task("literal"))
        .await
        .expect("start execution");
    assert_eq!(drain(&pool, &Handlers::with_core_tasks()).await, 1);
    assert_eq!(run_state(&pool, lit, "a").await.as_deref(), Some("SUCCESS"));
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
    let ex = start_execution(&pool, ws, None, source)
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

use egret_workflow::executor::{build_plan, TaskKind};
use egret_workflow::worker::resume;
use egret_workflow::{render_str, RenderContext};

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

/// **The caller tier W3b Task 5 owns, with the empty context it has today.**
///
/// `advance()` is pure and cannot render, and `worker` does not render either,
/// so a `Loop` waits at `CREATED` until something writes `TaskRun::rendered`
/// and calls [`resume`]. This is that something, and it is not a fake: it
/// calls the real `render_str` with the real (empty) `RenderContext`, which is
/// the identity on a literal `values:` and an error on anything else. When
/// Task 5 fills the context, this function keeps working unchanged.
///
/// Returns how many loop runs it rendered, so a caller can drive to a fixed
/// point rather than guessing a number of rounds.
async fn render_pending_loops(pool: &sqlx::PgPool) -> usize {
    let pending: Vec<(Uuid, String, i32, i32)> = sqlx::query_as(
        "SELECT e.id, e.source, r.run_ix, r.task_ix \
           FROM workflow.task_runs r JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE r.rendered IS NULL AND r.state = 'CREATED' AND r.type_id IS NULL",
    )
    .fetch_all(pool)
    .await
    .expect("read unrendered flowable runs");

    let mut touched: Vec<Uuid> = Vec::new();
    for (execution, source, run_ix, task_ix) in pending {
        let plan = build_plan(&source).expect("the stored source plans");
        let TaskKind::Loop { values, .. } = &plan.tasks[task_ix as usize].kind else {
            continue;
        };
        let rendered = render_str(values, &RenderContext::new()).expect("a literal `values:`");
        sqlx::query(
            "UPDATE workflow.task_runs SET rendered = $1 WHERE execution_id = $2 AND run_ix = $3",
        )
        .bind(&rendered)
        .bind(execution)
        .bind(run_ix)
        .execute(pool)
        .await
        .expect("write the rendered values");
        if !touched.contains(&execution) {
            touched.push(execution);
        }
    }
    for id in &touched {
        resume(pool, *id).await.expect("resume");
    }
    touched.len()
}

/// Render every waiting loop and run every claimable task until nothing moves.
async fn settle_everything(pool: &sqlx::PgPool, reg: &Handlers) {
    for _ in 0..64 {
        let rendered = render_pending_loops(pool).await;
        let ran = egret_workflow::worker::tick(pool, 16, reg)
            .await
            .expect("tick");
        if rendered == 0 && ran == 0 {
            return;
        }
    }
    panic!("the loop never settled in 64 rounds");
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
    let ex = start_execution(&pool, ws, None, &source)
        .await
        .expect("start execution");

    assert_eq!(
        sub_executions(&pool, ex, 0).await,
        Vec::new(),
        "nothing is spawned until the caller has rendered `values:`"
    );
    assert_eq!(render_pending_loops(&pool).await, 1, "one loop to render");

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
         only thing that orders these, which is why migration 0135 makes two \
         iterations sharing one index unrepresentable."
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
    let ex = start_execution(&pool, ws, None, &source)
        .await
        .expect("start execution");

    render_pending_loops(&pool).await;
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
    let ex = start_execution(&pool, ws, None, &source)
        .await
        .expect("start execution");

    render_pending_loops(&pool).await;
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
    let ex = start_execution(&pool, ws, None, &source)
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
    let ex = start_execution(&pool, ws, None, &source)
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
    let ex = start_execution(&pool, ws, None, &source)
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
    let ex = start_execution(&pool, ws, None, &source)
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
    let ex = start_execution(&pool, ws, None, &source)
        .await
        .expect("start execution");

    render_pending_loops(&pool).await;
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
