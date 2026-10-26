//! W5a: Kestra's script tasks and their `taskRunner:` — the model, and the
//! save-time refusal of everything W5a does not implement.
//!
//! Ported under Kestra's own FQCNs (programme spec §8 rule 4):
//! `io.kestra.plugin.scripts.shell.Commands`, `shell.Script`,
//! `python.Commands`, `python.Script`. `egret.container` and
//! `egret.script.python` (W4) are **aliases** of `shell.Commands` and
//! `python.Script` (W5a spec §3), declared in `spec::registry::TASK_TYPES`.
//!
//! [`parse`] is pure. It reads a leaf task's opaque `spec` map (what is left
//! after `models::Task`'s deserializer took `id`, `type`, `description`,
//! `retry`, `timeout`, `disabled`, `runIf`, `allowFailure`, `allowWarning`)
//! and answers a [`ScriptTask`] or every [`ScriptRefusal`] it found.
//! `spec::validate` reports those refusals as errors, so they block publish
//! and show on a draft; [`gate`] is the governance check publish and the
//! script handler (`worker::handlers::script`, at run time) call after it.
//!
//! # The property tables are Task 1's measurement
//!
//! Every property the artifact accepts on a type is in exactly one of
//! [`ScriptKind::implemented_properties`], [`HANDLED_BY_TASK_MODEL`] and
//! [`ScriptKind::refused_properties`]; `tests/scripts_gate.rs` asserts that
//! partition against `oracle-exec/w5a-scripts.json`'s `egret_expected`, so a
//! property cannot be dropped from all three without a test failing. A
//! property Kestra itself does not accept is refused as unknown, which is what
//! Kestra does at save (`Unrecognized field …`, measured).
//!
//! # Where egret deliberately differs (W5a spec §2)
//!
//! * `taskRunner` absent means the **configured** default runner, not Docker.
//! * `io.kestra.plugin.core.runner.Process` is refused at save: it would run
//!   the task's commands inside `egret-server` (Task 1 measured Kestra running
//!   them in its own server container as uid 0). [`RunnerKind`] has no
//!   `Process` variant, so a parsed task cannot carry one.
//! * `io.kestra.plugin.ee.kubernetes.runner.Kubernetes` (Enterprise-only in
//!   Kestra) is implemented under the same FQCN, as a closed property set.
//! * The seven base-task properties egret's `Task` model does not read
//!   (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`,
//!   `workerSelector`) are refused on these types instead of being dropped.
//!   **Other task types still drop them silently**; that is outside W5a.
//! * `warningOnStdErr` is accepted and has **no effect**, as on the artifact
//!   (Task 1: SUCCESS with `true`, `false` or unset; Kestra's own field reads
//!   "Not used anymore, will be removed soon"). It does NOT set WARNING.
//! * The two aliases also accept W4's `image` as a synonym of
//!   `containerImage`, so a flow saved before W5a still publishes. The FQCN
//!   spellings do not: Kestra refuses `image` on a task.
//! * Values egret would have to evaluate (a `{{ }}` string where a list or a
//!   boolean is expected) are refused by name rather than accepted and
//!   misread; an `{{ }}` **image** is accepted and gated at run time.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::models::{Flow, Task, TaskKind};

pub mod gate;

/// The feature key that gates script tasks (`egret_workspace::features`).
/// Off by default: an absent `workspace_features` row means disabled.
pub const FEATURE_KEY: &str = "workflow_containers";

pub const SHELL_COMMANDS: &str = "io.kestra.plugin.scripts.shell.Commands";
pub const SHELL_SCRIPT: &str = "io.kestra.plugin.scripts.shell.Script";
pub const PYTHON_COMMANDS: &str = "io.kestra.plugin.scripts.python.Commands";
pub const PYTHON_SCRIPT: &str = "io.kestra.plugin.scripts.python.Script";

/// W4's own spellings, kept as aliases (W5a spec §3).
pub const ALIAS_CONTAINER: &str = "egret.container";
pub const ALIAS_SCRIPT_PYTHON: &str = "egret.script.python";

pub const DOCKER_RUNNER: &str = "io.kestra.plugin.scripts.runner.docker.Docker";
pub const KUBERNETES_RUNNER: &str = "io.kestra.plugin.ee.kubernetes.runner.Kubernetes";
pub const PROCESS_RUNNER: &str = "io.kestra.plugin.core.runner.Process";

/// The platform half of the gate's inputs, held by `routes::WorkflowState`:
/// `workflow.plugin_configurations` and the default runner. The workspace
/// half (the feature) is read per request.
#[derive(Debug, Clone, Default)]
pub struct ScriptSettings {
    pub plugin_configurations: egret_config::PluginConfigurations,
    /// The runner a script task with no `taskRunner:` runs on:
    /// `workflow.container_runner` (`runtime::handlers_for`). `None` when it
    /// is `none`, in which case such a task is refused at publish -- nothing
    /// would run it.
    pub default_runner: Option<RunnerKind>,
}

/// Why a task naming `asked` cannot run on a deployment whose
/// `workflow.container_runner` is `configured` -- one sentence for publish and
/// for the run-time refusal (`worker::handlers::script`).
pub fn runner_unavailable(asked: RunnerKind, configured: RunnerKind) -> String {
    format!(
        "`taskRunner` is '{}', but this egret runs script tasks on '{}' \
         (workflow.container_runner); remove `taskRunner` to use it",
        asked.fqcn(),
        configured.fqcn()
    )
}

/// Which of the four script task types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptKind {
    ShellCommands,
    ShellScript,
    PythonCommands,
    PythonScript,
}

impl ScriptKind {
    pub const ALL: [ScriptKind; 4] = [
        ScriptKind::ShellCommands,
        ScriptKind::ShellScript,
        ScriptKind::PythonCommands,
        ScriptKind::PythonScript,
    ];

    /// Kestra's FQCN, the canonical `type:`.
    pub fn fqcn(self) -> &'static str {
        match self {
            ScriptKind::ShellCommands => SHELL_COMMANDS,
            ScriptKind::ShellScript => SHELL_SCRIPT,
            ScriptKind::PythonCommands => PYTHON_COMMANDS,
            ScriptKind::PythonScript => PYTHON_SCRIPT,
        }
    }

    /// `commands` for the `Commands` types, `script` for the `Script` types:
    /// the one property required at save (Task 1, `required_at_save`).
    pub fn body_property(self) -> &'static str {
        match self {
            ScriptKind::ShellCommands | ScriptKind::PythonCommands => "commands",
            ScriptKind::ShellScript | ScriptKind::PythonScript => "script",
        }
    }

    fn is_python(self) -> bool {
        matches!(self, ScriptKind::PythonCommands | ScriptKind::PythonScript)
    }

    /// The properties W5a implements on this type (Task 1's
    /// `implemented_in_w5a`). `timeout` is read by the `Task` model and so
    /// never reaches [`parse`]; it is listed because W5a implements it.
    pub fn implemented_properties(self) -> Vec<&'static str> {
        let mut v = COMMON_IMPLEMENTED.to_vec();
        v.push(self.body_property());
        v.sort_unstable();
        v
    }

    /// The properties the artifact accepts on this type and W5a refuses at
    /// save, by name (Task 1's `refused_in_w5a`).
    pub fn refused_properties(self) -> Vec<&'static str> {
        let mut v = COMMON_REFUSED.to_vec();
        if self.is_python() {
            v.extend_from_slice(PYTHON_REFUSED);
        }
        v.sort_unstable();
        v
    }
}

/// Implemented on all four types, besides `commands`/`script`.
const COMMON_IMPLEMENTED: &[&str] = &[
    "beforeCommands",
    "containerImage",
    "env",
    "failFast",
    "inputFiles",
    "interpreter",
    "namespaceFiles",
    "outputDirectory",
    "outputFiles",
    "taskRunner",
    "timeout",
    "warningOnStdErr",
];

/// Accepted by the artifact on every task and read by egret's `Task` model
/// (Task 1's `handled_by_egret_task_model`). Normally absent from a leaf's
/// spec; ignored by [`parse`] if a caller passes them.
pub const HANDLED_BY_TASK_MODEL: &[&str] = &[
    "allowFailure",
    "allowWarning",
    "description",
    "disabled",
    "id",
    "retry",
    "runIf",
    "type",
];

/// Refused on all four types. Seven of these are base-`Task` properties
/// (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`,
/// `workerSelector`) that egret's `Task` model would otherwise drop without
/// effect; the other two (`runner`, `targetOS`) are the script tasks' own.
/// `outputDirectory` left this list in W9a Task 6: it is implemented over
/// internal storage. *(2026-10-05, W9b Task 5: `namespaceFiles` left it too,
/// staged from namespace files -- `storage::nsfiles::stage`.)*
const COMMON_REFUSED: &[&str] = &[
    "assets",
    "logLevel",
    "logToFile",
    "policyRefs",
    "runner",
    "targetOS",
    "taskCache",
    "version",
    "workerSelector",
];

/// Refused on the two `python.*` types only.
const PYTHON_REFUSED: &[&str] = &[
    "dependencies",
    "dependencyCacheEnabled",
    "packageManager",
    "pythonVersion",
    "uvAutoInstallEnabled",
    "uvInstallerSha256",
    "uvInstallerVersion",
];

/// Docker runner: implemented properties (Task 1).
pub const DOCKER_IMPLEMENTED: &[&str] = &["cpu", "delete", "memory", "pullPolicy", "type"];
/// Docker runner: accepted by the artifact, refused by W5a (Task 1). Includes
/// the schema-hidden `image` Kestra still accepts.
pub const DOCKER_REFUSED: &[&str] = &[
    "config",
    "credentials",
    "deviceRequests",
    "entryPoint",
    "extraHosts",
    "fileHandlingStrategy",
    "host",
    "image",
    "killGracePeriod",
    "networkMode",
    "portBindings",
    "privileged",
    "resume",
    "shmSize",
    "user",
    "version",
    "volumes",
    "wait",
];
/// Docker `cpu`: a closed object with one property, implemented.
pub const DOCKER_CPU_IMPLEMENTED: &[&str] = &["cpus"];
/// Docker `memory`: `memory` implemented, the other five refused.
pub const DOCKER_MEMORY_IMPLEMENTED: &[&str] = &["memory"];
pub const DOCKER_MEMORY_REFUSED: &[&str] = &[
    "kernelMemory",
    "memoryReservation",
    "memorySwap",
    "memorySwappiness",
    "oomKillDisable",
];
/// Kubernetes runner: the closed set W5a implements; anything else is refused.
pub const KUBERNETES_IMPLEMENTED: &[&str] = &["delete", "pullPolicy", "resources", "type"];
/// Kubernetes `resources`: exactly these paths.
pub const KUBERNETES_RESOURCE_PATHS: &[&str] =
    &["limit.cpu", "limit.memory", "request.cpu", "request.memory"];

/// Where a script task runs. **No `Process`**: it is refused while parsing, so
/// a [`ScriptTask`] cannot carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunnerKind {
    Docker,
    Kubernetes,
}

impl RunnerKind {
    /// The runner's FQCN, which is also its `plugin_configurations` key.
    pub fn fqcn(self) -> &'static str {
        match self {
            RunnerKind::Docker => DOCKER_RUNNER,
            RunnerKind::Kubernetes => KUBERNETES_RUNNER,
        }
    }
}

/// `pullPolicy`, both runners. Default `IF_NOT_PRESENT` (Task 1, and
/// `AbstractExecScript.java:52-58`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullPolicy {
    Always,
    #[default]
    IfNotPresent,
    Never,
}

/// One side (`request` or `limit`) of Kubernetes `resources`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceAmounts {
    pub cpu: Option<String>,
    pub memory: Option<String>,
    /// `ephemeral-storage` (final review I5). **Never the author's**: Kestra's
    /// `resources` has no such key and the parser refuses it, so only egret's
    /// `workflow.kubernetes.{default,max}_resources` set it.
    pub ephemeral_storage: Option<String>,
}

/// Kubernetes `resources`: `request`/`limit` for cpu and memory, nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resources {
    pub request: ResourceAmounts,
    pub limit: ResourceAmounts,
}

/// A parsed `taskRunner:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerSpec {
    pub kind: RunnerKind,
    pub pull_policy: PullPolicy,
    /// Delete the container/pod when the task ends. Default `true`.
    pub delete: bool,
    /// Kubernetes only.
    pub resources: Option<Resources>,
    /// Docker only: `cpu.cpus`, as written.
    pub cpu: Option<String>,
    /// Docker only: `memory.memory`, as written.
    pub memory: Option<String>,
}

impl RunnerSpec {
    /// What `taskRunner: {type: <kind>}` with no other property parses to:
    /// `pullPolicy: IF_NOT_PRESENT`, `delete: true`, nothing else set.
    pub fn defaults(kind: RunnerKind) -> Self {
        RunnerSpec {
            kind,
            pull_policy: PullPolicy::default(),
            delete: true,
            resources: None,
            cpu: None,
            memory: None,
        }
    }
}

/// A parsed script task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptTask {
    pub kind: ScriptKind,
    /// As written; `None` means [`default_image`]. See [`ScriptTask::image`].
    pub container_image: Option<String>,
    /// `commands` (the `Commands` types); empty on the `Script` types.
    pub commands: Vec<String>,
    /// `script` (the `Script` types); `None` on the `Commands` types.
    pub script: Option<String>,
    pub before_commands: Vec<String>,
    /// `None` means the default `["/bin/sh", "-c"]`.
    pub interpreter: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
    /// Inline contents only; a storage URI (a string) is refused.
    pub input_files: BTreeMap<String, String>,
    pub output_files: Vec<String>,
    /// `outputDirectory`, default `false` (W9a Task 6): every file the
    /// command writes under `{{ outputDir }}` / `$OUTPUT_DIR` is collected.
    pub output_directory: bool,
    /// Default `true`: `set -e` is prepended iff this is true (Task 1).
    pub fail_fast: bool,
    /// Accepted; **no effect** (see the module doc).
    pub warning_on_std_err: bool,
    /// `None`: absent or `null`, i.e. the configured default runner.
    pub task_runner: Option<RunnerSpec>,
    /// `namespaceFiles` (W9b Task 5), parsed by
    /// [`NamespaceFilesSpec::parse`](crate::storage::nsfiles::stage::NamespaceFilesSpec::parse);
    /// `None`: absent or `null`, no namespace file is staged.
    pub namespace_files: Option<crate::storage::nsfiles::stage::NamespaceFilesSpec>,
}

impl ScriptTask {
    /// The image the task runs: `containerImage` as written, or the type's
    /// [`default_image`]. Either must pass the allowlist.
    pub fn image(&self) -> &str {
        self.container_image
            .as_deref()
            .unwrap_or_else(|| default_image(self.kind))
    }
}

/// Kestra's default image per type, measured in Task 1: `ubuntu` for
/// `shell.*`, `python:3.13-slim` for `python.*`.
pub fn default_image(kind: ScriptKind) -> &'static str {
    match kind {
        ScriptKind::ShellCommands | ScriptKind::ShellScript => "ubuntu",
        ScriptKind::PythonCommands | ScriptKind::PythonScript => "python:3.13-slim",
    }
}

/// The script kind a `type:` names, by FQCN or by alias; `None` for any
/// other type.
pub fn kind_of(type_id: &str) -> Option<ScriptKind> {
    match type_id {
        SHELL_COMMANDS | ALIAS_CONTAINER => Some(ScriptKind::ShellCommands),
        SHELL_SCRIPT => Some(ScriptKind::ShellScript),
        PYTHON_COMMANDS => Some(ScriptKind::PythonCommands),
        PYTHON_SCRIPT | ALIAS_SCRIPT_PYTHON => Some(ScriptKind::PythonScript),
        _ => None,
    }
}

fn is_alias(type_id: &str) -> bool {
    matches!(type_id, ALIAS_CONTAINER | ALIAS_SCRIPT_PYTHON)
}

/// Why a script task cannot be saved. `path` is relative to the task
/// (`logLevel`, `taskRunner.volumes`, `taskRunner.resources.limit.gpu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRefusal {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

/// Kestra accepts the property; W5a does not implement it.
pub const CODE_NOT_IMPLEMENTED: &str = "script_property_not_implemented";
/// Kestra does not accept the property either.
pub const CODE_UNKNOWN: &str = "script_property_unknown";
pub const CODE_REQUIRED: &str = "script_property_required";
pub const CODE_INVALID: &str = "script_property_invalid";
/// `inputFiles` given as ONE string (a storage URI or an expression) rather
/// than a map. A map VALUE that is a `kestra://` URI is accepted (W9a spec §5).
pub const CODE_INPUT_FILES_URI: &str = "script_input_files_uri_not_implemented";
/// Both `image` (the alias's legacy spelling) and `containerImage`.
pub const CODE_IMAGE_CONFLICT: &str = "script_image_conflict";
/// `io.kestra.plugin.core.runner.Process`.
pub const CODE_RUNNER_REFUSED: &str = "script_runner_refused";
pub const CODE_RUNNER_UNKNOWN: &str = "script_runner_unknown";
pub const CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED: &str = "script_runner_property_not_implemented";
pub const CODE_RUNNER_PROPERTY_UNKNOWN: &str = "script_runner_property_unknown";

fn refusal(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ScriptRefusal {
    ScriptRefusal {
        code,
        path: path.into(),
        message: message.into(),
    }
}

/// Parse a script task's leaf `spec` (see the module doc) into a
/// [`ScriptTask`], or every reason it cannot be saved.
///
/// `type_id` is the task's `type:` as written (FQCN or alias). A type that is
/// not a script type answers one [`CODE_UNKNOWN`] refusal at path `type`.
pub fn parse(type_id: &str, spec: &Value) -> Result<ScriptTask, Vec<ScriptRefusal>> {
    let Some(kind) = kind_of(type_id) else {
        return Err(vec![refusal(
            CODE_UNKNOWN,
            "type",
            format!("'{type_id}' is not a script task type"),
        )]);
    };
    let empty = serde_json::Map::new();
    let map = match spec {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => {
            return Err(vec![refusal(
                CODE_INVALID,
                "",
                "a script task's properties must be a map",
            )])
        }
    };

    let mut out = ScriptTask {
        kind,
        container_image: None,
        commands: Vec::new(),
        script: None,
        before_commands: Vec::new(),
        interpreter: None,
        env: BTreeMap::new(),
        input_files: BTreeMap::new(),
        output_files: Vec::new(),
        output_directory: false,
        fail_fast: true,
        warning_on_std_err: true,
        task_runner: None,
        namespace_files: None,
    };
    let mut errs = Vec::new();
    let implemented = kind.implemented_properties();
    let refused = kind.refused_properties();
    let mut legacy_image: Option<String> = None;

    for (key, value) in map {
        let key = key.as_str();
        // An explicit `null` is Kestra's "absent" for every property.
        if value.is_null() {
            continue;
        }
        if HANDLED_BY_TASK_MODEL.contains(&key) {
            continue;
        }
        if key == "image" && is_alias(type_id) {
            if let Some(s) = string(value, key, &mut errs) {
                legacy_image = Some(s);
            }
            continue;
        }
        if refused.contains(&key) {
            errs.push(refusal(
                CODE_NOT_IMPLEMENTED,
                key,
                format!(
                    "property '{key}' is accepted by Kestra on {} but is not implemented by \
                     egret ({}); remove it",
                    kind.fqcn(),
                    crate::spec::keys::lift_of(crate::spec::keys::Site::Task(kind.fqcn()), key)
                        .expect("every refused script property has a lift_of row")
                        .short()
                ),
            ));
            continue;
        }
        if !implemented.contains(&key) {
            errs.push(refusal(
                CODE_UNKNOWN,
                key,
                format!(
                    "unknown property '{key}' on {}: neither Kestra nor egret accepts it",
                    kind.fqcn()
                ),
            ));
            continue;
        }
        match key {
            "containerImage" => out.container_image = string(value, key, &mut errs),
            "commands" => out.commands = string_list(value, key, &mut errs),
            "script" => out.script = string(value, key, &mut errs),
            "beforeCommands" => out.before_commands = string_list(value, key, &mut errs),
            "interpreter" => out.interpreter = Some(string_list(value, key, &mut errs)),
            "env" => out.env = string_map(value, key, &mut errs),
            "namespaceFiles" => {
                match crate::storage::nsfiles::stage::NamespaceFilesSpec::parse(value) {
                    Ok(spec) => out.namespace_files = Some(spec),
                    Err((p, message)) => {
                        let at = if p.is_empty() {
                            key.to_string()
                        } else {
                            format!("{key}.{p}")
                        };
                        errs.push(refusal(
                            CODE_INVALID,
                            at.clone(),
                            format!("'{at}' {message}"),
                        ));
                    }
                }
            }
            "inputFiles" => {
                if value.is_string() {
                    errs.push(refusal(
                        CODE_INPUT_FILES_URI,
                        key,
                        "'inputFiles' given as one storage URI or expression is not \
                         implemented by egret; give a map of file name to inline contents or to \
                         a kestra:// URI",
                    ));
                } else {
                    out.input_files = string_map(value, key, &mut errs);
                }
            }
            "outputFiles" => {
                out.output_files = string_list(value, key, &mut errs);
                // A literal entry is judged now (W5a Task 5 fix round 1):
                // `*`/`?` in the file name are implemented, `**`, `[...]`, a
                // wildcard directory and an escaping path are refused by
                // name. A `{{ }}` entry is judged after rendering, at run.
                // The list's own limits (fix round 3): entries and distinct
                // literal patterns; a `{{ }}` entry is counted at run.
                if let Some(problem) = crate::worker::commands::output_list_problem(
                    out.output_files
                        .iter()
                        .map(String::as_str)
                        .filter(|rel| !gate::is_unrendered(rel)),
                ) {
                    errs.push(refusal(
                        CODE_INVALID,
                        key,
                        format!("'outputFiles': {problem}"),
                    ));
                }
                for rel in &out.output_files {
                    if gate::is_unrendered(rel) {
                        continue;
                    }
                    if let Some(problem) = crate::worker::commands::output_problem(rel) {
                        errs.push(refusal(
                            CODE_INVALID,
                            key,
                            format!("'outputFiles': {problem}"),
                        ));
                    }
                }
            }
            "outputDirectory" => {
                out.output_directory = boolean(value, key, &mut errs).unwrap_or(false)
            }
            "failFast" => out.fail_fast = boolean(value, key, &mut errs).unwrap_or(true),
            "warningOnStdErr" => {
                out.warning_on_std_err = boolean(value, key, &mut errs).unwrap_or(true)
            }
            "taskRunner" => out.task_runner = parse_runner(value, &mut errs),
            // `timeout` is the `Task` model's; nothing to read here.
            "timeout" => {}
            other => unreachable!("implemented property '{other}' has no reader"),
        }
    }

    if let Some(legacy) = legacy_image {
        if out.container_image.is_some() {
            errs.push(refusal(
                CODE_IMAGE_CONFLICT,
                "image",
                "both 'image' and 'containerImage' are set; 'image' is the W4 spelling of \
                 'containerImage', keep one",
            ));
        } else {
            out.container_image = Some(legacy);
        }
    }

    let body = kind.body_property();
    let body_present = match body {
        "commands" => map.get("commands").is_some_and(|v| !v.is_null()),
        _ => map.get("script").is_some_and(|v| !v.is_null()),
    };
    if !body_present {
        errs.push(refusal(
            CODE_REQUIRED,
            body,
            format!("'{body}' is required on {}", kind.fqcn()),
        ));
    }

    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

fn parse_runner(value: &Value, errs: &mut Vec<ScriptRefusal>) -> Option<RunnerSpec> {
    let Value::Object(map) = value else {
        errs.push(refusal(
            CODE_INVALID,
            "taskRunner",
            "'taskRunner' must be a map",
        ));
        return None;
    };
    let Some(ty) = map.get("type").and_then(Value::as_str) else {
        errs.push(refusal(
            CODE_REQUIRED,
            "taskRunner.type",
            "'taskRunner.type' is required",
        ));
        return None;
    };
    let kind = match ty {
        DOCKER_RUNNER => RunnerKind::Docker,
        KUBERNETES_RUNNER => RunnerKind::Kubernetes,
        PROCESS_RUNNER => {
            errs.push(refusal(
                CODE_RUNNER_REFUSED,
                "taskRunner.type",
                format!(
                    "task runner '{PROCESS_RUNNER}' is refused by egret: it would run the \
                     task's commands inside the egret server; use '{DOCKER_RUNNER}' or \
                     '{KUBERNETES_RUNNER}'"
                ),
            ));
            return None;
        }
        other => {
            errs.push(refusal(
                CODE_RUNNER_UNKNOWN,
                "taskRunner.type",
                format!(
                    "unknown task runner '{other}'; egret implements '{DOCKER_RUNNER}' and \
                     '{KUBERNETES_RUNNER}'"
                ),
            ));
            return None;
        }
    };

    let mut spec = RunnerSpec::defaults(kind);
    let before = errs.len();
    for (key, value) in map {
        let key = key.as_str();
        let path = format!("taskRunner.{key}");
        if value.is_null() || key == "type" {
            continue;
        }
        let known = match kind {
            RunnerKind::Docker => DOCKER_IMPLEMENTED.contains(&key),
            RunnerKind::Kubernetes => KUBERNETES_IMPLEMENTED.contains(&key),
        };
        if !known {
            let (code, why) = match kind {
                RunnerKind::Docker if DOCKER_REFUSED.contains(&key) => (
                    CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED,
                    "is accepted by Kestra but is not implemented by egret (roadmap package #17)",
                ),
                RunnerKind::Docker => (
                    CODE_RUNNER_PROPERTY_UNKNOWN,
                    "is unknown: neither Kestra nor egret accepts it",
                ),
                RunnerKind::Kubernetes => (
                    CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED,
                    "is not implemented by egret (roadmap package #17); the Kubernetes runner accepts only \
                     pullPolicy, resources and delete",
                ),
            };
            errs.push(refusal(
                code,
                path,
                format!("task runner property '{key}' on {ty} {why}"),
            ));
            continue;
        }
        match key {
            "pullPolicy" => match value.as_str() {
                Some("ALWAYS") => spec.pull_policy = PullPolicy::Always,
                Some("IF_NOT_PRESENT") => spec.pull_policy = PullPolicy::IfNotPresent,
                Some("NEVER") => spec.pull_policy = PullPolicy::Never,
                _ => errs.push(refusal(
                    CODE_INVALID,
                    path,
                    format!(
                        "'taskRunner.pullPolicy' must be ALWAYS, IF_NOT_PRESENT or NEVER, \
                         got {value}"
                    ),
                )),
            },
            "delete" => {
                if let Some(b) = boolean(value, &path, errs) {
                    spec.delete = b;
                }
            }
            "cpu" => {
                spec.cpu = closed_object(value, &path, DOCKER_CPU_IMPLEMENTED, &[], errs)
                    .and_then(|mut m| m.remove("cpus"))
            }
            "memory" => {
                spec.memory = closed_object(
                    value,
                    &path,
                    DOCKER_MEMORY_IMPLEMENTED,
                    DOCKER_MEMORY_REFUSED,
                    errs,
                )
                .and_then(|mut m| m.remove("memory"))
            }
            "resources" => spec.resources = parse_resources(value, errs),
            other => unreachable!("runner property '{other}' has no reader"),
        }
    }
    (errs.len() == before).then_some(spec)
}

/// A Docker `cpu`/`memory` object: `implemented` keys read as scalars,
/// `refused` keys refused as not implemented, anything else unknown.
fn closed_object(
    value: &Value,
    path: &str,
    implemented: &[&str],
    refused: &[&str],
    errs: &mut Vec<ScriptRefusal>,
) -> Option<BTreeMap<String, String>> {
    let Value::Object(map) = value else {
        errs.push(refusal(
            CODE_INVALID,
            path,
            format!("'{path}' must be a map"),
        ));
        return None;
    };
    let mut out = BTreeMap::new();
    for (key, v) in map {
        let sub = format!("{path}.{key}");
        if v.is_null() {
            continue;
        }
        if implemented.contains(&key.as_str()) {
            if let Some(s) = scalar(v, &sub, errs) {
                out.insert(key.clone(), s);
            }
        } else if refused.contains(&key.as_str()) {
            errs.push(refusal(
                CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED,
                sub.clone(),
                format!("'{sub}' is accepted by Kestra but is not implemented by egret (roadmap package #17)"),
            ));
        } else {
            errs.push(refusal(
                CODE_RUNNER_PROPERTY_UNKNOWN,
                sub.clone(),
                format!("'{sub}' is unknown: neither Kestra nor egret accepts it"),
            ));
        }
    }
    Some(out)
}

fn parse_resources(value: &Value, errs: &mut Vec<ScriptRefusal>) -> Option<Resources> {
    let path = "taskRunner.resources";
    let Value::Object(map) = value else {
        errs.push(refusal(
            CODE_INVALID,
            path,
            format!("'{path}' must be a map"),
        ));
        return None;
    };
    let mut out = Resources::default();
    for (side, v) in map {
        let side_path = format!("{path}.{side}");
        let target = match side.as_str() {
            "request" => &mut out.request,
            "limit" => &mut out.limit,
            _ => {
                errs.push(refusal(
                    CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED,
                    side_path.clone(),
                    format!(
                        "'{side_path}' is not implemented by egret (roadmap package #17); resources accepts \
                         exactly request.cpu, request.memory, limit.cpu and limit.memory"
                    ),
                ));
                continue;
            }
        };
        if v.is_null() {
            continue;
        }
        let Value::Object(amounts) = v else {
            errs.push(refusal(
                CODE_INVALID,
                side_path.clone(),
                format!("'{side_path}' must be a map"),
            ));
            continue;
        };
        for (res, amount) in amounts {
            let res_path = format!("{side_path}.{res}");
            if amount.is_null() {
                continue;
            }
            let slot = match res.as_str() {
                "cpu" => &mut target.cpu,
                "memory" => &mut target.memory,
                _ => {
                    errs.push(refusal(
                        CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED,
                        res_path.clone(),
                        format!(
                            "'{res_path}' is not implemented by egret (roadmap package #17); resources \
                             accepts exactly request.cpu, request.memory, limit.cpu and \
                             limit.memory"
                        ),
                    ));
                    continue;
                }
            };
            *slot = scalar(amount, &res_path, errs);
        }
    }
    Some(out)
}

fn invalid(path: &str, what: &str, value: &Value) -> ScriptRefusal {
    refusal(
        CODE_INVALID,
        path,
        format!("'{path}' must be {what}, got {value}"),
    )
}

fn string(value: &Value, path: &str, errs: &mut Vec<ScriptRefusal>) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        other => {
            errs.push(invalid(path, "a string", other));
            None
        }
    }
}

/// A string, number or boolean, as its text (Jackson's coercion of a YAML
/// scalar into a `String` field).
fn scalar(value: &Value, path: &str, errs: &mut Vec<ScriptRefusal>) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => {
            errs.push(invalid(path, "a string, number or boolean", other));
            None
        }
    }
}

fn boolean(value: &Value, path: &str, errs: &mut Vec<ScriptRefusal>) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        other => {
            errs.push(invalid(
                path,
                "a boolean (an expression is not implemented by egret)",
                other,
            ));
            None
        }
    }
}

fn string_list(value: &Value, path: &str, errs: &mut Vec<ScriptRefusal>) -> Vec<String> {
    let Value::Array(items) = value else {
        errs.push(invalid(
            path,
            "a list of strings (a single expression is not implemented by egret)",
            value,
        ));
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        if let Some(s) = scalar(item, &format!("{path}[{i}]"), errs) {
            out.push(s);
        }
    }
    out
}

fn string_map(
    value: &Value,
    path: &str,
    errs: &mut Vec<ScriptRefusal>,
) -> BTreeMap<String, String> {
    let Value::Object(map) = value else {
        errs.push(invalid(path, "a map of strings", value));
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (k, v) in map {
        if let Some(s) = scalar(v, &format!("{path}.{k}"), errs) {
            out.insert(k.clone(), s);
        }
    }
    out
}

/// Every script task in `flow`, with its validation path, in all four root
/// lists and at every depth (`spec::path::every_task`) — through [`Task::children`] **and**
/// [`Task::error_children`]. `spec::validate`'s own walk visits
/// `children()` only, so a script task inside a flowable's `errors:` would
/// escape a refusal that relied on it; this walk is what `validate` and
/// `publish` both use for script tasks.
pub(crate) fn script_tasks(flow: &Flow) -> Vec<(String, &Task, ScriptKind, &Value)> {
    crate::spec::path::every_task(flow)
        .into_iter()
        .filter_map(|(path, task)| match (&task.kind, kind_of(&task.r#type)) {
            (TaskKind::Leaf { spec }, Some(kind)) => Some((path, task, kind, spec)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_minimal_commands_task_parses_with_the_measured_defaults() {
        let t = parse(SHELL_COMMANDS, &json!({"commands": ["echo hi"]})).unwrap();
        assert_eq!(t.image(), "ubuntu");
        assert!(t.fail_fast);
        assert_eq!(t.task_runner, None);
        assert_eq!(t.commands, vec!["echo hi"]);
    }

    /// Spec §5 / D4: a refusal names the roadmap package that lifts it, from
    /// `keys::lift_of`, and never a retired W-number.
    #[test]
    fn a_refused_script_property_names_the_package_that_lifts_it() {
        use crate::spec::keys::{lift_of, Site};
        let cases: [(&str, &str, &str); 5] = [
            (SHELL_COMMANDS, "logLevel", "roadmap package #13"),
            (SHELL_COMMANDS, "runner", "roadmap package #17"),
            (
                SHELL_COMMANDS,
                "workerSelector",
                "Kestra Enterprise only, ignored by Kestra OSS too, roadmap \u{a7}3.3",
            ),
            (PYTHON_SCRIPT, "pythonVersion", "roadmap package #17"),
            (PYTHON_COMMANDS, "assets", "roadmap package #13"),
        ];
        for (ty, key, lift) in cases {
            let body = kind_of(ty).expect("a script type").body_property();
            let body_value = if body == "commands" {
                json!(["x"])
            } else {
                json!("x")
            };
            let errs =
                parse(ty, &json!({body: body_value, key: "v"})).expect_err("a refused property");
            assert_eq!(errs.len(), 1, "{key}: {errs:?}");
            assert_eq!(errs[0].code, CODE_NOT_IMPLEMENTED);
            assert_eq!(
                errs[0].message,
                format!(
                    "property '{key}' is accepted by Kestra on {ty} but is not implemented by \
                     egret ({lift}); remove it"
                ),
                "{key}"
            );
            assert_eq!(lift_of(Site::Task(ty), key).unwrap().short(), lift);
        }
    }

    /// Spec §5: the runner properties, `resources` and the Kubernetes closed
    /// set say `(roadmap package #17)`.
    #[test]
    fn a_refused_runner_property_names_roadmap_package_17() {
        let run = |runner: serde_json::Value| {
            parse(
                SHELL_COMMANDS,
                &json!({"commands": ["x"], "taskRunner": runner}),
            )
            .expect_err("refused")
        };
        let errs = run(json!({"type": DOCKER_RUNNER, "volumes": ["/a:/b"]}));
        assert_eq!(
            errs[0].message,
            format!(
                "task runner property 'volumes' on {DOCKER_RUNNER} is accepted by Kestra but is \
                 not implemented by egret (roadmap package #17)"
            )
        );
        let errs = run(json!({"type": DOCKER_RUNNER, "memory": {"kernelMemory": "1"}}));
        assert_eq!(
            errs[0].message,
            "'taskRunner.memory.kernelMemory' is accepted by Kestra but is not implemented by \
             egret (roadmap package #17)"
        );
        let errs = run(json!({"type": KUBERNETES_RUNNER, "namespace": "x"}));
        assert_eq!(
            errs[0].message,
            format!(
                "task runner property 'namespace' on {KUBERNETES_RUNNER} is not implemented by \
                 egret (roadmap package #17); the Kubernetes runner accepts only pullPolicy, \
                 resources and delete"
            )
        );
        let errs = run(json!({"type": KUBERNETES_RUNNER, "resources": {"extra": {}}}));
        assert_eq!(
            errs[0].message,
            "'taskRunner.resources.extra' is not implemented by egret (roadmap package #17); \
             resources accepts exactly request.cpu, request.memory, limit.cpu and limit.memory"
        );
        let errs = run(json!({"type": KUBERNETES_RUNNER, "resources": {"limit": {"gpu": "1"}}}));
        assert_eq!(
            errs[0].message,
            "'taskRunner.resources.limit.gpu' is not implemented by egret (roadmap package \
             #17); resources accepts exactly request.cpu, request.memory, limit.cpu and \
             limit.memory"
        );
    }

    #[test]
    fn the_python_handlers_default_image_is_the_python_types_default() {
        assert_eq!(
            crate::worker::handlers::python::DEFAULT_IMAGE,
            default_image(ScriptKind::PythonScript)
        );
    }

    /// W9a spec §5: a map VALUE that is a `kestra://` URI (or a template that
    /// renders to one) is saved; whether it is READABLE is decided at run,
    /// through the render snapshot (`handlers::script`'s
    /// `an_unreadable_uri_fails_the_task_at_render_and_calls_no_runner`) --
    /// publish cannot know which execution's files exist.
    #[test]
    fn an_input_files_value_that_is_a_uri_is_accepted_at_publish_and_a_string_map_still_is() {
        let uri = "kestra:///w9a/storage/in-files/executions/x/tasks/w/y/a.txt";
        let t = parse(
            SHELL_COMMANDS,
            &json!({
                "commands": ["cat staged.txt"],
                "inputFiles": {
                    "staged.txt": uri,
                    "d/nested.txt": "{{ outputs.w.uri }}",
                    "literal.txt": "just contents",
                },
            }),
        )
        .expect("a map of URIs, templates and contents saves");
        assert_eq!(t.input_files["staged.txt"], uri);
        assert_eq!(t.input_files["d/nested.txt"], "{{ outputs.w.uri }}");
        assert_eq!(t.input_files["literal.txt"], "just contents");
    }

    #[test]
    fn input_files_as_a_whole_string_is_still_refused_with_the_code() {
        for whole in ["kestra:///ns/file.txt", "{{ outputs.w.uri }}"] {
            let errs = parse(
                SHELL_COMMANDS,
                &json!({"commands": ["x"], "inputFiles": whole}),
            )
            .expect_err("one string is not a map");
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert_eq!(errs[0].code, CODE_INPUT_FILES_URI);
            assert_eq!(errs[0].path, "inputFiles");
            assert!(
                !errs[0].message.contains("W5a"),
                "W9a implements URI values; the message must not say W5a: {}",
                errs[0].message
            );
            assert!(
                errs[0].message.contains("kestra://"),
                "it says a map value may be a URI: {}",
                errs[0].message
            );
        }
    }

    #[test]
    fn a_script_task_nested_in_an_errors_branch_is_found() {
        let flow = crate::spec::parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: l\n        type: core.log\n        message: x\n    errors:\n      - id: e\n        type: egret.container\n        commands: [x]\n",
        )
        .unwrap();
        let found: Vec<String> = script_tasks(&flow).into_iter().map(|(p, ..)| p).collect();
        assert_eq!(found, vec!["tasks.s.e"]);
    }

    /// W9b Task 5: `namespaceFiles` is implemented on all four types,
    /// parsed by the one `NamespaceFilesSpec::parse`, and a malformed block
    /// is refused naming the property inside it.
    #[test]
    fn namespace_files_is_implemented_and_a_malformed_block_is_refused_by_name() {
        for kind in ScriptKind::ALL {
            assert!(kind.implemented_properties().contains(&"namespaceFiles"));
            assert!(!kind.refused_properties().contains(&"namespaceFiles"));
            let body = |nf: serde_json::Value| match kind.body_property() {
                "commands" => json!({"commands": ["x"], "namespaceFiles": nf}),
                _ => json!({"script": "x", "namespaceFiles": nf}),
            };
            let parsed = parse(
                kind.fqcn(),
                &body(json!({"namespaces": ["a.b"], "ifExists": "WARN"})),
            )
            .unwrap_or_else(|e| panic!("{}: {e:?}", kind.fqcn()));
            let nf = parsed.namespace_files.expect("kept");
            assert_eq!(
                nf.if_exists,
                crate::storage::nsfiles::stage::IfExistsProp::Value(
                    crate::storage::nsfiles::stage::IfExists::Warn
                )
            );
            let errs = parse(kind.fqcn(), &body(json!({"namespaces": "a.b"}))).unwrap_err();
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert_eq!(
                (errs[0].code, errs[0].path.as_str()),
                (CODE_INVALID, "namespaceFiles.namespaces")
            );
            assert!(
                errs[0].message.contains("must be a list of strings"),
                "{errs:?}"
            );
            let errs = parse(kind.fqcn(), &body(json!("x"))).unwrap_err();
            assert_eq!(errs[0].path, "namespaceFiles", "{errs:?}");
            // `null` is absent.
            let parsed = parse(kind.fqcn(), &body(serde_json::Value::Null)).unwrap();
            assert!(parsed.namespace_files.is_none());
        }
    }

    /// W9a Task 6: `outputDirectory` is implemented on all four types, read
    /// as a boolean, and no longer refused -- at publish, through `parse`.
    #[test]
    fn output_directory_is_implemented_and_no_longer_refused() {
        for kind in ScriptKind::ALL {
            assert!(kind.implemented_properties().contains(&"outputDirectory"));
            assert!(!kind.refused_properties().contains(&"outputDirectory"));
            let body = match kind.body_property() {
                "commands" => json!({"commands": ["x"], "outputDirectory": true}),
                _ => json!({"script": "x", "outputDirectory": true}),
            };
            let parsed = parse(kind.fqcn(), &body)
                .unwrap_or_else(|e| panic!("{} saves with outputDirectory: {e:?}", kind.fqcn()));
            assert!(parsed.output_directory);
            let mut not_bool = body.clone();
            not_bool["outputDirectory"] = json!("{{ inputs.x }}");
            let errs = parse(kind.fqcn(), &not_bool).expect_err("a boolean, as Kestra's");
            assert!(errs.iter().any(|r| r.path == "outputDirectory"), "{errs:?}");
        }
        let parsed = parse(SHELL_COMMANDS, &json!({"commands": ["x"]})).unwrap();
        assert!(!parsed.output_directory, "default false");
    }
}
