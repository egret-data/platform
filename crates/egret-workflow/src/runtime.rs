//! The two background loops that make this engine run, and a safety statement
//! per statement they issue.
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
//! lock of any kind. Each gets its own statement below, and each statement is
//! proved by a test in `tests/runtime.rs` that runs **two tickers against one
//! database** — because a guard nobody has seen fail is a guard nobody has seen.
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
//! ## 3. [`wake`](crate::worker::wake)/[`resume`](crate::worker::resume) — safe by **idempotence under the execution's row lock**
//!
//! [`wake`] hands each id [`due_retries`] named to [`resume`], and `resume`'s
//! `load` takes `SELECT … FROM workflow.executions WHERE id = $1 FOR UPDATE`
//! (`worker/claim.rs:1521`). So the second replica's `resume` **blocks** behind
//! the first's — it does not skip, and it must not: skipping would be a woken
//! execution dropped. When it acquires the lock it re-reads the state the first
//! one committed, and `advance()` is pure over `(plan, state, now)`, so it sees
//! a run that is no longer `RETRYING` and produces **no second reopen**. The
//! reopened run's attempt is opened once.
//!
//! **The honest residue, stated rather than rounded to "no-op":** the loser's
//! settle still writes a new `executions.seq` even though it produces no
//! commands — [`due_retries`]'s own doc names that cost for the same statement.
//! A redundant `seq` bump is invisible to every consumer; a second attempt would
//! not be, and that is the invariant the test asserts.
//!
//! Proved by `two_tickers_reopen_a_due_retry_exactly_once`, which asserts on the
//! run's **attempt count** (2, not 3) and on the handler's call log for that run
//! — the two places a double reopen is visible.
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
//! which is why [`reap`] returning a count is safe where [`wake`] returning ids
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
//! loop).

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::worker::Registry;

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
    pub working_timeout: Duration,
}

/// Start the claim ticker and the reaper ticker; returns immediately.
///
/// **A no-op when `cfg.enabled` is false** — nothing is spawned, so the registry
/// is never called and no statement is ever issued. That is the shipping default
/// (`egret_config::WorkflowConfig::worker_enabled`).
///
/// Both tickers stop on `cancel`. One per replica and no leader election; see
/// this module's doc for why that is safe **per statement** rather than once.
pub fn spawn(pool: PgPool, reg: Arc<dyn Registry>, cfg: RuntimeConfig, cancel: CancellationToken) {
    if !cfg.enabled {
        tracing::info!("workflow worker disabled by config; no claim loop, no reaper");
        return;
    }

    tracing::info!(
        tick_interval_secs = cfg.tick_interval.as_secs(),
        batch = cfg.batch,
        reaper_interval_secs = cfg.reaper_interval.as_secs(),
        working_timeout_s = cfg.working_timeout.as_secs(),
        "workflow worker started"
    );

    // ── Ticker 1: the claim loop ────────────────────────────────────────────
    {
        let pool = pool.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
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
                        match crate::worker::tick(&pool, cfg.batch, &*reg).await {
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
        });
    }

    // ── Ticker 2: the lease reaper ──────────────────────────────────────────
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.reaper_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => {
                    match crate::worker::reap(&pool, cfg.working_timeout).await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(reclaimed = n, "workflow leases reclaimed"),
                        Err(e) => tracing::warn!(error = %e, "workflow reaper failed"),
                    }
                }
            }
        }
        tracing::info!("workflow reaper stopped");
    });
}
