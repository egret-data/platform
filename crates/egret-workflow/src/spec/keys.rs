//! The key tables the save-time property check and the measuring instrument
//! share (spec `2026-10-19-workflow-no-silent-drops-design.md` section 2.3):
//! which keys egret's models read at each site, and which package lifts each
//! key Kestra accepts and egret does not act on (D12).
//!
//! `spec::schema::measure` imports these instead of keeping its own copies,
//! so the instrument and the check read one table.

// Some tables are read only by the later tasks' walkers (plan Tasks 3-7).
#![allow(dead_code)]

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
}

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";
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
            ("STRING" | "SECRET", "validator") => Some(Package(3)),
            ("INT" | "FLOAT" | "DURATION", "min" | "max") => Some(Package(3)),
            ("DATE" | "DATETIME" | "TIME", "after" | "before") => Some(Package(3)),
            (
                "SELECT" | "MULTISELECT",
                "values" | "expression" | "allowCustomValue" | "isRadio" | "autoSelectFirst",
            ) => Some(Package(3)),
            ("JSON", "jsonSchema") => Some(Package(3)),
            ("FORM", "inputs") => Some(Enterprise),
            ("REUSABLE_INPUTS", "ref" | "namespace" | "revision") => Some(Enterprise),
            _ => None,
        },
    }
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

/// The keys under which a task holds child tasks: a list of tasks, a
/// `Switch`'s `cases` (a map of lists), a `Dag`'s `tasks` (a list of
/// `{task, dependsOn}` nodes).
const CHILD_LISTS: &[&str] = &[
    "tasks", "then", "else", "errors", "finally", "defaults", "cases",
];

/// The flow's four root lists, as `spec::path::root_lists` names them.
const ROOT_LISTS: &[&str] = &["tasks", "errors", "finally", "afterExecution"];

/// The save-time key census (spec D11): the keys of `source` that serde
/// dropped. Called only by [`crate::spec::validate_source`]; the run-time gate
/// never reaches it. `source` has already parsed as a [`Flow`]; if it does not
/// parse as a mapping here, the census adds nothing (it fails open, never a
/// false refusal).
pub(crate) fn findings(source: &str, _flow: &Flow, report: &mut ValidationReport) {
    let doc = serde_norway::from_str::<Value>(source);
    debug_assert!(
        doc.is_ok(),
        "a source that parsed as a Flow parses as a Value"
    );
    let Ok(Value::Object(flow)) = doc else {
        return;
    };
    let mut w = Walk { report };
    w.check(FLOW_ROW, Site::Flow, "the flow", &flow, "");
    if let Some(retry) = flow.get("retry") {
        w.retry(retry, "retry");
    }
    for list in ROOT_LISTS {
        if let Some(Value::Array(tasks)) = flow.get(*list) {
            for task in tasks {
                w.task(task, list);
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
                w.check(
                    row,
                    Site::Trigger(row),
                    &format!("trigger '{id}'"),
                    t,
                    &format!("triggers.{id}"),
                );
            }
        }
    }
}

struct Walk<'a> {
    report: &'a mut ValidationReport,
}

impl Walk<'_> {
    fn task(&mut self, task: &Value, prefix: &str) {
        let Some(t) = task.as_object() else { return };
        let Some(id) = t.get("id").and_then(Value::as_str) else {
            return;
        };
        let path = crate::spec::path::task_path(prefix, id);
        match t.get("type").and_then(Value::as_str).and_then(task_row) {
            // A core leaf is checked against the keys its handler reads (spec
            // 3.2), which say more than the committed table: a refined finding
            // replaces the table's for the same key, never a second one.
            Some(row) if leaf_keys(row).is_some() => self.core_leaf(row, id, t, &path),
            Some(row) => self.check(row, Site::Task(row), &format!("task '{id}'"), t, &path),
            None => {}
        }
        if let Some(retry) = t.get("retry") {
            self.retry(retry, &format!("{path}.retry"));
        }
        for key in CHILD_LISTS {
            match t.get(*key) {
                Some(Value::Array(children)) => {
                    for child in children {
                        // A `Dag` node holds its task under `task`.
                        match child.get("task") {
                            Some(inner) if child.get("type").is_none() => self.task(inner, &path),
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
                _ => {}
            }
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

    fn retry(&mut self, retry: &Value, path: &str) {
        let Some(r) = retry.as_object() else { return };
        let row = match r.get("type").and_then(Value::as_str) {
            Some("constant") => "io.kestra.core.models.tasks.retrys.Constant-2",
            Some("exponential") => "io.kestra.core.models.tasks.retrys.Exponential-2",
            Some("random") => "io.kestra.core.models.tasks.retrys.Random-2",
            _ => return,
        };
        // A retry has no `Site`: no lift row names its keys yet (Tasks 3, 6).
        self.check_with(row, None, "the retry", r, path);
    }

    fn check(
        &mut self,
        row: &str,
        site: Site<'_>,
        what: &str,
        object: &serde_json::Map<String, Value>,
        path: &str,
    ) {
        self.check_with(row, Some(site), what, object, path);
    }

    /// One object against its row.
    fn check_with(
        &mut self,
        row: &str,
        site: Option<Site<'_>>,
        what: &str,
        object: &serde_json::Map<String, Value>,
        path: &str,
    ) {
        let Some(props) = property_treatment().get(row) else {
            return;
        };
        for key in object.keys() {
            // Kestra's `warningOnRetry` (`Retry.java`): `true` ends a retried
            // task that succeeds WARNING. Said once, in its own words; `false`
            // is the default, which egret's behaviour already is.
            if site.is_none() && key == "warningOnRetry" {
                if object.get(key) == Some(&Value::Bool(true)) {
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
            let at = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            match props.get(key) {
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
        assert_eq!(lift_of(Site::Input("SELECT"), "values"), Some(Package(3)));
        assert_eq!(lift_of(Site::Flow, "policyRefs"), Some(Enterprise));
        // Not lifted by #1's table: honoured, #2's, or unknown to Kestra.
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
        /// #2's keys (Rule 1): neither read, lifted nor refused by #1.
        const TWO: &[&str] = &["finally", "errors"];
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
                if matches!(site, Site::Task(_)) && TWO.contains(&k) {
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

    #[test]
    fn a_flow_of_known_properties_has_no_finding() {
        let w = census(
            "id: f\nnamespace: a.b\ndescription: d\nlabels:\n  a: b\ninputs:\n  - id: i\n    type: STRING\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Switch\n    value: x\n    cases:\n      x:\n        - id: t\n          type: core.log\n          message: hi\n    defaults:\n      - id: d\n        type: core.log\n        message: hi\n",
        );
        assert!(w.is_empty(), "{w:?}");
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
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/corpus/valids");
        let mut checked = 0;
        let mut unknown = Vec::new();
        for entry in std::fs::read_dir(dir).expect("the corpus") {
            let path = entry.expect("an entry").path();
            let src = std::fs::read_to_string(&path).expect("a flow");
            checked += 1;
            // A corpus flow egret's model cannot parse has no model to pair
            // with; the census fails open on it, as in production.
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
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
        assert!(checked > 300, "the corpus is where it was: {checked} files");
        assert!(unknown.is_empty(), "{unknown:#?}");
    }
}
