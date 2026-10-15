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
        container_default_timeout: Duration::from_secs(3600),
        scheduler_enabled: false,
        trigger_interval: Duration::from_millis(tick_ms),
        log_limits: egret_workflow::logs::LogLimits::default(),
        log_retention_days: 30,
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
        container_default_timeout: Duration::from_secs(3600),
        scheduler_enabled: false,
        trigger_interval: Duration::from_secs(3600),
        log_limits: egret_workflow::logs::LogLimits::default(),
        log_retention_days: 30,
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
         and the second `resume` must find, under the execution's FOR NO KEY UPDATE \
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
        container_default_timeout: Duration::from_secs(3600),
        scheduler_enabled: false,
        trigger_interval: Duration::from_secs(3600),
        log_limits: egret_workflow::logs::LogLimits::default(),
        log_retention_days: 30,
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

// ── W12: the scheduler ticker ────────────────────────────────────────────────
//
// **Isolation.** A scheduler pass is global -- `fire_due` fires every due
// trigger in the database it is pointed at, not just the calling test's. That
// is safe here for the reason the claim tests above are: every test in this
// file is a `#[sqlx::test]`, so each has a scratch database of its own and no
// test's ticker can see another test's triggers. None of the tests above
// declares a `triggers:` block (`one_task`, `one_task_with_retry` and
// `a_lone_bounded_pause` are the only fixtures, and they start executions with
// `flow_id = None`), and `fast()` leaves the scheduler off, so no existing test
// ever runs the third ticker. The assertions below are per flow anyway.

/// A one-task flow with a single `*/1 * * * *` schedule `s`.
fn cron_flow(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntriggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n\
         \x20   cron: \"*/1 * * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n"
    )
}

/// The scheduler-only variant of [`fast`]: the claim loop and reaper off, the
/// scheduler on and ticking every `ms`.
fn scheduler_only(ms: u64) -> RuntimeConfig {
    RuntimeConfig {
        enabled: false,
        scheduler_enabled: true,
        trigger_interval: Duration::from_millis(ms),
        ..fast(10, 4)
    }
}

/// A draft flow declaring [`cron_flow`]'s schedule, through `FlowRepo::create`,
/// so `scheduler::sync` has already written its trigger row -- NOT yet due.
async fn seed_cron_flow(pool: &sqlx::PgPool, id: &str) -> Uuid {
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &cron_flow(id), "draft").await
}

async fn executions_of_flow(pool: &sqlx::PgPool, flow: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
        .bind(flow)
        .fetch_one(pool)
        .await
        .expect("count the flow's executions")
}

/// **The shipping default is OFF, end to end**: the config a deployment that
/// says nothing gets, carried into `spawn` the way `egret-bin` carries it, and a
/// trigger that is due the whole time.
///
/// The worker is ON here, deliberately: the only gate left to stop the fire is
/// the scheduler's own, so the zero is about that gate and nothing else.
#[sqlx::test(migrations = "../../migrations")]
async fn the_scheduler_is_off_by_default(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let parsed: egret_config::WorkflowConfig =
        serde_json::from_str("{}").expect("an empty [workflow] section parses");
    assert!(
        !parsed.scheduler_enabled,
        "**the ruling**: `scheduler_enabled` defaults to FALSE, for \
         `worker_enabled`'s reason -- a loop that has never run in production \
         is opt-in"
    );
    assert_eq!(parsed.trigger_interval_secs, 1);
    assert_eq!(
        parsed,
        egret_config::Config::default().workflow,
        "serde's fallback and the defaults layer must agree on every key"
    );

    let flow = seed_cron_flow(&pool, "off_by_default").await;
    common::make_due(&Db { pool: pool.clone() }, flow, "s", 1).await;

    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        scheduler_enabled: parsed.scheduler_enabled,
        trigger_interval: Duration::from_millis(10),
        ..fast(10, 4)
    };
    spawn(pool.clone(), Arc::new(Recorder::new()), cfg, cancel.clone());
    // Twenty scheduler intervals; `interval` fires its first tick immediately.
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    assert_eq!(
        executions_of_flow(&pool, flow).await,
        0,
        "a scheduler the config left off must fire nothing, although the \
         trigger was due for all 200 ms"
    );
}

/// **The scheduler's gate is its own**: scheduler on and worker off creates the
/// execution, and nothing then runs it.
///
/// The half that is asserted is the one `if !cfg.enabled { return }` in front
/// of all three tickers would break -- the execution must be **created**. The
/// other half (it does not move) is what "worker off" means, and it is why the
/// runtime warns at start.
#[sqlx::test(migrations = "../../migrations")]
async fn the_scheduler_gate_is_independent_of_the_worker_gate(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let flow = seed_cron_flow(&pool, "gates").await;
    common::make_due(&Db { pool: pool.clone() }, flow, "s", 1).await;

    let reg = Arc::new(Recorder::new());
    let cancel = CancellationToken::new();
    spawn(
        pool.clone(),
        reg.clone(),
        scheduler_only(10),
        cancel.clone(),
    );

    wait_until(
        "the scheduler, with the worker off, to create the due tick's execution",
        Duration::from_secs(5),
        || async {
            match executions_of_flow(&pool, flow).await {
                0 => Err("no execution yet".into()),
                _ => Ok(()),
            }
        },
    )
    .await;
    let ex: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE flow_id = $1 AND scheduled_date IS NOT NULL",
    )
    .bind(flow)
    .fetch_one(&pool)
    .await
    .expect("exactly one scheduled execution");
    let created_as = execution_state(&pool, ex).await;

    // Twenty claim intervals: a claim loop that had started would take the run
    // on its first tick.
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    // The brief this test was written from said "stays CREATED". It does not
    // START there: `start_execution_in` runs the execution's first settle, so
    // a fresh execution is RUNNING with its first task run RUNNING and
    // unleased -- exactly where a webhook-started one sits
    // (`spawn_with_the_worker_disabled_never_calls_the_registry`).
    assert_eq!(created_as, "RUNNING", "a fired schedule's first settle ran");
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("RUNNING"));
    assert_eq!(
        execution_state(&pool, ex).await,
        created_as,
        "and with the worker off nothing advances it"
    );
    assert_eq!(reg.seen(), Vec::new(), "no handler was called");
    assert!(
        lease_of(&pool, ex).await.is_none(),
        "and no claim was issued: the lease is what a claim writes"
    );
}

/// **Two replicas never fire one tick twice** -- the `SKIP LOCKED` claim of
/// `scheduler/mod.rs`'s statement 1, under two tickers against one database.
///
/// Ten flows, each exactly three ticks behind: 30 ticks. Asserted on ROWS:
/// every `(flow_id, scheduled_date)` appears once, the total is exactly 30, and
/// each flow has exactly the three instants it was owed.
///
/// **And it says which mechanism held.** The unique index
/// `workflow_executions_one_per_schedule_tick` is a backstop: a second fire of
/// one tick fails on it, `fire_due` counts that as a consumed tick, and the rows
/// come out identical. So the rows alone cannot tell "the lock excluded the
/// second replica" from "the index absorbed its fire". A `BEFORE INSERT`
/// trigger installed in this test's scratch database counts every *attempted*
/// scheduled insert with `nextval`, which is non-transactional and so survives
/// the savepoint rollback of an insert the index refused. Attempts == rows ==
/// 30 means the lock held; attempts > rows means an insert was rolled back --
/// with the lock gone, that is the index doing the work.
///
/// Measured (W12 Task 5): dropping only `SKIP LOCKED` survives with attempts
/// == rows (a blocked claim re-reads the committed row, so the lock still
/// excludes); dropping the whole lock dies here with 12-30 inserts absorbed
/// and the rows still right; dropping the lock and the index dies on
/// duplicate rows. The difference `attempts - rows` counts ANY rolled-back
/// scheduled insert, not only an index refusal: with neither lock nor index,
/// one run of three showed 1.
#[sqlx::test(migrations = "../../migrations")]
async fn two_scheduler_tickers_never_fire_one_tick_twice(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let mut flows = Vec::new();
    for i in 0..10 {
        flows.push(seed_cron_flow(&pool, &format!("twice{i}")).await);
    }

    // The attempt counter. Its own sequence, in this test's scratch database.
    for ddl in [
        "CREATE SEQUENCE public.w12_fire_attempts",
        "CREATE FUNCTION public.w12_count_fire() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.scheduled_date IS NOT NULL THEN \
           PERFORM nextval('public.w12_fire_attempts'); END IF; RETURN NEW; END $$",
        "CREATE TRIGGER w12_count_fire BEFORE INSERT ON workflow.executions \
         FOR EACH ROW EXECUTE FUNCTION public.w12_count_fire()",
    ] {
        sqlx::query(ddl)
            .execute(&pool)
            .await
            .expect("install the attempt counter");
    }

    // One minute guard for all ten, not ten: `make_due`'s own guard is per
    // call, so ten calls straddling a boundary would owe the first flows a
    // fourth tick. Twenty seconds of margin, then every row is set in one
    // statement from the first one's date.
    let sec: f64 = sqlx::query_scalar("SELECT extract(second FROM now())::float8")
        .fetch_one(&pool)
        .await
        .unwrap();
    if sec >= 40.0 {
        tokio::time::sleep(Duration::from_secs_f64(60.5 - sec)).await;
    }
    common::make_due(&db, flows[0], "s", 3).await;
    let n = sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = \
           (SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1) \
         WHERE flow_id = ANY($2) AND trigger_id = 's'",
    )
    .bind(flows[0])
    .bind(&flows[1..])
    .execute(&pool)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 9);
    let first: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1")
            .bind(flows[0])
            .fetch_one(&pool)
            .await
            .unwrap();
    let owed: Vec<_> = (0..3)
        .map(|k| first + chrono::Duration::minutes(k))
        .collect();

    let cancel = CancellationToken::new();
    let reg = Arc::new(Recorder::new());
    spawn(pool.clone(), reg.clone(), scheduler_only(5), cancel.clone());
    spawn(pool.clone(), reg.clone(), scheduler_only(5), cancel.clone());

    let total = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.executions \
              WHERE flow_id = ANY($1) AND scheduled_date IS NOT NULL",
        )
        .bind(&flows)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    wait_until(
        "30 scheduled executions",
        Duration::from_secs(10),
        || async {
            match total().await {
                30.. => Ok(()),
                n => Err(format!("{n} of 30")),
            }
        },
    )
    .await;
    // Let an in-flight pass finish: nothing else is due until the next minute,
    // so a pass that is still running can only add a double fire.
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    let dup: Vec<(Uuid, chrono::DateTime<chrono::Utc>, i64)> = sqlx::query_as(
        "SELECT flow_id, scheduled_date, count(*) FROM workflow.executions \
          WHERE flow_id = ANY($1) AND scheduled_date IS NOT NULL \
          GROUP BY flow_id, scheduled_date HAVING count(*) > 1",
    )
    .bind(&flows)
    .fetch_all(&pool)
    .await
    .unwrap();
    let attempts: i64 = sqlx::query_scalar(
        "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM public.w12_fire_attempts",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let rows = total().await;
    eprintln!(
        "two_scheduler_tickers: rows = {rows}, insert attempts = {attempts}, \
         rolled back = {}",
        attempts - rows
    );

    assert_eq!(
        dup,
        Vec::new(),
        "one tick fired twice: (flow, tick, executions). Neither the claim's \
         lock nor the unique index held"
    );
    assert_eq!(rows, 30, "ten flows owed three ticks each");
    for f in &flows {
        assert_eq!(
            common::scheduled_dates(&db, *f).await,
            owed,
            "each flow gets exactly the three instants it was owed"
        );
    }
    assert_eq!(
        attempts,
        rows,
        "the rows are right, but {} scheduled insert(s) were attempted and \
         rolled back -- a double fire the claim's `FOR UPDATE OF t SKIP LOCKED` \
         should have excluded, absorbed by \
         `workflow_executions_one_per_schedule_tick`. The index is the \
         backstop, not the mechanism",
        attempts - rows
    );
}

/// **The end to end: a `cron:` runs to SUCCESS with nothing in this test
/// driving it.** No call to `tick`, `fire_due` or `claim` below -- only
/// `spawn`, the production handlers, and the database.
///
/// The control runs first against the same due trigger: the worker on and the
/// scheduler off must leave the flow with no execution at all, so the SUCCESS
/// after it is the scheduler's doing and not something the fixture created.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cron_runs_to_success_with_nothing_driving_it(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let flow = seed_cron_flow(&pool, "e2e").await;
    common::make_due(&Db { pool: pool.clone() }, flow, "s", 1).await;
    let handlers = Arc::new(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let cfg = |scheduler_enabled| RuntimeConfig {
        scheduler_enabled,
        trigger_interval: Duration::from_millis(50),
        ..fast(50, 8)
    };

    // ── control: the scheduler off ──
    let cancel = CancellationToken::new();
    spawn(pool.clone(), handlers.clone(), cfg(false), cancel.clone());
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();
    assert_eq!(
        executions_of_flow(&pool, flow).await,
        0,
        "the control: with the scheduler off, a due cron creates nothing"
    );

    // ── the real thing ──
    let cancel = CancellationToken::new();
    spawn(pool.clone(), handlers.clone(), cfg(true), cancel.clone());
    wait_until(
        "a scheduled execution of the cron flow to reach SUCCESS",
        Duration::from_secs(15),
        || async {
            let states: Vec<String> = sqlx::query_scalar(
                "SELECT state FROM workflow.executions \
                  WHERE flow_id = $1 AND scheduled_date IS NOT NULL",
            )
            .bind(flow)
            .fetch_all(&pool)
            .await
            .unwrap();
            if states.iter().any(|s| s == "SUCCESS") {
                Ok(())
            } else {
                Err(format!("scheduled executions: {states:?}"))
            }
        },
    )
    .await;
    cancel.cancel();
}

/// **`sync`'s upsert, meeting a trigger a pass has claimed, WAITS -- and the
/// advance the pass commits survives the save.** `scheduler/mod.rs`'s
/// statement on `sync`.
///
/// The pass is held inside its transaction, after the claim and at its final
/// `UPDATE`, by a `BEFORE UPDATE` trigger (this test's scratch database only)
/// that waits for an advisory lock this test holds whenever a row's date
/// *moves* -- which the pass's advance does and a save with an unchanged
/// `spec_key` does not. A save of the same flow is then started, and it is
/// observed **blocked on the row lock** in `pg_stat_activity` rather than
/// inferred from timing. Only then does the test release the pass. The save
/// proceeds, reads the advanced row, and keeps its dates: the trigger is one
/// tick ahead, not rewound to the date the save could have read before.
///
/// **Nothing here is timed** (the flaky-wall-clock package): the hold used to
/// be a `pg_sleep(1)`, and on a loaded machine the pass committed before the
/// save was seen waiting. Now the pass is held until released.
#[sqlx::test(migrations = "../../migrations")]
async fn a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = common::seed_flow(&db, ws, user, &cron_flow("wait"), "draft").await;
    common::make_due(&db, flow, "s", 1).await;
    let due: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap();
    // The release: an advisory lock held on a connection of this test's own,
    // which the hold waits to share.
    let mut release = pool.acquire().await.expect("a connection for the release");
    sqlx::query("SELECT pg_advisory_lock(1386)")
        .execute(&mut *release)
        .await
        .expect("take the release lock");
    for ddl in [
        "CREATE FUNCTION public.w12_hold() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.next_evaluation_date IS DISTINCT FROM OLD.next_evaluation_date THEN \
           PERFORM pg_advisory_xact_lock_shared(1386); END IF; RETURN NEW; END $$",
        "CREATE TRIGGER w12_hold BEFORE UPDATE ON workflow.triggers \
         FOR EACH ROW EXECUTE FUNCTION public.w12_hold()",
    ] {
        sqlx::query(ddl)
            .execute(&pool)
            .await
            .expect("install the hold");
    }
    // Backends waiting on a lock of the kind `what` (`pg_stat_activity`'s
    // `wait_event`): `advisory` is the held pass, `transactionid` or `tuple`
    // a writer queued behind the pass's row lock.
    let backends = |what: &'static [&'static str]| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_stat_activity \
                  WHERE datname = current_database() AND pid <> pg_backend_pid() \
                    AND wait_event_type = 'Lock' AND wait_event = ANY($1)",
            )
            .bind(what)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let pass = {
        let pool = pool.clone();
        tokio::spawn(async move { egret_workflow::scheduler::fire_due(&pool).await })
    };
    wait_until(
        "the pass to be held inside its transaction, at its final UPDATE",
        Duration::from_secs(60),
        || async {
            match backends(&["advisory"]).await {
                0 => Err("no backend waiting for the release".into()),
                _ => Ok(()),
            }
        },
    )
    .await;

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let edited = cron_flow("wait").replace("message: hi", "message: edited");
    let save = tokio::spawn(async move {
        repo.update(
            ws,
            flow,
            user,
            &egret_workflow::UpdateFlowInput {
                source: Some(edited),
                plan: Some(serde_json::json!({})),
                ..Default::default()
            },
        )
        .await
    });
    wait_until(
        "the save's `sync` to be blocked on the claimed trigger's row lock",
        Duration::from_secs(60),
        || async {
            match backends(&["transactionid", "tuple"]).await {
                0 => Err("no backend waiting on a row lock".into()),
                _ => Ok(()),
            }
        },
    )
    .await;
    assert!(!save.is_finished(), "the save must still be waiting");
    assert!(!pass.is_finished(), "the pass is still held");

    // Release the pass: it commits, and the save proceeds.
    sqlx::query("SELECT pg_advisory_unlock(1386)")
        .execute(&mut *release)
        .await
        .expect("release the pass");

    let report = pass.await.unwrap().expect("the pass");
    assert_eq!(report.fired, 1, "{report:?}");
    save.await
        .unwrap()
        .expect("the save, once the pass committed");

    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after,
        due + chrono::Duration::minutes(1),
        "the save kept the pass's advance rather than rewinding to the date it \
         would have read before the pass committed"
    );
    assert_eq!(executions_of_flow(&pool, flow).await, 1);
}

/// **W7f §3.7 / §3.11: a save that re-derives a backfill and a pause of that
/// backfill serialise on the flow row.** The pause is SQL standing in for
/// the route (Task 6), in the route's lock order: the flow `FOR NO KEY
/// UPDATE`, then the trigger row, then the backfill row. It is held right
/// after its flow lock while a save that changes the cron starts; the save is
/// observed **blocked on a lock** in `pg_stat_activity`, not inferred from
/// timing. The pause then locks the trigger, pauses and commits; the save
/// proceeds, re-derives, and neither write is lost.
#[sqlx::test(migrations = "../../migrations")]
async fn a_save_that_re_derives_a_backfill_waits_for_a_pause_on_the_flow_row(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = common::seed_flow(&db, ws, user, &cron_flow("pause"), "draft").await;
    let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT date_trunc('hour', now())")
        .fetch_one(&pool)
        .await
        .unwrap();
    let w0 = now - chrono::Duration::hours(2);
    common::stage_backfill(
        &db,
        flow,
        "s",
        (w0, w0 + chrono::Duration::minutes(9)),
        w0,
        10,
        serde_json::json!({}),
        serde_json::json!([]),
    )
    .await;
    // Three ticks done: `current_tick` on an odd minute.
    let cur = w0 + chrono::Duration::minutes(3);
    sqlx::query(
        "UPDATE workflow.trigger_backfills SET current_tick = $2, ticks_done = 3 WHERE flow_id = $1",
    )
    .bind(flow)
    .bind(cur)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = $2 WHERE flow_id = $1")
        .bind(flow)
        .bind(cur)
        .execute(&pool)
        .await
        .unwrap();

    let mut pause = pool.begin().await.expect("the pause's transaction");
    sqlx::query("SELECT id FROM workflow.flows WHERE id = $1 FOR NO KEY UPDATE")
        .bind(flow)
        .execute(&mut *pause)
        .await
        .expect("the pause's flow lock");

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let edited = cron_flow("pause").replace("*/1 * * * *", "*/2 * * * *");
    let save = tokio::spawn(async move {
        repo.update(
            ws,
            flow,
            user,
            &egret_workflow::UpdateFlowInput {
                source: Some(edited),
                plan: Some(serde_json::json!({})),
                ..Default::default()
            },
        )
        .await
    });
    wait_until(
        "the save to be blocked on the flow row the pause holds",
        Duration::from_secs(60),
        || async {
            match lock_waiters(&pool, &["transactionid", "tuple"]).await {
                0 => Err("no backend waiting on a row lock".into()),
                _ => Ok(()),
            }
        },
    )
    .await;
    assert!(!save.is_finished(), "the save must still be waiting");

    sqlx::query(
        "SELECT trigger_id FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's' FOR UPDATE",
    )
    .bind(flow)
    .execute(&mut *pause)
    .await
    .expect("the pause's trigger lock");
    sqlx::query(
        "UPDATE workflow.trigger_backfills SET paused = true, paused_at = now() \
          WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .execute(&mut *pause)
    .await
    .expect("pause");
    pause.commit().await.expect("commit the pause");

    save.await
        .unwrap()
        .expect("the save, once the pause committed");

    let (paused, tick, total, done): (bool, chrono::DateTime<chrono::Utc>, i32, i32) =
        sqlx::query_as(
            "SELECT paused, current_tick, total_ticks, ticks_done \
               FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = 's'",
        )
        .bind(flow)
        .fetch_one(&pool)
        .await
        .unwrap();
    let even = w0 + chrono::Duration::minutes(4);
    // Left under `*/2` in [w0+4, w0+9]: w0+4, w0+6, w0+8.
    assert_eq!((paused, tick, total, done), (true, even, 3 + 3, 3));
    let next: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        next,
        Some(even),
        "the trigger follows the re-derived backfill"
    );
}

/// Backends of this test's database waiting on a lock of the kind `what`
/// (`pg_stat_activity`'s `wait_event`): `advisory` is a held transaction,
/// `transactionid` or `tuple` a writer queued behind a row lock.
async fn lock_waiters(pool: &sqlx::PgPool, what: &'static [&'static str]) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_stat_activity \
          WHERE datname = current_database() AND pid <> pg_backend_pid() \
            AND wait_event_type = 'Lock' AND wait_event = ANY($1)",
    )
    .bind(what)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn wait_for_waiters(pool: &sqlx::PgPool, label: &str, what: &'static [&'static str]) {
    wait_until(label, Duration::from_secs(60), || async {
        match lock_waiters(pool, what).await {
            0 => Err(format!("no backend waiting on {what:?}")),
            _ => Ok(()),
        }
    })
    .await;
}

/// The trigger switch route (W7e Task 5), sent from a task of its own.
fn switch_off_by_route(
    app: &Arc<common::App>,
    ws: Uuid,
    user: egret_core::UserId,
    flow: Uuid,
) -> tokio::task::JoinHandle<axum::http::StatusCode> {
    let app = app.clone();
    tokio::spawn(async move {
        common::put(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{flow}/triggers/s/disabled"),
            user,
            serde_json::json!({"disabled": true}),
        )
        .await
        .status()
    })
}

/// A save of `flow` that edits a task (its `spec_key` unchanged), from a task
/// of its own.
fn save_by_repo(
    db: &Db,
    ws: Uuid,
    user: egret_core::UserId,
    flow: Uuid,
    id: &str,
) -> tokio::task::JoinHandle<egret_core::Result<egret_workflow::repo::FlowRow>> {
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let edited = cron_flow(id).replace("message: hi", "message: edited");
    tokio::spawn(async move {
        repo.update(
            ws,
            flow,
            user,
            &egret_workflow::UpdateFlowInput {
                source: Some(edited),
                plan: Some(serde_json::json!({})),
                ..Default::default()
            },
        )
        .await
    })
}

/// `(workflow.triggers.disabled, the switch rows)` of `flow`'s `s`.
async fn switch_state(pool: &sqlx::PgPool, flow: Uuid) -> (bool, Vec<String>) {
    let copy: bool = sqlx::query_scalar(
        "SELECT disabled FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(pool)
    .await
    .unwrap();
    (
        copy,
        common::switched_off(&Db { pool: pool.clone() }, flow).await,
    )
}

/// **A switch-off that meets a pass holding the trigger row waits, and the
/// pass's fire is the last** (W7e spec §3.2 "Concurrency", against a pass).
///
/// The pass is held at its final `UPDATE` as in
/// [`a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`]. The
/// route takes the flow `FOR NO KEY UPDATE`, which the pass's `KEY SHARE`
/// (its execution insert) does not block, and is then observed blocked on the
/// trigger row the claim holds. Released, the pass commits its one fire, the
/// route commits after it, and a later pass with the next tick due fires
/// nothing: after the route answers, no new fire of that schedule starts.
#[sqlx::test(migrations = "../../migrations")]
async fn a_switch_off_that_meets_a_claimed_trigger_waits_and_the_pass_fires_last(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let flow = common::seed_flow(&db, ws, user, &cron_flow("swoff"), "draft").await;
    let app = Arc::new(common::app(&db).await);
    common::make_due(&db, flow, "s", 1).await;
    let due: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut release = pool.acquire().await.expect("a connection for the release");
    sqlx::query("SELECT pg_advisory_lock(1388)")
        .execute(&mut *release)
        .await
        .expect("take the release lock");
    for ddl in [
        "CREATE FUNCTION public.w7e_hold_pass() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.next_evaluation_date IS DISTINCT FROM OLD.next_evaluation_date THEN \
           PERFORM pg_advisory_xact_lock_shared(1388); END IF; RETURN NEW; END $$",
        "CREATE TRIGGER w7e_hold_pass BEFORE UPDATE ON workflow.triggers \
         FOR EACH ROW EXECUTE FUNCTION public.w7e_hold_pass()",
    ] {
        sqlx::query(ddl)
            .execute(&pool)
            .await
            .expect("install the hold");
    }

    let pass = {
        let pool = pool.clone();
        tokio::spawn(async move { egret_workflow::scheduler::fire_due(&pool).await })
    };
    wait_for_waiters(
        &pool,
        "the pass to be held inside its transaction, at its final UPDATE",
        &["advisory"],
    )
    .await;
    let route = switch_off_by_route(&app, ws, user, flow);
    wait_for_waiters(
        &pool,
        "the switch route to be blocked on the claimed trigger's row lock",
        &["transactionid", "tuple"],
    )
    .await;
    assert!(!route.is_finished(), "the route must still be waiting");
    assert!(!pass.is_finished(), "the pass is still held");

    sqlx::query("SELECT pg_advisory_unlock(1388)")
        .execute(&mut *release)
        .await
        .expect("release the pass");
    let report = pass.await.unwrap().expect("the pass");
    assert_eq!(report.fired, 1, "{report:?}");
    assert_eq!(route.await.unwrap(), axum::http::StatusCode::OK);
    sqlx::query("DROP TRIGGER w7e_hold_pass ON workflow.triggers")
        .execute(&pool)
        .await
        .unwrap();

    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_evaluation_date FROM workflow.triggers WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after,
        due + chrono::Duration::minutes(1),
        "the switch-off kept the pass's advance"
    );
    assert_eq!(switch_state(&pool, flow).await, (true, vec!["s".into()]));
    assert_eq!(executions_of_flow(&pool, flow).await, 1);

    // The clock passing the next tick, without waiting a minute for it: the
    // stored date is moved to the current minute, which is due.
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) \
          WHERE flow_id = $1",
    )
    .bind(flow)
    .execute(&pool)
    .await
    .unwrap();
    let report = egret_workflow::scheduler::fire_due(&pool)
        .await
        .expect("a later pass");
    assert_eq!(report.fired, 0, "{report:?}");
    assert_eq!(
        executions_of_flow(&pool, flow).await,
        1,
        "no fire after the route answered"
    );
}

/// Run `q` (an advisory lock or unlock) on the test's release connection.
async fn advisory(release: &mut sqlx::pool::PoolConnection<sqlx::Postgres>, q: &'static str) {
    sqlx::query(q).execute(&mut **release).await.expect(q);
}

/// **A save and a switch on one flow end with the switch's value, in either
/// order** (W7e spec §3.2 "Concurrency", against a save).
///
/// The switch route locks the flow `FOR NO KEY UPDATE` first, as a save's
/// `UPDATE workflow.flows` does, so the two serialise on the flow row and the
/// save's `sync` never computes the effective copy from a switch about to
/// change. Each order holds the first writer inside its transaction (an
/// advisory lock this test releases), observes the second blocked on a row
/// lock, then releases:
///
/// 1. save, then switch: the save is held at its `sync` upsert on
///    `workflow.triggers` (a `BEFORE UPDATE` trigger), after its flow
///    `UPDATE`;
/// 2. switch, then save: the route is held at its insert into
///    `workflow.disabled_triggers` (a `BEFORE INSERT` trigger), after it
///    locked the flow and the trigger row.
///
/// Without the flow lock, order 2 loses the update: the save reads no switch
/// row, waits on the trigger row the route holds, and writes `disabled =
/// false` over the route's `true` once the route commits.
#[sqlx::test(migrations = "../../migrations")]
async fn a_save_and_a_switch_on_one_flow_end_with_the_switch(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let first = common::seed_flow(&db, ws, user, &cron_flow("save_first"), "draft").await;
    let second = common::seed_flow(&db, ws, user, &cron_flow("switch_first"), "draft").await;
    let app = Arc::new(common::app(&db).await);
    let mut release = pool.acquire().await.expect("a connection for the release");
    sqlx::query(
        "CREATE FUNCTION public.w7e_hold() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_advisory_xact_lock_shared(1389); RETURN NEW; END $$",
    )
    .execute(&pool)
    .await
    .expect("the hold function");

    // ── 1. the save first ──
    advisory(&mut release, "SELECT pg_advisory_lock(1389)").await;
    sqlx::query(
        "CREATE TRIGGER w7e_hold BEFORE UPDATE ON workflow.triggers \
         FOR EACH ROW EXECUTE FUNCTION public.w7e_hold()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let save = save_by_repo(&db, ws, user, first, "save_first");
    wait_for_waiters(
        &pool,
        "the save to be held at its sync upsert, holding the flow row",
        &["advisory"],
    )
    .await;
    let route = switch_off_by_route(&app, ws, user, first);
    wait_for_waiters(
        &pool,
        "the switch route to be blocked on the flow row the save holds",
        &["transactionid", "tuple"],
    )
    .await;
    assert!(!route.is_finished() && !save.is_finished());
    advisory(&mut release, "SELECT pg_advisory_unlock(1389)").await;
    save.await.unwrap().expect("the save");
    assert_eq!(route.await.unwrap(), axum::http::StatusCode::OK);
    sqlx::query("DROP TRIGGER w7e_hold ON workflow.triggers")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        switch_state(&pool, first).await,
        (true, vec!["s".into()]),
        "save, then switch: the switch's value"
    );

    // ── 2. the switch first ──
    advisory(&mut release, "SELECT pg_advisory_lock(1389)").await;
    sqlx::query(
        "CREATE TRIGGER w7e_hold BEFORE INSERT ON workflow.disabled_triggers \
         FOR EACH ROW EXECUTE FUNCTION public.w7e_hold()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let route = switch_off_by_route(&app, ws, user, second);
    wait_for_waiters(
        &pool,
        "the route to be held at its switch-row insert, holding the flow and the trigger row",
        &["advisory"],
    )
    .await;
    let save = save_by_repo(&db, ws, user, second, "switch_first");
    wait_for_waiters(
        &pool,
        "the save to be blocked on a row lock the route holds",
        &["transactionid", "tuple"],
    )
    .await;
    assert!(!route.is_finished() && !save.is_finished());
    advisory(&mut release, "SELECT pg_advisory_unlock(1389)").await;
    assert_eq!(route.await.unwrap(), axum::http::StatusCode::OK);
    save.await.unwrap().expect("the save");
    assert_eq!(
        switch_state(&pool, second).await,
        (true, vec!["s".into()]),
        "switch, then save: the switch's value, not the one the save would \
         have computed before the switch committed"
    );
}

/// **A flow saved before migration 0156 still fires** (final review I1).
///
/// 0156 created `workflow.triggers` empty, and only a `FlowRepo` write calls
/// `sync`, so a flow nobody re-saves would never get its row. The scheduler
/// ticker therefore reconciles every non-deleted flow once, before its first
/// pass. Both flows here are inserted by raw SQL -- as a pre-0156 row is --
/// so no `sync` ran for either; the cron fires every second, so the first
/// tick after the reconciliation is due within one.
#[sqlx::test(migrations = "../../migrations")]
async fn flows_saved_before_the_trigger_table_are_reconciled_at_start(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let src = |id: &str| {
        format!(
            "id: {id}\nnamespace: t\ntriggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n\
             \x20   cron: \"* * * * * *\"\n    withSeconds: true\ntasks:\n  - id: a\n\
             \x20   type: io.kestra.plugin.core.log.Log\n    message: hi\n"
        )
    };
    let insert = |id: &'static str, deleted: bool| {
        let pool = pool.clone();
        let source = src(id);
        async move {
            egret_workflow::spec::parse(&source).expect("fixture must parse");
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO workflow.flows \
                   (workspace_id, namespace, flow_id, source, plan, created_by, deleted_at) \
                 VALUES ($1, 't', $2, $3, '{}'::jsonb, $4, \
                         CASE WHEN $5 THEN now() END) \
                 RETURNING id",
            )
            .bind(ws)
            .bind(id)
            .bind(&source)
            .bind(user.0)
            .bind(deleted)
            .fetch_one(&pool)
            .await
            .expect("insert a flow behind FlowRepo's back")
        }
    };
    let live = insert("pre_0156", false).await;
    let deleted = insert("pre_0156_deleted", true).await;
    let rows_of = |flow: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM workflow.triggers WHERE flow_id = $1",
            )
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(rows_of(live).await, 0, "control: no sync ran at insert");

    let cancel = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Recorder::new()),
        scheduler_only(10),
        cancel.clone(),
    );
    wait_until(
        "the reconciled flow to fire",
        Duration::from_secs(5),
        || async {
            match executions_of_flow(&pool, live).await {
                0 => Err("no execution yet".into()),
                _ => Ok(()),
            }
        },
    )
    .await;
    cancel.cancel();

    assert_eq!(rows_of(live).await, 1, "the reconciliation wrote its row");
    assert_eq!(
        rows_of(deleted).await,
        0,
        "a deleted flow is not reconciled"
    );
}

// ── W13a Task 4: the Flow-trigger processor on the scheduler ticker ─────────
//
// `#[sqlx::test]` for the reason every test above gives: a pass is global, so
// each of these needs a database of its own. Both drive the processor ONLY
// through `spawn`.

/// A one-task flow in namespace `w13` with no trigger.
fn w13_source(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: w13\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n"
    )
}

/// A one-task flow with one unconditioned Flow trigger `on_done` on SUCCESS.
fn w13_listener(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: w13\ntriggers:\n  - id: on_done\n\
         \x20   type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: fired\n"
    )
}

/// **The end to end: a Flow trigger runs its listener to SUCCESS with nothing
/// in this test driving it.** No call to `tick`, `fire_due` or
/// `process_events` below -- only `start_execution` for the producer, `spawn`,
/// the production handlers, and the database.
///
/// The control runs first: the worker on and the scheduler off. The producer
/// runs to SUCCESS and its event is written, and the listener gets no
/// execution at all. Turning the scheduler on then processes that backlog
/// (spec §4.4 point 6) -- the same event, not a new one.
#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_trigger_runs_to_success_with_nothing_driving_it(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let producer_src = w13_source("producer");
    let producer = common::seed_flow(&db, ws, user, &producer_src, "draft").await;
    let listener = common::seed_flow(&db, ws, user, &w13_listener("listener"), "draft").await;
    let handlers = Arc::new(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let cfg = |scheduler_enabled| RuntimeConfig {
        scheduler_enabled,
        trigger_interval: Duration::from_millis(50),
        ..fast(50, 8)
    };

    // ── control: the scheduler off ──
    let cancel = CancellationToken::new();
    spawn(pool.clone(), handlers.clone(), cfg(false), cancel.clone());
    let source = start_execution(
        &pool,
        ws,
        Some(producer),
        &producer_src,
        serde_json::json!({}),
    )
    .await
    .expect("start the producer");
    wait_until(
        "the producer to reach SUCCESS",
        Duration::from_secs(10),
        || async {
            match execution_state(&pool, source).await.as_str() {
                "SUCCESS" => Ok(()),
                s => Err(s.to_string()),
            }
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();
    assert_eq!(
        executions_of_flow(&pool, listener).await,
        0,
        "the control: with the scheduler off, the listener never runs"
    );
    assert_eq!(
        common::events(&db).await,
        vec![(source, "SUCCESS".to_string())],
        "and the event waits in the outbox"
    );

    // ── the real thing ──
    let cancel = CancellationToken::new();
    spawn(pool.clone(), handlers.clone(), cfg(true), cancel.clone());
    wait_until(
        "the listener's execution to reach SUCCESS",
        Duration::from_secs(20),
        || async {
            let states: Vec<String> = sqlx::query_scalar(
                "SELECT state FROM workflow.executions \
                  WHERE flow_id = $1 AND trigger_execution_id = $2",
            )
            .bind(listener)
            .bind(source)
            .fetch_all(&pool)
            .await
            .unwrap();
            if states.iter().any(|s| s == "SUCCESS") {
                Ok(())
            } else {
                Err(format!("listener executions: {states:?}"))
            }
        },
    )
    .await;
    cancel.cancel();
    assert_eq!(
        executions_of_flow(&pool, listener).await,
        1,
        "exactly one fire"
    );
}

/// **Two processors never fire one (event, listener) twice.** Ten producer
/// executions have already reached SUCCESS -- ten events -- and three
/// listeners hear each, so thirty fires are owed. Two scheduler tickers race
/// for them.
///
/// Asserted on rows (no `(event, listener)` twice, exactly 30) **and** on
/// insert *attempts*, counted by a non-transactional sequence as W12's
/// `two_scheduler_tickers_never_fire_one_tick_twice` does: under the claim's
/// lock no fire is even attempted twice, and a rows-only test would pass with
/// the lock gone, because the unique index and `REPEATABLE READ`'s own
/// conflict on the event row both roll the duplicate back.
#[sqlx::test(migrations = "../../migrations")]
async fn two_processors_never_fire_one_event_twice(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let producer_src = w13_source("producer");
    let producer = common::seed_flow(&db, ws, user, &producer_src, "draft").await;
    let mut listeners = Vec::new();
    for i in 0..3 {
        listeners
            .push(common::seed_flow(&db, ws, user, &w13_listener(&format!("l{i}")), "draft").await);
    }
    for _ in 0..10 {
        let id = start_execution(
            &pool,
            ws,
            Some(producer),
            &producer_src,
            serde_json::json!({}),
        )
        .await
        .expect("start a producer");
        common::drive_to(&db, id, "SUCCESS").await;
    }
    assert_eq!(common::events(&db).await.len(), 10, "control: ten events");

    for ddl in [
        "CREATE SEQUENCE public.w13_fire_attempts",
        "CREATE FUNCTION public.w13_count_fire() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.flow_trigger_event_id IS NOT NULL THEN \
           PERFORM nextval('public.w13_fire_attempts'); END IF; RETURN NEW; END $$",
        "CREATE TRIGGER w13_count_fire BEFORE INSERT ON workflow.executions \
         FOR EACH ROW EXECUTE FUNCTION public.w13_count_fire()",
    ] {
        sqlx::query(ddl)
            .execute(&pool)
            .await
            .expect("install the attempt counter");
    }

    let cancel = CancellationToken::new();
    let reg = Arc::new(Recorder::new());
    spawn(pool.clone(), reg.clone(), scheduler_only(5), cancel.clone());
    spawn(pool.clone(), reg.clone(), scheduler_only(5), cancel.clone());
    let total = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.executions WHERE flow_trigger_event_id IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    wait_until("30 fired executions", Duration::from_secs(15), || async {
        match total().await {
            30.. => Ok(()),
            n => Err(format!("{n} of 30")),
        }
    })
    .await;
    wait_until("the outbox to drain", Duration::from_secs(5), || async {
        match common::events(&db).await.len() {
            0 => Ok(()),
            n => Err(format!("{n} events left")),
        }
    })
    .await;
    // Let an in-flight pass finish: nothing is left to claim, so a pass still
    // running can only add a double fire.
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    let dup: Vec<(Option<i64>, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT flow_trigger_event_id, flow_id, count(*) FROM workflow.executions \
          WHERE flow_trigger_event_id IS NOT NULL \
          GROUP BY flow_trigger_event_id, flow_id HAVING count(*) > 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let attempts: i64 = sqlx::query_scalar(
        "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM public.w13_fire_attempts",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let rows = total().await;
    eprintln!(
        "two_processors: rows = {rows}, insert attempts = {attempts}, rolled back = {}",
        attempts - rows
    );

    assert_eq!(dup, Vec::new(), "one (event, listener) fired twice");
    assert_eq!(rows, 30, "ten events, three listeners each");
    for l in &listeners {
        assert_eq!(
            executions_of_flow(&pool, *l).await,
            10,
            "each listener once per event"
        );
    }
    assert_eq!(
        attempts,
        rows,
        "the rows are right, but {} fire(s) were attempted and rolled back -- a \
         double fire the claim's `FOR UPDATE SKIP LOCKED` should have excluded, \
         absorbed by the unique index or by REPEATABLE READ's conflict on the \
         event row. Those are the backstop, not the mechanism",
        attempts - rows
    );
}

/// **A flow-trigger backlog does not hold the schedule back** (W13a final
/// review I3). The processor runs on the scheduler's ticker right after
/// `fire_due`, so an unbounded pass delays every cron tick -- and shutdown --
/// until the whole outbox is drained. A pass is capped instead
/// (`MAX_EVENTS_PER_PASS` or `MAX_PASS_DURATION`, whichever first), and the
/// rest waits for the next tick.
///
/// 200 events are waiting (written by SQL, as a backlog left by a scheduler
/// that was off), and each fire is slowed to ~20 ms by a `BEFORE INSERT`
/// sleep, so draining them takes several seconds. A cron is owed three ticks,
/// and `fire_due` catches up one tick per pass. When the third tick has fired,
/// the backlog must still be there: the schedule was served between capped
/// passes, not after the drain. Then the backlog drains, every event once.
#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_trigger_backlog_does_not_hold_the_schedule_back(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let cron = seed_cron_flow(&pool, "tick").await;
    let (ws, user) = common::seed_workspace(&db).await;
    let producer_src = w13_source("producer");
    let producer = common::seed_flow(&db, ws, user, &producer_src, "draft").await;
    let listener = common::seed_flow(&db, ws, user, &w13_listener("listener"), "draft").await;
    let source = start_execution(
        &pool,
        ws,
        Some(producer),
        &producer_src,
        serde_json::json!({}),
    )
    .await
    .expect("start the producer");
    const BACKLOG: i64 = 200;
    sqlx::query(
        "INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state) \
         SELECT $1, $2, 'SUCCESS' FROM generate_series(1, $3)",
    )
    .bind(source)
    .bind(ws)
    .bind(BACKLOG)
    .execute(&pool)
    .await
    .expect("seed the backlog");
    for ddl in [
        "CREATE FUNCTION public.w13_slow_fire() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN PERFORM pg_sleep(0.02); RETURN NEW; END $$"
            .to_string(),
        format!(
            "CREATE TRIGGER w13_slow_fire BEFORE INSERT ON workflow.executions \
             FOR EACH ROW WHEN (NEW.flow_id = '{listener}') EXECUTE FUNCTION public.w13_slow_fire()"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl))
            .execute(&pool)
            .await
            .expect("slow the listener's fires down");
    }
    let sec: f64 = sqlx::query_scalar("SELECT extract(second FROM now())::float8")
        .fetch_one(&pool)
        .await
        .unwrap();
    if sec >= 40.0 {
        tokio::time::sleep(Duration::from_secs_f64(60.5 - sec)).await;
    }
    common::make_due(&db, cron, "s", 3).await;

    let outbox = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workflow.flow_trigger_events")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let cancel = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Recorder::new()),
        scheduler_only(50),
        cancel.clone(),
    );
    wait_until(
        "the cron's three owed ticks",
        Duration::from_secs(20),
        || async {
            match common::scheduled_dates(&db, cron).await.len() {
                3.. => Ok(()),
                n => Err(format!("{n} of 3")),
            }
        },
    )
    .await;
    let left = outbox().await;
    assert!(
        left > 0,
        "the third cron tick fired only after the whole backlog of {BACKLOG} was drained: \
         the processor's pass held the ticker"
    );
    eprintln!("backlog left when the third tick fired: {left} of {BACKLOG}");

    wait_until(
        "the backlog to drain over later passes",
        Duration::from_secs(60),
        || async {
            match outbox().await {
                0 => Ok(()),
                n => Err(format!("{n} events left")),
            }
        },
    )
    .await;
    cancel.cancel();
    assert_eq!(
        executions_of_flow(&pool, listener).await,
        BACKLOG,
        "every event fired its listener once"
    );
}

/// **The scheduler ticker sweeps expired `dependsOn` windows** (W13b, spec
/// §5.3 step 6): once per pass, after `process_events`. Two listener windows
/// are written by SQL, one expired and one live; with nothing but `spawn`
/// running, the expired one goes and the live one stays.
#[sqlx::test(migrations = "../../migrations")]
async fn the_scheduler_ticker_sweeps_expired_windows(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let depends_on = |id: &str| {
        format!(
            "id: {id}\nnamespace: w13\ntriggers:\n  - id: on_done\n\
             \x20   type: io.kestra.plugin.core.trigger.Flow\n    dependsOn:\n\
             \x20     - namespace: w13\n        flowId: producer\ntasks:\n\
             \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: fired\n"
        )
    };
    let expired = common::seed_flow(&db, ws, user, &depends_on("expired"), "draft").await;
    let live = common::seed_flow(&db, ws, user, &depends_on("live"), "draft").await;
    for (flow, start, end) in [
        (
            expired,
            "now() - interval '2 days'",
            "now() - interval '1 day'",
        ),
        (
            live,
            "now() - interval '1 hour'",
            "now() + interval '1 day'",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO workflow.flow_trigger_windows \
               (flow_id, trigger_id, window_start, window_end, results) \
             VALUES ($1, 'on_done', {start}, {end}, '{{}}')"
        )))
        .bind(flow)
        .execute(&pool)
        .await
        .expect("seed a window");
    }
    let windows = || async {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT flow_id FROM workflow.flow_trigger_windows ORDER BY flow_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    assert_eq!(windows().await.len(), 2, "control");

    let cancel = CancellationToken::new();
    let reg = Arc::new(Recorder::new());
    spawn(pool.clone(), reg, scheduler_only(50), cancel.clone());
    wait_until(
        "the expired window to be swept",
        Duration::from_secs(10),
        || async {
            match windows().await.as_slice() {
                [only] if *only == live => Ok(()),
                other => Err(format!("{other:?}")),
            }
        },
    )
    .await;
    cancel.cancel();
}

// ── W5a Task 3: the lease is renewed while a handler runs ───────────────────

/// **A run longer than `working_timeout` is not reaped while its worker
/// lives.**
///
/// The real runtime, reaper ticking every 100 ms against a 2 s
/// `working_timeout`, and a handler that takes 5 s. Before W5a the reaper
/// cleared the lease at 2 s, the handler's result then hit the fence
/// (`LeaseLost`), and the next tick ran the handler a second time — so the
/// instrument is the handler's call count, which is the only place a second
/// run is visible (the attempt count does not move on a reclaim; see
/// `worker::claim::tick`'s doc, cause 3). Asserted beside it: one outcome, one
/// attempt, and the lease observed to move forward mid-handler.
#[sqlx::test(migrations = "../../migrations")]
async fn a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let ws = seed(&pool).await;
    let ex = start_execution(&pool, ws, None, &one_task("long"), serde_json::json!({}))
        .await
        .expect("start execution");

    let reg = Arc::new(Recorder::new().delaying_the_first_call_by(Duration::from_secs(5)));
    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        reaper_interval: Duration::from_millis(100),
        working_timeout: Duration::from_secs(2),
        ..fast(50, 1)
    };
    spawn(pool.clone(), reg.clone(), cfg, cancel.clone());

    wait_until("the handler started", Duration::from_secs(5), || async {
        if reg.count() >= 1 {
            Ok(())
        } else {
            Err("no call yet".into())
        }
    })
    .await;
    let first = lease_of(&pool, ex).await.expect("leased");
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    let later = lease_of(&pool, ex).await;
    assert!(
        matches!(later, Some(t) if t > first),
        "past one working_timeout the lease is still held and has moved \
         forward: claimed {first}, now {later:?}"
    );

    wait_until(
        "the execution terminal",
        Duration::from_secs(15),
        || async {
            if is_terminal(&pool, ex).await {
                Ok(())
            } else {
                Err(format!("calls {}", reg.count()))
            }
        },
    )
    .await;
    cancel.cancel();

    assert_eq!(
        reg.seen_for(ex),
        vec![("a".to_string(), 1)],
        "exactly one handler call: the reaper never handed the run to a second"
    );
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("SUCCESS"));
    assert_eq!(attempts_of(&pool, ex, "a").await, 1);
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
}

/// `WorkflowConfig::container_default_timeout_s`'s 3600 is a second copy of
/// [`DEFAULT_CONTAINER_TIMEOUT`](egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT)
/// in a crate that cannot name it; this is the comparison, for
/// [`the_config_default_working_timeout_equals_the_workers_own`]'s reason.
#[test]
fn the_config_default_container_timeout_equals_the_workers_own() {
    let cfg = egret_config::Config::default().workflow;
    assert_eq!(
        Duration::from_secs(cfg.container_default_timeout_s),
        egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT
    );
    assert_eq!(
        egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
        Duration::from_secs(3600),
        "W5a spec §6: one hour"
    );
    assert_eq!(
        egret_workflow::worker::WorkerTiming::default(),
        egret_workflow::worker::WorkerTiming {
            working_timeout: egret_workflow::worker::DEFAULT_WORKING_TIMEOUT,
            container_default_timeout: egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
        }
    );
}

// ── W5a Task 5: the deployment's registry, and the orphan sweep's call site ──

/// W5b: a `SecretSource` for registries these tests build but never run a
/// plugin task through.
struct NoSecrets;

#[async_trait::async_trait]
impl egret_workflow::runners::secrets::SecretSource for NoSecrets {
    async fn get(
        &self,
        _: uuid::Uuid,
        name: &str,
    ) -> Result<
        std::collections::BTreeMap<String, String>,
        egret_workflow::runners::secrets::SecretError,
    > {
        Err(egret_workflow::runners::secrets::SecretError::NotFound { name: name.into() })
    }
}

fn no_secrets() -> std::sync::Arc<dyn egret_workflow::runners::secrets::SecretSource> {
    std::sync::Arc::new(NoSecrets)
}

/// A pool that is never connected: `handlers_for` stores it for the run-time
/// feature read and the re-attach window, and none of these tests runs a task.
fn unconnected_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool parses")
}

/// **With `container_runner = none` (the default) the production registry is
/// what it was before W5a** -- `Handlers::with_core_tasks(egret_workflow::storage::current())`, no orphan
/// sweep, no default runner for publish -- plus, since W9a Task 11, the two
/// core storage tasks that need the pool (`handlers::storage::register`), on
/// every deployment; and with `docker` it is that plus exactly the four
/// script types and their two aliases.
#[tokio::test]
async fn with_container_runner_none_the_registry_is_unchanged() {
    use egret_workflow::scripts::RunnerKind;
    use egret_workflow::worker::Handlers;

    let mut cfg = egret_config::Config::default().workflow;
    assert_eq!(
        cfg.container_runner,
        egret_config::ContainerRunner::None,
        "`none` is the shipping default"
    );
    let (reg, default_runner) =
        egret_workflow::runtime::handlers_for(&cfg, unconnected_pool(), no_secrets())
            .await
            .expect("`none` needs nothing");
    let mut want: Vec<String> = Handlers::with_core_tasks(egret_workflow::storage::current())
        .type_ids()
        .into_iter()
        .map(String::from)
        .collect();
    want.extend([
        egret_workflow::worker::handlers::storage::DELETE.to_string(),
        egret_workflow::worker::handlers::storage::PURGE_CURRENT_EXECUTION_FILES.to_string(),
        // W9b Task 6: the namespace tasks, registered beside them.
        egret_workflow::worker::handlers::namespace::UPLOAD_FILES.to_string(),
        egret_workflow::worker::handlers::namespace::DOWNLOAD_FILES.to_string(),
        // W9b Task 7.
        egret_workflow::worker::handlers::namespace::DELETE_FILES.to_string(),
        egret_workflow::worker::handlers::namespace::PURGE_FILES.to_string(),
    ]);
    // W9c Task 5: the six KV tasks, registered beside them.
    want.extend(
        egret_workflow::worker::handlers::kv::DECLARED
            .iter()
            .map(|c| c.0.to_string()),
    );
    want.sort_unstable();
    assert_eq!(reg.type_ids(), want);
    assert_eq!(default_runner, None, "publish has no runner to default to");
    assert!(reg.orphan_sweep().is_none(), "nothing to sweep");

    // The control: the same call with a runner configured DOES register --
    // so the equality above is not a function that never registers anything.
    cfg.container_runner = egret_config::ContainerRunner::Docker;
    // W5b: a runner needs the plugin host's image, a boot error without it.
    let err = egret_workflow::runtime::handlers_for(&cfg, unconnected_pool(), no_secrets())
        .await
        .expect_err("a runner without workflow.plugin_host.image");
    assert!(
        err.to_string().contains("workflow.plugin_host.image"),
        "{err}"
    );
    cfg.plugin_host.image = Some("egret-plugin-host:dev".into());
    let (reg, default_runner) =
        egret_workflow::runtime::handlers_for(&cfg, unconnected_pool(), no_secrets())
            .await
            .expect("the Docker runner is built without a daemon");
    let core = Handlers::with_core_tasks(egret_workflow::storage::current())
        .type_ids()
        .len();
    let added: Vec<&str> = reg
        .type_ids()
        .into_iter()
        .filter(|id| {
            Handlers::with_core_tasks(egret_workflow::storage::current())
                .get(id)
                .is_none()
        })
        .filter(|id| !id.starts_with(egret_workflow::plugins::PREFIX))
        .collect();
    // W5b: plus one `kestra.<FQCN>` per catalog class, over the same runner.
    let plugins: Vec<&str> = reg
        .type_ids()
        .into_iter()
        .filter(|id| id.starts_with(egret_workflow::plugins::PREFIX))
        .collect();
    assert_eq!(
        plugins,
        egret_workflow::plugins::prefixed_types().collect::<Vec<_>>()
    );
    // W9a Task 9: nine, with Batch ×2, CopyIn and CopyOut re-admitted.
    assert_eq!(plugins.len(), 9);
    assert_eq!(
        added,
        vec![
            "egret.container",
            "egret.script.python",
            // W9c Task 5.
            "io.kestra.plugin.core.kv.Delete",
            "io.kestra.plugin.core.kv.Get",
            "io.kestra.plugin.core.kv.GetKeys",
            "io.kestra.plugin.core.kv.PurgeKV",
            "io.kestra.plugin.core.kv.Put",
            "io.kestra.plugin.core.kv.Set",
            "io.kestra.plugin.core.namespace.DeleteFiles",
            "io.kestra.plugin.core.namespace.DownloadFiles",
            "io.kestra.plugin.core.namespace.PurgeFiles",
            "io.kestra.plugin.core.namespace.UploadFiles",
            "io.kestra.plugin.core.storage.Delete",
            "io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles",
            "io.kestra.plugin.scripts.python.Commands",
            "io.kestra.plugin.scripts.python.Script",
            "io.kestra.plugin.scripts.shell.Commands",
            "io.kestra.plugin.scripts.shell.Script",
        ]
    );
    assert_eq!(reg.type_ids().len(), core + 2 + 4 + 6 + 6 + 9);
    assert_eq!(default_runner, Some(RunnerKind::Docker));
    assert!(
        reg.orphan_sweep().is_some(),
        "the runner is the reaper's sweep"
    );
}

/// A Kubernetes configuration egret refuses stops the deployment at start,
/// naming the setting -- before any cluster client is built.
#[tokio::test]
async fn a_refused_kubernetes_configuration_stops_the_process_at_start() {
    let mut cfg = egret_config::Config::default().workflow;
    cfg.container_runner = egret_config::ContainerRunner::Kubernetes;
    cfg.kubernetes.run_as_user = 0;
    // W5b: an image, so the refusal under test is the uid's and not the
    // missing plugin host image's (checked first).
    cfg.plugin_host.image = Some("egret-plugin-host:dev".into());
    let err = egret_workflow::runtime::handlers_for(&cfg, unconnected_pool(), no_secrets())
        .await
        .expect_err("runAsNonRoot pods cannot run as uid 0");
    assert!(err.to_string().contains("run_as_user"), "{err}");
}

/// `workflow.kubernetes.*` reaches the runner's `PodConfig` field for field.
#[test]
fn the_kubernetes_config_becomes_the_pod_config() {
    let mut k = egret_config::WorkflowKubernetesConfig {
        namespace: "tasks-x".into(),
        image_pull_secrets: vec!["regcred".into()],
        ..Default::default()
    };
    k.max_resources.request.cpu = Some("3".into());
    let pod = egret_workflow::runtime::pod_config(&k);
    assert_eq!(pod.namespace, "tasks-x");
    assert_eq!(pod.run_as_user, 1000);
    assert_eq!(pod.helper_image, "egret-task-helper:dev");
    assert_eq!(pod.image_pull_secrets, vec!["regcred".to_string()]);
    assert_eq!(pod.default_resources.request.cpu.as_deref(), Some("100m"));
    assert_eq!(pod.default_resources.limit.memory.as_deref(), Some("512Mi"));
    assert_eq!(pod.max_resources.request.cpu.as_deref(), Some("3"));
    assert_eq!(pod.max_resources.limit.memory.as_deref(), Some("2Gi"));
}

/// An [`OrphanSweep`](egret_workflow::worker::commands::OrphanSweep) that
/// counts its calls.
#[derive(Debug, Default)]
struct CountingSweep {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl egret_workflow::worker::commands::OrphanSweep for CountingSweep {
    async fn sweep_orphans(&self, _pool: &sqlx::PgPool) -> egret_core::Result<u64> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }
}

/// **The reaper's tick calls the registry's orphan sweep** (W5a spec §5,
/// *Orphans*; the call site Task 4 left to Task 5) -- through the real
/// `spawn`, so the wiring is what is judged. The claim loop is parked; only
/// the reaper ticks.
#[sqlx::test(migrations = "../../migrations")]
async fn the_reaper_tick_sweeps_the_configured_runners_orphans(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let sweep = Arc::new(CountingSweep::default());
    let reg = egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current())
        .with_orphan_sweep(sweep.clone());
    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        tick_interval: Duration::from_secs(3600),
        reaper_interval: Duration::from_millis(50),
        ..fast(3_600_000, 1)
    };
    let handle = spawn(pool.clone(), Arc::new(reg), cfg, cancel.clone());
    wait_until(
        "three sweeps on the reaper's tick",
        Duration::from_secs(10),
        || {
            let n = sweep.calls.load(Ordering::SeqCst);
            async move {
                if n >= 3 {
                    Ok(())
                } else {
                    Err(format!("{n} sweeps"))
                }
            }
        },
    )
    .await;
    cancel.cancel();
    handle.join().await;
}

/// **A pause that commits between a pass's claim and its read of the backfill
/// row writes nothing** (W7f spec §3.6, D22).
///
/// The claim's snapshot predates the pause, so the claim's `NOT EXISTS (…
/// paused)` lets the trigger through; `fire_one`'s fresh read of the backfill,
/// a later statement, sees the committed pause and returns `Frozen`. The pass
/// is parked right after its claim by the test-only hook
/// (`scheduler::test_hold`, armed for this flow only), the pause is written
/// while it is parked -- the pass holds the trigger row, not the backfill
/// row, so the write does not wait -- and then the pass is released.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pause_between_the_claim_and_the_backfill_read_fires_nothing(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = common::seed_flow(&db, ws, user, &cron_flow("bfpause"), "draft").await;
    let w0: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT date_trunc('minute', now()) - interval '10 minutes'")
            .fetch_one(&pool)
            .await
            .unwrap();
    common::stage_backfill(
        &db,
        flow,
        "s",
        (w0, w0 + chrono::Duration::minutes(4)),
        w0,
        5,
        serde_json::json!({}),
        serde_json::json!([]),
    )
    .await;
    let trigger_row = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT row_to_json(t) FROM workflow.triggers t WHERE flow_id = $1",
            )
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let trigger_before = trigger_row().await;

    let hold = egret_workflow::scheduler::test_hold::arm(flow);
    let pass = {
        let pool = pool.clone();
        tokio::spawn(async move { egret_workflow::scheduler::fire_due(&pool).await })
    };
    tokio::time::timeout(Duration::from_secs(60), hold.reached())
        .await
        .expect("the pass to be parked after its claim");
    sqlx::query(
        "UPDATE workflow.trigger_backfills SET paused = true, paused_at = now() \
          WHERE flow_id = $1",
    )
    .bind(flow)
    .execute(&pool)
    .await
    .expect("pause while the pass is parked");
    let backfill_paused = common::backfill_row(&db, flow, "s").await;
    hold.release();

    let report = tokio::time::timeout(Duration::from_secs(60), pass)
        .await
        .expect("the pass to finish once released")
        .expect("the pass task")
        .expect("the pass");
    assert_eq!(
        report,
        egret_workflow::scheduler::PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    let executions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(executions, 0, "nothing fired");
    assert_eq!(
        trigger_row().await,
        trigger_before,
        "the trigger row untouched"
    );
    assert_eq!(
        common::backfill_row(&db, flow, "s").await,
        backfill_paused,
        "the backfill row untouched"
    );
}
