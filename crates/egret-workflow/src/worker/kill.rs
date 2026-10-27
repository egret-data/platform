//! **The kill transaction** -- workflow kill spec
//! `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md` §3.2. The
//! route (`routes::kill_execution`) calls [`kill_execution`]; an `Exit` with
//! `state: KILLED` reaches the cascade half through `claim`'s settle
//! ([`cascade`]).
//!
//! # What one kill writes
//!
//! For the execution, and with `cascade` for every live descendant:
//!
//! 1. a running leaf (`RUNNING` or `SUBMITTED`) becomes `KILLING` with its
//!    lease untouched, **leased or not** (final review I1, 2026-10-18): a
//!    holder stops the handler and writes `KILLED` (spec §4.2); with no lease
//!    -- waiting for its first claim, or reaped while its container may still
//!    run -- the kill-only claim cancels it by run and attempt and writes
//!    `KILLED` (spec §4.3);
//! 2. every other live leaf -- `CREATED`, `RETRYING`, `PAUSED`, and every
//!    `Subflow` run -- becomes `KILLED`, with its `ended_at` and its open
//!    attempt closed `KILLED`;
//! 3. flowables, a `Pause` and a `WorkingDirectory` block among them, are
//!    left to `executor::kill::advance_killing`;
//! 4. the execution becomes `KILLING`, and the settle that writes all of this
//!    runs `advance()`, so an execution with nothing left running is `KILLED`
//!    when the transaction commits.
//!
//! The commands are `executor::kill::kill_leaves`; they are written the way
//! every other state change is, `load` (the execution's lock), `apply`, then
//! `claim::settle`, which is also how `claim::commit_kill_with` writes a
//! worker's `KILLED`: `persist` stamps the run's `ended_at`,
//! `stamps::stamp_attempt_ends` the attempt's, and nothing here writes a
//! state column by hand.
//!
//! # A terminal execution whose `afterExecution:` still runs (Q3)
//!
//! End hooks #2b, the user's answer to Q3 (2026-10-27), a security
//! divergence from Kestra, which answers 409 and force-executes the list: a
//! kill of a terminal execution with a live run of its `afterExecution:` is
//! accepted ([`KillOutcome::AfterExecution`]). The phase's live runs are
//! killed by `executor::kill::kill_phase` (leaves as above, its flowables
//! `KILLING` at once), the cascade reaches only the children of the phase's
//! own runs, and the execution's state, history and `ended_at` are not
//! written: `advance` then closes the phase (`executor::kill::close_phase`)
//! and creates no further after-task. With nothing of the phase live, the
//! 409 stands.
//!
//! # The run rows are locked before they are written
//!
//! The claim takes a row with `FOR UPDATE SKIP LOCKED` and no execution lock,
//! so the execution's lock alone does not order the kill against a claim.
//! Every run row of the execution is therefore locked `FOR UPDATE` first: a
//! claim that got there first has committed by the time the lock is held,
//! and its holder finds the run `KILLING`; one that comes after skips the
//! locked row, and finds it terminal, or `KILLING` -- which it takes only as a
//! kill-only claim -- once this commits. (Until final review I1 this lock also
//! read the lease, to write an unleased running run `KILLED`; no lease is
//! read now.)
//!
//! # Which executions are children
//!
//! A **loop iteration** (`parent_execution_id` set and `parent_run_ix` set)
//! and a **`Subflow` child** (`trigger_execution_id` set and `trigger_run_ix`
//! set, migration 0138), recursively. Not a replay of the execution being
//! killed, which has `parent_execution_id` and no `parent_run_ix`, and not an
//! execution a Flow trigger fired, which has `trigger_execution_id` and no
//! `trigger_run_ix`.
//!
//! **A replay of a descendant IS one** (final review M2, 2026-10-18): a
//! subflow child or loop iteration retried with `CREATE_NEW_EXECUTION` ends
//! `RETRIED` and continues in a replacement linked the replay's way
//! (`worker::replay`'s table). That replacement is the child's work going on,
//! so the cascade follows the replay link from depth 1 down, never from the
//! root. (A loop iteration is not replayable today, `replay::spawn_due`; the
//! arm does not depend on that.)
//!
//! **egret's `Subflow` child is not found through `execution_trigger`**:
//! `subflow::spawn_one` writes the two link columns and leaves
//! `execution_trigger` NULL (that column is the webhook's and the
//! scheduler's), so the plan's `execution_trigger->>'type' = '…Subflow'`
//! would match no row. The columns are the same fact, and the ones the
//! completion path reads (`subflow::trigger_of`).
//!
//! # Lock order: the execution, then its descendants by depth, then id
//!
//! The execution is locked first, by `load`. The descendants are read with
//! one recursive CTE over both links and locked in `(depth, id)` order, each
//! by its own `load`. Depth first, not id alone, is what makes two kills
//! agree: the links form a tree, so in a kill of an ancestor `P` every
//! descendant of a child `C` is deeper than `C`, and is locked after it. A
//! kill of `C` also locks `C` first. Whichever takes `C` first therefore goes
//! on into `C`'s subtree while the other waits on `C` holding nothing in it,
//! and below `C` both use the same order (depths differ by a constant). A
//! pure id order would let a kill of `P` hold a descendant of `C` whose id is
//! lower than `C`'s while waiting for `C`.
//!
//! The CTE is read before the descendants are locked, so an execution not yet
//! locked can still spawn a child in between. The CTE is read again once
//! every execution it named is locked, and again, until it names nothing new;
//! a locked execution under `KILLING` spawns nothing (`advance_killing`).
//!
//! **One deadlock this order cannot remove**: a child that ends inside a
//! worker's transaction reports to its parent (`spawn::on_execution_terminal`,
//! `subflow::on_child_terminal`), which locks child-then-parent, the reverse
//! of a kill. Postgres detects it and aborts one side; [`kill_execution`]
//! runs its transaction again when it is the one aborted.
//!
//! # An execution that cannot be loaded is closed by force (debt D4)
//!
//! When `claim::load` fails for a reason in the row's own content
//! (`claim::row_content_fault`), the execution -- the one named, or one
//! descendant of a cascade -- is closed by `worker::force_close` instead of
//! failing the kill: retrying cannot change the answer, and a kill is the
//! operator's way out. [`kill_execution_reporting`] says whether that
//! happened, for the route's audit. Any other error is returned as before.

use std::collections::HashSet;

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::claim::{self, ExecutionCtx, Frame, Job, Stack};
use super::force_close;
use super::from_wire;
use super::txn::{transaction, InTxn, Txn};
use crate::executor::{apply, ExecutionState, RunState};

/// What [`kill_execution`] found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    /// A leaf of the execution is being stopped -- by the worker that holds
    /// it, or by the kill-only claim that will take it: the execution is
    /// `KILLING` until then.
    Killing,
    /// Nothing was running: the execution is `KILLED`.
    Killed,
    /// The execution was already terminal under its lock, in this state, and
    /// no run of its `afterExecution:` was live (spec D3: refused, nothing
    /// written).
    Terminated(RunState),
    /// The execution had ended, in `state`, and its `afterExecution:` was
    /// still running (end hooks #2b, the user's answer to Q3, 2026-10-27):
    /// the phase's live runs are killed and the execution's own state,
    /// history and `ended_at` are left as they were. `stopped` is true when
    /// no run of the phase is live once the kill has committed; otherwise a
    /// running task is still to be stopped by a worker.
    AfterExecution { state: RunState, stopped: bool },
}

/// How many times a kill that Postgres chose as a deadlock's victim is run
/// again. See the module doc's last paragraph.
const DEADLOCK_RETRIES: usize = 3;

/// **Kill `execution`** in `workspace`, and with `cascade` every live loop
/// and `Subflow` descendant, in one transaction (spec §3.2). `NotFound` for
/// an execution that is not in `workspace`.
pub async fn kill_execution(
    pool: &PgPool,
    workspace: Uuid,
    execution: Uuid,
    cascade: bool,
) -> Result<KillOutcome> {
    Ok(
        kill_execution_reporting(pool, workspace, execution, cascade)
            .await?
            .0,
    )
}

/// [`kill_execution`], and whether this transaction closed the execution or
/// one of its descendants by force (module doc, "An execution that cannot be
/// loaded"): the route audits it.
pub async fn kill_execution_reporting(
    pool: &PgPool,
    workspace: Uuid,
    execution: Uuid,
    cascade: bool,
) -> Result<(KillOutcome, bool)> {
    let mut retries = 0;
    loop {
        let killed = transaction!(pool, |at| {
            kill_in(at, workspace, execution, cascade).await
        });
        match killed {
            Err(e) if retries < DEADLOCK_RETRIES && is_deadlock(&e) => {
                retries += 1;
                tracing::info!(%execution, "a kill was a deadlock's victim; running it again");
            }
            other => return other,
        }
    }
}

async fn kill_in(
    at: &mut Txn,
    workspace: Uuid,
    execution: Uuid,
    cascade: bool,
) -> Result<(KillOutcome, bool)> {
    let (conn, gate) = at.parts();
    let mut forced = false;
    let mut reported = Vec::new();
    // The phase kill (Q3): the execution's terminal state, and the runs of
    // its `afterExecution:`, which bound the cascade and the outcome.
    let mut phase: Option<(RunState, Vec<i32>)> = None;
    match claim::load(conn, execution, Stack::Fresh(gate.clone())).await {
        Ok((ctx, state, now)) => {
            if ctx.workspace_id != workspace {
                return Err(Error::NotFound("execution".into()));
            }
            if let Some(&last) = state.history.last().filter(|s| s.is_terminal()) {
                // **A kill stops `afterExecution:`** (end hooks #2b, the user's
                // answer to Q3, 2026-10-27; a security divergence from Kestra,
                // which answers 409 and force-executes the list): a tenant must
                // not be able to write a task that no one can kill. Only the
                // phase's runs are killed; the execution keeps the state,
                // history and `ended_at` it ended with. With nothing of the
                // phase live, the refusal is as before.
                if !crate::executor::after_execution_live(&ctx.plan, &state) {
                    return Ok((KillOutcome::Terminated(last), false));
                }
                let runs = (0..state.runs.len())
                    .filter(|&ix| crate::executor::after_execution::in_phase(&ctx.plan, &state, ix))
                    .map(|ix| i32::try_from(ix).expect("a run index fits the column"))
                    .collect();
                phase = Some((last, runs));
                let (before, state) =
                    kill_loaded(conn, &ctx, state, crate::executor::kill::kill_phase).await?;
                claim::settle(conn, &ctx, &before, state, now, None).await?;
            } else {
                let (before, state) =
                    kill_loaded(conn, &ctx, state, crate::executor::kill::kill_leaves).await?;
                claim::settle(conn, &ctx, &before, state, now, None).await?;
            }
        }
        Err(e) if claim::row_content_fault(&e) => {
            // Debt D4. The row exists (the fault is in its content), and the
            // two checks the loaded arm makes are made here, off the row
            // `load` has already locked.
            let (ws, state): (Uuid, String) =
                sqlx::query_as("SELECT workspace_id, state FROM workflow.executions WHERE id = $1")
                    .bind(execution)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(db_err)?;
            if ws != workspace {
                return Err(Error::NotFound("execution".into()));
            }
            if let Some(last) = from_wire(&state).filter(|s| s.is_terminal()) {
                return Ok((KillOutcome::Terminated(last), false));
            }
            let why = force_close::reason(&e);
            let closed = force_close::close(
                conn,
                execution,
                &why,
                None,
                Stack::Fresh(gate.clone()).caller(),
            )
            .await?;
            tracing::warn!(
                %execution,
                error = %why,
                outcome = closed.state.wire(),
                "a kill could not load this execution, so it was closed by force"
            );
            forced = true;
            reported = closed.reported;
        }
        Err(e) => return Err(e),
    }
    if cascade {
        // Each one settled on its own, in lock order, after its parent's
        // settle: a child that ends `KILLED` reports to a parent whose run is
        // already terminal, which is a no-op.
        // A phase kill reaches only the children of the phase's own runs: a
        // `wait: false` child of the flow's `tasks:` is not the phase's work.
        let found = descendants(
            conn,
            execution,
            phase.as_ref().map(|(_, runs)| runs.as_slice()),
            || Stack::Fresh(gate.clone()),
        )
        .await?;
        forced |= found.forced;
        for job in found.jobs {
            claim::run_job(conn, job).await?;
        }
    }
    // The root's own report last: its parent is outside the tree.
    for job in reported {
        claim::run_job(conn, job).await?;
    }
    if let Some((state, runs)) = phase {
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow.task_runs \
              WHERE execution_id = $1 AND run_ix = ANY($2) \
                AND NOT workflow.state_is_terminal(state)",
        )
        .bind(execution)
        .bind(&runs)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
        return Ok((
            KillOutcome::AfterExecution {
                state,
                stopped: live == 0,
            },
            forced,
        ));
    }
    let after: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(execution)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
    Ok((
        if from_wire(&after) == Some(RunState::Killed) {
            KillOutcome::Killed
        } else {
            KillOutcome::Killing
        },
        forced,
    ))
}

/// **The kill of an `Exit` with `state: KILLED`** (kill-and-exit Task 6,
/// decision 5): every live descendant of `ctx`'s execution, killed as a
/// cascade would, as settles for the walk to run inside the `Exit`'s own.
/// Kestra's kill event for an `Exit` cascades; the `Exit` itself only ends
/// its own execution's runs (`executor::exit`), so a `Subflow` child it
/// `KILLED` the run of would otherwise run on.
pub(super) async fn cascade(conn: &mut PgConnection, ctx: &ExecutionCtx) -> Result<Vec<Job>> {
    Ok(
        descendants(conn, ctx.id, None, || Stack::Nested(ctx.caller()))
            .await?
            .jobs,
    )
}

/// What [`descendants`] found.
struct Descendants {
    /// The settles not yet run, in lock order, then the reports of the
    /// descendants closed by force.
    jobs: Vec<Job>,
    /// Whether any descendant was closed by force.
    forced: bool,
}

/// Every live descendant of `root`, locked in the module doc's order and with
/// the kill applied, as settles not yet run.
///
/// **A descendant that cannot be loaded is closed by force** (debt D4) and
/// the walk goes on. Its report to its parent comes after every settle: the
/// parent may be a descendant whose kill is one of those settles, and the
/// report must read the parent as that settle leaves it
/// (`worker::force_close`'s module doc).
///
/// `only` (a phase kill, Q3) restricts depth 1 to the children of those runs
/// of `root`; `None` takes every child.
async fn descendants(
    conn: &mut PgConnection,
    root: Uuid,
    only: Option<&[i32]>,
    stack: impl Fn() -> Stack,
) -> Result<Descendants> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    let mut reported = Vec::new();
    let mut forced = false;
    loop {
        let found: Vec<Uuid> = sqlx::query_scalar(DESCENDANTS)
            .bind(root)
            .bind(only)
            .fetch_all(&mut *conn)
            .await
            .map_err(db_err)?;
        let new: Vec<Uuid> = found.into_iter().filter(|id| seen.insert(*id)).collect();
        if new.is_empty() {
            jobs.append(&mut reported);
            return Ok(Descendants { jobs, forced });
        }
        for id in new {
            let (ctx, state, now) = match claim::load(conn, id, stack()).await {
                Ok(loaded) => loaded,
                Err(e) if claim::row_content_fault(&e) => {
                    let why = force_close::reason(&e);
                    let closed = force_close::close(conn, id, &why, None, stack().caller()).await?;
                    tracing::warn!(
                        execution = %id,
                        root = %root,
                        error = %why,
                        outcome = closed.state.wire(),
                        "a cascading kill could not load this descendant, so it was closed by force"
                    );
                    forced = true;
                    reported.extend(closed.reported);
                    continue;
                }
                Err(e) => return Err(e),
            };
            // It ended between the read and the lock.
            if state.history.last().is_some_and(|s| s.is_terminal()) {
                continue;
            }
            let (before, state) =
                kill_loaded(conn, &ctx, state, crate::executor::kill::kill_leaves).await?;
            jobs.push(Job::Settle(Box::new(Frame::loaded(
                ctx, before, state, now,
            ))));
        }
    }
}

/// The live descendants of `$1`, through both links, in lock order. Through
/// terminal executions too: a `wait: false` child, or an iteration of a loop
/// that ended on its first failure, can be terminal over a live child.
///
/// **And through a descendant's replay** (final review M2): the third arm
/// follows `parent_execution_id` with no `parent_run_ix` -- a
/// `CREATE_NEW_EXECUTION` replacement (`worker::replay`) -- from depth 1 down
/// only. A subflow child that ended `RETRIED` continues in its replacement,
/// which is part of the tree being killed; the root's own replay is not
/// (module doc, "Which executions are children").
///
/// `$2` (a phase kill, Q3): when not NULL, a child of the root is followed
/// only when its link names one of these runs of the root.
const DESCENDANTS: &str = "WITH RECURSIVE tree (id, depth) AS ( \
       SELECT $1::uuid, 0 \
     UNION ALL \
       SELECT e.id, t.depth + 1 \
         FROM workflow.executions e \
         JOIN tree t \
           ON (e.parent_execution_id = t.id AND e.parent_run_ix IS NOT NULL) \
           OR (e.trigger_execution_id = t.id AND e.trigger_run_ix IS NOT NULL) \
           OR (t.depth > 0 AND e.parent_execution_id = t.id AND e.parent_run_ix IS NULL) \
        WHERE t.depth > 0 OR $2::int[] IS NULL \
           OR e.parent_run_ix = ANY($2) OR e.trigger_run_ix = ANY($2) \
     ) \
     SELECT t.id FROM tree t JOIN workflow.executions e ON e.id = t.id \
      WHERE t.depth > 0 AND NOT workflow.state_is_terminal(e.state) \
      ORDER BY t.depth, t.id";

/// The loaded `state` of `ctx`'s execution, and the same with `kill`'s
/// commands applied -- `executor::kill::kill_leaves`, or for a phase kill
/// `executor::kill::kill_phase` -- under the run rows' locks (module doc).
async fn kill_loaded(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    state: ExecutionState,
    kill: fn(&crate::executor::Plan, &ExecutionState) -> Vec<crate::executor::Command>,
) -> Result<(ExecutionState, ExecutionState)> {
    sqlx::query(
        "SELECT 1 FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix FOR UPDATE",
    )
    .bind(ctx.id)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    let before = state.clone();
    let mut state = state;
    let cmds = kill(&ctx.plan, &state);
    for cmd in &cmds {
        apply(&mut state, cmd);
    }
    Ok((before, state))
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

/// Postgres's `deadlock_detected` (`40P01`), anywhere in the error's chain.
/// Also `claim::run_one`'s and `claim::commit_kill`'s, for the same deadlock
/// seen from the worker's side.
pub(super) fn is_deadlock(e: &Error) -> bool {
    let Error::Internal(e) = e else {
        return false;
    };
    e.chain().any(|c| {
        c.downcast_ref::<sqlx::Error>()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|d| d.code())
            .is_some_and(|code| code == "40P01")
    })
}
