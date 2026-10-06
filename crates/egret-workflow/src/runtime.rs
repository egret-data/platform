//! The background loops that make this engine run, and a safety statement per
//! statement they issue.
//!
//! **W12 added a third ticker, the schedule-trigger loop, behind its own gate
//! (`scheduler_enabled`). Its per-statement safety section is
//! [`scheduler`](crate::scheduler)'s module doc, not this one.** Everything
//! below is about the claim loop and the reaper.
//!
//! Everything below this module works and, until W10 Task 2, **nothing ran it**:
//! [`worker::tick`](crate::worker::tick) had no caller in any `crates/*/src`, so
//! a webhook or an API call created an `workflow.executions` row and nothing
//! ever advanced it. [`spawn`] is the caller. It is deliberately thin — two
//! `tokio::time::interval` tickers over calls this crate already exports — and
//! the substance of the module is the concurrency argument below, which is
//! **not** thin and which the code it guards cannot state for itself.
//!
//! The shape is `egret_automation::runner::spawn`'s, down to the
//! `MissedTickBehavior::Skip` and the `tokio::select!` on a
//! [`CancellationToken`]: `egret-bin` cancels one token on SIGINT and every
//! background loop it started stops.
//!
//! # Two tickers, not one
//!
//! The reaper's cadence is not the worker's and cannot be. A claim tick's
//! interval is the latency between "a task became runnable" and "a handler has
//! it" — seconds at most. A reaper pass can only reclaim leases already past
//! `working_timeout` (an hour, by default), so running it at the claim's
//! cadence would issue 3600 `UPDATE`s to reclaim what one would. They are
//! separate `tokio::spawn`s reading separate intervals, and a slow reaper pass
//! must never delay a claim tick.
//!
//! # The `SKIP LOCKED` argument covers the claim and NOTHING ELSE
//!
//! `egret-automation`'s scheduler needs no leader election because its claim is
//! `FOR UPDATE SKIP LOCKED`, and this worker's claim is the same statement — so
//! it is tempting to write that sentence once and let it stand for the loop.
//! **It does not.** A tick issues three statements and a reaper pass issues a
//! fourth; three of the four contain no `SKIP LOCKED` at all and one contains no
//! lock of any kind. Each gets its own statement below, and each has its own
//! test in `tests/runtime.rs` — because a guard nobody has seen fail is a guard
//! nobody has seen.
//!
//! **Three of the four run two tickers against one database; statement 2 runs
//! two concurrent direct calls, and the difference is named rather than
//! smoothed over.** `due_retries` returns its selection, and a selection is not
//! observable from inside a ticker — the ticker swallows the value into `tick`.
//! Wrapping it in tickers would trade the assertion for nondeterminism and add
//! no interleaving the direct pair does not already produce.
//!
//! ## 1. [`claim`](crate::worker::claim) — safe by **exclusion**
//!
//! `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n) RETURNING …`
//! (`worker/claim.rs:360`). Two replicas ticking together **partition** the
//! runnable set: a row another transaction has locked is skipped rather than
//! waited on, so neither claimer blocks and no run is taken twice. This is the
//! only one of the four whose safety is exclusion at selection time, and it is
//! the reason N replicas is the design rather than a hazard.
//!
//! Proved by `two_tickers_against_one_database_never_hand_one_run_to_two_handlers`,
//! which asserts on the **handler's call log** — the only place a duplicate run
//! is visible at all, since a run executed twice that succeeds twice leaves a
//! trace identical to one executed once.
//!
//! ## 2. [`due_retries`](crate::worker::due_retries) — **not** safe by exclusion, and it does not try to be
//!
//! A plain `SELECT` (`worker/claim.rs:614`). **No `FOR UPDATE`, no
//! `SKIP LOCKED`, no lock of any kind.** Two replicas whose wakers run in the
//! same instant both name the same due execution, every time, and that is not a
//! race to be closed — it is the statement's contract. Selection here is a
//! *read*; the exclusion happens one call later.
//!
//! Proved by `due_retries_excludes_nothing_so_two_tickers_both_name_the_same_execution`,
//! which asserts the duplication **positively**. It is written that way on
//! purpose: a test that asserted "only one namer wins" would be asserting a
//! property this statement does not have, and would pass only by accident of
//! timing — which is the shipped-wrong-assertion failure this branch pays the
//! most for.
//!
//! **What that test actually runs is `tokio::join!` on two direct
//! `due_retries` calls plus a third sequential one — not two tickers**, and the
//! name is the older half. The value under assertion is the statement's
//! *return*, which no ticker exposes; the third call is what says selection
//! consumes nothing. Read the name as "two concurrent wakers", which is what it
//! measures.
//!
//! ## 3. [`wake`](crate::worker::wake)/[`resume`](crate::worker::resume) — safe by **idempotence under the execution's row lock**
//!
//! [`wake`](crate::worker::wake) hands each id
//! [`due_retries`](crate::worker::due_retries) named to
//! [`resume`](crate::worker::resume), and `resume`'s
//! `load` takes `SELECT … FROM workflow.executions WHERE id = $1 FOR NO KEY
//! UPDATE` (`FOR UPDATE` until W13a final review I2; the weaker mode conflicts
//! with itself exactly as `FOR UPDATE` does, so nothing below changes)
//! (`worker/claim.rs:1521`). So the second replica's `resume` **blocks** behind
//! the first's — it does not skip, and it must not: skipping would be a woken
//! execution dropped. When it acquires the lock it re-reads the state the first
//! one committed, and `advance()` is pure over `(plan, state, now)`, so it sees
//! a run that is no longer `RETRYING` and produces **no second reopen**. The
//! reopened run's attempt is opened once.
//!
//! **The honest residue, stated rather than rounded to "no-op":** the loser's
//! settle still writes a new `executions.seq` even though it produces no
//! commands — [`due_retries`](crate::worker::due_retries)'s own doc names that
//! cost for the same statement.
//! A redundant `seq` bump is invisible to every consumer; a second attempt would
//! not be, and that is the invariant the test asserts.
//!
//! Proved by `two_tickers_reopen_a_due_retry_exactly_once`, which asserts on the
//! run's **attempt count** (2, not 3) and on the handler's call log for that run
//! — the two places a double reopen is visible.
//!
//! ### The lock is load-bearing, and a surviving mutant does not say otherwise
//!
//! **Do not remove the `FOR NO KEY UPDATE`.** W10 Task 2 mutation F flipped
//! `claim::load` from `Lock::Yes` to `Lock::No` — the whole of the
//! serialisation this statement names — and
//! `two_tickers_reopen_a_due_retry_exactly_once` stayed green, 3 runs of 3,
//! against a control of 10/10. That is a fact about the **fixture**, not about
//! the lock.
//!
//! **Why the mutant lives: idempotence by VALUE, not exclusion.** `advance()`
//! is pure over `(plan, state, now)`. Two racers that both `load` before either
//! commits read the *same* `before` and therefore compute the *same* `after`,
//! so the loser's write puts back exactly the bytes the winner wrote. Writing a
//! value twice is invisible.
//!
//! **What the lock actually buys is the window the fixture never opens.**
//! Nothing in `resume` delays between `load` and `settle`, so this ordering
//! never occurs in the test:
//!
//! 1. loser `load`s — sees the run `RETRYING`, deadline elapsed;
//! 2. winner commits the reopen — run `CREATED`, attempt 2 open;
//! 3. the claim takes attempt 2 and a handler runs it;
//! 4. **loser writes its stale `after` back**, reopening a run that has already
//!    been reopened — a third attempt, from a snapshot three steps old.
//!
//! Step 4 is a **lost update**, and it is not prevented by anything else on this
//! path. Row-level write locks serialise the two writers but do not stop it:
//! `persist`'s UPDATE is keyed `WHERE execution_id = $8 AND run_ix = $9`, so
//! after the loser unblocks the predicate still matches and its stale values
//! land.
//!
//! **Two mechanisms an earlier draft of this paragraph named do NOT apply here,
//! and the correction is kept because the wrong version is the more plausible
//! reading.** `resume` calls `settle(…, None)`, so `persist`'s `release` is
//! `None`: its `rows_affected` fence sits inside `if let Some(l) = lease` and is
//! **never evaluated** on this path, and its `claimed_at` term binds NULL and is
//! vacuously true. And `UNIQUE (execution_id, run_ix)` (migration 0134) guards
//! the **INSERT** branch — `ix >= before.runs.len()`, a run being created —
//! whereas reopening an existing run is the **UPDATE** branch. Neither is a
//! backstop for step 4.
//!
//! So: the test asserts the **outcome** a deployment has (one reopen), the lock
//! is what keeps that outcome true in an interleaving the test cannot force, and
//! *"the mutant survives"* is a statement about the fixture's reach. A
//! deterministic proof needs a seam that delays a `resume` between its `load`
//! and its `settle`; there is none today, and a flaky proof of a real hazard is
//! worse than a named unforced one.
//!
//! ## 4. [`reap`](crate::worker::reap) — safe by **re-evaluation under READ COMMITTED**
//!
//! One `UPDATE … SET claimed_at = NULL WHERE claimed_at IS NOT NULL AND state =
//! 'RUNNING' AND claimed_at < now() - interval` (`worker/reaper.rs:39`). No
//! `SKIP LOCKED` and no explicit lock. Two reapers meeting on one row: the
//! second blocks on the first's row-level write lock, and when the first commits
//! Postgres **re-evaluates the `WHERE` clause against the committed version**
//! (READ COMMITTED's `EvalPlanQual`), where `claimed_at IS NULL` is now false —
//! so the row is dropped from the second's update rather than cleared twice.
//! Clearing an already-clear lease would be harmless anyway; what would **not**
//! be is the second reaper clearing a lease a *third* worker has since taken,
//! and the re-evaluation is exactly what stops that.
//!
//! The counts split between the two passes and only their union is meaningful,
//! which is why [`reap`](crate::worker::reap) returning a count is safe where
//! [`wake`](crate::worker::wake) returning ids
//! is necessary: a reclaimed lease is fungible.
//!
//! The predicate's *other* direction — a lease **inside** the timeout is left
//! alone, so a live worker mid-handler is not reclaimed out from under itself —
//! is `reap`'s own property and `tests/worker.rs`'s
//! `reap_reclaims_the_expired_lease_and_leaves_the_live_one` is its instrument.
//! This module's test re-asserts it **under two concurrent reapers**, because
//! that test runs one.
//!
//! Proved by `two_tickers_reclaim_an_expired_lease_once_and_leave_a_live_one`.
//!
//! # A fixed-interval ticker, NOT a drain loop
//!
//! [`worker::tick`](crate::worker::tick) returns **results committed**, not work
//! done: its doc says *"a tick that only woke something returns 0"*, and that is
//! deliberate. A caller that looped `while tick(..)? > 0` would spin for ever on
//! an execution whose retry it had just reopened — the wake commits, the count
//! is 0, and the loop would read that as "idle" only if the wake had done
//! nothing, which it did not.
//!
//! So this loop calls `tick` **once per interval** and reads the count for
//! logging only. The latency cost is real and is named rather than hidden: with
//! `batch` smaller than the runnable set, a burst drains at `batch` runs per
//! `tick_interval`. The lever is `batch`, not a drain.
//!
//! Pinned by two tests whose names say so:
//! `a_tick_that_only_woke_something_returns_zero_so_a_drain_loop_would_never_stop`
//! (the contract) and
//! `the_worker_ticks_on_a_fixed_interval_rather_than_draining_to_zero` (the
//! loop). Both die when the loop is rewritten as `while tick(..)? > 0`
//! (mutation C, 2 runs of 2, control 10/10).
//!
//! # What is NOT asserted here, named rather than left to be found
//!
//! **The REAPER's `MissedTickBehavior::Skip` has no test.** Deleting it
//! (mutation A2, 2 runs of 2) leaves all 10 tests green, where deleting the
//! *worker's* (mutation A1, 3 runs of 3) kills
//! `a_paused_ticker_does_not_fire_catch_up_ticks` every time. The callers were
//! enumerated before anything was concluded: three tests drive the reaper
//! ticker and not one of them holds it past a single one of its own intervals,
//! because [`reap`](crate::worker::reap) is one `UPDATE` with no await inside it
//! and the only way to make it slow is to lock the table out from under it.
//!
//! And even then the property would stay invisible, which is the honest half:
//! `reap` takes **no batch**, so it reclaims everything on each pass and N
//! catch-up passes leave the database in exactly the state one pass does. The
//! worker's `Skip` is observable *only* because `batch` bounds a tick's work.
//! This is a **load** property with no behavioural consequence — one `UPDATE`
//! per missed interval against a table with nothing to give — and killing its
//! mutant would mean counting calls from inside production code. It is kept for
//! the same reason `egret-automation`'s is, and it is recorded here as unproved
//! rather than reported as covered.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::logs::{spawn_pruner, spawn_writer, LogLimits, LogRepo, PruneConfig, WriterConfig};
use crate::worker::{Handlers, Registry};

/// What [`spawn`] needs to know, in this crate's own vocabulary.
///
/// Deliberately **not** `egret_config::WorkflowConfig`: `egret-config` sits
/// below this crate in the graph, and a leaf engine that could only be started
/// by a caller holding the binary's config type would be one no test could
/// start. `egret-bin` builds this from `config.workflow` at the call site, which
/// is the same shape `egret_automation::runner::spawn` takes (a bare
/// `Duration`, not a config struct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// **The gate, and it lives here rather than at the call site.**
    ///
    /// `egret-bin` could equally write `if config.workflow.worker_enabled {
    /// spawn(…) }`, and that is what `egret-automation`'s ticker does. It is
    /// here instead for one reason: a gate inside `main()` is a gate no test can
    /// reach, and this one is the difference between "this deployment runs
    /// workflows" and "this deployment silently does not". There is exactly one
    /// gate — the call site passes the flag and does not repeat the `if` — so
    /// the two cannot drift.
    ///
    /// It gates the claim loop and the reaper **only**; the scheduler has its
    /// own, [`scheduler_enabled`](Self::scheduler_enabled).
    ///
    /// Defaults to `false` at `egret_config::WorkflowConfig::worker_enabled`.
    pub enabled: bool,
    /// Between claim ticks.
    pub tick_interval: Duration,
    /// `tick`'s `n`: the most runs one tick may claim, and the most executions
    /// its waker may name.
    pub batch: usize,
    /// Between reaper passes.
    pub reaper_interval: Duration,
    /// How long a lease may go unreleased before a reaper pass takes it back.
    /// The claim loop renews the lease of the run it is executing every third
    /// of this (W5a), so only a lease whose worker stopped renewing expires.
    pub working_timeout: Duration,
    /// W5a: the budget of a script or container task that declares no
    /// `timeout:`. `egret_config::WorkflowConfig::container_default_timeout_s`;
    /// see [`DEFAULT_CONTAINER_TIMEOUT`](crate::worker::DEFAULT_CONTAINER_TIMEOUT).
    pub container_default_timeout: Duration,
    /// **The scheduler's own gate (W12), independent of [`enabled`](Self::enabled).**
    ///
    /// When true, [`spawn`] starts a third ticker that calls
    /// [`scheduler::fire_due`](crate::scheduler::fire_due), whatever `enabled`
    /// says. Defaults to `false` at
    /// `egret_config::WorkflowConfig::scheduler_enabled`.
    pub scheduler_enabled: bool,
    /// Between scheduler passes.
    pub trigger_interval: Duration,
    /// W11: each attempt's log cap, handed to the registry with the sink by
    /// [`spawn_with_logs`]. `egret_config::WorkflowConfig::{log_run_max_lines,
    /// log_run_max_bytes}`.
    pub log_limits: LogLimits,
    /// W11: stored log lines older than this many days are pruned hourly by
    /// [`spawn_with_logs`]; `0` keeps them for ever (no pruner is started).
    /// `egret_config::WorkflowConfig::log_retention_days`.
    pub log_retention_days: i32,
}

/// What [`spawn`] and [`spawn_with_logs`] started, to wait for at shutdown.
#[derive(Debug, Default)]
pub struct RuntimeHandle {
    claim_loop: Option<tokio::task::JoinHandle<()>>,
    log_writer: Option<tokio::task::JoinHandle<()>>,
    pruner: Option<tokio::task::JoinHandle<()>>,
    /// W9a: the execution-file pruner ([`crate::storage::prune`]).
    storage_pruner: Option<tokio::task::JoinHandle<()>>,
}

impl RuntimeHandle {
    /// Wait for the claim loop, then the log writer (which the claim loop's
    /// end releases, see [`spawn_with_logs`]), then the pruners. Unbounded if a
    /// tick is in flight; the caller bounds it.
    pub async fn join(self) {
        for h in [
            self.claim_loop,
            self.log_writer,
            self.pruner,
            self.storage_pruner,
        ]
        .into_iter()
        .flatten()
        {
            let _ = h.await;
        }
    }
}

/// [`spawn`], plus W11's durable log: the Postgres log writer, the retention
/// pruner, and `handlers` wired to write through the writer.
///
/// **Why it takes [`Handlers`] and not `Arc<dyn Registry>`.** The sink must be
/// in the registry before the registry is shared, and an `Arc<dyn Registry>`
/// can no longer be given one. The caller (`egret-bin`) still decides WHICH
/// handlers run; this function decides only where their lines go, so it cannot
/// forget to: [`Handlers::with_log_sink`] is called here, with
/// [`RuntimeConfig::log_limits`].
///
/// With `cfg.enabled` false this is [`spawn`]: no writer and no pruner,
/// because the worker is the only producer of lines (spec §4.3).
///
/// W9a: with the worker on and a store installed ([`crate::storage::current`]),
/// it also starts the execution-file pruner
/// ([`crate::storage::prune::spawn_storage_pruner`]) with the store's
/// `retention_days`; `0` starts none. It stops on `cancel`.
///
/// # Shutdown order, and what enforces it
///
/// The writer is **not** cancelled by `cancel`. It has its own token, and the
/// only thing that fires it is the **claim loop's task ending** (a drop guard
/// inside that task, so a panic fires it too). The claim loop checks `cancel`
/// only between ticks, so by the time it ends, any tick in flight -- and every
/// handler writing through the sink -- has returned. Then the writer stops
/// accepting, drains what is queued and exits within
/// [`WriterConfig::drain_within`](crate::logs::WriterConfig::drain_within).
/// A write that still arrives after that (nothing in this crate produces one)
/// is dropped with one `warn` (spec §11). The pruner stops on `cancel`.
/// [`RuntimeHandle::join`] waits for the claim loop, then the writer.
pub fn spawn_with_logs(
    pool: PgPool,
    handlers: Handlers,
    cfg: RuntimeConfig,
    cancel: CancellationToken,
) -> RuntimeHandle {
    if !cfg.enabled {
        return spawn(pool, Arc::new(handlers), cfg, cancel);
    }
    let repo = LogRepo::new(pool.clone());
    let writer_stop = CancellationToken::new();
    let (sink, writer) = spawn_writer(repo.clone(), WriterConfig::default(), writer_stop.clone());
    let pruner = spawn_pruner(
        repo,
        PruneConfig::new(cfg.log_retention_days),
        cancel.clone(),
    );
    let reg: Arc<dyn Registry> = Arc::new(handlers.with_log_sink(Arc::new(sink), cfg.log_limits));

    // W9a (spec §10): the execution-file pruner, on its own task beside the
    // log pruner, when this process has a store. Its retention is the
    // store's own (`FileStore::retention_days`, `workflow.storage_retention_days`)
    // -- the number `Unreadable::Pruned` names -- so the two cannot differ.
    let storage_pruner = crate::storage::current().and_then(|fs| {
        let cfg = crate::storage::prune::StoragePruneConfig::new(fs.retention_days);
        crate::storage::prune::spawn_storage_pruner(pool.clone(), fs, cfg, cancel.clone())
    });

    let mut handle = spawn_inner(pool, reg, cfg, cancel, Some(writer_stop));
    handle.log_writer = Some(writer);
    handle.pruner = pruner;
    handle.storage_pruner = storage_pruner;
    handle
}

/// Start the claim ticker and the reaper ticker, and the scheduler ticker;
/// returns immediately.
///
/// **Two gates, independent of each other.** With `cfg.enabled` false neither
/// the claim loop nor the reaper is spawned, so the registry is never called
/// and neither issues a statement. With `cfg.scheduler_enabled` false the
/// scheduler ticker is not spawned. Both are the shipping default
/// (`egret_config::WorkflowConfig::{worker_enabled, scheduler_enabled}`).
/// Scheduler on with the worker off is allowed and warned about: executions
/// are created and nothing runs them.
///
/// Every ticker stops on `cancel`. One per replica and no leader election; see
/// this module's doc, and [`scheduler`](crate::scheduler)'s, for why that is
/// safe **per statement** rather than once.
///
/// **No durable log**: every attempt's lines go to whatever `reg` answers
/// [`Registry::log_sink`] with (`tracing`, for [`Handlers`] without a sink).
/// Production calls [`spawn_with_logs`]; this is for registries a test builds.
pub fn spawn(
    pool: PgPool,
    reg: Arc<dyn Registry>,
    cfg: RuntimeConfig,
    cancel: CancellationToken,
) -> RuntimeHandle {
    spawn_inner(pool, reg, cfg, cancel, None)
}

/// [`spawn`]'s body; `release_on_stop` is fired when the claim loop ends.
fn spawn_inner(
    pool: PgPool,
    reg: Arc<dyn Registry>,
    cfg: RuntimeConfig,
    cancel: CancellationToken,
    release_on_stop: Option<CancellationToken>,
) -> RuntimeHandle {
    let mut handle = RuntimeHandle::default();
    if cfg.enabled {
        handle.claim_loop = Some(spawn_worker(
            pool.clone(),
            reg,
            cfg,
            cancel.clone(),
            release_on_stop,
        ));
    } else {
        tracing::info!("workflow worker disabled by config; no claim loop, no reaper");
    }

    // ── Ticker 3: the schedule-trigger loop (W12), behind its OWN gate ──────
    if cfg.scheduler_enabled {
        if !cfg.enabled {
            tracing::warn!(
                "workflow scheduler on with the worker off: executions will be created \
                 and nothing will run them"
            );
        }
        spawn_scheduler(pool, cfg.trigger_interval, cancel);
    } else {
        tracing::info!("workflow scheduler disabled by config; no trigger loop");
    }
    handle
}

/// The third ticker: one [`fire_due`](crate::scheduler::fire_due) pass per
/// `every`. A pass fires every due trigger once and returns, so this is a
/// fixed-interval loop for the same reason the claim loop is; a trigger that is
/// behind catches up one tick per pass.
///
/// After each `fire_due`, one
/// [`process_events_with`](crate::flow_trigger::process_events_with) pass
/// (W13a): the due Flow-trigger events in the outbox, each once, up to
/// [`PassLimits`](crate::flow_trigger::PassLimits)' cap (500 events or about
/// 1 s, final review I3) so a backlog cannot hold `fire_due` or shutdown back;
/// the rest waits for the next tick. Then one
/// [`sweep_expired_windows`](crate::flow_trigger::sweep_expired_windows)
/// (W13b). A chain therefore advances at least one
/// hop per tick, and more when a fired execution's first settle already writes
/// an event its listeners hear (spec §4.4 points 4-5).
///
/// Before the first pass it runs [`reconcile`](crate::scheduler::reconcile)
/// once, so a flow saved before migration 0156 gets its trigger rows (final
/// review I1). A reconciliation that cannot even list the flows is logged and
/// the ticker starts anyway: every flow saved since 0156 already has its rows.
fn spawn_scheduler(pool: PgPool, every: Duration, cancel: CancellationToken) {
    tracing::info!(
        trigger_interval_secs = every.as_secs(),
        "workflow scheduler started"
    );
    tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("workflow scheduler stopped");
                return;
            }
            r = crate::scheduler::reconcile(&pool) => match r {
                Ok(r) => tracing::info!(
                    synced = r.synced,
                    failed = r.failed,
                    "workflow schedules reconciled"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "workflow schedule reconciliation failed; the ticker starts anyway"
                ),
            },
        }
        // A capped pass (final review I3), its kept events due again one tick
        // later (F2), and the shutdown token checked between its events.
        let limits = crate::flow_trigger::PassLimits {
            retry_backoff: every,
            ..Default::default()
        };
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => {
                    match crate::scheduler::fire_due(&pool).await {
                        Ok(r) if r.fired > 0 || r.errored > 0 => tracing::info!(
                            fired = r.fired,
                            skipped = r.skipped,
                            errored = r.errored,
                            frozen = r.frozen,
                            "workflow schedule pass"
                        ),
                        Ok(r) if r.skipped > 0 || r.frozen > 0 => tracing::debug!(
                            skipped = r.skipped,
                            frozen = r.frozen,
                            "workflow schedule pass"
                        ),
                        Ok(_) => {}
                        // A failed pass must not end the loop: its trigger's
                        // transaction rolled back, so the next pass sees the
                        // row exactly as this one found it and retries.
                        Err(e) => tracing::warn!(error = %e, "workflow schedule pass failed"),
                    }
                    // W13a: the Flow-trigger outbox, after the schedules and
                    // behind the same switch (spec §4.4). With the scheduler
                    // off the outbox grows; turned on, this drains it.
                    match crate::flow_trigger::process_events_with(&pool, &limits, &cancel).await {
                        Ok(r) if r.fired > 0 || r.born_failed > 0 => tracing::info!(
                            events = r.events,
                            fired = r.fired,
                            born_failed = r.born_failed,
                            accumulated = r.accumulated,
                            contended = r.contended,
                            refused = r.refused,
                            errored = r.errored,
                            "workflow flow-trigger pass"
                        ),
                        Ok(r) if r.events > 0 => tracing::debug!(
                            events = r.events,
                            accumulated = r.accumulated,
                            contended = r.contended,
                            refused = r.refused,
                            errored = r.errored,
                            "workflow flow-trigger pass"
                        ),
                        Ok(_) => {}
                        // Never ends the loop: the failed event's transaction
                        // rolled back, so the next pass claims it again.
                        Err(e) => tracing::warn!(error = %e, "workflow flow-trigger pass failed"),
                    }
                    // W13b: the expired `dependsOn` windows, once per pass,
                    // after the events (spec §5.3 step 6). A failure is
                    // retried by the next pass; an expired window left
                    // behind is replaced by its next matching event anyway.
                    match crate::flow_trigger::sweep_expired_windows(&pool).await {
                        Ok(0) => {}
                        Ok(n) => tracing::debug!(swept = n, "workflow flow-trigger windows swept"),
                        Err(e) => tracing::warn!(error = %e, "workflow flow-trigger window sweep failed"),
                    }
                }
            }
        }
        tracing::info!("workflow scheduler stopped");
    });
}

/// The claim ticker and the reaper ticker; `cfg.enabled` is checked by the
/// caller.
fn spawn_worker(
    pool: PgPool,
    reg: Arc<dyn Registry>,
    cfg: RuntimeConfig,
    cancel: CancellationToken,
    release_on_stop: Option<CancellationToken>,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        tick_interval_secs = cfg.tick_interval.as_secs(),
        batch = cfg.batch,
        reaper_interval_secs = cfg.reaper_interval.as_secs(),
        working_timeout_s = cfg.working_timeout.as_secs(),
        "workflow worker started"
    );

    let reg_for_sweep = reg.clone();
    // ── Ticker 1: the claim loop ────────────────────────────────────────────
    let timing = crate::worker::WorkerTiming {
        working_timeout: cfg.working_timeout,
        container_default_timeout: cfg.container_default_timeout,
    };
    let claim_loop = {
        let pool = pool.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            // W11: the log writer's stop signal, fired when this task ends --
            // after its loop has broken, so after any tick in flight has
            // returned -- or unwinds. See `spawn_with_logs`.
            let _release = release_on_stop.map(CancellationToken::drop_guard);
            let mut tick = tokio::time::interval(cfg.tick_interval);
            // A replica that pauses (or a laptop that sleeps) must not then fire
            // a burst of catch-up ticks: the runnable runs are still there and
            // the next tick claims them. `Burst` -- the default -- would issue
            // one round trip per missed interval against a database that has
            // nothing new to give.
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tick.tick() => {
                        // NOT `while n > 0`. See this module's doc: `tick`
                        // returns results COMMITTED, and a tick that only woke
                        // something returns 0 while having changed the database.
                        match crate::worker::tick_with(&pool, cfg.batch, &*reg, timing).await {
                            Ok(0) => {}
                            Ok(n) => tracing::debug!(committed = n, "workflow tick"),
                            // A failing tick must not end the loop. The next one
                            // re-reads the database, and the run this one could
                            // not settle is still there -- either claimable or
                            // leased for the reaper.
                            Err(e) => tracing::warn!(error = %e, "workflow tick failed"),
                        }
                    }
                }
            }
            tracing::info!("workflow claim loop stopped");
        })
    };

    // ── Ticker 2: the lease reaper, and (W5a) the orphan sweep ───────────────
    //
    // The sweep rides the reaper's tick (spec §5, *Orphans*): it removes the
    // containers or pods of runs that are TERMINAL (both halves, W5a Task 4
    // fix round 1), so it can never touch a live run's work, and a pass that
    // fails is logged and retried on the next tick like a failed reap.
    let sweep = reg_for_sweep.orphan_sweep();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.reaper_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => {
                    reaper_pass(&pool, cfg.working_timeout, sweep.as_deref()).await;
                }
            }
        }
        tracing::info!("workflow reaper stopped");
    });
    claim_loop
}

/// One reaper tick: reclaim expired leases, then sweep the orphans of ended
/// runs when a container runner is configured. Each half logs its own
/// failure; neither stops the loop.
pub async fn reaper_pass(
    pool: &PgPool,
    working_timeout: Duration,
    sweep: Option<&dyn crate::worker::commands::OrphanSweep>,
) {
    match crate::worker::reap(pool, working_timeout).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(reclaimed = n, "workflow leases reclaimed"),
        Err(e) => tracing::warn!(error = %e, "workflow reaper failed"),
    }
    if let Some(sweep) = sweep {
        match sweep.sweep_orphans(pool).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "task containers of ended runs removed"),
            Err(e) => tracing::warn!(error = %e, "workflow orphan sweep failed"),
        }
    }
}

/// **The worker's registry for a deployment (W5a Task 5)**: `egret-bin`'s one
/// call, so the decision is here where a test can reach it.
///
/// * `container_runner = none`: [`Handlers::with_core_tasks`] plus the core
///   storage tasks that need the pool (`Delete`, `PurgeCurrentExecutionFiles`,
///   [`crate::worker::handlers::storage::register`], W9a Task 11) -- no
///   script handler, no orphan sweep -- and `None` for the publish gate's
///   default runner.
/// * `docker` / `kubernetes`: the same, plus the four script types and their
///   two aliases over ONE runner built here ([`crate::worker::handlers::script::register`]),
///   with the run-time gate reading `workspace_features` from `pool` and the
///   allowlist from `plugin_configurations`; the runner is also the registry's
///   orphan sweep. Returns the runner kind, which publish uses for a task with
///   no `taskRunner:`.
///
/// `Err` for a `plugin_configurations` that fails
/// `WorkflowConfig::validate_plugin_configurations` (W5b), checked first;
/// for a Kubernetes configuration egret refuses (`run_as_user <= 0`) or
/// a cluster client that cannot be built: a deployment that asked for
/// containers and cannot have them must not start as if it had none.
///
/// W5b: with a runner, every catalog class is registered as `kestra.<FQCN>`
/// ([`crate::worker::handlers::kestra`]) over the SAME runner `Arc`, and
/// `secrets` is where their `secret()` reads (egret-bin passes its
/// `WorkspaceSecretSource`). `workflow.plugin_host.image` is then required
/// (`WorkflowConfig::validate_plugin_host`), a boot error.
pub async fn handlers_for(
    cfg: &egret_config::WorkflowConfig,
    pool: PgPool,
    secrets: Arc<dyn crate::runners::secrets::SecretSource>,
) -> egret_core::Result<(Handlers, Option<crate::scripts::RunnerKind>)> {
    use crate::scripts::RunnerKind;
    use crate::worker::commands::{DockerRunner, OrphanSweep, TaskRunner};
    use crate::worker::handlers::script::{register, PgFeatureState, ScriptRuntime};

    // W5b: the boot check on `plugin_configurations`, first -- before
    // `container_runner = none` returns below, so a misplaced
    // `allowed-types`/`allowed-images` refuses to start on every deployment.
    cfg.validate_plugin_configurations()
        .map_err(egret_core::Error::InvalidInput)?;
    // W5b: the plugin host's image (required with a runner) and heap.
    cfg.validate_plugin_host()
        .map_err(egret_core::Error::InvalidInput)?;
    let mut handlers = Handlers::with_core_tasks(crate::storage::current());
    // W9a Task 11: `Delete` and `PurgeCurrentExecutionFiles` need the pool,
    // so they are registered here, for every deployment, with or without a
    // container runner.
    crate::worker::handlers::storage::register(
        &mut handlers,
        pool.clone(),
        crate::storage::current(),
    );
    // W9b Task 6: `UploadFiles` and `DownloadFiles`, which need the pool too.
    crate::worker::handlers::namespace::register(
        &mut handlers,
        pool.clone(),
        crate::storage::current(),
    );
    let (kind, runner, sweep): (RunnerKind, Arc<dyn TaskRunner>, Arc<dyn OrphanSweep>) = match cfg
        .container_runner
    {
        egret_config::ContainerRunner::None => return Ok((handlers, None)),
        egret_config::ContainerRunner::Docker => {
            let mut r = DockerRunner::new().with_logs(LogRepo::new(pool.clone()));
            // W9a Task 5: stored inputs stream from the installed store.
            if let Some(files) = crate::storage::current() {
                r = r.with_files(files);
            }
            let r = Arc::new(r);
            (RunnerKind::Docker, r.clone(), r)
        }
        egret_config::ContainerRunner::Kubernetes => {
            let pod = pod_config(&cfg.kubernetes);
            pod.validate()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e}")))?;
            let client = kube::Client::try_default().await.map_err(|e| {
                egret_core::Error::Internal(anyhow::anyhow!(
                    "workflow.container_runner is kubernetes but no cluster client could be \
                         built: {e}"
                ))
            })?;
            let mut r =
                crate::worker::k8s::KubernetesRunner::new(client, pod, LogRepo::new(pool.clone()));
            if let Some(files) = crate::storage::current() {
                r = r.with_files(files);
            }
            let r = Arc::new(r);
            (RunnerKind::Kubernetes, r.clone(), r)
        }
    };
    let features: Arc<dyn crate::worker::handlers::script::FeatureState> =
        Arc::new(PgFeatureState::new(pool.clone()));
    register(
        &mut handlers,
        Arc::new(ScriptRuntime {
            runner_kind: kind,
            runner: runner.clone(),
            plugin_configurations: cfg.plugin_configurations.clone(),
            features: features.clone(),
            files: crate::storage::current(),
            // W9b Task 5: namespace files are staged on this pool.
            pool: Some(pool),
        }),
    );
    crate::worker::handlers::kestra::register(
        &mut handlers,
        Arc::new(crate::worker::handlers::kestra::KestraRuntime {
            runner,
            runner_kind: kind,
            features,
            plugin_configurations: cfg.plugin_configurations.clone(),
            host: cfg.plugin_host.clone(),
            secrets,
            // W9a Task 9: the plugin storage bridge stages from, and collects
            // into, the installed store.
            files: crate::storage::current(),
        }),
    );
    Ok((handlers.with_orphan_sweep(sweep), Some(kind)))
}

/// `workflow.kubernetes.*` in the runner's own vocabulary.
pub fn pod_config(k: &egret_config::WorkflowKubernetesConfig) -> crate::worker::k8s::PodConfig {
    fn side(a: &egret_config::TaskResourceAmounts) -> crate::scripts::ResourceAmounts {
        crate::scripts::ResourceAmounts {
            cpu: a.cpu.clone(),
            memory: a.memory.clone(),
            ephemeral_storage: a.ephemeral_storage.clone(),
        }
    }
    fn both(r: &egret_config::TaskResources) -> crate::scripts::Resources {
        crate::scripts::Resources {
            request: side(&r.request),
            limit: side(&r.limit),
        }
    }
    crate::worker::k8s::PodConfig {
        namespace: k.namespace.clone(),
        run_as_user: k.run_as_user,
        helper_image: k.helper_image.clone(),
        default_resources: both(&k.default_resources),
        max_resources: both(&k.max_resources),
        image_pull_secrets: k.image_pull_secrets.clone(),
    }
}
