//! The key tables the save-time property check and the measuring instrument
//! share (spec `2026-10-19-workflow-no-silent-drops-design.md` section 2.3):
//! which keys egret's models read at each site, and which package lifts each
//! key Kestra accepts and egret does not act on (D12).
//!
//! `spec::schema::measure` imports these instead of keeping its own copies,
//! so the instrument and the check read one table.

// Nothing in production reads these until the save-time walker (plan Task 2).
#![allow(dead_code)]

use crate::models;
use crate::spec::schema::wire_fields_of;
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
}
