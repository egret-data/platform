//! `validate(flow: &Flow) -> ValidationReport` -- egret's own three
//! validation layers, run in order (spec section 9):
//!
//! 1. **Structural** -- shape a flow must have regardless of what its tasks
//!    do: unique task ids across the WHOLE tree (walked via `Task::children()`,
//!    not just `flow.tasks` -- `duplicate_task_ids_are_found_inside_control_flow`
//!    is what proves the walk is recursive), no dependency cycle inside a
//!    `core.dag`, every `dependsOn` in a `core.dag` names a task that exists
//!    in that dag, `errors:`/`finally:`/`afterExecution:` handlers can never
//!    collide with a normal successor's id (the shared `seen_ids` namespace
//!    the duplicate-id check already builds), and no `core.subflow` that
//!    names the flow it lives in.
//! 2. **Semantic** -- fields left `Option<T>` (or, for a list, a plain
//!    `Vec<T>` with `#[serde(default)]`) purely so parsing would accept
//!    `null`/absent the way Kestra's Jackson layer does, deferring the
//!    `@NotNull`/`@NotEmpty` bean-validation check to here. Every field
//!    tagged "Task 4 owes ..." in `models/`, plus 11 control-flow fields
//!    fix round 3 found still hard-required at parse
//!    (`Sequential`/`Parallel`/`Dag`/`Loop`/`LoopUntil`/`AllowFailure`/
//!    `WorkingDirectory`'s `tasks`, `If::condition`, `DagNode::task`,
//!    `Subflow::namespace`/`flow_id`) and `Loop::concurrency_limit`, which
//!    accepted absence but rejected an explicit `null`; see the task report
//!    for the full pairing of field -> `walk_task` arm -> test codes.
//!    `namespace`'s pattern; input ids unique; a `Switch` with neither
//!    `cases` nor `defaults` (the statically-decidable half of "exhaustive-
//!    or-defaulted" -- see the `Switch` arm in `walk_task` for why the
//!    other half is an explicit, documented deferral rather than a check).
//! 3. **Not-yet-enforced** -- features that parse and validate cleanly but
//!    that no package in this branch actually acts on yet
//!    (`NOT_YET_ENFORCED` and, for Kestra control-flow classes this crate
//!    hasn't ported to a `TaskKind` variant -- `Pause` today --
//!    `KESTRA_FLOW_CONTROL`, both below). These are warnings, never errors:
//!    a declared-but-unimplemented feature must still publish, or a green
//!    publish would lie about what the product does. This layer is why an
//!    unported control-flow type must never be left to fall silently through
//!    to `Leaf`: a `Pause` a user believes is a manual-approval gate that
//!    quietly runs straight through it is worse than refusing to publish the
//!    flow at all.

use std::collections::{HashMap, HashSet};

use crate::models::{
    canonical_flow_type, Flow, Input, Retry, Sla, SlaCommon, Task, TaskKind, Trigger, TriggerKind,
};
use crate::spec::registry;

/// A single validation finding: a machine-checkable `code`, a human message,
/// and a `path` locating it inside the flow.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    pub message: String,
    pub path: String,
}

/// The result of validating a parsed flow. Errors block publish; warnings do
/// not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ValidationReport {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

/// Features that parse today but nothing enforces yet, and the package that
/// will.
///
/// Naming the package is not decoration. A user who writes `concurrency:
/// {limit: 1}` and sees a green publish will reasonably conclude it is
/// enforced; the warning is the only thing standing between them and that
/// conclusion.
const NOT_YET_ENFORCED: &[(&str, &str)] = &[
    ("concurrency", "W8"),
    ("sla", "W8"),
    ("checks", "W8"),
    ("quotas", "W8"),
    ("retry", "W3"),
    ("timeout", "W3"),
    ("triggers", "W6"),
];

/// Whether a Kestra `io.kestra.plugin.core.flow.*` class that changes what
/// runs next (`FlowableTask`/`ExecutableTask` -- as opposed to an ordinary
/// `RunnableTask` that just runs and produces an output, like `Sleep` or
/// `PurgeFlows`, which live in the same Java package but are *not* listed
/// here) has a `TaskKind` variant in this crate.
#[derive(Debug, Clone, Copy)]
enum KestraFlowControl {
    /// Ported -- ordinary structural/semantic checks apply, same as any
    /// other `TaskKind` variant.
    Ported,
    /// Not ported: falls through to `TaskKind::Leaf`, which is
    /// indistinguishable from an ordinary task -- so whatever control-flow
    /// behaviour it declared (a manual-approval gate, for `Pause`) silently
    /// does not happen. Must warn, naming the package that will port it.
    Unported { package: &'static str },
}

/// Every class under `kestra/core/src/main/java/io/kestra/plugin/core/flow/`
/// (checked at the pinned `96893b589`) whose whole purpose is to change
/// control flow, and whether this crate has ported it to a `TaskKind`
/// variant. Both the ported AND unported classes are listed -- not just
/// `pause` -- so this table is the single place a reviewer checks when
/// `TaskKind` grows a new variant (flip that entry to `Ported`) or Kestra's
/// `flow/` package grows a new class (add a row).
///
/// Fix round 1, finding 1 first added this table keyed to a `find(...)`
/// lookup: `walk_task` warned only when a `Leaf` task's canonical type had a
/// row here. That was fail-OPEN -- it reproduced the exact bug it was meant
/// to fix, one layer up: `Pause` warned because someone had already noticed
/// it and added a row, but a class Kestra adds tomorrow (or a typo of an
/// existing one) fell straight through in silence, same as `Pause` did
/// before round 1. Fix round 2 made the CHECK fail-closed instead (anything
/// spelled like Kestra's control-flow namespace that reaches `Leaf` warns
/// unconditionally -- see the `TaskKind::Leaf` arm in `walk_task`) and
/// demoted this table to supplying only the package name for an entry that
/// has one; a miss still warns, just without naming a package.
///
/// `Sequential`/`Parallel`/`If`/`Switch`/`Dag`/`Loop`/`LoopUntil`/`Subflow`/
/// `AllowFailure`/`WorkingDirectory` are `TaskKind` variants (Task 2).
/// `Pause` (`Pause.java`) is a manual-approval / scheduled-resume gate with
/// no variant -- Task 2's own report flagged it ("a real `FlowableTask` ...
/// I did *not* port ... in case a later task expects it") and this is that
/// later task. Package per the programme spec's table (row W8: "pause/resume
/// ... W8").
///
/// Staleness in this table is asymmetric, and only one direction is
/// dangerous. If `TaskKind` later grows a variant for something marked
/// `Unported` here (say, a future `TaskKind::Pause`) and this row is never
/// updated to `Ported`, that is HARMLESS: `kind_for` in `task.rs` would
/// dispatch that canonical name to the new variant before `walk_task` ever
/// reaches the `TaskKind::Leaf` arm, so this row becomes dead documentation,
/// never a wrong warning. The dangerous direction -- a real unported class
/// with no row, or no table at all -- is the one the fail-closed check
/// below now covers regardless of this table's contents. Silence is what
/// made fix round 2 necessary; this table is not allowed to be the only
/// thing standing between a new Kestra class and silence again.
const KESTRA_FLOW_CONTROL: &[(&str, KestraFlowControl)] = &[
    ("sequential", KestraFlowControl::Ported),
    ("parallel", KestraFlowControl::Ported),
    ("if", KestraFlowControl::Ported),
    ("switch", KestraFlowControl::Ported),
    ("dag", KestraFlowControl::Ported),
    ("loop", KestraFlowControl::Ported),
    ("loopuntil", KestraFlowControl::Ported),
    ("subflow", KestraFlowControl::Ported),
    ("allowfailure", KestraFlowControl::Ported),
    ("workingdirectory", KestraFlowControl::Ported),
    ("pause", KestraFlowControl::Unported { package: "W8" }),
];

/// Which of `NOT_YET_ENFORCED`'s features this flow actually declares.
/// Accumulated while walking the flow so each feature warns exactly once,
/// regardless of how many tasks/places declare it.
#[derive(Debug, Default)]
struct FeaturePresence {
    concurrency: bool,
    sla: bool,
    checks: bool,
    quotas: bool,
    retry: bool,
    timeout: bool,
    triggers: bool,
}

pub fn validate(flow: &Flow) -> ValidationReport {
    let mut report = ValidationReport::default();

    validate_namespace(flow, &mut report);
    validate_flow_level_fields(flow, &mut report);

    let mut features = FeaturePresence {
        concurrency: flow.concurrency.is_some(),
        sla: !flow.sla.is_empty(),
        checks: !flow.checks.is_empty(),
        quotas: !flow.quotas.is_empty(),
        retry: flow.retry.is_some(),
        timeout: false,
        triggers: !flow.triggers.is_empty(),
    };

    check_retry(&flow.retry, "retry", &mut report);

    // `seen_ids` spans all FOUR root lists in one shared id-namespace, not
    // just `flow.tasks` -- matching Kestra's own `Flow.allTasks()` (`tasks`,
    // `errors`, `_finally`, `afterExecution` concatenated into one stream)
    // and `findTaskByTaskId`, which searches that same combined stream. This
    // is also the concrete, checkable form §9's structural-layer bullet
    // "errors:/finally: handlers are not referenced as normal successors"
    // takes in THIS model: Kestra's Java additionally walks each
    // control-flow node's own local `errors`/`finally` fields
    // (`AbstractBranch.allChildTasks()`), but Task 2 deliberately did not
    // port those per-node fields at all (dropped, not modeled -- see
    // `task.rs`'s comment on `Sequential`), so the only place an
    // errors/finally handler's id COULD collide with a normal successor's
    // id in this crate's model is across these four flow-level lists. A
    // shared namespace here means a task id declared under `errors:`/
    // `finally:`/`afterExecution:` can never silently double as (be
    // "referenced as") a normal `tasks:` successor.
    let mut seen_ids: HashMap<String, Vec<String>> = HashMap::new();
    for (list_name, list) in [
        ("tasks", &flow.tasks),
        ("errors", &flow.errors),
        ("finally", &flow.finally_),
        ("afterExecution", &flow.after_execution),
    ] {
        for task in list {
            let path = format!("{list_name}.{}", task.id);
            walk_task(flow, task, path, &mut seen_ids, &mut features, &mut report);
        }
    }

    for (id, paths) in &seen_ids {
        if paths.len() > 1 {
            report.errors.push(Finding {
                code: "duplicate_task_id",
                message: format!(
                    "task id '{id}' is declared more than once: {}",
                    paths.join(", ")
                ),
                path: paths[0].clone(),
            });
        }
    }

    validate_inputs(&flow.inputs, &mut report);

    for trigger in &flow.triggers {
        validate_trigger(trigger, &mut report);
    }

    emit_not_yet_enforced(&features, &mut report);

    report
}

/// `namespace` against Kestra's own pattern (`AbstractFlow.java`):
/// `^[a-z0-9][a-z0-9._-]*`, `@Size(min = 1, max = 150)`. A hand-written
/// character check, not a `regex` dependency for one pattern.
fn validate_namespace(flow: &Flow, report: &mut ValidationReport) {
    if !is_valid_namespace(&flow.namespace) {
        report.errors.push(Finding {
            code: "invalid_namespace",
            message: format!(
                "namespace '{}' must match ^[a-z0-9][a-z0-9._-]* and be 1-150 characters long",
                flow.namespace
            ),
            path: "namespace".to_string(),
        });
    }
}

fn is_valid_namespace(ns: &str) -> bool {
    if ns.is_empty() || ns.chars().count() > 150 {
        return false;
    }
    let mut chars = ns.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Task 4's `@NotNull`/`@NotEmpty` debts that live directly on `Flow` --
/// `tasks` (empty check), `Concurrency::limit`, `FlowOutput::type`, and both
/// `Sla` fields (`duration` on `MaxDuration`, `behavior` shared by both
/// variants via `SlaCommon`).
fn validate_flow_level_fields(flow: &Flow, report: &mut ValidationReport) {
    if flow.tasks.is_empty() {
        report.errors.push(Finding {
            code: "empty_tasks",
            message: "a flow must declare at least one task (`tasks` is `@NotEmpty` in Kestra)"
                .to_string(),
            path: "tasks".to_string(),
        });
    }

    if let Some(c) = &flow.concurrency {
        match c.limit {
            None => report.errors.push(Finding {
                code: "concurrency_limit_required",
                message: "concurrency.limit is required when concurrency is declared".to_string(),
                path: "concurrency.limit".to_string(),
            }),
            Some(limit) if limit < 1 => report.errors.push(Finding {
                code: "concurrency_limit_too_low",
                message: format!("concurrency.limit must be >= 1, got {limit}"),
                path: "concurrency.limit".to_string(),
            }),
            _ => {}
        }
    }

    for (i, output) in flow.outputs.iter().enumerate() {
        if output.r#type.is_none() {
            report.errors.push(Finding {
                code: "flow_output_type_required",
                message: format!("output '{}' is missing a required type", output.id),
                path: format!("outputs[{i}].type"),
            });
        }
    }

    for (i, sla) in flow.sla.iter().enumerate() {
        let common = sla_common(sla);
        if common.behavior.is_none() {
            report.errors.push(Finding {
                code: "sla_behavior_required",
                message: format!("sla '{}' is missing a required behavior", common.id),
                path: format!("sla[{i}].behavior"),
            });
        }
        if let Sla::MaxDuration { duration, .. } = sla {
            if duration.is_none() {
                report.errors.push(Finding {
                    code: "sla_duration_required",
                    message: format!("sla '{}' is missing a required duration", common.id),
                    path: format!("sla[{i}].duration"),
                });
            }
        }
    }
}

/// Fix round 1, finding 3: §9's semantic layer requires "input ids unique";
/// `validate()` never read `flow.inputs` at all before this. Same shape as
/// the task-id duplicate check, but its own pass -- `Input` ids and `Task`
/// ids are different namespaces in Kestra (`{{ inputs.x }}` vs a task's own
/// id), so an input and a task are allowed to share an id and this check
/// must not conflate them with `seen_ids` above.
///
/// §9 names only "input ids unique" for this layer; `Input::r#type` is
/// already a required (non-`Option`) field with no `#[serde(default)]`
/// (`input.rs`), so an absent or wrong-typed `type` is already a *parse*
/// error today, same as `Flow::id`/`Flow::namespace` -- there is no separate
/// `Option`-then-validate debt for it the way `FlowOutput::type` has, so
/// nothing else on `Input` is owed here.
fn validate_inputs(inputs: &[Input], report: &mut ValidationReport) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for input in inputs {
        *seen.entry(input.id.as_str()).or_insert(0) += 1;
    }
    for (id, count) in seen {
        if count > 1 {
            report.errors.push(Finding {
                code: "duplicate_input_id",
                message: format!("input id '{id}' is declared more than once"),
                path: format!("inputs.{id}"),
            });
        }
    }
}

fn sla_common(sla: &Sla) -> &SlaCommon {
    match sla {
        Sla::MaxDuration { common, .. } => common,
        Sla::ExecutionAssertion { common, .. } => common,
    }
}

/// Walk one task node and its subtree, recording:
/// - its id, for the structural duplicate-id check run after the whole walk;
/// - whether it declares `timeout`/`retry` (for the not-yet-enforced layer);
/// - Task 4's `@NotNull` debts specific to each control-flow kind
///   (`Parallel`/`Dag::concurrent`, `Switch::value`, `Loop::values`,
///   `LoopUntil::condition`, and each `Retry` subtype's interval fields);
/// - the two remaining structural checks, `dag_cycle` and `self_subflow`.
///
/// Recurses through `Task::children()`, not `flow.tasks` -- this is the
/// walk Step 7's mutation proves is load-bearing.
fn walk_task(
    flow: &Flow,
    task: &Task,
    path: String,
    seen_ids: &mut HashMap<String, Vec<String>>,
    features: &mut FeaturePresence,
    report: &mut ValidationReport,
) {
    seen_ids
        .entry(task.id.clone())
        .or_default()
        .push(path.clone());

    if task.timeout.is_some() {
        features.timeout = true;
    }
    if task.retry.is_some() {
        features.retry = true;
    }
    check_retry(&task.retry, &format!("{path}.retry"), report);

    match &task.kind {
        // Fix round 3: `Sequential::tasks` is `@NotEmpty` on `AbstractBranch`
        // in Java -- see `task.rs`'s doc comment on `Sequential` for why
        // this was hard-required at parse instead until now, and why it's a
        // plain `Vec<Task>` (empty on absence) rather than `Option<Vec<Task>>`.
        TaskKind::Sequential(s) => {
            if s.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "sequential_tasks_required",
                    message: format!(
                        "task '{}': sequential must declare at least one task",
                        task.id
                    ),
                    path: format!("{path}.tasks"),
                });
            }
        }
        TaskKind::Parallel(p) => {
            if p.concurrent.is_none() {
                report.errors.push(Finding {
                    code: "parallel_concurrent_required",
                    message: format!("task '{}': concurrent is required (explicit null)", task.id),
                    path: format!("{path}.concurrent"),
                });
            }
            // Fix round 3: `tasks` carries the same `@NotEmpty`-on-
            // `AbstractBranch` debt `Sequential` documents above.
            if p.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "parallel_tasks_required",
                    message: format!(
                        "task '{}': parallel must declare at least one task",
                        task.id
                    ),
                    path: format!("{path}.tasks"),
                });
            }
        }
        // Fix round 3: `If::condition` is `@NotNull` in Java, same bean-
        // validation-not-parsing principle as `Parallel::concurrent`. `then`
        // is separately `@NotEmpty` -- found in the same sweep, NOT one of
        // the 11 fields this round scoped, and deliberately left unfixed
        // here; see the task report.
        TaskKind::If(i) => {
            if i.condition.is_none() {
                report.errors.push(Finding {
                    code: "if_condition_required",
                    message: format!("task '{}': condition is required (explicit null)", task.id),
                    path: format!("{path}.condition"),
                });
            }
        }
        TaskKind::Dag(d) => {
            // `Dag::concurrent` carries the identical `@NotNull`-is-bean-
            // validation debt `Parallel::concurrent` does (task.rs's own
            // comment on `Dag` says so by cross-reference rather than
            // repeating the phrase "Task 4 owes" verbatim) -- checked here
            // for the same reason, not because a literal grep hit named it.
            if d.concurrent.is_none() {
                report.errors.push(Finding {
                    code: "dag_concurrent_required",
                    message: format!("task '{}': concurrent is required (explicit null)", task.id),
                    path: format!("{path}.concurrent"),
                });
            }
            // Fix round 3: `Dag::tasks` is `@NotEmpty` in Java (`Dag.java`'s
            // own field, not inherited from `AbstractBranch`).
            if d.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "dag_tasks_required",
                    message: format!("task '{}': dag must declare at least one node", task.id),
                    path: format!("{path}.tasks"),
                });
            }
            // Fix round 3: `DagTask::task` is `@NotNull` in Java -- the
            // weakest of the 11 fields this round fixes, but checked for
            // consistency with every other field in this file rather than
            // carving out an exception (see `DagNode::task`'s doc comment
            // in `task.rs`).
            for (i, node) in d.tasks.iter().enumerate() {
                if node.task.is_none() {
                    report.errors.push(Finding {
                        code: "dag_node_task_required",
                        message: format!(
                            "task '{}': dag node #{i} has no task (explicit null)",
                            task.id
                        ),
                        path: format!("{path}.tasks[{i}].task"),
                    });
                }
            }
            // A node with no task has no id to key an edge on, so it's
            // excluded here rather than panicking or fabricating one --
            // `dag_node_task_required` above already reports it, and any
            // `dependsOn` some OTHER node aimed at this node's (unknowable)
            // id will separately, correctly, read as dangling.
            let edges: Vec<(String, Vec<String>)> = d
                .tasks
                .iter()
                .filter_map(|n| {
                    n.task
                        .as_ref()
                        .map(|t| (t.id.clone(), n.depends_on.clone()))
                })
                .collect();

            // Fix round 1, finding 2: a `dependsOn` naming a task that
            // doesn't exist in this dag was previously silent -- `has_cycle`
            // only ever walks edges that ARE present in the adjacency map,
            // so a dangling reference just never gets visited rather than
            // failing anything. Checked against this dag's own node ids
            // only (not the whole flow): `dependsOn` is Kestra's dag-local
            // dependency syntax, not a flow-wide task reference.
            let known_ids: HashSet<&str> = edges.iter().map(|(id, _)| id.as_str()).collect();
            for (node_id, deps) in &edges {
                for dep in deps {
                    if !known_ids.contains(dep.as_str()) {
                        report.errors.push(Finding {
                            code: "dag_dependency_not_found",
                            message: format!(
                                "task '{}': dag node '{node_id}' has dependsOn '{dep}', which \
                                 does not name a task in this dag",
                                task.id
                            ),
                            path: format!("{path}.tasks.{node_id}.dependsOn"),
                        });
                    }
                }
            }

            if has_cycle(&edges) {
                report.errors.push(Finding {
                    code: "dag_cycle",
                    message: format!("task '{}' has a dependency cycle in its dag", task.id),
                    path: path.clone(),
                });
            }
        }
        TaskKind::Switch(s) => {
            if s.value.is_none() {
                report.errors.push(Finding {
                    code: "switch_value_required",
                    message: format!("task '{}': value is required (explicit null)", task.id),
                    path: format!("{path}.value"),
                });
            }
            // Fix round 1, finding 4: §9 requires "Switch cases exhaustive-
            // or-defaulted". The half of that this crate CAN check
            // statically: a switch with neither `cases` nor `defaults` has
            // no branch to run under any match and is unreachable dead
            // control flow. The other half -- whether a non-empty `cases`
            // actually covers every value `value` (a rendered runtime
            // expression, W2) could ever produce, when `defaults` is absent
            // -- is not decidable here: `value`'s domain isn't known until
            // it's rendered, and `cases`' keys are arbitrary strings, so
            // there is no sound way to prove exhaustiveness at validate
            // time without either false negatives (accepting a switch that
            // misses a case) or false positives (rejecting a switch whose
            // author knows their input domain is fully covered by `cases`
            // and deliberately omitted `defaults`). Deferring that half
            // explicitly, the same way `Execution::original_id` is deferred
            // -- not implemented, not silent.
            if s.cases.is_empty() && s.defaults.is_empty() {
                report.errors.push(Finding {
                    code: "switch_no_branches",
                    message: format!(
                        "task '{}': switch has neither cases nor defaults, so it runs nothing \
                         under any match",
                        task.id
                    ),
                    path: path.clone(),
                });
            }
        }
        TaskKind::Loop(l) => {
            if l.values.is_none() {
                report.errors.push(Finding {
                    code: "loop_values_required",
                    message: format!("task '{}': values is required (explicit null)", task.id),
                    path: format!("{path}.values"),
                });
            }
            // Fix round 3: `tasks` carries the same `@NotEmpty`-on-
            // `AbstractBranch` debt `Sequential` documents above.
            if l.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "loop_tasks_required",
                    message: format!("task '{}': loop must declare at least one task", task.id),
                    path: format!("{path}.tasks"),
                });
            }
            // Fix round 3 secondary item: `Loop::concurrency_limit` is
            // `@NotNull @Builder.Default = 1` in Java -- a boxed `Integer`,
            // not a primitive, so an explicit `null` genuinely parses in
            // Kestra and must parse here too. Found in the same sweep as
            // the 11 fields: it already accepted absence but rejected an
            // explicit `null`, the exact asymmetry `Parallel::concurrent`
            // was fixed against.
            if l.concurrency_limit.is_none() {
                report.errors.push(Finding {
                    code: "loop_concurrency_limit_required",
                    message: format!(
                        "task '{}': concurrencyLimit is required (explicit null)",
                        task.id
                    ),
                    path: format!("{path}.concurrencyLimit"),
                });
            }
        }
        TaskKind::LoopUntil(l) => {
            if l.condition.is_none() {
                report.errors.push(Finding {
                    code: "loop_until_condition_required",
                    message: format!("task '{}': condition is required (explicit null)", task.id),
                    path: format!("{path}.condition"),
                });
            }
            // Fix round 3: `tasks` carries the same `@NotEmpty`-on-
            // `AbstractBranch` debt `Sequential` documents above.
            if l.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "loop_until_tasks_required",
                    message: format!(
                        "task '{}': loopUntil must declare at least one task",
                        task.id
                    ),
                    path: format!("{path}.tasks"),
                });
            }
        }
        // Fix round 3: `Subflow::namespace` is `@NotEmpty` and
        // `Subflow::flow_id` is `@NotNull` in Java, both bean-validation-
        // not-parsing like everything else in this file. No longer a match
        // guard (round 2 had `TaskKind::Subflow(s) if s.namespace ==
        // flow.namespace && s.flow_id == flow.id => { ... }`) because the
        // required-field checks below must run for EVERY subflow, not only
        // ones whose namespace/flow_id happen to be present and match --
        // `self_subflow` is now a nested check, only reachable once both
        // are known to be `Some`.
        TaskKind::Subflow(s) => {
            if s.namespace.is_none() {
                report.errors.push(Finding {
                    code: "subflow_namespace_required",
                    message: format!("task '{}': namespace is required (explicit null)", task.id),
                    path: format!("{path}.namespace"),
                });
            }
            if s.flow_id.is_none() {
                report.errors.push(Finding {
                    code: "subflow_flow_id_required",
                    message: format!("task '{}': flowId is required (explicit null)", task.id),
                    path: format!("{path}.flowId"),
                });
            }
            if let (Some(ns), Some(fid)) = (&s.namespace, &s.flow_id) {
                if *ns == flow.namespace && *fid == flow.id {
                    report.errors.push(Finding {
                        code: "self_subflow",
                        message: format!(
                            "task '{}' is a subflow that names its own flow ('{ns}/{fid}')",
                            task.id
                        ),
                        path: path.clone(),
                    });
                }
            }
        }
        // Fix round 3: `AllowFailure`/`WorkingDirectory` both `extends
        // Sequential` with no fields of their own, so `tasks` carries the
        // same `@NotEmpty`-on-`AbstractBranch` (via `Sequential`) debt
        // `Sequential` documents above.
        TaskKind::AllowFailure(a) => {
            if a.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "allow_failure_tasks_required",
                    message: format!(
                        "task '{}': allowFailure must declare at least one task",
                        task.id
                    ),
                    path: format!("{path}.tasks"),
                });
            }
        }
        TaskKind::WorkingDirectory(w) => {
            if w.tasks.is_empty() {
                report.errors.push(Finding {
                    code: "working_directory_tasks_required",
                    message: format!(
                        "task '{}': workingDirectory must declare at least one task",
                        task.id
                    ),
                    path: format!("{path}.tasks"),
                });
            }
        }
        // Fix round 2: round 1's version of this check only warned when the
        // task's canonical type had a row in `KESTRA_FLOW_CONTROL` --
        // fail-OPEN, reproducing the exact bug it was meant to fix, one
        // layer up. Fail-CLOSED instead: the table supplies only the
        // package name for the message, never the decision to warn.
        //
        // A `Leaf` is how every *ordinary* task is represented (`egret.sql`,
        // an unrecognised third-party plugin, ...), but it is ALSO where a
        // genuine Kestra control-flow class lands if this crate hasn't
        // ported it to a `TaskKind` variant. Both look identical here (an
        // opaque `spec` map), so:
        //   1. Anything spelled like Kestra's control-flow namespace (the
        //      FQCN prefix, or the `core.` alias this crate ALSO shares
        //      with its own registered leaf types) warns unconditionally --
        //      a table hit names the package; a miss still warns, just
        //      without naming one, so the next class Kestra adds warns the
        //      moment it's written, not the moment someone adds a row.
        //   2. Anything else that is neither a `TaskKind` variant nor
        //      `registry::known_type(...)` warns too, under its own code --
        //      this is what catches a typo like `egret.sqll` before Task 6
        //      turns it into a hard publish refusal.
        // `registry::known_type` is checked FIRST, so `core.log`/`core.http`/
        // `core.slack` -- which share the `core.` alias spelling with real
        // Kestra control-flow aliases -- never reach either warning: the
        // rule is "neither a control-flow variant nor a registered type",
        // not "starts with `core.`".
        TaskKind::Leaf { .. } if registry::known_type(&task.r#type).is_none() => {
            if let Some(canonical) = canonical_flow_type(&task.r#type) {
                let package = KESTRA_FLOW_CONTROL
                    .iter()
                    .find_map(|(name, status)| match status {
                        KestraFlowControl::Unported { package } if *name == canonical => {
                            Some(*package)
                        }
                        _ => None,
                    });
                let message = match package {
                    Some(package) => format!(
                        "task '{}' has type '{}', a Kestra control-flow type this crate has not \
                         ported to a typed task kind yet; it currently runs as an inert no-op \
                         instead of doing what it declares. Enforcement lands in {package}",
                        task.id, task.r#type
                    ),
                    None => format!(
                        "task '{}' has type '{}', which is spelled like a Kestra control-flow \
                         task (a `io.kestra.plugin.core.flow.*` FQCN or its `core.` alias) but \
                         egret does not implement it yet; it currently runs as an inert no-op \
                         instead of doing what it declares",
                        task.id, task.r#type
                    ),
                };
                report.warnings.push(Finding {
                    code: "not_yet_enforced",
                    message,
                    path: path.clone(),
                });
            } else {
                report.warnings.push(Finding {
                    code: "unknown_task_type",
                    message: format!(
                        "task '{}' has type '{}', which egret does not recognise as a task type; \
                         check for a typo, or add it to the task-type registry if it's meant to \
                         be a real type",
                        task.id, task.r#type
                    ),
                    path: path.clone(),
                });
            }
        }
        // A registered `Leaf` (the guard above was false) -- an ordinary
        // task type, nothing to check here. Written as an explicit arm for
        // this specific variant rather than a trailing `_ => {}`: every
        // `TaskKind` variant now has its own arm above, so this keeps the
        // match exhaustive on variants rather than falling back to a
        // wildcard -- a FUTURE `TaskKind` variant this match doesn't yet
        // know about becomes a compile error here, not silent do-nothing,
        // the same fail-closed instinct fix round 2 applied to the
        // control-flow namespace check.
        TaskKind::Leaf { .. } => {}
    }

    for child in task.children() {
        let child_path = format!("{path}.{}", child.id);
        walk_task(flow, child, child_path, seen_ids, features, report);
    }
}

/// Detect a dependency cycle among a `core.dag`'s nodes, given as
/// `(id, dependsOn)` pairs already extracted from `Dag::tasks` -- that
/// field's element type, `DagNode`, is private to `models` and not
/// reachable from this module, so the caller extracts the two fields this
/// check needs rather than this function naming that type.
fn has_cycle(edges: &[(String, Vec<String>)]) -> bool {
    let adj: HashMap<&str, &[String]> = edges
        .iter()
        .map(|(id, deps)| (id.as_str(), deps.as_slice()))
        .collect();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut stack: HashSet<&str> = HashSet::new();

    fn dfs<'a>(
        id: &'a str,
        adj: &HashMap<&'a str, &'a [String]>,
        visited: &mut HashSet<&'a str>,
        stack: &mut HashSet<&'a str>,
    ) -> bool {
        if stack.contains(id) {
            return true;
        }
        if visited.contains(id) {
            return false;
        }
        visited.insert(id);
        stack.insert(id);
        if let Some(deps) = adj.get(id) {
            for dep in deps.iter() {
                if dfs(dep.as_str(), adj, visited, stack) {
                    return true;
                }
            }
        }
        stack.remove(id);
        false
    }

    for (id, _) in edges {
        if dfs(id.as_str(), &adj, &mut visited, &mut stack) {
            return true;
        }
    }
    false
}

/// Task 4's `@NotNull` debt on `Retry`'s three `@JsonSubTypes`: whichever
/// interval field(s) a subtype has (`interval` for `Constant`; `interval` +
/// `maxInterval` for `Exponential`; `minInterval` + `maxInterval` for
/// `Random`) must be present. Shared by `Flow::retry` and every
/// `Task::retry` via `retry_path` naming where the retry lives.
fn check_retry(retry: &Option<Retry>, retry_path: &str, report: &mut ValidationReport) {
    let Some(retry) = retry else {
        return;
    };

    let mut missing = Vec::new();
    match retry {
        Retry::Constant { interval, .. } => {
            if interval.is_none() {
                missing.push("interval");
            }
        }
        Retry::Exponential {
            interval,
            max_interval,
            ..
        } => {
            if interval.is_none() {
                missing.push("interval");
            }
            if max_interval.is_none() {
                missing.push("maxInterval");
            }
        }
        Retry::Random {
            min_interval,
            max_interval,
            ..
        } => {
            if min_interval.is_none() {
                missing.push("minInterval");
            }
            if max_interval.is_none() {
                missing.push("maxInterval");
            }
        }
    }

    for field in missing {
        report.errors.push(Finding {
            code: "retry_interval_required",
            message: format!("{retry_path}: {field} is required (explicit null)"),
            path: format!("{retry_path}.{field}"),
        });
    }
}

/// Task 4's `@NotNull` debt on `Schedule::cron`.
fn validate_trigger(trigger: &Trigger, report: &mut ValidationReport) {
    if let TriggerKind::Schedule(s) = &trigger.kind {
        if s.cron.is_none() {
            report.errors.push(Finding {
                code: "schedule_cron_required",
                message: format!("trigger '{}': cron is required (explicit null)", trigger.id),
                path: format!("triggers.{}.cron", trigger.id),
            });
        }
    }
}

fn emit_not_yet_enforced(features: &FeaturePresence, report: &mut ValidationReport) {
    let present: [(&str, bool); 7] = [
        ("concurrency", features.concurrency),
        ("sla", features.sla),
        ("checks", features.checks),
        ("quotas", features.quotas),
        ("retry", features.retry),
        ("timeout", features.timeout),
        ("triggers", features.triggers),
    ];
    for (name, is_present) in present {
        if !is_present {
            continue;
        }
        let (_, package) = NOT_YET_ENFORCED
            .iter()
            .find(|(n, _)| *n == name)
            .expect("every feature name here has a NOT_YET_ENFORCED entry");
        report.warnings.push(Finding {
            code: "not_yet_enforced",
            message: format!(
                "`{name}` is declared in this flow but is not yet enforced by any package in \
                 this branch; enforcement lands in {package}"
            ),
            path: name.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::spec::{parse, validate};

    #[test]
    fn a_syntax_error_carries_line_and_column() {
        let err = parse("id: a\n  namespace: bad indent\n").unwrap_err();
        assert!(
            err.line.is_some(),
            "a syntax error must locate itself: {err:?}"
        );
    }

    #[test]
    fn duplicate_task_ids_are_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n  - id: t\n    type: y\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "duplicate_task_id"),
            "{r:?}"
        );
    }

    /// Nested duplicates are the case a shallow check misses.
    #[test]
    fn duplicate_task_ids_are_found_inside_control_flow() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n  - id: outer\n    type: core.parallel\n    tasks:\n      - id: t\n        type: y\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "duplicate_task_id"),
            "{r:?}"
        );
    }

    #[test]
    fn a_bad_namespace_is_an_error() {
        let f = parse("id: f\nnamespace: Not-Valid\ntasks:\n  - id: t\n    type: x\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "invalid_namespace"),
            "{r:?}"
        );
    }

    /// The single most important behaviour in this task. A feature that parses
    /// but is not enforced must SAY SO: without this, a user writes
    /// `concurrency: {limit: 1}`, sees a green publish, and reasonably concludes
    /// it is enforced.
    #[test]
    fn declared_but_unimplemented_features_warn_and_name_their_package() {
        let f = parse(
            "id: f\nnamespace: a.b\nconcurrency:\n  limit: 1\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        let w = r
            .warnings
            .iter()
            .find(|w| w.code == "not_yet_enforced")
            .unwrap_or_else(|| panic!("no not_yet_enforced warning: {r:?}"));
        assert!(w.message.contains("concurrency"), "{}", w.message);
        assert!(
            w.message.contains("W8"),
            "the warning must name the package: {}",
            w.message
        );
        assert!(
            r.errors.is_empty(),
            "a declared-but-unimplemented feature is not an error"
        );
    }

    #[test]
    fn a_dag_cycle_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: x\n        dependsOn: [b]\n      - task:\n          id: b\n          type: x\n        dependsOn: [a]\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "dag_cycle"), "{r:?}");
    }

    #[test]
    fn a_subflow_naming_itself_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: a.b\n    flowId: f\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "self_subflow"), "{r:?}");
    }

    // -- Fix round 1 --

    /// Finding 1 (CRITICAL): `Pause` has no `TaskKind` variant, so it falls
    /// through to `Leaf` and is indistinguishable from an ordinary task --
    /// previously invisible to validation entirely. A user pasting a Kestra
    /// flow whose `Pause` is a manual-approval gate must be told it does not
    /// gate anything yet, not shown a green publish.
    #[test]
    fn pause_falls_through_to_leaf_and_warns_naming_its_package() {
        for spelling in ["io.kestra.plugin.core.flow.Pause", "core.pause"] {
            let yaml =
                format!("id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: {spelling}\n");
            let f = parse(&yaml).unwrap();
            let r = validate(&f);
            let w = r
                .warnings
                .iter()
                .find(|w| w.code == "not_yet_enforced" && w.path == "tasks.gate")
                .unwrap_or_else(|| {
                    panic!("{spelling}: no unported-control-flow warning for 'gate': {r:?}")
                });
            assert!(w.message.contains("W8"), "{spelling}: {}", w.message);
            assert!(
                r.errors.is_empty(),
                "{spelling}: an unported control-flow type is a warning, not an error: {r:?}"
            );
        }
    }

    /// An ordinary, ALREADY-ported control-flow type must not trip the
    /// unported-control-flow warning -- guards against the check matching
    /// too broadly (e.g. on canonical-name substring rather than exact
    /// lookup).
    #[test]
    fn an_ordinary_leaf_type_does_not_warn_as_unported_control_flow() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    sql: SELECT 1\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.code == "not_yet_enforced" && w.path == "tasks.t"),
            "an ordinary leaf type must not be mistaken for unported control flow: {r:?}"
        );
    }

    /// Finding 2 (Important): a `dependsOn` naming a task that does not
    /// exist in the same dag was silent -- `has_cycle` only ever walks
    /// edges that exist, so a dangling reference was simply absent from the
    /// adjacency map.
    #[test]
    fn dag_dependency_naming_a_missing_task_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: x\n        dependsOn: [nope]\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "dag_dependency_not_found"),
            "{r:?}"
        );
    }

    /// Companion to the above: a `dependsOn` naming a task that DOES exist
    /// must not be flagged -- the check must not be satisfiable by
    /// rejecting everything.
    #[test]
    fn dag_dependency_naming_a_real_task_is_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: x\n      - task:\n          id: b\n          type: x\n        dependsOn: [a]\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors
                .iter()
                .any(|e| e.code == "dag_dependency_not_found"),
            "a valid dependsOn chain must not be flagged: {r:?}"
        );
    }

    /// Finding 3 (Important): `validate()` never read `flow.inputs` at all,
    /// so duplicate input ids were silent.
    #[test]
    fn duplicate_input_ids_are_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: x\n    type: STRING\n  - id: x\n    type: STRING\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "duplicate_input_id"),
            "{r:?}"
        );
    }

    /// Companion: distinct input ids must not be flagged.
    #[test]
    fn distinct_input_ids_are_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: x\n    type: STRING\n  - id: y\n    type: STRING\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code == "duplicate_input_id"),
            "{r:?}"
        );
    }

    /// Finding 4 (Minor): the statically-decidable half of "Switch cases
    /// exhaustive-or-defaulted" -- neither `cases` nor `defaults` means the
    /// switch runs nothing under any match.
    #[test]
    fn switch_with_no_cases_and_no_defaults_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.switch\n    value: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "switch_no_branches"),
            "{r:?}"
        );
    }

    /// `defaults` alone is a complete, legitimate switch (matches nothing ->
    /// falls to `defaults`) and must not be flagged.
    #[test]
    fn switch_with_only_defaults_is_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.switch\n    value: x\n    defaults:\n      - id: d\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code == "switch_no_branches"),
            "{r:?}"
        );
    }

    /// `cases` alone is also legitimate (an author may know their input
    /// domain is fully covered) -- must not be flagged either, since
    /// exhaustiveness itself is an explicit, documented deferral, not an
    /// error.
    #[test]
    fn switch_with_only_cases_is_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.switch\n    value: x\n    cases:\n      a:\n        - id: c\n          type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code == "switch_no_branches"),
            "{r:?}"
        );
    }

    /// The rule that governs every check added in this fix round: false
    /// positives matter as much as false negatives, because this validator
    /// runs on every keystroke-pause in W7's editor. A legitimate flow using
    /// nested control flow, a valid `Dag` dependency chain,
    /// `errors:`/`finally:` handlers, and an input referencing another input
    /// via `dependsOn` must produce zero errors.
    #[test]
    fn a_legitimate_flow_with_nested_control_flow_produces_no_spurious_errors() {
        let yaml = "id: nightly\nnamespace: company.analytics\ninputs:\n  - id: run_date\n    type: DATE\n  - id: dry_run\n    type: BOOL\n    dependsOn:\n      inputs: [run_date]\ntasks:\n  - id: pipeline\n    type: core.dag\n    tasks:\n      - task:\n          id: extract\n          type: egret.sql\n          database: d\n          sql: SELECT 1\n      - task:\n          id: load\n          type: egret.sql\n          database: d\n          sql: SELECT 2\n        dependsOn: [extract]\n  - id: fanout\n    type: core.parallel\n    tasks:\n      - id: enrich\n        type: egret.python\n        notebook: nb\nerrors:\n  - id: alert\n    type: core.slack\nfinally:\n  - id: cleanup\n    type: core.log\n";
        let f = parse(yaml).unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    // -- Fix round 2: fail-CLOSED on the namespace, not on table membership --

    /// The test that would have caught round 1's regression: a Kestra
    /// control-flow FQCN this crate has never heard of -- not in
    /// `KESTRA_FLOW_CONTROL` at all -- must still warn, because the whole
    /// point is catching the next class Kestra adds BEFORE anyone notices it
    /// and adds a row for it.
    #[test]
    fn a_fictional_kestra_flow_control_fqcn_warns_even_with_no_table_row() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: io.kestra.plugin.core.flow.NotARealClass\n",
        )
        .unwrap();
        let r = validate(&f);
        let w = r
            .warnings
            .iter()
            .find(|w| w.code == "not_yet_enforced" && w.path == "tasks.gate")
            .unwrap_or_else(|| {
                panic!("a Kestra flow-control FQCN with no table row must still warn: {r:?}")
            });
        assert!(
            !w.message.contains("W1")
                && !w.message.contains("W3")
                && !w.message.contains("W6")
                && !w.message.contains("W8"),
            "a class with no table row must not fabricate a package name: {}",
            w.message
        );
        assert!(
            r.errors.is_empty(),
            "still a warning, never an error: {r:?}"
        );
    }

    /// The same failure mode via the `core.` alias space: an unregistered
    /// alias that looks like it could be a Kestra control-flow shorthand
    /// must warn too, not just the FQCN spelling.
    #[test]
    fn an_unregistered_core_alias_also_warns() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: core.somenewgate\n")
            .unwrap();
        let r = validate(&f);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "not_yet_enforced" && w.path == "tasks.gate"),
            "{r:?}"
        );
    }

    /// Anything that is neither a `TaskKind` variant nor
    /// `registry::known_type(...)`, and does not even look like Kestra's
    /// control-flow namespace, still warns -- under its own code, so a typo
    /// like `egret.sqll` is visible to the author before Task 6 turns it
    /// into a hard publish refusal.
    #[test]
    fn a_completely_unknown_type_warns_as_unknown_task_type() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sqll\n").unwrap();
        let r = validate(&f);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "unknown_task_type" && w.path == "tasks.t"),
            "{r:?}"
        );
        assert!(
            r.errors.is_empty(),
            "an unknown type is a warning today, not an error -- Task 6 hard-refuses it at publish: {r:?}"
        );
    }

    /// The false-positive guard the fix round asked for by name: egret's own
    /// registered leaf types (`core.log`/`core.http`/`core.slack`, which
    /// share the `core.` alias spelling with real Kestra control-flow
    /// aliases), an ordinary `egret.*` leaf, and every ported control-flow
    /// type must all stay completely silent -- "neither a control-flow
    /// variant nor a registered type", not "starts with `core.`".
    #[test]
    fn registered_leaf_types_and_every_ported_control_flow_type_stay_silent() {
        let yaml = "id: f\nnamespace: a.b\ntasks:\n  - id: log\n    type: core.log\n  - id: http\n    type: core.http\n  - id: slack\n    type: core.slack\n  - id: sql\n    type: egret.sql\n    sql: SELECT 1\n  - id: seq\n    type: core.sequential\n    tasks:\n      - id: seq_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: par\n    type: core.parallel\n    tasks:\n      - id: par_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: cond\n    type: core.if\n    condition: \"true\"\n    then:\n      - id: cond_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: sw\n    type: core.switch\n    value: x\n    defaults:\n      - id: sw_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: d_child\n          type: egret.sql\n          sql: SELECT 1\n  - id: lp\n    type: core.loop\n    values: [1]\n    tasks:\n      - id: lp_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: lpu\n    type: core.loopuntil\n    condition: x\n    tasks:\n      - id: lpu_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: sf\n    type: core.subflow\n    namespace: a.b\n    flowId: other\n  - id: af\n    type: core.allowfailure\n    tasks:\n      - id: af_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: wd\n    type: core.workingdirectory\n    tasks:\n      - id: wd_child\n        type: egret.sql\n        sql: SELECT 1\n";
        let f = parse(yaml).unwrap();
        let r = validate(&f);
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.code == "not_yet_enforced" || w.code == "unknown_task_type"),
            "registered leaf types and every ported control-flow type must stay silent: {r:?}"
        );
        assert!(r.errors.is_empty(), "{r:?}");
    }

    // -- Fix round 3: 11 control-flow fields were hard-required at parse
    // where Kestra's requirement is bean-validation only (parse must accept
    // absent/null; only THIS layer may reject it). Each pair below tests
    // both directions: absent -> a semantic error with the right code (not
    // a parse error), and present -> silent. Without the "present -> silent"
    // half, a check that rejected everything would still pass.

    #[test]
    fn sequential_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f =
            parse("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.sequential\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "sequential_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn sequential_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.sequential\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn parallel_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f =
            parse("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: core.parallel\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "parallel_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn parallel_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: core.parallel\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn if_condition_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    then:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "if_condition_required"),
            "{r:?}"
        );
    }

    #[test]
    fn if_condition_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    condition: \"true\"\n    then:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn dag_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "dag_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn dag_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: c\n          type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    /// The weakest of the 11 (a dag node with no task is unusable either
    /// way), fixed for consistency rather than a concrete need -- see
    /// `DagNode::task`'s doc comment in `task.rs`.
    #[test]
    fn dag_node_task_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - {}\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "dag_node_task_required"),
            "{r:?}"
        );
    }

    #[test]
    fn dag_node_task_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: c\n          type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn loop_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "loop_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn loop_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn loop_until_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loopuntil\n    condition: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "loop_until_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn loop_until_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loopuntil\n    condition: x\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn subflow_namespace_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    flowId: other\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "subflow_namespace_required"),
            "{r:?}"
        );
    }

    #[test]
    fn subflow_namespace_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: a.b\n    flowId: other\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn subflow_flow_id_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: a.b\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "subflow_flow_id_required"),
            "{r:?}"
        );
    }

    #[test]
    fn subflow_flow_id_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: a.b\n    flowId: other\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn allow_failure_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: a\n    type: core.allowfailure\n")
            .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "allow_failure_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn allow_failure_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: a\n    type: core.allowfailure\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn working_directory_tasks_absent_is_a_semantic_error_not_a_parse_error() {
        let f =
            parse("id: f\nnamespace: a.b\ntasks:\n  - id: w\n    type: core.workingdirectory\n")
                .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "working_directory_tasks_required"),
            "{r:?}"
        );
    }

    #[test]
    fn working_directory_tasks_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: w\n    type: core.workingdirectory\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    /// Secondary item: `Loop::concurrency_limit` accepted absence (it has a
    /// `#[serde(default = "default_concurrency_limit")]`) but rejected an
    /// explicit `null` before this round -- the same asymmetry
    /// `Parallel::concurrent`/`Dag::concurrent` were already fixed against.
    #[test]
    fn loop_concurrency_limit_explicit_null_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n    concurrencyLimit: null\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "loop_concurrency_limit_required"),
            "{r:?}"
        );
    }

    #[test]
    fn loop_concurrency_limit_absent_key_defaults_and_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n    tasks:\n      - id: c\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors
                .iter()
                .any(|e| e.code == "loop_concurrency_limit_required"),
            "an absent key must take Kestra's own default of 1, not error: {r:?}"
        );
    }

    /// Finding 2: nothing enforces that `registry::TASK_TYPES` and
    /// `KESTRA_FLOW_CONTROL` are disjoint. If a future edit ever registers a
    /// leaf type whose `r#type` canonicalises onto a `KESTRA_FLOW_CONTROL`
    /// row (e.g. registering `core.pause` before a real `TaskKind::Pause`
    /// variant exists), the `registry::known_type` guard on the
    /// `TaskKind::Leaf` arm would silence that type -- reproducing this
    /// task's original bug one layer further out. This makes that collision
    /// impossible to introduce quietly.
    #[test]
    fn the_registry_and_the_control_flow_table_are_disjoint() {
        use crate::models::canonical_flow_type;
        use crate::spec::registry;

        for spec in registry::TASK_TYPES {
            if let Some(canonical) = canonical_flow_type(spec.r#type) {
                assert!(
                    !super::KESTRA_FLOW_CONTROL
                        .iter()
                        .any(|(name, _)| *name == canonical),
                    "registry type '{}' canonicalises to '{canonical}', which collides with a \
                     KESTRA_FLOW_CONTROL row -- registering it would silence the \
                     unported-control-flow warning for that class",
                    spec.r#type
                );
            }
        }
    }
}
