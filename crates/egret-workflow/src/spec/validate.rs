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
//!    `@NotNull`/`@NotEmpty` bean-validation check to here. A field tagged
//!    "Task 4 owes ..." in `models/` is where this layer's coverage STARTED,
//!    plus 11 control-flow fields fix round 3 found still hard-required at
//!    parse (`Sequential`/`Parallel`/`Dag`/`Loop`/`LoopUntil`/
//!    `AllowFailure`/`WorkingDirectory`'s `tasks`, `If::condition`/`then`,
//!    `DagNode::task`, `Subflow`/`Dependency`'s `namespace`/`flow_id`) and
//!    `Loop::concurrency_limit`, which accepted absence but rejected an
//!    explicit `null`; see the task report for the full pairing of field ->
//!    `walk_task` arm -> test codes. `namespace`'s pattern (on `Flow`,
//!    `Subflow`, and `Dependency` alike) and `Flow::id`'s; input ids
//!    unique; a `Switch` with neither `cases` nor `defaults` (the
//!    statically-decidable half of "exhaustive-or-defaulted" -- see the
//!    `Switch` arm in `walk_task` for why the other half is an explicit,
//!    documented deferral rather than a check); `Quota`'s and `Check`'s
//!    `@NotNull` fields (`validate_flow_level_fields`).
//!
//!    **The "Task 4 owes" tag is a HISTORY marker, not a completeness
//!    guarantee, and must not be read as one.** Whole-branch review, minor
//!    finding 5: `Quota`'s and `Check`'s `@NotNull` fields were written up
//!    in prose in `models/flow.rs` but never tagged, so this layer's own
//!    coverage (built by walking tagged fields) silently excluded them --
//!    the mitigation against silent omission reproduced silent omission.
//!    The only claim this file's scope actually stands behind is what its
//!    OWN test module exercises: a field with a `@NotNull`/`@NotEmpty`
//!    debt is covered if and only if a test in `tests` (below) proves a
//!    violating value is rejected. When you add a bean-validation debt to a
//!    `models/` type, add its check here and a test for it in the SAME
//!    change -- do not rely on a later "sweep" to find the tag.
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

use regex::Regex;
use serde::Serialize;

use crate::models::{
    canonical_flow_type, Flow, Input, InputType, Retry, Sla, SlaCommon, Task, TaskKind, Trigger,
    TriggerKind,
};
use crate::spec::registry;
use crate::trigger::depends_on::{depends_on_refusals, RefusalAt};

/// A single validation finding: a machine-checkable `code`, a human message,
/// and a `path` locating it inside the flow.
///
/// `Serialize` (Task 6): a warning must survive into `publish`'s frozen
/// snapshot, so a reader of a published version can see what was declared
/// but is not yet enforced -- see `publish`'s own doc comment.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
///
/// # Two rows were false, and nothing here could have said so
///
/// This list is a **user-visible claim about this codebase's behaviour**, and
/// it had not been re-read since it was written. W4 Task 2's fix round found
/// `("timeout", "W3")` still here while the same commit enforced `timeout:` in
/// the worker — a flow with `timeout: PT1S` was told "not yet enforced;
/// enforcement lands in W3" and then had its task failed at one second. Reading
/// the other six rows rather than only the reported one found a **second**:
///
/// * **`retry` was enforced by W3a Task 7**, and by both paths.
///   `plan::build_plan` reads `flow.retry` (`plan.rs:662`) and `task.retry`
///   (`:745`) through `retry::policy_of`, `retry::policy_for` walks a run's
///   parent chain and falls back to the flow's, and `retry::scan` emits
///   `Command::ScheduleRetry`. W4 Task 1 then added the waker that makes the
///   delay elapse. Fourteen baselined corpus entries declare one.
/// * **`timeout` is enforced by W4 Task 2**, in `worker::timeout`, around
///   `Registry::run` — which is where the artifact enforces it too.
///
/// Both rows are gone. **`sla` is the third row to be re-checked and the
/// FIRST to be narrowed rather than kept or deleted** — W8 Task 6. It said
/// *"`concurrency`, `sla`, `checks` and `quotas` have W1 model shapes and no
/// reader outside this validator"*, and half of that is now false: `crate::sla`
/// reads `Flow::sla` and `worker::claim::settle` acts on it. But only half —
/// `SLA.Type` has two subtypes and they are two different mechanisms in the
/// artifact (see [`crate::sla`]), of which `EXECUTION_ASSERTION` is enforced
/// and `MAX_DURATION` is not. So the row stays and its PREDICATE narrows:
/// [`FeaturePresence::sla`] is now *declares a `MAX_DURATION`*, and a flow
/// whose `sla:` is assertions only publishes without the warning, because for
/// that flow the warning would be false. Warning about an enforced feature is
/// the mirror of the divergence `plan::pause_gap` refuses — *"a publish that
/// says yes over an executor that says no"* — and it costs a user the same
/// way: they read the warning and work around a rule that is already working.
///
/// The three that remain un-narrowed were re-checked the same way:
/// `concurrency`, `checks` and `quotas` have W1 model shapes and no reader
/// outside this validator, and `triggers` is W6's.
/// `oracle-exec/README.md`'s unclaimed table records the same split for
/// `sla:`. **It no longer has a row for flow `concurrency:` and that does not
/// change this one**: W8 Task 1
/// deleted that row because the construct holds no baselined entry under the
/// execution oracle, which is a statement about what the RATCHET can score and
/// not about whether this validator must warn. A flow declaring `concurrency:`
/// still gets no enforcement, so the warning here stays exactly as it was —
/// the two tables answer different questions, and conflating them is how a
/// deleted row turns into a silently dropped warning.
///
/// `the_not_yet_enforced_list_is_pinned_in_both_directions` is what makes a
/// wrong row fail from now on — the reported finding was not only that the row
/// was stale but that **no test pinned it**, so a false claim could sit here
/// indefinitely.
const NOT_YET_ENFORCED: &[(&str, &str)] = &[
    ("concurrency", "W8"),
    ("sla", "W8"),
    ("checks", "W8"),
    ("quotas", "W8"),
    ("triggers", "W6"),
];

/// Whether a Kestra `io.kestra.plugin.core.flow.*` class that changes what
/// runs next (`FlowableTask`/`ExecutableTask` -- as opposed to an ordinary
/// `RunnableTask` that just runs and produces an output, like `Sleep` or
/// `PurgeFlows`, which live in the same Java package but are *not* listed
/// here) has a `TaskKind` variant in this crate.
///
/// Every canonical name this crate treats as Kestra control flow — the
/// SPECIFICATION half of a two-table equality guard, and **not a production
/// lookup any more**. W8 Task 3 fix round 1.
///
/// # What it used to do, and why it stopped
///
/// Two callers read it: `is_runnable_child`, which refused a `Leaf` whose
/// canonical name had a row, and `walk_task`, which read a row's package name
/// into the `not_yet_enforced` message. **Both arms are unreachable now**, for
/// one reason: `models::kind_for` dispatches every name below to a typed
/// `TaskKind` variant, so no task carrying one of them can reach `Leaf` at all.
/// Measured — the two name sets are identical, 11 each, difference empty in
/// both directions — and both dead arms are deleted rather than commented,
/// which is what this crate did with the two `isRetrying` guards W3a measured
/// unreachable.
///
/// # So what is it FOR
///
/// It is the list a reviewer checks when Kestra's `flow/` package grows a
/// class, and `the_two_control_flow_tables_name_the_same_types` is what stops
/// it drifting from `kind_for` in silence. That equality is a **new invariant
/// this task created** — before W8 Task 3 the two tables genuinely differed,
/// by `pause` — so it is stated by a running program rather than left to be
/// remembered.
///
/// **`waitfor` was in NEITHER, and W8 Task 4 closed it — in `kind_for`, with
/// this table following.** `io.kestra.plugin.core.flow.WaitFor` is Kestra's
/// alias for `LoopUntil` (`LoopUntil.java`'s `@Plugin(aliases = …)`), and
/// while `kind_for` had no row for it a `WaitFor` reached `Leaf`, so a
/// `WaitFor` child of a `WorkingDirectory` published clean where the identical
/// `LoopUntil` child was refused with `working_directory_child_not_runnable`.
/// Since `is_runnable_child` is `matches!(kind, Leaf)` and consults this table
/// no longer, the fix was a `kind_for` arm — which the equality guard below
/// then forced back into this table, so the one-table edit could not have
/// passed. **That is what made Step 0 a decision rather than a patch**: the
/// two tables are one fact with two spellings, and the authority is `kind_for`.
///
/// **The publish hole is closed and the class is now empty.** Of the 70
/// distinct `type:` strings in the corpus, the names that reach `Leaf` through
/// `canonical_flow_type` are `Sleep` and `WorkingDirectoryTest$LocalFiles`,
/// both `RunnableTask` at the pin and correctly accepted. There is no third.
///
/// `#[cfg(test)]` because a production constant nothing reads is a lie about
/// where the authority lives; the authority is `kind_for`, and this is the
/// claim held against it.
#[cfg(test)]
const KESTRA_FLOW_CONTROL: &[&str] = &[
    "sequential",
    "parallel",
    "if",
    "switch",
    "dag",
    "loop",
    "loopuntil",
    "subflow",
    "allowfailure",
    "workingdirectory",
    "pause",
    // W8 Task 4. Kestra's alias on `LoopUntil`, not a class of its own, which
    // is why reading `core/src/main/java/io/kestra/plugin/core/flow/` could
    // never have produced it and why it was missing from both tables for five
    // packages.
    "waitfor",
];

/// Whether a task may be a child of a [`TaskKind::WorkingDirectory`] --
/// Kestra's `task instanceof RunnableTask<?>`, as far as a model with no
/// classpath can read it.
///
/// Two ways to answer no, and they cover different flows:
///
/// - a typed [`TaskKind`] variant. Every one of them is a Kestra flowable or
///   `ExecutableTask`; none is a `RunnableTask`. This catches a `Sequential`,
///   a `Parallel`, a `Subflow`, a nested `WorkingDirectory`, ...
/// - a `Leaf` whose canonical type has a `KESTRA_FLOW_CONTROL` row. This is
///   the arm `invalids/workingdirectory-invalid` needs: its offending child
///   is a `Pause`, which is a real Kestra flowable that this crate has NOT
///   ported, so it reaches `Leaf` and is otherwise indistinguishable from an
///   ordinary task.
///
/// Anything else -- `egret.sql`, `io.kestra.plugin.core.log.Log`, a
/// third-party plugin, `io.kestra.plugin.core.flow.Sleep` -- is runnable as
/// far as this crate can tell, which is the fail-open direction. See the
/// caller for why that is the right way round for a refusal.
///
/// **The member of that fail-open class this crate got WRONG was
/// `io.kestra.plugin.core.flow.WaitFor`, and W8 Task 4 closed it.** It is a
/// real Kestra flowable that had no row in either table, because it is
/// `LoopUntil`'s alias rather than its own class (see that table's doc), so it
/// reached `TaskKind::Leaf` and was accepted as a child where the `LoopUntil`
/// spelling of the very same construct was refused by the typed-variant arm.
/// Measured at W8 Task 1 fix round 5 and deliberately not fixed there -- the
/// table is production and that round was doc-only. It is fixed at the
/// authority instead: `models::kind_for` now maps both spellings to
/// `TaskKind::LoopUntil`, so `matches!(kind, Leaf)` is false for both and the
/// two spellings are refused identically.
/// `a_waitfor_child_of_a_working_directory_is_refused_like_a_loopuntil_one` is
/// the instrument, and it drives BOTH spellings so a fix that reached only one
/// of them fails. Latent until then: every `WorkingDirectory` child in the
/// corpus is a `WorkingDirectoryTest$LocalFiles`, so **no golden entry can
/// show it** and the ratchet cannot judge this line either way.
fn is_runnable_child(task: &Task) -> bool {
    matches!(task.kind, TaskKind::Leaf { .. })
}

/// Which of `NOT_YET_ENFORCED`'s features this flow actually declares.
///
/// **All five are flow-level, and that is new.** It used to be accumulated
/// during the task walk as well, because `retry:` and `timeout:` are
/// task-level and were on the list. Both are enforced now (see
/// `NOT_YET_ENFORCED`), so nothing remains that a task can declare, and
/// `walk_task` no longer takes this at all — clippy's `only_used_in_recursion`
/// is what pointed that out, on a parameter the walk was still threading
/// through and no longer reading.
#[derive(Debug, Default)]
struct FeaturePresence {
    concurrency: bool,
    /// **Declares a `MAX_DURATION`**, not "declares an `sla:`" — W8 Task 6.
    ///
    /// See [`NOT_YET_ENFORCED`]'s doc: `EXECUTION_ASSERTION` is enforced from
    /// W8 Task 6 and `MAX_DURATION` is not, so the flow-level question this
    /// warning answers is no longer the same as "is the key present".
    /// `an_execution_assertion_only_sla_does_not_warn_and_a_max_duration_does`
    /// is the pair that pins it.
    sla: bool,
    checks: bool,
    quotas: bool,
    triggers: bool,
}

/// **The save-time report**: what a draft shows and what blocks publish
/// (`routes::draft_dto`, `POST .../validate`, `publish`). It is
/// [`validate_for_run`] plus the findings that only SAVING a flow may raise
/// ([`save_only_findings`]).
///
/// W5b fix round 1 (user ruling 2A): `pluginDefaults:` is refused here and
/// NOT at run time. egret dropped the key silently before W5b, so a version
/// published then may carry it; spec §4.1: *"The refusal shows on a draft and
/// blocks publish. A draft that runs anyway applies nothing, as Kestra's read
/// path does."* The run-time gate, `executor::build_plan`, calls
/// [`validate_for_run`], which cannot raise a save-only finding because it
/// never calls [`save_only_findings`] -- the separation is in which function
/// is called, not in a flag a caller must remember to pass.
pub fn validate(flow: &Flow) -> ValidationReport {
    let mut report = validate_for_run(flow);
    save_only_findings(flow, &mut report);
    report
}

/// Findings that refuse SAVING a flow but must never stop a stored version
/// from running (see [`validate`]). Today: a top-level `pluginDefaults:`,
/// whatever its value, on every flow (W5b spec §4.1) -- Kestra 2.0 removed it
/// and refuses the key at save.
fn save_only_findings(flow: &Flow, report: &mut ValidationReport) {
    use crate::plugins;
    if flow.plugin_defaults.is_some() {
        report.errors.push(Finding {
            code: plugins::CODE_REMOVED_PLUGIN_DEFAULTS,
            message: plugins::REMOVED_PLUGIN_DEFAULTS_MESSAGE.to_string(),
            path: "pluginDefaults".to_string(),
        });
    }
    secret_findings(flow, report);
}

/// W5b fix round 1 (I2), spec §6: `secret()` resolves only in a `kestra.*`
/// task's properties, so a call anywhere else -- a script task's `commands`
/// or `env`, `runIf`, an input, a `Log`, a trigger, a flowable's own fields --
/// is an error naming where it is; and on a plugin task, a `namespace=` or
/// `full=` written in the call is an error naming the argument.
///
/// **Save-only**, for [`save_only_findings`]' reason: the render refuses both
/// at run anyway (`functions::SECRET_REFUSAL`, `secrets::parse_call`), so a
/// stored draft that carries one still plans and fails at the task, as before.
///
/// Walks the flow as it serialises -- every string egret keeps, keys too --
/// with the plugin tasks (`plugins::plugin_tasks`, which walks `errors:`
/// children) exempt for their PROPERTIES only: their `id`, `runIf`,
/// `timeout` and the other keys egret's `Task` model reads are walked like
/// everything else, because the plugin render never sees them. A task
/// object is recognised as a plugin task by its (`id`, `type`) pair.
fn secret_findings(flow: &Flow, report: &mut ValidationReport) {
    use crate::runners::secrets::{
        refused_argument_message, text_uses_secret, written_refused_arguments,
    };
    use std::collections::HashSet;

    /// What `models::Task` reads itself; everything else on a leaf is its spec.
    const TASK_KEYS: [&str; 9] = [
        "id",
        "type",
        "description",
        "retry",
        "timeout",
        "disabled",
        "runIf",
        "allowFailure",
        "allowWarning",
    ];

    fn strings<'a>(v: &'a serde_json::Value, path: String, out: &mut Vec<(String, &'a str)>) {
        match v {
            serde_json::Value::String(s) => out.push((path, s)),
            serde_json::Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    strings(item, format!("{path}[{i}]"), out);
                }
            }
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.push((path.clone(), k));
                    strings(v, join(&path, k), out);
                }
            }
            _ => {}
        }
    }

    fn join(path: &str, key: &str) -> String {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    }

    struct Walk<'a> {
        plugin: HashSet<(&'a str, &'a str)>,
        report: &'a mut ValidationReport,
    }

    impl Walk<'_> {
        fn outside(&mut self, path: String, text: &str) {
            if text_uses_secret(text) {
                self.report.errors.push(Finding {
                    code: crate::plugins::CODE_SECRET_OUTSIDE_PLUGIN_TASK,
                    message: format!(
                        "secret() is refused at {path}: {}",
                        crate::runners::SECRET_REFUSAL
                    ),
                    path,
                });
            }
        }

        fn property(&mut self, path: String, text: &str) {
            for arg in written_refused_arguments(text) {
                self.report.errors.push(Finding {
                    code: crate::plugins::CODE_SECRET_ARGUMENT_REFUSED,
                    message: refused_argument_message(arg),
                    path: path.clone(),
                });
            }
        }

        fn value(&mut self, v: &serde_json::Value, path: String) {
            match v {
                serde_json::Value::String(s) => self.outside(path, s),
                serde_json::Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        // A list of tasks (or inputs, triggers) reads by id.
                        let here = match item.get("id").and_then(serde_json::Value::as_str) {
                            Some(id) => join(&path, id),
                            None => format!("{path}[{i}]"),
                        };
                        self.value(item, here);
                    }
                }
                serde_json::Value::Object(map) => {
                    let id = map.get("id").and_then(serde_json::Value::as_str);
                    let ty = map.get("type").and_then(serde_json::Value::as_str);
                    let is_plugin =
                        matches!((id, ty), (Some(i), Some(t)) if self.plugin.contains(&(i, t)));
                    for (k, v) in map {
                        let here = join(&path, k);
                        self.outside(path.clone(), k);
                        if is_plugin && !TASK_KEYS.contains(&k.as_str()) {
                            let mut found = Vec::new();
                            strings(v, here, &mut found);
                            for (p, text) in found {
                                self.property(p, text);
                            }
                        } else {
                            self.value(v, here);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let Ok(doc) = serde_json::to_value(flow) else {
        return;
    };
    let tasks = crate::plugins::plugin_tasks(flow);
    let mut walk = Walk {
        plugin: tasks
            .iter()
            .map(|(_, t, _)| (t.id.as_str(), t.r#type.as_str()))
            .collect(),
        report,
    };
    walk.value(&doc, String::new());
}

/// **The run-time gate** (`executor::build_plan`'s third gate): every
/// structural, semantic and not-yet-enforced finding -- everything
/// [`validate`] reports except [`save_only_findings`]. A stored version that
/// passes this runs.
pub fn validate_for_run(flow: &Flow) -> ValidationReport {
    let mut report = ValidationReport::default();

    validate_namespace(flow, &mut report);
    validate_flow_id(flow, &mut report);
    validate_flow_level_fields(flow, &mut report);

    let features = FeaturePresence {
        concurrency: flow.concurrency.is_some(),
        // **The narrowing, and it is the whole of it** -- W8 Task 6. `matches!`
        // over the variant rather than a text needle: a flow can write either
        // subtype, both, or the same subtype twice, and only the presence of a
        // `MAX_DURATION` makes "not yet enforced" a true sentence.
        sla: flow
            .sla
            .iter()
            .any(|s| matches!(s, Sla::MaxDuration { .. })),
        checks: !flow.checks.is_empty(),
        quotas: !flow.quotas.is_empty(),
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
            walk_task(flow, task, path, &mut seen_ids, &mut report);
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

    validate_script_tasks(flow, &mut report);
    validate_plugin_tasks(flow, &mut report);

    validate_inputs(&flow.inputs, &mut report);

    for trigger in &flow.triggers {
        validate_trigger(trigger, &mut report);
    }
    validate_schedule_inputs(flow, &mut report);
    validate_dashed_references(flow, &mut report);

    emit_not_yet_enforced(&features, &mut report);

    report
}

/// W5a: every script task (`crate::scripts`) is parsed, and each property,
/// runner or value W5a does not implement is an ERROR naming it -- never
/// ignored (W5a spec §3). Walks `children()` AND `error_children()` through
/// `scripts::script_tasks`, unlike `walk_task`, so a script task inside a
/// flowable's `errors:` is refused too.
///
/// The governance gate (feature, allowlist) is NOT here: it needs the
/// workspace and the platform configuration, which this pure function does
/// not have. `publish::publish_gated` runs it after this.
fn validate_script_tasks(flow: &Flow, report: &mut ValidationReport) {
    for (path, task, _kind, spec) in crate::scripts::script_tasks(flow) {
        if let Err(refusals) = crate::scripts::parse(&task.r#type, spec) {
            for r in refusals {
                report.errors.push(Finding {
                    code: r.code,
                    message: format!("task '{}': {}", task.id, r.message),
                    path: if r.path.is_empty() {
                        path.clone()
                    } else {
                        format!("{path}.{}", r.path)
                    },
                });
            }
        }
    }
}

/// W5b: every Kestra plugin task (`crate::plugins`, `type: kestra.<FQCN>`)
/// is parsed against the embedded catalog, and each refusal is an ERROR
/// naming it (spec §4). A bare catalog FQCN is an error carrying the prefixed
/// spelling. Both walk `children()` AND `error_children()`
/// (`plugins::plugin_tasks`), unlike `walk_task`, so a plugin task inside a
/// flowable's `errors:` is refused too -- which is also why the bare-FQCN
/// error is raised here rather than in `walk_task`'s `Leaf` arm.
///
/// (`pluginDefaults:` is NOT here: it is a save-only finding, see
/// [`save_only_findings`].)
///
/// The governance gate (features, `allowed-types`, a runner) is NOT here;
/// `publish::check_plugin_tasks` runs it after this.
fn validate_plugin_tasks(flow: &Flow, report: &mut ValidationReport) {
    use crate::plugins;
    let refused = plugins::plugin_tasks(flow)
        .into_iter()
        .filter_map(|(path, task, spec)| {
            plugins::parse(&task.r#type, spec)
                .err()
                .map(|rs| (path, task, rs))
        })
        .chain(
            plugins::bare_plugin_types(flow)
                .into_iter()
                .map(|(path, task, r)| (path, task, vec![r])),
        );
    for (path, task, refusals) in refused {
        for r in refusals {
            report.errors.push(Finding {
                code: r.code,
                message: format!("task '{}': {}", task.id, r.message),
                path: if r.path.is_empty() {
                    path.clone()
                } else {
                    format!("{path}.{}", r.path)
                },
            });
        }
    }
}

/// `namespace` against Kestra's own pattern (`AbstractFlow.java`):
/// `^[a-z0-9][a-z0-9._-]*`, `@Size(min = 1, max = 150)`. A hand-written
/// character check, not a `regex` dependency for one pattern. W9b: then no
/// `_files` segment (`nsfiles::access::check_files_segment`), a second finding
/// only when the pattern passes, so no message is doubled.
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
    } else if let Err(message) =
        crate::storage::nsfiles::access::check_files_segment(&flow.namespace)
    {
        // W9b: a `_files` segment would put this namespace's execution files
        // under another namespace's files (W9b spec §3). The same function
        // refuses it at run time (`nsfiles::access::check_namespace`, which
        // `authorize` calls). Only this rule: an empty segment (`a..b`) still
        // publishes, and is refused by namespace files alone (fix round 1 M3).
        report.errors.push(Finding {
            code: "namespace_files_segment",
            message,
            path: "namespace".to_string(),
        });
    }
}

/// `Flow::id` against the SAME `is_valid_flow_id` a `Subflow::flow_id`
/// (`TaskKind::Subflow`'s arm, below) and now a `Dependency::flow_id`
/// (`validate_trigger`, above) are both already checked against -- Kestra's
/// own flow `id` pattern, `AbstractFlow.java`'s `@Pattern`/`@Size`. Whole-
/// branch review, minor finding 4: `Flow::id` was the ONE place in this
/// crate that pattern was defined but never applied to the field it exists
/// for -- `id: 'BAD ID/with slash'` and `id: ''` both validated clean before
/// this. `flow.id` is `String`, not `Option<String>` (no separate
/// `@NotBlank` debt to carry the way `Subflow::flow_id` has -- an absent
/// `id` key is already a *parse* error, since the field has no
/// `#[serde(default)]`), so this is a single pattern check, not a
/// presence-then-pattern pair like the `Option`-wrapped fields this file
/// otherwise checks.
fn validate_flow_id(flow: &Flow, report: &mut ValidationReport) {
    if !is_valid_flow_id(&flow.id) {
        report.errors.push(Finding {
            code: "invalid_flow_id",
            message: format!(
                "id '{}' must match ^[a-zA-Z0-9][a-zA-Z0-9._-]* and be 1-100 characters long",
                flow.id
            ),
            path: "id".to_string(),
        });
    }
}

pub(crate) fn is_valid_namespace(ns: &str) -> bool {
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

/// A Kestra flow `id` against Kestra's OWN pattern for that field --
/// `AbstractFlow.java` (`data-portal/kestra` @ `96893b589`, lines 30-33):
/// `@Pattern(regexp = "^[a-zA-Z0-9][a-zA-Z0-9._-]*") @Size(min = 1, max =
/// 100)`. Deliberately NOT [`is_valid_namespace`] reused under a different
/// name: a flow id's alphabet allows UPPERCASE letters (`namespace`'s does
/// not) and its length cap is 100, not 150 -- Kestra draws these as two
/// distinct patterns on two distinct fields, and collapsing them here would
/// silently narrow what a real Kestra flow id can be.
///
/// Fix round 2 (Task 6), Finding 1: this is what makes
/// `Subflow::flow_id` -- half of the pair `publish::subflow_ref_name`
/// encodes into one `name_or_id` as `"{namespace}/{flowId}"` -- unable to
/// carry a `/`. See `is_valid_namespace`'s use in the `TaskKind::Subflow`
/// arm below for the other half.
pub(crate) fn is_valid_flow_id(id: &str) -> bool {
    let len = id.chars().count();
    if len == 0 || len > 100 {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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

    // Whole-branch review, minor finding 5: `Quota.java`'s `@NotNull
    // Duration duration`, `@NotNull @Positive Long limit`, and `@NotNull
    // Behavior behavior` -- `quotas: [{}]` validated with zero errors
    // before this, the same "declared but never checked" gap `Check` below
    // shares. See this file's module doc for why these were missed (a
    // selector-coverage bug, not a one-off oversight) and why the fix is
    // both the checks below AND that doc's reworded claim.
    for (i, quota) in flow.quotas.iter().enumerate() {
        if quota.duration.is_none() {
            report.errors.push(Finding {
                code: "quota_duration_required",
                message: format!("quotas[{i}] is missing a required duration"),
                path: format!("quotas[{i}].duration"),
            });
        }
        if quota.limit.is_none() {
            report.errors.push(Finding {
                code: "quota_limit_required",
                message: format!("quotas[{i}] is missing a required limit"),
                path: format!("quotas[{i}].limit"),
            });
        }
        if quota.behavior.is_none() {
            report.errors.push(Finding {
                code: "quota_behavior_required",
                message: format!("quotas[{i}] is missing a required behavior"),
                path: format!("quotas[{i}].behavior"),
            });
        }
    }

    // `Check.java`'s `@NotNull @NotEmpty String when` -- unlike `message`
    // (genuinely optional, `@NotEmpty` only, no `@NotNull` stacked on it;
    // `models/flow.rs`'s own comment on `Check::when` vs `Check::message`
    // draws this exact distinction), `when` must be BOTH present and a
    // non-empty rendered source, or a check with nothing to evaluate would
    // validate clean.
    for (i, check) in flow.checks.iter().enumerate() {
        let blank = check.when.as_ref().is_none_or(|w| w.source().is_empty());
        if blank {
            report.errors.push(Finding {
                code: "check_when_required",
                message: format!("checks[{i}] is missing a required, non-empty `when`"),
                path: format!("checks[{i}].when"),
            });
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
    // kestra: `ArrayInputValidator` and `MultiselectInputValidator` --
    // character for character the same four-way test in both files, on the
    // two input types that have an `itemType` at all. An `ARRAY of ARRAY` and
    // an `ARRAY of MULTISELECT`/`SELECT` have no flat wire form for the
    // execute form to collect, and an `ARRAY of SECRET` would put a secret
    // through the same path a plain value takes.
    //
    // The gate is the input's OWN type. In Java the annotation is on
    // `ArrayInput`/`MultiselectInput`, so no other input type can reach the
    // validator; here `Input` is one flat struct, so the gate has to be
    // written. Without it a `STRING` input that happens to carry a stray
    // `itemType:` would be refused --- an over-refusal invented by this
    // crate's own flattening rather than by Kestra.
    for input in inputs {
        if !matches!(input.r#type, InputType::Array | InputType::Multiselect) {
            continue;
        }
        let Some(item) = input.item_type else {
            continue;
        };
        if matches!(
            item,
            InputType::Array | InputType::Secret | InputType::Multiselect | InputType::Select
        ) {
            report.errors.push(Finding {
                code: "input_item_type_invalid",
                message: format!(
                    "input '{}': `itemType` cannot be {}",
                    input.id,
                    serde_json::to_value(item)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_else(|| format!("{item:?}")),
                ),
                path: format!("inputs.{}.itemType", input.id),
            });
        }
    }

    // W9a Task 7 fix round 1 (M5): `FileInput.allowedFileExtensions`, whose
    // own doc requires "Each extension must start with a dot". An entry
    // without one can never match (the file's extension is taken WITH its
    // dot, `storage::inputs::extension_allowed`), so it would refuse every
    // upload; refused here instead, naming the input. Gated on the input's
    // own type for `itemType`'s reason above.
    for input in inputs {
        if input.r#type != InputType::File {
            continue;
        }
        let bad: Vec<String> = input
            .allowed_file_extensions
            .iter()
            .flatten()
            .filter(|e| !e.starts_with('.'))
            .map(|e| format!("{e:?}"))
            .collect();
        if !bad.is_empty() {
            report.errors.push(Finding {
                code: "input_file_extension_invalid",
                message: format!(
                    "input '{}': each `allowedFileExtensions` entry must start with a dot \
                     (\".csv\"), and {} does not",
                    input.id,
                    bad.join(", ")
                ),
                path: format!("inputs.{}.allowedFileExtensions", input.id),
            });
        }
    }

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

/// A `Schedule` trigger must be able to supply every input that has no
/// `defaults:` -- otherwise the flow can never start on its own schedule.
///
/// kestra: `FlowValidator.findMissingInputsForScheduleTriggers`
/// (`FlowValidator.java:284-312`), reproduced field for field:
///
/// - it runs only when the flow has **both** triggers and inputs;
/// - "required" is not consulted at all. The filter is `getDefaults() == null`,
///   so an `required: false` input with no default is still missing. That is
///   Kestra's rule as written, and this ports the rule rather than the name;
/// - a `Schedule` with no `inputs:` map is an empty map, not a skip;
/// - the missing ids are joined with `" | "`, which is Kestra's own separator
///   and is kept so the two messages read alike side by side;
/// - only `Schedule` triggers. A `Webhook` or a `Flow` trigger carries its own
///   inputs at fire time and is not checked, which is `filter(Schedule.class::isInstance)`.
///
/// **One narrowing, named.** Kestra reads `resolvableInputs()`, which expands
/// a `FORM` input into its dotted-path leaves (`FlowInterface.java:71-82`);
/// this reads `flow.inputs` flat, because W1's `Input` models no nested
/// `inputs:` and therefore has no leaves to expand. For a flow with no `FORM`
/// input the two are the same list -- and no corpus flow declares one. A flow
/// that does would be judged on the `FORM` node's own id here rather than on
/// its leaves; that is an under-refusal, and it is W1's model to close.
fn validate_schedule_inputs(flow: &Flow, report: &mut ValidationReport) {
    if flow.triggers.is_empty() || flow.inputs.is_empty() {
        return;
    }
    let without_defaults: Vec<&str> = flow
        .inputs
        .iter()
        .filter(|i| i.defaults.is_none())
        .map(|i| i.id.as_str())
        .collect();
    if without_defaults.is_empty() {
        return;
    }
    for trigger in &flow.triggers {
        let TriggerKind::Schedule(schedule) = &trigger.kind else {
            continue;
        };
        let supplied = schedule.inputs.as_ref();
        let missing: Vec<&str> = without_defaults
            .iter()
            .copied()
            .filter(|id| !supplied.is_some_and(|m| m.contains_key(*id)))
            .collect();
        if !missing.is_empty() {
            report.errors.push(Finding {
                code: "schedule_missing_inputs",
                message: format!(
                    "schedule trigger '{}' does not supply input(s) that have no defaults: {}",
                    trigger.id,
                    missing.join(" | ")
                ),
                path: format!("triggers.{}", trigger.id),
            });
        }
    }
}

/// Pebble reads `-` as subtraction, so `{{ inputs.server-fqn }}` is
/// `inputs.server` minus `fqn` and never resolves. Kestra refuses the flow
/// rather than letting it fail at render.
///
/// kestra: `FlowValidator.java:121-169`, and the two patterns are its own,
/// **including the one that is arguably a defect**: the `inputs` pattern is
/// built as `"\\{\\{\\s*inputs." + id + "\\s*\\}\\}"` with the `.` NOT escaped,
/// so it also matches `inputs_server-fqn`. Copying it is the Global
/// Constraint ("upstream defects are copied"); inventing the escaped version
/// would refuse a flow this Kestra accepts, which is the direction that must
/// not ship. The `outputs` pattern DOES escape its dots and requires a
/// trailing `.<something>`, so `{{ outputs.produce-output }}` alone does not
/// match -- also copied.
///
/// **What is scanned differs from Kestra's, in the safe direction.** Kestra
/// reflects over `object.getClass().getDeclaredFields()` -- the concrete
/// task class's OWN fields, so an inherited one (`description`, `runIf`) is
/// never scanned. This scans the task's whole serialised form, which is a
/// superset. A superset can only refuse MORE, so it is checked against the
/// corpus rather than argued: over the 294 golden entries these two rules
/// fire on exactly the two flows Kestra refuses for them and on nothing else
/// (`egret_refuses_every_flow_kestra_refuses`, plus the ratchet's regression
/// arm over all 266 `runs` entries).
fn validate_dashed_references(flow: &Flow, report: &mut ValidationReport) {
    let all_tasks: Vec<&Task> = flow
        .tasks
        .iter()
        .chain(&flow.errors)
        .chain(&flow.finally_)
        .chain(&flow.after_execution)
        .flat_map(descendants)
        .collect();

    let input_patterns: Vec<Regex> = flow
        .inputs
        .iter()
        .filter(|i| i.id.contains('-'))
        .filter_map(|i| Regex::new(&format!(r"\{{\{{\s*inputs.{}\s*\}}\}}", i.id)).ok())
        .collect();
    let output_patterns: Vec<Regex> = all_tasks
        .iter()
        .filter(|t| t.id.contains('-'))
        .filter_map(|t| {
            Regex::new(&format!(
                r"\{{\{{\s*outputs\.{}\.[^}}]+\s*\}}\}}",
                regex::escape(&t.id)
            ))
            .ok()
        })
        .collect();
    if input_patterns.is_empty() && output_patterns.is_empty() {
        return;
    }

    let offending = |patterns: &[Regex], rendered: &[(String, String)]| -> Vec<String> {
        rendered
            .iter()
            .filter(|(_, text)| patterns.iter().any(|p| p.is_match(text)))
            .map(|(id, _)| id.clone())
            .collect()
    };

    let task_text: Vec<(String, String)> = all_tasks
        .iter()
        .map(|t| (t.id.clone(), serialised(t)))
        .collect();
    let output_text: Vec<(String, String)> = flow
        .outputs
        .iter()
        .map(|o| (o.id.clone(), serialised(o)))
        .collect();

    for (code, what, patterns, population, label) in [
        (
            "dashed_input_reference",
            "input",
            &input_patterns,
            &task_text,
            "tasks",
        ),
        (
            "dashed_output_reference",
            "output",
            &output_patterns,
            &task_text,
            "tasks",
        ),
        (
            "dashed_output_reference",
            "output",
            &output_patterns,
            &output_text,
            "outputs",
        ),
    ] {
        let ids = offending(patterns, population);
        if !ids.is_empty() {
            report.errors.push(Finding {
                code,
                message: format!(
                    "use {what}s[key-name] instead of {what}s.key-name -- a key with a dash \
                     needs bracket notation, because Pebble reads the dash as subtraction; \
                     offending {label}: [{}]",
                    ids.join(", ")
                ),
                path: label.to_string(),
            });
        }
    }
}

/// A task and every task under it, in Kestra's `allTasksWithChilds()` sense.
///
/// Iterative, not recursive: the frontier admits each node once, so it
/// terminates after at most one pop per task for any input, which is a bound
/// by construction rather than a constant to justify.
fn descendants(root: &Task) -> Vec<&Task> {
    let mut out = Vec::new();
    let mut frontier = vec![root];
    while let Some(task) = frontier.pop() {
        out.push(task);
        frontier.extend(task.children());
    }
    out
}

/// A value's serialised form, for a textual scan. An unserialisable value
/// contributes the empty string rather than panicking -- this is a validator,
/// and a validator that panics on a flow is worse than one that misses it.
fn serialised<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn sla_common(sla: &Sla) -> &SlaCommon {
    match sla {
        Sla::MaxDuration { common, .. } => common,
        Sla::ExecutionAssertion { common, .. } => common,
    }
}

/// Walk one task node and its subtree, recording:
/// - its id, for the structural duplicate-id check run after the whole walk;
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
    report: &mut ValidationReport,
) {
    seen_ids
        .entry(task.id.clone())
        .or_default()
        .push(path.clone());

    // `timeout:` and `retry:` used to be recorded here for the
    // not-yet-enforced layer. Both are enforced now — see `NOT_YET_ENFORCED` —
    // so the walk no longer collects them. `check_retry` below is a different
    // thing and stays: it validates the SHAPE of a `retry:` block, which is
    // required whether or not anything acts on it.
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
        // is separately `@NotEmpty` in Java -- found in the same sweep, NOT
        // one of the 11 fields that round scoped, and left unfixed at the
        // time (see the task report). Closed here: whole-branch review,
        // minor finding 6 -- the last member of that same 11-field set of
        // `@NotEmpty`/`@NotNull` control-flow checks this file otherwise
        // finished together (`Sequential`/`Parallel`/`Dag`/`Loop`/
        // `LoopUntil`/`AllowFailure`/`WorkingDirectory`'s `tasks`,
        // `DagNode::task`, `Subflow::namespace`/`flow_id`), left out is
        // exactly how finding 5's `Quota`/`Check` gap happened too -- a
        // completed sweep with one member missing reads, from the outside,
        // identical to a finished one.
        TaskKind::If(i) => {
            if i.condition.is_none() {
                report.errors.push(Finding {
                    code: "if_condition_required",
                    message: format!("task '{}': condition is required (explicit null)", task.id),
                    path: format!("{path}.condition"),
                });
            }
            if i.then.is_empty() {
                report.errors.push(Finding {
                    code: "if_then_required",
                    message: format!(
                        "task '{}': if must declare at least one task in `then`",
                        task.id
                    ),
                    path: format!("{path}.then"),
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
        //
        // Task 6 fix round 2, Finding 1 (Important): presence alone was not
        // enough. `publish::subflow_ref_name` encodes this pair into one
        // string as `"{namespace}/{flowId}"`, and a `namespace` (or
        // `flowId`) containing its OWN `/` decodes back into a DIFFERENT
        // pair than the author wrote -- `namespace: "a/b", flowId: "c"`
        // encoded to `"a/b/c"` and decoded, via `split_once('/')`, as
        // `("a", "b/c")`. The character checks below close that: they
        // reject anything that could never have been created as a real
        // flow's `namespace`/`id` in the first place, `/` included, so the
        // encoding's separator can never collide with a value it carries.
        TaskKind::Subflow(s) => {
            match &s.namespace {
                None => report.errors.push(Finding {
                    code: "subflow_namespace_required",
                    message: format!("task '{}': namespace is required (explicit null)", task.id),
                    path: format!("{path}.namespace"),
                }),
                // Same grammar `Flow::namespace` itself is checked against --
                // `validate_namespace`, above -- via the shared
                // `is_valid_namespace`, so a subflow can never name a
                // namespace no real flow could ever have.
                Some(ns) if !is_valid_namespace(ns) => report.errors.push(Finding {
                    code: "subflow_namespace_invalid",
                    message: format!(
                        "task '{}': subflow namespace '{ns}' must match ^[a-z0-9][a-z0-9._-]* \
                         and be 1-150 characters long -- the same grammar `namespace:` itself \
                         uses (see `validate_namespace`), so a subflow can never encode a `/` \
                         into `publish::subflow_ref_name`'s namespace half",
                        task.id
                    ),
                    path: format!("{path}.namespace"),
                }),
                Some(_) => {}
            }
            match &s.flow_id {
                None => report.errors.push(Finding {
                    code: "subflow_flow_id_required",
                    message: format!("task '{}': flowId is required (explicit null)", task.id),
                    path: format!("{path}.flowId"),
                }),
                // Kestra's own flow `id` pattern -- see `is_valid_flow_id`'s
                // doc comment for the exact `AbstractFlow.java` citation.
                Some(fid) if !is_valid_flow_id(fid) => report.errors.push(Finding {
                    code: "subflow_flow_id_invalid",
                    message: format!(
                        "task '{}': subflow flowId '{fid}' must match \
                         ^[a-zA-Z0-9][a-zA-Z0-9._-]* and be 1-100 characters long -- Kestra's \
                         own flow id pattern (`AbstractFlow.java`'s `id` field), so a subflow \
                         can never encode a `/` into `publish::subflow_ref_name`'s flowId half",
                        task.id
                    ),
                    path: format!("{path}.flowId"),
                }),
                Some(_) => {}
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
        // W8 Task 3. `tasks:` is NOT required -- `valids/pause-delay` writes
        // none and the artifact published it with a 200 -- so the only check
        // here is the honesty one: a `Pause` whose `behavior:`, `onPause:`,
        // `tasks:` or `errors:` this executor does not reproduce must SAY so,
        // which is the same thing the blanket row in `KESTRA_FLOW_CONTROL` used
        // to say about every `Pause`.
        TaskKind::Pause(p) => {
            if let Some(property) = crate::executor::pause_gap(p) {
                report.warnings.push(Finding {
                    code: "not_yet_enforced",
                    message: format!(
                        "task '{}' is a pause with '{property}', which egret parses but does \
                         not run: the pause itself waits, and '{property}' is ignored. \
                         Enforcement lands in W8",
                        task.id
                    ),
                    path: format!("{path}.{property}"),
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
            // kestra: `WorkingDirectoryTaskValidator.isValid` --
            // `value.getTasks().stream().anyMatch(task -> !(task instanceof
            // RunnableTask<?>))` -> "Only runnable tasks are allowed as
            // children of a WorkingDirectory task". A `WorkingDirectory`
            // hands its whole subtree to ONE worker in one working
            // directory; a flowable is driven by the executor, which is not
            // in that worker, so there is no way for it to run there at all.
            //
            // **`is_runnable_child` is fail-OPEN, and the asymmetry is
            // deliberate.** `KESTRA_FLOW_CONTROL`'s other use --- the
            // `not_yet_enforced` warning below --- is fail-CLOSED, warning
            // for anything spelled like `io.kestra.plugin.core.flow.*`,
            // because a spurious warning costs a reader one line. This is a
            // REFUSAL, and the same fail-closed spelling test would refuse
            // `io.kestra.plugin.core.flow.Sleep` and
            // `io.kestra.plugin.core.flow.PurgeFlows` --- the two classes in
            // that Java package that ARE `RunnableTask`s (measured at the
            // checkout: they are the only two classes in that package
            // that implement it) --- and both are legitimate children of a
            // `WorkingDirectory`. Over-refusal is the mirror of the class
            // this rule exists to close, so the residual is stated instead:
            // a control-flow class Kestra adds tomorrow, inside a
            // `WorkingDirectory`, is accepted here until it gains a row.
            for child in &w.tasks {
                if !is_runnable_child(child) {
                    report.errors.push(Finding {
                        code: "working_directory_child_not_runnable",
                        message: format!(
                            "task '{}': only runnable tasks are allowed as children of a \
                             workingDirectory, and '{}' has type '{}', which is a \
                             control-flow task",
                            task.id, child.id, child.r#type
                        ),
                        path: format!("{path}.tasks.{}", child.id),
                    });
                }
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
        // W5b: a `kestra.` type (in the catalog or not) and a bare catalog
        // FQCN are judged by `validate_plugin_tasks`, which refuses the
        // ones that cannot run with an ERROR naming them; neither gets the
        // generic warning below, which would say the opposite ("check for a
        // typo") of the error's hint.
        TaskKind::Leaf { .. }
            if registry::known_type(&task.r#type).is_none()
                && !task.r#type.starts_with(crate::plugins::PREFIX)
                && crate::plugins::bare_hint(&task.r#type).is_none() =>
        {
            if canonical_flow_type(&task.r#type).is_some() {
                // **No `KESTRA_FLOW_CONTROL` lookup, and the arm that did one
                // is GONE.** W8 Task 3 fix round 1. It named the package that
                // would port an `Unported` row, and since W8 Task 3 ported
                // `pause` — the last one — every row is ported, so that arm
                // could not be reached and the `Unported` variant could not be
                // constructed. Deleted rather than left behind an
                // `#[allow(dead_code)]`, which is what this crate did with the
                // two `isRetrying` guards W3a measured unreachable: a branch
                // nobody can drive is a branch nobody can check.
                //
                // The FAIL-CLOSED design it existed inside is untouched and is
                // the half that matters: anything spelled like Kestra
                // control flow that reaches `Leaf` and is not a registered
                // type warns, whether or not a table has a row for it. That
                // test is `canonical_flow_type(...).is_some()`, right here,
                // and it never read the table.
                report.warnings.push(Finding {
                    code: "not_yet_enforced",
                    message: format!(
                        "task '{}' has type '{}', which is spelled like a Kestra control-flow \
                         task (a `io.kestra.plugin.core.flow.*` FQCN or its `core.` alias) but \
                         egret does not implement it yet; it currently runs as an inert no-op \
                         instead of doing what it declares",
                        task.id, task.r#type
                    ),
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
        walk_task(flow, child, child_path, seen_ids, report);
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

/// Task 4's `@NotNull` debt on `Schedule::cron`, plus (whole-branch review
/// finding 2) the `Flow` trigger's `dependsOn` list -- each `Dependency`
/// names another flow by the SAME `(namespace, flowId)` pair `Subflow` does,
/// so it is checked against the SAME grammar `is_valid_namespace`/
/// `is_valid_flow_id`, for the identical reason: `publish::
/// resolve_trigger_dependencies` encodes the pair into one
/// `"{namespace}/{flowId}"` string via `publish::subflow_ref_name`, and a
/// `/` smuggled into either half would decode back into a different pair
/// than the author wrote. No `self_dependency` check to match `Subflow`'s
/// `self_subflow`, deliberately: a flow trigger depending on its OWN past
/// executions is a legitimate Kestra pattern (recursive/self-triggering),
/// not a synchronous call that would deadlock the way a subflow calling
/// itself would.
///
/// W13b Task 3 fix round 1: the `dependsOn` checks themselves now live in
/// `trigger::depends_on::depends_on_refusals` (same codes, messages and
/// paths), so the run-time gate a draft's listener passes through applies
/// exactly them.
fn validate_trigger(trigger: &Trigger, report: &mut ValidationReport) {
    match &trigger.kind {
        TriggerKind::Schedule(s) => {
            if s.cron.is_none() {
                report.errors.push(Finding {
                    code: "schedule_cron_required",
                    message: format!("trigger '{}': cron is required (explicit null)", trigger.id),
                    path: format!("triggers.{}.cron", trigger.id),
                });
            }
        }
        TriggerKind::Flow(ft) => {
            // Each dependency's `namespace`/`flowId`, `minSatisfied`, `window`
            // and duplicate conditions: ONE list, shared with the run-time
            // gate so a draft (which saves with these errors and still gets a
            // listener row) is refused there too. See `trigger::depends_on`,
            // *One refusal list*.
            let base = format!("triggers.{}", trigger.id);
            for r in depends_on_refusals(ft) {
                let path = match r.at {
                    RefusalAt::Window("") => format!("{base}.window"),
                    RefusalAt::Window(field) => format!("{base}.window.{field}"),
                    RefusalAt::MinSatisfied => format!("{base}.minSatisfied"),
                    RefusalAt::Dependency(i) => format!("{base}.dependsOn[{i}]"),
                    RefusalAt::DependencyField(i, field) => {
                        format!("{base}.dependsOn[{i}].{field}")
                    }
                };
                report.errors.push(Finding {
                    code: r.code,
                    message: format!("trigger '{}': {}", trigger.id, r.message),
                    path,
                });
            }
        }
        _ => {}
    }
}

fn emit_not_yet_enforced(features: &FeaturePresence, report: &mut ValidationReport) {
    let present: [(&str, bool); NOT_YET_ENFORCED.len()] = [
        ("concurrency", features.concurrency),
        ("sla", features.sla),
        ("checks", features.checks),
        ("quotas", features.quotas),
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
    use super::NOT_YET_ENFORCED;
    use crate::spec::{parse, validate};

    // ---------- W3a Task 8: refusing what Kestra refuses ----------
    //
    // Each of the four rules below closes an over-acceptance the W3a execution
    // oracle measured against `kestra/kestra:v2.0.0-rc13`. Every one gets both
    // arms: the flow Kestra refuses, AND the nearest flow Kestra accepts --
    // because a rule that fires on both is an over-refusal, which is the mirror
    // of the class it was written to close.

    /// kestra: `WorkingDirectoryTaskValidator` -- "Only runnable tasks are
    /// allowed as children of a WorkingDirectory task", the refusal
    /// `invalids/workingdirectory-invalid` earns.
    #[test]
    fn a_flowable_inside_a_working_directory_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: wd\n    type: core.workingdirectory\n    \
             tasks:\n      - id: log\n        type: io.kestra.plugin.core.log.Log\n        \
             message: hi\n      - id: par\n        type: core.parallel\n        tasks:\n          \
             - id: inner\n            type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        let f0 = r
            .errors
            .iter()
            .find(|e| e.code == "working_directory_child_not_runnable")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(f0.message.contains("'par'"), "{}", f0.message);
    }

    /// The arm `invalids/workingdirectory-invalid` actually needs: its
    /// offending child is a `Pause`, which this crate has NOT ported, so it
    /// reaches `TaskKind::Leaf` and is otherwise indistinguishable from an
    /// ordinary task. A rule written only over the typed variants misses it.
    #[test]
    fn an_unported_kestra_flowable_inside_a_working_directory_is_an_error_too() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: wd\n    type: core.workingdirectory\n    \
             tasks:\n      - id: pause\n        type: io.kestra.plugin.core.flow.Pause\n        \
             pauseDuration: PT5S\n",
        )
        .unwrap();
        assert!(
            validate(&f)
                .errors
                .iter()
                .any(|e| e.code == "working_directory_child_not_runnable"),
            "a `Pause` is a Kestra flowable even though it reaches `Leaf` here"
        );
    }

    /// The fail-open half, and it is the whole reason the rule keys on
    /// `KESTRA_FLOW_CONTROL` rather than on the `io.kestra.plugin.core.flow.*`
    /// spelling. `Sleep` and `PurgeFlows` are the two classes in that Java
    /// package that ARE `RunnableTask`s, so both are legitimate children -- and
    /// a spelling test would refuse a flow Kestra accepts.
    ///
    /// **The member this list did not contain was a `WaitFor`**, which this
    /// rule also allowed and should not have. It was named rather than added,
    /// because an assertion inside a test called `..._is_allowed` would defend
    /// the wrong answer; W8 Task 4 closed it and put the assertion in a test of
    /// its own,
    /// `a_waitfor_child_of_a_working_directory_is_refused_like_a_loopuntil_one`,
    /// below.
    #[test]
    fn a_runnable_that_merely_lives_in_kestras_flow_package_is_allowed() {
        for ty in [
            "io.kestra.plugin.core.flow.Sleep",
            "io.kestra.plugin.core.flow.PurgeFlows",
            "io.kestra.plugin.core.log.Log",
            "egret.sql",
        ] {
            let f = parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: wd\n    type: core.workingdirectory\n    \
                 tasks:\n      - id: child\n        type: {ty}\n"
            ))
            .unwrap();
            assert!(
                !validate(&f)
                    .errors
                    .iter()
                    .any(|e| e.code == "working_directory_child_not_runnable"),
                "{ty} is a runnable task and must be allowed as a child"
            );
        }
    }

    /// **The over-acceptance W8 Task 4 Step 0 closed, driven from BOTH
    /// spellings.**
    ///
    /// `io.kestra.plugin.core.flow.WaitFor` and
    /// `io.kestra.plugin.core.flow.LoopUntil` are one Kestra class -- the first
    /// is an `@Plugin(aliases = ...)` on the second -- so a
    /// `WorkingDirectory` must refuse both or accept both. Before Task 4 it
    /// accepted the alias and refused the class name, because `kind_for` had a
    /// row for one and not the other and `is_runnable_child` is
    /// `matches!(kind, Leaf)`.
    ///
    /// **Both spellings are asserted, and that is the point of the test rather
    /// than thoroughness**: a fix applied to one table, or to one spelling,
    /// leaves this red. The `core.` alias forms are driven too, because
    /// `canonical_flow_type` accepts both prefixes and a row that worked only
    /// for the FQCN would be a second half-fix.
    #[test]
    fn a_waitfor_child_of_a_working_directory_is_refused_like_a_loopuntil_one() {
        for ty in [
            "io.kestra.plugin.core.flow.WaitFor",
            "io.kestra.plugin.core.flow.LoopUntil",
            "core.waitfor",
            "core.loopuntil",
        ] {
            let f = parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: wd\n    type: core.workingdirectory\n    \
                 tasks:\n      - id: child\n        type: {ty}\n        \
                 condition: \"{{{{ true }}}}\"\n        tasks:\n          - id: inner\n            \
                 type: io.kestra.plugin.core.log.Log\n            message: hi\n"
            ))
            .unwrap_or_else(|e| panic!("{ty}: {e:?}"));
            let r = validate(&f);
            assert!(
                r.errors
                    .iter()
                    .any(|e| e.code == "working_directory_child_not_runnable"),
                "{ty} is a Kestra flowable and must be refused as a \
                 `WorkingDirectory` child: {r:?}"
            );
        }
    }

    /// The subtree a `Leaf`-typed `WaitFor` used to hide.
    ///
    /// `walk_task` recurses through `Task::children()`, and a `Leaf` has none
    /// -- so while `WaitFor` reached `Leaf` its nested tasks were validated by
    /// nothing at all. The instrument is a nested task that is INVALID on its
    /// own (a `Sequential` with no `tasks:`), asserted to be reported under
    /// both spellings: the finding can only appear if the walk descended.
    #[test]
    fn a_waitfor_subtree_is_validated_because_walk_task_descends_through_children() {
        for ty in [
            "io.kestra.plugin.core.flow.WaitFor",
            "io.kestra.plugin.core.flow.LoopUntil",
        ] {
            let f = parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: outer\n    type: {ty}\n    \
                 condition: \"{{{{ true }}}}\"\n    tasks:\n      - id: empty\n        \
                 type: core.sequential\n"
            ))
            .unwrap_or_else(|e| panic!("{ty}: {e:?}"));
            let r = validate(&f);
            assert!(
                r.errors
                    .iter()
                    .any(|e| e.code == "sequential_tasks_required"),
                "{ty}: the nested `Sequential` must be reached by `walk_task`: {r:?}"
            );
        }
    }

    /// kestra: `FlowValidator.findMissingInputsForScheduleTriggers` --
    /// `invalids/scheduled-missing-inputs-defaults{,-2}`.
    #[test]
    fn a_schedule_that_cannot_supply_an_input_without_a_default_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user\n    type: STRING\n    defaults: John\n  \
             - id: user2\n    type: STRING\ntasks:\n  - id: t\n    type: x\ntriggers:\n  \
             - id: every_minute\n    type: io.kestra.plugin.core.trigger.Schedule\n    \
             cron: \"*/1 * * * *\"\n",
        )
        .unwrap();
        let r = validate(&f);
        let f0 = r
            .errors
            .iter()
            .find(|e| e.code == "schedule_missing_inputs")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(
            f0.message.contains("user2") && !f0.message.contains("'user'"),
            "only the input with no default is missing: {}",
            f0.message
        );
    }

    /// Three ways a scheduled flow is fine, each of which a rule written one
    /// step too wide would refuse.
    #[test]
    fn a_schedule_is_silent_when_every_input_can_be_supplied() {
        // 1. the schedule supplies it. This is the row that needed
        //    `Schedule::inputs` restored to the model -- W1 had dropped the
        //    field as "deprecated", which the checkout does not say.
        let supplied = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user2\n    type: STRING\ntasks:\n  \
             - id: t\n    type: x\ntriggers:\n  - id: s\n    \
             type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    \
             inputs:\n      user2: bob\n",
        )
        .unwrap();
        // 2. every input has a default.
        let defaulted = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user2\n    type: STRING\n    \
             defaults: bob\ntasks:\n  - id: t\n    type: x\ntriggers:\n  - id: s\n    \
             type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n",
        )
        .unwrap();
        // 3. the trigger is not a Schedule. Kestra's filter is
        //    `Schedule.class::isInstance`; a webhook carries its inputs at
        //    fire time.
        let webhook = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user2\n    type: STRING\ntasks:\n  \
             - id: t\n    type: x\ntriggers:\n  - id: w\n    \
             type: io.kestra.plugin.core.trigger.Webhook\n    key: k\n",
        )
        .unwrap();
        for (what, flow) in [
            ("supplied by the schedule", supplied),
            ("has a default", defaulted),
            ("not a schedule", webhook),
        ] {
            let r = validate(&flow);
            assert!(
                !r.errors.iter().any(|e| e.code == "schedule_missing_inputs"),
                "{what}: {r:?}"
            );
        }
    }

    /// kestra: `ArrayInputValidator` / `MultiselectInputValidator` --
    /// `invalids/inputs-validation`.
    #[test]
    fn an_item_type_an_array_cannot_hold_is_an_error() {
        for item in ["ARRAY", "SECRET", "MULTISELECT", "SELECT"] {
            for outer in ["ARRAY", "MULTISELECT"] {
                let f = parse(&format!(
                    "id: f\nnamespace: a.b\ninputs:\n  - id: i\n    type: {outer}\n    \
                     itemType: {item}\ntasks:\n  - id: t\n    type: x\n"
                ))
                .unwrap();
                let r = validate(&f);
                let f0 = r
                    .errors
                    .iter()
                    .find(|e| e.code == "input_item_type_invalid")
                    .unwrap_or_else(|| panic!("{outer} of {item}: {r:?}"));
                assert!(
                    f0.message
                        .ends_with(&format!("`itemType` cannot be {item}")),
                    "{}",
                    f0.message
                );
            }
        }
    }

    /// W9a Task 7 fix round 1 (M5): each `allowedFileExtensions` entry of a
    /// FILE input must start with `.` (`FileInput.java`'s own doc: "Each
    /// extension must start with a dot"), refused naming the input; an entry
    /// with its dot, and the property on no FILE input, are silent.
    #[test]
    fn an_allowed_file_extension_without_its_dot_is_an_error_naming_the_input() {
        let src = |ty: &str, list: &str| {
            parse(&format!(
                "id: f\nnamespace: a.b\ninputs:\n  - id: up\n    type: {ty}\n    \
                 allowedFileExtensions: {list}\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap()
        };
        let r = validate(&src("FILE", "[\".csv\", \"txt\"]"));
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "input_file_extension_invalid")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(
            e.message.contains("input 'up'") && e.message.contains("\"txt\""),
            "{}",
            e.message
        );
        assert_eq!(e.path, "inputs.up.allowedFileExtensions");
        for (ty, list) in [("FILE", "[\".csv\", \".TXT\"]"), ("FILE", "[]")] {
            let r = validate(&src(ty, list));
            assert!(
                !r.errors
                    .iter()
                    .any(|e| e.code == "input_file_extension_invalid"),
                "{ty} {list}: {r:?}"
            );
        }
    }

    /// Both silent arms. The second is the one this crate's flattening makes
    /// necessary: in Java the validator is attached to `ArrayInput` and
    /// `MultiselectInput` and can never see any other input type, so a gate on
    /// the input's own type is what stops the rule inventing a refusal.
    #[test]
    fn a_legal_item_type_and_an_item_type_on_another_input_type_are_both_silent() {
        for (outer, item) in [
            ("ARRAY", "STRING"),
            ("ARRAY", "INT"),
            ("MULTISELECT", "STRING"),
            ("STRING", "ARRAY"),
            ("SELECT", "SECRET"),
        ] {
            let f = parse(&format!(
                "id: f\nnamespace: a.b\ninputs:\n  - id: i\n    type: {outer}\n    \
                 itemType: {item}\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap();
            let r = validate(&f);
            assert!(
                !r.errors.iter().any(|e| e.code == "input_item_type_invalid"),
                "{outer} with itemType {item}: {r:?}"
            );
        }
    }

    /// kestra: `FlowValidator.java:121-137` --
    /// `invalids/inputs-key-with-subtraction-symbol-validation`.
    #[test]
    fn a_dotted_reference_to_a_dashed_input_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: server-fqn\n    type: STRING\n    \
             defaults: animal\ntasks:\n  - id: hello\n    \
             type: io.kestra.plugin.core.log.Log\n    message: \"{{inputs.server-fqn}}\"\n",
        )
        .unwrap();
        let r = validate(&f);
        let f0 = r
            .errors
            .iter()
            .find(|e| e.code == "dashed_input_reference")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(f0.message.contains("[hello]"), "{}", f0.message);
    }

    /// kestra: `FlowValidator.java:139-169` --
    /// `invalids/outputs-key-with-subtraction-symbol-validation`. Both
    /// populations Kestra scans are covered: the tasks and the flow's own
    /// `outputs:`.
    #[test]
    fn a_dotted_reference_to_a_dashed_task_output_is_an_error_in_both_populations() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: produce-output\n    \
             type: io.kestra.plugin.core.debug.Return\n    format: x\n  - id: use_output\n    \
             type: io.kestra.plugin.core.log.Log\n    \
             message: \"{{ outputs.produce-output.value }}\"\noutputs:\n  - id: final\n    \
             type: STRING\n    value: \"{{ outputs.produce-output.value }}\"\n",
        )
        .unwrap();
        let r = validate(&f);
        let found: Vec<&str> = r
            .errors
            .iter()
            .filter(|e| e.code == "dashed_output_reference")
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(
            found,
            vec!["tasks", "outputs"],
            "Kestra reports the offending TASKS and the offending OUTPUTS \
             separately, and so does this: {r:?}"
        );
    }

    /// The silent arms, and the second is Kestra's own defect being copied.
    #[test]
    fn bracket_notation_and_an_undashed_id_are_both_silent() {
        // 1. bracket notation -- the spelling the message tells the author to
        //    use. It must not be refused, or the error would be unfixable.
        let bracketed = "id: f\nnamespace: a.b\ninputs:\n  - id: server-fqn\n    \
                         type: STRING\n    defaults: a\ntasks:\n  - id: hello\n    \
                         type: io.kestra.plugin.core.log.Log\n    \
                         message: \"{{inputs['server-fqn']}}\"\n";
        // 2. no dash in the id at all: no pattern is built, so nothing is
        //    scanned.
        let undashed = "id: f\nnamespace: a.b\ninputs:\n  - id: serverfqn\n    \
                        type: STRING\n    defaults: a\ntasks:\n  - id: hello\n    \
                        type: io.kestra.plugin.core.log.Log\n    \
                        message: \"{{inputs.serverfqn}}\"\n";
        // 3. `{{ inputs.server-fqn | upper }}` does NOT match. Kestra's
        //    pattern requires the reference to be the WHOLE expression
        //    (`\{\{\s*inputs.<id>\s*\}\}`), so a filtered one slips through
        //    upstream too. Copied deliberately: inventing the wider rule would
        //    refuse a flow this Kestra accepts, which is the direction that
        //    must not ship.
        let filtered = "id: f\nnamespace: a.b\ninputs:\n  - id: server-fqn\n    \
                        type: STRING\n    defaults: a\ntasks:\n  - id: hello\n    \
                        type: io.kestra.plugin.core.log.Log\n    \
                        message: \"{{ inputs.server-fqn | upper }}\"\n";
        for (what, src) in [
            ("bracket notation", bracketed),
            ("no dash", undashed),
            (
                "Kestra's own pattern does not reach a filtered reference",
                filtered,
            ),
        ] {
            let f = parse(src).unwrap();
            let r = validate(&f);
            assert!(
                !r.errors.iter().any(|e| e.code.starts_with("dashed_")),
                "{what}: {r:?}"
            );
        }
    }

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

    /// W9b: the save-time refusal of a `_files` namespace segment; its
    /// run-time twin is `nsfiles::access`'s
    /// `a_files_segment_namespace_is_refused_for_every_access_and_actor`.
    #[test]
    fn publish_refuses_a_files_segment_namespace_by_name() {
        for ns in ["a._files", "a._files.b", "_x._files"] {
            let f = parse(&format!(
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap();
            let found: Vec<_> = validate(&f)
                .errors
                .into_iter()
                .filter(|e| e.path == "namespace")
                .collect();
            if ns.starts_with('_') {
                // Refused by the pattern; named once.
                assert_eq!(found.len(), 1, "{found:?}");
                assert_eq!(found[0].code, "invalid_namespace");
                continue;
            }
            assert_eq!(found.len(), 1, "{ns}: {found:?}");
            assert_eq!(found[0].code, "namespace_files_segment");
            assert!(
                found[0].message.contains(ns) && found[0].message.contains("'_files'"),
                "{found:?}"
            );
        }
        // The controls: `_files` inside a segment is not the segment.
        for ns in ["a.files", "a.x_files", "a._filesx"] {
            let f = parse(&format!(
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap();
            assert!(
                !validate(&f).errors.iter().any(|e| e.path == "namespace"),
                "{ns}"
            );
        }
    }

    /// W9b fix round 1 M3: the global namespace rule is unchanged -- an
    /// empty segment publishes; only namespace files refuse it
    /// (`nsfiles::access`'s `an_empty_namespace_segment_is_refused_by_namespace_files_only`).
    #[test]
    fn an_empty_namespace_segment_still_publishes() {
        for ns in ["a..b", "a.", "a.b..c"] {
            let f = parse(&format!(
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap();
            assert!(
                !validate(&f).errors.iter().any(|e| e.path == "namespace"),
                "{ns}"
            );
        }
    }

    /// W9b fix round 1 I1: a flow id `_files` is refused at publish (by
    /// Kestra's own id pattern: no leading `_`), so `{ns}/_files/executions/…`
    /// is never a flow's execution URI. Its slug could not be `_files` either
    /// (`storage::uri`'s `no_flow_slug_is_the_files_marker`).
    #[test]
    fn a_flow_id_of_files_is_refused() {
        for id in ["_files", "_Files", "_files.x"] {
            let f = parse(&format!(
                "id: {id}\nnamespace: a\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap();
            let found: Vec<_> = validate(&f)
                .errors
                .into_iter()
                .filter(|e| e.path == "id")
                .collect();
            assert_eq!(found.len(), 1, "{id}: {found:?}");
            assert_eq!(found[0].code, "invalid_flow_id");
            assert!(found[0].message.contains(&format!("'{id}'")), "{found:?}");
        }
        // The control: `files` and `x_files` are legal ids.
        for id in ["files", "x_files"] {
            let f = parse(&format!(
                "id: {id}\nnamespace: a\ntasks:\n  - id: t\n    type: x\n"
            ))
            .unwrap();
            assert!(!validate(&f).errors.iter().any(|e| e.path == "id"), "{id}");
        }
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

    /// **The list is a claim about this codebase, and this is what makes a
    /// wrong row fail.** The reported finding was not only that
    /// `("timeout", "W3")` had gone stale — it was that **no test pinned any
    /// row**, so a false claim could sit in a user-visible warning
    /// indefinitely. Reading the other six then found `retry` stale as well.
    ///
    /// Both directions, because either half alone is satisfiable by a mistake:
    /// a validator that never warns passes the negative half, and one that
    /// warns about everything passes the positive half.
    #[test]
    fn the_not_yet_enforced_list_is_pinned_in_both_directions() {
        // The list itself, by name and by package.
        assert_eq!(
            NOT_YET_ENFORCED,
            &[
                ("concurrency", "W8"),
                ("sla", "W8"),
                ("checks", "W8"),
                ("quotas", "W8"),
                ("triggers", "W6"),
            ],
            "a row added here is a claim that egret does NOT enforce that \
             feature; a row removed is a claim that it now does. Both need the \
             evidence this constant's doc carries. **A row can also NARROW** -- \
             `sla` did at W8 Task 6 -- and that moves `FeaturePresence`, not \
             this list, which is why \
             `an_execution_assertion_only_sla_does_not_warn_and_a_max_duration_does` \
             exists beside this test rather than inside it"
        );

        // NEGATIVE: the two features that ARE enforced must not warn. A flow
        // declaring both, at flow level and at task level, so neither of
        // `validate`'s two collection points can reintroduce one alone.
        let enforced = parse(
            "id: f\nnamespace: a.b\nretry:\n  type: constant\n  interval: PT1S\n\
             tasks:\n  - id: t\n    type: x\n    timeout: PT1S\n    retry:\n\
             \x20     type: constant\n      interval: PT1S\n",
        )
        .unwrap();
        let r = validate(&enforced);
        assert!(
            !r.warnings.iter().any(|w| w.code == "not_yet_enforced"),
            "`retry:` is enforced by `executor::retry` (W3a Task 7, plus W4 \
             Task 1's waker) and `timeout:` by `worker::timeout` (W4 Task 2), \
             so neither may be reported as not yet enforced: {:?}",
            r.warnings
        );
        assert!(r.errors.is_empty(), "{r:?}");

        // POSITIVE CONTROL: a feature that really is unenforced still warns,
        // through the same code path, on a flow of the same shape.
        let unenforced = parse(
            "id: f\nnamespace: a.b\nconcurrency:\n  limit: 1\ntasks:\n\
             \x20 - id: t\n    type: x\n    timeout: PT1S\n",
        )
        .unwrap();
        let r = validate(&unenforced);
        let names: Vec<&str> = r
            .warnings
            .iter()
            .filter(|w| w.code == "not_yet_enforced")
            .map(|w| w.path.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["concurrency"],
            "exactly the unenforced feature warns, and the enforced `timeout:` \
             beside it does not: {:?}",
            r.warnings
        );
    }

    /// **The `sla` row narrowed rather than went** — W8 Task 6, and both halves
    /// are asserted because either alone is satisfiable by a mistake.
    ///
    /// `SLA.Type`'s two subtypes are two different mechanisms in the artifact
    /// and only one of them is enforced here, so *"`sla` is declared and not
    /// enforced"* stopped being a question about the KEY and became a question
    /// about the VARIANT. A validator that kept warning on every `sla:` would
    /// tell the author of an `EXECUTION_ASSERTION` flow to work around a rule
    /// that already works; one that stopped warning altogether would hide the
    /// `MAX_DURATION` gap. The pair below is what separates them.
    #[test]
    fn an_execution_assertion_only_sla_does_not_warn_and_a_max_duration_does() {
        let assertion_only = parse(
            "id: f\nnamespace: a.b\nsla:\n  - id: a\n    type: EXECUTION_ASSERTION\n    \
             behavior: CANCEL\n    assert: \"{{ false }}\"\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        let r = validate(&assertion_only);
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.code == "not_yet_enforced" && w.path == "sla"),
            "an `EXECUTION_ASSERTION` IS enforced -- `crate::sla` evaluates it \
             and `worker::claim::settle` acts on it -- so warning here would be \
             a false sentence in a user-visible message: {:?}",
            r.warnings
        );
        assert!(r.errors.is_empty(), "{r:?}");

        let has_max_duration = parse(
            "id: f\nnamespace: a.b\nsla:\n  - id: a\n    type: EXECUTION_ASSERTION\n    \
             behavior: CANCEL\n    assert: \"{{ false }}\"\n  - id: d\n    \
             type: MAX_DURATION\n    behavior: FAIL\n    duration: PT1S\ntasks:\n  \
             - id: t\n    type: x\n",
        )
        .unwrap();
        let r = validate(&has_max_duration);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "not_yet_enforced" && w.path == "sla"),
            "a `MAX_DURATION` beside an enforced assertion must still warn -- \
             nothing in this crate evaluates one, and the flow above is the \
             MIXED case, which is the one a per-key predicate gets wrong: {:?}",
            r.warnings
        );
        assert!(r.errors.is_empty(), "{r:?}");

        // And the degenerate mixed case in the other order, so the `any` above
        // is not passing on declaration order.
        let reversed = parse(
            "id: f\nnamespace: a.b\nsla:\n  - id: d\n    type: MAX_DURATION\n    \
             behavior: FAIL\n    duration: PT1S\n  - id: a\n    \
             type: EXECUTION_ASSERTION\n    behavior: CANCEL\n    assert: \"{{ false }}\"\n\
             tasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        assert!(validate(&reversed)
            .warnings
            .iter()
            .any(|w| w.code == "not_yet_enforced" && w.path == "sla"));
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

    /// Finding 1 (CRITICAL), **as W8 Task 3 left it.** The original test asserted
    /// that a `Pause` warns unconditionally, because it had no `TaskKind`
    /// variant and nothing ran it: a user pasting a Kestra flow whose `Pause` is
    /// a manual-approval gate had to be told it gates nothing.
    ///
    /// It gates now. `executor::build_plan` runs the wait — bounded by
    /// `pauseDuration:`/`timeout:`, unbounded otherwise, which is what the
    /// pinned artifact does with each — so a blanket warning would be the
    /// OPPOSITE lie, telling a user that a feature which works does not.
    ///
    /// What survives is the half that is still true, and the test now asserts
    /// both directions of it because only the pair can fail for the right
    /// reason: a `Pause` carrying one of the four properties `build_plan` does
    /// not reproduce still warns and still names W8, and a plain one does not
    /// warn at all. `executor::pause_gap` is the single predicate both this and
    /// `build_plan` ask, so a flow warned here is exactly a flow planned opaque
    /// there.
    #[test]
    fn a_pause_warns_only_for_the_properties_egret_does_not_run() {
        for spelling in ["io.kestra.plugin.core.flow.Pause", "core.pause"] {
            // Silent: both shapes this executor reproduces.
            for (label, props) in [("unbounded", ""), ("bounded", "    pauseDuration: PT1S\n")] {
                let yaml = format!(
                    "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: {spelling}\n{props}"
                );
                let r = validate(&parse(&yaml).unwrap());
                assert!(
                    !r.warnings
                        .iter()
                        .any(|w| w.code == "not_yet_enforced" && w.path.starts_with("tasks.gate")),
                    "{spelling}/{label}: a pause egret runs must not be reported as \
                     unenforced: {r:?}"
                );
                assert!(r.errors.is_empty(), "{spelling}/{label}: {r:?}");
            }

            // Loud: the four it does not.
            for (property, props) in [
                ("behavior", "    behavior: FAIL\n"),
                (
                    "onPause",
                    "    onPause:\n      id: hi\n      type: io.kestra.plugin.core.log.Log\n      message: hi\n",
                ),
                (
                    "tasks",
                    "    tasks:\n      - id: after\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n",
                ),
                (
                    "errors",
                    "    errors:\n      - id: oops\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n",
                ),
            ] {
                // NOT a `\`-continued literal: Rust's line continuation eats
                // the next line's leading whitespace, and the first draft of
                // this test lost the four spaces that make `pauseDuration:` a
                // property of `gate` rather than a second mapping key. YAML
                // refused it and the failure named a column, not a cause.
                let yaml = format!(
                    "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: {spelling}\n    pauseDuration: PT1S\n{props}"
                );
                let f = parse(&yaml).unwrap_or_else(|e| panic!("{spelling}/{property}: {e:?}"));
                let r = validate(&f);
                let w = r
                    .warnings
                    .iter()
                    .find(|w| w.code == "not_yet_enforced" && w.path.starts_with("tasks.gate"))
                    .unwrap_or_else(|| {
                        panic!("{spelling}/{property}: no warning for 'gate': {r:?}")
                    });
                assert!(
                    w.message.contains(property),
                    "{spelling}/{property}: the warning must name the property that is \
                     ignored, not merely the task: {}",
                    w.message
                );
                assert!(w.message.contains("W8"), "{spelling}/{property}: {}", w.message);
                assert!(
                    r.errors.is_empty(),
                    "{spelling}/{property}: an unenforced property is a warning, not an \
                     error: {r:?}"
                );
            }
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
        let yaml = "id: nightly\nnamespace: company.analytics\ninputs:\n  - id: run_date\n    type: DATE\n  - id: dry_run\n    type: BOOL\n    dependsOn:\n      inputs: [run_date]\ntasks:\n  - id: pipeline\n    type: core.dag\n    tasks:\n      - task:\n          id: extract\n          type: egret.sql\n          database: d\n          sql: SELECT 1\n      - task:\n          id: load\n          type: egret.sql\n          database: d\n          sql: SELECT 2\n        dependsOn: [extract]\n  - id: fanout\n    type: core.parallel\n    tasks:\n      - id: enrich\n        type: egret.sql\n        database: d\n        sql: SELECT 3\nerrors:\n  - id: alert\n    type: core.slack\nfinally:\n  - id: cleanup\n    type: core.log\n";
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

    // -- Task 6 fix round 2, Finding 1: presence was not enough. A subflow's
    // `namespace`/`flowId` are encoded into one `publish::subflow_ref_name`
    // string, and a value carrying its own `/` collides with the separator.

    /// The exact collision the reviewer proved live: `namespace: "a/b"`,
    /// `flowId: "c"` used to encode to `"a/b/c"` and decode, via
    /// `split_once('/')`, as `("a", "b/c")` -- a different pair than
    /// declared. `is_valid_namespace` (already enforced on `Flow::namespace`
    /// itself, `validate_namespace` above) now closes it on the subflow side
    /// too.
    #[test]
    fn subflow_namespace_containing_a_slash_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: \"a/b\"\n    flowId: c\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "subflow_namespace_invalid"),
            "{r:?}"
        );
    }

    /// The same collision, the other half of the pair: a `flowId` carrying
    /// its own `/` is rejected against Kestra's own flow `id` pattern
    /// (`is_valid_flow_id`'s doc comment cites the exact `AbstractFlow.java`
    /// lines), not a pattern invented for this check alone.
    #[test]
    fn subflow_flow_id_containing_a_slash_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: a.b2\n    flowId: \"c/d\"\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "subflow_flow_id_invalid"),
            "{r:?}"
        );
    }

    /// Companion to both: a legitimate, dotted namespace (namespaces are
    /// THEMSELVES dot-separated, which is exactly why `/` and not `.` had to
    /// be the encoding's separator) must still publish silently -- the fix
    /// must not reject everything with a `.` in it.
    #[test]
    fn subflow_with_a_legitimate_dotted_namespace_and_flow_id_is_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n",
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

    /// **The two control-flow tables name the same types, in both
    /// directions.** W8 Task 3 fix round 1.
    ///
    /// This invariant did not exist before W8 Task 3 — the tables genuinely
    /// differed, by `pause` — and that task both created it and depends on it:
    /// `is_runnable_child`'s `Leaf` arm and `walk_task`'s package-name arm were
    /// deleted as unreachable, and *unreachable* is true only while `kind_for`
    /// dispatches every name `KESTRA_FLOW_CONTROL` holds. A guard that states
    /// it is what keeps the deletion safe, rather than a comment that says it
    /// was safe once.
    ///
    /// **Both directions, and they fail for different edits:**
    ///
    /// * *table ⊆ `kind_for`* is driven by a PROBE per name — a real flow, run
    ///   through the real parser — asserting the task does not reach `Leaf`.
    ///   A row added here alone fails this half.
    /// * *`kind_for` ⊆ table* is read out of `models/task.rs`'s source, because
    ///   `kind_for` is a private `match` on string literals and nothing in Rust
    ///   can enumerate its arms. An arm added there alone fails this half.
    ///
    /// The source parse is the weaker instrument and is guarded twice: it must
    /// find the arms it expects (a regex that silently matched nothing would
    /// make the direction vacuous), and the probe direction independently
    /// exercises every name it claims to have found.
    ///
    /// **W8 Task 4 added `"waitfor"` to BOTH, and this test is why it had to
    /// be both.** The `kind_for` arm is what fixes the publish-side
    /// over-acceptance (a `WaitFor` child of a `WorkingDirectory`); this test
    /// refused the half-edit, exactly as its previous revision predicted it
    /// would. The tables now hold **12** names each.
    #[test]
    fn the_two_control_flow_tables_name_the_same_types() {
        use crate::models::{canonical_flow_type, TaskKind};

        // ---- direction 1: every row reaches a typed kind, by probe ---------
        let mut not_typed = Vec::new();
        for name in super::KESTRA_FLOW_CONTROL {
            // The canonical name is what `kind_for` dispatches on, and
            // `core.<name>` is the alias spelling `canonical_flow_type`
            // reduces. Probing through the real parser rather than calling
            // `kind_for` (which is private) is also the stronger test: it goes
            // through the same path a published flow does.
            let yaml = format!("id: t\ntype: core.{name}\n");
            let task: crate::models::Task = serde_norway::from_str(&yaml)
                .unwrap_or_else(|e| panic!("{name}: probe does not parse: {e}"));
            assert_eq!(
                canonical_flow_type(&task.r#type).as_deref(),
                Some(*name),
                "{name}: the probe does not canonicalise to the row it is probing"
            );
            if matches!(task.kind, TaskKind::Leaf { .. }) {
                not_typed.push(*name);
            }
        }
        assert!(
            not_typed.is_empty(),
            "these KESTRA_FLOW_CONTROL rows reach `TaskKind::Leaf`, so `kind_for` \
             has no arm for them: {not_typed:?}. Either add the arm, or the two \
             deletions W8 Task 3 made in this file (`is_runnable_child`'s `Leaf` \
             arm and `walk_task`'s package-name arm) stop being unreachable and \
             have to come back"
        );

        // ---- direction 2: every `kind_for` arm has a row, by source --------
        let src =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/models/task.rs"))
                .expect("read models/task.rs");
        let body = {
            let from = src.find("fn kind_for(").expect("kind_for still exists");
            let rest = &src[from..];
            &rest[..rest.find("\n}\n").expect("kind_for has an end")]
        };
        let dispatched: std::collections::BTreeSet<&str> = body
            .split("Some(\"")
            .skip(1)
            .filter_map(|chunk| chunk.split('"').next())
            .collect();
        assert_eq!(
            dispatched.len(),
            super::KESTRA_FLOW_CONTROL.len(),
            "the source walk found {} dispatched name(s) and the table holds {}. \
             A walk that found NOTHING would make the assertion below vacuous, \
             which is why the count is checked before the sets are: {dispatched:?}",
            dispatched.len(),
            super::KESTRA_FLOW_CONTROL.len()
        );
        let listed: std::collections::BTreeSet<&str> =
            super::KESTRA_FLOW_CONTROL.iter().copied().collect();
        assert_eq!(
            dispatched, listed,
            "`models::kind_for` and `KESTRA_FLOW_CONTROL` name different types. \
             `kind_for` is the authority and this table is the claim held \
             against it; whichever moved, both must"
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
    ///
    /// # It read only `r#type` for a while, and a later task's own change made
    /// it blind
    ///
    /// `registry::known_type` matches a row by its `r#type` **or by any of its
    /// `aliases`**, and this guard walked `r#type` alone -- so the silencing it
    /// exists to prevent could be introduced through an alias without it
    /// noticing, and the "impossible to introduce quietly" above was no longer
    /// true in its own scope. **SWAP proof** (W10 Task 5 fix round 2): putting
    /// `aliases: &["core.pause"]` on the `core.http` row -- which has no handler
    /// -- left `--lib` at **820 passed, 0 failed**.
    ///
    /// The guard now walks every name `known_type` can match, so the two cannot
    /// disagree about what "registered" means. **That is the fix rather than a
    /// narrowed doc sentence**: the sentence was describing the property worth
    /// having, and the guard was the half that had fallen behind.
    #[test]
    fn the_registry_and_the_control_flow_table_are_disjoint() {
        use crate::models::canonical_flow_type;
        use crate::spec::registry;

        for spec in registry::TASK_TYPES {
            // **Every name `known_type` can match**, not just the primary one.
            // A `chain` rather than two loops so a name added to either half is
            // covered by construction.
            for name in std::iter::once(spec.r#type).chain(spec.aliases.iter().copied()) {
                if let Some(canonical) = canonical_flow_type(name) {
                    assert!(
                        !super::KESTRA_FLOW_CONTROL.contains(&canonical.as_str()),
                        "registry type '{}' (matched here as '{name}') canonicalises to \
                         '{canonical}', which collides with a KESTRA_FLOW_CONTROL row -- \
                         registering it would silence the unported-control-flow warning \
                         for that class",
                        spec.r#type
                    );
                }
            }
        }
    }

    // ---------- Whole-branch review, minor finding 4: `Flow::id` ----------

    #[test]
    fn flow_id_containing_a_slash_is_an_error() {
        let f = parse("id: 'bad id/with slash'\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n")
            .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "invalid_flow_id"),
            "{r:?}"
        );
    }

    #[test]
    fn flow_id_empty_is_an_error() {
        let f = parse("id: ''\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "invalid_flow_id"),
            "{r:?}"
        );
    }

    #[test]
    fn flow_id_valid_is_silent() {
        let f = parse("id: nightly_rollup.v2\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n")
            .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code == "invalid_flow_id"),
            "{r:?}"
        );
    }

    // ---------- Whole-branch review, minor finding 5: `Quota`/`Check` ----------

    #[test]
    fn quota_missing_fields_are_errors() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\nquotas:\n  - {}\n")
            .unwrap();
        let r = validate(&f);
        for code in [
            "quota_duration_required",
            "quota_limit_required",
            "quota_behavior_required",
        ] {
            assert!(r.errors.iter().any(|e| e.code == code), "{code}: {r:?}");
        }
    }

    #[test]
    fn quota_present_fields_are_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n\
             quotas:\n  - duration: PT1H\n    limit: 5\n    behavior: FAIL\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code.starts_with("quota_")),
            "{r:?}"
        );
    }

    #[test]
    fn check_when_absent_is_an_error() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\nchecks:\n  - {}\n")
            .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "check_when_required"),
            "{r:?}"
        );
    }

    #[test]
    fn check_when_empty_string_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\nchecks:\n  - when: \"\"\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "check_when_required"),
            "an empty `when` is @NotEmpty, not just @NotNull: {r:?}"
        );
    }

    #[test]
    fn check_when_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n\
             checks:\n  - when: \"{{ outputs.t.value == true }}\"\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code == "check_when_required"),
            "{r:?}"
        );
    }

    /// `Check::message` is `@NotEmpty` only (genuinely optional, no
    /// `@NotNull` stacked on it) -- unlike `when`, its absence must not
    /// error. Not itself a new check this fix round adds (message has no
    /// `@NotNull`), but pinned so a future change cannot conflate the two
    /// fields' bean-validation strength.
    #[test]
    fn check_message_absent_is_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n\
             checks:\n  - when: \"true\"\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code.starts_with("check_")),
            "{r:?}"
        );
    }

    // ---------- Whole-branch review, minor finding 6: `If::then` ----------

    #[test]
    fn if_then_empty_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    condition: \"true\"\n    then: []\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "if_then_required"),
            "{r:?}"
        );
    }

    #[test]
    fn if_then_present_is_silent() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    condition: \"true\"\n    then:\n      - id: t\n        type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code == "if_then_required"),
            "{r:?}"
        );
    }

    // ---------- Whole-branch review, IMPORTANT finding 2: `Dependency` ----------

    fn flow_with_dependency(dep_yaml: &str) -> String {
        format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n\
             triggers:\n  - id: chain\n    type: core.flow\n    dependsOn:\n      - {dep_yaml}\n"
        )
    }

    #[test]
    fn dependency_namespace_absent_is_an_error() {
        let f = parse(&flow_with_dependency("flowId: other")).unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "dependency_namespace_required"),
            "{r:?}"
        );
    }

    #[test]
    fn dependency_flow_id_absent_is_an_error() {
        let f = parse(&flow_with_dependency("namespace: a.b")).unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "dependency_flow_id_required"),
            "{r:?}"
        );
    }

    /// Same encoding hazard `subflow_namespace_containing_a_slash_is_an_error`
    /// closes for `Subflow` -- a `/` inside `Dependency::namespace` would
    /// otherwise decode ambiguously against `publish::subflow_ref_name`'s
    /// `"{namespace}/{flowId}"` encoding.
    #[test]
    fn dependency_namespace_containing_a_slash_is_an_error() {
        let f = parse(&flow_with_dependency(
            "namespace: \"a/b\"\n        flowId: other",
        ))
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "dependency_namespace_invalid"),
            "{r:?}"
        );
    }

    #[test]
    fn dependency_flow_id_containing_a_slash_is_an_error() {
        let f = parse(&flow_with_dependency(
            "namespace: a.b\n        flowId: \"c/d\"",
        ))
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "dependency_flow_id_invalid"),
            "{r:?}"
        );
    }

    #[test]
    fn dependency_with_a_legitimate_namespace_and_flow_id_is_not_an_error() {
        let f = parse(&flow_with_dependency(
            "namespace: a.b\n        flowId: upstream",
        ))
        .unwrap();
        let r = validate(&f);
        assert!(
            !r.errors.iter().any(|e| e.code.starts_with("dependency_")),
            "{r:?}"
        );
    }

    // ---------- W13b Task 2: a flow trigger's `window` and `minSatisfied` ----------
    //
    // Each refusal has its nearest accepted neighbour beside it: a rule that
    // fires on both is an over-refusal, the mirror of what it closes.

    /// A flow whose one trigger is a flow trigger with `body` appended (each
    /// line of `body` already indented for the trigger's mapping), listening on
    /// `deps` dependencies.
    fn flow_trigger_flow(deps: usize, body: &str) -> String {
        let mut depends_on = String::new();
        if deps > 0 {
            depends_on.push_str("    dependsOn:\n");
            for i in 0..deps {
                depends_on.push_str(&format!(
                    "      - namespace: a.b\n        flowId: p{i}\n        states: [SUCCESS]\n"
                ));
            }
        }
        format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
             triggers:\n  - id: listen\n    type: io.kestra.plugin.core.trigger.Flow\n\
             {depends_on}{body}"
        )
    }

    /// The error findings for a flow trigger, as `(code, path)`.
    fn trigger_errors(deps: usize, body: &str) -> Vec<(&'static str, String)> {
        let src = flow_trigger_flow(deps, body);
        let f = parse(&src).unwrap_or_else(|e| panic!("{src}\ndoes not parse: {e:?}"));
        validate(&f)
            .errors
            .into_iter()
            .map(|e| (e.code, e.path))
            .collect()
    }

    fn assert_refused(deps: usize, body: &str, code: &str, path: &str) {
        let errors = trigger_errors(deps, body);
        assert!(
            errors.iter().any(|(c, p)| *c == code && p == path),
            "expected {code} at {path} for\n{body}\ngot {errors:?}"
        );
    }

    fn assert_clean(deps: usize, body: &str) {
        let errors = trigger_errors(deps, body);
        assert!(
            errors.is_empty(),
            "expected no errors for\n{body}\ngot {errors:?}"
        );
    }

    const W: &str = "triggers.listen.window";

    #[test]
    fn every_measured_window_shape_is_accepted() {
        // The shapes W13b Task 1 fired on the artifact, plus no window at all.
        for body in [
            "",
            "    window:\n      every: P1D\n",
            "    window:\n      every: PT6H\n",
            "    window:\n      every: PT90M\n",
            "    window:\n      every: PT45S\n",
            "    window:\n      every: PT2M\n      offset: PT1M\n",
            "    window:\n      every: P1D\n      offset: PT6H\n",
            "    window:\n      lookback: PT2M\n",
            "    window:\n      from: \"03:00:00\"\n      to: \"05:00:00\"\n",
            "    window:\n      deadline: \"05:00:00\"\n",
            "    window:\n      deadline: \"12:00:00\"\n      timezone: Asia/Ho_Chi_Minh\n",
            "    window:\n      every: PT1H\n      timezone: Asia/Kolkata\n",
            "    window:\n      from: \"10:00\"\n      to: \"12:00\"\n      timezone: Asia/Ho_Chi_Minh\n",
            "    window: {}\n",
        ] {
            assert_clean(2, body);
        }
    }

    #[test]
    fn a_deadline_with_a_from_is_refused() {
        assert_refused(
            2,
            "    window:\n      deadline: \"09:00\"\n      from: \"08:00\"\n",
            "window_conflicting_fields",
            &format!("{W}.from"),
        );
        // And with every other field of another kind.
        for (field, value) in [
            ("to", "\"10:00\""),
            ("every", "PT1H"),
            ("offset", "PT1H"),
            ("lookback", "PT1H"),
        ] {
            assert_refused(
                2,
                &format!("    window:\n      deadline: \"09:00\"\n      {field}: {value}\n"),
                "window_conflicting_fields",
                &format!("{W}.{field}"),
            );
        }
        assert_clean(2, "    window:\n      deadline: \"09:00\"\n");
    }

    #[test]
    fn a_from_without_a_to_is_refused_and_so_is_the_reverse() {
        assert_refused(
            2,
            "    window:\n      from: \"08:00\"\n",
            "window_daily_bound_missing",
            &format!("{W}.to"),
        );
        assert_refused(
            2,
            "    window:\n      to: \"08:00\"\n",
            "window_daily_bound_missing",
            &format!("{W}.from"),
        );
        assert_clean(
            2,
            "    window:\n      from: \"08:00\"\n      to: \"09:00\"\n",
        );
    }

    #[test]
    fn a_daily_window_with_a_duration_field_is_refused() {
        for field in ["every", "offset", "lookback"] {
            assert_refused(
                2,
                &format!("    window:\n      from: \"08:00\"\n      to: \"09:00\"\n      {field}: PT1H\n"),
                "window_conflicting_fields",
                &format!("{W}.{field}"),
            );
        }
    }

    /// The artifact's own 422, word for word.
    #[test]
    fn a_lookback_with_an_every_is_refused_with_kestras_message() {
        let src = flow_trigger_flow(2, "    window:\n      every: PT1H\n      lookback: PT2M\n");
        let r = validate(&parse(&src).unwrap());
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "window_conflicting_fields")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert_eq!(e.path, format!("{W}.every"));
        assert!(
            e.message
                .ends_with("Window of type `SLIDING_WINDOW` cannot have an every duration."),
            "{}",
            e.message
        );
        assert_refused(
            2,
            "    window:\n      lookback: PT2M\n      offset: PT1M\n",
            "window_conflicting_fields",
            &format!("{W}.offset"),
        );
        assert_clean(2, "    window:\n      lookback: PT2M\n");
    }

    #[test]
    fn an_unparseable_duration_is_refused_in_every_field() {
        for (field, value, others) in [
            ("every", "P1M", ""),
            ("every", "PT1.5S", ""),
            ("every", "garbage", ""),
            ("offset", "-PT1H", "      every: P1D\n"),
            ("offset", "P1Y", "      every: P1D\n"),
            ("lookback", "P1W", ""),
            ("lookback", "pt2m", ""),
        ] {
            assert_refused(
                2,
                &format!("    window:\n{others}      {field}: {value}\n"),
                "window_duration_invalid",
                &format!("{W}.{field}"),
            );
        }
        // W13b Task 2 fix round 1: the 366-day cap, at the cap and cap + 1
        // second, in each of the three duration fields.
        for (field, others) in [
            ("every", ""),
            ("offset", "      every: P1D\n"),
            ("lookback", ""),
        ] {
            assert_refused(
                2,
                &format!("    window:\n{others}      {field}: P366DT1S\n"),
                "window_duration_invalid",
                &format!("{W}.{field}"),
            );
            assert_clean(2, &format!("    window:\n{others}      {field}: P366D\n"));
        }
        // The message is the parser's, so "no fixed length" reaches the author.
        let src = flow_trigger_flow(2, "    window:\n      every: P1M\n");
        let r = validate(&parse(&src).unwrap());
        assert!(
            r.errors
                .iter()
                .any(|e| e.message.contains("no fixed length")),
            "{r:?}"
        );
    }

    #[test]
    fn a_zero_every_or_lookback_is_refused_and_a_zero_offset_is_not() {
        assert_refused(
            2,
            "    window:\n      every: PT0S\n",
            "window_duration_not_positive",
            &format!("{W}.every"),
        );
        assert_refused(
            2,
            "    window:\n      lookback: P0D\n",
            "window_duration_not_positive",
            &format!("{W}.lookback"),
        );
        assert_clean(2, "    window:\n      every: PT1H\n      offset: PT0S\n");
    }

    #[test]
    fn an_offset_without_an_every_is_refused() {
        assert_refused(
            2,
            "    window:\n      offset: PT6H\n",
            "window_offset_without_every",
            &format!("{W}.offset"),
        );
        assert_clean(2, "    window:\n      every: P1D\n      offset: PT6H\n");
    }

    /// Kestra reads a window only on the `dependsOn` arm, so there it is
    /// inert; egret refuses rather than silently ignoring a declaration.
    #[test]
    fn a_window_on_a_trigger_with_no_depends_on_is_refused() {
        assert_refused(
            0,
            "    window:\n      every: PT1H\n",
            "window_without_depends_on",
            W,
        );
        // `dependsOn: []` is the same arm as no `dependsOn` (`ListUtils.isEmpty`).
        assert_refused(
            0,
            "    dependsOn: []\n    window:\n      every: PT1H\n",
            "window_without_depends_on",
            W,
        );
        assert_clean(0, "");
        assert_clean(1, "    window:\n      every: PT1H\n");
    }

    #[test]
    fn an_unparseable_time_of_day_is_refused() {
        for (field, body) in [
            ("deadline", "    window:\n      deadline: \"25:00\"\n"),
            ("deadline", "    window:\n      deadline: \"9am\"\n"),
            (
                "from",
                "    window:\n      from: \"08:00:00.5\"\n      to: \"09:00\"\n",
            ),
            (
                "to",
                "    window:\n      from: \"08:00\"\n      to: \"9:00\"\n",
            ),
        ] {
            assert_refused(2, body, "window_time_invalid", &format!("{W}.{field}"));
        }
    }

    #[test]
    fn an_empty_daily_window_is_refused() {
        for body in [
            "    window:\n      from: \"12:00\"\n      to: \"10:00\"\n",
            "    window:\n      from: \"10:00\"\n      to: \"10:00:00\"\n",
        ] {
            assert_refused(2, body, "window_daily_range_empty", &format!("{W}.to"));
        }
        assert_refused(
            2,
            "    window:\n      deadline: \"00:00\"\n",
            "window_daily_range_empty",
            &format!("{W}.deadline"),
        );
        assert_clean(
            2,
            "    window:\n      from: \"00:00\"\n      to: \"23:59:59\"\n",
        );
        assert_clean(2, "    window:\n      deadline: \"00:00:01\"\n");
    }

    #[test]
    fn a_timezone_that_is_not_an_iana_zone_is_refused() {
        for zone in ["Mars/Olympus_Mons", "\"+07:00\"", "asia/kolkata", "\"\""] {
            assert_refused(
                2,
                &format!("    window:\n      every: PT1H\n      timezone: {zone}\n"),
                "window_timezone_invalid",
                &format!("{W}.timezone"),
            );
        }
        for zone in ["UTC", "Europe/Paris", "Asia/Ho_Chi_Minh"] {
            assert_clean(
                2,
                &format!("    window:\n      every: PT1H\n      timezone: {zone}\n"),
            );
        }
    }

    #[test]
    fn min_satisfied_below_one_is_refused_under_any_mode() {
        for mode in ["ALL", "ANY", "AT_LEAST"] {
            for n in [0, -1] {
                assert_refused(
                    2,
                    &format!("    mode: {mode}\n    minSatisfied: {n}\n"),
                    "min_satisfied_not_positive",
                    "triggers.listen.minSatisfied",
                );
            }
        }
    }

    #[test]
    fn at_least_without_min_satisfied_is_refused() {
        assert_refused(
            2,
            "    mode: AT_LEAST\n",
            "min_satisfied_required",
            "triggers.listen.minSatisfied",
        );
        assert_clean(2, "    mode: AT_LEAST\n    minSatisfied: 1\n");
    }

    #[test]
    fn at_least_asking_for_more_conditions_than_declared_is_refused() {
        assert_refused(
            2,
            "    mode: AT_LEAST\n    minSatisfied: 3\n",
            "min_satisfied_exceeds_conditions",
            "triggers.listen.minSatisfied",
        );
        assert_clean(2, "    mode: AT_LEAST\n    minSatisfied: 2\n");
        // Only AT_LEAST reads it; under ALL/ANY an oversized value is inert,
        // as on Kestra, and is not refused.
        assert_clean(2, "    mode: ALL\n    minSatisfied: 3\n");
        assert_clean(2, "    mode: ANY\n    minSatisfied: 3\n");
    }

    /// The artifact refuses an eighth window field with 422 `Unrecognized
    /// field "fireOnce"`; egret refuses it at PARSE (`Window` has
    /// `deny_unknown_fields`), so no draft carrying it can be stored.
    #[test]
    fn fire_once_and_any_other_unknown_window_field_fail_to_parse() {
        for field in ["fireOnce: true", "bogusField: 1", "resetOnSuccess: false"] {
            let src = flow_trigger_flow(
                2,
                &format!("    window:\n      every: PT1H\n      {field}\n"),
            );
            let e = parse(&src).expect_err(field);
            let name = field.split(':').next().unwrap();
            assert!(
                e.message.contains(&format!("unknown field `{name}`")),
                "{field}: {}",
                e.message
            );
        }
        assert!(parse(&flow_trigger_flow(2, "    window:\n      every: PT1H\n")).is_ok());
    }
}
