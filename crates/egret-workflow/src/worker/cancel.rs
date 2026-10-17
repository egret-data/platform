//! The kill path: what the worker does to a handler it has taken a run away
//! from.
//!
//! W3b shipped [`TaskHandler::cancel`](super::TaskHandler) with no caller and
//! said so in four places. This module is the caller.
//!
//! # The artifact has this exact seam, and it was read rather than inferred
//!
//! Measured on the pinned image
//! `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`,
//! by `javap -c` over the classes extracted from `/app/kestra` (the image
//! carries a JRE, so the disassembly is done outside it):
//!
//! * `io.kestra.core.models.WorkerJobLifecycle` declares `void kill()` and
//!   `void stop()`, **both `default` and both empty** — `RunnableTask` extends
//!   it, so every Kestra task has a kill and the overwhelming majority of them
//!   do nothing.
//! * `AbstractWorkerCallable.callWithTimeout` catches
//!   `dev.failsafe.TimeoutExceededException` and, in order: runs the caller's
//!   metric `Runnable`, calls `this.kill(null)`, clears the interrupt flag with
//!   `Thread.interrupted()`, and hands `new TimeoutExceededException(duration)`
//!   to `exceptionHandler`.
//! * `WorkerTaskCallable.kill(State.Type)` calls `task.kill()` **first**, and
//!   its exception table catches `java.lang.Exception` around exactly that
//!   call: on a throw it logs `"Error while killing task: '{}'"` at WARN with
//!   the task's type, and then calls `super.kill(state)` regardless — the
//!   `finally` arm does the same on any other throwable.
//!
//! Three facts fall out of that disassembly and all three are load-bearing
//! here:
//!
//! 1. **The budget's expiry is the only `kill(null)` site.** `kill(null)`
//!    appears in `callWithTimeout`'s catch block; the other callers of
//!    `kill(State.Type)` pass a state, which is the execution-level kill. So
//!    this module's [`guarded`] hangs off
//!    [`timeout::Guarded::Expired`](super::timeout::Guarded). **The
//!    execution-level kill is the second site** (workflow kill spec §4.2,
//!    2026-10-17): `claim::finish_kill` calls [`kill`] for a run the kill route
//!    moved to `KILLING` under the worker's lease, after `claim::under_lease`
//!    found it and dropped the handler. Both sites go through [`kill`], so a
//!    handler stops with the one grace it applies (the plugin host's
//!    `kill_grace_s`, zero for a script task) whichever site asked. See *Where
//!    the kill path does NOT go* below.
//! 2. **A kill that fails does not change the run.** The state comes from
//!    `exceptionHandler(TimeoutExceededException)` — `FAILED`, carrying
//!    `Timeout after HH:mm:ss.SSS` — and it is computed *after* `kill` has
//!    already swallowed its own failure. So [`guarded`] returns the budget's
//!    outcome untouched whether the cancel succeeded or refused, and the
//!    refusal goes to `tracing` exactly as Kestra's goes to slf4j. The second
//!    site keeps the rule: a killed run is recorded `KILLED` whatever
//!    `cancel` answered.
//! 3. **egret is louder about a kill it cannot deliver, and that is the only
//!    sense in which it does more.** `WorkerJobLifecycle.kill()` has an empty
//!    default body, so a Kestra task that cannot stop itself is
//!    indistinguishable from one that did; `TaskHandler::cancel` has no default
//!    body, so egret's five handlers each had to answer, and
//!    [`handlers::sql`](super::handlers::sql) answers `Err`.
//!
//!    **Do not read that as egret cancelling more than Kestra.** Fix round 1
//!    corrected exactly that reading: one layer below the interface,
//!    `io.kestra.plugin.jdbc.AbstractJdbcQuery.kill()` calls
//!    `Statement.cancel()` and `close()` on a handle it kept, and Kestra's
//!    Trino and MySQL query tasks both extend it. For the one task class egret
//!    has a counterpart to, the artifact does strictly **more**, and
//!    `handlers::sql`'s `cancel` carries the disassembly and the scope
//!    judgement that leaves it undone.
//!
//! # Where the kill path does NOT go, and that is the finding
//!
//! The obvious other candidate is the seam W3b's Critical came from: [`reap`]
//! clears a lease and a second worker claims the run while the first is still
//! in its handler. **Nothing here can cancel that handler, and the reason is
//! structural rather than an omission.** [`reap`] is a bare `UPDATE` run by
//! whichever process calls it — in production a different one from the worker
//! holding the run — and a lease is a `timestamptz` in a row, not a handle to
//! a future. The abandoned worker learns it was reaped only when
//! [`apply_outcome`](super::apply_outcome) refuses its write, by which time its
//! handler has already returned and there is nothing left to stop.
//!
//! What protects that seam is therefore the fence and not a cancel, which is
//! why `a_killed_handler_cannot_commit_over_the_worker_that_replaced_it`
//! asserts the fence on this path rather than a cancellation.
//!
//! The same is true of the third candidate, and the corpus names it: a
//! `Parallel` whose sibling failed leaves a `Sleep` in flight
//! (`valids/parallel-fail-with-flowable`). Kestra FAILED that run; egret
//! completes it, because abandoning a run is `advance()`'s decision and
//! `advance()` has no worker to tell. That entry stays in the baseline and this
//! module does not move it — see the report's ratchet section.
//!
//! [`reap`]: super::reap

use egret_core::Result;

use super::timeout::{self, Guarded};
use super::{Outcome, Registry, TaskInput};

/// What came of asking a handler to stop.
///
/// A value rather than a `Result` swallowed at the call site: the refusal is
/// the interesting half, and a `let _ = reg.cancel(..)` is exactly the shape
/// that made `TaskHandler::cancel` worth refusing to give a default body.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Killed {
    /// The handler answered `Ok(())` — it had nothing to stop, or it stopped
    /// it.
    Stopped,
    /// The handler refused, carrying its own reason.
    ///
    /// **Not an error the tick propagates.** The run has already failed on its
    /// budget and that is the answer the artifact gives; turning a refusal into
    /// a worker failure would abort the tick and leave the run uncommitted,
    /// which is strictly worse than a run that is correctly FAILED beside a
    /// warning that its work may still be running.
    Refused(String),
}

/// **How far the budget's own cancel got** (final review T4), for a kill found
/// afterwards: not asked, asked and still in flight, or returned.
///
/// A flag set before the cancel was awaited could not tell the second from
/// the third, and the second is not "stopped": when the kill poll finds the
/// run `KILLING` while this cancel is running, `claim::under_lease` drops the
/// handler future, and the cancel inside it with it. The kill path must then
/// ask again; only a cancel that returned is not repeated (a second one is
/// not reliably harmless, `claim::run_one`'s commit-time comment).
#[derive(Debug, Default)]
pub(super) struct Asked(std::sync::atomic::AtomicU8);

impl Asked {
    const IN_FLIGHT: u8 = 1;
    const DONE: u8 = 2;

    fn set(&self, to: u8) {
        self.0.store(to, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether the kill path has to ask the handler to stop: unless the
    /// budget's cancel has already returned.
    pub(super) fn kill_must_cancel(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst) != Self::DONE
    }
}

/// Run `task` under `budget`, and kill the handler if the budget expires.
///
/// This is [`timeout::guard`] plus the artifact's `kill(null)`. The outcome is
/// the budget's, unaltered — see the module doc's fact 2 for why a refused
/// kill does not change it.
///
/// `asked` records the cancel as in flight before it is awaited and as done
/// once it returns ([`Asked`]), so a caller whose run is then found killed
/// (`claim::run_one`) asks again only if this cancel did not finish.
pub(super) async fn guarded(
    reg: &dyn Registry,
    task: &TaskInput,
    budget: Option<std::time::Duration>,
    asked: &Asked,
) -> Result<Outcome> {
    let guarded = timeout::guard(budget, reg.run(task)).await?;
    if matches!(guarded, Guarded::Expired(_)) {
        asked.set(Asked::IN_FLIGHT);
        // **The value is read here, which is the whole reason it is a value.**
        // Fix round 1: `kill` used to log and this line used to discard what it
        // returned, so [`Killed`]'s own doc — "a value rather than a `Result`
        // swallowed at the call site" — described nothing. The log is the
        // caller's decision (the artifact makes it in `WorkerTaskCallable.kill`,
        // the caller of `task.kill()`), so it lives here and `kill` reports.
        let killed = kill(reg, task).await;
        asked.set(Asked::DONE);
        match killed {
            Killed::Stopped => {}
            Killed::Refused(reason) => {
                tracing::warn!(
                    execution = %task.execution_id,
                    run = %task.run_id,
                    task_id = %task.task_id,
                    type_id = %task.type_id,
                    "the worker could not stop this task after its budget expired, \
                     so its work may still be running: {reason}"
                );
            }
        }
    }
    // **One return path, and that is the invariant made structural.** The
    // outcome is the budget's either way; there is no branch here in which a
    // kill — delivered or refused — could rewrite it.
    Ok(guarded.outcome())
}

/// Ask `reg` to stop `task`, and **report** what came of it.
///
/// This function decides nothing: it dispatches and turns the answer into a
/// [`Killed`]. [`guarded`] is what logs a refusal — the artifact splits it the
/// same way, with `RunnableTask.kill()` throwing and its *caller*
/// `WorkerTaskCallable.kill` doing the `logger.warn`.
///
/// The refusal is a `tracing` event rather than a row in the execution's log
/// (W11's `task.log`): Kestra's `logger.warn` here is the worker's own logger,
/// not the task run's, and it is deliberately **not** written to
/// `task_runs.message` either: that column is the
/// reason the task failed, which an author reads, while a kill that could not
/// be delivered is an operator's fact about egret. The artifact keeps the two
/// apart the same way (the message comes from `TimeoutExceededException`, the
/// kill failure from `logger.warn`). Appending would also be a divergence the
/// oracle cannot see — `golden.json` records no failure message — which is the
/// shape this programme has shipped by accident before.
pub(super) async fn kill(reg: &dyn Registry, task: &TaskInput) -> Killed {
    match reg.cancel(task).await {
        Ok(()) => Killed::Stopped,
        Err(e) => Killed::Refused(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::executor::RunState;
    use crate::worker::handlers::testing::input;
    use crate::worker::{Handlers, TaskHandler};

    /// A handler that never finishes, and answers `cancel` however it was
    /// built.
    #[derive(Debug)]
    struct Endless {
        type_id: &'static str,
        refuse: bool,
        cancels: AtomicUsize,
    }

    impl Endless {
        fn new(type_id: &'static str, refuse: bool) -> Arc<Self> {
            Arc::new(Self {
                type_id,
                refuse,
                cancels: AtomicUsize::new(0),
            })
        }
        fn cancels(&self) -> usize {
            self.cancels.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TaskHandler for Endless {
        fn type_id(&self) -> &'static str {
            self.type_id
        }
        async fn run(&self, _task: &TaskInput) -> Result<Outcome> {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(Outcome::success())
        }
        async fn cancel(&self, _task: &TaskInput) -> Result<()> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                Err(egret_core::Error::Internal(anyhow::anyhow!(
                    "this handler holds something it cannot stop"
                )))
            } else {
                Ok(())
            }
        }
    }

    /// A handler that finishes at once.
    #[derive(Debug)]
    struct Prompt(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl TaskHandler for Prompt {
        fn type_id(&self) -> &'static str {
            "test.prompt"
        }
        async fn run(&self, _task: &TaskInput) -> Result<Outcome> {
            Ok(Outcome::failed_with("the handler's own reason"))
        }
        async fn cancel(&self, _task: &TaskInput) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn registry(h: Arc<dyn TaskHandler>) -> Handlers {
        let mut r = Handlers::new();
        r.add(h);
        r
    }

    /// **The budget's expiry calls `cancel`, and nothing else does.**
    ///
    /// Both directions, because a kill path wired to fire unconditionally
    /// passes the first assertion: a handler that answered inside its budget
    /// must not be cancelled, and cancelling one that has already returned is
    /// how a pod-backed handler in W4's successor would be killed after it had
    /// already written its output.
    #[tokio::test]
    async fn a_handler_is_cancelled_when_its_budget_expires_and_never_otherwise() {
        let endless = Endless::new("test.endless", false);
        let reg = registry(endless.clone());
        let out = guarded(
            &reg,
            &input("test.endless", serde_json::json!({})),
            Some(Duration::from_millis(50)),
            &Default::default(),
        )
        .await
        .expect("a timeout is a task failure, not a worker failure");
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("Timeout after 00:00:00.050"));
        assert_eq!(
            endless.cancels(),
            1,
            "the handler must be told to stop exactly once when its budget \
             expires -- dropping the future is only half of what \
             `callWithTimeout` does"
        );

        let cancels = Arc::new(AtomicUsize::new(0));
        let reg = registry(Arc::new(Prompt(cancels.clone())));
        let out = guarded(
            &reg,
            &input("test.prompt", serde_json::json!({})),
            Some(Duration::from_secs(30)),
            &Default::default(),
        )
        .await
        .expect("no worker failure");
        assert_eq!(out.message.as_deref(), Some("the handler's own reason"));
        assert_eq!(
            cancels.load(Ordering::SeqCst),
            0,
            "a handler that answered inside its budget has already stopped, and \
             cancelling it would reach a task that had finished its work"
        );

        // And no budget at all cannot expire, so it cannot kill.
        let out = guarded(
            &reg,
            &input("test.prompt", serde_json::json!({})),
            None,
            &Default::default(),
        )
        .await
        .expect("no worker failure");
        assert_eq!(out.message.as_deref(), Some("the handler's own reason"));
        assert_eq!(
            cancels.load(Ordering::SeqCst),
            0,
            "a task with no `timeout:` is never killed by this path"
        );
    }

    /// **A refused kill is reported and does not change the run.**
    ///
    /// The artifact's own arrangement, read off `WorkerTaskCallable.kill`'s
    /// exception table: `task.kill()` throwing is caught, logged, and the
    /// terminal is still `exceptionHandler`'s. A version that propagated the
    /// refusal would abort the tick and leave the run uncommitted, which this
    /// asserts against by reading the outcome rather than only the absence of
    /// an error.
    #[tokio::test]
    async fn a_refused_kill_is_reported_and_leaves_the_budgets_own_outcome_alone() {
        let endless = Endless::new("test.endless", true);
        let reg = registry(endless.clone());
        let task = input("test.endless", serde_json::json!({}));

        assert_eq!(
            kill(&reg, &task).await,
            Killed::Refused("this handler holds something it cannot stop".to_string()),
            "the refusal must reach the caller carrying the handler's own reason"
        );

        let out = guarded(
            &reg,
            &task,
            Some(Duration::from_millis(50)),
            &Default::default(),
        )
        .await
        .expect("a refused kill must not become a worker failure that aborts the tick");
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            out.message.as_deref(),
            Some("Timeout after 00:00:00.050"),
            "the message is the budget's, byte for byte: the kill's failure is \
             an operator's fact and does not belong in the author-facing reason \
             the task failed"
        );
        assert_eq!(endless.cancels(), 2, "one per call, and both were made");
    }

    /// A type with no handler cannot be cancelled either, and says which type.
    ///
    /// Reachable: `Registry::run` refuses the same type, so a run whose type is
    /// unregistered never gets a budget — but a registry is a runtime table and
    /// the kill path must not panic or claim success on a miss.
    #[tokio::test]
    async fn an_unregistered_type_refuses_the_kill_and_names_itself() {
        let reg = Handlers::new();
        let Killed::Refused(reason) =
            kill(&reg, &input("test.absent", serde_json::json!({}))).await
        else {
            panic!("a registry with no handler must not report a task as stopped");
        };
        assert!(reason.contains("test.absent"), "got {reason}");
    }
}
