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
//! 3. **Not-yet-enforced** -- FLOW-LEVEL features that parse and validate
//!    cleanly but that no package in this branch actually acts on yet
//!    (`NOT_YET_ENFORCED`, below: `concurrency`, `sla`, `checks`, `quotas`,
//!    triggers). These are warnings, never errors: the flow still runs, it
//!    just runs without the feature, and the warning is what stops a green
//!    publish lying about that.
//!
//!    **A TASK egret cannot run is not this layer any more, and that is
//!    no-silent-stall Task 1** (spec §3). A task-level construct egret has no
//!    executor for does not run "without the feature": `build_plan` plans it
//!    an opaque runnable no handler serves, and every execution that reached
//!    it stalled in `RUNNING` for ever. So each is an ERROR, in
//!    `validate_for_run`, which publish and every start path ask:
//!    `unsupported_allow_failure` (until #680, which made `AllowFailure` a
//!    flowable; since then only `unsupported_allow_failure_retry`, a block
//!    `retry:`; `unsupported_working_directory` likewise went with #683 --
//!    noted 2026-10-17, kill Task 2 fix round 1),
//!    `unsupported_loop_until_property` (a bound `executor::loop_until_gap`
//!    names), `unsupported_pause_property` (`executor::pause_gap`, W7b),
//!    `unsupported_foreach` (W7b), `unsupported_flow_task` (a type spelled
//!    like Kestra control flow that egret does not implement -- it was the
//!    task-level `not_yet_enforced` warning) and `unknown_task_type` (a
//!    warning until then). A draft still SAVES with any of them; only
//!    publish and start refuse. This is why an unported control-flow type
//!    must never be left to fall silently through to `Leaf`: a `Pause` a user
//!    believes is a manual-approval gate that quietly runs straight through
//!    it is worse than refusing to publish the flow at all.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde::Serialize;

use crate::models::{
    canonical_flow_type, Flow, Input, InputType, Property, Retry, Sla, SlaCommon, Task, TaskKind,
    Trigger, TriggerKind,
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
/// outside this validator.
///
/// **`triggers` is the second row to narrow** -- W7a's final review. The row
/// said "enforcement lands in W6", and W7a's Problems tab showed that sentence
/// on every flow with a trigger. It was false for the three typed kinds:
/// `Schedule` fires from `scheduler::{sync,fire}` (W6), `Webhook` from
/// `webhook_routes` (W12) and `Flow` from `flow_trigger` (W13). A trigger of
/// any other `type` parses to `TriggerKind::Leaf`, which nothing reads, so it
/// is declared and never fires: for that flow the warning is true. So the row
/// stays, its package names no W-number, and [`FeaturePresence::triggers`] is
/// now *declares a `Leaf` trigger*.
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
///
/// **2026-10-26 (no-silent-drops Task 8, spec D4):** the second column is no
/// longer a package name but the closing clause of the warning, taken from the
/// roadmap (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`): `concurrency`
/// is package #8, `sla` and `checks` #16, `quotas` is Kestra Enterprise only,
/// and the `triggers` row names #11 and #18. The old "W8" and "a later package"
/// named a package that no longer exists, or none. The prose above is history
/// and still speaks of W-numbers.
const NOT_YET_ENFORCED: &[(&str, &str)] = &[
    ("concurrency", "roadmap package #8 lifts it"),
    ("sla", "roadmap package #16 lifts it"),
    ("checks", "roadmap package #16 lifts it"),
    (
        "quotas",
        "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap \u{a7}3.3)",
    ),
    (
        "triggers",
        "egret fires only Schedule, Webhook and Flow triggers; roadmap package #11 lifts \
         ScheduleOnDates and http.Trigger, #18 lifts plugin triggers, and McpToolTrigger is not \
         engine scope (roadmap \u{a7}3.3)",
    ),
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
/// both `RunnableTask` at the pin. There is no third.
///
/// **"Accepted" was the word here, and since no-silent-stall Task 1 it holds
/// for `Sleep` only.** `Sleep` is a registered type with a handler.
/// `WorkingDirectoryTest$LocalFiles` is a Kestra TEST class with no handler
/// (its five flows were 422 at harvest, `task-type-not-in-image`), and a
/// `Leaf` spelled like control flow that no registry row names is now the
/// ERROR `unsupported_flow_task` rather than a `not_yet_enforced` warning,
/// because a run reaching it stalled with `NoHandler`. Neither outcome reads
/// this table.
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
///
/// **An `Exit` is a `Leaf` and is refused anyway** (kill-and-exit Task 3 fix
/// round 1, 2026-10-17). It is an `ExecutionUpdatableTask` in Kestra
/// (`Exit.java:71`), not a `RunnableTask`, so Kestra's
/// `WorkingDirectoryTaskValidator` refuses it; and egret runs it in
/// `advance()` (`executor::TaskKind::Exit`), which would bypass the block's
/// finish claim. Every executor-level registered type is refused the same way
/// (`spec::registry::EXECUTOR_LEVEL`).
fn is_runnable_child(task: &Task) -> bool {
    matches!(task.kind, TaskKind::Leaf { .. }) && !registry::executor_level(&task.r#type)
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

/// **The save-time report** with the key census of package #1 (spec D11):
/// [`validate`] plus [`crate::spec::keys::findings`], which reads the raw
/// source for the keys serde dropped. Every caller that reports on a SAVED
/// or PUBLISHING flow calls this; the run-time gate never does
/// (`executor::build_plan` calls [`validate_for_run`]), so a version
/// published before #1 keeps running.
pub fn validate_source(source: &str, flow: &Flow) -> ValidationReport {
    let mut report = validate(flow);
    crate::spec::keys::findings(source, flow, &mut report);
    report
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
    duplicate_trigger_ids(flow, report);
    secret_findings(flow, report);
    http_findings(flow, report);
    schedule_findings(flow, report);
    input_save_findings(&flow.inputs, report);
}

/// The warning for a Schedule `when` or `lateMaximumDelay`: Kestra accepts and
/// honours the key, egret's scheduler refuses it when it syncs the trigger.
/// egret's own words, and key names only (spec D10): the author's value is
/// never echoed. It does not embed `ScheduleError`'s `Display` because that one
/// ends in the same package clause (it is the trigger row's `last_error`).
fn sync_refusal(id: &str, key: &str) -> Finding {
    Finding {
        code: "schedule_not_resolvable",
        message: format!(
            "trigger '{id}': core.schedule `{key}` is not supported: egret refuses it when it syncs the trigger, so this Schedule trigger will not fire. Roadmap package #11 lifts it"
        ),
        path: format!("triggers.{id}.{key}"),
    }
}

/// KI-122: what the scheduler would refuse about a `Schedule`, said at save.
/// Warnings, not errors: the flow saves and runs by hand as before, and the
/// trigger row records the same refusal as its `last_error`; the editor now
/// says it before the first tick instead of after it.
///
/// - `schedule_not_resolvable`: [`ScheduleTrigger::resolve`] refuses it -- a
///   cron that does not parse, an unknown timezone, a `when` or a
///   `lateMaximumDelay` egret does not support -- so it never fires
///   (`scheduler::sync` stores the same message). A `cron` left out is the
///   error `schedule_cron_required` already, so it is not said twice.
/// - `schedule_input_templated`: an `inputs` value with `{{ }}` or `{% %}`,
///   which every tick refuses (`scheduler::fire::refuse_templated`): a
///   scheduled execution has no context to render it in.
///
/// [`ScheduleTrigger::resolve`]: crate::trigger::ScheduleTrigger::resolve
fn schedule_findings(flow: &Flow, report: &mut ValidationReport) {
    use crate::trigger::{ScheduleError, ScheduleTrigger};
    for trigger in &flow.triggers {
        let TriggerKind::Schedule(schedule) = &trigger.kind else {
            continue;
        };
        // `resolve` names the first refusal only, and `when` comes before
        // everything else, so a second resolve without `when` finds the next:
        // each of `when` and `lateMaximumDelay` is said once, at its own key
        // (no-silent-drops D6/D17), and `keys.rs` leaves both keys to this.
        let mut refusal = ScheduleTrigger::resolve(trigger);
        if let Err(ScheduleError::UnsupportedWhenCondition) = &refusal {
            report.warnings.push(sync_refusal(&trigger.id, "when"));
            let mut without_when = trigger.clone();
            without_when.when = None;
            refusal = ScheduleTrigger::resolve(&without_when);
        }
        match refusal {
            Ok(_) | Err(ScheduleError::MissingCron) => {}
            Err(ScheduleError::UnsupportedLateMaximumDelay) => {
                report
                    .warnings
                    .push(sync_refusal(&trigger.id, "lateMaximumDelay"));
            }
            Err(e) => report.warnings.push(Finding {
                code: "schedule_not_resolvable",
                message: format!("trigger '{}' will never fire: {e}", trigger.id),
                path: format!("triggers.{}", trigger.id),
            }),
        }
        for (key, value) in schedule.inputs.iter().flatten() {
            if crate::scheduler::templated(value) {
                report.warnings.push(Finding {
                    code: "schedule_input_templated",
                    message: format!(
                        "trigger '{}': inputs.{key} contains template syntax, which a \
                         scheduled execution cannot render; every tick of this schedule \
                         will be refused",
                        trigger.id
                    ),
                    path: format!("triggers.{}.inputs.{key}", trigger.id),
                });
            }
        }
    }
}

/// W7e spec §3.8: a trigger id declared twice makes the switch ambiguous.
/// Save-only on purpose -- raised from `validate_for_run` it would stop every
/// run of a stored flow that already declares one.
fn duplicate_trigger_ids(flow: &Flow, report: &mut ValidationReport) {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut reported: HashSet<&str> = HashSet::new();
    for t in &flow.triggers {
        if !seen.insert(t.id.as_str()) && reported.insert(t.id.as_str()) {
            report.errors.push(Finding {
                code: "duplicate_trigger_id",
                message: format!("trigger id '{}' is declared more than once", t.id),
                path: format!("triggers.{}", t.id),
            });
        }
    }
}

/// The code of an http task property egret refuses (spec D4): a finding at
/// save, and the same refusal fails a stored flow's run
/// (`http::build_request`).
pub const CODE_HTTP_OPTION_REFUSED: &str = "http_option_refused";

/// Package #4 (spec D4): every http task's [`crate::http::refusals`], one
/// error each, at the property's path. **Save-only**, as D4 states: a stored
/// version that carries one still plans and its run fails at the task naming
/// the same property, rather than the whole execution being refused at start.
/// A task whose properties do not even read as an http task is left to the
/// run, which names the shape (publish cannot know what a template renders
/// to).
fn http_findings(flow: &Flow, report: &mut ValidationReport) {
    for (path, task, spec) in crate::plugins::leaves(flow) {
        if !crate::worker::handlers::http::resolves_secrets(&task.r#type) {
            continue;
        }
        let Ok(parsed) = crate::http::HttpSpec::from_value(spec) else {
            continue;
        };
        // `encryptBody` is a property of `Request` only, and the refusal's
        // text is about Request's `body` output. On `Download`/`SseRequest`
        // the key is not Kestra's at all, so `spec::keys` already reports it
        // as `unknown_property`, an error at the same path; a second finding
        // there would be noise with the wrong reason (package #4 Task C6).
        let is_request = registry::known_type(&task.r#type)
            .is_some_and(|row| row.r#type == crate::worker::handlers::http::REQUEST);
        for message in crate::http::refusals(&parsed) {
            let key = message.split(':').next().unwrap_or_default().to_string();
            if key == "encryptBody" && !is_request {
                continue;
            }
            report.errors.push(Finding {
                code: CODE_HTTP_OPTION_REFUSED,
                message: format!("task '{}': {message}", task.id),
                path: format!("{path}.{key}"),
            });
        }
    }
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
///
/// W9c Task 4 (W9c spec §7): a `Set` or `Put` task's `value` is scanned on
/// its own, and a call there is `secret_in_kv_value`, reported instead of
/// `secret_outside_plugin_task` for that property.
fn secret_findings(flow: &Flow, report: &mut ValidationReport) {
    use crate::runners::secrets::{
        refused_argument_message, text_uses_secret, written_refused_arguments,
    };
    use crate::worker::handlers::kv;
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

        /// W9c spec §7: every string of a `Set` or `Put` task's `value`, keys
        /// too, that calls `secret()` is [`CODE_SECRET_IN_KV_VALUE`], INSTEAD
        /// of the generic refusal, which this property never reports.
        ///
        /// [`CODE_SECRET_IN_KV_VALUE`]: crate::plugins::CODE_SECRET_IN_KV_VALUE
        fn kv_value(&mut self, task: &str, v: &serde_json::Value, path: String) {
            let mut found = Vec::new();
            strings(v, path, &mut found);
            for (p, text) in found {
                if text_uses_secret(text) {
                    self.report.errors.push(Finding {
                        code: crate::plugins::CODE_SECRET_IN_KV_VALUE,
                        message: crate::plugins::secret_in_kv_value_message(task, &p),
                        path: p,
                    });
                }
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
                    // W9c spec §7: a `Set` or `Put` task's `value`.
                    let kv_value_of = match (id, ty) {
                        (Some(i), Some(t)) if t == kv::SET || t == kv::PUT => Some(i),
                        _ => None,
                    };
                    for (k, v) in map {
                        let here = join(&path, k);
                        self.outside(path.clone(), k);
                        if let (Some(task), "value") = (kv_value_of, k.as_str()) {
                            self.kv_value(task, v, here);
                        } else if is_plugin && !TASK_KEYS.contains(&k.as_str()) {
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
    // Package #4 Task C3 (spec D5): `secret()` resolves on the http tasks
    // too (`worker::handlers::http::SECRET_TYPES`, every spelling), so their
    // properties are exempt as a plugin task's are.
    let tasks = crate::plugins::leaves(flow)
        .into_iter()
        .filter(|(_, t, _)| {
            t.r#type.starts_with(crate::plugins::PREFIX)
                || crate::worker::handlers::http::resolves_secrets(&t.r#type)
        });
    let mut walk = Walk {
        plugin: tasks
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
        // Narrowed at W7a's final review: Schedule, Webhook and Flow fire;
        // only a trigger of another type (`Leaf`) is declared and unenforced.
        triggers: flow
            .triggers
            .iter()
            .any(|t| matches!(t.kind, TriggerKind::Leaf { .. })),
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
    for (list_name, list) in super::path::root_lists(flow) {
        for task in list {
            let path = super::path::task_path(list_name, &task.id);
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
    validate_after_execution(flow, &mut report);

    validate_inputs(&flow.inputs, &mut report);

    for trigger in &flow.triggers {
        validate_trigger(trigger, &mut report);
    }
    validate_schedule_inputs(flow, &mut report);
    validate_dashed_references(flow, &mut report);

    emit_not_yet_enforced(&features, &mut report);

    report
}

/// End hooks #2b (spec D12): `Pause`, `Exit` and `LoopUntil` (`WaitFor` is its
/// alias) are refused anywhere inside `afterExecution:`. That list runs once
/// the execution is terminal, so a `Pause` would mark an ended execution
/// `PAUSED`, an `Exit` would set its state, and a `LoopUntil` waits between
/// iterations with `PauseRun` (the pause machinery, package #9);
/// `after_execution::drive` asserts none of them happens. Walks
/// `children()` and `error_children()` (finally lists included) from the
/// list's roots. Lives in the run-time gate so `build_plan` refuses at start.
/// Key and id names only (package #1, D10).
fn validate_after_execution(flow: &Flow, report: &mut ValidationReport) {
    fn walk(task: &Task, path: String, report: &mut ValidationReport) {
        if let Some(message) = after_execution_refusal(task) {
            report.errors.push(Finding {
                code: AFTER_EXECUTION_REFUSAL_CODE,
                message,
                path: path.clone(),
            });
        }
        for child in task.children().into_iter().chain(task.error_children()) {
            walk(child, super::path::task_path(&path, &child.id), report);
        }
    }
    for task in &flow.after_execution {
        walk(
            task,
            super::path::task_path("afterExecution", &task.id),
            report,
        );
    }
}

/// The code of [`after_execution_refusal`]'s finding.
pub(crate) const AFTER_EXECUTION_REFUSAL_CODE: &str = "unsupported_after_execution_task";

/// **The one D12 predicate** (end hooks #2b): why `task`, found anywhere
/// inside `afterExecution:`, is refused -- `Pause`, `Exit`, or `LoopUntil`
/// (`WaitFor` is its alias) -- or `None`. It looks at `task` alone; the
/// caller walks the subtree.
///
/// Two readers, one text: [`validate_after_execution`] refuses such a flow at
/// save and at start, and the planner (`executor::plan::flatten_flow`)
/// records it in `Plan::after_execution_refused` so that a flow STORED before
/// the refusal existed -- an execution running at deploy (D4), or a
/// `CREATE_NEW_EXECUTION` replacement planned with `plan_stored` -- fails
/// that after-run with this message instead of running it
/// (`executor::after_execution::drive`, final-review fix 2026-10-28).
pub(crate) fn after_execution_refusal(task: &Task) -> Option<String> {
    let (kind, why, package) = match &task.kind {
        TaskKind::Pause(_) => (
            "a Pause",
            "the execution has already ended, and a Pause would pause it again",
            9,
        ),
        TaskKind::Leaf { .. } if task.r#type == crate::executor::exit::TYPE_ID => (
            "an Exit",
            "the execution has already ended, and an Exit would set its state",
            20,
        ),
        TaskKind::LoopUntil(_) => (
            "a LoopUntil",
            "it waits between iterations by pausing the run, and the execution has already ended",
            9,
        ),
        _ => return None,
    };
    Some(format!(
        "task '{}' is {kind} inside afterExecution, which egret does not run: {why}. Roadmap package #{package} lifts it",
        task.id
    ))
}

/// W5a: every script task (`crate::scripts`) is parsed, and each property,
/// runner or value W5a does not implement is an ERROR naming it -- never
/// ignored (W5a spec §3). Walks `children()` AND `error_children()` through
/// `scripts::script_tasks` -- as `walk_task` does too since no-silent-stall
/// Task 1 -- so a script task inside a flowable's `errors:` is refused too.
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
/// (`plugins::plugin_tasks`), so a plugin task inside a flowable's `errors:`
/// is refused too. The bare-FQCN error is raised here rather than in
/// `walk_task`'s `Leaf` arm because `walk_task` did not reach `errors:` when it
/// was written; it does since no-silent-stall Task 1, and the arm still leaves
/// both plugin spellings to this function, so nothing is reported twice.
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
/// only when the pattern passes, so no message is doubled. W9c: then no `_kv`
/// segment (`nsfiles::access::check_kv_segment`), only when both pass.
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
    } else if let Err(message) = crate::storage::nsfiles::access::check_kv_segment(&flow.namespace)
    {
        // W9c: a `_kv` segment would put this namespace's execution files and
        // namespace files under another namespace's KV values (W9c spec §3).
        // The same function refuses it at run time (`check_namespace`).
        report.errors.push(Finding {
            code: "namespace_kv_segment",
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
/// `Subflow::flow_id` unable to carry a `/`. It was introduced because
/// publish then encoded the pair into one `name_or_id` as
/// `"{namespace}/{flowId}"`. See `is_valid_namespace`'s use in the
/// `TaskKind::Subflow` arm below for the other half.
/// (Debt §5.1, 2026-10-19: publish no longer encodes the pair -- it looks
/// `namespace` and `flowId` up as two columns, `FlowRepo::find_in_workspace`
/// -- so the grammar now stands on being Kestra's own, not on a separator.)
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

/// Inputs package #3a (spec D11): the input checks Kestra makes when a flow
/// is SAVED and egret did not. Save-only ([`save_only_findings`]), never the
/// run gate: a version published before these existed keeps running, and at
/// run time Kestra ignores an unknown `dependsOn` id too (D6). The text after
/// `input '<id>': ` is Kestra's own message, character for character; the
/// two `dependsOn` messages name the input themselves, as Kestra's do.
///
/// - `input_depends_on_unknown`, `input_depends_on_cycle`:
///   `FlowValidator.checkFlowInputsDependencyGraph` (FlowValidator.java:241-270).
///   The graph is keyed by input id; a repeated id merges its `dependsOn`
///   lists, as Java's `putIfAbsent` + `addAll` does. Inputs are visited in
///   declared order (Java's is `HashMap` order). For each input, its unknown
///   dependencies in listed order, then the cycle `CycleDependency.findCycle`
///   finds from it, printed as Java's `List.toString` (`[a, b]`). Every input
///   on a cycle reports it from its own start, as in Kestra; one that only
///   leads INTO a cycle reports the cycle too. FORM children are not
///   expanded (W1 does not model FORM).
/// - `input_defaults_with_prefill`: `InputValidator.java:53-59`, every type.
/// - `input_file_defaults_invalid`: `FileInputValidator.java:33-50`.
/// - `input_bound_invalid` (review I1): an INT / FLOAT / DURATION `min` or
///   `max`, or a DATETIME / DATE / TIME `after` or `before`, that does not
///   parse for its type. Kestra's fields are typed, so Jackson refuses the
///   flow when it reads it; a non-integral INT bound, which Jackson would
///   truncate (`ACCEPT_FLOAT_AS_INT`), is refused here (a recorded
///   divergence). See `inputs_check::bound_refusals`.
/// - `input_validator_invalid`: `RegexValidator.java:26-32` on STRING and
///   SECRET, compiled with `fancy-regex` (D10) rather than `java.util.regex`.
///   Compiled by `inputs_check::anchored_validator`, the start check's own
///   function: the pattern as written (as Kestra compiles it) and its
///   anchored form, so save and start cannot disagree (review I2).
///
/// Not added (D11): "a default needs `required: true`".
fn input_save_findings(inputs: &[Input], report: &mut ValidationReport) {
    input_dependency_findings(inputs, report);
    for input in inputs {
        if input.defaults.is_some() && input.prefill.is_some() {
            report.errors.push(Finding {
                code: "input_defaults_with_prefill",
                message: format!(
                    "input '{}': Inputs with a default value cannot also have a prefill.",
                    input.id
                ),
                path: format!("inputs.{}.prefill", input.id),
            });
        }
        if input.r#type == InputType::File {
            if let Some(why) = input.defaults.as_ref().and_then(file_defaults_refusal) {
                report.errors.push(Finding {
                    code: "input_file_defaults_invalid",
                    message: format!("input '{}': {why}", input.id),
                    path: format!("inputs.{}.defaults", input.id),
                });
            }
        }
        for (field, cause) in crate::inputs_check::bound_refusals(input) {
            report.errors.push(Finding {
                code: "input_bound_invalid",
                message: format!("input '{}': invalid `{field}`: {cause}", input.id),
                path: format!("inputs.{}.{field}", input.id),
            });
        }
        if matches!(input.r#type, InputType::String | InputType::Secret) {
            if let Some(v) = input.validator.as_deref() {
                if crate::inputs_check::anchored_validator(v).is_err() {
                    report.errors.push(Finding {
                        code: "input_validator_invalid",
                        message: format!("input '{}': invalid pattern [{v}]", input.id),
                        path: format!("inputs.{}.validator", input.id),
                    });
                }
            }
        }
    }
}

/// The two `dependsOn` checks of [`input_save_findings`].
fn input_dependency_findings(inputs: &[Input], report: &mut ValidationReport) {
    let mut order: Vec<&str> = Vec::new();
    let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
    for input in inputs {
        let deps = graph.entry(input.id.as_str()).or_insert_with(|| {
            order.push(input.id.as_str());
            Vec::new()
        });
        if let Some(d) = &input.depends_on {
            deps.extend(d.inputs.iter().map(String::as_str));
        }
    }
    for &id in &order {
        for &dep in &graph[id] {
            if !graph.contains_key(dep) {
                report.errors.push(Finding {
                    code: "input_depends_on_unknown",
                    message: format!(
                        "Input with id '{id}' depends on a non-existent input '{dep}'."
                    ),
                    path: format!("inputs.{id}.dependsOn.inputs"),
                });
            }
        }
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        if let Some(cycle) = find_input_cycle(id, &graph, &mut visiting, &mut visited, &mut path) {
            report.errors.push(Finding {
                code: "input_depends_on_cycle",
                message: format!(
                    "Cycle dependency detected for input with id '{id}': [{}]",
                    cycle.join(", ")
                ),
                path: format!("inputs.{id}.dependsOn"),
            });
        }
    }
}

/// kestra: `FlowValidator.CycleDependency.findCycle`, line for line: the
/// part of the current path from the first visit of the node met again.
fn find_input_cycle<'a>(
    id: &'a str,
    graph: &HashMap<&'a str, Vec<&'a str>>,
    visiting: &mut HashSet<&'a str>,
    visited: &mut HashSet<&'a str>,
    path: &mut Vec<&'a str>,
) -> Option<Vec<&'a str>> {
    if visiting.contains(id) {
        let start = path.iter().position(|p| *p == id).unwrap_or(0);
        return Some(path[start..].to_vec());
    }
    if visited.contains(id) {
        return None;
    }
    visiting.insert(id);
    path.push(id);
    if let Some(deps) = graph.get(id) {
        for &dep in deps {
            if let Some(cycle) = find_input_cycle(dep, graph, visiting, visited, path) {
                return Some(cycle);
            }
        }
    }
    visiting.remove(id);
    visited.insert(id);
    path.pop();
    None
}

/// Why a FILE input's `defaults` is refused, in Kestra's words, or `None`.
///
/// Kestra renders the property with an empty context and converts it to a
/// `java.net.URI`: a render or conversion failure is the "rendered
/// immediately" message, and a URI whose scheme is not `file` or `nsfile`
/// (no scheme included: `something` is refused) is the other one. The render
/// here is egret's with an empty context; the URI is checked by
/// [`java_uri_scheme`].
fn file_defaults_refusal(defaults: &Property<serde_json::Value>) -> Option<&'static str> {
    const NOT_IMMEDIATE: &str = "inputs of type 'FILE' only support `defaults` with expression \
                                 that can be rendered immediately";
    const BAD_SCHEME: &str = "inputs of type 'FILE' only support `defaults` as local files \
                              using a file URI or as namespace files using a nsfile URI";
    let Ok(rendered) = crate::runners::env::render_str(
        defaults.source(),
        &crate::runners::env::RenderContext::new(),
    ) else {
        return Some(NOT_IMMEDIATE);
    };
    match java_uri_scheme(&rendered) {
        Err(()) => Some(NOT_IMMEDIATE),
        Ok(Some(scheme)) if scheme == "file" || scheme == "nsfile" => None,
        Ok(_) => Some(BAD_SCHEME),
    }
}

/// The scheme `java.net.URI.create(text)` would give: `Err` when Java throws
/// (a character RFC 2396 forbids, a `%` not followed by two hex digits, or an
/// empty scheme-specific part after the scheme), `Ok(None)` for a relative
/// reference. An approximation of Java's parser: the characters it refuses
/// and the scheme rule (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` before
/// the first `:`, which must come before any `/`, `?` or `#`).
fn java_uri_scheme(text: &str) -> Result<Option<&str>, ()> {
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        let forbidden = c.is_ascii_control()
            || matches!(
                c,
                ' ' | '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}'
            );
        let bad_escape = c == '%'
            && !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit));
        if forbidden || bad_escape {
            return Err(());
        }
    }
    let Some(colon) = text.find(':') else {
        return Ok(None);
    };
    let head = &text[..colon];
    if head.contains(['/', '?', '#']) {
        return Ok(None);
    }
    let mut chars = head.chars();
    let scheme_like = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_like || colon + 1 == text.len() {
        // `:x` / `1a:b` and `file:` all throw in Java.
        return Err(());
    }
    Ok(Some(head))
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
/// walk Step 7's mutation proves is load-bearing -- and, since
/// no-silent-stall Task 1, through `Task::error_children()` as well, so every
/// check here (the refusals of tasks egret cannot run, the duplicate-id
/// record, the semantic checks) reaches a flowable's own `errors:`, which
/// `build_plan` flattens.
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
            // no-silent-stall Task 1 (spec §3). A bound `build_plan` cannot
            // read without a `RunContext` plans the task an opaque runnable
            // (`executor::loop_until_gap`, the one predicate both ask), which
            // no handler runs: every execution of it stalled in `RUNNING`
            // (`NotALeaf`, reaped, reclaimed). So it is an ERROR, at the
            // field's path, as `unsupported_pause_property` is.
            if let Some(field) = crate::executor::loop_until_gap(l) {
                report.errors.push(Finding {
                    code: "unsupported_loop_until_property",
                    message: format!(
                        "task '{}' is a loopUntil whose '{field}' is not a literal, which egret \
                         does not support: egret reads it when the flow is planned, before \
                         anything is rendered, so it cannot run this task and the flow cannot \
                         be published or started. '{field}' must be a literal value. Roadmap package #9 \
                         lifts it",
                        task.id
                    ),
                    path: format!("{path}.{field}"),
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
        // enough. Publish then encoded this pair into one
        // string as `"{namespace}/{flowId}"`, and a `namespace` (or
        // `flowId`) containing its OWN `/` decodes back into a DIFFERENT
        // pair than the author wrote -- `namespace: "a/b", flowId: "c"`
        // encoded to `"a/b/c"` and decoded, via `split_once('/')`, as
        // `("a", "b/c")`. The character checks below close that: they
        // reject anything that could never have been created as a real
        // flow's `namespace`/`id` in the first place, `/` included, so the
        // encoding's separator can never collide with a value it carries.
        // Debt §5.1 (2026-10-19): publish no longer encodes the pair (it is
        // looked up as two columns, `FlowRepo::find_in_workspace`); the
        // checks stay because they are Kestra's own grammar, and the
        // messages say that.
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
                         and be 1-150 characters long -- Kestra's own namespace pattern, the \
                         same grammar `namespace:` itself uses",
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
                         own flow id pattern (`AbstractFlow.java`'s `id` field)",
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
            // Block-level `retry:` is a non-goal (spec §2): the image retries the
            // failing child, and egret has no measured retry for the block. It is
            // refused, not run without its retry, so the flow cannot stall or
            // silently lose the property.
            if task.retry.is_some() {
                report.errors.push(Finding {
                    code: "unsupported_allow_failure_retry",
                    message: format!(
                        "task '{}' has type '{}': a retry on an AllowFailure block is not \
                         supported; put `retry:` on the child task that should be retried. Roadmap \
                         package #9 lifts it",
                        task.id, task.r#type
                    ),
                    path: format!("{path}.retry"),
                });
            }
        }
        // W8 Task 3. `tasks:` is NOT required -- `valids/pause-delay` writes
        // none and the artifact published it with a 200. A `Pause` whose
        // `behavior:`, `onPause:`, `tasks:` or `errors:` this executor does not
        // reproduce (`executor::pause_gap`, the single source of which four) is
        // an ERROR -- the user's ruling of 2026-10-11. It was a
        // `not_yet_enforced` warning until then, which let the flow publish and
        // start, and every execution of it stalled in `RUNNING` for ever:
        // `build_plan` planned the task an opaque runnable, the worker claimed
        // it and failed with `NotALeaf` before recording an outcome, and the
        // reaper released the claim on every working timeout. As an error it
        // is in `validate_for_run`, so publish refuses it and so does every
        // start path (`build_plan`'s third gate, inside
        // `worker::start_execution_in_on` and the `Subflow` target's plan).
        TaskKind::Pause(p) => {
            if let Some(property) = crate::executor::pause_gap(p) {
                // D14: Kestra's Pause has no `tasks` (`Pause.java:158`), so no
                // package lifts it; the others are #9's (`keys::lift_of`).
                let message = if property == "tasks" {
                    format!(
                        "task '{}' is a pause with 'tasks', which Kestra's Pause does not \
                         have: it is an unknown property, refused on egret as Kestra refuses \
                         it. Put the tasks after the Pause instead",
                        task.id
                    )
                } else {
                    format!(
                        "task '{}' is a pause with '{property}', which egret does not \
                         support: egret cannot run this task, so the flow cannot be \
                         published or started. Remove '{property}', or use a plain Pause \
                         (optionally bounded by pauseDuration or timeout). Roadmap package \
                         #9 lifts it",
                        task.id
                    )
                };
                report.errors.push(Finding {
                    code: "unsupported_pause_property",
                    message,
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
            // W9b Task 5 (spec §7, the user's ruling): the block is parsed by
            // the script tasks' own parse and kept; egret does not run
            // `WorkingDirectory`, so nothing about it changes at run.
            if let Some(v) = &w.namespace_files {
                if let Err((p, message)) =
                    crate::storage::nsfiles::stage::NamespaceFilesSpec::parse(v)
                {
                    let at = if p.is_empty() {
                        "namespaceFiles".to_string()
                    } else {
                        format!("namespaceFiles.{p}")
                    };
                    report.errors.push(Finding {
                        code: "working_directory_namespace_files_invalid",
                        message: format!("task '{}': '{at}' {message}", task.id),
                        path: format!("{path}.{at}"),
                    });
                }
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
            // deliberate.** The `Leaf` arm below is fail-CLOSED, reporting
            // anything spelled like `io.kestra.plugin.core.flow.*` that is not
            // a registered type -- an error, `unsupported_flow_task`, since
            // no-silent-stall Task 1, because no handler could run it. This
            // check is about something else, being a CHILD of a
            // `WorkingDirectory`, and the same fail-closed spelling test would refuse
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
                             workingDirectory, and '{}' has type '{}', which is {}",
                            task.id,
                            child.id,
                            child.r#type,
                            if registry::executor_level(&child.r#type) {
                                "a task the executor runs on the execution itself"
                            } else {
                                "a control-flow task"
                            }
                        ),
                        path: format!("{path}.tasks.{}", child.id),
                    });
                }
            }
            // The block runs (spec §4.2), but its properties run in steps: each
            // is refused by name until its claim runs it (spec §8, no silent
            // drop). `namespaceFiles` and `inputFiles` run in the prepare claim
            // (spec §5.1), so neither is refused. Task 7 removes each remaining
            // refusal when its property runs.
            // `outputFiles` is collected by the finish claim (spec §5.5), and each
            // name must be one `output_problem` accepts; a name it refuses fails the
            // publish here, not the run.
            if let Some(declared) = &w.output_files {
                let names: Option<Vec<&str>> = match declared {
                    serde_json::Value::Array(items) => items.iter().map(|v| v.as_str()).collect(),
                    _ => None,
                };
                match names {
                    None => report.errors.push(Finding {
                        code: "working_directory_output_files_invalid",
                        message: format!(
                            "task '{}': 'outputFiles' is a list of file names",
                            task.id
                        ),
                        path: format!("{path}.outputFiles"),
                    }),
                    Some(names) => {
                        for name in names {
                            if let Some(problem) = crate::worker::commands::output_problem(name) {
                                report.errors.push(Finding {
                                    code: "working_directory_output_files_invalid",
                                    message: format!("task '{}': {problem}", task.id),
                                    path: format!("{path}.outputFiles"),
                                });
                            }
                        }
                    }
                }
            }
            // `cache` is kept by the prepare and finish claims (spec §5.4); a cache
            // egret cannot keep is refused here, by name.
            if let Some(v) = &w.cache {
                if let Err(problem) = crate::worker::working_dir::parse_cache(v) {
                    report.errors.push(Finding {
                        code: "working_directory_cache_invalid",
                        message: format!("task '{}': {problem}", task.id),
                        path: format!("{path}.cache"),
                    });
                }
            }
        }
        // (Wherever this comment says "warns", read "is refused": both
        // findings below are ERRORS since no-silent-stall Task 1, spec §3.)
        //
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
        //      with its own registered leaf types) is the ERROR
        //      `unsupported_flow_task` -- a table hit names the package; a
        //      miss is still refused, just without naming one, so the next
        //      class Kestra adds is refused the moment it's written, not the
        //      moment someone adds a row.
        //   2. Anything else that is neither a `TaskKind` variant nor
        //      `registry::known_type(...)` is the ERROR `unknown_task_type`
        //      -- this is what catches a typo like `egret.sqll`.
        // Both were warnings until no-silent-stall Task 1 made them errors,
        // so publish and every start refuse them.
        // `registry::known_type` is checked FIRST, so `core.log` (and until
        // package #4 Task C6 removed their rows, `core.http`/`core.slack`;
        // see `registry::REMOVED_TYPES`) -- which share the `core.` alias spelling with real
        // Kestra control-flow aliases -- never reach either error: the
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
            if let Some(hint) = registry::removed_type(&task.r#type) {
                // Package #4 Task C6 (2026-10-28, spec D10): a type egret
                // registered once and removed. Checked before the `core.`
                // control-flow arm, which would otherwise call `core.http`
                // and `core.slack` Kestra control-flow tasks.
                report.errors.push(Finding {
                    code: "unknown_task_type",
                    message: format!(
                        "task '{}' has type '{}', which egret no longer recognises: the type \
                         was removed because egret never ran it; {hint}",
                        task.id, task.r#type
                    ),
                    path: path.clone(),
                });
            } else if canonical_flow_type(&task.r#type).as_deref() == Some("foreach") {
                // **`ForEach` is an ERROR, the user's ruling of 2026-10-12**
                // (W7b Task 7b). It is not a class in the pinned checkout --
                // Kestra's name for it is `Loop` (`models::task`'s
                // `foreach_is_really_named_loop_in_this_checkout`) -- and Kestra
                // 2.0 refuses it with a 422. Under the generic warning below it
                // published as a draft and every start path ran it: planned an
                // opaque runnable no handler serves, so its body never ran. As
                // an error it is in `validate_for_run`, so `build_plan` refuses
                // the flow and no start path can run it.
                report.errors.push(Finding {
                    code: "unsupported_foreach",
                    message: format!(
                        "task '{}' has type '{}', which egret does not run: ForEach is not a \
                         Kestra 2.0 task type, so the flow cannot be published or started. Use \
                         io.kestra.plugin.core.flow.Loop (alias core.loop), which takes the \
                         same values and tasks",
                        task.id, task.r#type
                    ),
                    path: path.clone(),
                });
            } else if canonical_flow_type(&task.r#type).is_some() {
                // **No `KESTRA_FLOW_CONTROL` lookup, and the arm that did one
                // is GONE.** W8 Task 3 fix round 1. It named the package that
                // would port an `Unported` row, and since W8 Task 3 ported
                // `pause` — the last one — every row is ported, so that arm
                // could not be reached and the `Unported` variant could not be
                // constructed.
                //
                // The FAIL-CLOSED design it existed inside is untouched and is
                // the half that matters: anything spelled like Kestra control
                // flow that reaches `Leaf` and is not a registered type is
                // reported, whether or not a table has a row for it. That test
                // is `canonical_flow_type(...).is_some()`, right here, and it
                // never read the table.
                //
                // **An ERROR since no-silent-stall Task 1** (spec §3), where it
                // was the task-level `not_yet_enforced` warning: no handler
                // serves such a type, so a draft that started stalled in
                // `RUNNING` with `NoHandler`. The flow-level `not_yet_enforced`
                // warnings (`emit_not_yet_enforced`) are a different thing and
                // stay warnings.
                report.errors.push(Finding {
                    code: "unsupported_flow_task",
                    message: format!(
                        "task '{}' has type '{}', which is spelled like a Kestra control-flow \
                         task (a `io.kestra.plugin.core.flow.*` FQCN or its `core.` alias) but \
                         egret does not implement it: egret cannot run this task, so the flow \
                         cannot be published or started",
                        task.id, task.r#type
                    ),
                    path: path.clone(),
                });
            } else {
                // An ERROR since no-silent-stall Task 1 (spec §3): no handler
                // serves an unknown type, so a draft that started stalled in
                // `RUNNING` with `NoHandler`. Publish already refused it.
                report.errors.push(Finding {
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

    // no-silent-stall Task 1 (spec §3): `error_children()` too, which
    // `build_plan` flattens (a `Sequential`/`Parallel`/`If`/`Switch`/`Dag`'s
    // own `errors:`), so a construct this walk refuses cannot slip in under
    // one. Every check of the walk reaches those tasks now, the duplicate-id
    // one included: an `errors:` handler's id joins `seen_ids`, as it is in
    // Kestra's `allTasks()` (`AbstractBranch.allChildTasks()` is `tasks +
    // errors + finally`). It is also what keeps `worker::claim::leaf`
    // unambiguous: that lookup finds a task BY ID through `children()` and
    // `error_children()`, first match wins, so two tasks sharing an id there
    // would hand a worker whichever came first.
    for child in task.children().into_iter().chain(task.error_children()) {
        let child_path = super::path::task_path(&path, &child.id);
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
/// `is_valid_flow_id`: Kestra's own. (Introduced because publish then
/// encoded the pair into one `"{namespace}/{flowId}"` string, where a `/`
/// smuggled into either half would decode back into a different pair; since
/// debt §5.1, 2026-10-19, publish looks the pair up as two columns.) No `self_dependency` check to match `Subflow`'s
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
        let (_, lift) = NOT_YET_ENFORCED
            .iter()
            .find(|(n, _)| *n == name)
            .expect("every feature name here has a NOT_YET_ENFORCED entry");
        report.warnings.push(Finding {
            code: "not_yet_enforced",
            message: format!(
                "`{name}` is declared in this flow but egret does not enforce it yet; {lift}"
            ),
            path: name.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::NOT_YET_ENFORCED;
    use crate::spec::{parse, validate, validate_for_run};

    fn walk_rs(dir: &str) -> Vec<String> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).expect("a source directory") {
            let path = entry.expect("an entry").path();
            let name = path.to_string_lossy().into_owned();
            if path.is_dir() {
                out.extend(walk_rs(&name));
            } else if name.ends_with(".rs") {
                out.push(name);
            }
        }
        out
    }

    /// **No user-visible text names a retired package** (spec D4).
    #[test]
    fn no_message_names_a_w_package_or_a_later_package() {
        const STALE: &[&str] = &["(W5a)", "(W5b)", "lands in W", "a later package"];
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut offenders = Vec::new();
        for path in walk_rs(root) {
            let text = std::fs::read_to_string(&path).unwrap();
            let prod = text.split("#[cfg(test)]").next().unwrap();
            for (n, line) in prod.lines().enumerate() {
                let code = line.trim_start();
                // Doc comments and `//` lines are history; only text a user can
                // receive is checked, which is what sits inside a string literal.
                if code.starts_with("//") {
                    continue;
                }
                // Any other line is checked whole, up to a trailing `//`: a
                // literal's last line (`... (W5a); only the name"`) has one
                // quote only, so splitting on quotes would miss it.
                let code = code.split(" // ").next().unwrap_or(code);
                let stale = STALE.iter().any(|s| code.contains(s));
                if stale {
                    offenders.push(format!("{path}:{}: {code}", n + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }

    /// **No production caller reports on a saved flow without the key census**
    /// (spec D11). `validate(&Flow)` stays public for tests and for
    /// `validate_source` itself; a production call to it outside `spec/` would
    /// publish a flow whose unknown keys nobody checked.
    #[test]
    fn every_production_save_path_calls_validate_source() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut offenders = Vec::new();
        for path in walk_rs(root) {
            if path.contains("/spec/") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let prod = text.split("#[cfg(test)]").next().unwrap();
            // `spec`'s `validate` reaches a file either by path or by a `use`;
            // other functions named `validate` (`runners/javaformat.rs`,
            // `worker/k8s/pod_spec.rs`) are not it.
            let imported = prod.lines().any(|l| {
                l.trim_start().starts_with("use crate::spec::")
                    && l.contains("validate")
                    && !l.contains("validate_source")
            });
            for (n, line) in prod.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") || code.contains("fn validate") {
                    continue;
                }
                let by_path =
                    code.contains("spec::validate(&") || code.contains("validate::validate(&");
                let by_use =
                    imported && (code.contains(" validate(&") || code.contains("(validate(&"));
                if by_path || by_use {
                    offenders.push(format!("{path}:{}: {code}", n + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }

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
             - id: inner\n            type: core.log\n",
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

    /// Kill-and-exit Task 3 fix round 1: an `Exit` child of a
    /// `WorkingDirectory` is refused, as Kestra refuses it (only a
    /// `RunnableTask` may be a block child), and a `Log` beside it is not.
    #[test]
    fn an_exit_child_of_a_working_directory_is_refused() {
        let flow = |ty: &str, extra: &str| {
            parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: wd\n    type: core.workingdirectory\n    \
                 tasks:\n      - id: child\n        type: {ty}\n{extra}"
            ))
            .unwrap_or_else(|e| panic!("{ty}: {e:?}"))
        };
        let refused = |f: &crate::models::Flow| {
            validate(f)
                .errors
                .into_iter()
                .find(|e| e.code == "working_directory_child_not_runnable")
        };
        let exit = flow(
            "io.kestra.plugin.core.execution.Exit",
            "        state: KILLED\n",
        );
        let finding = refused(&exit).expect("an Exit child is refused");
        assert_eq!(finding.path, "tasks.wd.tasks.child", "{finding:?}");
        let log = flow("io.kestra.plugin.core.log.Log", "        message: hi\n");
        assert!(
            refused(&log).is_none(),
            "the control: a Log child is runnable"
        );
    }

    /// Group C2 Task 5 (decision D4): `SetVariables`, `UnsetVariables` and
    /// `Labels` are `ExecutionUpdatableTask`s in Kestra, as `Exit` is, so a
    /// `WorkingDirectory` refuses each as a child. egret refuses them through
    /// `EXECUTOR_LEVEL`; one assertion per FQCN, and the `Log` control.
    #[test]
    fn the_three_update_types_are_refused_inside_a_working_directory() {
        // Arrange
        let flow = |ty: &str, extra: &str| {
            parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: wd\n    type: core.workingdirectory\n    \
                 tasks:\n      - id: child\n        type: {ty}\n{extra}"
            ))
            .unwrap_or_else(|e| panic!("{ty}: {e:?}"))
        };
        let refused = |f: &crate::models::Flow| {
            validate(f)
                .errors
                .into_iter()
                .find(|e| e.code == "working_directory_child_not_runnable")
        };
        let set = flow(
            "io.kestra.plugin.core.execution.SetVariables",
            "        variables:\n          a: 1\n",
        );
        let unset = flow(
            "io.kestra.plugin.core.execution.UnsetVariables",
            "        variables:\n          - a\n",
        );
        let labels = flow(
            "io.kestra.plugin.core.execution.Labels",
            "        labels:\n          k: v\n",
        );
        let log = flow("io.kestra.plugin.core.log.Log", "        message: hi\n");

        // Act + Assert
        let finding = refused(&set).expect("a SetVariables child is refused");
        assert_eq!(finding.path, "tasks.wd.tasks.child", "{finding:?}");
        let finding = refused(&unset).expect("an UnsetVariables child is refused");
        assert_eq!(finding.path, "tasks.wd.tasks.child", "{finding:?}");
        let finding = refused(&labels).expect("a Labels child is refused");
        assert_eq!(finding.path, "tasks.wd.tasks.child", "{finding:?}");
        assert!(
            refused(&log).is_none(),
            "the control: a Log child is runnable"
        );
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

    /// KI-122: what the scheduler would refuse is a warning at save, with the
    /// scheduler's own words, and the flow still validates without an error.
    #[test]
    fn a_schedule_the_scheduler_would_refuse_is_a_warning_at_save() {
        let schedule = |body: &str| {
            parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n\
                 triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n{body}"
            ))
            .unwrap()
        };
        let codes = |r: &super::ValidationReport| -> Vec<(&'static str, String)> {
            r.warnings
                .iter()
                .filter(|w| w.code.starts_with("schedule_"))
                .map(|w| (w.code, w.path.clone()))
                .collect()
        };

        let bad_cron = validate(&schedule("    cron: \"61 * * * *\"\n"));
        assert!(bad_cron.errors.is_empty(), "{bad_cron:?}");
        assert_eq!(
            codes(&bad_cron),
            vec![("schedule_not_resolvable", "triggers.s".to_string())]
        );
        // D10 + spec §5: the save-time message for a `when` / `lateMaximumDelay`
        // names the key and the package and carries none of the author's value.
        let sentinel = "SENTINEL-value-1d2f";
        for (key, line, want) in [
            (
                "when",
                format!("    when: \"{{{{ '{sentinel}' }}}}\"\n"),
                "trigger 's': core.schedule `when` is not supported: egret refuses it when it \
                 syncs the trigger, so this Schedule trigger will not fire. Roadmap package \
                 #11 lifts it",
            ),
            (
                "lateMaximumDelay",
                format!("    lateMaximumDelay: {sentinel}\n"),
                "trigger 's': core.schedule `lateMaximumDelay` is not supported: egret \
                 refuses it when it syncs the trigger, so this Schedule trigger will not \
                 fire. Roadmap package #11 lifts it",
            ),
        ] {
            let r = validate(&schedule(&format!("    cron: \"0 * * * *\"\n{line}")));
            let m = &r
                .warnings
                .iter()
                .find(|w| w.path == format!("triggers.s.{key}"))
                .unwrap_or_else(|| panic!("{key}: {r:?}"))
                .message;
            assert_eq!(m, want, "{key}");
            assert!(
                !m.contains(sentinel),
                "{key}: the author's value leaked: {m}"
            );
        }
        let w = &bad_cron
            .warnings
            .iter()
            .find(|w| w.code == "schedule_not_resolvable")
            .unwrap()
            .message;
        assert!(w.contains("invalid cron"), "{w}");

        let bad_zone = validate(&schedule(
            "    cron: \"0 * * * *\"\n    timezone: Mars/Olympus\n",
        ));
        assert_eq!(
            codes(&bad_zone),
            vec![("schedule_not_resolvable", "triggers.s".to_string())]
        );

        let templated = validate(&schedule(
            "    cron: \"0 * * * *\"\n    inputs:\n      day: \"{{ trigger.date }}\"\n      plain: x\n",
        ));
        assert!(templated.errors.is_empty(), "{templated:?}");
        assert_eq!(
            codes(&templated),
            vec![(
                "schedule_input_templated",
                "triggers.s.inputs.day".to_string()
            )]
        );

        // Control: a schedule the scheduler takes says nothing.
        let fine = validate(&schedule(
            "    cron: \"0 * * * *\"\n    inputs:\n      plain: x\n",
        ));
        assert!(codes(&fine).is_empty(), "{fine:?}");
    }

    /// kestra: `FlowValidator.findMissingInputsForScheduleTriggers` --
    /// `invalids/scheduled-missing-inputs-defaults{,-2}`.
    #[test]
    fn a_schedule_that_cannot_supply_an_input_without_a_default_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user\n    type: STRING\n    defaults: John\n  \
             - id: user2\n    type: STRING\ntasks:\n  - id: t\n    type: core.log\ntriggers:\n  \
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
             - id: t\n    type: core.log\ntriggers:\n  - id: s\n    \
             type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    \
             inputs:\n      user2: bob\n",
        )
        .unwrap();
        // 2. every input has a default.
        let defaulted = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user2\n    type: STRING\n    \
             defaults: bob\ntasks:\n  - id: t\n    type: core.log\ntriggers:\n  - id: s\n    \
             type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n",
        )
        .unwrap();
        // 3. the trigger is not a Schedule. Kestra's filter is
        //    `Schedule.class::isInstance`; a webhook carries its inputs at
        //    fire time.
        let webhook = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: user2\n    type: STRING\ntasks:\n  \
             - id: t\n    type: core.log\ntriggers:\n  - id: w\n    \
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
                     itemType: {item}\ntasks:\n  - id: t\n    type: core.log\n"
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
                 allowedFileExtensions: {list}\ntasks:\n  - id: t\n    type: core.log\n"
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
                 itemType: {item}\ntasks:\n  - id: t\n    type: core.log\n"
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

    /// W7e: a trigger id declared twice is a SAVE-only finding. It must not
    /// appear in `validate_for_run`, or a stored flow that declares one would
    /// stop running.
    #[test]
    fn duplicate_trigger_id_is_save_only() {
        let dup = "id: f\nnamespace: a.b\ntasks:\n  - id: x\n    type: core.log\ntriggers:\n  \
                   - id: t\n    type: core.schedule\n    cron: \"* * * * *\"\n  - id: t\n    \
                   type: core.webhook\n    key: k\n";
        let f = parse(dup).unwrap();
        let r = validate(&f);
        let hits: Vec<_> = r
            .errors
            .iter()
            .filter(|e| e.code == "duplicate_trigger_id")
            .collect();
        assert_eq!(hits.len(), 1, "{r:?}");
        assert_eq!(hits[0].path, "triggers.t");
        let run = validate_for_run(&f);
        assert!(
            !run.errors.iter().any(|e| e.code == "duplicate_trigger_id"),
            "{run:?}"
        );
        let distinct = dup.replacen(
            "id: t\n    type: core.webhook",
            "id: u\n    type: core.webhook",
            1,
        );
        let r = validate(&parse(&distinct).unwrap());
        assert!(
            !r.errors.iter().any(|e| e.code == "duplicate_trigger_id"),
            "{r:?}"
        );
    }

    #[test]
    fn duplicate_task_ids_are_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n  - id: t\n    type: y\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n  - id: outer\n    type: core.parallel\n    tasks:\n      - id: t\n        type: y\n",
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
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n"
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
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n"
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
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n"
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
                "id: {id}\nnamespace: a\ntasks:\n  - id: t\n    type: core.log\n"
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
                "id: {id}\nnamespace: a\ntasks:\n  - id: t\n    type: core.log\n"
            ))
            .unwrap();
            assert!(!validate(&f).errors.iter().any(|e| e.path == "id"), "{id}");
        }
    }

    /// W9c: the save-time refusal of a `_kv` namespace segment; its run-time
    /// twin is `nsfiles::access`'s
    /// `a_kv_segment_namespace_is_refused_for_every_resource_access_and_actor`.
    #[test]
    fn publish_refuses_a_kv_segment_namespace_by_name() {
        for ns in ["a._kv", "a._kv.b", "x.y._kv", "_kv.a"] {
            let f = parse(&format!(
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n"
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
            assert_eq!(found[0].code, "namespace_kv_segment");
            assert!(
                found[0].message.contains(&format!("'{ns}'")) && found[0].message.contains("'_kv'"),
                "{found:?}"
            );
        }
        // Both markers: the `_files` finding alone, named once.
        let f = parse("id: f\nnamespace: a._files._kv\ntasks:\n  - id: t\n    type: core.log\n")
            .unwrap();
        let found: Vec<_> = validate(&f)
            .errors
            .into_iter()
            .filter(|e| e.path == "namespace")
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, "namespace_files_segment");
        // The controls: `_kv` inside a segment is not the segment.
        for ns in ["a.kv", "a.x_kv", "a._kvx", "a.b_kv.c", "kv"] {
            let f = parse(&format!(
                "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n"
            ))
            .unwrap();
            assert!(
                !validate(&f).errors.iter().any(|e| e.path == "namespace"),
                "{ns}"
            );
        }
    }

    /// W9c: a flow id `_kv` is refused at publish by Kestra's own id pattern
    /// (no leading `_`), as `_files` is (`a_flow_id_of_files_is_refused`), so
    /// `{ns}/_kv/…` is never a flow's execution prefix.
    #[test]
    fn a_flow_id_of_kv_is_refused() {
        for id in ["_kv", "_KV", "_kv.x"] {
            let f = parse(&format!(
                "id: {id}\nnamespace: a\ntasks:\n  - id: t\n    type: core.log\n"
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
        for id in ["kv", "x_kv"] {
            let f = parse(&format!(
                "id: {id}\nnamespace: a\ntasks:\n  - id: t\n    type: core.log\n"
            ))
            .unwrap();
            assert!(!validate(&f).errors.iter().any(|e| e.path == "id"), "{id}");
        }
    }

    /// W9c: the corpus's KV flow (`oracle-exec/corpus/valids/kv.yaml`, an
    /// over-refusal guard) still validates with no error finding: the `_kv`
    /// rule is a namespace SEGMENT rule, not a word rule.
    #[test]
    fn valids_kv_still_publishes() {
        let yaml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/oracle-exec/corpus/valids/kv.yaml"
        ))
        .expect("the corpus flow");
        let f = parse(&yaml).unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn a_bad_namespace_is_an_error() {
        let f =
            parse("id: f\nnamespace: Not-Valid\ntasks:\n  - id: t\n    type: core.log\n").unwrap();
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
            "id: f\nnamespace: a.b\nconcurrency:\n  limit: 1\ntasks:\n  - id: t\n    type: core.log\n",
        )
        .unwrap();
        let r = validate(&f);
        let w = r
            .warnings
            .iter()
            .find(|w| w.code == "not_yet_enforced")
            .unwrap_or_else(|| panic!("no not_yet_enforced warning: {r:?}"));
        assert!(w.message.contains("concurrency"), "{}", w.message);
        assert_eq!(
            w.message,
            "`concurrency` is declared in this flow but egret does not enforce it yet; \
             roadmap package #8 lifts it",
            "the warning must name the roadmap package"
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
                ("concurrency", "roadmap package #8 lifts it"),
                ("sla", "roadmap package #16 lifts it"),
                ("checks", "roadmap package #16 lifts it"),
                (
                    "quotas",
                    "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap \u{a7}3.3)"
                ),
                (
                    "triggers",
                    "egret fires only Schedule, Webhook and Flow triggers; roadmap package #11 \
                     lifts ScheduleOnDates and http.Trigger, #18 lifts plugin triggers, and \
                     McpToolTrigger is not engine scope (roadmap \u{a7}3.3)"
                ),
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
             tasks:\n  - id: t\n    type: core.log\n    timeout: PT1S\n    retry:\n\
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
             \x20 - id: t\n    type: core.log\n    timeout: PT1S\n",
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

    /// **The `triggers` row narrowed** -- W7a's final review. The three
    /// trigger kinds egret fires must not warn, one flow per kind so no single
    /// kind can hide behind another; a trigger of any other type still warns.
    #[test]
    fn only_a_trigger_egret_does_not_fire_warns_not_yet_enforced() {
        let triggers_warning = |yaml: &str| {
            let r = validate(&parse(yaml).unwrap());
            r.warnings
                .iter()
                .filter(|w| w.code == "not_yet_enforced" && w.path == "triggers")
                .map(|w| w.message.clone())
                .collect::<Vec<_>>()
        };
        for kind in [
            "  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 * * * *\"\n",
            "  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: k1234567\n",
            "  - id: f\n    type: io.kestra.plugin.core.trigger.Flow\n    conditions: []\n",
            "  - id: s\n    type: core.schedule\n    cron: \"0 * * * *\"\n",
        ] {
            let yaml = format!("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\ntriggers:\n{kind}");
            assert_eq!(
                triggers_warning(&yaml),
                Vec::<String>::new(),
                "egret fires this trigger, so `not yet enforced` would be false:\n{yaml}"
            );
        }

        // POSITIVE CONTROL: a trigger egret does not fire, alone and beside a Schedule.
        let leaf = "  - id: q\n    type: io.kestra.plugin.aws.sqs.Trigger\n    queueUrl: https://example\n";
        for extra in [
            "",
            "  - id: s\n    type: core.schedule\n    cron: \"0 * * * *\"\n",
        ] {
            let yaml = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\ntriggers:\n{leaf}{extra}"
            );
            let found = triggers_warning(&yaml);
            assert_eq!(found.len(), 1, "{yaml}");
            assert!(
                found[0].contains("egret fires only Schedule, Webhook and Flow triggers")
                    && !found[0].contains("W6"),
                "{found:?}"
            );
        }
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
             behavior: CANCEL\n    assert: \"{{ false }}\"\ntasks:\n  - id: t\n    type: core.log\n",
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
             - id: t\n    type: core.log\n",
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
             tasks:\n  - id: t\n    type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: core.log\n        dependsOn: [b]\n      - task:\n          id: b\n          type: core.log\n        dependsOn: [a]\n",
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

    /// Finding 1 (CRITICAL), **as the 2026-10-11 ruling leaves it.** W8 Task 3
    /// made a `Pause` run, bounded by `pauseDuration:`/`timeout:` or unbounded,
    /// and kept a `not_yet_enforced` WARNING for a `Pause` declaring one of the
    /// four properties `build_plan` does not reproduce. That warning let the
    /// flow publish and start, and the execution then stalled in `RUNNING`
    /// for ever: `pause_gap` planned the task an opaque runnable, no worker
    /// could run it (`WorkerError::NotALeaf`), and the reaper released the
    /// claim on every timeout.
    ///
    /// The ruling: such a `Pause` is a validation ERROR, so the editor shows
    /// it, publish refuses it and every start path refuses it (they all go
    /// through `build_plan`'s `validate_for_run` gate). Both directions are
    /// asserted, because only the pair can fail for the right reason: a plain
    /// pause (unbounded, `pauseDuration:`, `timeout:`) is valid and silent,
    /// and each of the four is an error that names the property and says what
    /// to do. `executor::pause_gap` stays the one predicate both sides ask.
    #[test]
    fn a_pause_with_a_property_egret_does_not_run_is_an_error_and_a_plain_one_is_valid() {
        for spelling in ["io.kestra.plugin.core.flow.Pause", "core.pause"] {
            // Valid and silent: the shapes this executor reproduces.
            for (label, props) in [
                ("unbounded", ""),
                ("pauseDuration", "    pauseDuration: PT1S\n"),
                ("timeout", "    timeout: PT1S\n"),
            ] {
                let yaml = format!(
                    "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: {spelling}\n{props}"
                );
                let r = validate(&parse(&yaml).unwrap());
                assert!(r.errors.is_empty(), "{spelling}/{label}: {r:?}");
                assert!(
                    !r.warnings.iter().any(|w| w.path.starts_with("tasks.gate")),
                    "{spelling}/{label}: a pause egret runs must not be reported: {r:?}"
                );
            }

            // Refused: the four it does not.
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
                // property of `gate` rather than a second mapping key.
                let yaml = format!(
                    "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: {spelling}\n    pauseDuration: PT1S\n{props}"
                );
                let f = parse(&yaml).unwrap_or_else(|e| panic!("{spelling}/{property}: {e:?}"));
                let r = validate(&f);
                let e = r
                    .errors
                    .iter()
                    .find(|e| e.code == "unsupported_pause_property")
                    .unwrap_or_else(|| {
                        panic!("{spelling}/{property}: no error for 'gate': {r:?}")
                    });
                assert_eq!(e.path, format!("tasks.gate.{property}"), "{spelling}/{property}");
                assert!(
                    e.message.contains(&format!("'{property}'")),
                    "{spelling}/{property}: the error must name the property: {}",
                    e.message
                );
                if property == "tasks" {
                    // D14: not a Kestra property, refused permanently, no package.
                    assert_eq!(
                        e.message,
                        "task 'gate' is a pause with 'tasks', which Kestra's Pause does not \
                         have: it is an unknown property, refused on egret as Kestra refuses \
                         it. Put the tasks after the Pause instead",
                        "{spelling}/{property}"
                    );
                    assert!(!e.message.contains("Roadmap package"), "{}", e.message);
                } else {
                    // What to do: remove it, or use a plain pause; and who lifts it.
                    assert!(
                        e.message.contains(&format!("Remove '{property}'")),
                        "{spelling}/{property}: {}",
                        e.message
                    );
                    assert!(
                        e.message.ends_with(
                            "or use a plain Pause (optionally bounded by pauseDuration or \
                             timeout). Roadmap package #9 lifts it"
                        ),
                        "{spelling}/{property}: {}",
                        e.message
                    );
                }
                // The stalling-run sentence of the old warning is gone: the
                // flow can no longer reach a run, so it would be false.
                assert!(!e.message.contains("stalls"), "{spelling}/{property}: {}", e.message);
                assert!(
                    !r.warnings.iter().any(|w| w.path.starts_with("tasks.gate")),
                    "{spelling}/{property}: an error, not also a warning: {r:?}"
                );
                // The run-time gate refuses it too: it is not a save-only
                // finding, so no start path can run it.
                assert!(
                    validate_for_run(&f)
                        .errors
                        .iter()
                        .any(|e| e.code == "unsupported_pause_property"),
                    "{spelling}/{property}: validate_for_run must refuse it"
                );
            }
        }
    }

    /// The user's ruling of 2026-10-12 (W7b Task 7b). `ForEach` is not a type
    /// egret runs -- Kestra's own name for it in the pinned checkout is
    /// `Loop`, and Kestra 2.0 refuses `ForEach` with a 422 -- and until the
    /// ruling it drew only the generic `not_yet_enforced` warning, so only a
    /// saved draft could start (publish already refused it), and the run
    /// stalled with `NoHandler`, as measured in Task 7b: no handler serves
    /// it, and it did not end SUCCESS. It is an ERROR now, at the top level and nested inside a
    /// flowable, under every spelling `canonical_flow_type` folds to
    /// `foreach`, and the message names `Loop` and its alias. `Loop` itself,
    /// under both spellings, stays valid and silent: the control.
    #[test]
    fn foreach_is_an_error_that_names_loop_and_loop_stays_valid() {
        let foreach = |spelling: &str, indent: &str| {
            format!(
                "{indent}- id: fan\n{indent}  type: {spelling}\n{indent}  values: [1, 2]\n\
                 {indent}  tasks:\n{indent}    - id: body\n{indent}      type: core.log\n\
                 {indent}      message: hi\n"
            )
        };
        for spelling in [
            "io.kestra.plugin.core.flow.ForEach",
            "core.foreach",
            "io.kestra.plugin.core.flow.Foreach",
            "core.forEach",
        ] {
            for (label, yaml, path) in [
                (
                    "top-level",
                    format!("id: f\nnamespace: a.b\ntasks:\n{}", foreach(spelling, "  ")),
                    "tasks.fan",
                ),
                (
                    "nested in a Sequential",
                    format!(
                        "id: f\nnamespace: a.b\ntasks:\n  - id: seq\n    type: core.sequential\n    tasks:\n{}",
                        foreach(spelling, "      ")
                    ),
                    "tasks.seq.fan",
                ),
                (
                    "nested in a Loop body",
                    format!(
                        "id: f\nnamespace: a.b\ntasks:\n  - id: lp\n    type: core.loop\n    values: [1]\n    tasks:\n{}",
                        foreach(spelling, "      ")
                    ),
                    "tasks.lp.fan",
                ),
            ] {
                let f = parse(&yaml).unwrap_or_else(|e| panic!("{spelling}/{label}: {e:?}\n{yaml}"));
                let r = validate(&f);
                let e = r
                    .errors
                    .iter()
                    .find(|e| e.code == "unsupported_foreach")
                    .unwrap_or_else(|| panic!("{spelling}/{label}: no error: {r:?}"));
                assert_eq!(e.path, path, "{spelling}/{label}");
                assert!(e.message.contains(spelling), "{spelling}/{label}: {}", e.message);
                assert!(
                    e.message.contains("io.kestra.plugin.core.flow.Loop")
                        && e.message.contains("core.loop"),
                    "{spelling}/{label}: the error must name Loop and its alias: {}",
                    e.message
                );
                assert!(
                    !r.warnings.iter().any(|w| w.path == path),
                    "{spelling}/{label}: an error, not also a warning: {r:?}"
                );
                assert!(
                    validate_for_run(&f)
                        .errors
                        .iter()
                        .any(|e| e.code == "unsupported_foreach"),
                    "{spelling}/{label}: validate_for_run must refuse it"
                );
            }
        }
        // The control: `Loop` under both spellings is valid and silent.
        for spelling in ["io.kestra.plugin.core.flow.Loop", "core.loop"] {
            let yaml = format!("id: f\nnamespace: a.b\ntasks:\n{}", foreach(spelling, "  "));
            let r = validate(&parse(&yaml).unwrap());
            assert!(
                r.errors.is_empty() && r.warnings.is_empty(),
                "{spelling}: {r:?}"
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: core.log\n        dependsOn: [nope]\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: core.log\n      - task:\n          id: b\n          type: core.log\n        dependsOn: [a]\n",
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
            "id: f\nnamespace: a.b\ninputs:\n  - id: x\n    type: STRING\n  - id: x\n    type: STRING\ntasks:\n  - id: t\n    type: core.log\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "duplicate_input_id"),
            "{r:?}"
        );
    }

    /// Inputs package #3a: a flow with `inputs_yaml` under `inputs:`.
    fn with_inputs(inputs_yaml: &str) -> crate::models::Flow {
        parse(&format!(
            "id: f\nnamespace: a.b\ninputs:\n{inputs_yaml}tasks:\n  - id: t\n    type: core.log\n"
        ))
        .unwrap_or_else(|e| panic!("{inputs_yaml}: {e:?}"))
    }

    /// (code, message, path) of every error with `code`, in report order.
    fn errors_of(f: &crate::models::Flow, code: &str) -> Vec<(String, String)> {
        validate(f)
            .errors
            .into_iter()
            .filter(|e| e.code == code)
            .map(|e| (e.message, e.path))
            .collect()
    }

    /// kestra: FlowValidator.java:241-270. A `dependsOn` id no input has is
    /// refused at save, in Kestra's words, for each unknown id in order; and
    /// only at save -- the run gate (D6: ignored at run time) stays silent.
    #[test]
    fn depends_on_an_unknown_input_is_refused_at_save_only() {
        let f = with_inputs(
            "  - id: a\n    type: STRING\n  - id: b\n    type: STRING\n    dependsOn:\n      \
             inputs: [a, x, y]\n",
        );
        assert_eq!(
            errors_of(&f, "input_depends_on_unknown"),
            [
                (
                    "Input with id 'b' depends on a non-existent input 'x'.".to_string(),
                    "inputs.b.dependsOn.inputs".to_string()
                ),
                (
                    "Input with id 'b' depends on a non-existent input 'y'.".to_string(),
                    "inputs.b.dependsOn.inputs".to_string()
                ),
            ]
        );
        assert!(validate_for_run(&f).errors.is_empty(), "run gate");
        let ok = with_inputs(
            "  - id: a\n    type: STRING\n  - id: b\n    type: STRING\n    dependsOn:\n      \
             inputs: [a]\n",
        );
        assert!(validate(&ok).errors.is_empty(), "{:?}", validate(&ok));
    }

    /// kestra: `CycleDependency.findCycle`. Each input on the cycle reports it
    /// from its own start, as Java's `List.toString`; a self-dependency is a
    /// cycle of one; an input that only leads into a cycle reports it too.
    #[test]
    fn a_depends_on_cycle_is_refused_with_kestras_path() {
        let f = with_inputs(
            "  - id: a\n    type: STRING\n    dependsOn:\n      inputs: [b]\n\
             \x20 - id: b\n    type: STRING\n    dependsOn:\n      inputs: [c]\n\
             \x20 - id: c\n    type: STRING\n    dependsOn:\n      inputs: [b]\n\
             \x20 - id: s\n    type: STRING\n    dependsOn:\n      inputs: [s]\n",
        );
        let got: Vec<String> = errors_of(&f, "input_depends_on_cycle")
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        assert_eq!(
            got,
            [
                "Cycle dependency detected for input with id 'a': [b, c]",
                "Cycle dependency detected for input with id 'b': [b, c]",
                "Cycle dependency detected for input with id 'c': [c, b]",
                "Cycle dependency detected for input with id 's': [s]",
            ]
        );
        assert_eq!(
            errors_of(&f, "input_depends_on_cycle")[0].1,
            "inputs.a.dependsOn"
        );
        assert!(validate_for_run(&f).errors.is_empty(), "run gate");
        // A diamond is not a cycle.
        let ok = with_inputs(
            "  - id: a\n    type: STRING\n\
             \x20 - id: b\n    type: STRING\n    dependsOn:\n      inputs: [a]\n\
             \x20 - id: c\n    type: STRING\n    dependsOn:\n      inputs: [a]\n\
             \x20 - id: d\n    type: STRING\n    dependsOn:\n      inputs: [b, c]\n",
        );
        assert!(errors_of(&ok, "input_depends_on_cycle").is_empty());
    }

    /// kestra: InputValidator.java:53-59, on any input type.
    #[test]
    fn defaults_with_prefill_is_refused() {
        let f = with_inputs(
            "  - id: n\n    type: INT\n    defaults: 1\n    prefill: 2\n\
             \x20 - id: d\n    type: STRING\n    defaults: x\n\
             \x20 - id: p\n    type: STRING\n    required: false\n    prefill: y\n",
        );
        assert_eq!(
            errors_of(&f, "input_defaults_with_prefill"),
            [(
                "input 'n': Inputs with a default value cannot also have a prefill.".to_string(),
                "inputs.n.prefill".to_string()
            )]
        );
        assert!(validate_for_run(&f).errors.is_empty(), "run gate");
    }

    /// kestra: FileInputValidator.java:33-50. `file` and `nsfile` pass; any
    /// other scheme, and no scheme at all (`invalids/inputs-validation`'s
    /// `something`), is the scheme message; a value Java's URI parser throws
    /// on, or a default that does not render, is the other one.
    #[test]
    fn a_file_default_must_be_a_file_or_nsfile_uri() {
        const SCHEME: &str = "only support `defaults` as local files using a file URI or as \
                              namespace files using a nsfile URI";
        const RENDER: &str = "only support `defaults` with expression that can be rendered \
                              immediately";
        for (default, want) in [
            ("file:///tmp/a.csv", None),
            ("nsfile:///data/a.csv", None),
            ("'{{ \"nsfile:///a\" }}'", None),
            ("something", Some(SCHEME)),
            ("https://example.com/a.csv", Some(SCHEME)),
            ("s3://bucket/a", Some(SCHEME)),
            ("'file:///a b'", Some(RENDER)),
            ("'{{ inputs.nope }}'", Some(RENDER)),
        ] {
            let f = with_inputs(&format!(
                "  - id: up\n    type: FILE\n    defaults: {default}\n"
            ));
            let got = errors_of(&f, "input_file_defaults_invalid");
            match want {
                None => assert!(got.is_empty(), "{default}: {got:?}"),
                Some(w) => {
                    assert_eq!(got.len(), 1, "{default}: {got:?}");
                    assert_eq!(
                        got[0].0,
                        format!("input 'up': inputs of type 'FILE' {w}"),
                        "{default}"
                    );
                    assert_eq!(got[0].1, "inputs.up.defaults");
                }
            }
        }
        // Only FILE: the same default on a STRING input is silent.
        let f = with_inputs("  - id: s\n    type: STRING\n    defaults: something\n");
        assert!(errors_of(&f, "input_file_defaults_invalid").is_empty());
    }

    /// kestra: RegexValidator.java:26-32 on STRING and SECRET.
    /// `invalids/inputs-bad-validator-syntax`'s lone `\` is refused, with
    /// Kestra's text; Java-valid lookaround and backreferences compile under
    /// fancy-regex (D10); another type's `validator` is not read.
    #[test]
    fn a_validator_that_does_not_compile_is_refused() {
        for (ty, v, bad) in [
            ("STRING", r"'\'", true),
            ("SECRET", "'[a-'", true),
            ("STRING", "'(unclosed'", true),
            ("STRING", "'^[a-z]+$'", false),
            ("STRING", r"'(?=.*\d)\w+'", false),
            ("SECRET", r"'(a)\1'", false),
            ("INT", r"'\'", false),
            // Review I2's probes: an extended-mode comment passes (Java
            // compiles it, and the start check now anchors it); a stray `)`
            // that would escape the anchoring is refused.
            ("STRING", "'(?x) [a-z]+ # letters'", false),
            ("STRING", "'abc)|(?:.*'", true),
        ] {
            let f = with_inputs(&format!("  - id: v\n    type: {ty}\n    validator: {v}\n"));
            let got = errors_of(&f, "input_validator_invalid");
            if bad {
                let raw: String = serde_norway::from_str(v).unwrap();
                assert_eq!(
                    got,
                    [(
                        format!("input 'v': invalid pattern [{raw}]"),
                        "inputs.v.validator".to_string()
                    )],
                    "{ty} {v}"
                );
            } else {
                assert!(got.is_empty(), "{ty} {v}: {got:?}");
            }
        }
    }

    /// Review I1: a bound that does not parse for its type is refused at save
    /// (and not by the run gate), with the cause the start check would give.
    /// A bound that parses, and a bound on a type that has none, is silent.
    #[test]
    fn a_bound_that_does_not_parse_for_its_type_is_refused() {
        for (decl, want) in [
            (
                "type: INT\n    min: abc",
                Some(("min", "Cannot convert value 'abc' to Integer")),
            ),
            (
                "type: INT\n    max: 1.5",
                Some(("max", "Cannot convert value '1.5' to Integer")),
            ),
            ("type: INT\n    min: 1\n    max: '7'", None),
            ("type: INT\n    min: 2.0", None),
            (
                "type: FLOAT\n    min: x1",
                Some(("min", "Cannot convert value 'x1' to Float")),
            ),
            ("type: FLOAT\n    min: 0.5\n    max: 2", None),
            (
                "type: DURATION\n    max: 1h",
                Some(("max", "Cannot convert value '1h' to Duration")),
            ),
            ("type: DURATION\n    min: PT1H", None),
            (
                "type: DATE\n    after: yesterday",
                Some(("after", "Cannot convert value 'yesterday' to LocalDate")),
            ),
            ("type: DATE\n    before: '2026-01-01'", None),
            (
                "type: DATETIME\n    before: '2026-01-01'",
                Some(("before", "Cannot convert value '2026-01-01' to Instant")),
            ),
            ("type: DATETIME\n    after: '2026-01-01T00:00:00Z'", None),
            (
                "type: TIME\n    after: '25:00'",
                Some(("after", "Cannot convert value '25:00' to LocalTime")),
            ),
            ("type: TIME\n    after: '10:00:00'", None),
            ("type: STRING\n    min: abc\n    after: x", None),
        ] {
            let f = with_inputs(&format!("  - id: b\n    {decl}\n    required: false\n"));
            let got = errors_of(&f, "input_bound_invalid");
            match want {
                None => assert!(got.is_empty(), "{decl}: {got:?}"),
                Some((field, cause)) => assert_eq!(
                    got,
                    [(
                        format!("input 'b': invalid `{field}`: {cause}"),
                        format!("inputs.b.{field}")
                    )],
                    "{decl}"
                ),
            }
            assert!(validate_for_run(&f).errors.is_empty(), "run gate: {decl}");
        }
    }

    /// Companion: distinct input ids must not be flagged.
    #[test]
    fn distinct_input_ids_are_not_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ninputs:\n  - id: x\n    type: STRING\n  - id: y\n    type: STRING\ntasks:\n  - id: t\n    type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.switch\n    value: x\n    defaults:\n      - id: d\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.switch\n    value: x\n    cases:\n      a:\n        - id: c\n          type: core.log\n",
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
        let yaml = "id: nightly\nnamespace: company.analytics\ninputs:\n  - id: run_date\n    type: DATE\n  - id: dry_run\n    type: BOOL\n    dependsOn:\n      inputs: [run_date]\ntasks:\n  - id: pipeline\n    type: core.dag\n    tasks:\n      - task:\n          id: extract\n          type: egret.sql\n          database: d\n          sql: SELECT 1\n      - task:\n          id: load\n          type: egret.sql\n          database: d\n          sql: SELECT 2\n        dependsOn: [extract]\n  - id: fanout\n    type: core.parallel\n    tasks:\n      - id: enrich\n        type: egret.sql\n        database: d\n        sql: SELECT 3\nerrors:\n  - id: alert\n    type: core.log\nfinally:\n  - id: cleanup\n    type: core.log\n";
        let f = parse(yaml).unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    // -- Fix round 2: fail-CLOSED on the namespace, not on table membership --

    /// The test that would have caught round 1's regression: a Kestra
    /// control-flow FQCN this crate has never heard of -- not in
    /// `KESTRA_FLOW_CONTROL` at all -- must still be reported, because the
    /// whole point is catching the next class Kestra adds BEFORE anyone
    /// notices it and adds a row for it.
    ///
    /// **An error since no-silent-stall Task 1 (spec §3), and no longer a
    /// warning**: no handler serves such a type, so a run that reached it
    /// stalled. `unsupported_flow_task` replaces the task-level
    /// `not_yet_enforced`; the flow-level `not_yet_enforced` warnings are
    /// untouched (`declared_but_unimplemented_features_warn_and_name_their_package`).
    #[test]
    fn a_fictional_kestra_flow_control_fqcn_is_refused_even_with_no_table_row() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: io.kestra.plugin.core.flow.NotARealClass\n",
        )
        .unwrap();
        let r = validate(&f);
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "unsupported_flow_task" && e.path == "tasks.gate")
            .unwrap_or_else(|| {
                panic!("a Kestra flow-control FQCN with no table row must still be refused: {r:?}")
            });
        assert!(
            !e.message.contains("W1")
                && !e.message.contains("W3")
                && !e.message.contains("W6")
                && !e.message.contains("W8"),
            "a class with no table row must not fabricate a package name: {}",
            e.message
        );
        assert!(r.warnings.is_empty(), "an error, not also a warning: {r:?}");
    }

    /// The same failure mode via the `core.` alias space: an unregistered
    /// alias that looks like it could be a Kestra control-flow shorthand
    /// is refused too, not just the FQCN spelling.
    #[test]
    fn an_unregistered_core_alias_is_refused_too() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: core.somenewgate\n")
            .unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "unsupported_flow_task" && e.path == "tasks.gate"),
            "{r:?}"
        );
    }

    /// Anything that is neither a `TaskKind` variant nor
    /// `registry::known_type(...)`, and does not even look like Kestra's
    /// control-flow namespace, is refused under its own code, so a typo like
    /// `egret.sqll` is visible to the author. **An error since no-silent-stall
    /// Task 1** (spec §3): a warning let a draft start, and the run stalled
    /// with `NoHandler`.
    #[test]
    fn a_completely_unknown_type_is_the_error_unknown_task_type() {
        let f = parse("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sqll\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "unknown_task_type" && e.path == "tasks.t"),
            "{r:?}"
        );
        assert!(r.warnings.is_empty(), "an error, not also a warning: {r:?}");
    }

    /// The false-positive guard the fix round asked for by name: egret's own
    /// registered leaf types (`core.log`, which shares the `core.` alias
    /// spelling with real Kestra control-flow aliases; `core.http` and
    /// `core.slack` were here until package #4 Task C6 removed their rows --
    /// see `a_removed_type_is_unknown_with_a_hint_under_every_position`), an
    /// ordinary `egret.*` leaf, and every ported control-flow
    /// type must all stay completely silent -- "neither a control-flow
    /// variant nor a registered type", not "starts with `core.`".
    ///
    /// `core.allowfailure` and `core.workingdirectory` left this flow at
    /// no-silent-stall Task 1: they are typed variants egret does NOT run, so
    /// each is an error now (`every_static_refusal_is_an_error_at_its_path_under_every_spelling_and_position`).
    /// Package #4 Task C6 (2026-10-28, spec D10): `core.http`, `core.slack`
    /// and `egret.dataset.refresh` lost their rows. A flow still naming one is
    /// `unknown_task_type` with the type's hint -- one finding, at its path,
    /// in `tasks:` and `errors:` alike -- and NOT `unsupported_flow_task`,
    /// which the `core.` spelling would otherwise reach.
    #[test]
    fn a_removed_type_is_unknown_with_a_hint_under_every_position() {
        for (t, hint) in crate::spec::registry::REMOVED_TYPES {
            assert!(
                crate::spec::registry::known_type(t).is_none(),
                "{t} has a row"
            );
            let yaml = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: a\n    type: {t}\nerrors:\n  - id: b\n    type: {t}\n"
            );
            let r = validate(&parse(&yaml).unwrap());
            assert_eq!(r.errors.len(), 2, "{t}: {r:?}");
            for path in ["tasks.a", "errors.b"] {
                let e = r
                    .errors
                    .iter()
                    .find(|e| e.path == path)
                    .unwrap_or_else(|| panic!("{t} at {path}: {r:?}"));
                assert_eq!(e.code, "unknown_task_type", "{t}");
                assert!(
                    e.message.contains("no longer recognises") && e.message.ends_with(hint),
                    "{t}: {}",
                    e.message
                );
            }
        }
        // The control: an unknown `core.` spelling that was never a row is
        // still the control-flow refusal.
        let r = validate(
            &parse("id: f\nnamespace: a.b\ntasks:\n  - id: a\n    type: core.ftp\n").unwrap(),
        );
        assert!(
            r.errors.iter().any(|e| e.code == "unsupported_flow_task"),
            "{r:?}"
        );
    }

    #[test]
    fn registered_leaf_types_and_every_ported_control_flow_type_stay_silent() {
        let yaml = "id: f\nnamespace: a.b\ntasks:\n  - id: log\n    type: core.log\n  - id: sql\n    type: egret.sql\n    sql: SELECT 1\n  - id: seq\n    type: core.sequential\n    tasks:\n      - id: seq_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: par\n    type: core.parallel\n    tasks:\n      - id: par_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: cond\n    type: core.if\n    condition: \"true\"\n    then:\n      - id: cond_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: sw\n    type: core.switch\n    value: x\n    defaults:\n      - id: sw_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: d_child\n          type: egret.sql\n          sql: SELECT 1\n  - id: lp\n    type: core.loop\n    values: [1]\n    tasks:\n      - id: lp_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: lpu\n    type: core.loopuntil\n    condition: x\n    tasks:\n      - id: lpu_child\n        type: egret.sql\n        sql: SELECT 1\n  - id: sf\n    type: core.subflow\n    namespace: a.b\n    flowId: other\n";
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.sequential\n    tasks:\n      - id: c\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: core.parallel\n    tasks:\n      - id: c\n        type: core.log\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty(), "{r:?}");
    }

    #[test]
    fn if_condition_absent_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    then:\n      - id: c\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    condition: \"true\"\n    then:\n      - id: c\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: c\n          type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: c\n          type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n    tasks:\n      - id: c\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loopuntil\n    condition: x\n    tasks:\n      - id: c\n        type: core.log\n",
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
    // `namespace`/`flowId` were then encoded into one `"{namespace}/{flowId}"`
    // string, and a value carrying its own `/` collided with the separator.
    // (Publish no longer encodes it, debt §5.1; the rule is Kestra's grammar.)

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
    fn allow_failure_tasks_present_is_valid() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: a\n    type: core.allowfailure\n    tasks:\n      - id: c\n        type: core.log\n",
        )
        .unwrap();
        let r = validate(&f);
        // The construct is planned now (spec §5.2), so a block with children
        // and no `retry:` draws nothing at all.
        assert!(r.errors.is_empty() && r.warnings.is_empty(), "{r:?}");
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

    /// The block runs now (plan Task 3), so a block with children and none of
    /// the properties that are not run yet draws nothing at all.
    #[test]
    fn working_directory_tasks_present_is_valid() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: w\n    type: core.workingdirectory\n    tasks:\n      - id: c\n        type: core.log\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.is_empty() && r.warnings.is_empty(), "{r:?}");
    }

    /// Secondary item: `Loop::concurrency_limit` accepted absence (it has a
    /// `#[serde(default = "default_concurrency_limit")]`) but rejected an
    /// explicit `null` before this round -- the same asymmetry
    /// `Parallel::concurrent`/`Dag::concurrent` were already fixed against.
    #[test]
    fn loop_concurrency_limit_explicit_null_is_a_semantic_error_not_a_parse_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n    concurrencyLimit: null\n    tasks:\n      - id: c\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: core.loop\n    values: [1]\n    tasks:\n      - id: c\n        type: core.log\n",
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
        let f = parse(
            "id: 'bad id/with slash'\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "invalid_flow_id"),
            "{r:?}"
        );
    }

    #[test]
    fn flow_id_empty_is_an_error() {
        let f = parse("id: ''\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n").unwrap();
        let r = validate(&f);
        assert!(
            r.errors.iter().any(|e| e.code == "invalid_flow_id"),
            "{r:?}"
        );
    }

    #[test]
    fn flow_id_valid_is_silent() {
        let f =
            parse("id: nightly_rollup.v2\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n")
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
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\nquotas:\n  - {}\n",
        )
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
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
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\nchecks:\n  - {}\n",
        )
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\nchecks:\n  - when: \"\"\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: i\n    type: core.if\n    condition: \"true\"\n    then:\n      - id: t\n        type: core.log\n",
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
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
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
    /// otherwise have decoded ambiguously against publish's former
    /// `"{namespace}/{flowId}"` encoding (gone since debt §5.1; the rule is
    /// Kestra's grammar).
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

    // ─── W9c Task 4: `secret()` in a KV value (W9c spec §7) ──────────────────

    /// A `Set` or `Put` task `s` whose `value` is `value_yaml` (YAML, already
    /// indented under the key).
    fn kv_task(ty: &str, value_yaml: &str) -> String {
        format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: {ty}\n    key: k\n    value:{value_yaml}"
        )
    }

    /// `tasks.<id>.value…` of every finding of `code`.
    fn paths_of(src: &str, code: &str) -> Vec<String> {
        validate(&parse(src).unwrap())
            .errors
            .into_iter()
            .filter(|f| f.code == code)
            .map(|f| f.path)
            .collect()
    }

    #[test]
    fn a_secret_in_a_set_or_put_value_is_refused_with_its_own_code_and_message() {
        use crate::plugins::{
            secret_in_kv_value_message, CODE_SECRET_IN_KV_VALUE, CODE_SECRET_OUTSIDE_PLUGIN_TASK,
        };
        use crate::worker::handlers::kv::{PUT, SET};
        const SECRET: &str = "\"{{ secret('X') }}\"";
        for ty in [SET, PUT] {
            for (shape, value, path) in [
                ("the whole value", format!(" {SECRET}\n"), "tasks.s.value"),
                (
                    "a nested map leaf",
                    format!("\n      a:\n        b: {SECRET}\n"),
                    "tasks.s.value.a.b",
                ),
                (
                    "a list item",
                    format!("\n      - plain\n      - {SECRET}\n"),
                    "tasks.s.value[1]",
                ),
                (
                    "a map key",
                    format!("\n      {SECRET}: 1\n"),
                    "tasks.s.value",
                ),
            ] {
                let src = kv_task(ty, &value);
                let f = parse(&src).unwrap();
                let report = validate(&f);
                let found: Vec<_> = report
                    .errors
                    .iter()
                    .filter(|e| e.code == CODE_SECRET_IN_KV_VALUE)
                    .collect();
                assert_eq!(found.len(), 1, "{ty} {shape}: {:?}", report.errors);
                assert_eq!(found[0].path, path, "{ty} {shape}");
                assert_eq!(
                    found[0].message,
                    secret_in_kv_value_message("s", path),
                    "{ty} {shape}"
                );
                assert!(
                    found[0].message.contains("task 's'")
                        && found[0].message.contains(path)
                        && found[0].message.contains(
                            "the secret's value would be stored in the KV store, where anyone \
                             who can read this namespace's KV (`workflow:read`) could read it, \
                             unmasked; pass the secret to the task that needs it instead."
                        ),
                    "{}",
                    found[0].message
                );
                // Instead of the generic refusal, never beside it.
                assert!(
                    !report
                        .errors
                        .iter()
                        .any(|e| e.code == CODE_SECRET_OUTSIDE_PLUGIN_TASK),
                    "{ty} {shape}: {:?}",
                    report.errors
                );
            }
            // The control: a value with no secret() is clean.
            let src = kv_task(ty, " \"{{ 'plain' }}\"\n");
            assert!(paths_of(&src, CODE_SECRET_IN_KV_VALUE).is_empty());
        }
    }

    #[test]
    fn a_secret_elsewhere_on_a_set_task_keeps_the_generic_refusal() {
        use crate::plugins::{CODE_SECRET_IN_KV_VALUE, CODE_SECRET_OUTSIDE_PLUGIN_TASK};
        use crate::worker::handlers::kv::{PUT, SET};
        for ty in [SET, PUT] {
            let src = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: {ty}\n    \
                 key: \"{{{{ secret('X') }}}}\"\n    value: v\n"
            );
            assert_eq!(
                paths_of(&src, CODE_SECRET_OUTSIDE_PLUGIN_TASK),
                vec!["tasks.s.key".to_string()],
                "{ty}"
            );
            assert!(paths_of(&src, CODE_SECRET_IN_KV_VALUE).is_empty(), "{ty}");
        }
        // A `value` property on any other task is not a KV value.
        let src = "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.debug.Return\n    \
                   format: x\n    value: \"{{ secret('X') }}\"\n";
        assert_eq!(
            paths_of(src, CODE_SECRET_OUTSIDE_PLUGIN_TASK),
            vec!["tasks.s.value".to_string()]
        );
        assert!(paths_of(src, CODE_SECRET_IN_KV_VALUE).is_empty());
    }

    /// Package #4 Task C3 (spec D5): `secret()` resolves on the http tasks, so
    /// their properties are exempt under every spelling -- while their `runIf`
    /// (which egret's task model reads, not the handler) and a `Log` beside
    /// them are still refused. A `namespace=` written in the call is refused
    /// there as on a plugin task.
    #[test]
    fn a_secret_in_an_http_tasks_properties_is_allowed_under_every_spelling() {
        use crate::plugins::{CODE_SECRET_ARGUMENT_REFUSED, CODE_SECRET_OUTSIDE_PLUGIN_TASK};
        for ty in [
            "io.kestra.plugin.core.http.Request",
            "io.kestra.plugin.fs.http.Request",
        ] {
            let src = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    type: {ty}\n    \
                 uri: https://example.com/\n    \
                 headers:\n      Authorization: \"Bearer {{{{ secret('TOKEN') }}}}\"\n    \
                 options:\n      auth:\n        type: BASIC\n        username: u\n        \
                 password: \"{{{{ secret('PW') }}}}\"\n    \
                 runIf: \"{{{{ secret('X') }}}}\"\n  \
                 - id: l\n    type: io.kestra.plugin.core.log.Log\n    \
                 message: \"{{{{ secret('X') }}}}\"\n"
            );
            assert_eq!(
                paths_of(&src, CODE_SECRET_OUTSIDE_PLUGIN_TASK),
                vec!["tasks.r.runIf".to_string(), "tasks.l.message".to_string()],
                "{ty}"
            );
            let src = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    type: {ty}\n    \
                 uri: \"https://example.com/{{{{ secret('K', namespace='x') }}}}\"\n"
            );
            assert_eq!(
                paths_of(&src, CODE_SECRET_ARGUMENT_REFUSED),
                vec!["tasks.r.uri".to_string()],
                "{ty}"
            );
            assert!(paths_of(&src, CODE_SECRET_OUTSIDE_PLUGIN_TASK).is_empty());
        }
    }

    /// Package #4 (spec D4, and the C3 userinfo decision): each refused http
    /// property is an error at its path, at save only -- the run gate leaves
    /// it to the task, which fails naming the same property.
    #[test]
    fn the_http_refusals_are_save_time_errors_at_their_paths() {
        use crate::spec::validate::CODE_HTTP_OPTION_REFUSED;
        let src = "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    \
                   type: io.kestra.plugin.fs.http.Request\n    \
                   uri: https://user:pw@example.com/\n    encryptBody: true\n    \
                   options:\n      proxy:\n        address: proxy.corp\n      \
                   ssl:\n        insecureTrustAllCertificates: true\n      \
                   auth:\n        type: DIGEST\n";
        let mut paths = paths_of(src, CODE_HTTP_OPTION_REFUSED);
        paths.sort();
        assert_eq!(
            paths,
            [
                "tasks.r.encryptBody",
                "tasks.r.options.auth.type",
                "tasks.r.options.proxy",
                "tasks.r.options.ssl.insecureTrustAllCertificates",
                "tasks.r.uri",
            ]
        );
        let f = parse(src).unwrap();
        let messages: Vec<String> = validate(&f)
            .errors
            .into_iter()
            .filter(|e| e.code == CODE_HTTP_OPTION_REFUSED)
            .map(|e| e.message)
            .collect();
        assert!(
            messages.iter().all(|m| m.starts_with("task 'r': ")),
            "{messages:?}"
        );
        assert!(
            !messages.iter().any(|m| m.contains("pw")),
            "the URI's password is never echoed: {messages:?}"
        );
        // Save-only: the run gate does not raise it.
        assert!(!validate_for_run(&f)
            .errors
            .iter()
            .any(|e| e.code == CODE_HTTP_OPTION_REFUSED));
        // The control: the allowed spellings are clean.
        let ok = "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    \
                  type: io.kestra.plugin.core.http.Request\n    \
                  uri: https://example.com/\n    encryptBody: false\n    \
                  options:\n      ssl:\n        insecureTrustAllCertificates: false\n      \
                  auth:\n        type: BEARER\n        token: t\n";
        assert!(paths_of(ok, CODE_HTTP_OPTION_REFUSED).is_empty());
        assert!(validate(&parse(ok).unwrap()).errors.is_empty());
    }

    /// Package #4 Task C6 (the C4 review's concern): `encryptBody` on
    /// `Download`/`SseRequest` is ONE finding, `unknown_property` (the key is
    /// not Kestra's on those classes), not that plus Request's refusal, whose
    /// text is about Request's `body` output. On `Request` it is the refusal
    /// alone: `spec::keys` no longer ALSO calls it unknown there.
    #[test]
    fn encrypt_body_is_one_finding_on_each_http_task() {
        use crate::spec::validate::CODE_HTTP_OPTION_REFUSED;
        for (ty, code) in [
            ("io.kestra.plugin.core.http.Download", "unknown_property"),
            ("io.kestra.plugin.fs.http.Download", "unknown_property"),
            ("io.kestra.plugin.core.http.SseRequest", "unknown_property"),
            (
                "io.kestra.plugin.core.http.Request",
                CODE_HTTP_OPTION_REFUSED,
            ),
            ("io.kestra.plugin.fs.http.Request", CODE_HTTP_OPTION_REFUSED),
        ] {
            let src = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    type: {ty}\n    \
                 uri: https://example.com/\n    encryptBody: true\n"
            );
            let r = crate::spec::validate::validate_source(&src, &parse(&src).unwrap());
            let at: Vec<&str> = r
                .errors
                .iter()
                .filter(|e| e.path == "tasks.r.encryptBody")
                .map(|e| e.code)
                .collect();
            assert_eq!(at, [code], "{ty}: {r:?}");
        }
        // `false` is accepted on Request -- it was `unknown_property` until
        // this task -- and is still unknown on the other two.
        for (ty, findings) in [
            ("io.kestra.plugin.core.http.Request", 0),
            ("io.kestra.plugin.fs.http.Request", 0),
            ("io.kestra.plugin.core.http.Download", 1),
        ] {
            let src = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    type: {ty}\n    \
                 uri: https://example.com/\n    encryptBody: false\n"
            );
            let r = crate::spec::validate::validate_source(&src, &parse(&src).unwrap());
            assert_eq!(r.errors.len(), findings, "{ty}: {r:?}");
        }
    }

    /// The corpus's two KV flows carry no `secret()`: the scan adds no
    /// finding to either, and `valids/kv` validates clean for publish and
    /// for the run.
    #[test]
    fn valids_kv_and_retry_with_flowable_errors_still_publish_and_plan() {
        for (name, src) in [
            (
                "kv",
                include_str!("../../oracle-exec/corpus/valids/kv.yaml"),
            ),
            (
                "retry-with-flowable-errors",
                include_str!("../../oracle-exec/corpus/valids/retry-with-flowable-errors.yaml"),
            ),
        ] {
            let f = parse(src).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let errors = validate(&f).errors;
            assert!(
                !errors.iter().any(|e| e.code.starts_with("secret_")),
                "{name}: {errors:?}"
            );
            if name == "kv" {
                assert!(errors.is_empty(), "{name}: {errors:?}");
                assert!(
                    crate::spec::validate_for_run(&f).errors.is_empty(),
                    "{name}"
                );
            }
        }
    }

    // ---------- no-silent-stall Task 1: static refusals (spec §3) ----------
    //
    // Every construct `build_plan` plans as an opaque `Runnable` because egret
    // has no executor for it is a validation ERROR: publish refuses it and,
    // because `validate_for_run` is `build_plan`'s third gate, so does every
    // start path. Before this, each of them published (clean, or with a
    // warning) and every execution of it stalled in `RUNNING` for ever.

    /// One construct of §3's table: the code it must draw, the spellings it
    /// must draw it under, the property lines of one task (`id: c`), the path
    /// suffix the finding sits at below the task's own path, and the words the
    /// message must contain.
    struct Refused {
        code: &'static str,
        spellings: &'static [&'static str],
        body: fn(&str) -> Vec<String>,
        suffix: &'static str,
        hints: &'static [&'static str],
    }

    fn allow_failure_retry_body(ty: &str) -> Vec<String> {
        vec![
            "id: c".into(),
            format!("type: {ty}"),
            "retry:".into(),
            "  type: constant".into(),
            "  interval: PT1S".into(),
            "  maxAttempts: 3".into(),
            "tasks:".into(),
            "  - id: inner".into(),
            "    type: core.log".into(),
            "    message: hi".into(),
        ]
    }

    fn loop_until_gap_body(ty: &str) -> Vec<String> {
        vec![
            "id: c".into(),
            format!("type: {ty}"),
            "condition: \"{{ true }}\"".into(),
            "checkFrequency:".into(),
            "  maxIterations: \"{{ inputs.n }}\"".into(),
            "tasks:".into(),
            "  - id: inner".into(),
            "    type: core.log".into(),
            "    message: hi".into(),
        ]
    }

    fn leaf_body(ty: &str) -> Vec<String> {
        vec!["id: c".into(), format!("type: {ty}")]
    }

    const REFUSED: &[Refused] = &[
        Refused {
            code: "unsupported_allow_failure_retry",
            spellings: &[
                "io.kestra.plugin.core.flow.AllowFailure",
                "core.allowfailure",
                "io.kestra.plugin.core.flow.allowFAILURE",
                "core.AllowFailure",
            ],
            body: allow_failure_retry_body,
            suffix: ".retry",
            hints: &["put `retry:` on the child task that should be retried. Roadmap package #9 lifts it"],
        },
        Refused {
            code: "unsupported_loop_until_property",
            spellings: &[
                "io.kestra.plugin.core.flow.LoopUntil",
                "core.loopuntil",
                "io.kestra.plugin.core.flow.WaitFor",
                "core.waitfor",
                "io.kestra.plugin.core.flow.LOOPUNTIL",
                "core.WaitFor",
            ],
            body: loop_until_gap_body,
            suffix: ".checkFrequency.maxIterations",
            hints: &["'checkFrequency.maxIterations'", "must be a literal value. Roadmap package #9 lifts it"],
        },
        Refused {
            code: "unsupported_flow_task",
            spellings: &[
                "io.kestra.plugin.core.flow.NotARealClass",
                "core.somenewgate",
                "io.kestra.plugin.core.flow.PurgeFlows",
                "core.NotARealClass",
            ],
            body: leaf_body,
            suffix: "",
            hints: &[],
        },
        Refused {
            code: "unknown_task_type",
            // A Kestra-shipped type egret does not port; `core.output.OutputValues`
            // was the example until group C1 Task 2 registered it.
            spellings: &[
                "egret.sqll",
                "io.kestra.plugin.notifications.slack.SlackIncomingWebhook",
            ],
            body: leaf_body,
            suffix: "",
            hints: &["check for a typo"],
        },
    ];

    /// The task's lines as a list item at `indent` (`- id: c` ...).
    fn item(lines: &[String], indent: &str) -> String {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    format!("{indent}- {l}\n")
                } else {
                    format!("{indent}  {l}\n")
                }
            })
            .collect()
    }

    /// The task's lines as a mapping at `indent` (a `Dag` node's `task:`).
    fn mapping(lines: &[String], indent: &str) -> String {
        lines.iter().map(|l| format!("{indent}{l}\n")).collect()
    }

    /// The five nested positions of the brief, plus the top level, as
    /// `(label, flow source, the task's path)`.
    fn positions(lines: &[String]) -> Vec<(&'static str, String, &'static str)> {
        let head = "id: f\nnamespace: a.b\n";
        vec![
            (
                "top level",
                format!("{head}tasks:\n{}", item(lines, "  ")),
                "tasks.c",
            ),
            (
                "a Sequential's tasks:",
                format!(
                    "{head}tasks:\n  - id: seq\n    type: core.sequential\n    tasks:\n{}",
                    item(lines, "      ")
                ),
                "tasks.seq.c",
            ),
            (
                "a Sequential's errors:",
                format!(
                    "{head}tasks:\n  - id: seq\n    type: core.sequential\n    tasks:\n      \
                     - id: ok\n        type: core.log\n        message: hi\n    errors:\n{}",
                    item(lines, "      ")
                ),
                "tasks.seq.c",
            ),
            (
                "an If's then:",
                format!(
                    "{head}tasks:\n  - id: cond\n    type: core.if\n    condition: \"{{{{ true }}}}\"\n    then:\n{}",
                    item(lines, "      ")
                ),
                "tasks.cond.c",
            ),
            (
                "a Dag node",
                format!(
                    "{head}tasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n{}",
                    mapping(lines, "          ")
                ),
                "tasks.d.c",
            ),
            (
                "the flow's finally:",
                format!(
                    "{head}tasks:\n  - id: ok\n    type: core.log\n    message: hi\nfinally:\n{}",
                    item(lines, "  ")
                ),
                "finally.c",
            ),
        ]
    }

    /// Every construct of §3, under every spelling, at every position: an
    /// ERROR with its own code, at the task's path (or the field's, for a
    /// `LoopUntil`), whose message carries §3's hint and the type, and which
    /// is not ALSO a warning. `validate_for_run` carries it too, which is what
    /// makes `build_plan` and every start path refuse the flow.
    #[test]
    fn every_static_refusal_is_an_error_at_its_path_under_every_spelling_and_position() {
        for c in REFUSED {
            for spelling in c.spellings {
                let lines = (c.body)(spelling);
                for (label, yaml, path) in positions(&lines) {
                    let at = format!("{}/{spelling}/{label}", c.code);
                    let f = parse(&yaml).unwrap_or_else(|e| panic!("{at}: {e:?}\n{yaml}"));
                    let r = validate(&f);
                    let want = format!("{path}{}", c.suffix);
                    let e = r
                        .errors
                        .iter()
                        .find(|e| e.code == c.code)
                        .unwrap_or_else(|| panic!("{at}: no `{}` error: {r:?}", c.code));
                    assert_eq!(e.path, want, "{at}");
                    assert!(
                        e.message.contains("'c'"),
                        "{at}: names the task: {}",
                        e.message
                    );
                    if c.code != "unsupported_loop_until_property" {
                        assert!(
                            e.message.contains(spelling),
                            "{at}: names the type: {}",
                            e.message
                        );
                    }
                    for hint in c.hints {
                        assert!(
                            e.message.contains(hint),
                            "{at}: hint {hint:?}: {}",
                            e.message
                        );
                    }
                    assert_eq!(
                        r.errors.iter().filter(|e| e.code == c.code).count(),
                        1,
                        "{at}: reported exactly once: {r:?}"
                    );
                    assert!(
                        !r.warnings.iter().any(|w| w.path.starts_with(path)),
                        "{at}: an error, not also a warning: {r:?}"
                    );
                    assert!(
                        validate_for_run(&f).errors.iter().any(|e| e.code == c.code),
                        "{at}: validate_for_run must refuse it"
                    );
                }
            }
        }
    }

    /// `loop_until_gap` names four fields; each is refused at its own path,
    /// and the same `LoopUntil` with all four written as literals is valid
    /// and silent -- the control that shows the error is about the
    /// expression and not about the construct.
    #[test]
    fn each_loop_until_field_egret_cannot_read_is_refused_by_name_and_literals_are_valid() {
        for (field, lines) in [
            (
                "checkFrequency.interval",
                "    checkFrequency:\n      interval: \"{{ inputs.i }}\"\n",
            ),
            (
                "checkFrequency.maxDuration",
                "    checkFrequency:\n      maxDuration: \"{{ inputs.d }}\"\n",
            ),
            (
                "checkFrequency.maxIterations",
                "    checkFrequency:\n      maxIterations: \"{{ inputs.n }}\"\n",
            ),
            (
                "failOnMaxReached",
                "    failOnMaxReached: \"{{ inputs.f }}\"\n",
            ),
        ] {
            let yaml = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: lu\n    type: core.loopuntil\n    \
                 condition: \"{{{{ true }}}}\"\n{lines}    tasks:\n      - id: inner\n        \
                 type: core.log\n        message: hi\n"
            );
            let r = validate(&parse(&yaml).unwrap_or_else(|e| panic!("{field}: {e:?}")));
            let e = r
                .errors
                .iter()
                .find(|e| e.code == "unsupported_loop_until_property")
                .unwrap_or_else(|| panic!("{field}: {r:?}"));
            assert_eq!(e.path, format!("tasks.lu.{field}"));
            assert!(e.message.contains(&format!("'{field}'")), "{}", e.message);
            assert!(
                e.message
                    .ends_with("must be a literal value. Roadmap package #9 lifts it"),
                "{}",
                e.message
            );
        }
        let literal = "id: f\nnamespace: a.b\ntasks:\n  - id: lu\n    type: core.loopuntil\n    \
                       condition: \"{{ true }}\"\n    checkFrequency:\n      interval: PT1S\n      \
                       maxDuration: PT1M\n      maxIterations: 3\n    failOnMaxReached: true\n    \
                       tasks:\n      - id: inner\n        type: core.log\n        message: hi\n";
        let r = validate(&parse(literal).unwrap());
        assert!(r.errors.is_empty() && r.warnings.is_empty(), "{r:?}");
    }

    /// A `WorkingDirectory` with no `tasks:` draws its `*_tasks_required` and
    /// nothing else: the block is runnable now, so there is no refusal beside it.
    #[test]
    fn an_empty_working_directory_draws_only_its_tasks_required() {
        let ty = "core.workingdirectory";
        let r = validate(
            &parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: c\n    type: {ty}\n"
            ))
            .unwrap(),
        );
        let codes: Vec<&str> = r.errors.iter().map(|e| e.code).collect();
        assert_eq!(codes, ["working_directory_tasks_required"], "{ty}: {r:?}");
    }

    /// A typo is reported ONCE, as an error, wherever it sits -- including
    /// under a flowable's `errors:`, which the walk now descends into, and
    /// which `validate_script_tasks`/`validate_plugin_tasks` already walk.
    #[test]
    fn an_unknown_type_is_one_error_wherever_it_sits() {
        for (label, yaml, _) in positions(&leaf_body("egret.sqll")) {
            let r = validate(&parse(&yaml).unwrap());
            let all: Vec<_> = r
                .errors
                .iter()
                .chain(r.warnings.iter())
                .filter(|f| f.code == "unknown_task_type")
                .collect();
            assert_eq!(all.len(), 1, "{label}: {r:?}");
            assert!(
                r.errors.iter().any(|e| e.code == "unknown_task_type"),
                "{label}: an error now, not a warning: {r:?}"
            );
        }
    }

    /// The descent into `error_children()` reaches the walk's OTHER checks
    /// too, and this is the one that changes what is refused: a task under a
    /// flowable's `errors:` now takes part in the duplicate-id check, as it
    /// does in Kestra's `allTasks()` (`AbstractBranch.allChildTasks()` is
    /// `tasks + errors + finally`). Before, the second `a` below was invisible
    /// to `seen_ids`. The control: distinct ids stay clean.
    #[test]
    fn a_task_under_a_flowables_errors_takes_part_in_every_walk_check() {
        let dup =
            "id: f\nnamespace: a.b\ntasks:\n  - id: a\n    type: core.log\n    message: hi\n  \
                   - id: seq\n    type: core.sequential\n    tasks:\n      - id: b\n        \
                   type: core.log\n        message: hi\n    errors:\n      - id: a\n        \
                   type: core.log\n        message: hi\n";
        let r = validate(&parse(dup).unwrap());
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "duplicate_task_id")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(e.message.contains("tasks.seq.a"), "{}", e.message);

        let empty = "id: f\nnamespace: a.b\ntasks:\n  - id: seq\n    type: core.sequential\n    \
                     tasks:\n      - id: b\n        type: core.log\n        message: hi\n    \
                     errors:\n      - id: inner\n        type: core.sequential\n";
        let r = validate(&parse(empty).unwrap());
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "sequential_tasks_required" && e.path == "tasks.seq.inner.tasks"),
            "a semantic check reaches a task under errors: {r:?}"
        );

        let clean = dup.replace("errors:\n      - id: a\n", "errors:\n      - id: c\n");
        assert!(validate(&parse(&clean).unwrap()).errors.is_empty());
    }

    // ---- end hooks B2 (spec D12): Pause and Exit inside afterExecution ----

    const PAUSE: &str = "io.kestra.plugin.core.flow.Pause";
    const EXIT: &str = "io.kestra.plugin.core.execution.Exit";

    fn after_exec(list: &str, body: &str) -> String {
        let base = if list == "tasks" {
            String::new()
        } else {
            "tasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
                .to_string()
        };
        format!("id: f\nnamespace: a.b\n{base}{list}:\n{body}")
    }

    fn refusals(src: &str) -> Vec<(String, String)> {
        validate_for_run(&parse(src).unwrap())
            .errors
            .into_iter()
            .filter(|e| e.code == "unsupported_after_execution_task")
            .map(|e| (e.path, e.message))
            .collect()
    }

    #[test]
    fn a_pause_inside_after_execution_is_refused_at_start() {
        let src = after_exec("afterExecution", &format!("  - id: p\n    type: {PAUSE}\n"));
        let got = refusals(&src);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "afterExecution.p");
        assert_eq!(
            got[0].1,
            "task 'p' is a Pause inside afterExecution, which egret does not run: the execution has already ended, and a Pause would pause it again. Roadmap package #9 lifts it"
        );
        let err = crate::executor::build_plan(&src).expect_err("build_plan refuses it");
        assert!(
            err.message.starts_with("unsupported_after_execution_task:"),
            "{}",
            err.message
        );
    }

    const LOOP_UNTIL: &str = "io.kestra.plugin.core.flow.LoopUntil";

    fn loop_until_body(id: &str, ty: &str) -> String {
        format!(
            "  - id: {id}\n    type: {ty}\n    condition: \"{{{{ true }}}}\"\n    checkFrequency:\n      interval: PT0S\n    tasks:\n      - id: c\n        type: io.kestra.plugin.core.log.Log\n        message: m\n"
        )
    }

    #[test]
    fn a_loop_until_inside_after_execution_is_refused_at_start() {
        let src = after_exec("afterExecution", &loop_until_body("lu", LOOP_UNTIL));
        let got = refusals(&src);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "afterExecution.lu");
        assert_eq!(
            got[0].1,
            "task 'lu' is a LoopUntil inside afterExecution, which egret does not run: it waits between iterations by pausing the run, and the execution has already ended. Roadmap package #9 lifts it"
        );
        let err = crate::executor::build_plan(&src).expect_err("build_plan refuses it");
        assert!(
            err.message.starts_with("unsupported_after_execution_task:"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_wait_for_nested_in_an_if_inside_after_execution_is_refused() {
        let body = format!(
            "  - id: gate\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{{{ true }}}}\"\n    then:\n{}",
            loop_until_body("w", "io.kestra.plugin.core.flow.WaitFor")
                .lines()
                .map(|l| format!("    {l}\n"))
                .collect::<String>()
        );
        let got = refusals(&after_exec("afterExecution", &body));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "afterExecution.gate.w");
    }

    #[test]
    fn a_loop_until_in_tasks_is_not_refused_by_the_after_execution_gate() {
        let src = after_exec("tasks", &loop_until_body("lu", LOOP_UNTIL));
        assert!(refusals(&src).is_empty());
        crate::executor::build_plan(&src).expect("a LoopUntil in tasks still plans");
    }

    #[test]
    fn an_exit_nested_in_an_if_inside_after_execution_is_refused() {
        let body = format!(
            "  - id: gate\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{{{ true }}}}\"\n    then:\n      - id: x\n        type: {EXIT}\n"
        );
        let got = refusals(&after_exec("afterExecution", &body));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "afterExecution.gate.x");
        assert_eq!(
            got[0].1,
            "task 'x' is an Exit inside afterExecution, which egret does not run: the execution has already ended, and an Exit would set its state. Roadmap package #20 lifts it"
        );
    }

    #[test]
    fn a_pause_in_a_finally_list_inside_after_execution_is_refused() {
        let body = format!(
            "  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: m\n    finally:\n      - id: p\n        type: {PAUSE}\n"
        );
        let got = refusals(&after_exec("afterExecution", &body));
        assert_eq!(
            got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(),
            ["afterExecution.seq.p"]
        );
    }

    #[test]
    fn a_pause_in_tasks_is_not_touched_by_this_rule() {
        let body = format!("  - id: p\n    type: {PAUSE}\n  - id: x\n    type: {EXIT}\n");
        assert!(refusals(&after_exec("tasks", &body)).is_empty());
        assert!(refusals(&after_exec("errors", &body)).is_empty());
    }
}
