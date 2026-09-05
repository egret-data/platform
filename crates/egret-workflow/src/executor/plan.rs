//! The published, reference-resolved flow: the shape
//! [`advance`](super::advance) walks, and the builder that turns a flow's YAML
//! into it.
//!
//! No I/O and no clock. [`build_plan`] reads a `&str` and nothing else.

use std::time::Duration;

use crate::models::{Task, TaskKind as ModelKind};

/// An index into [`Plan::tasks`].
pub type TaskIx = usize;

/// The published, reference-resolved flow. Built once, never mutated.
// No `Eq`: `RetryDelay::Exponential::delay_factor` is Kestra's own `Double`
// (`Exponential.java`'s `delayFactor`), and a float has no total equality. It
// reaches here through `PlanTask::retry`, so the whole chain is `PartialEq`
// only -- which is all any caller uses.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Plan {
    /// Flattened; children are addressed by index. Pre-order, so a parent
    /// always precedes its children — an ordering convenience for a reader,
    /// not something any rule depends on.
    pub tasks: Vec<PlanTask>,
    /// The flow's top-level `tasks:`.
    pub roots: Vec<TaskIx>,
    /// The flow's top-level `errors:`.
    pub errors: Vec<TaskIx>,
    /// The flow's own `retry:`, `Flow.java`'s `AbstractRetry retry`.
    ///
    /// It is the **last** of Kestra's three sources, consulted only for a run
    /// whose task has no `retry:` and none of whose ancestors has one --
    /// `ExecutorService.java:576-599`. See
    /// [`retry::policy_for`](super::retry::policy_for), which is that
    /// precedence in one place.
    pub retry: Option<RetryPolicy>,
}

impl Plan {
    /// True when `task` is inside some [`TaskKind::Loop`]'s `tasks:`.
    ///
    /// A loop body never gets a run in the parent execution — each value
    /// produces a separate sub-execution instead — so a task that answers
    /// `true` here is one no [`Command::CreateRun`](super::Command::CreateRun)
    /// may ever name.
    ///
    /// **"Inside" is the whole subtree, not the direct children**, and that is
    /// the reading the behaviour forces rather than a convenience: an `If`
    /// nested in a `Sequential` in a loop body is just as absent from the
    /// parent's `taskRunList` as the `Sequential` is. The two readings differ on
    /// this corpus — `the_two_readings_of_inside_a_loop_differ_by_a_measured_
    /// amount` derives both counts — so taking the narrow one silently would be
    /// a choice nothing could see.
    ///
    /// **O(n²) in `tasks.len()` per call**, not linear: it rebuilds the frontier
    /// from every `Loop` on each call and its `seen` set is a `Vec` searched
    /// linearly. That is stated rather than rounded off because a wrong
    /// complexity in a doc comment is how one gets copied into a hot path. It is
    /// harmless where it is used — 82 loop-body tasks across the whole corpus,
    /// and no caller is on `advance()`'s path — and a `BTreeSet` would fix it
    /// the day one is.
    ///
    /// Used by the oracle rather than by [`advance`](super::advance), which
    /// never descends into a `Loop`'s list and so cannot create one of these
    /// runs by construction. That is exactly why the guard is worth having:
    /// "cannot happen by construction" is a claim about today's `descend`, and
    /// `no_run_is_ever_created_for_a_task_inside_a_loop` is what would notice
    /// it stopping being true.
    pub fn is_loop_body(&self, task: TaskIx) -> bool {
        let mut frontier: Vec<TaskIx> = self
            .tasks
            .iter()
            .filter_map(|t| match &t.kind {
                TaskKind::Loop { tasks, .. } => Some(tasks.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        // `flatten_one` reserves a parent's index before its children's, so a
        // child's index is always greater than its parent's and this walk
        // strictly descends. It visits each task at most once for any plan.
        let mut seen: Vec<TaskIx> = Vec::new();
        while let Some(ix) = frontier.pop() {
            if ix == task {
                return true;
            }
            if seen.contains(&ix) {
                continue;
            }
            seen.push(ix);
            frontier.extend(self.children_of(ix));
        }
        false
    }

    /// Every task index this task's own lists hold, whatever kind it is.
    ///
    /// One place, so a new [`TaskKind`] cannot be added to the plan and left
    /// out of the walk — which is how `is_loop_body` would quietly start
    /// answering `false` for a body nested in whatever the new kind is.
    ///
    /// **`errors` is part of it, and that is Kestra's own walk**:
    /// `AbstractBranch.allChildTasks()` is `ListUtils.concat(tasks, errors,
    /// _finally)`. Task 7 is the first task for which the list is ever
    /// non-empty, and including it moves the number
    /// `the_two_readings_of_inside_a_loop_differ_by_a_measured_amount`
    /// derives — which is why that test derives it rather than remembering it.
    fn children_of(&self, ix: TaskIx) -> Vec<TaskIx> {
        let own_errors = self.tasks[ix].errors.iter().copied();
        let mut out: Vec<TaskIx> = own_errors.collect();
        out.extend(self.direct_task_children(ix));
        out
    }

    /// [`Plan::children_of`] without the `errors:` list — the `tasks:` half
    /// alone.
    fn direct_task_children(&self, ix: TaskIx) -> Vec<TaskIx> {
        match &self.tasks[ix].kind {
            TaskKind::Runnable { .. } => Vec::new(),
            TaskKind::Sequential { tasks }
            | TaskKind::Parallel { tasks, .. }
            | TaskKind::Loop { tasks, .. } => tasks.clone(),
            TaskKind::If { then_, else_, .. } => then_.iter().chain(else_).copied().collect(),
            TaskKind::Switch { cases, default, .. } => cases
                .iter()
                .flat_map(|(_, t)| t.iter())
                .chain(default)
                .copied()
                .collect(),
            TaskKind::Dag { nodes } => nodes.iter().map(|n| n.task).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanTask {
    /// The author's task id, as written.
    pub id: String,
    pub kind: TaskKind,
    /// This task's own `retry:`.
    ///
    /// **Task 3's reason for leaving this `None` was wrong, measured.** It
    /// said "Kestra's `retry.interval`/`maxDuration` are `Property<Duration>`
    /// — unrendered source text — and rendering one needs a `RenderContext`
    /// this pure builder does not have". In the artifact they are not:
    /// `core/src/main/java/io/kestra/core/models/tasks/Task.java:56` is
    /// `protected AbstractRetry retry;` — a plain POJO — and every field
    /// inside it (`AbstractRetry.maxDuration`, `AbstractRetry.maxAttempts`,
    /// `Constant.interval`, `Exponential.interval`/`maxInterval`,
    /// `Random.minInterval`/`maxInterval`) is a plain `java.time.Duration` or
    /// `Integer`. Kestra never renders a retry. The `Property` wrapper is
    /// **W1's own**, added by `models::RetryCommon` where Kestra has none —
    /// line 59 of the same Java file, `protected Property<Duration> timeout;`,
    /// is what that sentence describes, and `timeout` is a different field
    /// with a different owner (see [`PlanTask::timeout`]).
    ///
    /// So a pure builder can read a retry after all, and does: the source text
    /// of each duration is parsed as an ISO-8601 literal by
    /// [`retry::parse_duration`](super::retry::parse_duration). A field whose
    /// source is not a literal leaves the whole policy `None` — measured
    /// unreachable from this corpus by
    /// `every_corpus_retry_is_written_in_literals`.
    pub retry: Option<RetryPolicy>,
    /// **Still always `None`, and after Task 7 there is only one reason
    /// left.** The `Property<Duration>` half of this comment is the only place
    /// it is true — `Task.java:59` really is `protected Property<Duration>
    /// timeout;` — but it is not the reason, because W3a never enforces this
    /// field at all. Measured: in this artifact `timeout:` is enforced by the
    /// **worker**
    /// (`worker/src/main/java/io/kestra/worker/processors/internals/WorkerTaskCallable.java:61`
    /// wraps `task.run` in `callWithTimeout`), not by the executor. Kestra's own
    /// validator says so out loud —
    /// `core/src/main/java/io/kestra/core/services/FlowService.java:574` warns
    /// "cannot use the 'timeout' property as it's only relevant for runnable
    /// tasks", with `Pause` the single exception. So the only `timeout:`
    /// `advance()` will ever enforce is a `Pause`'s, and W4's worker owns the
    /// rest.
    pub timeout: Option<Duration>,
    /// This task's own `errors:`.
    ///
    /// **Populated from Task 7, which had to widen W1's model first.** Task 3
    /// recorded this as a model gap rather than a decision: `models::Sequential`
    /// and `models::Parallel` dropped the Java class's `errors`/`finally`
    /// fields, so there was nothing in the parsed flow to read.
    ///
    /// **Task 7 added `errors` to FIVE model types**, not the two an earlier
    /// draft of this comment named: `Sequential`, `Parallel`, `If`, `Switch`
    /// and `Dag`. Java declares the field on all five — `AbstractBranch` for
    /// the first two, and `If.java:111`, `Switch.java:112`, `Dag.java:106`,
    /// which are not `AbstractBranch` at all and carry their own — and
    /// [`build_plan`] flattens all five. The draft said "and only those two,
    /// the other `AbstractBranch` ports stay opaque `Runnable`s"; both halves
    /// were false in the same commit that wrote them, and a reader who trusted
    /// it would conclude the `If`/`Switch`/`Dag` arms do not exist.
    ///
    /// **The types that really are left out are `Loop` and every opaque one,
    /// and only one of those is a gap.** `AllowFailure`, `Pause` and
    /// `WorkingDirectory` are planned [`TaskKind::Runnable`], so their children
    /// get no run and a field on them would indeed be unreadable. **`Loop` is
    /// not opaque** — it has a [`TaskKind::Loop`] — and `models::Loop` has no
    /// `errors` field, so three corpus flows' lists (`valids/all-flowable`,
    /// `valids/finally-loop`, `valids/finally-loop-parallel`) are invisible
    /// here. All three are baselined, so nothing banks on it; it is recorded as
    /// a gap by `every_corpus_errors_list_is_on_a_flowable_egret_plans` and in
    /// `oracle-exec/README.md`'s unclaimed table rather than left to be
    /// rediscovered.
    ///
    /// `finally` is left out of all of them, because no W3a task implements it
    /// and a second dead field is not a smaller version of the same fix.
    ///
    /// Empty means Kestra's `errors: null`, and the two are not distinguishable
    /// here for the same reason `Switch::default` records: W1's field is a bare
    /// `Vec<Task>`. Kestra *can* tell them apart —
    /// `findTaskDependingFlowState` branches on `resolvedErrors == null` — but
    /// only where `finally` is also present, which W3a does not implement, so
    /// the two spellings reach the same answer everywhere this executor can
    /// look.
    pub errors: Vec<TaskIx>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskKind {
    /// Anything the executor does not see into: it hands the task to a worker
    /// and waits for a terminal state. Tasks 3 and 4 put **every** task type
    /// here except the three flowables they implement, including the flowables
    /// Tasks 5-8 own — see [`build_plan`] for why that is the safe default and
    /// not an approximation.
    Runnable {
        type_id: String,
    },
    Sequential {
        tasks: Vec<TaskIx>,
    },
    /// `io.kestra.plugin.core.flow.Parallel`.
    ///
    /// `concurrency` is Kestra's `concurrent`, **`None` meaning unlimited** —
    /// which is what Kestra's own `0` means ("If the value is `0`, no limit
    /// exist and all tasks will start at the same time",
    /// `core/src/main/java/io/kestra/plugin/core/flow/Parallel.java`), so `0`
    /// is normalised to `None` here rather than kept as a second spelling of
    /// the same thing.
    ///
    /// It is `Option<u32>` and not `Property<i32>` because [`build_plan`] is
    /// pure: Kestra renders `concurrent` through a `RunContext`, and a builder
    /// that cannot render can only read a literal. Every `concurrent:` in the
    /// 345-flow corpus **is** a literal — measured, and re-measured on every
    /// run by `every_corpus_concurrent_is_a_literal_integer` — so nothing this
    /// oracle compares depends on the fallback.
    Parallel {
        tasks: Vec<TaskIx>,
        concurrency: Option<u32>,
    },
    /// `io.kestra.plugin.core.flow.If`.
    ///
    /// `condition` is the author's expression, **unrendered**. [`advance`] is
    /// pure and cannot render, so the caller renders it and writes the result
    /// into [`TaskRun::rendered`](crate::executor::TaskRun::rendered) — the
    /// field Task 3 added for exactly this. Holding the source here rather
    /// than a `bool` is what makes "not rendered yet" representable at all.
    ///
    /// `then_` and `else_` are each driven as a **sequential** list, which is
    /// Kestra's shape (`If.resolveNexts` calls
    /// `FlowableUtils.resolveSequentialNexts`) and is measured: an `If` whose
    /// `then` is `[a, b(Fail), c]` produced the taskruns `if(FAILED),
    /// a(SUCCESS), b(FAILED)` on the running artifact — `c` was never created.
    If {
        condition: String,
        then_: Vec<TaskIx>,
        else_: Vec<TaskIx>,
    },
    /// `io.kestra.plugin.core.flow.Switch`.
    ///
    /// `value` is unrendered for the same reason as
    /// [`If::condition`](TaskKind::If); the caller supplies the render.
    ///
    /// `cases` is a `Vec` of pairs rather than a map because the plan holds
    /// indices and the lookup is a single equality against the rendered value;
    /// Kestra's own `childTasks` is `cases.entrySet().stream().filter(entry ->
    /// entry.getKey().equals(value)).findFirst()`, an equality on the key and
    /// nothing else. Key order is W1's `BTreeMap` order and no rule reads it:
    /// exactly one case is ever driven.
    ///
    /// **`default` empty means Kestra's `defaults: null`, and that is a
    /// measured limit rather than a modelling choice.** W1's
    /// `models::Switch::defaults` is a bare `Vec<Task>`, so an absent
    /// `defaults:` and an explicit `defaults: []` arrive here identically. On
    /// the running artifact they are *not* the same: with no matching case, an
    /// absent `defaults:` ends the `Switch` `FAILED`
    /// (`Switch.childTasks` returns `null`, and `FlowableUtils.resolveState`
    /// logs "No task found" and returns `FAILED` — measured on
    /// `valids/fail-on-switch` and `valids/switch-impossible`), while
    /// `defaults: []` leaves the `Switch` `RUNNING` for ever (measured live:
    /// a probe flow was still `RUNNING` after 45 s with only the `sw` taskrun).
    /// No corpus flow writes `defaults: []`, and `defaults: null` is refused by
    /// W1's parser today, so the two spellings this crate can tell apart are
    /// the two that occur.
    Switch {
        value: String,
        cases: Vec<(String, Vec<TaskIx>)>,
        default: Vec<TaskIx>,
    },
    /// `io.kestra.plugin.core.flow.Dag` — [`Parallel`](TaskKind::Parallel) plus
    /// a dependency predicate.
    ///
    /// There is no `concurrency` here even though Kestra's `Dag` carries the
    /// same `concurrent` field, and that is a **gap, not a decision**: no
    /// corpus flow sets it on a `Dag` (measured by the same test), so a cap
    /// implemented here would be a rule nothing could ever exercise. Task 8 or
    /// whoever first needs it should add it with a test that fails without it.
    Dag {
        nodes: Vec<DagNode>,
    },
    /// `io.kestra.plugin.core.flow.Loop` — Kestra's `ForEach` under its real
    /// class name (there is no `ForEach.java` in this checkout; W1's
    /// `models::Loop` records the same correction).
    ///
    /// **It does not iterate inside the execution.** Each value produces a
    /// separate **sub-execution**, and the body's tasks never get a run in the
    /// parent: measured over the whole golden, the 41 entries that carry a
    /// sub-execution carry 142 of them between them, and not one loop body task
    /// appears in its parent's `taskRunList`. That is why [`Plan::is_loop_body`]
    /// exists and why [`advance`](super::advance) never descends into `tasks`.
    ///
    /// `values` is the author's expression, **unrendered**, exactly as
    /// [`If::condition`](TaskKind::If) is: `advance()` is pure, so the caller
    /// renders it and writes the result into
    /// [`TaskRun::rendered`](crate::executor::TaskRun::rendered). It is read at
    /// `CREATED` rather than at `RUNNING` — see
    /// [`loop_::open`](super::flowable::loop_::open) — because on this artifact
    /// an empty `values` never reaches `RUNNING` at all.
    ///
    /// `concurrency_limit` is Kestra's `concurrencyLimit`, kept in Kestra's own
    /// spelling rather than normalised the way
    /// [`Parallel::concurrency`](TaskKind::Parallel) is: **`0` means unlimited,
    /// `1` means full serialisation, `n` means up to `n` iterations at once**
    /// (`Loop.java:225-229`). The **default is `1`**, not `0` — `Loop.java:233`
    /// is `@Builder.Default private Integer concurrencyLimit = 1;`, W1's
    /// `default_concurrency_limit()` ports it, and
    /// `the_default_concurrency_limit_is_one_and_the_corpus_can_tell`
    /// re-derives it from Kestra's own traces rather than from either reading.
    /// A `0` normalised to `None` here would lose the distinction the field
    /// exists for, since `1` and "no limit" are different numbers of spawns.
    Loop {
        values: String,
        tasks: Vec<TaskIx>,
        concurrency_limit: u32,
    },
}

/// One node of a [`TaskKind::Dag`]: a task and the sibling nodes it waits for.
///
/// `depends_on` holds **[`TaskIx`]es, resolved at plan-build time**. Kestra
/// carries the author's id strings and re-resolves them on every
/// `resolveNexts` call (`Dag.java`'s `controlTask()`), which is where its
/// "Not existing task id in dependency" error comes from at run time as well
/// as at publish time. Resolving once, in [`build_plan`], makes that error
/// unrepresentable later: an unresolvable id cannot become a `DagNode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DagNode {
    pub task: TaskIx,
    pub depends_on: Vec<TaskIx>,
}

/// A task's, an ancestor's or the flow's `retry:`, as far as a pure builder
/// can read it — which, measured, is all of it. See [`PlanTask::retry`].
///
/// **`max_attempts` is `Option<u32>` and Task 3's draft had a bare `u32`.**
/// `AbstractRetry.maxAttempts` is a nullable `Integer` and its absence is not
/// "zero": `TaskRun.nextRetryDate` guards with `retry.getMaxAttempts() != null
/// && this.attemptNumber() >= retry.getMaxAttempts()`, so an absent
/// `maxAttempts` means *no attempt cap at all* and the retry is bounded only by
/// `maxDuration`. Two corpus flows are written that way —
/// `valids/retry-failed-flow-duration` and `valids/retry-failed-task-duration`
/// — and a `0` default would have stopped both before their first retry.
///
/// **`interval` became [`RetryDelay`] for the same class of reason.** Only
/// `constant` has an `interval`; `exponential` has `interval`/`maxInterval`/
/// `delayFactor` and `random` has `minInterval`/`maxInterval`, and the corpus
/// uses all three (`valids/retry-expo`, `valids/retry-random`). Flattening
/// them onto one `Duration` would have made two of the three unrepresentable.
#[derive(Clone, Debug, PartialEq)]
pub struct RetryPolicy {
    /// Kestra spells it `maxAttempts`, measured. `None` is Kestra's own
    /// `null` — no cap.
    pub max_attempts: Option<u32>,
    /// How long to wait before the next attempt. A span, not a clock read.
    pub delay: RetryDelay,
    pub max_duration: Option<Duration>,
    pub behavior: RetryBehavior,
    /// `AbstractRetry.warningOnRetry`, default `false`. **Read by nothing in
    /// W3a and that is stated rather than hidden**: it turns a retried task's
    /// eventual success into `WARNING`, which [`RunState`](super::RunState)
    /// cannot hold — `valids/retry-success` and
    /// `valids/task-allow-failure-runnable` are the two entries that need it
    /// and both stay baselined for that reason.
    pub warning_on_retry: bool,
}

/// `AbstractRetry`'s three `@JsonSubTypes`, reduced to the one method
/// `advance` calls: `nextRetryDate(attemptCount, lastAttempt)`.
#[derive(Clone, Debug, PartialEq)]
pub enum RetryDelay {
    /// `Constant.java:38` — `lastAttempt.plus(interval)`, ignoring the count.
    Constant { interval: Duration },
    /// `Exponential.java:51` — `interval * delayFactor^(attemptCount - 1)`,
    /// capped at `maxInterval`. `delay_factor` is `None` for Kestra's own
    /// `delayFactor == null ? 2d : delayFactor`.
    Exponential {
        interval: Duration,
        max_interval: Duration,
        delay_factor: Option<f64>,
    },
    /// `Random.java:42` — `random.nextLong(minInterval, maxInterval)`.
    ///
    /// **`advance` is pure and takes the lower bound**, which is a divergence
    /// and is recorded as one. Nothing this oracle compares can see it: the
    /// normalised trace carries no clock, the delay reaches only
    /// [`Command::ScheduleRetry`](super::Command)'s `not_before`, and the
    /// oracle's own tier 4 jumps `now` to whatever that is. What a wrong value
    /// *could* change is how many attempts fit inside a `maxDuration`, so the
    /// choice is not free in general — `valids/retry-random` is bounded by
    /// `maxAttempts: 3` with `maxDuration: PT15S` against a worst case of
    /// 3 × 0.5 s, so it cannot be reached there.
    /// `a_random_retry_delay_is_inside_kestras_own_bounds` pins the bounds so a
    /// later task that wants a real RNG has the assertion already written.
    Random {
        min_interval: Duration,
        max_interval: Duration,
    },
}

/// Wire spellings `RETRY_FAILED_TASK` / `CREATE_NEW_EXECUTION`, measured from
/// the eight `valids/retry-*.yml` flows Task 1 recovered: the four
/// `retry-failed-*` terminate `FAILED` and the four `retry-new-execution-*`
/// terminate `RETRIED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryBehavior {
    RetryFailedTask,
    CreateNewExecution,
}

/// Why a flow cannot be planned — the refusal the `invalids` corpus is about.
///
/// Task 8 asserts on the message per group, so that a refusal which becomes a
/// *different* refusal is visible instead of silently still passing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanError {
    pub message: String,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlanError {}

/// The deepest task nesting [`build_plan`] will descend, and the derived bound
/// on its recursion over author input.
///
/// The arithmetic is in the constant, over a measured cost.
/// `the_depth_bound_is_derived_from_the_deepest_corpus_flow` in
/// `executor/mod.rs` re-measures `MEASURED_MAX_CORPUS_DEPTH` from the 345
/// committed corpus flows on every run and fails if it moves, so this is a
/// bound over a measurement rather than over a memory of one.
///
/// **The measurement is a lower bound**: W1's model drops every control-flow
/// type's `errors`/`finally`, which Java's `AbstractBranch.allChildTasks()`
/// counts, so a subtree hanging off a task-level `errors:` is invisible to the
/// walk *and* to this builder alike. The 8× is what absorbs that, and the
/// figure may rise when Task 7 widens the model. See the test's doc.
pub(super) const MEASURED_MAX_CORPUS_DEPTH: usize = 6;
pub(super) const MAX_TASK_DEPTH: usize = 8 * MEASURED_MAX_CORPUS_DEPTH;

/// Kestra's own classes for the three flowables Tasks 3 and 4 implement.
const SEQUENTIAL_TYPE: &str = "io.kestra.plugin.core.flow.Sequential";
const PARALLEL_TYPE: &str = "io.kestra.plugin.core.flow.Parallel";
const DAG_TYPE: &str = "io.kestra.plugin.core.flow.Dag";
const IF_TYPE: &str = "io.kestra.plugin.core.flow.If";
const SWITCH_TYPE: &str = "io.kestra.plugin.core.flow.Switch";
const LOOP_TYPE: &str = "io.kestra.plugin.core.flow.Loop";

/// Build a [`Plan`] from a flow's YAML source, or refuse it.
///
/// # What this refuses, and what it deliberately does not
///
/// It refuses six things: source W1's parser cannot read; nesting deeper than
/// [`MAX_TASK_DEPTH`]; a `Dag` whose `dependsOn` names a task that is not one
/// of its own nodes, or whose nodes form a cycle; an `If` with no `condition`;
/// and a `Switch` with no `value`. The `Dag` pair is Task 4's and is exactly
/// the two checks Kestra's own `DagTaskValidator` makes; the `If`/`Switch` pair
/// is Task 5's and is the same shape as the `Dag` node with no task — a field
/// that is `Option` here only because `@NotNull` is bean validation rather than
/// parsing, and whose absence leaves the task with nothing to branch on. All
/// five are refusals about *this flow*, not about what egret has implemented,
/// which is what separates them from the restraint below.
///
/// It refuses **nothing else** — in particular it does not refuse a flow
/// because it uses a flowable no task has implemented yet, and that restraint
/// is load-bearing rather than lazy.
///
/// The oracle's `refused` verdict compares one thing: that `build_plan` returns
/// `Err`. A builder that refused every unimplemented flowable would return
/// `Err` for most of the `invalids` corpus too — and the ratchet would report
/// those entries as *matching*, inviting a maintainer to bank 28 refusals that
/// egret gets right for a reason that has nothing to do with the flow being
/// invalid. Task 8 owns refusal. Until then an unimplemented flowable is
/// planned as an opaque [`TaskKind::Runnable`], which produces a visibly wrong
/// trace — the parent is there and its children are not — and so stays honestly
/// baselined.
///
/// That opacity is applied to `AllowFailure` and `WorkingDirectory` too, even
/// though both `extend Sequential` in Java and would "work" if mapped to
/// [`TaskKind::Sequential`]. `AllowFailure` turns a child's failure into
/// `WARNING`, which `RunState` cannot hold; `WorkingDirectory` hands its whole
/// subtree to one worker. Mapping either would trade a visible mismatch for a
/// plausible-looking wrong trace, which is the worse of the two.
pub fn build_plan(source: &str) -> Result<Plan, PlanError> {
    let flow = crate::spec::parse(source).map_err(|e| PlanError {
        message: match (e.line, e.column) {
            (Some(l), Some(c)) => {
                format!("cannot parse flow at line {l} column {c}: {}", e.message)
            }
            _ => format!("cannot parse flow: {}", e.message),
        },
    })?;

    let mut tasks = Vec::new();
    let roots = flatten_list(&mut tasks, &flow.tasks, 1)?;
    let errors = flatten_list(&mut tasks, &flow.errors, 1)?;
    Ok(Plan {
        tasks,
        roots,
        errors,
        retry: super::retry::policy_of(flow.retry.as_ref()),
    })
}

fn flatten_list(
    out: &mut Vec<PlanTask>,
    tasks: &[Task],
    depth: usize,
) -> Result<Vec<TaskIx>, PlanError> {
    tasks
        .iter()
        .map(|t| flatten_one(out, t, depth))
        .collect::<Result<Vec<_>, _>>()
}

fn flatten_one(out: &mut Vec<PlanTask>, task: &Task, depth: usize) -> Result<TaskIx, PlanError> {
    if depth > MAX_TASK_DEPTH {
        return Err(PlanError {
            message: format!(
                "task id '{}' nests deeper than the {MAX_TASK_DEPTH} levels this \
                 executor will descend (the deepest flow in Kestra's own corpus nests \
                 {MEASURED_MAX_CORPUS_DEPTH})",
                task.id
            ),
        });
    }

    // Reserved before the children are flattened, so `tasks` reads in
    // pre-order and a child's index is always greater than its parent's.
    let ix = out.len();
    out.push(PlanTask {
        id: task.id.clone(),
        kind: TaskKind::Runnable {
            type_id: task.r#type.clone(),
        },
        retry: super::retry::policy_of(task.retry.as_ref()),
        timeout: None,
        errors: Vec::new(),
    });

    // Kestra dispatches on the wire `type`, and so does this: W1's
    // `models::TaskKind::Sequential` is reached by that type string and by no
    // other, but matching on the parsed variant alone would also catch a
    // future model change that maps a second type onto it.
    match (&task.kind, task.r#type.as_str()) {
        // `errors` is flattened AFTER `tasks` for both, so `out` stays
        // pre-order and the error subtree sits behind the task subtree it
        // handles -- which is also the order Kestra creates their runs in.
        (ModelKind::Sequential(seq), SEQUENTIAL_TYPE) => {
            let children = flatten_list(out, &seq.tasks, depth + 1)?;
            out[ix].errors = flatten_list(out, &seq.errors, depth + 1)?;
            out[ix].kind = TaskKind::Sequential { tasks: children };
        }
        (ModelKind::Parallel(par), PARALLEL_TYPE) => {
            let children = flatten_list(out, &par.tasks, depth + 1)?;
            out[ix].errors = flatten_list(out, &par.errors, depth + 1)?;
            out[ix].kind = TaskKind::Parallel {
                tasks: children,
                concurrency: literal_concurrency(par.concurrent.as_ref()),
            };
        }
        (ModelKind::If(cond), IF_TYPE) => {
            // `condition` is `Option<String>` in W1's model because `@NotNull`
            // is bean validation and not parsing. An `If` with no condition has
            // nothing to decide on -- the same shape as a `Dag` node with no
            // task, and refused for the same reason: it cannot be planned, not
            // because egret has not implemented something. This Kestra refuses
            // it at publish: `tasks[if].condition: must not be null`, measured
            // on `invalids/if-without-condition`.
            let condition = cond.condition.clone().ok_or_else(|| PlanError {
                message: format!("if '{}' has no condition to branch on", task.id),
            })?;
            let then_ = flatten_list(out, &cond.then, depth + 1)?;
            let else_ = flatten_list(out, &cond.r#else, depth + 1)?;
            out[ix].errors = flatten_list(out, &cond.errors, depth + 1)?;
            out[ix].kind = TaskKind::If {
                condition,
                then_,
                else_,
            };
        }
        (ModelKind::Switch(sw), SWITCH_TYPE) => {
            // Same rule, same reason: `value` is `@NotNull` in Java, and a
            // `Switch` with nothing to match on cannot be planned.
            let value = sw
                .value
                .as_ref()
                .map(|v| v.source().to_string())
                .ok_or_else(|| PlanError {
                    message: format!("switch '{}' has no value to match on", task.id),
                })?;
            let mut cases = Vec::with_capacity(sw.cases.len());
            for (key, tasks) in &sw.cases {
                cases.push((key.clone(), flatten_list(out, tasks, depth + 1)?));
            }
            let default = flatten_list(out, &sw.defaults, depth + 1)?;
            out[ix].errors = flatten_list(out, &sw.errors, depth + 1)?;
            out[ix].kind = TaskKind::Switch {
                value,
                cases,
                default,
            };
        }
        (ModelKind::Loop(lp), LOOP_TYPE) => {
            // `values` is `@NotNull` in Java, and a `Loop` with nothing to
            // iterate cannot be planned -- the same shape and the same reason
            // as an `If` with no `condition`. Unlike that one this refusal has
            // no corpus flow behind it: every one of the 43 corpus flows that
            // uses a `Loop` writes `values:`
            // (`every_corpus_loop_declares_its_values_and_a_readable_limit`
            // re-derives that on every run), so
            // `a_loop_with_no_values_is_refused_by_name` is what holds it.
            let values = lp
                .values
                .as_ref()
                .map(|v| v.source().to_string())
                .ok_or_else(|| PlanError {
                    message: format!("loop '{}' has no values to iterate", task.id),
                })?;
            let children = flatten_list(out, &lp.tasks, depth + 1)?;
            out[ix].kind = TaskKind::Loop {
                values,
                tasks: children,
                concurrency_limit: literal_concurrency_limit(lp.concurrency_limit),
            };
        }
        (ModelKind::Dag(dag), DAG_TYPE) => {
            // `task` is `Option<Task>` in W1's model because `@NotNull` is bean
            // validation and not parsing. A node with no task has no id to be
            // depended on and nothing to run, so it cannot be planned — and
            // stripping the `Option` here is what lets everything below work in
            // terms of a task rather than a maybe-task.
            let mut nodes = Vec::with_capacity(dag.tasks.len());
            for node in &dag.tasks {
                let child = node.task.as_ref().ok_or_else(|| PlanError {
                    message: format!("dag '{}' has a node with no task", task.id),
                })?;
                nodes.push((child, node.depends_on.as_slice()));
            }
            out[ix].kind = TaskKind::Dag {
                nodes: flatten_dag(out, &task.id, &nodes, depth + 1)?,
            };
            out[ix].errors = flatten_list(out, &dag.errors, depth + 1)?;
        }
        _ => {}
    }

    Ok(ix)
}

/// Kestra's `concurrent`, as far as a builder with no `RunContext` can read it.
///
/// `None` out is **unlimited**, and there are three ways to reach it, only one
/// of which is a limitation:
///
/// - the field is absent, so W1's model supplies Kestra's own default of `0`;
/// - it is written `0`, which is Kestra's spelling of "no limit";
/// - it is an expression, or an explicit `null`, or a value no `u32` can hold.
///
/// Only the third is a gap, and it is measured to be unreachable from this
/// corpus: `every_corpus_concurrent_is_a_literal_integer` re-derives, from all
/// 345 flows on every run, that every `concurrent:` written anywhere in it is a
/// literal. Kestra's own cap is guarded by `concurrent > 0`, so "cannot read
/// it" and "not capped" land in the same place there too.
fn literal_concurrency(source: Option<&crate::models::Property<i32>>) -> Option<u32> {
    match source?.source().trim().parse::<u32>() {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(n),
    }
}

/// Kestra's `Loop.concurrencyLimit`, as far as a pure builder can read it.
///
/// **The default is `1`, and it is not the same field as
/// [`Parallel::concurrent`](TaskKind::Parallel).** `Loop.java:233` is
/// `@Builder.Default private Integer concurrencyLimit = 1;` — full
/// serialisation — where `Parallel.java`'s `concurrent` defaults to `0`, which
/// is unlimited. The two look alike and mean opposite things at their defaults,
/// so the number here comes from W1's own port of the Java default
/// (`models::task::default_concurrency_limit`) and is re-derived from Kestra's
/// traces by `the_default_concurrency_limit_is_one_and_the_corpus_can_tell`.
///
/// `None` is reached only by an explicit `concurrencyLimit: null`, which W1
/// accepts (bean validation is not parsing) and which Kestra refuses at publish
/// (`@NotNull`). There is nothing to iterate differently for it, so it takes the
/// class default; no corpus flow writes it. A negative value is not
/// representable as a count and takes the default too — Kestra has no guard for
/// one either (`Math.min(negative, size)` would reach `subList(0, negative)`),
/// and `every_corpus_loop_declares_its_values_and_a_readable_limit` measures
/// that no corpus flow writes one.
fn literal_concurrency_limit(source: Option<i32>) -> u32 {
    match source {
        Some(n) if n >= 0 => n as u32,
        _ => 1,
    }
}

/// Flatten a `Dag`'s nodes, refusing the two things Kestra's own
/// `DagTaskValidator` refuses, in its order: a `dependsOn` naming a task that
/// is not a sibling node, then a cycle.
///
/// **Both refusals belong here rather than in `advance()`**, which is the
/// plan's instruction and also the artifact's shape: Kestra runs the same two
/// checks at publish time (`DagTaskValidator.isValid`) and again at run time
/// (`Dag.controlTask()`), and the publish-time one is why
/// `invalids/dag-cyclicdependency` and `invalids/dag-notexist-task` are
/// `refused` in the golden at all. Resolving `dependsOn` to indices here makes
/// the run-time copy unrepresentable.
fn flatten_dag(
    out: &mut Vec<PlanTask>,
    dag_id: &str,
    nodes: &[(&Task, &[String])],
    depth: usize,
) -> Result<Vec<DagNode>, PlanError> {
    let ids: Vec<&str> = nodes.iter().map(|(t, _)| t.id.as_str()).collect();

    // Kestra: "Not existing task id in dependency: ..." --
    // `DagTaskValidator.isValid`, via `Dag.dagCheckNotExistTask`.
    let unknown: Vec<&str> = nodes
        .iter()
        .flat_map(|(_, deps)| deps.iter())
        .map(String::as_str)
        .filter(|d| !ids.contains(d))
        .collect();
    if !unknown.is_empty() {
        return Err(PlanError {
            message: format!(
                "dag '{dag_id}' depends on task id(s) that are not nodes of it: {}",
                unknown.join(", ")
            ),
        });
    }

    // Kestra: "Cyclic dependency detected: ..." -- `Dag.dagCheckCyclicDependencies`.
    let cyclic: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| reaches_itself(id, nodes))
        .collect();
    if !cyclic.is_empty() {
        return Err(PlanError {
            message: format!(
                "dag '{dag_id}' has a cyclic dependency through task id(s): {}",
                cyclic.join(", ")
            ),
        });
    }

    // Flattened in declaration order, so `out` stays pre-order and a node's own
    // subtree sits between it and the next node.
    let mut plan_ix = Vec::with_capacity(nodes.len());
    for (task, _) in nodes {
        plan_ix.push(flatten_one(out, task, depth)?);
    }

    Ok(nodes
        .iter()
        .zip(&plan_ix)
        .map(|((_, deps), &ix)| DagNode {
            task: ix,
            depends_on: deps
                .iter()
                .map(|d| {
                    let pos = ids.iter().position(|id| id == d).expect(
                        "flatten_dag resolves dependsOn only after the \
                             not-a-node check above has passed",
                    );
                    plan_ix[pos]
                })
                .collect(),
        })
        .collect())
}

/// Whether `start` is reachable from itself by following `dependsOn`.
///
/// Kestra's `Dag.hasCycle`, iteratively — and iteratively **on purpose**: the
/// programme requires a derived bound on every recursive descent over author
/// input, and a worklist whose `visited` set admits each node id at most once
/// terminates after at most `nodes.len()` pops for any input at all, cyclic
/// ones included. That is a bound by construction rather than a constant to
/// justify, which is the better of the two where it is available.
fn reaches_itself(start: &str, nodes: &[(&Task, &[String])]) -> bool {
    let deps_of = |id: &str| -> &[String] {
        nodes
            .iter()
            .find(|(t, _)| t.id == id)
            .map_or(&[][..], |(_, deps)| deps)
    };
    let mut visited: Vec<&str> = Vec::new();
    let mut stack: Vec<&str> = vec![start];
    while let Some(current) = stack.pop() {
        for dep in deps_of(current) {
            if dep == start {
                return true;
            }
            if !visited.contains(&dep.as_str()) {
                visited.push(dep.as_str());
                stack.push(dep.as_str());
            }
        }
    }
    false
}
