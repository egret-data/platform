//! `validate(flow: &Flow) -> ValidationReport` -- egret's own three
//! validation layers, run in order (spec section 9):
//!
//! 1. **Structural** -- shape a flow must have regardless of what its tasks
//!    do: unique task ids across the WHOLE tree (walked via `Task::children()`,
//!    not just `flow.tasks` -- `duplicate_task_ids_are_found_inside_control_flow`
//!    is what proves the walk is recursive), no dependency cycle inside a
//!    `core.dag`, and no `core.subflow` that names the flow it lives in.
//! 2. **Semantic** -- fields Tasks 1-3 left `Option<T>` purely so parsing
//!    would accept `null`/absent the way Kestra's Jackson layer does,
//!    deferring the `@NotNull` bean-validation check to here. Every field
//!    tagged "Task 4 owes ..." in `models/` is checked in this layer; see
//!    the task report for the full list found by grepping for that phrase.
//! 3. **Not-yet-enforced** -- features that parse and validate cleanly but
//!    that no package in this branch actually acts on yet
//!    (`NOT_YET_ENFORCED` below). These are warnings, never errors: a
//!    declared-but-unimplemented feature must still publish, or a green
//!    publish would lie about what the product does.

use std::collections::{HashMap, HashSet};

use crate::models::{Flow, Retry, Sla, SlaCommon, Task, TaskKind, Trigger, TriggerKind};

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
        TaskKind::Parallel(p) => {
            if p.concurrent.is_none() {
                report.errors.push(Finding {
                    code: "parallel_concurrent_required",
                    message: format!("task '{}': concurrent is required (explicit null)", task.id),
                    path: format!("{path}.concurrent"),
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
            let edges: Vec<(String, Vec<String>)> = d
                .tasks
                .iter()
                .map(|n| (n.task.id.clone(), n.depends_on.clone()))
                .collect();
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
        }
        TaskKind::Loop(l) => {
            if l.values.is_none() {
                report.errors.push(Finding {
                    code: "loop_values_required",
                    message: format!("task '{}': values is required (explicit null)", task.id),
                    path: format!("{path}.values"),
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
        }
        TaskKind::Subflow(s) if s.namespace == flow.namespace && s.flow_id == flow.id => {
            report.errors.push(Finding {
                code: "self_subflow",
                message: format!(
                    "task '{}' is a subflow that names its own flow ('{}/{}')",
                    task.id, s.namespace, s.flow_id
                ),
                path: path.clone(),
            });
        }
        _ => {}
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
}
