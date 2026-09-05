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
    match (&task.kind, task.r#type.as_str()) {
        (ModelKind::Sequential(seq), SEQUENTIAL_TYPE) => {
            let children = flatten_list(out, &seq.tasks, depth + 1)?;
            out[ix].kind = TaskKind::Sequential { tasks: children };
        }
        (ModelKind::Parallel(par), PARALLEL_TYPE) => {
            let children = flatten_list(out, &par.tasks, depth + 1)?;
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
            out[ix].kind = TaskKind::Switch {
                value,
                cases,
                default,
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
