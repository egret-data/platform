//! The key tables the save-time property check and the measuring instrument
//! share (spec `2026-10-19-workflow-no-silent-drops-design.md` section 2.3):
//! which keys egret's models read at each site, and which package lifts each
//! key Kestra accepts and egret does not act on (D12).
//!
//! `spec::schema::measure` imports these instead of keeping its own copies,
//! so the instrument and the check read one table.

use serde_json::Value;

use crate::models::{self, Flow};
use crate::spec::schema::{
    property_treatment, wire_fields_of, Treatment, FLOWABLE_TYPES, FLOW_ROW, TRIGGER_TYPES,
};
use crate::spec::validate::{Finding, ValidationReport};
use crate::worker::handlers as h;

/// What `models::Task`'s deserializer reads itself (`models/task.rs:764-781`).
pub(crate) const TASK_MODEL_KEYS: &[&str] = &[
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

/// Kestra `Task.java` keys egret's `Task` does not read
/// (`k/core/models/tasks/Task.java:45,68,74,77,85,97,102`).
pub(crate) const TASK_JAVA_UNREAD: &[&str] = &[
    "version",
    "workerSelector",
    "policyRefs",
    "logLevel",
    "logToFile",
    "taskCache",
    "assets",
];

/// The base keys `models::Trigger`'s deserializer reads
/// (`models/trigger.rs:363-400`).
pub(crate) const TRIGGER_BASE_KEYS: &[&str] = &[
    "id",
    "type",
    "description",
    "when",
    "disabled",
    "labels",
    "stopAfter",
    "logToFile",
    "failOnTriggerError",
    "allowConcurrent",
];

/// Who lifts a key Kestra accepts and egret does not act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lift {
    /// A roadmap package (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md` section 2).
    Package(u8),
    /// Kestra Enterprise only; Kestra OSS ignores it too (roadmap section 3.3).
    Enterprise,
    /// egret design kept (roadmap section 3.2); the text says what replaces the key.
    KeptDesign(&'static str),
}

impl Lift {
    /// "roadmap package #13", "Kestra Enterprise only, ignored by Kestra OSS too, roadmap section 3.3", ...
    pub(crate) fn short(self) -> String {
        match self {
            Lift::Package(n) => format!("roadmap package #{n}"),
            Lift::Enterprise => {
                "Kestra Enterprise only, ignored by Kestra OSS too, roadmap \u{a7}3.3".into()
            }
            Lift::KeptDesign(why) => format!("{why} (roadmap \u{a7}3.2)"),
        }
    }

    /// "roadmap package #13 lifts it", "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap section 3.3)", ...
    pub(crate) fn sentence(self) -> String {
        match self {
            Lift::Package(n) => format!("roadmap package #{n} lifts it"),
            Lift::Enterprise => {
                "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap \u{a7}3.3)"
                    .into()
            }
            Lift::KeptDesign(why) => format!("{why} (roadmap \u{a7}3.2)"),
        }
    }
}

/// Where a key sits, for [`lift_of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Site<'a> {
    Flow,
    /// A task of this canonical type (core leaf, flowable, script or plugin).
    Task(&'a str),
    /// A typed trigger of this canonical type.
    Trigger(&'a str),
    /// An input of this `InputType` wire name ("STRING", ...).
    Input(&'a str),
    /// An entry of the flow's `checks` list.
    Check,
}

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";
const FLOW_TRIGGER: &str = "io.kestra.plugin.core.trigger.Flow";
/// D15, measured: Kestra persists a source `deleted: true` as a tombstone row
/// (`AbstractJdbcFlowRepository.save` writes the parsed `GenericFlow`, and the
/// `deleted` column is generated from the stored JSON, `baseline-postgres.sql:117`).
const DELETED_REASON: &str =
    "egret deletes a flow through its own delete action, never through a key in the source";
const DRAFT_REASON: &str =
    "egret's draft and publish split replaces it: a published version runs whatever `draft:` says";

/// The package that lifts a key Kestra accepts at `site` and egret does not
/// act on, or `None` (egret honours it, or Kestra does not have it).
pub(crate) fn lift_of(site: Site<'_>, key: &str) -> Option<Lift> {
    use Lift::{Enterprise, Package};
    match site {
        Site::Flow => match key {
            "workerSelector" | "policyRefs" => Some(Enterprise),
            "draft" => Some(Lift::KeptDesign(DRAFT_REASON)),
            "deleted" => Some(Lift::KeptDesign(DELETED_REASON)),
            _ => None,
        },
        Site::Task(t) => {
            // `Task.java` keys, every kind of task (spec 3.2, 3.3).
            match key {
                "logLevel" | "logToFile" | "taskCache" | "version" | "assets" => {
                    return Some(Package(13))
                }
                "workerSelector" | "policyRefs" => return Some(Enterprise),
                _ => {}
            }
            // Script-only keys (Task 8).
            if crate::scripts::kind_of(t).is_some() {
                match key {
                    "runner" | "targetOS" => return Some(Package(17)),
                    "dependencies"
                    | "dependencyCacheEnabled"
                    | "packageManager"
                    | "pythonVersion"
                    | "uvAutoInstallEnabled"
                    | "uvInstallerSha256"
                    | "uvInstallerVersion"
                        if matches!(
                            crate::scripts::kind_of(t),
                            Some(
                                crate::scripts::ScriptKind::PythonCommands
                                    | crate::scripts::ScriptKind::PythonScript
                            )
                        ) =>
                    {
                        return Some(Package(17))
                    }
                    _ => {}
                }
            }
            if t.starts_with(crate::plugins::PREFIX) && key == "taskRunner" {
                return Some(Package(18));
            }
            match (t, key) {
                ("io.kestra.plugin.core.flow.Loop", "outputs" | "fetchType") => Some(Package(6)),
                (
                    "io.kestra.plugin.core.flow.Subflow",
                    "revision" | "labels" | "inheritLabels" | "scheduleDate" | "restartBehavior",
                ) => Some(Package(12)),
                ("io.kestra.plugin.core.flow.Pause", "onResume") => Some(Package(9)),
                _ => None,
            }
        }
        Site::Trigger(t) => {
            match key {
                "stopAfter" | "failOnTriggerError" | "allowConcurrent" => return Some(Package(11)),
                "logToFile" | "logLevel" | "version" | "assets" => return Some(Package(13)),
                "workerSelector" | "policyRefs" => return Some(Enterprise),
                _ => {}
            }
            match (t, key) {
                (SCHEDULE | WEBHOOK, "labels" | "when") => Some(Package(11)),
                (SCHEDULE, "lateMaximumDelay") => Some(Package(11)),
                (WEBHOOK, "wait" | "returnOutputs" | "responseContentType" | "responseCode") => {
                    Some(Package(11))
                }
                _ => None,
            }
        }
        Site::Input(ty) => match (ty, key) {
            // No JSON Schema validator is in the dependency tree (spec D12):
            // the value is parsed but not checked against the schema.
            ("JSON", "jsonSchema") => Some(Package(3)),
            ("FORM", "inputs") => Some(Enterprise),
            ("REUSABLE_INPUTS", "ref" | "namespace" | "revision") => Some(Enterprise),
            _ => None,
        },
        Site::Check => match key {
            // `Check.setCondition` (`k/core/models/flows/check/Check.java:64-67`,
            // deprecated since 2.0.0) sets `when`: Jackson accepts the name.
            "condition" => Some(Package(16)),
            _ => None,
        },
    }
}

// ---------- the nested beans (spec 3.7) ----------
//
// `Retry` and `Sla` are internally tagged enums, which `wire_fields_of` cannot
// read, so their key sets are written from Kestra's classes; a unit test
// round-trips every listed key through the model (a key the model drops, or
// reads and the list omits, is red). The other beans are derived structs.

/// `AbstractRetry`'s fields (`k/core/models/tasks/retrys/AbstractRetry.java`:
/// `maxDuration` :31, `maxAttempts` :34, `warningOnRetry` :37, `behavior` :40)
/// and the `type` each subclass declares (`Constant.java:26`,
/// `Exponential.java:27`, `Random.java:27`).
const RETRY_COMMON_KEYS: &[&str] = &[
    "type",
    "maxDuration",
    "maxAttempts",
    "warningOnRetry",
    "behavior",
];

/// What a `retry:` of this `type` has: [`RETRY_COMMON_KEYS`] plus the subclass's
/// own (`Constant.java:29` `interval`; `Exponential.java:30,33,35` `interval`,
/// `maxInterval`, `delayFactor`; `Random.java:30,33` `minInterval`,
/// `maxInterval`). `None` for a `type` Kestra has no retry for.
pub(crate) fn retry_keys(ty: &str) -> Option<Vec<&'static str>> {
    let own: &[&str] = match ty {
        "constant" => &["interval"],
        "exponential" => &["interval", "maxInterval", "delayFactor"],
        "random" => &["minInterval", "maxInterval"],
        _ => return None,
    };
    Some(RETRY_COMMON_KEYS.iter().chain(own).copied().collect())
}

/// What an `sla:` entry of this `type` has: `SLA.java`'s `id` :42, `type` :45,
/// `behavior` :48, `labels` :53, plus `MaxDurationSLA.java:25` `duration` or
/// `ExecutionAssertionSLA.java:27-28` `assert` (`@JsonProperty("assert")`).
/// `None` for a `type` Kestra has no SLA for.
pub(crate) fn sla_keys(ty: &str) -> Option<Vec<&'static str>> {
    let own = match ty {
        "MAX_DURATION" => "duration",
        "EXECUTION_ASSERTION" => "assert",
        _ => return None,
    };
    Some(vec!["id", "type", "behavior", "labels", own])
}

/// The 20 `InputType` wire names (`models/input.rs:19-41`), the `type:`
/// strings Kestra has one class for each of.
pub(crate) const INPUT_TYPES: [&str; 20] = [
    "STRING",
    "INT",
    "FLOAT",
    "BOOL",
    "DATETIME",
    "DATE",
    "TIME",
    "DURATION",
    "FILE",
    "JSON",
    "YAML",
    "URI",
    "SECRET",
    "SELECT",
    "MULTISELECT",
    "ARRAY",
    "EMAIL",
    "ION",
    "FORM",
    "REUSABLE_INPUTS",
];

/// The keys every Kestra input class has (`Input.java:62-98`) and egret's
/// `Input` reads.
const INPUT_COMMON_KEYS: &[&str] = &[
    "id",
    "type",
    "description",
    "required",
    "defaults",
    "prefill",
    "displayName",
    "dependsOn",
];

/// What egret reads on an input of this `InputType` wire name: the common keys
/// plus the subtype's own that egret keeps. One egret struct serves all twenty
/// Kestra classes, so `itemType` and `allowedFileExtensions` are listed on the
/// types Kestra has them on only (spec 3.5). The keys Kestra has on a type and
/// egret drops are not here: [`lift_of`] names their package.
pub(crate) fn input_keys(ty: &str) -> Vec<&'static str> {
    let mut keys = INPUT_COMMON_KEYS.to_vec();
    match ty {
        "ARRAY" => keys.push("itemType"),
        "FILE" => keys.push("allowedFileExtensions"),
        "STRING" | "SECRET" => keys.push("validator"),
        "INT" | "FLOAT" | "DURATION" => keys.extend(["min", "max"]),
        "DATE" | "DATETIME" | "TIME" => keys.extend(["after", "before"]),
        "SELECT" => keys.extend([
            "values",
            "expression",
            "allowCustomValue",
            "isRadio",
            "autoSelectFirst",
        ]),
        "MULTISELECT" => keys.extend([
            "itemType",
            "values",
            "expression",
            "allowCustomValue",
            "autoSelectFirst",
        ]),
        _ => {}
    }
    keys
}

/// The fields a core leaf task's handler deserializes, or `None` for a type
/// with no such handler.
pub(crate) fn leaf_keys(t: &str) -> Option<Vec<&'static str>> {
    use h::storage::items;
    Some(match t {
        h::log::TYPE_ID => wire_fields_of::<h::log::Spec>(),
        h::ret::TYPE_ID => wire_fields_of::<h::ret::Spec>(),
        h::fail::TYPE_ID => wire_fields_of::<h::fail::Spec>(),
        h::sleep::TYPE_ID => wire_fields_of::<h::sleep::Spec>(),
        // Group C1 Task 2.
        h::output_values::TYPE_ID => wire_fields_of::<h::output_values::Spec>(),
        // Group C1 Task 3.
        h::assert::TYPE_ID => wire_fields_of::<h::assert::Spec>(),
        // Group C1 Task 4.
        h::purge_logs::TYPE_ID => wire_fields_of::<h::purge_logs::Spec>(),
        h::storage::WRITE => wire_fields_of::<h::storage::WriteSpec>(),
        h::storage::SIZE => wire_fields_of::<h::storage::UriSpec>(),
        h::storage::DELETE => wire_fields_of::<h::storage::DeleteSpec>(),
        // Reads no property: it purges the running execution's own files.
        h::storage::PURGE_CURRENT_EXECUTION_FILES => Vec::new(),
        h::storage::CONCAT => wire_fields_of::<items::ConcatSpec>(),
        h::storage::SPLIT => wire_fields_of::<items::SplitSpec>(),
        h::storage::REVERSE => wire_fields_of::<items::ReverseSpec>(),
        h::storage::DEDUPLICATE_ITEMS => wire_fields_of::<items::DeduplicateSpec>(),
        h::storage::FILTER_ITEMS => wire_fields_of::<items::FilterSpec>(),
        h::namespace::UPLOAD_FILES => wire_fields_of::<h::namespace::UploadSpec>(),
        h::namespace::DOWNLOAD_FILES => wire_fields_of::<h::namespace::DownloadSpec>(),
        h::namespace::DELETE_FILES => wire_fields_of::<h::namespace::DeleteSpec>(),
        h::namespace::PURGE_FILES => wire_fields_of::<h::namespace::PurgeSpec>(),
        h::kv::SET => wire_fields_of::<h::kv::SetSpec>(),
        h::kv::GET => wire_fields_of::<h::kv::GetSpec>(),
        h::kv::GET_KEYS => wire_fields_of::<h::kv::GetKeysSpec>(),
        h::kv::DELETE => wire_fields_of::<h::kv::DeleteSpec>(),
        h::kv::PUT => wire_fields_of::<h::kv::PutSpec>(),
        h::kv::PURGE_KV => wire_fields_of::<h::kv::PurgeSpec>(),
        // Kill-and-exit Task 3: no handler -- the executor runs it -- and
        // `build_plan` reads its leaf spec through this struct.
        crate::executor::exit::TYPE_ID => wire_fields_of::<crate::executor::exit::Spec>(),
        // Group C2 Task 5: executor-level as `Exit` is; the worker's render
        // pass reads each leaf spec through these structs (`executor::update`).
        crate::executor::update::SET_VARIABLES => {
            wire_fields_of::<crate::executor::update::SetSpec>()
        }
        crate::executor::update::UNSET_VARIABLES => {
            wire_fields_of::<crate::executor::update::UnsetSpec>()
        }
        crate::executor::update::LABELS => wire_fields_of::<crate::executor::update::LabelsSpec>(),
        _ => return None,
    })
}

/// Read only by the measuring instrument (`schema::measure`, test-only).
#[cfg(test)]
pub(crate) fn flowable_keys(t: &str) -> Vec<&'static str> {
    match t {
        "io.kestra.plugin.core.flow.Sequential" => wire_fields_of::<models::Sequential>(),
        "io.kestra.plugin.core.flow.Parallel" => wire_fields_of::<models::Parallel>(),
        "io.kestra.plugin.core.flow.If" => wire_fields_of::<models::If>(),
        "io.kestra.plugin.core.flow.Switch" => wire_fields_of::<models::Switch>(),
        "io.kestra.plugin.core.flow.Dag" => wire_fields_of::<models::Dag>(),
        "io.kestra.plugin.core.flow.Loop" => wire_fields_of::<models::Loop>(),
        "io.kestra.plugin.core.flow.LoopUntil" => wire_fields_of::<models::LoopUntil>(),
        "io.kestra.plugin.core.flow.Subflow" => wire_fields_of::<models::Subflow>(),
        "io.kestra.plugin.core.flow.AllowFailure" => wire_fields_of::<models::AllowFailure>(),
        "io.kestra.plugin.core.flow.WorkingDirectory" => {
            wire_fields_of::<models::WorkingDirectory>()
        }
        "io.kestra.plugin.core.flow.Pause" => wire_fields_of::<models::Pause>(),
        other => panic!("no typed model for flowable {other}"),
    }
}

pub(crate) fn trigger_keys(t: &str) -> Vec<&'static str> {
    match t {
        "io.kestra.plugin.core.trigger.Schedule" => wire_fields_of::<models::Schedule>(),
        "io.kestra.plugin.core.trigger.Webhook" => wire_fields_of::<models::Webhook>(),
        "io.kestra.plugin.core.trigger.Flow" => wire_fields_of::<models::FlowTrigger>(),
        other => panic!("no typed model for trigger {other}"),
    }
}

// ---------- the save-time check ----------
//
// Absorbs KI-121's `spec/properties.rs` (spec D17). The model is not
// `deny_unknown_fields` (a Kestra flow pasted in must parse), so serde drops a
// key it has no field for: a typo (`mesage:`), a key no Kestra class has, or a
// property Kestra documents that egret does not act on. This walk reads the RAW
// document, where the keys are still there. For now the list to check against
// is the committed measurement ([`crate::spec::schema::property_treatment`]);
// Tasks 3-7 move each site onto the key tables above.
//
// - a key the object's row does not list at all: `unknown_property`, an ERROR
//   (Kestra's Jackson refuses an unknown name; KI-121 only warned);
// - a key the row measures `ignored`: `not_honoured`, a warning that names the
//   package that lifts it (`lift_of`) when one is known.
//
// `honoured` and `warned` keys say nothing here (a `warned` one already has its
// `not_yet_enforced` warning), nor do `refused` ones, which a validation error
// or a publish gate already names. An object whose type resolves to no row (an
// egret-only type, a plugin whose row is named otherwise) is not checked.
//
// Objects checked: the flow document, every task at every depth (paths are
// `spec::path`'s, `tasks.a.b`), a task's or the flow's `retry`, every trigger.

/// Webhook keys whose use the route refuses with a 409 (`webhook::Unsupported`).
const WEBHOOK_REFUSED: &[&str] = &[
    "wait",
    "returnOutputs",
    "responseContentType",
    "responseCode",
];

#[derive(Clone, Copy)]
enum Kind {
    Input,
    Output,
}

/// The flow's four root lists, as `spec::path::root_lists` names them.
const ROOT_LISTS: &[&str] = &["tasks", "errors", "finally", "afterExecution"];

/// The save-time key census (spec D11): the keys of `source` that serde
/// dropped. Called only by [`crate::spec::validate_source`]; the run-time gate
/// never reaches it. `source` has already parsed as a [`Flow`]; if it does not
/// parse as a mapping here, the census adds nothing (it fails open, never a
/// false refusal).
pub(crate) fn findings(source: &str, model: &Flow, report: &mut ValidationReport) {
    after_execution_retry(model, report);
    let doc = serde_norway::from_str::<Value>(source);
    debug_assert!(
        doc.is_ok(),
        "a source that parsed as a Flow parses as a Value"
    );
    let Ok(Value::Object(flow)) = doc else {
        return;
    };
    Walk {
        report,
        seen: Seen::default(),
        flow: model,
        reached: Vec::new(),
    }
    .flow(&flow);
}

/// End hooks #2b (spec D11): `afterExecution:` runs once the execution has
/// ended and is never retried, so a `retry:` that reaches one of its tasks is
/// accepted and not acted on. A flow-level `retry:` reaches them when the
/// list is non-empty. Save and publish only, because the run-time gate must
/// not warn (package #1). Read from the model: `retry:` is typed on every
/// task, so this needs no source keys.
fn after_execution_retry(flow: &Flow, report: &mut ValidationReport) {
    const MESSAGE: &str = "an afterExecution task is not retried on egret: the execution has already ended. Roadmap package #13 lifts it";
    fn walk(task: &crate::models::Task, path: String, report: &mut ValidationReport) {
        if task.retry.is_some() {
            report.warnings.push(Finding {
                code: "not_honoured",
                message: MESSAGE.to_string(),
                path: format!("{path}.retry"),
            });
        }
        for child in task.children().into_iter().chain(task.error_children()) {
            walk(child, super::path::task_path(&path, &child.id), report);
        }
    }
    if flow.retry.is_some() && !flow.after_execution.is_empty() {
        report.warnings.push(Finding {
            code: "not_honoured",
            message: MESSAGE.to_string(),
            path: "retry".to_string(),
        });
    }
    for task in &flow.after_execution {
        walk(
            task,
            super::path::task_path("afterExecution", &task.id),
            report,
        );
    }
}

/// What the walk of `source` examined (the corpus test's floors).
#[cfg(test)]
pub(crate) fn seen(source: &str, flow: &Flow) -> Seen {
    let Ok(Value::Object(doc)) = serde_norway::from_str::<Value>(source) else {
        return Seen::default();
    };
    let mut scratch = ValidationReport::default();
    let mut walk = Walk {
        report: &mut scratch,
        seen: Seen::default(),
        flow,
        reached: Vec::new(),
    };
    walk.flow(&doc);
    walk.seen
}

/// The path of every model task that the raw walk of `source` does not reach
/// (a raw entry missing, or a different `id` at that place). The walk fails
/// open there, so this must be empty for every corpus flow. Read only by the
/// corpus test.
#[cfg(test)]
pub(crate) fn unpaired(source: &str, flow: &Flow) -> Vec<String> {
    let Ok(Value::Object(doc)) = serde_norway::from_str::<Value>(source) else {
        return crate::spec::path::every_task(flow)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
    };
    let mut scratch = ValidationReport::default();
    let mut walk = Walk {
        report: &mut scratch,
        seen: Seen::default(),
        flow,
        reached: Vec::new(),
    };
    walk.flow(&doc);
    let reached = walk.reached;
    crate::spec::path::every_task(flow)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| !reached.contains(p))
        .collect()
}

/// The raw lists a flowable holds child tasks in, in `Task::children()` then
/// `Task::error_children()` order (`models/task.rs:122-201`), keyed by the
/// canonical type. `errors` and `finally` appear where the model has them (A2). Every other type holds none: a script or
/// plugin property that merely looks like a task list is never walked.
fn child_lists(canonical: &str) -> &'static [&'static str] {
    match canonical {
        "io.kestra.plugin.core.flow.Sequential" | "io.kestra.plugin.core.flow.Parallel" => {
            &["tasks", "errors", "finally"]
        }
        "io.kestra.plugin.core.flow.If" => &["then", "else", "errors", "finally"],
        "io.kestra.plugin.core.flow.Switch" => &["cases", "defaults", "errors", "finally"],
        "io.kestra.plugin.core.flow.Dag" => &["tasks", "errors", "finally"],
        "io.kestra.plugin.core.flow.Loop"
        | "io.kestra.plugin.core.flow.LoopUntil"
        | "io.kestra.plugin.core.flow.WorkingDirectory" => &["tasks", "errors", "finally"],
        "io.kestra.plugin.core.flow.AllowFailure" => &["tasks", "finally", "errors"],
        "io.kestra.plugin.core.flow.Pause" => &["tasks", "onPause", "errors", "finally"],
        _ => &[],
    }
}

/// Kestra's runnable-only `Task.java` properties (`FlowService.warnings`,
/// `FlowService.java:574-582`): said once on a task that is not runnable, by
/// [`non_runnable_properties`], so the generic #13 / Enterprise `not_honoured`
/// warning for `taskCache` and `workerSelector` is not said a second time there.
const RUNNABLE_ONLY: &[&str] = &["timeout", "taskCache", "workerSelector"];

/// A task type Kestra's worker never runs (`!(task instanceof RunnableTask)`):
/// the 11 flowables, and the executor-level `Exit`, `SetVariables`,
/// `UnsetVariables` and `Labels`.
///
/// **Measured 2026-10-26: egret enforces `timeout` on none of them, so none is
/// left out of the `timeout` warning** (the warning is true for egret too).
/// `claim::leaf` (`worker/claim.rs:425-432`) hands a budget only to a
/// `TaskKind::Leaf`, so a `WorkingDirectory` or `AllowFailure` block's claim
/// resolves none (it is `NotALeaf`, `claim.rs:278-296`, and
/// `worker/working_dir.rs` never reads `timeout`); `executor/exit.rs` and
/// `executor/update.rs` never read it; `build_plan` copies a `timeout` into the
/// plan for a `Pause` alone (`executor/plan.rs:1438`, enforced at
/// `executor/flowable/pause.rs:243`), and a `Pause` is the type Kestra itself
/// exempts.
fn is_non_runnable(canonical: &str) -> bool {
    FLOWABLE_TYPES.iter().any(|(c, _)| *c == canonical)
        || matches!(
            canonical,
            crate::executor::exit::TYPE_ID
                | crate::executor::update::SET_VARIABLES
                | crate::executor::update::UNSET_VARIABLES
                | crate::executor::update::LABELS
        )
}

/// A flowable's own keys that #1 says nothing about. `finally` and `errors`
/// are model keys now (the walker descends into them, and a `Pause` `finally`
/// is `executor::pause_gap`'s alone); a `Pause` `tasks:` is `pause_gap`'s
/// (D14), so it is not reported a second time; `revision`, `updated` and
/// `tenantId` are server-managed and Kestra ignores what an author writes
/// (D15). `pluginDefaults` keeps its own `removed_plugin_defaults`
/// (`validate::save_only_findings`, spec 3.1).
fn silent(site: Option<Site<'_>>, key: &str) -> bool {
    match site {
        Some(Site::Flow) => matches!(key, "revision" | "updated" | "tenantId" | "pluginDefaults"),
        Some(Site::Task(t)) if FLOWABLE_TYPES.iter().any(|(c, _)| *c == t) => {
            matches!(key, "taskCache" | "workerSelector")
                || (t == "io.kestra.plugin.core.flow.Pause" && key == "tasks")
        }
        _ => false,
    }
}

/// Keys of Kestra's newest release (rc13), not at the pin: `Parallel` and
/// `Dag` `onChildFailure` is an unknown property here (D12).
fn newer_than_the_pin(site: Option<Site<'_>>, key: &str) -> bool {
    matches!(
        site,
        Some(Site::Task(
            "io.kestra.plugin.core.flow.Parallel" | "io.kestra.plugin.core.flow.Dag"
        ))
    ) && key == "onChildFailure"
}

/// What a walk examined, for the corpus test's floors: a walk that visits
/// nothing finds nothing, and a count says it was not vacuous.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Seen {
    pub triggers: usize,
    pub inputs: usize,
    pub outputs: usize,
    /// Nested beans checked: retries, concurrency, SLAs, checks,
    /// `checkFrequency`, Dag nodes and the two `dependsOn` kinds.
    pub beans: usize,
}

struct Walk<'a> {
    report: &'a mut ValidationReport,
    /// What the walk examined so far.
    seen: Seen,
    /// The parsed model of the same source, for the predicates that read it
    /// (`webhook::declared_unsupported`).
    flow: &'a Flow,
    /// The path of every task the walk reached (for [`unpaired`]).
    reached: Vec<String>,
}

impl Walk<'_> {
    fn flow(&mut self, flow: &serde_json::Map<String, Value>) {
        self.check(FLOW_ROW, Site::Flow, "the flow", flow, "");
        if let Some(retry) = flow.get("retry") {
            self.retry(retry, "retry");
        }
        self.flow_beans(flow);
        for list in ROOT_LISTS {
            if let Some(Value::Array(tasks)) = flow.get(*list) {
                for task in tasks {
                    self.task(task, list);
                }
            }
        }
        if let Some(Value::Array(triggers)) = flow.get("triggers") {
            for trigger in triggers {
                let Some(t) = trigger.as_object() else {
                    continue;
                };
                let Some(id) = t.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(row) = t.get("type").and_then(Value::as_str).and_then(trigger_row) {
                    self.seen.triggers += 1;
                    self.trigger(row, id, t);
                }
            }
        }
        for (list, kind) in [("inputs", Kind::Input), ("outputs", Kind::Output)] {
            if let Some(Value::Array(items)) = flow.get(list) {
                for item in items {
                    let Some(o) = item.as_object() else { continue };
                    let Some(id) = o.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    match kind {
                        Kind::Input => {
                            self.seen.inputs += 1;
                            self.input(id, o);
                        }
                        Kind::Output => {
                            self.seen.outputs += 1;
                            self.output(id, o);
                        }
                    }
                }
            }
        }
    }

    /// A typed trigger's keys (spec 3.4). Read: the base keys and the kind's
    /// own. A key Kestra has and egret does not act on warns with its package
    /// (`lift_of`); any other is `unknown_property`. Three families are said
    /// by the predicate that refuses them, so each key has one finding:
    /// Schedule `when` and `lateMaximumDelay` by `validate`'s
    /// `schedule_findings` (`ScheduleTrigger::resolve`), Webhook `wait`,
    /// `returnOutputs`, `responseContentType` and `responseCode` below
    /// (`webhook::declared_unsupported`). Neither predicate renders anything:
    /// they read the parsed model only (spec 7).
    fn trigger(&mut self, row: &str, id: &str, t: &serde_json::Map<String, Value>) {
        let allowed = |key: &str| {
            TRIGGER_BASE_KEYS.contains(&key)
                || trigger_keys(row).contains(&key)
                // `Schedule.interval` is `final ... = null` in Kestra: accepted, inert.
                || (row == SCHEDULE && key == "interval")
        };
        let path = format!("triggers.{id}");
        for (key, value) in t {
            let key = key.as_str();
            if (row == SCHEDULE && matches!(key, "when" | "lateMaximumDelay"))
                || (row == WEBHOOK && WEBHOOK_REFUSED.contains(&key))
            {
                continue;
            }
            // Kestra's default is `false`; only a written `false` diverges (D16)
            // from egret, which never locks.
            if key == "allowConcurrent" && *value != Value::Bool(false) {
                continue;
            }
            if let Some(lift) = lift_of(Site::Trigger(row), key) {
                let said = if key == "allowConcurrent" {
                    "egret never locks a trigger while its execution runs; "
                } else {
                    ""
                };
                self.report.warnings.push(Finding {
                    code: "not_honoured",
                    message: format!(
                        "trigger '{id}': property '{key}' is accepted by Kestra on {row} but egret does not act on it yet: {said}{}",
                        lift.sentence()
                    ),
                    path: format!("{path}.{key}"),
                });
            } else if !allowed(key) {
                self.report.errors.push(Finding {
                    code: "unknown_property",
                    message: format!(
                        "trigger '{id}': unknown property '{key}' on {row}: neither Kestra nor egret accepts it"
                    ),
                    path: format!("{path}.{key}"),
                });
            }
        }
        if row == WEBHOOK {
            self.webhook_refusals(id, &path);
        }
        // A Flow trigger's `dependsOn` entries are `Flow.Dependency`
        // (`k/core/plugin/core/trigger/Flow.java:364-386`); `flowRef` is
        // egret's own, filled at publish, and not a key Kestra has. `window`
        // is `deny_unknown_fields` already and is left to it.
        if row == FLOW_TRIGGER {
            self.unfiltered_flow_trigger(id, t, &path);
            if let Some(Value::Array(deps)) = t.get("dependsOn") {
                let read: Vec<&str> = wire_fields_of::<models::Dependency>()
                    .into_iter()
                    .filter(|k| *k != "flowRef")
                    .collect();
                for (i, dep) in deps.iter().enumerate() {
                    let Some(o) = dep.as_object() else { continue };
                    let at = format!("{path}.dependsOn[{i}]");
                    self.bean(
                        &format!("trigger '{id}': dependsOn[{i}]"),
                        &at,
                        o,
                        &read,
                        None,
                    );
                }
            }
        }
    }

    /// Kestra's warning for a Flow trigger that listens to everything
    /// (`FlowService.java:557-568`): no `dependsOn` entry, and `when` absent or
    /// the literal `"true"`. Kestra's text (`FlowService.java:564`) ends "for
    /// EVERY execution of EVERY flow on your instance"; egret's listeners hear
    /// the executions of their own workspace only (`flow_trigger/process.rs:572-578`,
    /// the listener query, `WHERE l.workspace_id = $1`), so "on your instance"
    /// is "in this workspace" here, the one word-group that differs.
    fn unfiltered_flow_trigger(
        &mut self,
        id: &str,
        t: &serde_json::Map<String, Value>,
        path: &str,
    ) {
        let no_depends_on = match t.get("dependsOn") {
            None | Some(Value::Null) => true,
            Some(Value::Array(deps)) => deps.is_empty(),
            Some(_) => false,
        };
        let unfiltered_when = match t.get("when") {
            None | Some(Value::Null) => true,
            Some(Value::String(w)) => w == "true",
            Some(Value::Bool(b)) => *b,
            Some(_) => false,
        };
        if no_depends_on && unfiltered_when {
            self.report.warnings.push(Finding {
                code: "flow_trigger_without_depends_on",
                message: format!(
                    "This flow will be triggered for EVERY execution of EVERY flow in this workspace. We recommend adding the dependsOn property to the Flow trigger '{id}'."
                ),
                path: path.to_string(),
            });
        }
    }

    /// Webhook `wait`, `returnOutputs`, `responseContentType`, `responseCode`:
    /// the route answers 409 to every call that reaches one
    /// (`webhook::declared_unsupported`). `declared_unsupported` names the first
    /// only, so each key is asked on a copy that declares that key alone.
    fn webhook_refusals(&mut self, id: &str, path: &str) {
        use crate::models::{TriggerKind, Webhook};
        use crate::trigger::webhook::declared_unsupported;
        let Some(TriggerKind::Webhook(w)) = self
            .flow
            .triggers
            .iter()
            .find(|t| t.id == id)
            .map(|t| &t.kind)
        else {
            return;
        };
        let blank = Webhook {
            wait: false,
            return_outputs: false,
            response_content_type: None,
            response_code: None,
            ..w.clone()
        };
        let alone: [(&str, Webhook); 4] = [
            (
                "wait",
                Webhook {
                    wait: w.wait,
                    ..blank.clone()
                },
            ),
            (
                "returnOutputs",
                Webhook {
                    return_outputs: w.return_outputs,
                    ..blank.clone()
                },
            ),
            (
                "responseContentType",
                Webhook {
                    response_content_type: w.response_content_type.clone(),
                    ..blank.clone()
                },
            ),
            (
                "responseCode",
                Webhook {
                    response_code: w.response_code.clone(),
                    ..blank
                },
            ),
        ];
        for (key, only) in alone {
            if let Some(u) = declared_unsupported(&only) {
                self.report.warnings.push(Finding {
                    code: "not_honoured",
                    message: format!(
                        "{}; every call to this webhook answers 409. {}",
                        u.message(id),
                        "Roadmap package #11 lifts it"
                    ),
                    path: format!("{path}.{key}"),
                });
            }
        }
    }

    /// An input's keys (spec 3.5): [`input_keys`] for its type are read; a key
    /// Kestra has on that type and egret drops warns with its package; any
    /// other, including a key of another input subtype, is `unknown_property`.
    fn input(&mut self, id: &str, input: &serde_json::Map<String, Value>) {
        let Some(ty) = input
            .get("type")
            .and_then(Value::as_str)
            .and_then(|t| INPUT_TYPES.iter().find(|n| **n == t))
        else {
            return;
        };
        // `dependsOn` is `DependsOn(inputs, condition)` (`k/core/models/flows/DependsOn.java:15-17`).
        if let Some(Value::Object(dep)) = input.get("dependsOn") {
            let read = wire_fields_of::<models::DependsOn>();
            let at = format!("inputs.{id}.dependsOn");
            self.bean(&format!("input '{id}': dependsOn"), &at, dep, &read, None);
        }
        let read = input_keys(ty);
        for key in input.keys().map(String::as_str) {
            if read.contains(&key) {
                continue;
            }
            let at = format!("inputs.{id}.{key}");
            if let Some(lift) = lift_of(Site::Input(ty), key) {
                self.report.warnings.push(Finding {
                    code: "not_honoured",
                    message: format!(
                        "input '{id}': property '{key}' is accepted by Kestra on a {ty} input but egret does not act on it yet; {}",
                        lift.sentence()
                    ),
                    path: at,
                });
            } else {
                self.report.errors.push(Finding {
                    code: "unknown_property",
                    message: format!(
                        "input '{id}': unknown property '{key}' on a {ty} input: neither Kestra nor egret accepts it"
                    ),
                    path: at,
                });
            }
        }
    }

    /// A flow output's keys: `FlowOutput` has all six Kestra fields
    /// (`Output.java`); any other key is `unknown_property`.
    fn output(&mut self, id: &str, output: &serde_json::Map<String, Value>) {
        let read = wire_fields_of::<models::FlowOutput>();
        for key in output.keys().map(String::as_str) {
            if !read.contains(&key) {
                self.report.errors.push(Finding {
                    code: "unknown_property",
                    message: format!(
                        "output '{id}': unknown property '{key}': neither Kestra nor egret accepts it"
                    ),
                    path: format!("outputs.{id}.{key}"),
                });
            }
        }
    }

    fn task(&mut self, task: &Value, prefix: &str) {
        let Some(t) = task.as_object() else { return };
        let Some(id) = t.get("id").and_then(Value::as_str) else {
            return;
        };
        let path = crate::spec::path::task_path(prefix, id);
        let ty = t.get("type").and_then(Value::as_str);
        let row = ty.and_then(task_row);
        let is_plugin = ty.is_some_and(|ty| ty.starts_with(crate::plugins::PREFIX));
        match row {
            // A core leaf is checked against the keys its handler reads (spec
            // 3.2), which say more than the committed table: a refined finding
            // replaces the table's for the same key, never a second one.
            Some(row) if leaf_keys(row).is_some() => self.core_leaf(row, id, t, &path),
            // Script and plugin tasks keep their own gates
            // (`script_property_unknown`, `plugin_property_unknown`; spec D13).
            Some(row) if crate::scripts::kind_of(row).is_some() || is_plugin => {}
            Some(row) => self.check(row, Site::Task(row), &format!("task '{id}'"), t, &path),
            None => {}
        }
        if let Some(retry) = t.get("retry") {
            self.retry(retry, &format!("{path}.retry"));
        }
        if let Some(row) = row.filter(|r| is_non_runnable(r)) {
            self.non_runnable_properties(row, id, t, &path);
        }
        self.reached.push(path.clone());
        let Some(row) = row else { return };
        // `LoopUntil.CheckFrequency` (`LoopUntil.java:245-264`).
        if row == "io.kestra.plugin.core.flow.LoopUntil" {
            if let Some(Value::Object(freq)) = t.get("checkFrequency") {
                let read = wire_fields_of::<models::CheckFrequency>();
                let at = format!("{path}.checkFrequency");
                self.bean(
                    &format!("task '{id}': checkFrequency"),
                    &at,
                    freq,
                    &read,
                    None,
                );
            }
        }
        for key in child_lists(row) {
            match t.get(*key) {
                Some(Value::Array(children)) => {
                    for (i, child) in children.iter().enumerate() {
                        // A `Dag` node holds its task under `task`
                        // (`Dag.DagTask`, `Dag.java:259-272`) and is a bean of
                        // its own, pathed as `validate` paths it
                        // (`{dag}.tasks[{i}]`).
                        match child.get("task") {
                            Some(inner) if child.get("type").is_none() => {
                                if let (true, Some(node)) =
                                    (row == "io.kestra.plugin.core.flow.Dag", child.as_object())
                                {
                                    let read = wire_fields_of::<models::DagNode>();
                                    let at = format!("{path}.{key}[{i}]");
                                    self.bean(
                                        &format!("task '{id}': dag node #{i}"),
                                        &at,
                                        node,
                                        &read,
                                        None,
                                    );
                                }
                                self.task(inner, &path)
                            }
                            _ => self.task(child, &path),
                        }
                    }
                }
                Some(Value::Object(cases)) if *key == "cases" => {
                    for children in cases.values() {
                        for child in children.as_array().into_iter().flatten() {
                            self.task(child, &path);
                        }
                    }
                }
                // `Pause.onPause` is one task.
                Some(single @ Value::Object(_)) if *key == "onPause" => self.task(single, &path),
                _ => {}
            }
        }
    }

    /// Kestra's own save-time warning for a runnable-only property on a task
    /// that is not runnable (`FlowService.java:574-582`, texts verbatim): `timeout`
    /// (not on a `Pause`, which Kestra exempts), `taskCache` and `workerSelector`.
    /// A key written `null` is absent for Kestra (`getX() != null`). One finding
    /// per key: the generic #13 / Enterprise warning is suppressed on these
    /// types (`silent`, `core_leaf`).
    fn non_runnable_properties(
        &mut self,
        canonical: &str,
        id: &str,
        task: &serde_json::Map<String, Value>,
        path: &str,
    ) {
        for key in RUNNABLE_ONLY {
            if task.get(*key).is_none_or(Value::is_null)
                || (*key == "timeout" && canonical == "io.kestra.plugin.core.flow.Pause")
            {
                continue;
            }
            self.report.warnings.push(Finding {
                code: "runnable_property_on_non_runnable",
                message: format!(
                    "The task '{id}' cannot use the '{key}' property as it's only relevant for runnable tasks."
                ),
                path: format!("{path}.{key}"),
            });
        }
    }

    /// A core leaf task's keys (spec 3.2): the handler's own and the task
    /// model's are read; `Task.java`'s are accepted by Kestra and not acted on
    /// (a warning naming the package that lifts them); any other key, `null`
    /// included, is a name neither Kestra nor egret has (an error, as Kestra's
    /// `Unrecognized field`, `YamlParser.java:144-157`). Both messages are
    /// egret's own.
    fn core_leaf(
        &mut self,
        canonical: &str,
        id: &str,
        task: &serde_json::Map<String, Value>,
        path: &str,
    ) {
        let Some(read) = leaf_keys(canonical) else {
            return;
        };
        for key in task.keys().map(String::as_str) {
            if read.contains(&key) || TASK_MODEL_KEYS.contains(&key) {
                continue;
            }
            let at = format!("{path}.{key}");
            if TASK_JAVA_UNREAD.contains(&key) {
                if is_non_runnable(canonical) && matches!(key, "taskCache" | "workerSelector") {
                    continue;
                }
                let lift = lift_of(Site::Task(canonical), key)
                    .expect("every TASK_JAVA_UNREAD key has a lift row");
                self.report.warnings.push(Finding {
                    code: "not_honoured",
                    message: format!(
                        "task '{id}': property '{key}' is accepted by Kestra on {canonical} but egret does not act on it yet; {}",
                        lift.sentence()
                    ),
                    path: at,
                });
            } else {
                self.report.errors.push(Finding {
                    code: "unknown_property",
                    message: format!(
                        "task '{id}': unknown property '{key}' on {canonical}: neither Kestra nor egret accepts it"
                    ),
                    path: at,
                });
            }
        }
    }

    /// A `retry` (task or flow), checked against the keys of its `type`
    /// ([`retry_keys`]). `warningOnRetry` is Kestra's (`AbstractRetry.java:37`):
    /// `true` ends a retried task that succeeds WARNING, which egret does not;
    /// `false` is the default, which egret's behaviour already is. Said once,
    /// in its own words.
    fn retry(&mut self, retry: &Value, path: &str) {
        let Some(r) = retry.as_object() else { return };
        let Some(read) = r.get("type").and_then(Value::as_str).and_then(retry_keys) else {
            return;
        };
        self.seen.beans += 1;
        for (key, value) in r {
            if key == "warningOnRetry" {
                if *value == Value::Bool(true) {
                    self.report.warnings.push(Finding {
                        code: "not_honoured",
                        message: format!(
                            "`retry.warningOnRetry` is accepted by Kestra but egret does not act on it yet: a retried task that succeeds ends SUCCESS, not WARNING; {}",
                            Lift::Package(13).sentence()
                        ),
                        path: format!("{path}.{key}"),
                    });
                }
                continue;
            }
            self.key_of(&read, "the retry", path, key, None);
        }
    }

    /// One nested bean: every key of `object` that `read` does not list is
    /// reported by [`Self::key_of`].
    fn bean(
        &mut self,
        what: &str,
        path: &str,
        object: &serde_json::Map<String, Value>,
        read: &[&str],
        site: Option<Site<'_>>,
    ) {
        self.seen.beans += 1;
        for key in object.keys() {
            self.key_of(read, what, path, key, site);
        }
    }

    /// `key` of a nested bean: read (silent), accepted by Kestra and not acted
    /// on (`lift_of` names the package: `not_honoured`), or unknown to both
    /// (`unknown_property`, as Kestra's Jackson refuses it). Both messages are
    /// egret's own and name the key, never a value (D10).
    fn key_of(&mut self, read: &[&str], what: &str, path: &str, key: &str, site: Option<Site<'_>>) {
        if read.contains(&key) {
            return;
        }
        let at = format!("{path}.{key}");
        match site.and_then(|s| lift_of(s, key)) {
            Some(lift) => self.report.warnings.push(Finding {
                code: "not_honoured",
                message: format!(
                    "{what}: property '{key}' is accepted by Kestra but egret does not act on it yet; {}",
                    lift.sentence()
                ),
                path: at,
            }),
            None => self.report.errors.push(Finding {
                code: "unknown_property",
                message: format!(
                    "{what}: unknown property '{key}': neither Kestra nor egret accepts it"
                ),
                path: at,
            }),
        }
    }

    /// The flow's `concurrency`, each `sla` and each `checks` entry
    /// (spec 3.7); `quotas` is skipped (an Enterprise warning already, and
    /// Kestra OSS does not read it).
    fn flow_beans(&mut self, flow: &serde_json::Map<String, Value>) {
        if let Some(Value::Object(c)) = flow.get("concurrency") {
            let read = wire_fields_of::<models::Concurrency>();
            self.bean("concurrency", "concurrency", c, &read, None);
        }
        if let Some(Value::Array(slas)) = flow.get("sla") {
            for (i, sla) in slas.iter().enumerate() {
                let Some(o) = sla.as_object() else { continue };
                let Some(read) = o.get("type").and_then(Value::as_str).and_then(sla_keys) else {
                    continue;
                };
                self.bean(&format!("sla[{i}]"), &format!("sla[{i}]"), o, &read, None);
            }
        }
        if let Some(Value::Array(checks)) = flow.get("checks") {
            let read = wire_fields_of::<models::Check>();
            for (i, check) in checks.iter().enumerate() {
                let Some(o) = check.as_object() else { continue };
                let path = format!("checks[{i}]");
                self.bean(&path.clone(), &path, o, &read, Some(Site::Check));
            }
        }
    }

    fn check(
        &mut self,
        row: &str,
        site: Site<'_>,
        what: &str,
        object: &serde_json::Map<String, Value>,
        path: &str,
    ) {
        let site = Some(site);
        let Some(props) = property_treatment().get(row) else {
            return;
        };
        for key in object.keys() {
            let at = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            if silent(site, key) {
                continue;
            }
            let treatment = if newer_than_the_pin(site, key) {
                None
            } else {
                props.get(key)
            };
            match treatment {
                None => self.report.errors.push(Finding {
                    code: "unknown_property",
                    message: format!(
                        "{what}: `{key}` is not a property of {}",
                        if row == FLOW_ROW { "a flow" } else { row }
                    ),
                    path: at,
                }),
                Some(Treatment::Ignored) => {
                    let lifted = site
                        .and_then(|s| lift_of(s, key))
                        .map(|l| format!("; {}", l.sentence()))
                        .unwrap_or_default();
                    self.report.warnings.push(Finding {
                        code: "not_honoured",
                        message: format!(
                            "{what}: `{key}` is accepted, but egret does not act on it{lifted}"
                        ),
                        path: at,
                    });
                }
                Some(Treatment::Honoured | Treatment::Warned | Treatment::Refused) => {}
            }
        }
    }
}

/// The row of a task `type`: a flowable's, a registry type's canonical
/// spelling (an alias resolves to it), or the type itself when the table
/// has a row of that name (a script task).
fn task_row(t: &str) -> Option<&'static str> {
    if let Some(row) = row_in(FLOWABLE_TYPES, t) {
        return Some(row);
    }
    if let Some(spec) = crate::spec::registry::known_type(t) {
        return Some(spec.r#type);
    }
    property_treatment()
        .get_key_value(t)
        .map(|(k, _)| k.as_str())
}

fn trigger_row(t: &str) -> Option<&'static str> {
    row_in(TRIGGER_TYPES, t)
}

fn row_in(
    table: &'static [(&'static str, &'static [&'static str])],
    t: &str,
) -> Option<&'static str> {
    table
        .iter()
        .find(|(canonical, aliases)| *canonical == t || aliases.contains(&t))
        .map(|(canonical, _)| *canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **One table names the package for every key** (spec D12). Spot rows from
    /// each site, so a row moved to the wrong package fails here by name.
    #[test]
    fn lift_of_names_the_package_the_roadmap_gives() {
        use Lift::*;
        let log = "io.kestra.plugin.core.log.Log";
        let sub = "io.kestra.plugin.core.flow.Subflow";
        assert_eq!(lift_of(Site::Task(log), "logLevel"), Some(Package(13)));
        assert_eq!(lift_of(Site::Task(log), "workerSelector"), Some(Enterprise));
        assert_eq!(lift_of(Site::Task(sub), "inheritLabels"), Some(Package(12)));
        assert_eq!(
            lift_of(Site::Task("io.kestra.plugin.core.flow.Loop"), "outputs"),
            Some(Package(6))
        );
        assert_eq!(
            lift_of(Site::Task("io.kestra.plugin.core.flow.Pause"), "onResume"),
            Some(Package(9))
        );
        assert_eq!(
            lift_of(
                Site::Trigger("io.kestra.plugin.core.trigger.Schedule"),
                "stopAfter"
            ),
            Some(Package(11))
        );
        assert_eq!(lift_of(Site::Input("FORM"), "inputs"), Some(Enterprise));
        assert_eq!(lift_of(Site::Input("SELECT"), "values"), None);
        assert_eq!(lift_of(Site::Flow, "policyRefs"), Some(Enterprise));
        // Not lifted by #1's table: honoured or unknown to Kestra.
        assert_eq!(lift_of(Site::Task(log), "message"), None);
        assert_eq!(
            lift_of(
                Site::Task("io.kestra.plugin.core.flow.Sequential"),
                "finally"
            ),
            None
        );
        assert_eq!(
            lift_of(
                Site::Task("io.kestra.plugin.core.flow.Parallel"),
                "onChildFailure"
            ),
            None
        );
        assert_eq!(lift_of(Site::Task(log), "invalid"), None);
    }

    /// The script and plugin rows Task 8 reads.
    #[test]
    fn lift_of_script_and_plugin_rows() {
        use Lift::*;
        let py = crate::scripts::PYTHON_SCRIPT;
        let sh = crate::scripts::SHELL_COMMANDS;
        assert_eq!(lift_of(Site::Task(sh), "runner"), Some(Package(17)));
        assert_eq!(lift_of(Site::Task(sh), "targetOS"), Some(Package(17)));
        assert_eq!(lift_of(Site::Task(py), "pythonVersion"), Some(Package(17)));
        assert_eq!(lift_of(Site::Task(sh), "pythonVersion"), None);
        assert_eq!(
            lift_of(Site::Task("kestra.io.x.Y"), "taskRunner"),
            Some(Package(18))
        );
        assert_eq!(
            lift_of(Site::Flow, "draft").map(|l| l.short().ends_with("(roadmap \u{a7}3.2)")),
            Some(true)
        );
    }

    #[test]
    fn lift_renders_both_forms() {
        assert_eq!(Lift::Package(13).short(), "roadmap package #13");
        assert_eq!(Lift::Package(13).sentence(), "roadmap package #13 lifts it");
        assert_eq!(
            Lift::Enterprise.sentence(),
            "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap \u{a7}3.3)"
        );
        assert_eq!(
            Lift::Enterprise.short(),
            "Kestra Enterprise only, ignored by Kestra OSS too, roadmap \u{a7}3.3"
        );
    }

    /// **The pin governs, and the rc13 export is the second instrument** (spec
    /// D12). For every typed row the committed schema export defines, each
    /// Kestra property is honoured by egret's model, lifted by `lift_of`, or one
    /// of the named differences. A Kestra property in none of the three is a
    /// silent drop this package missed.
    #[test]
    fn every_kestra_property_is_read_lifted_or_a_named_difference() {
        // (row, key, why) -- rc13 is newer than the pin (2026-09-01 vs 2026-08-17).
        const DIFFERENCES: &[(&str, &str, &str)] = &[
            (
                "io.kestra.plugin.core.flow.Parallel",
                "onChildFailure",
                "not at the pin; unknown_property",
            ),
            (
                "io.kestra.plugin.core.flow.Dag",
                "onChildFailure",
                "not at the pin; unknown_property",
            ),
        ];
        use crate::spec::schema::{kestra_definition, FLOWABLE_TYPES, TRIGGER_TYPES};
        let rows =
            FLOWABLE_TYPES
                .iter()
                .map(|(t, _)| (*t, Site::Task(t), flowable_keys(t)))
                .chain(TRIGGER_TYPES.iter().map(|(t, _)| {
                    (
                        *t,
                        Site::Trigger(t),
                        [trigger_keys(t), TRIGGER_BASE_KEYS.to_vec()].concat(),
                    )
                }))
                .chain(crate::spec::registry::TASK_TYPES.iter().filter_map(|s| {
                    leaf_keys(s.r#type).map(|k| (s.r#type, Site::Task(s.r#type), k))
                }));
        let mut missed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (row, site, egret) in rows {
            let Some(props) = kestra_definition(row).and_then(|d| d["properties"].as_object())
            else {
                continue;
            };
            for k in props.keys().map(String::as_str) {
                if egret.contains(&k) || TASK_MODEL_KEYS.contains(&k) || lift_of(site, k).is_some()
                {
                    continue;
                }
                if DIFFERENCES.iter().any(|(r, p, _)| *r == row && *p == k) {
                    seen.insert((row, k));
                    continue;
                }
                missed.push(format!("{row}.{k}"));
            }
        }
        assert!(
            missed.is_empty(),
            "Kestra properties egret neither reads nor lifts: {missed:#?}"
        );
        assert_eq!(
            seen.len(),
            DIFFERENCES.len(),
            "a DIFFERENCES row the export no longer has is stale"
        );
    }

    // ---------- the save-time check (moved from KI-121's `spec/properties.rs`) ----------

    /// `(path, code)` of the key census's findings for `source`, errors then
    /// warnings.
    fn census(source: &str) -> Vec<(String, &'static str)> {
        let flow = crate::spec::parse(source).expect("the test source parses");
        let mut report = ValidationReport::default();
        findings(source, &flow, &mut report);
        report
            .errors
            .into_iter()
            .chain(report.warnings)
            .map(|f| (f.path, f.code))
            .collect()
    }

    #[test]
    fn an_unknown_top_level_key_is_an_error() {
        let w = census("id: f\nnamespace: a.b\ncolour: blue\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        assert_eq!(w, vec![("colour".to_string(), "unknown_property")]);
    }

    /// Spec D1 / D17: KI-121 warned, #1 refuses, as Kestra's Jackson does.
    #[test]
    fn an_unknown_property_blocks_publish_by_being_an_error() {
        let source = "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    mesage: typo\n";
        let flow = crate::spec::parse(source).unwrap();
        let report = crate::spec::validate_source(source, &flow);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.code == "unknown_property" && e.path == "tasks.t.mesage"),
            "{report:?}"
        );
        assert!(
            report.warnings.iter().all(|w| w.code != "unknown_property"),
            "{report:?}"
        );
        // `validate` alone does not know the key: the census is the entry point's.
        assert!(crate::spec::validate(&flow).errors.is_empty());
    }

    #[test]
    fn an_unknown_task_property_is_an_error_at_any_depth() {
        let w = census(
            "id: f\nnamespace: a.b\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: t\n        type: core.log\n        message: hi\n        mesage: typo\n",
        );
        assert_eq!(
            w,
            vec![("tasks.outer.t.mesage".to_string(), "unknown_property")]
        );
    }

    /// A key Kestra accepts and egret ignores is a warning that names who
    /// lifts it (D2, D12): `onResume` is #9's, `logLevel` #13's.
    /// `warningOnRetry` is #13's too (Task 3).
    #[test]
    fn a_key_kestra_accepts_and_egret_ignores_is_not_honoured_and_names_its_package() {
        let source = "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    logLevel: INFO\n    retry:\n      type: constant\n      interval: PT1S\n      maxAttempts: 2\n      warningOnRetry: true\n  - id: gate\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n    onResume:\n      - id: ok\n        type: BOOLEAN\n";
        let flow = crate::spec::parse(source).unwrap();
        let report = crate::spec::validate_source(source, &flow);
        let got: Vec<_> = report
            .warnings
            .iter()
            .filter(|w| w.code == "not_honoured")
            .map(|w| (w.path.as_str(), w.message.as_str()))
            .collect();
        let at = |path: &str| {
            got.iter()
                .find(|(p, _)| *p == path)
                .unwrap_or_else(|| panic!("no not_honoured at {path}: {got:#?}"))
                .1
        };
        assert!(at("tasks.t.logLevel").contains("roadmap package #13 lifts it"));
        assert!(at("tasks.gate.onResume").contains("roadmap package #9 lifts it"));
        assert!(at("tasks.t.retry.warningOnRetry").contains("roadmap package #13 lifts it"));
        assert_eq!(got.len(), 3, "{got:#?}");
        assert!(report.warnings.iter().all(|w| w.code != "ignored_property"));
    }

    /// The save-time report every test below reads (spec D11).
    fn report(src: &str) -> ValidationReport {
        crate::spec::validate_source(src, &crate::spec::parse(src).unwrap())
    }

    /// `invalids/invalid-property`, Kestra's `Unrecognized field "invalid"`
    /// (`YamlParser.java:144-157`): refused at save, by name, at its path.
    #[test]
    fn an_unknown_key_on_a_core_task_is_refused() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n    invalid: x\n");
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "unknown_property")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert_eq!(e.path, "tasks.t.invalid");
        assert_eq!(e.message, "task 't': unknown property 'invalid' on io.kestra.plugin.core.debug.Return: neither Kestra nor egret accepts it");
        assert_eq!(r.errors.len(), 1, "one finding per key: {r:?}");
    }

    /// D13: a script task is gated by `script_property_unknown` and a plugin
    /// task by `plugin_property_unknown`, not by this walker. The W4 legacy
    /// aliases carry `image:`, which those gates accept and the property table
    /// has no row for.
    #[test]
    fn a_script_or_plugin_task_is_not_checked_for_unknown_keys_here() {
        for ty in [
            "egret.container",
            "io.kestra.plugin.scripts.shell.Commands",
            "egret.script.python",
            "kestra.some.Plugin",
        ] {
            let src = format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: {ty}\n    image: ubuntu\n"
            );
            let r = report(&src);
            assert!(
                r.errors.iter().all(|e| e.code != "unknown_property"),
                "{ty}: {r:?}"
            );
        }
    }

    /// A catalog plugin task has a property-treatment row, so the walker would
    /// resolve it; its typo and `taskRunner` belong to `plugin_property_unknown`
    /// and `plugin_property_not_implemented` (D13, `tests/plugins_gate.rs`).
    #[test]
    fn a_catalog_plugin_task_is_left_to_its_own_gate() {
        let ty = "kestra.io.kestra.plugin.jdbc.postgresql.Query";
        for key in [
            "urll: x",
            "taskRunner: {type: io.kestra.plugin.scripts.runner.docker.Docker}",
        ] {
            let src =
                format!("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: {ty}\n    {key}\n");
            let r = report(&src);
            assert!(
                r.errors.iter().all(|e| e.code != "unknown_property"),
                "{key}: {r:?}"
            );
        }
    }

    /// `pluginDefaults:` is refused once, as `removed_plugin_defaults`
    /// (spec 3.1), not also as an unknown flow property.
    #[test]
    fn plugin_defaults_is_refused_exactly_once() {
        let r = report("id: f\nnamespace: a.b\npluginDefaults: []\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n");
        assert_eq!(r.errors.len(), 1, "{r:?}");
        assert_eq!(r.errors[0].code, "removed_plugin_defaults", "{r:?}");
    }

    /// Kestra's `Log` has no `format` and its `Return` has no `message`
    /// (`Log.java:79,85`, `Return.java:91`): each key is refused on the other.
    #[test]
    fn a_log_key_on_a_return_and_a_return_key_on_a_log_are_refused() {
        for (ty, key) in [
            ("io.kestra.plugin.core.log.Log", "format"),
            ("io.kestra.plugin.core.debug.Return", "message"),
        ] {
            let r = report(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: {ty}\n    {key}: x\n"
            ));
            assert!(
                r.errors
                    .iter()
                    .any(|e| e.code == "unknown_property" && e.path == format!("tasks.t.{key}")),
                "{ty}: {r:?}"
            );
        }
    }

    /// The alias spelling is checked against the canonical row, and a typo of a
    /// real key (`mesage`) is the case the check exists for.
    #[test]
    fn a_typo_on_an_aliased_log_is_refused() {
        let r = report(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    mesage: hi\n",
        );
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "unknown_property" && e.path == "tasks.t.mesage")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(
            e.message.contains("on io.kestra.plugin.core.log.Log"),
            "{}",
            e.message
        );
    }

    #[test]
    fn an_unknown_top_level_key_is_refused_and_a_server_managed_one_is_not() {
        let r = report("id: f\nnamespace: a.b\nrevision: 7\nupdated: 2026-01-01T00:00:00Z\ntenantId: x\nlabelz: {a: b}\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        let unknown: Vec<_> = r
            .errors
            .iter()
            .filter(|e| e.code == "unknown_property")
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(unknown, ["labelz"]);
        assert!(r.warnings.iter().all(|w| w.code != "not_honoured"), "{r:?}");
    }

    #[test]
    fn flow_draft_and_worker_selector_warn_with_their_reason() {
        let r = report("id: f\nnamespace: a.b\ndraft: true\nworkerSelector: {tags: [x]}\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        assert!(r.errors.is_empty(), "{r:?}");
        let w: std::collections::BTreeMap<_, _> = r
            .warnings
            .iter()
            .filter(|w| w.code == "not_honoured")
            .map(|w| (w.path.as_str(), w.message.as_str()))
            .collect();
        assert!(w["draft"].ends_with("(roadmap \u{a7}3.2)"), "{w:?}");
        assert!(w["workerSelector"].contains("Kestra Enterprise"), "{w:?}");
        assert_eq!(w.len(), 2, "one finding per key: {w:?}");
    }

    /// D15 measured: a source `deleted: true` is persisted as a tombstone by
    /// Kestra (`AbstractJdbcFlowRepository.save`, `deleted` is a column
    /// generated from the stored JSON), so it is not silent here.
    #[test]
    fn flow_deleted_warns_because_kestra_acts_on_it() {
        let r = report("id: f\nnamespace: a.b\ndeleted: true\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        assert!(r.errors.is_empty(), "{r:?}");
        let w: Vec<_> = r
            .warnings
            .iter()
            .filter(|w| w.code == "not_honoured")
            .collect();
        assert_eq!(w.len(), 1, "{r:?}");
        assert_eq!(w[0].path, "deleted");
    }

    #[test]
    fn a_typo_inside_a_flowable_finally_is_refused_at_its_path() {
        only(
            &format!("{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{{id: a, type: core.log, message: m}}]\n    finally:\n      - id: z\n        type: core.log\n        message: m\n        mesage: typo\n"),
            "tasks.seq.z.mesage",
            "unknown_property",
        );
    }

    #[test]
    fn a_typo_inside_a_loop_errors_is_refused_at_its_path() {
        only(
            &format!("{HEAD}tasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{{id: a, type: core.log, message: m}}]\n    errors:\n      - id: e\n        type: core.log\n        message: m\n        mesage: typo\n"),
            "tasks.each.e.mesage",
            "unknown_property",
        );
    }

    #[test]
    fn finally_and_the_loops_errors_are_model_keys_now() {
        silent_census(&format!("{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{{id: a, type: core.log, message: m}}]\n    finally: [{{id: z, type: core.log, message: m}}]\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{{id: b, type: core.log, message: m}}]\n    errors: [{{id: e, type: core.log, message: m}}]\n"));
    }

    /// A `Pause` `finally:` is a model key to the walker; `pause_gap` alone
    /// refuses it, so the walker adds no finding of its own.
    #[test]
    fn a_pause_finally_draws_no_finding_from_the_walker() {
        silent_census(&format!("{HEAD}tasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    finally: [{{id: z, type: core.log, message: m}}]\n"));
    }

    #[test]
    fn flowable_keys_kestra_has_warn_with_their_package_and_others_are_refused() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: x.y\n    flowId: g\n    inheritLabels: true\n  - id: p\n    type: io.kestra.plugin.core.flow.Parallel\n    onChildFailure: FAILED\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n        mesage: typo\n");
        assert!(
            r.warnings.iter().any(|w| w.code == "not_honoured"
                && w.path == "tasks.s.inheritLabels"
                && w.message.ends_with("roadmap package #12 lifts it")),
            "{r:?}"
        );
        let unknown: std::collections::BTreeSet<_> = r
            .errors
            .iter()
            .filter(|e| e.code == "unknown_property")
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(
            unknown,
            std::collections::BTreeSet::from(["tasks.p.onChildFailure", "tasks.p.a.mesage"])
        );
        assert_eq!(r.errors.len(), 2, "one finding per key: {r:?}");
    }

    /// Loop `outputs`/`fetchType` are #6's; every flowable shares the
    /// `Task.java` keys (spec 3.3).
    #[test]
    fn loop_outputs_and_a_task_java_key_on_a_flowable_warn() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    outputs: {x: y}\n    fetchType: STORE\n    logLevel: INFO\n    tasks:\n      - id: c\n        type: core.log\n        message: c\n");
        assert!(r.errors.is_empty(), "{r:?}");
        let got: std::collections::BTreeMap<_, _> = r
            .warnings
            .iter()
            .filter(|w| w.code == "not_honoured")
            .map(|w| (w.path.as_str(), w.message.as_str()))
            .collect();
        assert!(
            got["tasks.l.outputs"].ends_with("roadmap package #6 lifts it"),
            "{got:?}"
        );
        assert!(
            got["tasks.l.fetchType"].ends_with("roadmap package #6 lifts it"),
            "{got:?}"
        );
        assert!(
            got["tasks.l.logLevel"].ends_with("roadmap package #13 lifts it"),
            "{got:?}"
        );
        assert_eq!(got.len(), 3, "{got:?}");
    }

    #[test]
    fn pause_on_resume_warns_and_pause_tasks_is_refused_once() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n");
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(
            r.warnings.iter().any(|w| w.path == "tasks.p.onResume"
                && w.message.ends_with("roadmap package #9 lifts it")),
            "{r:?}"
        );
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
        assert_eq!(
            r.errors
                .iter()
                .filter(|e| e.path == "tasks.p.tasks")
                .count(),
            1,
            "{r:?}"
        );
    }

    /// A plugin property that merely holds `{id, type}` objects is not a task
    /// list: the walk descends by kind, not by key name (spec 2.4).
    #[test]
    fn a_plugin_property_named_tasks_is_not_walked_as_tasks() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    errors:\n      - id: x\n        type: core.log\n        mesage: typo\n");
        // `errors` on a leaf is itself unknown; its content is not descended.
        assert_eq!(
            r.errors
                .iter()
                .filter(|e| e.code == "unknown_property")
                .count(),
            1,
            "{r:?}"
        );
    }

    /// `core_leaf`'s `.expect` cannot panic: every `TASK_JAVA_UNREAD` key has a
    /// lift row on every core leaf type (Task 3 review).
    #[test]
    fn every_task_java_unread_key_has_a_lift_row_on_every_core_leaf() {
        let mut checked = 0;
        for spec in crate::spec::registry::TASK_TYPES {
            let Some(read) = leaf_keys(spec.r#type) else {
                continue;
            };
            for key in TASK_JAVA_UNREAD {
                if read.contains(key) || TASK_MODEL_KEYS.contains(key) {
                    continue;
                }
                checked += 1;
                assert!(
                    lift_of(Site::Task(spec.r#type), key).is_some(),
                    "{} / {key}",
                    spec.r#type
                );
            }
        }
        assert!(checked > 100, "only {checked} pairs");
    }

    /// The instrument fails open, and no corpus flow reaches that branch: every
    /// model task of every corpus flow is reached by the raw walk.
    #[test]
    fn every_corpus_task_pairs_with_its_raw_map() {
        let root = format!("{}/oracle-exec/corpus", env!("CARGO_MANIFEST_DIR"));
        let mut unpaired = Vec::new();
        let mut tasks = 0usize;
        for dir in ["valids", "invalids"] {
            for entry in std::fs::read_dir(format!("{root}/{dir}")).unwrap() {
                let path = entry.unwrap().path();
                let src = std::fs::read_to_string(&path).unwrap();
                let Ok(flow) = crate::spec::parse(&src) else {
                    continue;
                };
                tasks += crate::spec::path::every_task(&flow).len();
                unpaired.extend(
                    super::unpaired(&src, &flow)
                        .into_iter()
                        .map(|p| format!("{}: {p}", path.display())),
                );
            }
        }
        assert!(unpaired.is_empty(), "{unpaired:#?}");
        // A zero would satisfy the assertion above while measuring nothing.
        assert!(tasks >= 300, "only {tasks} corpus tasks were walked");
        eprintln!("corpus tasks walked: {tasks}");
    }

    /// `null` is not a known key either (spec 3.2).
    #[test]
    fn a_null_key_is_unknown_not_skipped() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    ~: x\n");
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "unknown_property" && e.path == "tasks.t.~"),
            "{r:?}"
        );
    }

    /// Kestra accepts `logLevel` (`Task.java:77`); egret does not act on it, so
    /// it is a warning naming #13 and the flow still publishes.
    #[test]
    fn a_task_java_key_on_a_core_task_is_warned_with_its_package() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    logLevel: INFO\n");
        assert!(r.errors.is_empty(), "{r:?}");
        let w = r
            .warnings
            .iter()
            .find(|w| w.code == "not_honoured")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert_eq!(w.path, "tasks.t.logLevel");
        assert_eq!(w.message, "task 't': property 'logLevel' is accepted by Kestra on io.kestra.plugin.core.log.Log but egret does not act on it yet; roadmap package #13 lifts it");
        assert_eq!(r.warnings.len(), 1, "one finding per key: {r:?}");
    }

    /// An unknown key is refused at SAVE only: the run gate still plans it, so a
    /// version published before #1 keeps running (spec D1).
    #[test]
    fn an_unknown_key_does_not_stop_the_run_gate() {
        let src = "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n    invalid: x\n";
        assert!(crate::executor::build_plan(src).is_ok());
    }

    #[test]
    fn warning_on_retry_is_warned_on_a_task_and_on_the_flow() {
        let r = report("id: f\nnamespace: a.b\nretry:\n  type: constant\n  interval: PT1S\n  warningOnRetry: true\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      warningOnRetry: true\n");
        let got: std::collections::BTreeMap<_, _> = r
            .warnings
            .iter()
            .filter(|w| w.code == "not_honoured")
            .map(|w| (w.path.as_str(), w.message.as_str()))
            .collect();
        assert_eq!(got.len(), 2, "{r:?}");
        for path in ["retry.warningOnRetry", "tasks.t.retry.warningOnRetry"] {
            assert_eq!(got[path], "`retry.warningOnRetry` is accepted by Kestra but egret does not act on it yet: a retried task that succeeds ends SUCCESS, not WARNING; roadmap package #13 lifts it");
        }
    }

    /// `warningOnRetry: false` is Kestra's default: egret's behaviour is the
    /// same, so there is nothing to warn about.
    #[test]
    fn warning_on_retry_false_is_silent() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      warningOnRetry: false\n");
        assert!(r.warnings.iter().all(|w| w.code != "not_honoured"), "{r:?}");
    }

    /// An authored `flowRef` on a Subflow is not a Kestra property (the model
    /// field is internal): `unknown_property`, DB-free (Task 2 review).
    #[test]
    fn an_authored_flow_ref_on_a_subflow_is_unknown() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: a.b\n    flowId: g\n    flowRef: 00000000-0000-0000-0000-000000000001\n");
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "unknown_property" && e.path == "tasks.s.flowRef"),
            "{r:?}"
        );
    }

    #[test]
    fn an_unknown_trigger_property_is_an_error() {
        let w = census(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\ntriggers:\n  - id: s\n    type: core.schedule\n    cron: \"0 * * * *\"\n    crn: x\n",
        );
        assert_eq!(w, vec![("triggers.s.crn".to_string(), "unknown_property")]);
    }

    /// The flow's own `retry` is checked as a task's is (KI-121 covered it).
    #[test]
    fn an_unknown_flow_retry_property_is_an_error() {
        let w = census(
            "id: f\nnamespace: a.b\nretry:\n  type: constant\n  interval: PT1S\n  maxAttempts: 2\n  attempts: 3\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
        );
        assert_eq!(w, vec![("retry.attempts".to_string(), "unknown_property")]);
    }

    // ---------- Task 5: typed triggers, inputs and outputs (D6) ----------

    const LOG: &str = "tasks:\n  - id: t\n    type: core.log\n    message: hi\n";
    const SCHED: &str = "io.kestra.plugin.core.trigger.Schedule";
    const HOOK: &str = "io.kestra.plugin.core.trigger.Webhook";

    fn at<'a>(r: &'a ValidationReport, path: &str) -> Vec<&'a Finding> {
        r.errors
            .iter()
            .chain(&r.warnings)
            .filter(|f| f.path == path)
            .collect()
    }

    #[test]
    fn a_late_maximum_delay_warns_at_publish_that_the_schedule_will_not_fire() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: s\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    lateMaximumDelay: PT1H\n"));
        assert!(r.errors.is_empty(), "{r:?}");
        let w = r
            .warnings
            .iter()
            .find(|w| w.path == "triggers.s.lateMaximumDelay")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(
            w.message.contains("will not fire")
                && w.message.ends_with("Roadmap package #11 lifts it"),
            "{}",
            w.message
        );
        // One finding for the key: KI-122's `schedule_not_resolvable`, reworded.
        assert_eq!(w.code, "schedule_not_resolvable");
        assert_eq!(at(&r, "triggers.s.lateMaximumDelay").len(), 1, "{r:?}");
        assert!(at(&r, "triggers.s").is_empty(), "{r:?}");
    }

    #[test]
    fn a_schedule_when_and_a_late_maximum_delay_are_each_warned_once() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: s\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    when: \"{{{{ true }}}}\"\n    lateMaximumDelay: PT1H\n"));
        assert_eq!(at(&r, "triggers.s.when").len(), 1, "{r:?}");
        assert_eq!(at(&r, "triggers.s.lateMaximumDelay").len(), 1, "{r:?}");
        assert!(at(&r, "triggers.s.when")[0]
            .message
            .ends_with("Roadmap package #11 lifts it"));
        assert_eq!(
            r.warnings
                .iter()
                .filter(|w| w.code == "schedule_not_resolvable")
                .count(),
            2,
            "{r:?}"
        );
    }

    #[test]
    fn a_webhook_wait_warns_at_publish_that_every_call_answers_409() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: w\n    type: {HOOK}\n    key: k\n    wait: true\n"));
        assert!(r.errors.is_empty(), "{r:?}");
        let w = r
            .warnings
            .iter()
            .find(|w| w.path == "triggers.w.wait")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(
            w.message.contains("answers 409")
                && w.message.ends_with("Roadmap package #11 lifts it"),
            "{}",
            w.message
        );
        assert_eq!(at(&r, "triggers.w.wait").len(), 1, "{r:?}");
    }

    #[test]
    fn every_unsupported_webhook_key_is_warned_once_and_false_is_silent() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: w\n    type: {HOOK}\n    key: k\n    wait: true\n    returnOutputs: true\n    responseContentType: text/plain\n    responseCode: 201\n  - id: q\n    type: {HOOK}\n    key: k2\n    wait: false\n    returnOutputs: false\n"));
        for key in [
            "wait",
            "returnOutputs",
            "responseContentType",
            "responseCode",
        ] {
            assert_eq!(
                at(&r, &format!("triggers.w.{key}")).len(),
                1,
                "{key}: {r:?}"
            );
        }
        assert!(at(&r, "triggers.q.wait").is_empty(), "{r:?}");
        assert!(at(&r, "triggers.q.returnOutputs").is_empty(), "{r:?}");
    }

    #[test]
    fn stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not() {
        let r = report(&format!(
            "id: f\nnamespace: a.b\n{LOG}triggers:\n\
             \x20 - id: a\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    stopAfter: [FAILED]\n\
             \x20 - id: b\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    allowConcurrent: false\n\
             \x20 - id: c\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    allowConcurrent: true\n"
        ));
        let paths: std::collections::BTreeSet<_> = r
            .warnings
            .iter()
            .filter(|w| w.code == "not_honoured")
            .map(|w| w.path.as_str())
            .collect();
        assert_eq!(
            paths,
            std::collections::BTreeSet::from([
                "triggers.a.stopAfter",
                "triggers.b.allowConcurrent"
            ])
        );
        assert!(at(&r, "triggers.b.allowConcurrent")[0]
            .message
            .contains("egret never locks a trigger while its execution runs"));
    }

    #[test]
    fn labels_warn_on_schedule_and_webhook_and_when_on_a_webhook() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: s\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    labels:\n      a: b\n  - id: w\n    type: {HOOK}\n    key: k\n    labels:\n      a: b\n    when: \"{{{{ true }}}}\"\n"));
        for p in ["triggers.s.labels", "triggers.w.labels", "triggers.w.when"] {
            let f = at(&r, p);
            assert_eq!(f.len(), 1, "{p}: {r:?}");
            assert!(
                f[0].message.ends_with("roadmap package #11 lifts it"),
                "{}",
                f[0].message
            );
        }
    }

    #[test]
    fn a_trigger_key_kestra_has_and_egret_drops_names_its_package() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: s\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    logLevel: DEBUG\n    workerSelector: {{}}\n    interval: PT1H\n"));
        assert!(
            r.errors.is_empty(),
            "interval is inert in Kestra, not unknown: {r:?}"
        );
        assert!(at(&r, "triggers.s.logLevel")[0]
            .message
            .ends_with("roadmap package #13 lifts it"));
        assert!(at(&r, "triggers.s.workerSelector")[0]
            .message
            .contains("Enterprise"));
        assert!(at(&r, "triggers.s.interval").is_empty(), "{r:?}");
    }

    #[test]
    fn an_unknown_trigger_key_is_refused() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: s\n    type: {SCHED}\n    cron: \"0 * * * *\"\n    crn: x\n"));
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "unknown_property" && e.path == "triggers.s.crn"),
            "{r:?}"
        );
        assert_eq!(at(&r, "triggers.s.crn").len(), 1, "{r:?}");
    }

    /// One egret struct serves twenty Kestra classes: a key of another subtype
    /// is unknown to Kestra (spec 3.5).
    #[test]
    fn an_input_key_of_another_type_is_refused_and_its_own_kestra_key_is_read() {
        let r = report("id: f\nnamespace: a.b\ninputs:\n  - id: n\n    type: INT\n    min: 1\n    validator: \"^a$\"\n  - id: s\n    type: STRING\n    itemType: INT\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        assert!(r.warnings.is_empty(), "{r:?}");
        let unknown: std::collections::BTreeSet<_> = r
            .errors
            .iter()
            .filter(|e| e.code == "unknown_property")
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(
            unknown,
            std::collections::BTreeSet::from(["inputs.n.validator", "inputs.s.itemType"])
        );
        let m = &at(&r, "inputs.n.validator")[0].message;
        assert_eq!(
            m,
            "input 'n': unknown property 'validator' on a INT input: neither Kestra nor egret accepts it"
        );
    }

    #[test]
    fn the_keys_egret_reads_on_an_input_are_silent_on_their_own_type() {
        let r = report("id: f\nnamespace: a.b\ninputs:\n  - id: a\n    type: ARRAY\n    itemType: INT\n  - id: m\n    type: MULTISELECT\n    itemType: STRING\n    values: [x]\n  - id: f1\n    type: FILE\n    allowedFileExtensions: [.csv]\n  - id: b\n    type: BOOL\n    defaults: true\n    required: false\n    displayName: B\n    description: d\n    prefill: true\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        let w: Vec<_> = r.warnings.iter().map(|w| w.path.as_str()).collect();
        assert!(w.is_empty(), "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
    }

    /// The Task 1 review: the input rows' packages were unpinned.
    #[test]
    fn the_input_rows_name_the_package_the_roadmap_gives() {
        use Lift::{Enterprise, Package};
        let rows = [
            ("JSON", "jsonSchema", Package(3)),
            ("FORM", "inputs", Enterprise),
            ("REUSABLE_INPUTS", "ref", Enterprise),
            ("REUSABLE_INPUTS", "namespace", Enterprise),
            ("REUSABLE_INPUTS", "revision", Enterprise),
        ];
        for (ty, key, want) in rows {
            assert_eq!(lift_of(Site::Input(ty), key), Some(want), "{ty}.{key}");
        }
        // Trigger rows #11.
        for (t, key) in [
            (SCHED, "lateMaximumDelay"),
            (SCHED, "when"),
            (SCHED, "labels"),
            (HOOK, "wait"),
            (HOOK, "returnOutputs"),
            (HOOK, "responseContentType"),
            (HOOK, "responseCode"),
            (HOOK, "when"),
            (HOOK, "labels"),
            (SCHED, "stopAfter"),
            (SCHED, "failOnTriggerError"),
            (SCHED, "allowConcurrent"),
        ] {
            assert_eq!(
                lift_of(Site::Trigger(t), key),
                Some(Package(11)),
                "{t}.{key}"
            );
        }
    }

    /// The input tables agree with the committed measurement: for every input
    /// type, the keys the treatment table measures `ignored` are exactly those
    /// `lift_of` has a package for, and every other key it lists is read.
    #[test]
    fn input_keys_agree_with_the_committed_treatment() {
        for ty in INPUT_TYPES {
            let row = property_treatment()
                .iter()
                .find(|(k, _)| input_row_matches(k, ty))
                .unwrap_or_else(|| panic!("no treatment row for {ty}"))
                .1;
            for (key, treatment) in row {
                match treatment {
                    Treatment::Ignored => {
                        assert!(lift_of(Site::Input(ty), key).is_some(), "{ty}.{key}");
                        assert!(!input_keys(ty).contains(&key.as_str()), "{ty}.{key}");
                    }
                    _ => assert!(input_keys(ty).contains(&key.as_str()), "{ty}.{key}"),
                }
            }
            for key in input_keys(ty) {
                assert!(
                    row.contains_key(key),
                    "{ty}.{key} is read but Kestra has no such key"
                );
            }
        }
    }

    fn input_row_matches(row: &str, ty: &str) -> bool {
        let class = match ty {
            "BOOL" => "BoolInput",
            "DATETIME" => "DateTimeInput",
            "REUSABLE_INPUTS" => "ReusableInputsInput",
            "URI" => "URIInput",
            other => {
                let mut c = other.to_lowercase();
                c[..1].make_ascii_uppercase();
                return row.starts_with("io.kestra.core.models.flows.input.")
                    && row["io.kestra.core.models.flows.input.".len()..]
                        .strip_prefix(&format!("{c}Input"))
                        .is_some_and(|rest| rest.starts_with('-'));
            }
        };
        row.starts_with(&format!("io.kestra.core.models.flows.input.{class}-"))
    }

    #[test]
    fn an_unknown_output_key_is_refused() {
        let r = report(&format!("id: f\nnamespace: a.b\n{LOG}outputs:\n  - id: o\n    type: STRING\n    value: x\n    valu: y\n"));
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == "unknown_property" && e.path == "outputs.o.valu"),
            "{r:?}"
        );
        let ok = report(&format!("id: f\nnamespace: a.b\n{LOG}outputs:\n  - id: o\n    type: STRING\n    value: x\n    description: d\n    displayName: D\n    required: false\n"));
        assert!(ok.errors.is_empty() && ok.warnings.is_empty(), "{ok:?}");
    }

    #[test]
    fn a_flow_of_known_properties_has_no_finding() {
        let w = census(
            "id: f\nnamespace: a.b\ndescription: d\nlabels:\n  a: b\ninputs:\n  - id: i\n    type: STRING\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Switch\n    value: x\n    cases:\n      x:\n        - id: t\n          type: core.log\n          message: hi\n    defaults:\n      - id: d\n        type: core.log\n        message: hi\n",
        );
        assert!(w.is_empty(), "{w:?}");
    }

    // ---------- Task 6: the nested beans (spec 3.7) ----------

    const HEAD: &str = "id: f\nnamespace: a.b\n";

    /// One finding, at `path`, with `code`: the shape every nested-bean test asserts.
    fn only(src: &str, path: &str, code: &'static str) {
        let w = census(src);
        assert_eq!(w, vec![(path.to_string(), code)], "{src}");
    }

    fn silent_census(src: &str) {
        let w = census(src);
        assert!(w.is_empty(), "{w:?}\n{src}");
    }

    #[test]
    fn a_typo_in_a_task_retry_is_refused_at_its_path() {
        only(
            &format!("{HEAD}tasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      maxAttempt: 3\n"),
            "tasks.t.retry.maxAttempt",
            "unknown_property",
        );
    }

    #[test]
    fn each_retry_type_accepts_its_own_keys_and_refuses_another_types() {
        let task = |retry: &str| {
            format!(
                "{HEAD}tasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n{retry}"
            )
        };
        let common = "      maxAttempts: 2\n      maxDuration: PT1M\n      behavior: RETRY_FAILED_TASK\n      warningOnRetry: false\n";
        silent_census(&task(&format!(
            "      type: constant\n      interval: PT1S\n{common}"
        )));
        silent_census(&task(&format!(
            "      type: exponential\n      interval: PT1S\n      maxInterval: PT9S\n      delayFactor: 2.0\n{common}"
        )));
        silent_census(&task(&format!(
            "      type: random\n      minInterval: PT1S\n      maxInterval: PT9S\n{common}"
        )));
        // A key of another retry type is as unknown as a typo.
        only(
            &task("      type: constant\n      interval: PT1S\n      delayFactor: 2.0\n"),
            "tasks.t.retry.delayFactor",
            "unknown_property",
        );
        only(
            &task("      type: random\n      minInterval: PT1S\n      maxInterval: PT9S\n      interval: PT1S\n"),
            "tasks.t.retry.interval",
            "unknown_property",
        );
        only(
            &task("      type: exponential\n      interval: PT1S\n      maxInterval: PT9S\n      minInterval: PT1S\n"),
            "tasks.t.retry.minInterval",
            "unknown_property",
        );
    }

    #[test]
    fn a_flow_retry_is_checked_per_type_too() {
        only(
            &format!("{HEAD}retry:\n  type: exponential\n  interval: PT1S\n  maxInterval: PT9S\n  delayFactr: 2\n{LOG}"),
            "retry.delayFactr",
            "unknown_property",
        );
    }

    #[test]
    fn a_retry_key_has_one_finding_not_two() {
        let r = report(&format!("{HEAD}tasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      oops: 1\n"));
        assert_eq!(at(&r, "tasks.t.retry.oops").len(), 1, "{r:?}");
    }

    #[test]
    fn concurrency_keys_are_checked() {
        only(
            &format!("{HEAD}concurrency:\n  limit: 1\n  limt: 2\n{LOG}"),
            "concurrency.limt",
            "unknown_property",
        );
        silent_census(&format!(
            "{HEAD}concurrency:\n  limit: 1\n  behavior: QUEUE\n{LOG}"
        ));
    }

    #[test]
    fn each_sla_is_checked_against_its_type() {
        silent_census(&format!(
            "{HEAD}sla:\n  - id: a\n    type: MAX_DURATION\n    behavior: FAIL\n    duration: PT1H\n    labels:\n      k: v\n  - id: b\n    type: EXECUTION_ASSERTION\n    behavior: CANCEL\n    assert: \"{{{{ true }}}}\"\n{LOG}"
        ));
        only(
            &format!("{HEAD}sla:\n  - id: a\n    type: MAX_DURATION\n    behavior: FAIL\n    duration: PT1H\n    assert: x\n{LOG}"),
            "sla[0].assert",
            "unknown_property",
        );
        only(
            &format!("{HEAD}sla:\n  - id: a\n    type: MAX_DURATION\n    behavior: FAIL\n    duration: PT1H\n  - id: b\n    type: EXECUTION_ASSERTION\n    behavior: FAIL\n    assert: x\n    duration: PT1H\n{LOG}"),
            "sla[1].duration",
            "unknown_property",
        );
    }

    #[test]
    fn each_check_is_checked() {
        silent_census(&format!(
            "{HEAD}checks:\n  - when: \"{{{{ true }}}}\"\n    message: m\n    style: ERROR\n    behavior: FAIL_EXECUTION\n{LOG}"
        ));
        only(
            &format!("{HEAD}checks:\n  - when: x\n  - when: y\n    mesage: m\n{LOG}"),
            "checks[1].mesage",
            "unknown_property",
        );
    }

    /// Kestra's `Check.setCondition` (`Check.java:64-67`, deprecated) makes
    /// `condition` an accepted name that sets `when`; egret reads `when` only.
    #[test]
    fn the_deprecated_check_condition_warns_with_its_package() {
        let r = report(&format!(
            "{HEAD}checks:\n  - condition: x\n    message: m\n{LOG}"
        ));
        let f = at(&r, "checks[0].condition");
        assert_eq!(f.len(), 1, "{r:?}");
        assert_eq!(f[0].code, "not_honoured");
        assert!(
            f[0].message.contains("roadmap package #16"),
            "{}",
            f[0].message
        );
    }

    #[test]
    fn check_frequency_keys_are_checked() {
        let loop_until = |freq: &str| {
            format!("{HEAD}tasks:\n  - id: w\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ true }}}}\"\n    checkFrequency:\n{freq}    tasks:\n      - id: t\n        type: core.log\n        message: hi\n")
        };
        only(
            &loop_until("      interval: PT1S\n      interva: PT1S\n"),
            "tasks.w.checkFrequency.interva",
            "unknown_property",
        );
        silent_census(&loop_until(
            "      interval: PT1S\n      maxIterations: 3\n      maxDuration: PT1H\n",
        ));
    }

    #[test]
    fn a_dag_node_is_checked() {
        let dag = |second: &str| {
            format!("{HEAD}tasks:\n  - id: d\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task:\n          id: a\n          type: core.log\n          message: hi\n      - task:\n          id: b\n          type: core.log\n          message: hi\n{second}")
        };
        only(
            &dag("        dependOn: [a]\n"),
            "tasks.d.tasks[1].dependOn",
            "unknown_property",
        );
        silent_census(&dag("        dependsOn: [a]\n"));
    }

    #[test]
    fn an_input_depends_on_is_checked() {
        let input = |dep: &str| {
            format!("{HEAD}inputs:\n  - id: a\n    type: STRING\n  - id: i\n    type: STRING\n    dependsOn:\n{dep}{LOG}")
        };
        only(
            &input("      inputs: [a]\n      inpts: [a]\n"),
            "inputs.i.dependsOn.inpts",
            "unknown_property",
        );
        silent_census(&input(
            "      inputs: [a]\n      condition: \"{{ true }}\"\n",
        ));
    }

    #[test]
    fn a_flow_trigger_depends_on_entry_is_checked() {
        let trig = |entry: &str| {
            format!("{HEAD}{LOG}triggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    dependsOn:\n      - namespace: a.b\n        flowId: g\n{entry}")
        };
        only(
            &trig("      - namespace: a.b\n        flowid: h\n"),
            "triggers.l.dependsOn[1].flowid",
            "unknown_property",
        );
        silent_census(&trig(
            "      - namespace: a.b\n        flowId: h\n        states: [SUCCESS]\n        labels:\n          k: v\n        when: \"{{ true }}\"\n",
        ));
    }

    /// `flowRef` is egret's own, filled at publish; Kestra's `Dependency` has no
    /// such key, so an author writing it is refused as Kestra would.
    #[test]
    fn an_authored_flow_ref_on_a_dependency_is_unknown() {
        only(
            &format!("{HEAD}{LOG}triggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    dependsOn:\n      - namespace: a.b\n        flowId: g\n        flowRef: 00000000-0000-0000-0000-000000000000\n"),
            "triggers.l.dependsOn[0].flowRef",
            "unknown_property",
        );
    }

    /// `quotas` is an Enterprise warning already and Kestra OSS does not read it (spec 3.7).
    #[test]
    fn quotas_are_not_walked() {
        let w = census(&format!("{HEAD}quotas:\n  - bogus: 1\n{LOG}"));
        assert!(w.iter().all(|(p, _)| !p.starts_with("quotas")), "{w:?}");
    }

    /// Each hand-written key set lists exactly what the model reads: a key the
    /// const lists but the model drops would let a typo-shaped key through
    /// silently, and a key the model reads but the const omits would be a
    /// false refusal. Both are caught by round-tripping every listed key.
    fn round_trip(model_yaml: &str, listed: &[&str], serialised_as: fn(&Value) -> Value) {
        let v: Value = serde_norway::from_str(model_yaml).unwrap();
        let out = serialised_as(&v);
        let Value::Object(o) = out else {
            panic!("{out:?}")
        };
        let mut got: Vec<&str> = o.keys().map(String::as_str).collect();
        let mut want: Vec<&str> = listed.to_vec();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn every_listed_retry_key_deserializes_into_the_model() {
        let as_retry = |v: &Value| {
            serde_json::to_value(serde_json::from_value::<models::Retry>(v.clone()).unwrap())
                .unwrap()
        };
        for (ty, yaml) in [
            ("constant", "type: constant\ninterval: PT1S\nmaxDuration: PT1M\nmaxAttempts: 2\nwarningOnRetry: true\nbehavior: CREATE_NEW_EXECUTION\n"),
            ("exponential", "type: exponential\ninterval: PT1S\nmaxInterval: PT9S\ndelayFactor: 2.0\nmaxDuration: PT1M\nmaxAttempts: 2\nwarningOnRetry: true\nbehavior: CREATE_NEW_EXECUTION\n"),
            ("random", "type: random\nminInterval: PT1S\nmaxInterval: PT9S\nmaxDuration: PT1M\nmaxAttempts: 2\nwarningOnRetry: true\nbehavior: CREATE_NEW_EXECUTION\n"),
        ] {
            round_trip(yaml, &retry_keys(ty).unwrap(), as_retry);
        }
    }

    #[test]
    fn every_listed_sla_key_deserializes_into_the_model() {
        let as_sla = |v: &Value| {
            serde_json::to_value(serde_json::from_value::<models::Sla>(v.clone()).unwrap()).unwrap()
        };
        round_trip(
            "type: MAX_DURATION\nid: a\nbehavior: FAIL\nlabels:\n  - key: k\n    value: v\nduration: PT1H\n",
            &sla_keys("MAX_DURATION").unwrap(),
            as_sla,
        );
        round_trip(
            "type: EXECUTION_ASSERTION\nid: a\nbehavior: FAIL\nlabels:\n  - key: k\n    value: v\nassert: x\n",
            &sla_keys("EXECUTION_ASSERTION").unwrap(),
            as_sla,
        );
    }

    // ---------- Kestra's own save-time warnings (Task 7, spec 4) ----------

    fn kestra(r: &ValidationReport, path: &str) -> Vec<(&'static str, String)> {
        r.warnings
            .iter()
            .filter(|w| w.path == path)
            .map(|w| (w.code, w.message.clone()))
            .collect()
    }

    #[test]
    fn kestra_warns_on_timeout_on_a_flowable_but_not_on_a_pause() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    timeout: PT1M\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    timeout: PT1M\n");
        let w: Vec<_> = r
            .warnings
            .iter()
            .filter(|w| w.code == "runnable_property_on_non_runnable")
            .collect();
        assert_eq!(w.len(), 1, "{r:?}");
        assert_eq!(w[0].path, "tasks.s.timeout");
        assert_eq!(
            w[0].message,
            "The task 's' cannot use the 'timeout' property as it's only relevant for runnable tasks."
        );
    }

    #[test]
    fn task_cache_on_a_flowable_gets_kestras_warning_and_not_the_13_one() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Parallel\n    taskCache: {enabled: true}\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
        let at: Vec<_> = r
            .warnings
            .iter()
            .filter(|w| w.path == "tasks.p.taskCache")
            .collect();
        assert_eq!(at.len(), 1, "{r:?}");
        assert_eq!(at[0].code, "runnable_property_on_non_runnable");
        assert_eq!(
            at[0].message,
            "The task 'p' cannot use the 'taskCache' property as it's only relevant for runnable tasks."
        );
    }

    #[test]
    fn every_non_runnable_type_has_one_finding_per_runnable_property() {
        let bodies = [
            (
                "io.kestra.plugin.core.flow.Sequential",
                "    tasks:\n      - id: a\n        type: core.log\n        message: a\n",
            ),
            (
                "io.kestra.plugin.core.flow.WorkingDirectory",
                "    tasks:\n      - id: a\n        type: core.log\n        message: a\n",
            ),
            (
                "io.kestra.plugin.core.flow.AllowFailure",
                "    tasks:\n      - id: a\n        type: core.log\n        message: a\n",
            ),
            (
                "io.kestra.plugin.core.execution.Exit",
                "    state: SUCCESS\n",
            ),
            (
                "io.kestra.plugin.core.execution.SetVariables",
                "    variables: {a: b}\n",
            ),
            (
                "io.kestra.plugin.core.execution.UnsetVariables",
                "    variables: [a]\n",
            ),
            (
                "io.kestra.plugin.core.execution.Labels",
                "    labels: {a: b}\n",
            ),
        ];
        for (ty, body) in bodies {
            let src = format!("id: f\nnamespace: a.b\ntasks:\n  - id: x\n    type: {ty}\n    timeout: PT1M\n    taskCache: {{enabled: true}}\n    workerSelector: {{tags: [a]}}\n{body}");
            let r = report(&src);
            for key in ["timeout", "taskCache", "workerSelector"] {
                let got = kestra(&r, &format!("tasks.x.{key}"));
                assert_eq!(got.len(), 1, "{ty} {key}: {r:?}");
                assert_eq!(got[0].0, "runnable_property_on_non_runnable", "{ty} {key}");
                assert_eq!(
                    got[0].1,
                    format!("The task 'x' cannot use the '{key}' property as it's only relevant for runnable tasks.")
                );
            }
        }
    }

    #[test]
    fn a_runnable_task_keeps_the_13_and_enterprise_warnings_and_draws_no_kestra_one() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: a\n    timeout: PT1M\n    taskCache: {enabled: true}\n    workerSelector: {tags: [a]}\n");
        assert!(
            r.warnings
                .iter()
                .all(|w| w.code != "runnable_property_on_non_runnable"),
            "{r:?}"
        );
        assert_eq!(kestra(&r, "tasks.t.taskCache").len(), 1, "{r:?}");
        assert_eq!(kestra(&r, "tasks.t.taskCache")[0].0, "not_honoured");
        assert_eq!(kestra(&r, "tasks.t.workerSelector")[0].0, "not_honoured");
    }

    #[test]
    fn a_null_runnable_property_on_a_flowable_is_not_warned() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    timeout: null\n    taskCache: null\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
        assert!(
            r.warnings
                .iter()
                .all(|w| w.code != "runnable_property_on_non_runnable"),
            "{r:?}"
        );
    }

    #[test]
    fn a_nested_flowable_and_an_error_handler_are_reached() {
        let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: in\n        type: io.kestra.plugin.core.flow.Parallel\n        timeout: PT1M\n        tasks:\n          - id: a\n            type: core.log\n            message: a\nerrors:\n  - id: e\n    type: io.kestra.plugin.core.flow.Sequential\n    timeout: PT1M\n    tasks:\n      - id: b\n        type: core.log\n        message: a\n");
        assert_eq!(kestra(&r, "tasks.s.in.timeout").len(), 1, "{r:?}");
        assert_eq!(kestra(&r, "errors.e.timeout").len(), 1, "{r:?}");
    }

    const LISTENER: &str = "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\ntriggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n";

    #[test]
    fn a_flow_trigger_with_no_depends_on_warns_in_kestras_words_scoped_to_the_workspace() {
        let r = report(LISTENER);
        let w = r
            .warnings
            .iter()
            .find(|w| w.code == "flow_trigger_without_depends_on")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert_eq!(w.path, "triggers.l");
        assert_eq!(
            w.message,
            "This flow will be triggered for EVERY execution of EVERY flow in this workspace. \
             We recommend adding the dependsOn property to the Flow trigger 'l'."
        );
    }

    #[test]
    fn a_flow_trigger_with_depends_on_or_a_when_does_not_warn() {
        for extra in [
            "    dependsOn:\n      - namespace: a.b\n        flowId: g\n",
            "    when: \"{{ trigger.namespace == 'a.b' }}\"\n",
        ] {
            let r = report(&format!("{LISTENER}{extra}"));
            assert!(
                !r.warnings
                    .iter()
                    .any(|w| w.code == "flow_trigger_without_depends_on"),
                "{extra}: {r:?}"
            );
        }
    }

    #[test]
    fn a_literal_true_when_or_an_empty_depends_on_still_warns() {
        for extra in [
            "    when: \"true\"\n",
            "    dependsOn: []\n",
            "    when: null\n",
            // A YAML boolean is Kestra's string "true" too.
            "    when: true\n",
        ] {
            let r = report(&format!("{LISTENER}{extra}"));
            assert_eq!(
                r.warnings
                    .iter()
                    .filter(|w| w.code == "flow_trigger_without_depends_on")
                    .count(),
                1,
                "{extra}: {r:?}"
            );
        }
        // `when: false` is a condition (`!"true".equals("false")`): it filters,
        // so there is nothing to warn about.
        let r = report(&format!("{LISTENER}    when: false\n"));
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.code == "flow_trigger_without_depends_on"),
            "{r:?}"
        );
    }

    // ---- end hooks B2 (spec D11): retry reaching an afterExecution task ----

    fn retry_warnings(src: &str) -> Vec<(String, String)> {
        let flow = crate::spec::parse(src).unwrap();
        crate::spec::validate_source(src, &flow)
            .warnings
            .into_iter()
            .filter(|w| w.code == "not_honoured" && w.message.contains("not retried"))
            .map(|w| (w.path, w.message))
            .collect()
    }

    const NOT_RETRIED: &str = "an afterExecution task is not retried on egret: the execution has already ended. Roadmap package #13 lifts it";
    const RETRY: &str =
        "    retry:\n      type: constant\n      interval: PT1S\n      maxAttempts: 2\n";

    #[test]
    fn a_retry_inside_after_execution_warns_thirteen() {
        let src = format!(
            "{HEAD}tasks:\n  - id: t\n    type: core.log\n    message: m\n{RETRY}afterExecution:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: a\n        type: core.log\n        message: m\n        retry:\n          type: constant\n          interval: PT1S\n          maxAttempts: 2\n"
        );
        // `tasks:` carries a retry too and must stay quiet.
        assert_eq!(
            retry_warnings(&src),
            vec![(
                "afterExecution.seq.a.retry".to_string(),
                NOT_RETRIED.to_string()
            )]
        );
    }

    #[test]
    fn a_flow_retry_with_after_execution_warns_thirteen() {
        let with = format!(
            "{HEAD}retry:\n  type: constant\n  interval: PT1S\n  maxAttempts: 2\ntasks:\n  - id: t\n    type: core.log\n    message: m\nafterExecution:\n  - id: a\n    type: core.log\n    message: m\n"
        );
        assert_eq!(
            retry_warnings(&with),
            vec![("retry".to_string(), NOT_RETRIED.to_string())]
        );
        let without = with.replace(
            "afterExecution:\n  - id: a\n    type: core.log\n    message: m\n",
            "",
        );
        assert!(retry_warnings(&without).is_empty());
    }
}

#[cfg(test)]
mod corpus {
    use super::*;

    /// The oracle's valid Kestra flows (`oracle-exec/corpus/valids`, 323 of
    /// them) write only properties Kestra documents, so none may be called
    /// unknown: the check is the treatment table, and a false `unknown_property`
    /// here would be a row the walk resolves wrongly. Measured 2026-10-25 (KI-121):
    /// 0 unknown, 45 ignored (all keys the table measures `ignored`).
    #[test]
    fn no_valid_kestra_flow_has_an_unknown_property() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/corpus");
        let mut unknown = Vec::new();
        // What was read, parsed and examined, per directory: a count of files
        // alone passes when every parse fails, so each floor is on what the
        // walk visited.
        let mut files = [0usize; 2];
        let mut parsed = [0usize; 2];
        let mut seen = [Seen::default(); 2];
        for (n, dir) in ["valids", "invalids"].into_iter().enumerate() {
            for entry in std::fs::read_dir(format!("{root}/{dir}")).expect("the corpus") {
                let path = entry.expect("an entry").path();
                let src = std::fs::read_to_string(&path).expect("a flow");
                files[n] += 1;
                // A corpus flow egret's model cannot parse has no model to pair
                // with; the census fails open on it, as in production.
                let Ok(flow) = crate::spec::parse(&src) else {
                    continue;
                };
                parsed[n] += 1;
                let s = super::seen(&src, &flow);
                seen[n].triggers += s.triggers;
                seen[n].inputs += s.inputs;
                seen[n].outputs += s.outputs;
                seen[n].beans += s.beans;
                // Only the valid flows must draw no refusal; an invalid one
                // may (Kestra refuses it too).
                if dir == "valids" {
                    let mut report = ValidationReport::default();
                    findings(&src, &flow, &mut report);
                    for f in report
                        .errors
                        .iter()
                        .filter(|f| f.code == "unknown_property")
                    {
                        unknown.push(format!("{}: {}", path.display(), f.message));
                    }
                }
            }
        }
        eprintln!("corpus files {files:?} parsed {parsed:?} seen {seen:?}");
        assert!(unknown.is_empty(), "{unknown:#?}");
        // Measured 2026-10-26 (Task 6): valids 323 files, 322 parsed, 39
        // triggers, 106 inputs, 20 outputs, 94 nested beans; invalids 28
        // files, 23 parsed, 4 / 18 / 1 / 13. Each floor sits a little under.
        assert!(files[0] > 300 && files[1] >= 25, "files {files:?}");
        assert!(parsed[0] >= 315 && parsed[1] >= 20, "parsed {parsed:?}");
        let v = seen[0];
        assert!(v.triggers >= 35, "valids triggers {v:?}");
        assert!(v.inputs >= 100, "valids inputs {v:?}");
        assert!(v.outputs >= 18, "valids outputs {v:?}");
        assert!(v.beans >= 85, "valids nested beans {v:?}");
        let i = seen[1];
        assert!(
            i.triggers >= 3 && i.inputs >= 15 && i.beans >= 10,
            "invalids {i:?}"
        );
    }

    /// Kestra's two save-time warnings are warnings only: no corpus flow Kestra
    /// accepts is refused by them. The counts are printed for the report.
    #[test]
    fn kestras_own_warnings_never_refuse_a_corpus_flow() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/corpus");
        let mut by_code = std::collections::BTreeMap::<&str, usize>::new();
        for dir in ["valids", "invalids"] {
            for entry in std::fs::read_dir(format!("{root}/{dir}")).expect("the corpus") {
                let src = std::fs::read_to_string(entry.expect("an entry").path()).expect("a flow");
                let Ok(flow) = crate::spec::parse(&src) else {
                    continue;
                };
                let report = crate::spec::validate_source(&src, &flow);
                for w in &report.warnings {
                    if matches!(
                        w.code,
                        "runnable_property_on_non_runnable" | "flow_trigger_without_depends_on"
                    ) {
                        *by_code.entry(w.code).or_default() += 1;
                    }
                }
                assert!(
                    !report.errors.iter().any(|e| matches!(
                        e.code,
                        "runnable_property_on_non_runnable" | "flow_trigger_without_depends_on"
                    )),
                    "{dir}: {report:?}"
                );
            }
        }
        eprintln!("corpus Kestra warnings {by_code:?}");
    }
}
