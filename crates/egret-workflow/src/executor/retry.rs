//! `retry:` — the attempt lifecycle — and what `timeout:` turned out not to be.
//!
//! # `timeout:` is not here, and that is measured rather than deferred
//!
//! The plan gives this file "`retry`, `timeout` and `errors:`". Two of the
//! three are here. `timeout:` is enforced by Kestra's **worker**, not by its
//! executor: `worker/src/main/java/io/kestra/worker/processors/internals/
//! WorkerTaskCallable.java:61` wraps `task.run(runContext)` in
//! `callWithTimeout`, and Kestra's own validator says so out loud —
//! `core/src/main/java/io/kestra/core/services/FlowService.java:574` emits
//! "cannot use the 'timeout' property as it's only relevant for runnable
//! tasks" for every non-`RunnableTask`, with `Pause` the single exception. So
//! the only `timeout:` an `advance()` will ever enforce is a `Pause`'s, and
//! `Pause` is W8's — **W8 Task 3 shipped it**, in
//! [`flowable::pause`](super::flowable::pause) rather than here. W4's worker owns the rest. What W3a Task 7 *does* own of
//! the elapsed-time rules is [`RetryPolicy::max_duration`], which is below.
//!
//! # The two behaviours, and the instant each is measured from
//!
//! `AbstractRetry.Behavior` has exactly two values on the wire,
//! `RETRY_FAILED_TASK` and `CREATE_NEW_EXECUTION`, and the artifact reads a
//! *different clock* for each:
//!
//! | behaviour | `maxAttempts` counts | `maxDuration` is measured from |
//! |---|---|---|
//! | `RETRY_FAILED_TASK` | `TaskRun.attemptNumber()` — this run's attempts | `attempts.getFirst().getState().minDate()` (`TaskRun.java:337`) |
//! | `CREATE_NEW_EXECUTION` | `execution.getMetadata().getAttemptNumber()` | `execution.getMetadata().getOriginalCreatedDate()` (`TaskRun.java:316`) |
//!
//! [`TaskRun::started_at`](super::TaskRun::started_at) is the first; the
//! execution's creation is [`Tick`]`(0)` by construction, which is what that
//! type's doc means by "the origin is the execution's creation". Task 3 built
//! both accessors for exactly this.
//!
//! # What is written down here that this oracle cannot see
//!
//! Two things, both named rather than hidden:
//!
//! - **The base instant.** Kestra takes the next retry date from
//!   `this.lastAttempt().getState().maxDate()` — the instant the *last attempt
//!   ended*. [`Attempt`](super::Attempt) records only `started_at`, so this
//!   file uses `now`: the instant the executor is deciding at, which is the
//!   same executor cycle in which Kestra stamps that `maxDate`. They differ by
//!   however long the caller waited before asking again, and no tier of the
//!   oracle's environment ever waits — see `oracle-exec/README.md`, *what this
//!   oracle does not prove*, cause 1. Adding an `Attempt::ended_at` that
//!   nothing could tell apart from `started_at` would be a fourth field in
//!   the family Task 3 already had to make distinguishable.
//! - **`isErrorOrFinallyHandlingPending`** (`ExecutorService.java:868`), which
//!   holds an ancestor-owned retry timer until that ancestor's `errors:`/
//!   `finally:` runs have terminated. It is **not implemented**, and the
//!   reason is that nothing could see it fire: the two corpus flows that reach
//!   it (`valids/retry-with-flowable-errors`, `valids/allow-failure-with-retry`)
//!   both hang the retry on an `AllowFailure`, which `build_plan` plans as an
//!   opaque [`Runnable`](super::TaskKind::Runnable) and whose children
//!   therefore get no run at all. A guard nobody can see fail is the thing
//!   this package has now paid for six times.

use std::time::Duration;

use crate::models::{Retry as ModelRetry, RetryBehavior as ModelBehavior};

use super::command::Command;
use super::plan::{Plan, RetryBehavior, RetryDelay, RetryPolicy, TaskIx, TaskKind};
use super::state::{ExecutionState, RunIx, RunState, Tick};

/// Read W1's parsed `retry:` into a [`RetryPolicy`], or refuse the whole
/// policy.
///
/// **All-or-nothing on purpose.** A policy with an unreadable `interval` is not
/// a policy with a default interval — it is one this builder cannot evaluate,
/// and evaluating it anyway would invent a retry schedule the author did not
/// write. Kestra's own bean validation refuses the same shapes (`interval` is
/// `@NotNull` on all three subtypes), so a `None` here and Kestra's refusal
/// land in the same place: no retry happens.
///
/// Every field is a literal in Kestra, not a `Property` (see
/// [`PlanTask::retry`](super::PlanTask::retry)), so "unreadable" means an
/// expression W1 wrapped in a `Property` that does not parse as an ISO-8601
/// duration. `every_corpus_retry_is_written_in_literals` measures that no
/// corpus flow does that.
pub(super) fn policy_of(retry: Option<&ModelRetry>) -> Option<RetryPolicy> {
    let (common, delay) = match retry? {
        ModelRetry::Constant { common, interval } => (
            common,
            RetryDelay::Constant {
                interval: literal_duration(interval.as_ref())?,
            },
        ),
        ModelRetry::Exponential {
            common,
            interval,
            max_interval,
            delay_factor,
        } => (
            common,
            RetryDelay::Exponential {
                interval: literal_duration(interval.as_ref())?,
                max_interval: literal_duration(max_interval.as_ref())?,
                delay_factor: *delay_factor,
            },
        ),
        ModelRetry::Random {
            common,
            min_interval,
            max_interval,
        } => (
            common,
            RetryDelay::Random {
                min_interval: literal_duration(min_interval.as_ref())?,
                max_interval: literal_duration(max_interval.as_ref())?,
            },
        ),
    };

    Some(RetryPolicy {
        // Kestra's `@Min(1) Integer maxAttempts`: absent means no cap, which is
        // not the same as zero. A negative one is not representable as a count
        // and takes the same road as an absent one -- Kestra has no guard for
        // it either (`attemptNumber() >= -1` is true at once, so it would stop
        // every retry), and no corpus flow writes one.
        max_attempts: common.max_attempts.and_then(|n| u32::try_from(n).ok()),
        delay,
        max_duration: match common.max_duration.as_ref() {
            None => None,
            // An unreadable `maxDuration` refuses the policy for the same
            // reason an unreadable `interval` does: silently dropping the bound
            // would make the retry run LONGER than the author asked, which is
            // the over-acceptance direction.
            Some(_) => Some(literal_duration(common.max_duration.as_ref())?),
        },
        behavior: match common.behavior {
            ModelBehavior::RetryFailedTask => RetryBehavior::RetryFailedTask,
            ModelBehavior::CreateNewExecution => RetryBehavior::CreateNewExecution,
        },
        warning_on_retry: common.warning_on_retry,
    })
}

/// The policy governing `run`, in Kestra's precedence order
/// (`ExecutorService.java:576-599`): the task's own `retry:`, else the nearest
/// **ancestor** with one, else the flow's.
///
/// The ancestor arm is `searchForParentTaskWithRetry`
/// (`ExecutorService.java:841`), and two details of it are load-bearing:
///
/// - it is gated on `taskRun.getParentTaskRunId() != null`, so a **root-level**
///   run never consults an ancestor and goes straight to the flow's retry;
/// - when an ancestor with a retry exists, the flow's retry is **not**
///   consulted — Java's `else if (executor.getFlow().getRetry() != null)`
///   hangs off `if (retry != null)`, not off the search.
///
/// Kestra walks the *flow's* task tree (`findParentTasksByTaskId`); this walks
/// the *run's* parent chain, which is the same chain because a run's parent is
/// fixed at creation to the run of its task's parent. Using the runs is what
/// makes the walk terminate by construction: parent indices strictly decrease.
pub(super) fn policy_for<'a>(
    plan: &'a Plan,
    state: &ExecutionState,
    run: RunIx,
) -> Option<&'a RetryPolicy> {
    // End hooks #2b, D11: nothing in the post-terminal phase is retried. A
    // run under `afterExecution:` has no policy at all, so `retry::scan`
    // (never called in the phase anyway) and, above all,
    // `flowable::current_list`'s "will be retried" agree: a flowable inside
    // the list whose child failed is not left waiting for a retry nobody
    // will schedule (`a_flowable_in_the_phase_does_not_wait_for_a_retry`).
    if super::after_execution::in_phase(plan, state, run) {
        return None;
    }
    if let Some(own) = plan
        .tasks
        .get(state.runs[run].task)
        .and_then(|t| t.retry.as_ref())
    {
        return Some(own);
    }
    let mut parent = state.runs[run].parent;
    while let Some(ix) = parent {
        if let Some(policy) = plan
            .tasks
            .get(state.runs[ix].task)
            .and_then(|t| t.retry.as_ref())
        {
            return Some(policy);
        }
        parent = state.runs[ix].parent;
    }
    // Only a run with no ancestor carrying a retry reaches the flow's.
    plan.retry.as_ref()
}

/// Kestra's `TaskRun.nextRetryDate(retry)` and `nextRetryDate(retry,
/// execution)`, over [`Tick`]s.
///
/// Returns the instant the next attempt may start, or `None` when the policy is
/// exhausted. The two behaviours differ in both guards, which is why `behavior`
/// is read here rather than by the caller.
pub(super) fn next_retry_at(
    policy: &RetryPolicy,
    state: &ExecutionState,
    run: RunIx,
    now: Tick,
) -> Option<Tick> {
    let task_run = &state.runs[run];
    // `this.attempts == null || this.attempts.isEmpty()` -- a run that never
    // started an attempt has nothing to retry.
    //
    // **A `NonZeroU32` rather than an `if attempts == 0` guard, deliberately.**
    // The guard was written first and a mutation that neutered it killed
    // NOTHING: `scan` only offers `Failed` runs, and every run this corpus can
    // fail has started an attempt -- a `Loop` that fails at `CREATED` has none,
    // but a `Loop` is not a `Runnable` and `scan` skips it. The condition is
    // still real (with zero attempts, `count >= max_attempts` is false and a
    // `CREATE_NEW_EXECUTION` policy would schedule a retry for a run that never
    // ran), so the answer is to fold it into the type rather than to ship a
    // guard nobody has seen fire: `NonZeroU32::new(0)` is `None` by
    // construction and **there is no line here to delete**.
    //
    // That is the accurate claim, and a stronger one was withdrawn. A review
    // wrote a mutation of this line anyway --
    // `.and_then(|n| NonZeroU32::new(n.max(1)))?` -- which compiles, runs and
    // kills nothing. So the mutation is *writable*; what the type buys is that
    // the obvious edit (deleting a guard) no longer exists, not that no edit
    // does. The substantive point is unchanged and is what that mutation
    // confirms: the condition is unreachable from `scan`.
    let attempts = u32::try_from(task_run.attempts.len())
        .ok()
        .and_then(std::num::NonZeroU32::new)?
        .get();

    let (count, origin) = match policy.behavior {
        // `this.attemptNumber()`, and `attempts.getFirst().getState().minDate()`.
        RetryBehavior::RetryFailedTask => (attempts, task_run.started_at()?),
        // `execution.getMetadata().getAttemptNumber()`, and
        // `getOriginalCreatedDate()`.
        //
        // **This read was `(1, Tick(0))` until W6 Task 4, and both halves were
        // right about the FIRST execution and wrong about every replay.** The
        // comment said "this execution IS the first attempt, so its metadata
        // attempt number is 1 -- and `Tick`'s origin is the execution's
        // creation, so the origin is `Tick(0)`", which held for exactly as
        // long as no replay existed. A replay is attempt 2, 3, ... of the same
        // chain, and the instant its `maxDuration` is measured from is the
        // ORIGINAL execution's creation, which on its own clock is a negative
        // `Tick`. With the constants in place `1 >= max_attempts` is never
        // true and `next.0 > 0 + max_duration` is never true either, so a
        // chain started here would never end: `retry-new-execution-*` would
        // spawn replays until the driver's step bound, not the three
        // executions the golden holds. [`ExecutionMeta`] is where the caller
        // puts the two facts.
        RetryBehavior::CreateNewExecution => (state.metadata.attempt, state.metadata.origin),
    };

    if policy.max_attempts.is_some_and(|max| count >= max) {
        return None;
    }

    let next = Tick(
        now.0
            .saturating_add(millis(&delay_after(&policy.delay, count))),
    );
    if let Some(max) = policy.max_duration {
        // `nextDate.isAfter(origin.plus(maxDuration))` -- strictly after.
        if next.0 > origin.0.saturating_add(millis(&max)) {
            return None;
        }
    }
    Some(next)
}

/// `AbstractRetry.nextRetryDate(attemptCount, lastAttempt)`, as a span.
///
/// `count` is Kestra's `attemptCount`, which is `this.attempts.size()` — so it
/// is `1` before the first retry, not `0`. `Exponential` is the only one that
/// reads it.
fn delay_after(delay: &RetryDelay, count: u32) -> Duration {
    match delay {
        RetryDelay::Constant { interval } => *interval,
        RetryDelay::Exponential {
            interval,
            max_interval,
            delay_factor,
        } => {
            let factor = delay_factor.unwrap_or(2.0);
            let scaled = millis(interval) as f64 * factor.powi(count as i32 - 1);
            let cap = millis(max_interval);
            // Kestra: `delayMillis >= maxMillis ? maxMillis : Math.round(delayMillis)`.
            // `partial_cmp` rather than `>=` so the case Kestra's own comment
            // calls out -- `delayMillis` being `Double.POSITIVE_INFINITY` for a
            // very large attempt count, and, here, a `NaN` factor -- lands on
            // the cap rather than on a rounded `u64` cast.
            match scaled.partial_cmp(&(cap as f64)) {
                Some(std::cmp::Ordering::Less) => Duration::from_millis(scaled.round() as u64),
                _ => *max_interval,
            }
        }
        // The lower bound, deliberately -- see `RetryDelay::Random`.
        RetryDelay::Random { min_interval, .. } => *min_interval,
    }
}

fn millis(d: &Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// Kestra's retry pass over the whole task run list
/// (`ExecutorService.java:568-620`), for the one run it reaches first.
///
/// # Why it is a pass over the runs and not part of a task list's drive
///
/// It is Kestra's shape: the retry decision is taken in `handleFlowableTasks`,
/// which iterates `execution.getTaskRunList()` and asks each FAILED run whether
/// it should be retried — independently of which list that run belongs to and
/// of what that list decided. A retry that lived inside
/// [`sequential::drive`](super::flowable::sequential::drive) would have to be
/// written again in [`parallel`](super::flowable::parallel) and in
/// [`dag`](super::flowable::dag), which is three chances for one of them to
/// stop being true.
///
/// # Which runs are eligible
///
/// `task instanceof RunnableTask<?> || task instanceof Subflow` — a **flowable
/// is never retried**, however its own `retry:` reads. `valids/retry-flowable`
/// is the measurement: its `retry:` sits on a `Sequential` and it is the
/// `Sequential`'s *child* that runs three attempts, while the `Sequential`
/// itself has one and ends `FAILED`.
///
/// **This used to read *"`Subflow` is planned opaque today, so
/// [`TaskKind::Runnable`] is both halves of that test here"*, and W6 made that
/// false in the commit that could have left it standing.** A `Subflow` is a
/// [`TaskKind::Executable`] now, so the predicate spells both halves the way
/// Kestra does. It is corpus-reachable: `valids/subflow-parent-retry`'s
/// `subflow1` and `subflow2` each run **2** attempts and `valids/retry-subflow`'s
/// single `subflow` run runs **3**, and a second attempt starts a SECOND child
/// execution under the same `(execution, run_ix)` -- which is exactly why
/// migration 0138's reverse index is not unique. Both entries stay baselined,
/// on `state::apply`'s rule 1 rather than on this.
pub(super) fn scan(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    for run in 0..state.runs.len() {
        if state.runs[run].history.last() != Some(&RunState::Failed) {
            continue;
        }
        let task = state.runs[run].task;
        if !matches!(
            plan.tasks.get(task).map(|t| &t.kind),
            Some(TaskKind::Runnable { .. } | TaskKind::Executable { .. })
        ) {
            continue;
        }
        let Some(policy) = policy_for(plan, state, run) else {
            continue;
        };
        let Some(at) = next_retry_at(policy, state, run, now) else {
            continue;
        };
        return match policy.behavior {
            // `markWithTaskRunAs(execution, taskRunId, RETRYING, false)` plus an
            // `ExecutionDelay` at `at`. `markParents` is `false`, so an
            // ancestor flowable keeps the state it has -- which is what leaves
            // `valids/retry-flowable`'s `Sequential` RUNNING across all three
            // of its child's attempts.
            RetryBehavior::RetryFailedTask => vec![
                Command::SetRunState {
                    run,
                    state: RunState::Retrying,
                },
                Command::SetExecutionState {
                    state: RunState::Retrying,
                },
                Command::ScheduleRetry {
                    run,
                    not_before: at,
                },
            ],
            // `markWithTaskRunAs(execution, taskRunId, RETRIED, true)` -- and
            // `true` is `markParents`, so every ancestor run takes `RETRIED`
            // too, and so does the execution. `RETRIED` is terminal
            // (`State.java`'s `isTerminated()` lists it), so this execution
            // ends here and Kestra starts a *replay*: a whole new execution.
            //
            // # The `ScheduleRetry` is the artifact's `ExecutionDelay`, and it
            // used to be missing
            //
            // Both arms of `ExecutorService.java:603-608` build the SAME
            // `ExecutionDelay`, `.taskRunId(taskRun.getId()).date(nextRetryDate)`,
            // and differ only in its `delayType`: `RESTART_FAILED_TASK` for one
            // and **`RESTART_FAILED_FLOW`** for this one, which
            // `DefaultExecutor`'s execution-delay loop turns into
            // `executionService.replay(...)`. So the delay hangs on THIS run in
            // the artifact too, and this arm emitting none was a divergence
            // rather than a simplification -- one that left the replay with no
            // instant to happen at, so `worker::replay` would have started the
            // next execution the moment this one failed and `maxDuration`
            // would have measured nothing.
            //
            // Nothing reopens the run it names: `reopen_due` takes only
            // `Retrying` runs and this one is `Retried`, which is terminal. It
            // is read by `worker::claim::due_retries` and
            // `worker::replay::spawn_due`, exactly as the artifact reads it
            // from a different loop.
            RetryBehavior::CreateNewExecution => {
                let mut cmds = Vec::new();
                let mut ix = Some(run);
                while let Some(cur) = ix {
                    cmds.push(Command::SetRunState {
                        run: cur,
                        state: RunState::Retried,
                    });
                    ix = state.runs[cur].parent;
                }
                cmds.push(Command::SetExecutionState {
                    state: RunState::Retried,
                });
                cmds.push(Command::ScheduleRetry {
                    run,
                    not_before: at,
                });
                cmds
            }
        };
    }
    Vec::new()
}

/// The `ExecutionDelay` firing: `ExecutionService.retryTask`
/// (`ExecutionService.java:135`), which is `execution.withTaskRun(taskRun
/// .withState(CREATED)).withState(State.Type.RUNNING)`.
///
/// So a due retry writes exactly two things — the run back to `CREATED` and the
/// execution back to `RUNNING` — and the ordinary
/// [`open`](super::flowable::open) path does the rest on the next round, which
/// is what puts `SUBMITTED` and a fresh [`Attempt`](super::Attempt) into the
/// history in Kestra's own order.
///
/// It is emitted **alone**, ahead of everything else in [`advance`](super::advance),
/// because in the artifact it is a different loop entirely
/// (`DefaultExecutor.java:561`, the execution-delay loop) whose output is
/// re-submitted for processing rather than merged into the cycle that scheduled
/// it.
/// # It takes only `RETRYING`, and W8 Task 2's predicted `PAUSED` caller never arrived
///
/// **The prediction was WRONG and is kept because being wrong is the useful
/// part.** It read: *"A bounded `Pause` is the same shape — a run parked with a
/// [`TaskRun::not_before`](super::TaskRun) that comes due — so whoever emits
/// [`RunState::Paused`](super::RunState) must widen this arm or the run parks
/// for ever."* W8 Task 3 emitted `Paused`, banked eight entries, and **this arm
/// is byte-unchanged**.
///
/// The two shapes part at [`RunState::Created`](super::RunState). This function
/// reopens a retry by putting the run back into `CREATED`, which starts a NEW
/// attempt — measured, `valids/retry-failed-task-attempts` has four. A pause
/// that ends does the opposite: its attempt was never closed, and all twelve
/// resumed golden entries read run `CREATED RUNNING PAUSED <terminal>` over
/// attempt `CREATED RUNNING <terminal>`, one attempt taking the terminal
/// straight from the `RUNNING` it was left at. Reopening a pause here would
/// give it a second attempt and mismatch every one of the twelve.
///
/// So `flowable::sequential::walk` asks
/// [`flowable::resume_paused`](super::flowable::resume_paused) at the `PAUSED`
/// run instead — which is also where the decision belongs.
///
/// **The REASON given here was falsified by W8 Task 4 fix round 2, so it is
/// rewritten rather than repointed.** It read *"the terminal depends on WHICH
/// bound elapsed and only the plan knows that"*. `resume_paused(state, run,
/// now)` takes **no plan at all** and therefore cannot key on the bound. The
/// bound is read once, in the round the pause is ENTERED
/// ([`flowable::pause::drive`](super::flowable::pause)), and the terminal it
/// chose is carried on `Command::PauseRun` as a
/// [`Resume`](crate::executor::Resume). A `PAUSED` run holds the **answer**,
/// not the question — which is the whole reason the two `TaskKind`-keyed
/// guards could be deleted.
///
/// **What the paragraph got right is worth keeping too**: the ratchet WOULD
/// have caught the mistake. `driver::pass_time` moves the clock, calls
/// `worker::resume`, which reaches `claim::settle` and then
/// [`advance`](super::advance), which calls this function on the first line of
/// its body — so a `Paused` run nothing reopens is a stalled execution and a
/// red ratchet. `worker::claim::due_retries` is the one the ratchet cannot see,
/// because its gate is SQL the driver never runs — `pass_time` selects on
/// `not_before_ms` with no state term at all. **That one WAS a live defect and
/// W8 Task 7 fixed it**: the gate now carries a `'PAUSED'` arm, driven by
/// `tests/worker.rs`'s
/// `a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken`
/// and `the_waker_names_exactly_the_states_something_will_move`. The ratchet
/// reads **109 either way** — measured in both directions with the arm removed
/// and restored — which is what the sentence above means and why the two tests
/// had to exist.
pub(super) fn reopen_due(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    let due: Vec<RunIx> = (0..state.runs.len())
        .filter(|&ix| {
            state.runs[ix].history.last() == Some(&RunState::Retrying)
                && state.runs[ix].not_before.is_some_and(|t| t <= now)
                && plan.tasks.contains_index(state.runs[ix].task)
        })
        .collect();
    if due.is_empty() {
        return Vec::new();
    }
    // `withState(RUNNING)` once, however many runs are due: the execution has
    // one state, and it is `RETRYING` whenever any run is, because `scan`
    // wrote it there. Emitting it per run would push a duplicate `RUNNING`
    // into the execution's history, which the golden's histories never carry.
    let mut cmds = vec![Command::SetExecutionState {
        state: RunState::Running,
    }];
    cmds.extend(due.into_iter().map(|run| Command::SetRunState {
        run,
        state: RunState::Created,
    }));
    cmds
}

/// Java's `Duration.parse` over the ISO-8601 subset a `retry:` can be written
/// in: an optional sign, `P`, an optional day part, `T`, and any of hours,
/// minutes and (possibly fractional) seconds.
///
/// **Case-insensitive, and that is not a nicety.** `java.time.Duration.parse`
/// accepts either case, and `valids/retry-expo` writes `interval: PT0.25s`
/// with a lower-case `s`. A case-sensitive parser reads that as unparseable,
/// drops the whole policy, and loses the one corpus flow that exercises
/// `exponential` — silently, because a dropped policy just means no retry.
/// `every_corpus_retry_is_written_in_literals` is what would catch it.
///
/// Weeks (`P2W`) and months are not accepted: `Duration.parse` does not accept
/// months either, and nothing in the corpus writes a week. `None` for anything
/// else, which refuses the policy.
pub fn parse_duration(source: &str) -> Option<Duration> {
    let s = source.trim();
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    // A negative duration is representable in Java and not in `Duration`, and
    // there is nothing sensible for a retry to do with one, so it is refused
    // rather than folded to its absolute value.
    if negative {
        return None;
    }
    let s = s.strip_prefix(['P', 'p'])?;
    let (days, rest) = match s.split_once(['T', 't']) {
        Some((d, rest)) => (d, rest),
        // `P1D` with no time part.
        None => (s, ""),
    };

    let mut total = 0f64;
    let mut fields = 0usize;
    if !days.is_empty() {
        // The date half is days only. Weeks, months and years are not part of
        // `Duration.parse`'s grammar either, so `P1M` is refused here exactly
        // as Java refuses it -- and refusing it is what stops a `P1M` being
        // read as one minute.
        let mut d = days.to_string();
        total += take_unit(&mut d, ['D', 'd'], &mut fields)? * 86_400.0;
        if !d.is_empty() {
            return None;
        }
    }
    let mut rest = rest.to_string();
    total += take_unit(&mut rest, ['H', 'h'], &mut fields)? * 3600.0;
    total += take_unit(&mut rest, ['M', 'm'], &mut fields)? * 60.0;
    total += take_unit(&mut rest, ['S', 's'], &mut fields)?;
    // Anything left over is not part of the grammar -- `PT1S2` and `PT1X` both
    // land here rather than being read as `PT1S`.
    if !rest.is_empty() || fields == 0 {
        return None;
    }
    Duration::try_from_secs_f64(total).ok()
}

/// Consume `<number><unit>` from the front of `text` if it is there,
/// contributing to `fields`. `Some(0.0)` when the unit is absent, `None` when
/// it is present with a number that is not a non-negative decimal.
fn take_unit(text: &mut String, unit: [char; 2], fields: &mut usize) -> Option<f64> {
    let Some(at) = text.find(unit) else {
        return Some(0.0);
    };
    let value = text[..at].parse::<f64>().ok().filter(|v| *v >= 0.0)?;
    *text = text[at + 1..].to_string();
    *fields += 1;
    Some(value)
}

/// A `Property<Duration>`'s source read as a literal.
fn literal_duration(p: Option<&crate::models::Property<Duration>>) -> Option<Duration> {
    parse_duration(p?.source())
}

/// `Vec`'s bound check, spelled as a method so the filter above reads as one
/// condition rather than three.
trait ContainsIndex {
    fn contains_index(&self, ix: TaskIx) -> bool;
}
impl<T> ContainsIndex for Vec<T> {
    fn contains_index(&self, ix: TaskIx) -> bool {
        ix < self.len()
    }
}
