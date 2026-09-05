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
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Plan {
    /// Flattened; children are addressed by index. Pre-order, so a parent
    /// always precedes its children — an ordering convenience for a reader,
    /// not something any rule depends on.
    pub tasks: Vec<PlanTask>,
    /// The flow's top-level `tasks:`.
    pub roots: Vec<TaskIx>,
    /// The flow's top-level `errors:`.
    pub errors: Vec<TaskIx>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanTask {
    /// The author's task id, as written.
    pub id: String,
    pub kind: TaskKind,
    /// **Always `None` from Task 3's [`build_plan`], and the reason is not
    /// laziness.** Kestra's `retry.interval`/`maxDuration` are
    /// `Property<Duration>` — unrendered source text — and rendering one needs
    /// a `RenderContext` this pure builder does not have and must not acquire.
    /// Task 7 owns `retry:` and owns deciding where that render happens.
    pub retry: Option<RetryPolicy>,
    /// **Always `None` from Task 3's [`build_plan`]**, for the same
    /// `Property<Duration>` reason as [`PlanTask::retry`] — and for a second
    /// one, measured: in this artifact `timeout:` is enforced by the **worker**
    /// (`worker/src/main/java/io/kestra/worker/processors/internals/WorkerTaskCallable.java:58`
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
    /// **Always empty from Task 3's [`build_plan`], and this one is a model
    /// gap, not a decision.** W1's `models::Sequential` (and every other
    /// `AbstractBranch` port in `models/task.rs`) drops the Java class's
    /// `errors`/`finally` fields — "W1 is the model, not the executor" — so
    /// there is nothing in the parsed flow to read them from. Task 7 owns
    /// `errors:` and has to widen `models::Sequential` first.
    pub errors: Vec<TaskIx>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskKind {
    /// Anything the executor does not see into: it hands the task to a worker
    /// and waits for a terminal state. Task 3 puts **every** task type here
    /// except `io.kestra.plugin.core.flow.Sequential`, including the flowables
    /// Tasks 4-8 own — see [`build_plan`] for why that is the safe default and
    /// not an approximation.
    Runnable {
        type_id: String,
    },
    Sequential {
        tasks: Vec<TaskIx>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Kestra spells it `maxAttempts`, measured.
    pub max_attempts: u32,
    /// A span, not a clock read.
    pub interval: Duration,
    pub max_duration: Option<Duration>,
    pub behavior: RetryBehavior,
    pub warning_on_retry: bool,
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
pub(super) const MEASURED_MAX_CORPUS_DEPTH: usize = 6;
pub(super) const MAX_TASK_DEPTH: usize = 8 * MEASURED_MAX_CORPUS_DEPTH;

/// Kestra's own class for the one flowable Task 3 implements.
const SEQUENTIAL_TYPE: &str = "io.kestra.plugin.core.flow.Sequential";

/// Build a [`Plan`] from a flow's YAML source, or refuse it.
///
/// # What Task 3 refuses, and what it deliberately does not
///
/// It refuses exactly two things: source W1's parser cannot read, and nesting
/// deeper than [`MAX_TASK_DEPTH`]. It refuses **nothing else** — in particular
/// it does not refuse a flow because it uses a flowable Task 3 has not
/// implemented, and that restraint is load-bearing rather than lazy.
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
        retry: None,
        timeout: None,
        errors: Vec::new(),
    });

    // Kestra dispatches on the wire `type`, and so does this: W1's
    // `models::TaskKind::Sequential` is reached by that type string and by no
    // other, but matching on the parsed variant alone would also catch a
    // future model change that maps a second type onto it.
    if let (ModelKind::Sequential(seq), SEQUENTIAL_TYPE) = (&task.kind, task.r#type.as_str()) {
        let children = flatten_list(out, &seq.tasks, depth + 1)?;
        out[ix].kind = TaskKind::Sequential { tasks: children };
    }

    Ok(ix)
}
