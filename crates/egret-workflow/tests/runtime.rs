//! The loops' own tests: the gate, cancellation, the ticker's cadence, and a
//! **per-statement** concurrency property. Three of the four run two tickers
//! against one database; statement 2 runs two concurrent direct calls, because
//! the value it asserts is a selection's **return** and a ticker swallows that
//! into `tick`. The difference is stated here and at each test rather than
//! smoothed into one sentence that would be wrong about a quarter of them.
//!
//! # Why this file exists beside `tests/worker.rs`
//!
//! `tests/worker.rs` proves the durability properties of the calls `tick` makes.
//! It calls them **directly**, one at a time, from a test that is the only
//! thing touching the database. Everything here is about the *loop* — that it
//! runs at all, that it stops, that it does not drain, and that two replicas of
//! it can coexist — and none of that is reachable from a direct call.
//!
//! # The `SKIP LOCKED` sentence is not four sentences
//!
//! `egret-automation` needs no leader election because its claim is
//! `FOR UPDATE SKIP LOCKED`, and this worker's claim is the same statement. A
//! tick issues **three** statements and a reaper pass a **fourth**; three of the
//! four have no `SKIP LOCKED` and one has no lock at all. `runtime.rs`'s module
//! doc states each separately and each has its own test here. See in particular
//! [`due_retries_excludes_nothing_so_two_tickers_both_name_the_same_execution`],
//! which asserts the **absence** of exclusion positively: a test that asserted
//! "only one namer wins" would certify a property that statement does not have.
//!
//! # `#[sqlx::test]`, for `tests/worker.rs`'s reason
//!
//! The claim is **global** — a worker serves every workspace, which is the whole
//! point of `SKIP LOCKED` — so two of these tests sharing one database would
//! claim each other's runs. `#[sqlx::test]` gives a scratch database per test.
//! It resolves `DATABASE_URL` through dotenvy unconditionally, so an unexported
//! variable silently falls back to the **dev** database rather than skipping;
//! every test here calls `egret_test_db::assert_database_url_is_exported()`
//! first, which is that crate's own guard for exactly this.
//!
//! # Every wait here has a deadline and every deadline names what it was waiting for
//!
//! [`wait_until`] polls a predicate and **panics with the caller's label** when
//! it runs out. A test of a background loop that hangs tells the next reader
//! nothing; one that says *"all 8 executions terminal: 6 of 8 after 20s"* tells
//! them where to look.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_metastore::Db;
use egret_workflow::runtime::{spawn, RuntimeConfig};
use egret_workflow::worker::{claim, due_retries, start_execution, Outcome, Registry, TaskInput};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod common;

// ── Fixtures ────────────────────────────────────────────────────────────────

/// A one-task flow. Its `core.log.Log` run is what a worker claims; the
/// registries below answer for it, so nothing here depends on the real handler.
fn one_task(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

/// A one-task flow that retries once, `interval` apart. Paired with
/// [`Recorder::failing_every_first_attempt`] this is the only shape that
/// reaches the waker: a `RETRYING` run with no concurrently-live sibling.
fn one_task_with_retry(id: &str, interval: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    retry:\n      type: constant\n      interval: {interval}\n\
         \x20     maxAttempts: 2\n"
    )
}

/// A flow whose **only** task is a bounded `Pause`.
///
/// Deliberately without the successor `tests/worker.rs`'s `a_pause` carries, and
/// that absence is the whole instrument for
/// [`a_tick_that_only_woke_something_returns_zero_so_a_drain_loop_would_never_stop`]:
/// with a successor the wake produces a claimable run and the same tick claims
/// it, so the count is 1 and the property is invisible. With nothing behind the
/// pause, the wake ends the execution and the claim that follows it in the same
/// `tick` finds nothing — which is the documented "a tick that only woke
/// something returns 0", observed rather than quoted.
fn a_lone_bounded_pause(id: &str, bound: &str) -> String {
    format!(
        "id: {id}\n\
         namespace: t\n\
         tasks:\n\
         \x20 - id: pause\n\
         \x20   type: io.kestra.plugin.core.flow.Pause\n\
         \x20   pauseDuration: {bound}\n"
    )
}

/// Records `(execution, task, attempt)` for every run handed to it, and
/// optionally sleeps before answering.
///
/// The recording is the instrument for every concurrency property here: a run
/// executed twice that succeeds twice leaves a trace identical to one executed
/// once, so the handler's call log is the **only** place a duplicate is visible.
///
/// `delay_first` makes the first call slow, which is how the tests force two
/// tickers to actually **overlap** rather than merely interleave — a race that
/// never happens is not a race the test has seen.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(Uuid, String, usize)>>,
    calls: AtomicUsize,
    delay_first: Option<Duration>,
    fail_first_attempt: bool,
}

impl Recorder {
    fn new() -> Self {
        Self::default()
    }
    fn delaying_the_first_call_by(mut self, d: Duration) -> Self {
        self.delay_first = Some(d);
        self
    }
    fn failing_every_first_attempt(mut self) -> Self {
        self.fail_first_attempt = true;
        self
    }
    fn seen(&self) -> Vec<(Uuid, String, usize)> {
        self.seen.lock().unwrap().clone()
    }
    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    /// Only the calls concerning `execution`, so a test with two executions in
    /// flight can assert about one of them.
    fn seen_for(&self, execution: Uuid) -> Vec<(String, usize)> {
        self.seen()
            .into_iter()
            .filter(|(e, _, _)| *e == execution)
            .map(|(_, t, a)| (t, a))
            .collect()
    }
}

#[async_trait::async_trait]
impl Registry for Recorder {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        self.seen
            .lock()
            .unwrap()
            .push((task.execution_id, task.task_id.clone(), task.attempt));
        if first {
            if let Some(d) = self.delay_first {
                tokio::time::sleep(d).await;
            }
        }
        if self.fail_first_attempt && task.attempt == 1 {
            return Ok(Outcome::failed_with("the first attempt always fails"));
        }
        Ok(Outcome::success())
    }
    /// A `panic!` rather than `Ok(())`: this registry's whole instrument is
    /// *which runs were handed to a handler*, and a kill it silently absorbed
    /// would be a kill nobody could see. Nothing here declares a `timeout:`.
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing in these fixtures declares a `timeout:`, so the kill path \
             must never reach {}",
            task.task_id
        );
    }
}

/// The config every test starts from: enabled, fast, and with the reaper parked
/// an hour out so only the tests that are *about* the reaper ever see it.
fn fast(tick_ms: u64, batch: usize) -> RuntimeConfig {
    RuntimeConfig {
        enabled: true,
        tick_interval: Duration::from_millis(tick_ms),
        batch,
        reaper_interval: Duration::from_secs(3600),
        working_timeout: Duration::from_secs(3600),
    }
}

async fn seed(pool: &sqlx::PgPool) -> Uuid {
    let db = Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    ws
}

async fn execution_state(pool: &sqlx::PgPool, execution: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read execution state")
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

async fn lease_of(pool: &sqlx::PgPool, execution: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT claimed_at FROM workflow.task_runs WHERE execution_id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("read lease")
}

async fn is_terminal(pool: &sqlx::PgPool, execution: Uuid) -> bool {
    matches!(
        execution_state(pool, execution).await.as_str(),
        "SUCCESS" | "FAILED" | "WARNING" | "KILLED" | "CANCELLED"
    )
}

/// Poll `cond` every 20 ms until it holds, or panic naming what was still
/// missing. A background-loop test that hangs teaches nobody anything.
async fn wait_until<F, Fut>(label: &str, limit: Duration, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let start = std::time::Instant::now();
    let mut last = String::from("(never evaluated)");
    while start.elapsed() < limit {
        match cond().await {
            Ok(()) => return,
            Err(why) => last = why,
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out after {limit:?} waiting for {label}; last state: {last}");
}

// ── Step 2: the gate, cancellation, and the ticker's cadence ────────────────

/// **The gate, and it is the shipping default.**
///
/// `egret_config::WorkflowConfig::worker_enabled` defaults to `false`, so this
/// is what a deployment that says nothing gets: the runnable run sits there,
/// unclaimed and unleased, exactly as it did before this package existed.
///
/// Asserted on **three** things, because each one alone is satisfiable by a
/// different bug: the registry was never called (no handler ran), the lease is
/// still NULL (no claim was issued either — a claim that ran and then failed to
/// dispatch would leave one), and the execution has not moved.
#[sqlx::test(migrations = "../../migrations")]
async fn spawn_with_the_worker_disabled_never_calls_the_registry(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &one_task("off"), serde_json::json!({}))
        .await
        .expect("start execution");

    let reg = Arc::new(Recorder::new());
    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        enabled: false,
        ..fast(10, 4)
    };
    spawn(pool.clone(), reg.clone(), cfg, cancel.clone());

    // Twenty tick intervals. A ticker that started would have claimed this run
    // on its very first tick, which `tokio::time::interval` fires immediately.
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    assert_eq!(
        reg.seen(),
        Vec::new(),
        "a worker the config disabled must hand nothing to a handler"
    );
    assert!(
        lease_of(&pool, ex).await.is_none(),
        "and it must not have issued a claim either: the lease is what a claim \
         writes, so a NULL one is the only evidence that no claim statement ran"
    );
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("RUNNING"),
        "the execution must be exactly where the webhook left it"
    );
}

/// **A cancelled token stops BOTH tickers**, and the two halves are asserted
/// separately because one token cancelling one ticker is the bug this catches.
///
/// The worker half: work filed *after* the cancel is never picked up. The reaper
/// half: a lease backdated past the timeout *after* the cancel is never
/// reclaimed. Each is the other's control — a test that only watched the worker
/// would pass with the reaper's `select!` arm deleted.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cancelled_token_stops_both_tickers(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;

    let reg = Arc::new(Recorder::new());
    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        enabled: true,
        tick_interval: Duration::from_millis(10),
        batch: 4,
        reaper_interval: Duration::from_millis(10),
        working_timeout: Duration::from_secs(1),
    };
    spawn(pool.clone(), reg.clone(), cfg, cancel.clone());

    // The positive control: with the token live, both loops do their job.
    let before = start_execution(&pool, ws, None, &one_task("before"), serde_json::json!({}))
        .await
        .expect("start before");
    wait_until(
        "the live worker to finish the execution filed before the cancel",
        Duration::from_secs(20),
        || async {
            if is_terminal(&pool, before).await {
                Ok(())
            } else {
                Err(format!(
                    "execution is {}",
                    execution_state(&pool, before).await
                ))
            }
        },
    )
    .await;
    let calls_before_cancel = reg.count();
    assert_eq!(
        calls_before_cancel,
        1,
        "the control: exactly the one run filed before the cancel ran, {:?}",
        reg.seen()
    );

    cancel.cancel();
    // Both loops are inside `select!` on a token that is already cancelled, so
    // this is generous: they break on their very next poll.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // ── worker half ──
    let after = start_execution(&pool, ws, None, &one_task("after"), serde_json::json!({}))
        .await
        .expect("start after");
    // ── reaper half ──
    let leased = start_execution(&pool, ws, None, &one_task("leased"), serde_json::json!({}))
        .await
        .expect("start leased");
    let taken = claim(&pool, 10).await.expect("claim by hand");
    assert!(
        taken.iter().any(|c| c.execution_id == leased),
        "the fixture must actually hold a lease on the run it backdates"
    );
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = now() - interval '2 hours' \
          WHERE execution_id = $1",
    )
    .bind(leased)
    .execute(&pool)
    .await
    .expect("backdate the lease");

    // Fifty intervals of each ticker.
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        reg.count(),
        calls_before_cancel,
        "the claim loop must have stopped: it handled {:?} in total, and \
         everything past the first entry was filed after the cancel",
        reg.seen()
    );
    assert_eq!(
        run_state(&pool, after, "a").await.as_deref(),
        Some("RUNNING"),
        "an execution filed after the cancel must sit untouched"
    );
    assert!(
        lease_of(&pool, leased).await.is_some(),
        "the reaper must have stopped too: this lease is two hours past a \
         one-second timeout, so a live reaper would have cleared it within one \
         of its fifty intervals. This is the half a test that only watched the \
         worker cannot see"
    );
}

/// **`MissedTickBehavior::Skip`: a ticker held up past several of its own
/// intervals does not then fire a burst of catch-up ticks.**
///
/// The arithmetic, because the numbers are the test: the interval is 200 ms and
/// the first handler call blocks for 1200 ms, so **six** ticks are missed. Under
/// `Burst` — `tokio::time::interval`'s default, and what deleting the
/// `set_missed_tick_behavior` line restores — those six fire back to back the
/// instant the slow tick returns, so with `batch = 1` all seven runs are through
/// a handler by roughly t=1250 ms. Under `Skip` the next tick is at t=1400 ms
/// and the rest follow one interval apart, so at t=1500 ms at most **two** have
/// run.
///
/// Measured at t=1500 ms, which is 250 ms clear of `Burst`'s completion and
/// 100 ms clear of `Skip`'s third tick.
///
/// **Why it matters here and not only in `egret-automation`:** a replica whose
/// host slept, or whose event loop was starved, would otherwise wake and issue
/// one claim round trip per missed interval against a database that has nothing
/// new to give. The runnable runs are still there and the next tick takes them.
#[sqlx::test(migrations = "../../migrations")]
async fn a_paused_ticker_does_not_fire_catch_up_ticks(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    for i in 0..7 {
        start_execution(
            &pool,
            ws,
            None,
            &one_task(&format!("catchup{i}")),
            serde_json::json!({}),
        )
        .await
        .expect("start execution");
    }

    let reg = Arc::new(Recorder::new().delaying_the_first_call_by(Duration::from_millis(1200)));
    let cancel = CancellationToken::new();
    spawn(pool.clone(), reg.clone(), fast(200, 1), cancel.clone());

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let ran = reg.count();
    cancel.cancel();

    assert!(
        ran < 7,
        "the first tick's handler blocked for 1200 ms, i.e. past six of this \
         ticker's own 200 ms intervals. With MissedTickBehavior::Skip the next \
         tick is at t=1400 ms and only one or two more runs can have started by \
         t=1500 ms; with Burst (the default, which is what deleting the \
         set_missed_tick_behavior line restores) all six missed ticks fire back \
         to back and every one of the 7 runs is through a handler. {ran} ran."
    );
    assert!(
        ran >= 1,
        "the positive control: the ticker has to have run at all, or this test \
         passes against a loop that never started. {ran} ran."
    );
}

// ── Step 5: a fixed-interval ticker, not a drain loop ───────────────────────

/// **`tick` returns results COMMITTED, not work done — and here is a tick that
/// changed the database and returned 0.**
///
/// This is the fact a `while tick(..)? > 0` drain loop gets wrong, and it is
/// asserted rather than quoted. The fixture is a flow whose only task is a
/// bounded `Pause`: when the bound elapses, `tick`'s `wake` resumes the
/// execution to a terminal state and the `claim` that follows it **in the same
/// call** finds nothing runnable, so the count is 0 while the execution moved
/// from `RUNNING` to terminal.
///
/// A drain loop reading that 0 as "idle" is the benign half. The malign half is
/// the one `tick`'s own doc names: on a flow *with* a successor the same wake
/// reopens a run the same call then claims, so a loop that drained to zero would
/// never stop on an execution whose retry it had just reopened.
///
/// **If you are here because you want to "optimise" the loop into a drain:**
/// this is the red you will get, and the count you would be looping on is the
/// one below.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tick_that_only_woke_something_returns_zero_so_a_drain_loop_would_never_stop(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let ex = start_execution(
        &pool,
        ws,
        None,
        &a_lone_bounded_pause("lone-pause", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");

    let reg = Recorder::new();
    // Before the bound elapses: nothing to wake, nothing to claim.
    let idle = egret_workflow::worker::tick(&pool, 8, &reg)
        .await
        .expect("idle tick");
    assert_eq!(idle, 0, "a tick with nothing due claims nothing");
    assert!(
        !is_terminal(&pool, ex).await,
        "the control: the pause has not elapsed, so the execution is still \
         {}",
        execution_state(&pool, ex).await
    );

    // The bound elapses in REAL time; nothing rewinds `executions.created_at`.
    tokio::time::sleep(Duration::from_millis(1_300)).await;

    let woke = egret_workflow::worker::tick(&pool, 8, &reg)
        .await
        .expect("waking tick");
    assert_eq!(
        woke, 0,
        "**the property**: this tick woke the paused execution and committed \
         that, and it still returns 0, because the count is `results \
         committed` and the wake produced no claimable run. A caller that \
         looped `while tick(..)? > 0` would read this as idle."
    );
    assert!(
        is_terminal(&pool, ex).await,
        "...and the tick that returned 0 is the one that MOVED the execution, \
         which is what makes the 0 a lie about work done. It is {}",
        execution_state(&pool, ex).await
    );
    assert_eq!(
        reg.seen(),
        Vec::new(),
        "no handler ran: a `Pause` is a flowable the executor decides for \
         itself, so nothing here could have been claimed"
    );
}

/// **The loop ticks on a fixed interval rather than draining to zero.**
///
/// Three runnable executions, `batch = 1`, one tick every 2 seconds. A drain
/// loop puts all three through a handler within microseconds of the first tick;
/// a fixed-interval ticker puts through exactly one and waits.
///
/// Measured 600 ms in — 1.4 s clear of the second tick.
///
/// The name is the point: the next reader who "optimises" this loop into
/// `while tick(..)? > 0` gets this red, and
/// [`a_tick_that_only_woke_something_returns_zero_so_a_drain_loop_would_never_stop`]
/// is the reason.
#[sqlx::test(migrations = "../../migrations")]
async fn the_worker_ticks_on_a_fixed_interval_rather_than_draining_to_zero(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    for i in 0..3 {
        start_execution(
            &pool,
            ws,
            None,
            &one_task(&format!("nodrain{i}")),
            serde_json::json!({}),
        )
        .await
        .expect("start execution");
    }

    let reg = Arc::new(Recorder::new());
    let cancel = CancellationToken::new();
    spawn(pool.clone(), reg.clone(), fast(2_000, 1), cancel.clone());

    tokio::time::sleep(Duration::from_millis(600)).await;
    let ran = reg.count();
    cancel.cancel();

    assert_eq!(
        ran,
        1,
        "with `batch = 1` and a 2 s interval, exactly ONE run may have reached \
         a handler 600 ms in. A loop that drained to zero would have put all 3 \
         through on its first tick. It ran {ran}: {:?}",
        reg.seen()
    );
}

// ── Step 4: a safety statement per call, each with its own test ─────────────
//
// Statements 1, 3 and 4 run TWO TICKERS against one database. Statement 2 runs
// two concurrent direct calls, for the reason its own doc gives: what it
// asserts is `due_retries`' return value, which no ticker exposes.

/// **Statement 1 — `claim` is safe by EXCLUSION.** Two tickers against one
/// database never hand one run to two handlers.
///
/// `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n)`: a row a
/// second transaction holds is skipped rather than waited on, so two replicas
/// **partition** the runnable set.
///
/// Asserted on the **handler's call log**, which is the only place a duplicate
/// run is visible at all: a run executed twice that succeeds twice leaves a
/// database state and an execution trace identical to one executed once, so the
/// differential oracle is blind to this by construction.
///
/// **The overlap is forced, not hoped for.** The first handler call blocks for
/// 300 ms, so the ticker that wins the first claim is still inside a handler
/// while the other is claiming — two claim statements genuinely in flight
/// together, which is the situation `SKIP LOCKED` is for. Without that, two
/// tickers can interleave so politely that the race never occurs and the test
/// certifies nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn two_tickers_against_one_database_never_hand_one_run_to_two_handlers(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let mut all = Vec::new();
    for i in 0..8 {
        all.push(
            start_execution(
                &pool,
                ws,
                None,
                &one_task(&format!("two{i}")),
                serde_json::json!({}),
            )
            .await
            .expect("start execution"),
        );
    }

    let reg = Arc::new(Recorder::new().delaying_the_first_call_by(Duration::from_millis(300)));
    let cancel = CancellationToken::new();
    // Two replicas of the same loop, sharing one pool and one registry.
    spawn(pool.clone(), reg.clone(), fast(10, 4), cancel.clone());
    spawn(pool.clone(), reg.clone(), fast(10, 4), cancel.clone());

    wait_until(
        "all 8 executions to reach a terminal state",
        Duration::from_secs(30),
        || async {
            let mut pending = Vec::new();
            for ex in &all {
                if !is_terminal(&pool, *ex).await {
                    pending.push(*ex);
                }
            }
            if pending.is_empty() {
                Ok(())
            } else {
                Err(format!("{} of 8 still running: {pending:?}", pending.len()))
            }
        },
    )
    .await;
    cancel.cancel();

    let seen = reg.seen();
    let mut ids: Vec<Uuid> = seen.iter().map(|(e, _, _)| *e).collect();
    ids.sort();
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(
        ids, unique,
        "two tickers handed the same run to two handlers. The claim's \
         `FOR UPDATE SKIP LOCKED` is what makes N replicas the design rather \
         than a hazard, and this is the ONLY instrument on it — a second \
         execution that also succeeds leaves an identical trace. Call log: \
         {seen:?}"
    );
    assert_eq!(
        ids.len(),
        8,
        "and every run must have reached a handler exactly once: {seen:?}"
    );
}

/// **Statement 2 — `due_retries` excludes NOTHING, and that is asserted
/// positively.**
///
/// `SELECT … FROM workflow.task_runs r JOIN workflow.executions e …` with no
/// `FOR UPDATE`, no `SKIP LOCKED`, and no lock of any kind. Two replicas whose
/// wakers run in the same instant both name the same due execution — every
/// time, not sometimes.
///
/// **This test asserts the duplication rather than its absence, on purpose.**
/// The tempting test here is "only one namer wins", and it would be asserting a
/// property this statement does not have: it would pass by accident of timing,
/// go green for ever, and tell the next reader that the `SKIP LOCKED` argument
/// covers the waker. It does not. The safety lives one call later, in `resume`'s
/// `FOR UPDATE` on the execution row, and
/// [`two_tickers_reopen_a_due_retry_exactly_once`] is where that is proved.
///
/// It is also asserted **twice over**: concurrently (two in-flight selects) and
/// then again sequentially, because a selection that consumed what it named
/// would answer differently the second time.
///
/// **This one is NOT two tickers, unlike its three siblings, and the name is
/// the older half.** It is `tokio::join!` over two direct `due_retries` calls.
/// The thing under assertion is the statement's **return value**, and a ticker
/// swallows that into `tick` — so the ticker shape would cost the assertion and
/// buy no interleaving this does not already produce. Read the name as "two
/// concurrent wakers".
#[sqlx::test(migrations = "../../migrations")]
async fn due_retries_excludes_nothing_so_two_tickers_both_name_the_same_execution(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_retry("dup", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");

    // One tick, by hand, to fail the first attempt and park the run RETRYING.
    let reg = Recorder::new().failing_every_first_attempt();
    egret_workflow::worker::tick(&pool, 8, &reg)
        .await
        .expect("first attempt");
    assert_eq!(
        run_state(&pool, ex, "a").await.as_deref(),
        Some("RETRYING"),
        "the fixture must park the run RETRYING before the deadline is measured"
    );

    // The delay elapses in REAL time.
    tokio::time::sleep(Duration::from_millis(1_300)).await;

    let (left, right) = tokio::join!(due_retries(&pool, 8), due_retries(&pool, 8));
    let left = left.expect("left waker");
    let right = right.expect("right waker");
    assert_eq!(
        left,
        vec![ex],
        "the positive control: the selection names the due execution at all"
    );
    assert_eq!(
        right, left,
        "**the property**: two concurrent wakers name the SAME execution. \
         `due_retries` is a plain SELECT — no FOR UPDATE, no SKIP LOCKED — so \
         the claim's exclusion argument does not reach it and must not be \
         quoted for it. left={left:?} right={right:?}"
    );

    let again = due_retries(&pool, 8).await.expect("third waker");
    assert_eq!(
        again,
        vec![ex],
        "and selection consumes nothing: a third call, after two have already \
         named it, still names it. Nothing about being SELECTED moves the run"
    );
}

/// **Statement 3 — `wake`/`resume` are safe by IDEMPOTENCE under the
/// execution's row lock.** Two tickers reopen a due retry exactly once.
///
/// [`due_retries`] hands both tickers the same id (statement 2), and
/// `resume`'s `load` takes `SELECT … FROM workflow.executions WHERE id = $1
/// FOR UPDATE`. So the loser **blocks** rather than skipping — it must, since
/// skipping would be a woken execution dropped — and when it gets the lock it
/// re-reads the state the winner committed. `advance()` is pure over
/// `(plan, state, now)`, so it sees a run that is no longer `RETRYING` and
/// produces no second reopen.
///
/// Asserted on the **attempt count** and on the handler's call log for this
/// execution, which are the two places a double reopen is visible. The honest
/// residue — the loser's settle still bumps `executions.seq` — is invisible to
/// every consumer and is stated in `runtime.rs`'s module doc rather than
/// rounded off to "no-op".
#[sqlx::test(migrations = "../../migrations")]
async fn two_tickers_reopen_a_due_retry_exactly_once(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let ex = start_execution(
        &pool,
        ws,
        None,
        &one_task_with_retry("wake2", "PT1S"),
        serde_json::json!({}),
    )
    .await
    .expect("start execution");

    let reg = Arc::new(Recorder::new().failing_every_first_attempt());
    let cancel = CancellationToken::new();
    spawn(pool.clone(), reg.clone(), fast(10, 4), cancel.clone());
    spawn(pool.clone(), reg.clone(), fast(10, 4), cancel.clone());

    wait_until(
        "the retried execution to reach a terminal state",
        Duration::from_secs(30),
        || async {
            if is_terminal(&pool, ex).await {
                Ok(())
            } else {
                Err(format!(
                    "execution {} / run {:?}",
                    execution_state(&pool, ex).await,
                    run_state(&pool, ex, "a").await
                ))
            }
        },
    )
    .await;
    cancel.cancel();
    // Let any waker already in flight finish, so a third attempt this test
    // would otherwise miss has time to appear.
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        execution_state(&pool, ex).await,
        "SUCCESS",
        "the control: the retry ran and the second attempt succeeded"
    );
    assert_eq!(
        attempts_of(&pool, ex, "a").await,
        2,
        "**the property**: exactly TWO attempts. Both tickers' `due_retries` \
         named this execution (see \
         due_retries_excludes_nothing_so_two_tickers_both_name_the_same_execution), \
         and the second `resume` must find, under the execution's FOR UPDATE \
         lock, a run that is no longer RETRYING — so it opens nothing"
    );
    assert_eq!(
        reg.seen_for(ex),
        vec![("a".to_string(), 1), ("a".to_string(), 2)],
        "and the handler saw attempt 1 then attempt 2, once each"
    );
}

/// **Statement 4 — `reap` is safe by RE-EVALUATION under READ COMMITTED**, and
/// both directions of its predicate survive two concurrent reapers.
///
/// `UPDATE … SET claimed_at = NULL WHERE claimed_at IS NOT NULL AND state =
/// 'RUNNING' AND claimed_at < now() - interval`: no `SKIP LOCKED`, no explicit
/// lock. The second reaper blocks on the first's row-level write lock and, when
/// the first commits, Postgres re-evaluates the `WHERE` against the committed
/// version — `claimed_at IS NULL` now — so the row drops out of the second
/// update rather than being cleared twice. Clearing an already-clear lease would
/// be harmless; clearing one a *third* worker has since taken would not be, and
/// the re-evaluation is what stops that.
///
/// Both leases are taken by hand first — this is the replica that died between
/// the claim and the commit, which is the only thing that produces an
/// unreleased lease.
///
/// `tests/worker.rs`'s `reap_reclaims_the_expired_lease_and_leaves_the_live_one`
/// asserts the same predicate under **one** reaper; this asserts it under two,
/// and end to end: the reclaimed run goes on to reach a handler exactly once,
/// which a double reclaim followed by a double claim would break.
#[sqlx::test(migrations = "../../migrations")]
async fn two_tickers_reclaim_an_expired_lease_once_and_leave_a_live_one(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let stale = start_execution(&pool, ws, None, &one_task("stale"), serde_json::json!({}))
        .await
        .expect("start stale");
    let fresh = start_execution(&pool, ws, None, &one_task("fresh"), serde_json::json!({}))
        .await
        .expect("start fresh");

    let taken = claim(&pool, 10).await.expect("claim both by hand");
    assert_eq!(
        taken.len(),
        2,
        "both runs must be leased before anything runs"
    );

    // Only one lease is past the timeout. The other keeps the instant the claim
    // wrote, which is what a worker still inside its handler looks like.
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = now() - interval '2 hours' \
          WHERE execution_id = $1",
    )
    .bind(stale)
    .execute(&pool)
    .await
    .expect("backdate the stale lease");

    let reg = Arc::new(Recorder::new());
    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        enabled: true,
        tick_interval: Duration::from_millis(10),
        batch: 4,
        reaper_interval: Duration::from_millis(10),
        working_timeout: Duration::from_secs(3600),
    };
    spawn(pool.clone(), reg.clone(), cfg, cancel.clone());
    spawn(pool.clone(), reg.clone(), cfg, cancel.clone());

    wait_until(
        "the reclaimed execution to reach a terminal state",
        Duration::from_secs(30),
        || async {
            if is_terminal(&pool, stale).await {
                Ok(())
            } else {
                Err(format!(
                    "stale execution is {} with lease {:?}",
                    execution_state(&pool, stale).await,
                    lease_of(&pool, stale).await
                ))
            }
        },
    )
    .await;
    // Fifty more reaper intervals, so a second reclaim has every chance to show.
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();

    assert_eq!(
        reg.seen_for(stale),
        vec![("a".to_string(), 1)],
        "**the property**: the reclaimed run reached a handler exactly ONCE \
         under two concurrent reapers and two concurrent claim loops"
    );
    assert!(
        lease_of(&pool, fresh).await.is_some(),
        "**the other direction, and the one a widened predicate breaks**: a \
         lease INSIDE the timeout is left alone by BOTH reapers, which is why a \
         worker still inside a slow handler is not reclaimed out from under \
         itself"
    );
    assert_eq!(
        reg.seen_for(fresh),
        Vec::new(),
        "and nothing handed that still-leased run to a handler"
    );
}

// ── The two numbers that live in a crate this one cannot see ────────────────

/// **The config's defaults are the ones this crate's own constants and this
/// package's argument say they should be**, and nothing else compares them.
///
/// `egret-config` sits BELOW `egret-workflow`, so
/// `WorkflowConfig::working_timeout_s` cannot name
/// [`DEFAULT_WORKING_TIMEOUT`](egret_workflow::worker::DEFAULT_WORKING_TIMEOUT)
/// and its `3600` is a second copy of that constant that no compiler compares to
/// the first. This is the comparison, and it is why this crate carries
/// `egret-config` as a **dev**-dependency.
///
/// The `worker_enabled` half is the package's own ruling written as an
/// assertion: a loop that has never run in production is opt-in, for
/// `webhooks_enabled`'s reason. A future edit that flips the default to `true`
/// gets this red and has to argue with the message rather than with nobody.
#[test]
fn the_config_default_working_timeout_equals_the_workers_own() {
    let cfg = egret_config::Config::default().workflow;
    assert_eq!(
        Duration::from_secs(cfg.working_timeout_s),
        egret_workflow::worker::DEFAULT_WORKING_TIMEOUT,
        "the reaper's configured default must be the constant this crate ships \
         as `the product default` (worker/mod.rs). They are two literals in two \
         crates and this assertion is the only thing that makes them one number"
    );
    assert!(
        !cfg.worker_enabled,
        "**the ruling**: `worker_enabled` defaults to FALSE. `webhooks_enabled`'s \
         doc argues it — *off is the only safe default for a surface whose own \
         doc says it should not be exposed* — and a claim loop that has never \
         run in production inherits that argument. An operator who wants it says \
         so once, deliberately"
    );
    assert_eq!(
        cfg.tick_interval_secs, 1,
        "the claim tick is the whole latency between `a task became runnable` \
         and `a handler has it`; it is deliberately far finer than \
         `[automation] tick_interval_secs` (30), whose floor is a crontab minute"
    );
    assert_eq!(
        cfg.batch, 8,
        "`tick`'s `n`, bounding both of its statements"
    );
    assert_eq!(
        cfg.reaper_interval_secs, 60,
        "coarser than the claim tick on purpose: a pass can only reclaim leases \
         already past `working_timeout_s`, so running it more often reclaims \
         nothing sooner"
    );
}
