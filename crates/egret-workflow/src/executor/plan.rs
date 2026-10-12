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
    pub(super) fn children_of(&self, ix: TaskIx) -> Vec<TaskIx> {
        let own_errors = self.tasks[ix].errors.iter().copied();
        let mut out: Vec<TaskIx> = own_errors.collect();
        out.extend(self.direct_task_children(ix));
        out
    }

    /// [`Plan::children_of`] without the `errors:` list — the `tasks:` half
    /// alone.
    fn direct_task_children(&self, ix: TaskIx) -> Vec<TaskIx> {
        match &self.tasks[ix].kind {
            TaskKind::Runnable { .. } | TaskKind::Executable { .. } => Vec::new(),
            TaskKind::Sequential { tasks }
            | TaskKind::Parallel { tasks, .. }
            | TaskKind::Loop { tasks, .. }
            // W8 Task 4. A `LoopUntil`'s children ARE in the plan and are its
            // own `tasks:` -- unlike a `Loop`'s, which are planned here and
            // then run in a different execution, and unlike a `Pause`'s, which
            // a planned `Pause` never has.
            | TaskKind::LoopUntil { tasks, .. } => tasks.clone(),
            TaskKind::If { then_, else_, .. } => then_.iter().chain(else_).copied().collect(),
            TaskKind::Switch { cases, default, .. } => cases
                .iter()
                .flat_map(|(_, t)| t.iter())
                .chain(default)
                .copied()
                .collect(),
            TaskKind::Dag { nodes } => nodes.iter().map(|n| n.task).collect(),
            // A planned `Pause` has none: `build_plan` only assigns this kind
            // to a `Pause` whose `tasks:` is empty (`pause_gap`), so an empty
            // list here is the whole truth rather than a walk this kind is
            // missing.
            TaskKind::Pause { .. } => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanTask {
    /// The author's task id, as written.
    pub id: String,
    pub kind: TaskKind,
    /// This task's `runIf:`, **unrendered** — W3c.
    ///
    /// Carried for every kind, flowable and `Runnable` alike, because Kestra
    /// checks it in one place for all of them: `ExecutionEventMessageHandler`
    /// tests `TruthUtils.isTruthy(render(getRunIf()))` *before* it emits
    /// `SUBMITTED` for a runnable and before it moves a flowable to `RUNNING`
    /// (`executor/src/main/java/io/kestra/executor/handler/ExecutionEventMessageHandler.java:269`).
    /// Falsy means the run goes `CREATED → SKIPPED` and its children — if it
    /// has any — never exist. That is why the check lives at
    /// [`flowable::open`](super::flowable::open), which is every created run's
    /// single door, rather than beside any one flowable.
    ///
    /// **Unrendered for the same reason [`TaskKind::If::condition`] is**:
    /// `advance()` is pure, so the caller renders it and writes the answer into
    /// [`TaskRun::rendered_run_if`](super::TaskRun::rendered_run_if). `None`
    /// there means "not rendered yet", not "falsy" — reading it as falsy would
    /// silently skip every task the caller had not got to.
    pub run_if: Option<String>,
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
    /// **`None` for every kind but one since W8 Task 3**, which is the task the
    /// last paragraph below was addressed to: a [`TaskKind::Pause`] carries the
    /// `timeout:` its author wrote, parsed as an ISO-8601 literal, and
    /// [`super::flowable::pause`] ends the pause `FAILED` when it elapses.
    /// Everything below is the record of why it was `None` for EVERY kind, and
    /// stays true of every other one.
    ///
    /// The `Property<Duration>` half of this comment is the only place
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
    /// rest. **W8 Task 3 is where that exception stopped being a prediction**,
    /// and it is worth 1 of its 8 banked entries on its own
    /// (`valids/pause-timeout`): `every_corpus_pause_timeout_is_a_literal`
    /// derives that all three corpus declarations are literals a pure builder
    /// can read, and `the_bound_that_was_written_decides_the_terminal` pins
    /// what one decides.
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
    /// and only one of those is a gap.** `AllowFailure`, `LoopUntil` (and
    /// `WaitFor`, which is Kestra's alias for it and **not** egret's — see
    /// [`build_plan`]) and `WorkingDirectory` are planned
    /// [`TaskKind::Runnable`], so their children
    /// get no run and a field on them would indeed be unreadable. (Since
    /// no-silent-stall Task 1 every such task is also a validation error, so
    /// [`build_plan`] refuses the flow; see its doc.)
    /// **`Pause` was the fourth name in that list until W8 Task 3**, which gave
    /// it [`TaskKind::Pause`] — and a `Pause`'s `errors:` is still unreadable
    /// here, for a different reason that must not be merged with the one above:
    /// the field is not missing, the whole TASK is opaque, because
    /// [`pause_gap`] plans any `Pause` declaring one back to a `Runnable`.
    /// **This sentence omitted `LoopUntil`**, which is opaque for the same
    /// reason as the other two typed ones and stalls nine baselined entries;
    /// W8 Task 1 swept every list in this crate that claims to enumerate the
    /// opaque types against `every_other_flowable_stays_opaque`, which is the
    /// derivation. **Eight lists — seven prose and the derivation they are
    /// checked against — and all seven prose lists disagreed**: three omit
    /// `LoopUntil` and four still name `Subflow`, which stopped being
    /// opaque at W6 Task 3. This sentence first read "six disagreed … only
    /// `worker::timeout`'s module doc had it right", and then "none correct",
    /// which contradicted itself — the eighth list is the derivation and it is
    /// correct by construction. `worker::timeout` was the one list the sweep
    /// never re-read, and it names `Subflow` too. **The sweep's
    /// own headline was an instance of the class it was sweeping for.**
    ///
    /// **The mechanism is worth more than the instance, because it is ordinary:
    /// `grep` prints LINES and a prose list WRAPS.** `Subflow` ended
    /// `timeout.rs`'s line 95 and every needle the sweep searched for —
    /// `WorkingDirectory`, `AllowFailure`, `Pause` — sat on line 96, so the
    /// match printed line 96 and the list's first member was never on screen.
    /// Nothing was truncated; the line was 79 characters. A needle set drawn
    /// from the members you expect cannot see a member you do not.
    ///
    /// **And the structural half predicts WHICH list will be wrong**:
    /// `worker::timeout` is the only list-bearing file the sweep did not edit,
    /// so the one list it merely *classified* is the one that was wrong. The
    /// asymmetry is why that is not a coincidence — **a transcription that
    /// loses a member can acquit a wrong list, and only the acquittal is
    /// consequential.** Losing a legitimate member instead convicts a correct
    /// list, which costs a second look and nothing else. Re-read the lists you
    /// are not changing first. **`Loop` is
    /// not opaque** — it has a [`TaskKind::Loop`] — and `models::Loop` has no
    /// `errors` field, so **four** corpus flows' lists are invisible here:
    /// `valids/all-flowable`, `valids/finally-loop`,
    /// `valids/finally-loop-parallel` and `invalids/loop-switch-failed`, which
    /// is a `runs` entry despite its directory. This sentence said *three* and
    /// named the first three, while the test written in the same commit
    /// asserts all four (`every_corpus_errors_list_is_on_a_flowable_egret_plans`);
    /// the test is the derivation and the prose now agrees with it. All four
    /// are baselined, so nothing banks on it; it is recorded as a gap by that
    /// test and in `oracle-exec/README.md`'s unclaimed table rather than left
    /// to be rediscovered.
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
    /// This task's `allowFailure:` and `allowWarning:` — W8 Task 5.
    ///
    /// **Carried for every kind, flowable and `Runnable` alike, exactly like
    /// [`PlanTask::run_if`]**, and for the same reason: they are `Task.java`'s
    /// fields, not any one flowable's, and the artifact reads them at four
    /// different sites that between them cover both. See
    /// [`crate::executor::allow_failure`] for the sites and for the two
    /// spellings the artifact gives the same table.
    ///
    /// Plain `bool`s rather than `Option<bool>`: `Task.java`'s own default is
    /// `false` and `models::Task` has already collapsed an absent key onto it,
    /// so there is no third state to carry.
    pub allow_failure: bool,
    pub allow_warning: bool,
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
    /// A task the executor does not see into either, but which it **runs
    /// itself** rather than handing to a worker: Kestra's `ExecutableTask`, of
    /// which `io.kestra.plugin.core.flow.Subflow` is the one implementation
    /// this executor plans.
    ///
    /// # Why it is not a [`TaskKind::Runnable`], measured rather than argued
    ///
    /// It opens `src/executor/` for the first time since W3a, so the reason is
    /// a single measured fact and not a preference. [`super::flowable::start`]
    /// emits `SUBMITTED` for a `Runnable` -- the one thing in Kestra's record
    /// that separates a task the worker ran from one the executor ran -- and
    /// **no `Subflow` run in `golden.json` carries it**: all 24 executed ones,
    /// across 22 entries, read `CREATED RUNNING ...`, and the one skipped run
    /// reads `CREATED SKIPPED`. Re-derived from the golden on every run by
    /// `tests/executor.rs`'s
    /// `no_subflow_run_in_the_golden_passes_through_submitted`, so the sentence
    /// above is compared by a running program rather than remembered.
    ///
    /// A `Subflow` left planned `Runnable` therefore mismatches **every** entry
    /// W6 exists to bank, whatever else its handler got right, which is why
    /// "keep it opaque and let a handler spawn and wait" -- the cheaper answer
    /// the plan asked to be tried first -- banks nothing.
    ///
    /// # What is the same as a `Runnable`, and what is not
    ///
    /// Everything except three things:
    ///
    /// * no `SUBMITTED` ([`super::flowable::start`]);
    /// * [`super::flowable::start`] emits a
    ///   [`Command::SpawnSubflow`](super::Command::SpawnSubflow) beside the
    ///   `StartRun`, so the caller creates the child execution in the same
    ///   settle that started the run;
    /// * `worker::claim::persist` writes **no** `type_id` for it, so the claim
    ///   -- whose whole predicate for "a worker runs this" is
    ///   `type_id IS NOT NULL` -- never takes it and no handler is ever asked
    ///   for a type it does not have.
    ///
    /// It is retried exactly as a `Runnable` is: Kestra's own eligibility test
    /// is `task instanceof RunnableTask<?> || task instanceof Subflow`, and
    /// [`super::retry::scan`] now spells both halves.
    Executable {
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
    /// `io.kestra.plugin.core.flow.Pause` — W8 Task 3.
    ///
    /// **A flowable the executor drives that takes no children in the plan**,
    /// which is a shape none of the arms above has. It is not
    /// [`Runnable`](TaskKind::Runnable) (no worker ever sees it, and no `Pause`
    /// run in the golden carries `SUBMITTED` — 20 of them, derived by
    /// `no_pause_run_in_the_golden_passes_through_submitted`), it is not
    /// [`Executable`](TaskKind::Executable) (it spawns nothing), and it does
    /// not descend.
    ///
    /// # `duration` is the author's `pauseDuration:`, unrendered
    ///
    /// Same rule as [`If::condition`](TaskKind::If): [`advance`](super::advance)
    /// is pure, so the caller renders it into
    /// [`TaskRun::rendered`](crate::executor::TaskRun::rendered) and `advance`
    /// reads the answer. It is read at `RUNNING`, like an `If`'s condition and
    /// unlike a `Loop`'s `values` — the pause has to be a run with an open
    /// attempt before there is anything to measure the wait from.
    /// `valids/pause-duration-from-input` is the corpus flow that needs it
    /// (`pauseDuration: "{{inputs.delay}}"`).
    ///
    /// `None` means the author wrote no `pauseDuration:` at all, which is NOT
    /// the same as "not rendered yet" and is why this is an `Option<String>`
    /// rather than a `String`.
    ///
    /// # `timeout` is on [`PlanTask::timeout`], not here
    ///
    /// It is `Task.java`'s field, carried by every kind, and a `Pause` is the
    /// one place `advance()` enforces it — Kestra's own validator says
    /// `timeout:` is "only relevant for runnable tasks" **with `Pause` the
    /// single exception** (`FlowService.java:574`). See
    /// [`super::flowable::pause`] for which of the two bounds ends the pause
    /// and with which state.
    ///
    /// # What a `Pause` planned this way does NOT carry
    ///
    /// `tasks:`, `errors:`, `behavior:` and `onPause:`. A `Pause` declaring any
    /// of them is flattened an opaque [`Runnable`](TaskKind::Runnable) instead —
    /// see [`pause_gap`] — and then **refused**: since the user's ruling of
    /// 2026-10-11 `spec::validate` reports it as the error
    /// `unsupported_pause_property`, so [`build_plan`]'s validation gate
    /// returns `Err` and no plan holding one is ever returned. The opaque
    /// planning alone was not enough for a `Pause`: the opaque run got a
    /// `type_id`, the worker claimed it and failed with `NotALeaf` before
    /// recording an outcome, and the execution stalled in `RUNNING` for ever.
    /// **The same was true of `AllowFailure` and `WorkingDirectory`**, which
    /// published with no finding at all; no-silent-stall Task 1 refuses them
    /// the same way (`unsupported_allow_failure`,
    /// `unsupported_working_directory`).
    Pause {
        duration: Option<String>,
    },
    /// `io.kestra.plugin.core.flow.LoopUntil`, and its alias
    /// `io.kestra.plugin.core.flow.WaitFor` — W8 Task 4.
    ///
    /// It runs `tasks` as a **sequential** list (it is an `AbstractBranch`, and
    /// `LoopUntil.tasksTree` builds `GraphUtils.sequential`), and when every
    /// child has terminated without failing it asks one question: go round
    /// again, or stop. Going round again is a `PAUSED` of `interval`, after
    /// which every run under this one is thrown away and re-created — which is
    /// what makes the construct different from a `Sequential` and is why
    /// mapping it to one would have produced a plausible-looking wrong trace.
    ///
    /// # Every field here is a LITERAL, and `build_plan` cannot read anything
    /// else
    ///
    /// Kestra renders all five through a `RunContext`
    /// (`Property<Duration>`, `Property<Integer>`, `Property<Boolean>`).
    /// [`build_plan`] is pure, so it reads literals — and a `LoopUntil`
    /// declaring an expression for any of them is planned opaque instead, by
    /// [`loop_until_gap`], exactly as a `Pause` with a `behavior:` is -- and,
    /// since no-silent-stall Task 1, refused by `spec::validate` as
    /// `unsupported_loop_until_property`, so [`build_plan`] returns `Err` for
    /// it rather than a plan that would stall. That is measured unreachable
    /// from this corpus by `every_corpus_loop_until_bound_is_a_literal`, so the
    /// refusal is a stated limit rather than a guess.
    ///
    /// `condition` is the author's expression and is **unrendered**, like an
    /// [`If`](TaskKind::If)'s: the caller renders it and writes the answer into
    /// [`TaskRun::rendered`](crate::executor::TaskRun::rendered). It is the one
    /// field that has to stay a `String`.
    LoopUntil {
        tasks: Vec<TaskIx>,
        condition: String,
        /// `checkFrequency.interval`, Kestra's `@Builder.Default` of `PT1M`
        /// when the author wrote none.
        interval: std::time::Duration,
        /// `checkFrequency.maxIterations`. `None` is Kestra's "unlimited".
        max_iterations: Option<i64>,
        /// `checkFrequency.maxDuration`. `None` is Kestra's "unlimited".
        max_duration: Option<std::time::Duration>,
        /// `failOnMaxReached`, Kestra's `@Builder.Default` of `false`.
        fail_on_max_reached: bool,
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
    /// **This cited `a_random_retry_delay_is_inside_kestras_own_bounds` as
    /// pinning the bounds "so a later task that wants a real RNG has the
    /// assertion already written". No test of that name exists** — found by W8
    /// Task 7's final round's citation sweep. What does exist and does pin the
    /// bounds is
    /// [`the_exponential_delay_doubles_and_the_random_one_stays_inside_its_bounds`](super::tests),
    /// in `executor/mod.rs`; the sentence above promised a later task an
    /// assertion under a name it would not have found.
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
/// **It is measured two ways, and the second one is the builder's own walk.**
/// Through Task 7 there was only the first: a walk over the parsed flow via
/// `Task::children()`, which is `spec::validate`'s walk and excludes every
/// task-level `errors:` list. That was written when a task-level `errors:` was
/// invisible to this builder as well — and Task 7 made it visible, because
/// [`flatten_one`] now descends into `errors` at `depth + 1`. From Task 7 on,
/// the YAML-side reading was no longer measuring the recursion this bounds.
/// Re-measured over the builder's own output at the final review, the deepest
/// plan is **6** as well, so there is no live consequence and the 8× headroom
/// is intact either way; the test now derives BOTH readings and requires them
/// to agree, so a future divergence is a failure rather than a stale sentence.
///
/// `finally:` is outside both readings, because no W3a task implements it and
/// `models` carries no task-level field for it to be read from. That half of
/// the old caveat stands.
pub(super) const MEASURED_MAX_CORPUS_DEPTH: usize = 6;
pub(super) const MAX_TASK_DEPTH: usize = 8 * MEASURED_MAX_CORPUS_DEPTH;

/// The flowables this builder plans, in the **canonical** spelling
/// [`canonical_flow_type`](crate::models::canonical_flow_type) reduces
/// every accepted wire `type` to.
///
/// **These were the FQCN literals through Task 8, and that was a seam
/// defect.** W1 accepts four spellings of each — `io.kestra.plugin.core.flow.X`
/// and `core.X`, either of them in any case, because `canonical_flow_type`
/// strips the prefix and lowercases — so an alias parsed to
/// `ModelKind::Sequential` and then fell through this builder's `match` to an
/// opaque `Runnable`, taking its whole subtree with it. Latent, because no
/// corpus flow writes an alias; not harmless, because the missing subtree also
/// takes [`MAX_TASK_DEPTH`] out of reach. Asserted by
/// `every_spelling_of_a_flowable_reaches_the_same_plan`, which plans a chain
/// past the bound in each of the four and requires the refusal from every one.
const SEQUENTIAL_TYPE: &str = "sequential";
const PARALLEL_TYPE: &str = "parallel";
const DAG_TYPE: &str = "dag";
const IF_TYPE: &str = "if";
const SWITCH_TYPE: &str = "switch";
const LOOP_TYPE: &str = "loop";
/// `io.kestra.plugin.core.flow.Subflow`, in the same canonical spelling. W6.
const SUBFLOW_TYPE: &str = "subflow";
/// `io.kestra.plugin.core.flow.Pause`, in the same canonical spelling. W8.
const PAUSE_TYPE: &str = "pause";
/// Two canonical names, ONE construct: `LoopUntil.java` registers
/// `io.kestra.plugin.core.flow.WaitFor` as an alias on itself, so
/// `models::kind_for` maps both spellings to one `models::TaskKind::LoopUntil`
/// and the pair below is what this builder matches on. W8 Task 4 Step 0.
const LOOP_UNTIL_TYPE: &str = "loopuntil";
const WAIT_FOR_TYPE: &str = "waitfor";

/// The first property of this `Pause` that [`build_plan`] does not reproduce,
/// or `None` for a plain wait.
///
/// **One rule, two readers.** [`build_plan`] uses it to decide whether a
/// `Pause` becomes a [`TaskKind::Pause`] or stays an opaque
/// [`TaskKind::Runnable`], and `spec::validate` uses it to decide whether the
/// flow is refused with `unsupported_pause_property` (since 2026-10-11; it was
/// the `not_yet_enforced` warning before, which a `Pause` used to get
/// unconditionally). Two copies of this predicate would be two answers to "is
/// this pause reproduced", and a publish that says yes over an executor that
/// says no is exactly the over-acceptance this programme forbids.
///
/// Each of the four is a real construct with a measured trace this executor
/// does not produce, and the order is declaration order in `Pause.java`:
///
/// * `tasks:` — the list a pause runs **after** it is resumed. No baselined
///   corpus entry writes one, so a rule here would be a rule nothing can
///   exercise.
/// * `errors:` — `valids/pause-errors-finally-after-execution` writes one, and
///   its golden run history is `CREATED RUNNING PAUSED RUNNING FAILED`: the
///   pause re-enters `RUNNING` to drive the handler. Nothing else in this
///   corpus has that shape.
/// * `behavior:` — `RESUME`/`WARN`/`FAIL`/`CANCEL` at the moment the pause
///   ends. This read *"two of the four terminals it can produce, `WARNING` and
///   `CANCELLED`, are not in `RunState` at all, so the construct is
///   unrepresentable here"*; **both are in it now** (W8 Task 5 and W8 Task 6),
///   and the entry did **not** bank — see the measurement in the comment on
///   [`pause_gap`]'s `behavior:` arm. What holds it is a fifth outcome the
///   artifact produces for an out-of-enum value, and the fact that a DYNAMIC
///   value cannot be classified at plan time at all, which is where this
///   decision is taken.
/// * `onPause:` — a single task run when the pause begins.
///   `valids/pause_on_pause`'s golden records it as a run with `parent: null`,
///   i.e. a SIBLING of the pause rather than a child, `overlapping` it.
///
/// The order decides only which name the `unsupported_pause_property` error
/// prints (`spec::validate`, which refuses any non-`None` since 2026-10-11);
/// the decision is the same for any non-`None`.
pub fn pause_gap(p: &crate::models::Pause) -> Option<&'static str> {
    if !p.tasks.is_empty() {
        return Some("tasks");
    }
    if !p.errors.is_empty() {
        return Some("errors");
    }
    // **Any `behavior:`, literal or expression, and rounds 2-3 MEASURED why.**
    // Fix round 1 narrowed this to literals on the theory that an expression
    // always fails at resume. It does not. Re-measured on the pinned artifact
    // with a FRESH, never-reused flow id for every execution -- round 2's runs
    // were contaminated by flow-id reuse, where `execution.inputs` still reads
    // the posted value so "the input landed" cannot say which SOURCE ran --
    // the corpus's own expression form `"{{inputs.behavior}}"` gives, 3 runs
    // each:
    //
    //     FAIL -> FAILED/1     RESUME -> SUCCESS/2
    //     WARN -> WARNING/2    CANCEL -> CANCELLED/1
    //
    // Four DIFFERENT executions, which is `PauseTest.java:374-380`'s own
    // semantics. **For a value INSIDE the `Behavior` enum the artifact honours
    // it, in every form it is written in** -- so "an expression fails" was a
    // rule about a class the corpus's one instance never exercised.
    //
    // **The sentence stops at the enum, and round 2's did not.** Outside it the
    // artifact does not terminate at all: `NOPE`, lower-case `fail` and an
    // undefined `{{inputs.nosuch}}` each leave the execution `RUNNING` past
    // 120s. Round 2's grid was form x {`FAIL`, `RESUME`} and every cell
    // rendered to a valid constant, so it could not see that edge and the
    // conclusion drawn from it was wider than the grid.
    //
    // It is a gap for the reason it always was, and the measurement above is
    // what confirms that reason rather than replacing it: `behavior:` is
    // four-valued and the artifact really does produce all four terminals.
    //
    // **This comment carried a PREDICTION and W8 Task 6 measured it false.** It
    // said *"the entry becomes bankable when `RunState` gains `CANCELLED` as
    // well -- W8 Task 5 supplied `WARNING`, which is half of it"*. W8 Task 6
    // supplied the other half, the vocabulary is complete, and
    // `valids/pause-behavior` did **not** bank: that task's ratchet moved
    // 111 -> 109 and both ids were `sla:`'s. The prediction was a claim about
    // the STATE VOCABULARY over an entry held by a PLAN decision -- this one.
    // Two things still hold it, and neither is a missing variant:
    //
    // * a fifth outcome, the non-terminal HANG the artifact produces for a
    //   value outside the enum (`NOPE`, lower-case `fail`, an undefined
    //   `{{inputs.nosuch}}` -- each leaves the execution `RUNNING` past 120 s),
    //   which is not a terminal and so not representable by any variant; and
    // * **a dynamic value cannot even be classified here** -- it is known at
    //   render time and this decision is taken at plan time.
    //
    // So the whole task stays opaque, which is the only answer that cannot
    // over-accept. Being inside the vocabulary is necessary and not sufficient,
    // and this is the fourth entry-population on this branch to measure that.
    if p.behavior.is_some() {
        return Some("behavior");
    }
    if p.on_pause.is_some() {
        return Some("onPause");
    }
    None
}

/// Build a [`Plan`] from a flow's YAML source, or refuse it.
///
/// # Refusal has one surface, and it is shared with the HTTP publish path
///
/// Three gates, in order: W1's parser, this builder's own structural checks,
/// then W1's [`validate`](crate::spec::validate).
///
/// **The order is measured, not tidy.** `validate` already carries a rule for
/// three of the things this builder refuses --- `dag_cycle`,
/// `dag_dependency_not_found`, `if_condition_required` --- so running it first
/// makes those three arms unreachable from here, and
/// `egret_refuses_every_flow_kestra_refuses` said so out loud the moment the
/// order was wrong: `invalids/{if-without-condition, dag-cyclicdependency,
/// dag-notexist-task}` were all still refused, and all three refused for a
/// DIFFERENT reason than the group they are in. Three refusals silently
/// becoming three other refusals is exactly what that test's per-cause message
/// exists to catch, and it caught it on its first run. Running the builder
/// first keeps each of those arms the reason the caller is given, and keeps
/// Task 4's and Task 5's mutations on them killable.
///
/// **Running `validate` here is Task 8's central change, and it is about a
/// seam rather than about a rule.** Until Task 8 this function called
/// `spec::parse` and not `spec::validate`, so egret had *two* answers to "is
/// this flow acceptable": **publish** refused a flow that `advance()` would
/// happily plan. Publish, and not `POST /flows`: `routes::create_flow` and
/// `update_flow` go through `parse_draft`, which only *parses* — the report
/// `validate` produces is carried into the draft DTO and refuses nothing. The
/// gate that turns `report.errors` into a `400` is `publish::publish`
/// (`publish.rs:118-127`), behind `routes::publish_flow`. Seven of the flows this Kestra refuses were over-accepted here
/// while W1's `validate` had already been refusing them at publish since W1 —
/// `duplicate_task_id` (2), `duplicate_input_id`, `empty_tasks` (2),
/// `self_subflow`, `working_directory_tasks_required`. The gap was not a
/// missing rule; it was a missing call.
///
/// Measured before the change and re-measured after: over the golden's **266**
/// `runs` entries — every flow this pinned Kestra published *and ran* —
/// `validate` reports **zero** errors. So this gate refuses nothing Kestra
/// accepted, which is the property that separates it from Task 3's M8 (a
/// refusal that was right for a reason having nothing to do with the flow being
/// invalid). `egret_refuses_every_flow_kestra_refuses` asserts on the message
/// per cause so a refusal that becomes a *different* refusal is visible, and
/// the ratchet fails in both directions so an over-refusal is a regression.
///
/// Only `report.errors` refuses. A warning — the flow-level `not_yet_enforced`
/// — must not.
///
/// **`unknown_task_type` was named here as a warning, and it is an error since
/// no-silent-stall Task 1.** This paragraph said promoting it "would refuse
/// every plugin egret has not registered", and that is exactly what it does,
/// on purpose: a type no handler serves stalled every execution that reached
/// it, which is worse than a refusal that says why. It closed
/// `invalids/invalid-task`, which now sits in
/// `egret_refuses_every_flow_kestra_refuses`'s groups rather than its
/// `STILL_OVER_ACCEPTED`, and it refuses 44 flows this Kestra published
/// (`executor::PUBLISHED_BUT_REFUSED`, each with the type it lacks).
///
/// # What this builder refuses before `validate` gets to see it
///
/// Seven things, and **exactly one of them is a rule `validate` does not
/// have**: nesting deeper than [`MAX_TASK_DEPTH`]. The other six — a `Dag`
/// whose `dependsOn` names a task that is not one of its own nodes
/// (`dag_dependency_not_found`), or whose nodes form a cycle (`dag_cycle`); a
/// `Dag` node with no task (`dag_node_task_required`); an `If` with no
/// `condition` (`if_condition_required`); a `Switch` with no `value`
/// (`switch_value_required`); and a `Loop` with no `values`
/// (`loop_values_required`) — are all in `validate` too, and this builder is
/// simply the gate they reach first.
///
/// **This heading said "Five things `validate` does not cover" and then listed
/// seven, six of which `validate` covers.** Both halves were wrong, and the
/// second one mattered: the gate order above exists *because* `validate`
/// carries those six, so a paragraph claiming it does not carry them argues
/// against the order settled three paragraphs earlier in the same doc comment.
/// What running the builder first buys is the *reason the caller is given* —
/// "if 'x' has no condition to branch on" rather than `if_condition_required`
/// — and Tasks 4's and 5's mutations on those arms staying killable.
///
/// The `Dag` pair is Task 4's and is exactly the two checks Kestra's own
/// `DagTaskValidator` makes.
///
/// # What its FLATTENING deliberately does not refuse
///
/// The flattening ([`flatten_flow`], the first two gates) does not refuse a
/// flow because it uses a flowable no task has implemented yet, and that
/// restraint is load-bearing rather than lazy.
///
/// **The third gate does, since no-silent-stall Task 1, and the two are not
/// in tension.** An opaque `Runnable` that is not a `Leaf` is a task no
/// handler can run, and every execution of it stalled in `RUNNING`; so
/// `spec::validate` refuses each such construct BY NAME
/// (`unsupported_allow_failure`, `unsupported_working_directory`,
/// `unsupported_loop_until_property`, `unsupported_pause_property`, and for a
/// `Leaf`, `unsupported_flow_task` and `unknown_task_type`). That is not M8's
/// rule below: M8 refused by SPELLING, in the builder, and its refusals landed
/// on flows Kestra refused for other reasons. Measured at Task 1, the named
/// refusals banked one ratchet entry, `invalids/invalid-task`, which Kestra
/// refuses for the same reason (an unknown type), and moved no other line;
/// the flows Kestra ran that they refuse are named one by one in
/// `executor::PUBLISHED_BUT_REFUSED`. The corpus guard
/// `every_task_planned_opaque_is_refused_at_its_path` holds the rule.
///
/// The oracle's `refused` verdict compares one thing: that `build_plan` returns
/// `Err`. A builder that refused every unimplemented flowable would return
/// `Err` for most of the `invalids` corpus too — and the ratchet would report
/// those entries as *matching*, inviting a maintainer to bank refusals that
/// egret gets right for a reason that has nothing to do with the flow being
/// invalid. Task 3's M8 measured exactly that: 12 regressions and 8 bogus
/// banks from one such rule. An unimplemented flowable is planned as an opaque
/// [`TaskKind::Runnable`], which produces a visibly wrong trace — the parent is
/// there and its children are not — and so stays honestly baselined.
///
/// That opacity is applied to `AllowFailure` and `WorkingDirectory` (and the
/// third gate then refuses both), even though each would "work" if mapped to
/// [`TaskKind::Sequential`]: the first turns a child's failure into `WARNING`,
/// which `RunState` cannot hold, and the second hands its whole subtree to one
/// worker. Mapping either would trade a visible mismatch for a
/// plausible-looking wrong trace, which is the worse of the two.
///
/// **Two more types were in that list and are not any more, and both left it
/// per TASK rather than per type.** A `Pause` that is a plain wait is a
/// [`TaskKind::Pause`] (W8 Task 3) and a `LoopUntil` whose bounds this pure
/// builder can read is a [`TaskKind::LoopUntil`] (W8 Task 4); one that declares
/// something the executor does not reproduce is still a `Runnable` — see
/// [`pause_gap`] and [`loop_until_gap`], each of which is the single predicate
/// both this builder and `spec::validate`'s refusal ask
/// (`unsupported_pause_property`, `unsupported_loop_until_property`), so a
/// flow refused at publish is exactly a flow that would be planned opaque here
/// -- and this function returns `Err` for it.
///
/// **A fourth kind of opacity exists below all of this**, where a flowable
/// Kestra has and `kind_for` does not reaches `ModelKind::Leaf` with its
/// children inside an opaque `spec`. Three types have left it: `Subflow` at W6
/// Task 3 for [`TaskKind::Executable`] (a different thing from opaque — the
/// executor runs it itself), `Pause` at W8 Task 3, and **`WaitFor` at W8 Task
/// 4**. So that tier now holds no Kestra flowable at all, only ordinary leaves.
/// The enumeration is derived by
/// `executor::tests::every_other_flowable_stays_opaque`; this prose agrees with
/// that test and not the other way round.
///
/// **`WaitFor` was in the fourth tier and not in the three, W8 Task 1 fix
/// round 4 measured it, and W8 Task 4 closed it.** `models::kind_for` matched
/// `Some("loopuntil")` and had no row for `"waitfor"`; neither did
/// `spec::validate`'s `KESTRA_FLOW_CONTROL`. So
/// `io.kestra.plugin.core.flow.WaitFor` parsed to `ModelKind::Leaf` with its
/// `tasks:` inside the opaque `spec`, as `Pause` did until W8 Task 3 — and
/// seven of the ten corpus flows of the construct use that spelling.
/// **Every sentence in this crate saying `build_plan` plans `WaitFor` opaque
/// was TRUE while it was wrong about which tier**, because both tiers were
/// planned [`TaskKind::Runnable`] and the plan was byte-identical either way;
/// the derivation above could not see it for the same reason. The probe that
/// found it is the one that DECLARES which `models::TaskKind` arm it means to
/// reach. The model divergence was real below the plan — a `Leaf` has no
/// `children()`, a `LoopUntil` does, and `children()` is what `spec::validate`,
/// `publish::resolve_all` and `worker::claim` walk — and **no corpus entry
/// could show it**, which is why the instrument that closed it is a pair of
/// `spec::validate` tests and not the ratchet: deleting the `"waitfor"` row
/// today leaves the ratchet green at 118.
pub fn build_plan(source: &str) -> Result<Plan, PlanError> {
    let flow = crate::spec::parse(source).map_err(|e| PlanError {
        message: match (e.line, e.column) {
            (Some(l), Some(c)) => {
                format!("cannot parse flow at line {l} column {c}: {}", e.message)
            }
            _ => format!("cannot parse flow: {}", e.message),
        },
    })?;

    let plan = flatten_flow(&flow)?;

    // W1's `validate`, as the third gate -- its RUN-TIME half,
    // `validate_for_run`, which omits the save-only findings (W5b fix round
    // 1: a version published before W5b with `pluginDefaults:` must still
    // run, applying nothing; spec §4.1). `errors` only: a warning is a
    // warning, and refusing on one would turn a flow-level `not_yet_enforced`
    // (`concurrency:`, `sla:`, ...) into a ban. (`unknown_task_type` was this
    // comment's example until no-silent-stall Task 1 made it an error.)
    let report = crate::spec::validate_for_run(&flow);
    if let Some(first) = report.errors.first() {
        return Err(PlanError {
            message: match report.errors.len() {
                1 => format!("{}: {} [{}]", first.code, first.message, first.path),
                n => format!(
                    "{}: {} [{}] (and {} further validation error(s))",
                    first.code,
                    first.message,
                    first.path,
                    n - 1
                ),
            },
        });
    }

    Ok(plan)
}

/// [`build_plan`]'s first two gates without its third: the flattening alone,
/// over an already-parsed flow, with no `validate_for_run`.
///
/// `pub(crate)` for one reader beside [`build_plan`]: the corpus guard
/// `executor::tests::every_task_planned_opaque_is_refused_at_its_path`, which
/// has to see what this builder WOULD plan opaque for a flow that the third
/// gate now refuses (no-silent-stall Task 1). Asking `build_plan` instead
/// would answer `Err` for exactly the flows the guard is about.
pub(crate) fn flatten_flow(flow: &crate::models::Flow) -> Result<Plan, PlanError> {
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

/// Flatten a list of children, **dropping the ones the author disabled**.
///
/// # `disabled:` is a plan-time deletion, not a run-time skip
///
/// Kestra removes a disabled task from every *resolved task list* before it
/// looks at any of them: `Execution.removeDisabled` is called on `tasks`,
/// `errors` **and** `finally` at the top of
/// `Execution.findTaskDependingFlowState` (`Execution.java:585-587`), which is
/// what every flowable's `resolveNexts`/`resolveState` goes through, and again
/// on the flow's own root list in `FlowableUtils.resolveSequentialNexts`'s
/// two-argument form (`FlowableUtils.java:191`). A disabled task therefore gets
/// **no taskrun at all** — it is absent from `taskRunList`, not present with a
/// `SKIPPED` state — and a flowable whose children are *all* disabled still
/// runs and still succeeds (`FlowableUtils.resolveSequentialState`'s
/// `filter(!disabled).findAny().isEmpty() -> SUCCESS`, and `If.java:192-198`'s
/// own copy of it).
///
/// That is a *list* rule, so it belongs here rather than in `advance()`: the
/// six corpus flows that use the flag put it on a flow root
/// (`valids/disable-simple`), on a flow-level `errors:` entry
/// (`valids/disable-error`), and on a child of a `Sequential`, a `Parallel`, an
/// `If` and a `Switch` — every one of which reaches this function. Deleting the
/// task from the plan reproduces all of them, and it reproduces them for the
/// same reason Kestra gets them: there is no run because there is no task.
///
/// **A `SKIPPED` state is NOT what this produces, and the oracle README used to
/// say it was.** The golden's only `SKIPPED` runs come from `runIf:`, which is
/// **W3c's** and lives at the other end: a `runIf:` is rendered per run, so the
/// run has to exist to be skipped, and the check is in
/// [`flowable::open`](super::flowable) rather than here.
/// `disabled_removes_the_run_and_run_if_skips_it` asserts the two traces side
/// by side; see `oracle-exec/README.md`, *What W3c had to change*.
///
/// # The one position this does not reach, and why that is deliberate
///
/// A `Dag`'s nodes are flattened by [`flatten_dag`], not here, and a disabled
/// `Dag` node is left planned. Kestra removes it from `currentTasks` like any
/// other child but leaves it in `taskDependencies`
/// (`FlowableUtils.resolveDagNexts`), so every node that `dependsOn` it waits
/// for a run that is never created and the `Dag` never terminates. Copying that
/// would be reproducing a hang; dropping the node *and* its incoming edges
/// would be inventing a semantics Kestra does not have; dropping the node alone
/// would make `flatten_dag` refuse the flow for a dangling `dependsOn`, which
/// is an over-refusal. No corpus flow reaches the question —
/// `no_corpus_dag_node_is_disabled` re-derives that from all 345 flows on every
/// run — so the position is left as a named gap rather than guessed at.
fn flatten_list(
    out: &mut Vec<PlanTask>,
    tasks: &[Task],
    depth: usize,
) -> Result<Vec<TaskIx>, PlanError> {
    tasks
        .iter()
        .filter(|t| !t.disabled)
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
        run_if: task.run_if.clone(),
        retry: super::retry::policy_of(task.retry.as_ref()),
        timeout: None,
        errors: Vec::new(),
        allow_failure: task.allow_failure,
        allow_warning: task.allow_warning,
    });

    // Kestra dispatches on the wire `type`, and so does this -- through the
    // SAME reduction W1 dispatches through, `canonical_flow_type`, rather than
    // through a literal of its own. The pair is kept rather than matching on
    // the parsed variant alone so that a future model change mapping a second
    // type onto an existing `ModelKind` is caught here; what it must not be is
    // a SECOND opinion about which strings name a flowable, which is what the
    // FQCN literals were.
    match (
        &task.kind,
        crate::models::canonical_flow_type(&task.r#type).as_deref(),
    ) {
        // `errors` is flattened AFTER `tasks` for both, so `out` stays
        // pre-order and the error subtree sits behind the task subtree it
        // handles -- which is also the order Kestra creates their runs in.
        (ModelKind::Sequential(seq), Some(SEQUENTIAL_TYPE)) => {
            let children = flatten_list(out, &seq.tasks, depth + 1)?;
            out[ix].errors = flatten_list(out, &seq.errors, depth + 1)?;
            out[ix].kind = TaskKind::Sequential { tasks: children };
        }
        (ModelKind::Parallel(par), Some(PARALLEL_TYPE)) => {
            let children = flatten_list(out, &par.tasks, depth + 1)?;
            out[ix].errors = flatten_list(out, &par.errors, depth + 1)?;
            out[ix].kind = TaskKind::Parallel {
                tasks: children,
                concurrency: literal_concurrency(par.concurrent.as_ref()),
            };
        }
        (ModelKind::If(cond), Some(IF_TYPE)) => {
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
        (ModelKind::Switch(sw), Some(SWITCH_TYPE)) => {
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
        // W6. It takes no children: a `Subflow` runs a DIFFERENT flow, whose
        // tasks get runs in the child execution and never in this one -- which
        // is why `models::TaskKind::Subflow` has no `children()` either.
        (ModelKind::Subflow(_), Some(SUBFLOW_TYPE)) => {
            out[ix].kind = TaskKind::Executable {
                type_id: task.r#type.clone(),
            };
        }
        (ModelKind::Loop(lp), Some(LOOP_TYPE)) => {
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
        (ModelKind::Dag(dag), Some(DAG_TYPE)) => {
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
        // W8 Task 3. A `Pause` this executor reproduces carries its
        // `pauseDuration:` unrendered and its `timeout:` parsed; one that
        // declares anything else falls through to the opaque `Runnable` this
        // function already assigned above, which is the same answer
        // `AllowFailure` and `WorkingDirectory` get and for the same reason --
        // and, like theirs, the third gate refuses it.
        //
        // **`timeout:` is read HERE and nowhere else in `advance()`'s half of
        // the crate**, and it is a literal parse rather than a render for the
        // reason [`PlanTask::retry`] gives about durations: `build_plan` is
        // pure. `every_corpus_pause_timeout_is_a_literal` re-derives from all
        // 345 flows that no corpus `timeout:` on a `Pause` is an expression, so
        // the fallback is measured unreachable rather than assumed.
        (ModelKind::Pause(pause), Some(PAUSE_TYPE)) => {
            if pause_gap(pause).is_none() {
                out[ix].timeout = task
                    .timeout
                    .as_ref()
                    .and_then(|t| super::retry::parse_duration(t.source()));
                out[ix].kind = TaskKind::Pause {
                    duration: pause
                        .pause_duration
                        .as_ref()
                        .map(|d| d.source().to_string()),
                };
            }
        }
        // W8 Task 4. The two spellings are one arm because they are one Kestra
        // class; see `LOOP_UNTIL_TYPE`.
        (ModelKind::LoopUntil(lu), Some(LOOP_UNTIL_TYPE) | Some(WAIT_FOR_TYPE))
            if loop_until_gap(lu).is_none() =>
        {
            // `condition` is `@NotNull` in Java and there is nothing to
            // loop until without one -- the same shape and the same
            // refusal as an `If` with no `condition`, which
            // `spec::validate` also reports as
            // `loop_until_condition_required`.
            let condition = lu
                .condition
                .as_ref()
                .map(|c| c.source().to_string())
                .ok_or_else(|| PlanError {
                    message: format!("loopUntil '{}' has no condition to loop until", task.id),
                })?;
            let cf = lu.check_frequency.as_ref();
            // No `errors:` is flattened, because W1's `models::LoopUntil`
            // has no such field -- Kestra's `AbstractBranch` does, and the
            // omission is W1's rather than this task's. Nothing in the
            // corpus writes one on a `LoopUntil`, so `plan.tasks[ix].errors`
            // stays the empty slice every caller already handles.
            let children = flatten_list(out, &lu.tasks, depth + 1)?;
            out[ix].kind = TaskKind::LoopUntil {
                tasks: children,
                condition,
                interval: cf
                    .and_then(|c| c.interval.as_ref())
                    .and_then(|p| super::retry::parse_duration(p.source()))
                    .unwrap_or(DEFAULT_CHECK_INTERVAL),
                max_iterations: cf
                    .and_then(|c| c.max_iterations.as_ref())
                    .and_then(|p| p.source().trim().parse::<i64>().ok()),
                max_duration: cf
                    .and_then(|c| c.max_duration.as_ref())
                    .and_then(|p| super::retry::parse_duration(p.source())),
                fail_on_max_reached: lu
                    .fail_on_max_reached
                    .as_ref()
                    .and_then(|p| p.source().trim().parse::<bool>().ok())
                    .unwrap_or(false),
            };
        }
        _ => {}
    }

    Ok(ix)
}

/// `LoopUntil.CheckFrequency.interval`'s Java default --
/// `@Builder.Default private Property<Duration> interval =
/// Property.ofValue(Duration.ofMinutes(1));`.
///
/// It is a minute, and nothing in this corpus takes it: all nine comparable
/// `LoopUntil`/`WaitFor` flows write an `interval:` of their own (three `PT1S`,
/// six `PT0.2S`). Recorded as a constant with the citation rather than inlined,
/// because a default nobody exercises is exactly the kind of figure that goes
/// stale silently.
const DEFAULT_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// The first property of a `LoopUntil` this executor cannot read, or `None` if
/// it can read all of them -- [`pause_gap`]'s shape, for the same reason.
///
/// [`build_plan`] is pure, so every bound has to be a literal. Kestra renders
/// each of them through a `RunContext`, so an author may legally write
/// `maxIterations: "{{ inputs.n }}"` -- and a builder that quietly read that as
/// "no limit" would turn a bounded loop into an unbounded one, which is the
/// worst direction the ratchet cannot see (such an execution never settles, so
/// it is `Broken` rather than a mismatch). Planning the task opaque instead
/// produces a visibly wrong trace, which is the answer `AllowFailure`,
/// `WorkingDirectory` and a gapped `Pause` already get -- and, since
/// no-silent-stall Task 1, `spec::validate` refuses it as
/// `unsupported_loop_until_property` at the field this returns, so
/// [`build_plan`] answers `Err` instead of a plan that would stall.
///
/// `condition:` is deliberately NOT in here: it is unrendered by design and the
/// caller renders it, exactly as an `If`'s is.
///
/// The order decides only which field the `unsupported_loop_until_property`
/// error names; the decision is the same for any non-`None`.
pub fn loop_until_gap(l: &crate::models::LoopUntil) -> Option<&'static str> {
    let cf = l.check_frequency.as_ref();
    if let Some(p) = cf.and_then(|c| c.interval.as_ref()) {
        if super::retry::parse_duration(p.source()).is_none() {
            return Some("checkFrequency.interval");
        }
    }
    if let Some(p) = cf.and_then(|c| c.max_duration.as_ref()) {
        if super::retry::parse_duration(p.source()).is_none() {
            return Some("checkFrequency.maxDuration");
        }
    }
    if let Some(p) = cf.and_then(|c| c.max_iterations.as_ref()) {
        if p.source().trim().parse::<i64>().is_err() {
            return Some("checkFrequency.maxIterations");
        }
    }
    if let Some(p) = l.fail_on_max_reached.as_ref() {
        if p.source().trim().parse::<bool>().is_err() {
            return Some("failOnMaxReached");
        }
    }
    None
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
