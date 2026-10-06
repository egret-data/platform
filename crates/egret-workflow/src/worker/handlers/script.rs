//! **The one script handler** (W5a Task 5): Kestra's four script task types,
//! `io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`, and W4's two
//! aliases `egret.container` and `egret.script.python`, over the configured
//! [`TaskRunner`].
//!
//! # What one run does, in order (W5a spec §4)
//!
//! 1. **parse** the task's properties with [`scripts::parse`] -- the same
//!    function publish runs, so a draft that never passed publish (W13b's
//!    lesson) is refused here by the same named errors;
//! 2. **render** every property that can hold a `{{ }}` (`containerImage`,
//!    `commands`/`script`, `beforeCommands`, `env` values, `inputFiles`
//!    contents, `outputFiles`, `interpreter`), with `{{ workingDir }}` bound to
//!    the working directory (`w5a-scripts.json` `working_dir`);
//! 3. **gate** with [`gate::check`] -- the ONE gate function publish also calls
//!    -- on the **rendered** image, the task's runner, and the workspace's
//!    `workflow_containers` state **read now** ([`FeatureState`]), so turning
//!    the feature off stops the next run of a flow published while it was on;
//! 4. **choose the runner**: the task's `taskRunner:`, or the configured
//!    default ([`ScriptRuntime::runner_kind`]); a task naming a runner this
//!    deployment does not have fails, saying which one it has;
//! 5. **run** it.
//!
//! **Every refusal in 1-4 is `Outcome::failed_with(reason)` and no runner
//! creates anything**, so no container or pod exists for a refused run. The
//! one exception is a feature state that cannot be READ: that is egret's
//! outage, an `Err`, and the run stays leased for the reaper -- never run on
//! a guess.
//!
//! The one runner call a gate refusal makes is [`TaskRunner::adopt`], which
//! never creates: a container or pod that ALREADY exists for this
//! (run, attempt) passed the gate when it was created, and is followed to its
//! end (spec §4, "runs already in a pod finish"; the final review's must-fix
//! minor).
//!
//! The container or pod is deleted by [`TaskHandler::release`], which the
//! worker calls only after the outcome has committed (final review I3).
//!
//! # The script, as Kestra composes it (measured, `w5a-scripts.json`)
//!
//! One shell script, joined with `\n` ([`TaskCommands::argv`]):
//!
//! * no `beforeCommands`: `set -e` (iff `failFast`, default `true`), then the
//!   commands;
//! * with `beforeCommands`: `echo '##kestra:log:debug##'`, `set -e` (iff
//!   `failFast`), the before-commands, `echo '##kestra:log:info##'`, then the
//!   commands -- so the before-commands' output is logged at **DEBUG**
//!   (finding `before_commands`, `egret_expected.before_commands`);
//! * `shell.Script`: the `script` is inlined as the one command;
//! * `python.Script`: the `script` is written to `main.py` in the working
//!   directory and run as `python main.py` (Kestra writes `<digits>.py`; egret
//!   fixes the name, see `handlers::python`), under the environment
//!   [`ENV_DEFAULTS`] (both `python.*` types), the author's `env` over it.
//!
//! A non-zero exit logs Kestra's `Command failed with exit code <n>` at ERROR
//! and fails the task **with its outputs** (`exitCode`, `vars`, ...: finding
//! `failed_run_vars`).
//!
//! # `warningOnStdErr` has NO effect, and that is the measurement
//!
//! Task 1 measured `true`, `false` and unset alike: a script that writes to
//! stderr and exits 0 ends **SUCCESS**, its stderr logged at ERROR
//! (`egret_expected.warning_on_stderr`: `effect_on_state: none`). The plan's
//! "`warningOnStdErr` sets WARNING" was never measured and is not implemented.
//!
//! # Where egret differs from the artifact here, on the record
//!
//! * `outputFiles` follows Kestra (a non-recursive glob; a missing file
//!   ignored -- [`TaskCommands::output_files`]), and since W9a its value is
//!   the file's `kestra://` URI in internal storage, as Kestra's (W9a spec
//!   §6.2, breaking: W5a's value was the CONTENTS). `outputDirectory: true`
//!   collects everything under `{{ outputDir }}`. A task declaring either
//!   fails, before any runner, when egret has no `[storage]`. The output
//!   name's scheme is Kestra's (`IdUtils.from(path)-name`); its hash cannot
//!   be, since Kestra's working directory is a random temp dir.

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_config::PluginConfigurations;
use egret_core::{Error, Result};

use super::super::commands::{
    no_storage_for_outputs, relative_problem, CollectTo, LineObserver, Naming, NoObserver,
    StoredInput, TaskCommands, TaskRunner, LOG_DEBUG_MARKER, LOG_INFO_MARKER, OUTPUT_DIR,
    WORKING_DIR,
};
use super::super::{Handlers, Outcome, TaskHandler, TaskInput};
use super::container::{
    command_failed, mapped, outcome_for, render_all, render_values, EXIT_ON_ERROR,
};
use super::python::{NameHintSink, ENV_DEFAULTS, SCRIPT_FILE};
use crate::logs::LogLevel;
use crate::runners::files::is_namespace_file_uri;
use crate::runners::render_str;
use crate::scripts::{self, gate, RunnerKind, RunnerSpec, ScriptKind, ScriptTask};
use crate::storage::nsfiles::stage::{
    check_task_bytes, merge, resolve_input_uri, stage, RenderedNamespaceFiles, Staged,
};
use crate::storage::nsfiles::NsError;

/// The render variable Kestra binds to the working directory (`{{ workingDir }}`).
pub const WORKING_DIR_VAR: &str = "workingDir";

/// Kestra's `{{ outputDir }}` (`w9a-storage.json`
/// `egret_expected.output_directory.render_variable`), bound to
/// [`OUTPUT_DIR`] when `outputDirectory: true`.
pub const OUTPUT_DIR_VAR: &str = "outputDir";

/// Whether a workspace has `workflow_containers` ON -- the workspace half of
/// the gate's input, **read at run time** for every run.
#[async_trait::async_trait]
pub trait FeatureState: Send + Sync + std::fmt::Debug {
    /// `Err` is egret's own outage (the database did not answer): the run is
    /// not refused on a guess, it stays leased for the reaper.
    async fn containers_enabled(&self, workspace: uuid::Uuid) -> Result<bool>;

    /// Whether the workspace has `workflow_kestra_plugins` ON (W5b): the
    /// plugin handler's half of the gate, read at run time like
    /// [`containers_enabled`](Self::containers_enabled), with the same
    /// default-off rule. `Err` is egret's outage, as above.
    async fn kestra_plugins_enabled(&self, workspace: uuid::Uuid) -> Result<bool>;
}

/// [`FeatureState`] over `workspace_features`, through the same
/// [`effective_disabled`](egret_workspace::features::effective_disabled) the
/// `WsContext` gate and publish use, so an absent row means OFF
/// (`workflow_containers`'s `default_enabled`).
#[derive(Clone, Debug)]
pub struct PgFeatureState {
    pool: sqlx::PgPool,
}

impl PgFeatureState {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

impl PgFeatureState {
    /// Whether `key` is ON for `workspace`, through
    /// [`effective_disabled`](egret_workspace::features::effective_disabled):
    /// an absent row is the key's catalog default (OFF for both workflow
    /// keys). A dependency is not folded in here: the plugin gate reads
    /// `workflow_containers` itself (`plugins::gate`, rule 2).
    async fn enabled(&self, workspace: uuid::Uuid, key: &str, what: &str) -> Result<bool> {
        let rows: Vec<(String, bool)> = sqlx::query_as(
            "SELECT feature, enabled FROM workspace_features WHERE workspace_id = $1",
        )
        .bind(workspace)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            Error::Internal(anyhow::anyhow!(
                "reading the workspace's '{key}' feature for {what}: {e}"
            ))
        })?;
        Ok(!egret_workspace::features::effective_disabled(rows).contains(key))
    }
}

#[async_trait::async_trait]
impl FeatureState for PgFeatureState {
    async fn containers_enabled(&self, workspace: uuid::Uuid) -> Result<bool> {
        self.enabled(workspace, scripts::FEATURE_KEY, "a script task")
            .await
    }

    async fn kestra_plugins_enabled(&self, workspace: uuid::Uuid) -> Result<bool> {
        self.enabled(
            workspace,
            crate::plugins::FEATURE_KEY,
            "a Kestra plugin task",
        )
        .await
    }
}

/// A [`FeatureState`] that answers the same for every workspace -- for a test
/// that is not about the feature.
#[derive(Clone, Copy, Debug)]
pub struct FixedFeatureState(pub bool);

#[async_trait::async_trait]
impl FeatureState for FixedFeatureState {
    async fn containers_enabled(&self, _workspace: uuid::Uuid) -> Result<bool> {
        Ok(self.0)
    }
    async fn kestra_plugins_enabled(&self, _workspace: uuid::Uuid) -> Result<bool> {
        Ok(self.0)
    }
}

/// Everything a [`Script`] needs besides the task: built **once** per process
/// (`runtime::handlers_for`) and shared by the six registered spellings.
pub struct ScriptRuntime {
    /// Which runner [`runner`](Self::runner) is, i.e. `workflow.container_runner`
    /// -- also the runner of a task with no `taskRunner:`.
    pub runner_kind: RunnerKind,
    /// The one runner this deployment has.
    pub runner: Arc<dyn TaskRunner>,
    /// `workflow.plugin_configurations`: the allowlist [`gate::check`] reads.
    pub plugin_configurations: PluginConfigurations,
    /// The workspace half of the gate.
    pub features: Arc<dyn FeatureState>,
    /// Internal storage (`[storage]`), where a `kestra://` `inputFiles` value
    /// is read from (W9a spec §5). `None`: such a value fails the task.
    pub files: Option<Arc<crate::storage::FileStore>>,
    /// W9b Task 5: the database namespace files are listed and resolved on
    /// (`namespaceFiles`, and an `inputFiles` value naming a namespace file),
    /// one connection per run, outside any render. `None`: a task that
    /// declares either fails naming `[storage]`.
    pub pool: Option<sqlx::PgPool>,
}

impl std::fmt::Debug for ScriptRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptRuntime")
            .field("runner_kind", &self.runner_kind)
            .field("runner", &self.runner)
            .finish_non_exhaustive()
    }
}

/// Register the four script types -- and, through `add_publishable`, their
/// `TASK_TYPES` aliases `egret.container` and `egret.script.python` -- over
/// `rt`. Called only when `workflow.container_runner` is not `none`.
pub fn register(handlers: &mut Handlers, rt: Arc<ScriptRuntime>) {
    for kind in ScriptKind::ALL {
        handlers.add_publishable(Arc::new(Script::new(kind, rt.clone())));
    }
}

/// One script task type over the shared [`ScriptRuntime`].
#[derive(Debug)]
pub struct Script {
    kind: ScriptKind,
    rt: Arc<ScriptRuntime>,
}

impl Script {
    pub fn new(kind: ScriptKind, rt: Arc<ScriptRuntime>) -> Self {
        Self { kind, rt }
    }

    /// Step 2 of the module doc: every renderable property, and the script as
    /// Kestra composes it. `Err` is the task's failure, naming the property.
    fn render(
        &self,
        type_id: &str,
        task: &TaskInput,
        p: &ScriptTask,
    ) -> std::result::Result<Rendered, String> {
        let mut cx = task.render.clone();
        cx.insert(WORKING_DIR_VAR, serde_json::json!(WORKING_DIR));
        // Kestra's `{{ outputDir }}`, bound with the directory enabled
        // (`w9a-storage.json` `egret_expected.output_directory.render_variable`).
        if p.output_directory {
            cx.insert(OUTPUT_DIR_VAR, serde_json::json!(OUTPUT_DIR));
        }

        let one = |what: &str, s: &str| {
            render_str(s, &cx).map_err(|e| format!("{type_id}: `{what}` did not render: {e}"))
        };
        let image = one("containerImage", p.image())?;
        let interpreter = match &p.interpreter {
            Some(i) => render_all(type_id, "interpreter", i, &cx)?,
            None => Vec::new(),
        };
        let before = render_all(type_id, "beforeCommands", &p.before_commands, &cx)?;
        let author_env = render_values(type_id, "env", &p.env, &cx)?;
        let rendered_inputs = render_values(type_id, "inputFiles", &p.input_files, &cx)?;
        let (mut input_files, stored_inputs, ns_inputs) =
            self.stage_inputs(type_id, task, rendered_inputs)?;
        // W9b Task 5: the block's strings, with the same context, through
        // the one render path (which ends the render's namespace connection);
        // staged by `run` (`stage_namespace_files`), which has a connection.
        // An absent `namespaces` stays absent: `stage` reads it as the running
        // flow's own, after `[storage]` is checked (fix round 1, M5).
        let namespace_files = match &p.namespace_files {
            Some(nf) => Some(nf.render(|what, s| {
                render_str(s, &cx).map_err(|e| format!("{type_id}: `{what}` did not render: {e}"))
            })?),
            None => None,
        };
        let output_files = render_all(type_id, "outputFiles", &p.output_files, &cx)?;
        let script = p.script.as_deref().unwrap_or_default();

        let commands = match p.kind {
            ScriptKind::ShellCommands | ScriptKind::PythonCommands => {
                render_all(type_id, "commands", &p.commands, &cx)?
            }
            ScriptKind::ShellScript => vec![one("script", script)?],
            ScriptKind::PythonScript => {
                let body = one("script", script)?;
                if input_files.contains_key(SCRIPT_FILE)
                    || stored_inputs.contains_key(SCRIPT_FILE)
                    || ns_inputs.contains_key(SCRIPT_FILE)
                {
                    return Err(format!(
                        "{type_id}: `inputFiles` declares {SCRIPT_FILE:?}, which is where egret \
                         writes this task's `script`; name the file something else"
                    ));
                }
                input_files.insert(SCRIPT_FILE.to_string(), body);
                vec![format!("python {SCRIPT_FILE}")]
            }
        };

        // An input file's NAME is the author's: a problem with it is this
        // task's failure, decided before the gate and before any runner.
        for rel in input_files
            .keys()
            .chain(stored_inputs.keys())
            .chain(ns_inputs.keys())
        {
            if let Some(problem) = relative_problem(rel) {
                return Err(format!("{type_id}: `inputFiles`: {problem}"));
            }
        }

        let python = matches!(
            p.kind,
            ScriptKind::PythonCommands | ScriptKind::PythonScript
        );
        let mut env: BTreeMap<String, String> = if python {
            ENV_DEFAULTS
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        } else {
            BTreeMap::new()
        };
        env.extend(author_env);

        let cmds = TaskCommands {
            container_image: image,
            interpreter,
            before_commands: prefix(p.fail_fast, before),
            commands,
            env,
            input_files,
            stored_inputs,
            output_files,
            output_directory: p.output_directory,
            // Set by `run` once the commands render: see `collect_to`.
            collect_to: None,
            host_protocol: false,
        };
        Ok(Rendered {
            cmds,
            ns_inputs,
            namespace_files,
        })
    }

    /// W9b Task 5 (spec §7): resolve every `inputFiles` value that names a
    /// namespace file, stage the `namespaceFiles` block, and merge them into
    /// `cmds.stored_inputs` -- namespace files first, every `inputFiles`
    /// name over them whatever `ifExists` says (the user's ruling). One pool
    /// connection, taken here and returned before the runner is called; no
    /// render is involved, so no render connection is held.
    ///
    /// `Ok(Err(_))` is the task's failure, before the gate and before any
    /// runner; `Err` is egret's outage (the database or the store), so the
    /// run stays leased and is re-claimed with no attempt spent (spec §11).
    async fn stage_namespace_files(
        &self,
        type_id: &str,
        task: &TaskInput,
        cmds: &mut TaskCommands,
        ns_inputs: BTreeMap<String, String>,
        block: Option<RenderedNamespaceFiles>,
        script_file: Option<&str>,
    ) -> Result<std::result::Result<(), String>> {
        let block = block.filter(|b| b.enabled);
        if block.is_none() && ns_inputs.is_empty() {
            return Ok(Ok(()));
        }
        let fail = |why: String| format!("{type_id}: {why}");
        let (Some(pool), Some(files)) = (&self.rt.pool, &self.rt.files) else {
            return Ok(Err(fail(
                "namespace files cannot be staged: egret has no internal storage configured \
                 (the `[storage]` section of its configuration)"
                    .to_string(),
            )));
        };
        // Task 4's one source of the running flow's namespace: the
        // execution's flow, never a rendered `{{ flow.namespace }}`.
        let Some(own) = task.render.files().and_then(|f| f.0.flow_namespace()) else {
            return Ok(Err(fail(
                "namespace files cannot be staged: this run is not a task run of a flow's \
                 execution, whose namespace they are read as"
                    .to_string(),
            )));
        };
        let outage = |e: NsError| {
            Error::Internal(anyhow::anyhow!("{type_id}: staging namespace files: {e}"))
        };
        let mut resolved = BTreeMap::new();
        if !ns_inputs.is_empty() {
            let mut conn = pool.acquire().await.map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "{type_id}: staging namespace files: the database is unavailable: {e}"
                ))
            })?;
            for (name, uri) in ns_inputs {
                match resolve_input_uri(&mut conn, files, task.workspace_id, own, &uri)
                    .await
                    .map_err(outage)?
                {
                    Ok(input) => {
                        resolved.insert(name, input);
                    }
                    Err(why) => return Ok(Err(fail(format!("`inputFiles` {name:?}: {why}")))),
                }
            }
        }
        // `stage` takes its own connection for the listing and returns it
        // before matching (fix round 2 addendum).
        let staged = match block {
            None => Staged::default(),
            Some(b) => match stage(pool, files, task.workspace_id, own, &b)
                .await
                .map_err(outage)?
            {
                Ok(staged) => staged,
                Err(why) => return Ok(Err(fail(why))),
            },
        };
        for line in &staged.warnings {
            task.log.line(LogLevel::Warn, line).await;
        }
        // Fix round 1, M6 (the user's ruling of 2026-10-06, spec §7): the
        // task's own script file wins over a namespace file of its name, and
        // one WARN line names the namespace file it skipped. (An `inputFiles`
        // name over a namespace file is the user's earlier ruling, silent.)
        if let Some(script) = script_file {
            if let Some(skipped) = staged.files.get(script) {
                let ns = staged.sources.get(script).map_or("", String::as_str);
                task.log
                    .line(
                        LogLevel::Warn,
                        &format!(
                            "namespaceFiles: namespace file '{script}' of namespace '{ns}' \
                             ({}) was not staged: {type_id} writes its `script` to \
                             '{script}', and the task's script wins",
                            skipped.uri
                        ),
                    )
                    .await;
            }
        }
        let mut stored = std::mem::take(&mut cmds.stored_inputs);
        stored.extend(resolved);
        cmds.stored_inputs = merge(staged.files, &cmds.input_files, stored);
        // Fix round 1, M1: over what the runner will stream, after the merge.
        if let Err(why) = check_task_bytes(&cmds.stored_inputs, files.caps.max_task_bytes) {
            return Ok(Err(fail(why)));
        }
        Ok(Ok(()))
    }

    /// Where this run's output files go (W9a spec §5, §6.2): its workspace
    /// and its own task prefix, the render scope's `task_prefix` -- the
    /// writing execution's, an iteration's inside a `Loop`. `Err` is the
    /// task's failure, decided before the gate and before any runner:
    ///
    /// * no internal storage: W5a's contents-in-outputs path is gone, so a
    ///   task declaring an output fails naming `[storage]`;
    /// * a namespace, flow or task id no URI can carry (`uri::task_prefix`'s
    ///   `Err`, which the claim dropped when it built the scope): the
    ///   AUTHOR's error, named here -- never `Internal`, which would fence
    ///   the commit and re-claim the run for ever.
    fn collect_to(
        &self,
        type_id: &str,
        task: &TaskInput,
    ) -> std::result::Result<CollectTo, String> {
        let fail = |why: String| format!("{type_id}: {why}");
        if self.rt.files.is_none() {
            return Err(fail(no_storage_for_outputs()));
        }
        let to = |task_prefix: String| CollectTo {
            workspace: task.workspace_id,
            task_prefix,
            naming: Naming::KestraOutputName,
        };
        if let Some(prefix) = task.render.files().and_then(|f| f.0.task_prefix()) {
            return Ok(to(prefix.to_string()));
        }
        let flow = (
            render_str("{{ flow.namespace }}", &task.render),
            render_str("{{ flow.id }}", &task.render),
        );
        let (Ok(namespace), Ok(flow_id)) = flow else {
            return Err(fail(
                "this run has no storage prefix for its output files: it is not a task run of \
                 a flow's execution"
                    .to_string(),
            ));
        };
        crate::storage::uri::task_prefix(
            &namespace,
            &flow_id,
            task.execution_id,
            &task.task_id,
            task.run_id,
        )
        .map(to)
        .map_err(fail)
    }

    /// W9a spec §5: a rendered `inputFiles` value that is a `kestra://` URI is
    /// a file in internal storage, staged under its name; any other value
    /// stays contents (W5a, §6.1).
    ///
    /// The URI is resolved through the run's render snapshot -- the same
    /// readability decision `read()` and `fileExists()` make, taken before
    /// the render -- so an unreadable, missing or assembled URI fails the
    /// task HERE, before the gate and before any runner is called. The size
    /// is the index row's.
    fn stage_inputs(
        &self,
        type_id: &str,
        task: &TaskInput,
        rendered: BTreeMap<String, String>,
    ) -> std::result::Result<StagedInputs, String> {
        let mut contents = BTreeMap::new();
        let mut stored = BTreeMap::new();
        let mut namespace = BTreeMap::new();
        for (name, value) in rendered {
            // W9b Task 5 (the user's ruling on item 9): a namespace-file URI
            // is resolved by `run`, which has a connection. A bare path stays
            // contents, as W5a/W9a (Kestra writes a non-URI string as is).
            if is_namespace_file_uri(&value) {
                namespace.insert(name, value);
                continue;
            }
            if !value.starts_with(STORAGE_SCHEME) {
                contents.insert(name, value);
                continue;
            }
            let fail = |why: &str| format!("{type_id}: `inputFiles` {name:?}: {why}");
            let Some(scope) = task.render.files() else {
                return Err(fail(&format!(
                    "{value} cannot be read: this render has no file scope"
                )));
            };
            let meta = scope.0.lookup(&value).map_err(|u| fail(u.message()))?;
            let Some(files) = &self.rt.files else {
                return Err(fail(&format!(
                    "{value} cannot be read: egret has no internal storage configured (the \
                     `[storage]` section of its configuration)"
                )));
            };
            let object_uri = files
                .object_uri(task.workspace_id, &value)
                .map_err(|e| fail(&e))?;
            stored.insert(
                name,
                StoredInput {
                    uri: value,
                    object_uri,
                    size: meta.size,
                },
            );
        }
        Ok((contents, stored, namespace))
    }
}

/// `inputFiles` split: inline contents, files staged from storage, and
/// namespace-file URIs (name -> URI) `run` resolves.
type StagedInputs = (
    BTreeMap<String, String>,
    BTreeMap<String, StoredInput>,
    BTreeMap<String, String>,
);

/// What [`Script::render`] hands `run`: the commands, and what `run` stages
/// from namespace files before anything else uses them.
struct Rendered {
    cmds: TaskCommands,
    /// `inputFiles` values naming a namespace file, by input name.
    ns_inputs: BTreeMap<String, String>,
    /// The rendered `namespaceFiles` block, if the task has one.
    namespace_files: Option<RenderedNamespaceFiles>,
}

/// What makes an `inputFiles` value a file in internal storage rather than
/// contents -- Kestra's own test (`value.startsWith("kestra://")`). A value
/// with the scheme that does not parse is refused by the snapshot, naming it.
const STORAGE_SCHEME: &str = "kestra://";

/// The lines ahead of the commands, as the artifact writes them (module doc):
/// `set -e` iff `fail_fast`, and a DEBUG bracket around `before` when there
/// are any.
fn prefix(fail_fast: bool, before: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let bracket = !before.is_empty();
    if bracket {
        out.push(format!("echo '{LOG_DEBUG_MARKER}'"));
    }
    if fail_fast {
        out.push(EXIT_ON_ERROR.to_string());
    }
    if bracket {
        out.extend(before);
        out.push(format!("echo '{LOG_INFO_MARKER}'"));
    }
    out
}

#[async_trait::async_trait]
impl TaskHandler for Script {
    fn type_id(&self) -> &'static str {
        self.kind.fqcn()
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        // The spelling the author wrote: the aliases accept W4's `image`, and
        // every message names what the flow says.
        let type_id = task.type_id.as_str();

        // 1. parse -- publish's own function.
        let parsed = match scripts::parse(type_id, &task.spec) {
            Ok(p) => p,
            Err(refusals) => {
                let why: Vec<&str> = refusals.iter().map(|r| r.message.as_str()).collect();
                return Ok(Outcome::failed_with(format!(
                    "{type_id}: {}",
                    why.join("; ")
                )));
            }
        };

        // 2. render.
        let Rendered {
            mut cmds,
            ns_inputs,
            namespace_files,
        } = match self.render(type_id, task, &parsed) {
            Ok(r) => r,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        // 2b. namespace files (W9b Task 5): staged before anything uses the
        // inputs, and before the gate, so a refusal calls no runner.
        if let Err(why) = self
            .stage_namespace_files(
                type_id,
                task,
                &mut cmds,
                ns_inputs,
                namespace_files,
                (parsed.kind == ScriptKind::PythonScript).then_some(SCRIPT_FILE),
            )
            .await?
        {
            return Ok(Outcome::failed_with(why));
        }
        if cmds.has_outputs() {
            cmds.collect_to = match self.collect_to(type_id, task) {
                Ok(to) => Some(to),
                Err(why) => return Ok(Outcome::failed_with(why)),
            };
        }

        // 3. the gate, on the RENDERED image, with the feature read now.
        let spec = parsed
            .task_runner
            .clone()
            .unwrap_or_else(|| RunnerSpec::defaults(self.rt.runner_kind));
        let feature_enabled = self
            .rt
            .features
            .containers_enabled(task.workspace_id)
            .await?;
        let refused = gate::check(&gate::GateInput {
            feature_enabled,
            image: &cmds.container_image,
            runner: spec.kind,
            plugin_configurations: &self.rt.plugin_configurations,
        })
        .err();

        let hint = NameHintSink::default();
        let python = matches!(
            parsed.kind,
            ScriptKind::PythonCommands | ScriptKind::PythonScript
        );
        let observer: &dyn LineObserver = if python { &hint } else { &NoObserver };

        let ran = match refused {
            // **A refusal is final only if nothing is running yet** (spec §4:
            // "runs already in a pod finish"; the final review's must-fix
            // minor). A pod or container that already exists for this
            // (run, attempt) passed the gate when it was created; a re-claim
            // after the feature was turned off -- or the allowlist narrowed --
            // adopts it and finishes. `adopt` never creates, so the gate still
            // stands between the task and any NEW work.
            Some(refusal) => {
                let adopted = if spec.kind == self.rt.runner_kind {
                    self.rt.runner.adopt(task, &cmds, &spec, observer).await
                } else {
                    Ok(None)
                };
                match adopted {
                    Ok(Some(result)) => Ok(result),
                    Ok(None) => return Ok(Outcome::failed_with(format!("{type_id}: {refusal}"))),
                    Err(e) => Err(e),
                }
            }
            None => {
                // 4. the runner.
                if spec.kind != self.rt.runner_kind {
                    return Ok(Outcome::failed_with(format!(
                        "{type_id}: {}",
                        scripts::runner_unavailable(spec.kind, self.rt.runner_kind)
                    )));
                }
                // 5. run.
                self.rt.runner.run(task, &cmds, &spec, observer).await
            }
        };
        match ran {
            Ok(result) => {
                if result.exit_code != 0 {
                    task.log
                        .line(LogLevel::Error, &command_failed(result.exit_code))
                        .await;
                }
                // Fix round 1, I2 (spec §5): the run's own prefix whether or
                // not it declared an output -- a marker may name a URI under
                // it either way, and must not point at nothing.
                let prefix = cmds
                    .collect_to
                    .as_ref()
                    .map(|c| c.task_prefix.clone())
                    .or_else(|| {
                        task.render
                            .files()
                            .and_then(|f| f.0.task_prefix())
                            .map(str::to_string)
                    });
                let mut outcome = outcome_for(type_id, &result, prefix.as_deref());
                if let (Some(h), Some(message)) = (hint.hint(), outcome.message.as_mut()) {
                    message.push_str(". ");
                    message.push_str(&h);
                }
                Ok(outcome)
            }
            Err(e) => mapped(type_id, e),
        }
    }

    /// **Real cancellation**: the configured runner removes the attempt's
    /// container or pod (`TaskRunner::cancel`). The only runner that can have
    /// created anything for this task is the configured one -- a task naming
    /// another is refused before any runner is called -- so there is nothing
    /// to choose.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        self.rt.runner.cancel(task, std::time::Duration::ZERO).await
    }

    /// **After the outcome committed** (`claim::run_one`; final review I3):
    /// the configured runner deletes the attempt's container or pod, or keeps
    /// it for `taskRunner.delete: false`. The runner's `run` never does.
    ///
    /// The spec is re-parsed from the task -- `delete` is a literal boolean
    /// at save (a `{{ }}` there is refused), so this is the value `run` saw.
    /// A task that does not parse, or names another runner, never reached a
    /// runner, so there is nothing to free.
    async fn release(&self, task: &TaskInput) -> Result<()> {
        let Ok(parsed) = scripts::parse(task.type_id.as_str(), &task.spec) else {
            return Ok(());
        };
        let spec = parsed
            .task_runner
            .unwrap_or_else(|| RunnerSpec::defaults(self.rt.runner_kind));
        if spec.kind != self.rt.runner_kind {
            return Ok(());
        }
        self.rt.runner.release(task, &spec).await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::container::testing::Spy;
    use super::*;
    use crate::executor::RunState;
    use crate::worker::commands::CommandsResult;
    use crate::worker::handlers::testing::{input, recorded};
    use egret_config::{PluginConfiguration, PluginConfigurationValues};

    fn allow(runner: RunnerKind, images: &[&str]) -> PluginConfigurations {
        PluginConfigurations(vec![PluginConfiguration {
            r#type: runner.fqcn().to_string(),
            values: PluginConfigurationValues {
                allowed_images: Some(images.iter().map(|s| s.to_string()).collect()),
                allowed_types: None,
            },
        }])
    }

    /// Everything allowed on Docker: the feature on, any image.
    fn open(spy: Arc<Spy>) -> Script {
        script(scripts::SHELL_COMMANDS, spy, true, &["*", "*/*", "*/*/*"])
    }

    fn script(type_id: &str, spy: Arc<Spy>, feature: bool, images: &[&str]) -> Script {
        let kind = scripts::kind_of(type_id).expect("a script type");
        Script::new(
            kind,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: spy,
                plugin_configurations: allow(RunnerKind::Docker, images),
                features: Arc::new(FixedFeatureState(feature)),
                files: None,
                pool: None,
            }),
        )
    }

    fn alias_spec() -> serde_json::Value {
        serde_json::json!({
            "image": "alpine:3.20",
            "commands": ["echo one", "echo two"],
        })
    }

    // ---- the run-time gate --------------------------------------------------

    /// **A disabled feature fails the run and the runner is never called.**
    #[tokio::test]
    async fn a_disabled_feature_fails_the_run_and_calls_no_runner() {
        let spy = Spy::ok(CommandsResult::default());
        let h = script(scripts::ALIAS_CONTAINER, spy.clone(), false, &["*"]);
        let out = h
            .run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect("a refusal is the task's failure, not the worker's");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .expect("a message")
                .contains("'workflow_containers' feature is disabled"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty(), "no runner call");
    }

    /// **The image is gated AFTER rendering**: a `{{ }}` that renders outside
    /// the allowlist fails the run with no runner call; the same template
    /// rendering inside it runs (the control).
    #[tokio::test]
    async fn a_rendered_image_outside_the_allowlist_fails_and_calls_no_runner() {
        let run_with = |tag: &'static str| async move {
            let spy = Spy::ok(CommandsResult::default());
            let h = script(scripts::SHELL_COMMANDS, spy.clone(), true, &["ubuntu:*"]);
            let task = TaskInput {
                render: crate::runners::RenderContext::from_json_object(
                    &serde_json::json!({"inputs": {"image": tag}}),
                )
                .expect("an object"),
                ..input(
                    scripts::SHELL_COMMANDS,
                    serde_json::json!({
                        "containerImage": "{{ inputs.image }}",
                        "commands": ["true"],
                    }),
                )
            };
            let out = h.run(&task).await.expect("no worker error");
            let calls = spy.seen.lock().unwrap().len();
            (out, calls)
        };
        let (out, calls) = run_with("alpine:3").await;
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .expect("a message")
                .contains("image 'alpine:3'"),
            "{:?}",
            out.message
        );
        assert_eq!(calls, 0, "a refused image reaches no runner");

        let (out, calls) = run_with("ubuntu:24.04").await;
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(calls, 1);
    }

    /// **The default image is gated too** -- `ubuntu` matched as
    /// `ubuntu:latest` (Task 1's normalisation).
    #[tokio::test]
    async fn the_default_image_is_gated_as_ubuntu_latest() {
        let spy = Spy::ok(CommandsResult::default());
        let h = script(scripts::SHELL_COMMANDS, spy.clone(), true, &["python:*"]);
        let out = h
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({"commands": ["true"]}),
            ))
            .await
            .expect("no worker error");
        assert!(
            out.message
                .as_deref()
                .unwrap_or_default()
                .contains("(matched as 'ubuntu:latest')"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// **No `taskRunner:` runs on the configured runner, with its defaults;
    /// an explicit one is passed through; the OTHER runner is refused.**
    #[tokio::test]
    async fn the_default_runner_is_the_configured_one() {
        let spy = Spy::ok(CommandsResult::default());
        let h = open(spy.clone());
        h.run(&input(
            scripts::SHELL_COMMANDS,
            serde_json::json!({"commands": ["true"]}),
        ))
        .await
        .expect("no worker error");
        assert_eq!(
            spy.specs.lock().unwrap().as_slice(),
            [RunnerSpec::defaults(RunnerKind::Docker)]
        );

        let spy = Spy::ok(CommandsResult::default());
        let h = open(spy.clone());
        h.run(&input(
            scripts::SHELL_COMMANDS,
            serde_json::json!({
                "commands": ["true"],
                "taskRunner": {
                    "type": scripts::DOCKER_RUNNER,
                    "pullPolicy": "ALWAYS",
                    "delete": false,
                    "cpu": {"cpus": "0.5"},
                },
            }),
        ))
        .await
        .expect("no worker error");
        let specs = spy.specs.lock().unwrap().clone();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].pull_policy, scripts::PullPolicy::Always);
        assert!(!specs[0].delete);
        assert_eq!(specs[0].cpu.as_deref(), Some("0.5"));

        let spy = Spy::ok(CommandsResult::default());
        let h = Script::new(
            ScriptKind::ShellCommands,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: spy.clone(),
                plugin_configurations: PluginConfigurations(vec![
                    allow(RunnerKind::Docker, &["*"]).0.remove(0),
                    allow(RunnerKind::Kubernetes, &["*"]).0.remove(0),
                ]),
                features: Arc::new(FixedFeatureState(true)),
                files: None,
                pool: None,
            }),
        );
        let out = h
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({
                    "commands": ["true"],
                    "taskRunner": {"type": scripts::KUBERNETES_RUNNER},
                }),
            ))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .unwrap_or_default()
                .contains("this egret runs script tasks on"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// A property publish would refuse is refused at run too, by the same
    /// name, with no runner call (a draft that skipped publish, W13b).
    #[tokio::test]
    async fn a_property_publish_refuses_is_refused_at_run_and_calls_no_runner() {
        let spy = Spy::ok(CommandsResult::default());
        let h = open(spy.clone());
        let out = h
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({"commands": ["true"], "outputfiles": ["a"]}),
            ))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .unwrap_or_default()
                .contains("outputfiles"),
            "{:?}",
            out.message
        );
        let out = h
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({
                    "commands": ["true"],
                    "taskRunner": {"type": scripts::PROCESS_RUNNER},
                }),
            ))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    // ---- the script, as Kestra composes it ------------------------------------

    /// The alias keeps W4's shape: `image`, and `set -e` in front.
    #[tokio::test]
    async fn the_alias_becomes_one_shell_script_with_set_e_in_front() {
        let spy = Spy::ok(CommandsResult::default());
        let h = open(spy.clone());
        let out = h
            .run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success);
        let cmds = spy.only();
        assert_eq!(cmds.container_image, "alpine:3.20");
        assert_eq!(
            cmds.argv(),
            vec!["/bin/sh", "-c", "set -e\necho one\necho two"]
        );
        assert!(cmds.env.is_empty(), "a shell task gets no Python env");
    }

    /// `failFast: false` drops the `set -e`, and only that.
    #[tokio::test]
    async fn fail_fast_false_drops_the_set_e_and_only_that() {
        let spy = Spy::ok(CommandsResult::default());
        let mut s = alias_spec();
        s["failFast"] = serde_json::json!(false);
        open(spy.clone())
            .run(&input(scripts::ALIAS_CONTAINER, s))
            .await
            .expect("no worker error");
        assert_eq!(
            spy.only().argv(),
            vec!["/bin/sh", "-c", "echo one\necho two"]
        );
    }

    /// **`beforeCommands` run inside a DEBUG bracket, `set -e` iff `failFast`**
    /// -- the exact script Task 1 measured for both values
    /// (`egret_expected.before_commands.cases`).
    #[tokio::test]
    async fn before_commands_are_bracketed_at_debug_as_measured() {
        let cases = fixture_before_commands();
        assert_eq!(cases.len(), 2, "both failFast values are measured");
        for case in cases {
            let fail_fast = case["failFast"].as_bool().expect("failFast");
            let spy = Spy::ok(CommandsResult::default());
            open(spy.clone())
                .run(&input(
                    scripts::SHELL_COMMANDS,
                    serde_json::json!({
                        "commands": case["commands"],
                        "beforeCommands": case["beforeCommands"],
                        "failFast": fail_fast,
                    }),
                ))
                .await
                .expect("no worker error");
            let set_e = if case["set_e_prepended"].as_bool().expect("flag") {
                "set -e\n"
            } else {
                ""
            };
            assert_eq!(
                spy.only().argv(),
                vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    format!(
                        "echo '##kestra:log:debug##'\n{set_e}echo before-1\nfalse\necho \
                         '##kestra:log:info##'\necho main"
                    ),
                ],
                "failFast {fail_fast}"
            );
        }
    }

    /// Task 1's measurement, `oracle-exec/w5a-scripts.json`.
    fn w5a() -> serde_json::Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w5a-scripts.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
    }

    fn fixture_before_commands() -> Vec<serde_json::Value> {
        w5a()["egret_expected"]["before_commands"]["cases"]
            .as_array()
            .expect("cases")
            .clone()
    }

    /// And through the shipping `stream_line`, the bracket puts the
    /// before-commands' output at DEBUG and the commands' at INFO.
    #[tokio::test]
    async fn the_before_commands_output_is_logged_at_debug() {
        let (task, rec) = recorded(
            scripts::SHELL_COMMANDS,
            serde_json::json!({"commands": ["echo main"], "beforeCommands": ["echo before-1"]}),
        );
        let spy = Spy::ok(CommandsResult::default()).streaming(vec![
            (false, "##kestra:log:debug##".into()),
            (false, "before-1".into()),
            (false, "##kestra:log:info##".into()),
            (false, "main".into()),
        ]);
        open(spy).run(&task).await.expect("no worker error");
        assert_eq!(
            rec.texts(),
            vec![
                (LogLevel::Debug, "before-1".to_string()),
                (LogLevel::Info, "main".to_string()),
            ]
        );
    }

    /// `shell.Script` inlines the script after `set -e` (finding
    /// `script_composition`).
    #[tokio::test]
    async fn shell_script_inlines_the_script_after_set_e() {
        let spy = Spy::ok(CommandsResult::default());
        let h = script(scripts::SHELL_SCRIPT, spy.clone(), true, &["*"]);
        h.run(&input(
            scripts::SHELL_SCRIPT,
            serde_json::json!({"script": "echo a\necho b"}),
        ))
        .await
        .expect("no worker error");
        assert_eq!(
            spy.only().argv(),
            vec!["/bin/sh", "-c", "set -e\necho a\necho b"]
        );
    }

    /// `python.Script`: the script becomes `main.py`, run by `python`, under
    /// the measured environment with the author's entries over it, on the
    /// default image.
    #[tokio::test]
    async fn python_script_is_written_to_a_file_and_run_by_python() {
        let spy = Spy::ok(CommandsResult::default());
        let h = script(scripts::PYTHON_SCRIPT, spy.clone(), true, &["python:*"]);
        let out = h
            .run(&input(
                scripts::PYTHON_SCRIPT,
                serde_json::json!({
                    "script": "print('hi')",
                    "env": {"PYTHONUNBUFFERED": "false", "MINE": "1"},
                }),
            ))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        assert_eq!(cmds.container_image, "python:3.13-slim");
        assert_eq!(
            cmds.input_files.get(SCRIPT_FILE).map(String::as_str),
            Some("print('hi')")
        );
        assert_eq!(cmds.argv(), vec!["/bin/sh", "-c", "set -e\npython main.py"]);
        assert_eq!(
            cmds.env.get("PYTHONUNBUFFERED").map(String::as_str),
            Some("false")
        );
        assert_eq!(cmds.env.get("MINE").map(String::as_str), Some("1"));
        assert_eq!(
            cmds.env.get("PIP_ROOT_USER_ACTION").map(String::as_str),
            Some("ignore")
        );

        // And an author input file cannot take `main.py`'s place.
        let spy = Spy::ok(CommandsResult::default());
        let h = script(scripts::PYTHON_SCRIPT, spy.clone(), true, &["python:*"]);
        let out = h
            .run(&input(
                scripts::PYTHON_SCRIPT,
                serde_json::json!({"script": "pass", "inputFiles": {"main.py": "x"}}),
            ))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    // ---- W9a Task 5: `inputFiles` values that are kestra:// URIs ----------

    const WS: uuid::Uuid = uuid::Uuid::from_u128(0x5eed);

    /// `w9a-storage.json` `input_files_uri`'s rendered URI, in egret's form.
    fn staged_uri() -> String {
        format!(
            "kestra:///w9a/storage/in-files/executions/{}/tasks/w/{}/12811733253095936987.txt",
            uuid::Uuid::from_u128(1),
            uuid::Uuid::from_u128(2)
        )
    }

    fn mem_files() -> Arc<crate::storage::FileStore> {
        Arc::new(crate::storage::FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: crate::storage::Caps::default(),
            retention_days: 30,
        })
    }

    /// A script handler over `spy` with internal storage `files`.
    fn stored(spy: Arc<Spy>, files: Arc<crate::storage::FileStore>) -> Script {
        Script::new(
            ScriptKind::ShellCommands,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: spy,
                plugin_configurations: allow(RunnerKind::Docker, &["*"]),
                features: Arc::new(FixedFeatureState(true)),
                files: Some(files),
                pool: None,
            }),
        )
    }

    /// `w9a-storage.json` `input_files_uri`'s flow, as a task input whose
    /// render context has `outputs.w.uri` and a snapshot resolving `uri` to
    /// `resolution`.
    fn in_files_task(
        files: &Arc<crate::storage::FileStore>,
        uri: &str,
        resolution: std::result::Result<crate::storage::IndexedFile, crate::storage::Unreadable>,
    ) -> TaskInput {
        let snapshot = crate::storage::scope::Snapshot::from_parts(
            std::collections::BTreeMap::from([(uri.to_string(), resolution)]),
            files.clone(),
            WS,
            None,
        );
        let mut cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "outputs": {"w": {"uri": uri}},
        }))
        .expect("an object")
        .with_files(snapshot.into_scope());
        cx.insert("flow", serde_json::json!({"id": "in-files"}));
        let mut task = input(
            scripts::SHELL_COMMANDS,
            serde_json::json!({
                "containerImage": "alpine:3.20",
                "inputFiles": {
                    "staged.txt": "{{ outputs.w.uri }}",
                    "d/nested.txt": "{{ outputs.w.uri }}",
                    "literal.txt": "just contents",
                },
                "commands": ["cat staged.txt d/nested.txt literal.txt"],
            }),
        );
        task.workspace_id = WS;
        task.render = cx;
        task
    }

    fn indexed(uri: &str, size: u64) -> crate::storage::IndexedFile {
        crate::storage::IndexedFile {
            uri: uri.to_string(),
            execution_id: uuid::Uuid::from_u128(1),
            task_run_id: Some(uuid::Uuid::from_u128(2)),
            size,
        }
    }

    /// `w9a-storage.json` `input_files_uri`: a URI value lands under its key
    /// (sub-directories too) and the task never sees the URI; a plain value
    /// stays contents.
    #[tokio::test]
    async fn a_uri_input_is_staged_under_its_name_and_never_as_contents() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/w9a-storage.json");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read w9a-storage.json"))
                .expect("parse w9a-storage.json");
        let fixture = &doc["egret_expected"]["input_files_uri"];
        assert_eq!(fixture["basis"], "measured");
        assert_eq!(fixture["staged"][0]["task_reads"], "staged content");
        let files = mem_files();
        let uri = staged_uri();
        let spy = Spy::ok(CommandsResult::default());
        let out = stored(spy.clone(), files.clone())
            .run(&in_files_task(&files, &uri, Ok(indexed(&uri, 14))))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        let want = crate::worker::commands::StoredInput {
            uri: uri.clone(),
            object_uri: files.object_uri(WS, &uri).unwrap(),
            size: 14,
        };
        assert_eq!(
            cmds.stored_inputs,
            BTreeMap::from([
                ("d/nested.txt".to_string(), want.clone()),
                ("staged.txt".to_string(), want),
            ])
        );
        assert_eq!(
            cmds.input_files,
            BTreeMap::from([("literal.txt".to_string(), "just contents".to_string())]),
            "a URI is never passed as contents"
        );
        assert!(
            cmds.input_files.values().all(|v| !v.contains("kestra://")),
            "{:?}",
            cmds.input_files
        );
        assert!(
            cmds.stored_inputs["staged.txt"]
                .object_uri
                .contains(&WS.hyphenated().to_string()),
            "the object is in the RUN's workspace"
        );
        let shown = format!("{cmds:?}");
        assert!(
            shown.contains("\"staged.txt\": \"<14 bytes stored>\""),
            "{shown}"
        );
    }

    /// An object store whose every call fails as an unreachable S3 does
    /// (also the plugin bridge's outage test, W9b Task 10).
    pub(crate) struct DownStore;
    #[async_trait::async_trait]
    impl egret_storage::ObjectStore for DownStore {
        async fn put(
            &self,
            _: &str,
            _: Vec<u8>,
        ) -> std::result::Result<(), egret_storage::StorageError> {
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
        async fn get(&self, _: &str) -> std::result::Result<Vec<u8>, egret_storage::StorageError> {
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
        async fn delete(&self, _: &str) -> std::result::Result<(), egret_storage::StorageError> {
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
        async fn copy(
            &self,
            _: &str,
            _: &str,
        ) -> std::result::Result<(), egret_storage::StorageError> {
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
        async fn list(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, egret_storage::StorageError> {
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
        async fn get_stream(
            &self,
            _: &str,
        ) -> std::result::Result<
            Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
            egret_storage::StorageError,
        > {
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
    }

    /// Spec §11: the store unreachable while the runner stages is egret's
    /// outage -- a worker `Err`, no outcome -- while an object missing behind
    /// its index row is the author's failed task naming the URI.
    #[tokio::test]
    async fn an_unreachable_store_while_staging_is_a_worker_error_not_a_failed_task() {
        let uri = staged_uri();
        let down = Arc::new(crate::storage::FileStore {
            store: Arc::new(DownStore),
            ..Arc::into_inner(mem_files()).unwrap()
        });
        let spy = Spy::ok(CommandsResult::default()).staging(down.clone());
        let err = stored(spy.clone(), down.clone())
            .run(&in_files_task(&down, &uri, Ok(indexed(&uri, 14))))
            .await
            .expect_err("an unreachable store is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(err.to_string().contains(&uri), "{err}");

        // The object missing behind its row: the author's, naming the URI.
        let files = mem_files();
        let spy = Spy::ok(CommandsResult::default()).staging(files.clone());
        let out = stored(spy.clone(), files.clone())
            .run(&in_files_task(&files, &uri, Ok(indexed(&uri, 14))))
            .await
            .expect("a missing object is the task's failure");
        assert_eq!(out.state, RunState::Failed);
        let message = out.message.expect("a message");
        assert!(
            message.contains(&uri) && message.contains("missing"),
            "{message}"
        );

        // Control: the object there, the same flow succeeds.
        let files = mem_files();
        files
            .store
            .put(
                &files.object_uri(WS, &uri).unwrap(),
                b"staged content".to_vec(),
            )
            .await
            .unwrap();
        let spy = Spy::ok(CommandsResult::default()).staging(files.clone());
        let out = stored(spy, files.clone())
            .run(&in_files_task(&files, &uri, Ok(indexed(&uri, 14))))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
    }

    /// Spec §5: "a URI the snapshot refuses fails the task at render, before
    /// any container exists, naming the URI" -- through the snapshot's own
    /// resolution, the one readability decision (`resolve_readable_many`).
    #[tokio::test]
    async fn an_unreadable_uri_fails_the_task_at_render_and_calls_no_runner() {
        let files = mem_files();
        let uri = staged_uri();
        let cases = [
            crate::storage::Unreadable::NotReadable { uri: uri.clone() },
            crate::storage::Unreadable::Missing { uri: uri.clone() },
            // W9a Task 13: a pruned file is refused at staging too.
            crate::storage::Unreadable::Pruned {
                uri: uri.clone(),
                retention_days: 30,
            },
        ];
        for why in cases {
            let spy = Spy::ok(CommandsResult::default());
            let out = stored(spy.clone(), files.clone())
                .run(&in_files_task(&files, &uri, Err(why.clone())))
                .await
                .expect("the author's failure, not the worker's");
            let calls = spy.seen.lock().unwrap().len();
            assert_eq!(
                calls, 0,
                "the runner was called {calls} time(s) for {why:?}"
            );
            assert_eq!(out.state, RunState::Failed, "{why:?}");
            let message = out.message.expect("a message");
            assert!(message.contains(&uri), "{message}");
            assert!(
                message.contains("`inputFiles` \"d/nested.txt\""),
                "{message}"
            );
        }
        // A URI assembled at render is not in the snapshot: refused too.
        let spy = Spy::ok(CommandsResult::default());
        let mut task = in_files_task(&files, &uri, Ok(indexed(&uri, 14)));
        task.spec = serde_json::json!({
            "containerImage": "alpine:3.20",
            "inputFiles": {"x.txt": "{{ outputs.w.uri ~ '.other' }}"},
            "commands": ["cat x.txt"],
        });
        let out = stored(spy.clone(), files.clone())
            .run(&task)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(out.message.unwrap().contains("was not looked up"));
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// Every renderable property renders, `{{ workingDir }}` included, and
    /// inline `inputFiles` contents render (`input_files_inline`).
    #[tokio::test]
    async fn the_properties_render_and_a_broken_one_names_itself() {
        let spy = Spy::ok(CommandsResult::default());
        let cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "inputs": {"tag": "3.20", "who": "world"},
            "flow": {"id": "q2_input_files_inline", "namespace": "w5a.q2"},
        }))
        .expect("an object");
        let task = TaskInput {
            render: cx,
            ..input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({
                    "containerImage": "alpine:{{ inputs.tag }}",
                    "commands": ["echo {{ inputs.who }} > {{ workingDir }}/x"],
                    "env": {"WHO": "{{ inputs.who }}"},
                    "inputFiles": {"in.txt": "hello-input", "sub/nested.txt": "nested {{ flow.id }}"},
                    "outputFiles": ["{{ inputs.who }}.txt"],
                }),
            )
        };
        stored(spy.clone(), mem_files())
            .run(&task)
            .await
            .expect("no worker error");
        let cmds = spy.only();
        assert_eq!(cmds.container_image, "alpine:3.20");
        assert_eq!(cmds.commands, vec!["echo world > /egret/working-dir/x"]);
        assert_eq!(cmds.env.get("WHO").map(String::as_str), Some("world"));
        assert_eq!(cmds.output_files, vec!["world.txt"]);
        assert_eq!(
            cmds.input_files,
            BTreeMap::from([
                ("in.txt".to_string(), "hello-input".to_string()),
                (
                    "sub/nested.txt".to_string(),
                    "nested q2_input_files_inline".to_string()
                ),
            ])
        );

        let spy = Spy::ok(CommandsResult::default());
        let out = open(spy.clone())
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({"commands": ["echo {{ nope.missing }}"]}),
            ))
            .await
            .expect("an unrenderable template is a task failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .unwrap_or_default()
                .contains("`commands`"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// An escaping `inputFiles` name fails the task before any runner.
    #[tokio::test]
    async fn an_escaping_input_file_name_fails_the_task() {
        let spy = Spy::ok(CommandsResult::default());
        let out = open(spy.clone())
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({"commands": ["true"], "inputFiles": {"../x": "y"}}),
            ))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    // ---- outcomes -------------------------------------------------------------

    /// **A non-zero exit fails the task, keeps `exitCode` and `vars`, and logs
    /// Kestra's `Command failed with exit code <n>` at ERROR** (findings
    /// `failed_run_vars`, `fail_fast`).
    #[tokio::test]
    async fn a_non_zero_exit_fails_the_task_keeps_its_outputs_and_logs_the_measured_line() {
        let case = &w5a()["egret_expected"]["failed_run_vars_case"];
        let exit = i32::try_from(case["exitCode"].as_i64().expect("exitCode")).unwrap();
        let (task, rec) = recorded(scripts::ALIAS_CONTAINER, alias_spec());
        let spy = Spy::ok(CommandsResult {
            exit_code: exit,
            vars: case["vars"].as_object().expect("vars").clone(),
            ..CommandsResult::default()
        })
        .streaming(vec![]);
        let out = open(spy).run(&task).await.expect("no worker error");
        assert_eq!(out.state.wire(), case["state"].as_str().expect("state"));
        assert_eq!(
            out.message.as_deref(),
            Some(format!("egret.container: Command failed with exit code {exit}").as_str())
        );
        let outputs = out.outputs.expect("a failed command still carries outputs");
        assert_eq!(outputs["exitCode"], serde_json::json!(exit));
        assert_eq!(outputs["vars"], case["vars"]);
        assert_eq!(
            rec.texts(),
            vec![(
                LogLevel::Error,
                format!("Command failed with exit code {exit}")
            )]
        );
    }

    /// **`warningOnStdErr` has no effect on the state** -- Task 1's
    /// measurement, every case of it: stderr + exit 0 is SUCCESS, the stderr
    /// line at ERROR. (The plan said it sets WARNING; that was never measured.)
    #[tokio::test]
    async fn warning_on_std_err_has_no_effect_on_the_state() {
        let table = &w5a()["egret_expected"]["warning_on_stderr"];
        assert_eq!(table["effect_on_state"], "none");
        let cases = table["cases"].as_array().expect("cases");
        assert_eq!(cases.len(), 4);
        for case in cases {
            let type_id = case["type"].as_str().unwrap_or(scripts::SHELL_COMMANDS);
            let mut spec = match type_id {
                scripts::PYTHON_SCRIPT => serde_json::json!({"script": "x"}),
                _ => serde_json::json!({"commands": ["echo err >&2"]}),
            };
            if !case["warningOnStdErr"].is_null() {
                spec["warningOnStdErr"] = case["warningOnStdErr"].clone();
            }
            let (task, rec) = recorded(type_id, spec);
            let spy = Spy::ok(CommandsResult {
                exit_code: 0,
                stderr_line_count: 1,
                ..CommandsResult::default()
            })
            .streaming(vec![(true, "err".into())]);
            let h = script(type_id, spy, true, &["*", "*/*"]);
            let out = h.run(&task).await.expect("no worker error");
            assert_eq!(
                out.state.wire(),
                case["state"].as_str().expect("state"),
                "{case}"
            );
            assert_eq!(rec.texts(), vec![(LogLevel::Error, "err".to_string())]);
        }
    }

    /// A daemon that is not there is a WORKER failure; anything else fails the
    /// task.
    #[tokio::test]
    async fn an_unreachable_daemon_aborts_the_tick_and_a_bad_image_fails_the_task() {
        let spy = Spy::new(Err(Error::Internal(anyhow::anyhow!(
            "Cannot connect to the Docker daemon"
        ))));
        let err = open(spy)
            .run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect_err("egret's own outage must not fail the author's flow");
        assert!(err.to_string().contains("not the author's flow"), "{err}");

        let spy = Spy::new(Err(Error::InvalidInput(
            "docker create: Unable to find image 'nope:1' locally".into(),
        )));
        let out = open(spy)
            .run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(out
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("Unable to find image"));
    }

    /// A feature state that cannot be read is egret's outage: `Err`, and no
    /// runner call -- never a run on a guess.
    #[tokio::test]
    async fn an_unreadable_feature_state_is_a_worker_failure_and_calls_no_runner() {
        #[derive(Debug)]
        struct Down;
        #[async_trait::async_trait]
        impl FeatureState for Down {
            async fn containers_enabled(&self, _: uuid::Uuid) -> Result<bool> {
                Err(Error::Internal(anyhow::anyhow!("database down")))
            }
            async fn kestra_plugins_enabled(&self, _: uuid::Uuid) -> Result<bool> {
                Err(Error::Internal(anyhow::anyhow!("database down")))
            }
        }
        let spy = Spy::ok(CommandsResult::default());
        let h = Script::new(
            ScriptKind::ShellCommands,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: spy.clone(),
                plugin_configurations: allow(RunnerKind::Docker, &["*"]),
                features: Arc::new(Down),
                files: None,
                pool: None,
            }),
        );
        h.run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect_err("an unreadable gate is not an open gate");
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// The `python.Script` authoring hint still connects a `NameError` on one
    /// of Kestra's predefined names to its cause (W4 review Minor 7).
    #[tokio::test]
    async fn a_kestra_predefined_name_error_is_connected_to_its_cause() {
        let (task, _rec) = recorded(
            scripts::ALIAS_SCRIPT_PYTHON,
            serde_json::json!({"script": "print(null)"}),
        );
        let spy = Spy::ok(CommandsResult {
            exit_code: 1,
            ..CommandsResult::default()
        })
        .streaming(vec![(true, "NameError: name 'null' is not defined".into())]);
        let out = script(scripts::ALIAS_SCRIPT_PYTHON, spy, true, &["python:*"])
            .run(&task)
            .await
            .expect("no worker error");
        let msg = out.message.expect("a message");
        assert!(
            msg.starts_with("egret.script.python: Command failed with exit code 1"),
            "{msg}"
        );
        assert!(msg.contains("silently prepends"), "{msg}");
    }

    /// `cancel` reaches the configured runner.
    #[tokio::test]
    async fn cancel_asks_the_configured_runner_to_stop_the_attempt() {
        let spy = Spy::ok(CommandsResult::default());
        let task = TaskInput {
            run_id: uuid::Uuid::from_u128(9),
            ..input(scripts::ALIAS_CONTAINER, alias_spec())
        };
        open(spy.clone())
            .cancel(&task)
            .await
            .expect("a container can be stopped");
        assert_eq!(
            spy.cancelled.lock().unwrap().as_slice(),
            ["egret-run-00000000000000000000000000000009-1"]
        );
        assert_eq!(
            spy.cancel_graces.lock().unwrap().as_slice(),
            [std::time::Duration::ZERO],
            "a script task cancels with grace 0: W5a's cancel, unchanged (W5b)"
        );
    }

    /// **Registration**: the four FQCNs and the two aliases, every one a
    /// spelling publish accepts, each dispatching to a script handler.
    #[test]
    fn register_serves_the_four_types_and_the_two_aliases() {
        let mut reg = Handlers::new();
        register(
            &mut reg,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: Spy::ok(CommandsResult::default()),
                plugin_configurations: PluginConfigurations::default(),
                features: Arc::new(FixedFeatureState(false)),
                files: None,
                pool: None,
            }),
        );
        assert_eq!(
            reg.type_ids(),
            vec![
                "egret.container",
                "egret.script.python",
                "io.kestra.plugin.scripts.python.Commands",
                "io.kestra.plugin.scripts.python.Script",
                "io.kestra.plugin.scripts.shell.Commands",
                "io.kestra.plugin.scripts.shell.Script",
            ]
        );
        for id in reg.type_ids() {
            let row = crate::spec::registry::known_type(id)
                .unwrap_or_else(|| panic!("{id} is not publishable"));
            assert!(
                row.references.is_empty(),
                "{id}: an image and a script are not egret objects, so no RefField"
            );
        }
    }

    // ---- the final review's fixes -------------------------------------------

    /// **A refused run whose container already exists is adopted and
    /// finishes** (spec §4, "runs already in a pod finish"): the gate refuses
    /// (the feature is off now), the runner is asked to ADOPT -- never to run
    /// -- and the adopted result is the outcome. The control: with nothing to
    /// adopt, the same refusal fails the run, and still nothing was run.
    #[tokio::test]
    async fn a_refused_run_adopts_its_existing_container_and_never_creates_one() {
        let spy = Spy::ok(CommandsResult::default());
        *spy.adoptable.lock().unwrap() = Some(CommandsResult {
            exit_code: 0,
            ..CommandsResult::default()
        });
        let h = script(scripts::ALIAS_CONTAINER, spy.clone(), false, &["*"]);
        let out = h
            .run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 1);
        assert!(
            spy.seen.lock().unwrap().is_empty(),
            "`run` (create) never called"
        );

        let spy = Spy::ok(CommandsResult::default());
        let h = script(scripts::ALIAS_CONTAINER, spy.clone(), false, &["*"]);
        let out = h
            .run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .unwrap_or_default()
                .contains("feature is disabled"),
            "{:?}",
            out.message
        );
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 1);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// **A run the gate admits never asks to adopt**: `run` is the one call.
    #[tokio::test]
    async fn an_admitted_run_is_run_not_adopted() {
        let spy = Spy::ok(CommandsResult::default());
        let h = open(spy.clone());
        h.run(&input(scripts::ALIAS_CONTAINER, alias_spec()))
            .await
            .expect("no worker error");
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
        assert_eq!(spy.seen.lock().unwrap().len(), 1);
    }

    /// **`release` hands the runner the task's own `delete`** (final review
    /// I3) -- the default `true`, or the author's `false` -- and a task naming
    /// the runner this deployment does not have releases nothing, because no
    /// runner ever held anything for it.
    #[tokio::test]
    async fn release_passes_the_tasks_delete_to_the_configured_runner_only() {
        let spy = Spy::ok(CommandsResult::default());
        let h = open(spy.clone());
        let t = input(scripts::ALIAS_CONTAINER, alias_spec());
        h.release(&t).await.expect("released");
        let mut kept = alias_spec();
        kept["taskRunner"] = serde_json::json!({"type": scripts::DOCKER_RUNNER, "delete": false});
        h.release(&input(scripts::ALIAS_CONTAINER, kept))
            .await
            .expect("released");
        let mut other = alias_spec();
        other["taskRunner"] = serde_json::json!({"type": scripts::KUBERNETES_RUNNER});
        h.release(&input(scripts::ALIAS_CONTAINER, other))
            .await
            .expect("nothing to release");
        let name = crate::worker::commands::container_name(&t);
        assert_eq!(
            spy.released.lock().unwrap().as_slice(),
            [(name.clone(), true), (name, false)]
        );
    }

    // ---- W9a Task 6: outputs into internal storage ----------------------

    /// A task run's prefix, as the claim builds it.
    fn prefix_of(task: &TaskInput) -> String {
        crate::storage::uri::task_prefix(
            "w9a.t6",
            "f",
            task.execution_id,
            &task.task_id,
            task.run_id,
        )
        .unwrap()
    }

    /// A task whose render scope carries its own task prefix, as
    /// `claim::input_for` builds it.
    fn scoped(files: &Arc<crate::storage::FileStore>, spec: serde_json::Value) -> TaskInput {
        let mut task = input(scripts::SHELL_COMMANDS, spec);
        task.workspace_id = WS;
        let snapshot = crate::storage::scope::Snapshot::from_parts(
            BTreeMap::new(),
            files.clone(),
            WS,
            Some(prefix_of(&task)),
        );
        let mut cx = crate::runners::RenderContext::new().with_files(snapshot.into_scope());
        cx.insert(
            "flow",
            serde_json::json!({"id": "f", "namespace": "w9a.t6"}),
        );
        task.render = cx;
        task
    }

    /// The runner is told where the run's files go -- its own prefix, Kestra's
    /// naming -- and `outputDirectory` gives the command `{{ outputDir }}`.
    #[tokio::test]
    async fn the_runner_is_given_the_runs_prefix_and_the_output_directory() {
        let files = mem_files();
        let spy = Spy::ok(CommandsResult::default());
        let task = scoped(
            &files,
            serde_json::json!({
                "containerImage": "alpine:3.20",
                "outputFiles": ["*.csv"],
                "outputDirectory": true,
                "commands": ["echo x > {{ outputDir }}/od.txt"],
            }),
        );
        let out = stored(spy.clone(), files).run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Success, "{out:?}");
        let cmds = spy.only();
        assert_eq!(
            cmds.collect_to,
            Some(CollectTo {
                workspace: WS,
                task_prefix: prefix_of(&task),
                naming: Naming::KestraOutputName,
            })
        );
        assert!(cmds.output_directory);
        assert_eq!(
            cmds.commands,
            vec![format!("echo x > {OUTPUT_DIR}/od.txt")],
            "Kestra's render variable"
        );
    }

    /// No internal storage: a task declaring an output fails naming
    /// `[storage]`, before the gate and before any runner -- W5a's
    /// contents-in-outputs path no longer exists. A task declaring none runs.
    #[tokio::test]
    async fn an_output_without_storage_fails_naming_the_storage_section_before_the_runner() {
        for spec in [
            serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"], "outputFiles": ["a.txt"]}),
            serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"], "outputDirectory": true}),
        ] {
            let spy = Spy::ok(CommandsResult::default());
            let out = open(spy.clone())
                .run(&input(scripts::SHELL_COMMANDS, spec.clone()))
                .await
                .expect("the task's failure, not a worker's");
            assert_eq!(out.state, RunState::Failed);
            assert!(
                out.message.as_deref().unwrap().contains("[storage]"),
                "{spec}: {out:?}"
            );
            assert!(spy.seen.lock().unwrap().is_empty(), "no runner was called");
        }
        let spy = Spy::ok(CommandsResult::default());
        let out = open(spy.clone())
            .run(&input(
                scripts::SHELL_COMMANDS,
                serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"]}),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(spy.only().collect_to, None);
    }

    /// A namespace no URI can carry (Task 3's builder `Err`, which the claim
    /// drops when it builds the scope): the AUTHOR's failure, naming why,
    /// before any runner -- never `Internal`, which would fence the commit
    /// and re-claim the run for ever.
    #[tokio::test]
    async fn a_prefix_no_uri_can_carry_is_the_authors_failure_naming_it() {
        let files = mem_files();
        let spy = Spy::ok(CommandsResult::default());
        let mut task = input(
            scripts::SHELL_COMMANDS,
            serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"], "outputFiles": ["a.txt"]}),
        );
        let mut cx = crate::runners::RenderContext::new();
        cx.insert("flow", serde_json::json!({"id": "f", "namespace": "a..b"}));
        task.render = cx;
        let out = stored(spy.clone(), files)
            .run(&task)
            .await
            .expect("an author's namespace is never a worker failure");
        assert_eq!(out.state, RunState::Failed);
        let msg = out.message.unwrap();
        assert!(
            msg.contains("a..b") && msg.contains("cannot be read back"),
            "{msg}"
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// The collected files ride with the outcome, and `outputFiles` holds
    /// their URIs; a marker naming a URI under the prefix that was not
    /// collected fails the task (spec §5).
    #[tokio::test]
    async fn collected_files_ride_with_the_outcome_and_a_ghost_uri_fails_it() {
        let files = mem_files();
        let task = scoped(
            &files,
            serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"], "outputFiles": ["a.txt"]}),
        );
        let prefix = prefix_of(&task);
        let real = crate::storage::StoredFile {
            uri: format!("{prefix}abc-a.txt"),
            size: 3,
        };
        let result = CommandsResult {
            output_files: BTreeMap::from([("a.txt".to_string(), real.clone())]),
            ..CommandsResult::default()
        };
        let out = stored(Spy::ok(result.clone()), files.clone())
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{out:?}");
        assert_eq!(out.files, vec![real.clone()]);
        assert_eq!(
            out.outputs.as_ref().unwrap()["outputFiles"],
            serde_json::json!({"a.txt": real.uri})
        );
        let mut ghost = result;
        ghost
            .vars
            .insert("f".into(), serde_json::json!(format!("{prefix}ghost.txt")));
        let out = stored(Spy::ok(ghost), files).run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(out.message.unwrap().contains("ghost.txt"));
        assert_eq!(out.files, vec![real], "the real file is still indexed");
    }

    /// **Fix round 1, I2 (spec §5): the check runs whether or not the task
    /// declared an output.** A task with no `outputFiles` and no
    /// `outputDirectory` that prints a `::{"outputs": …}::` marker naming a
    /// URI under its OWN run's prefix fails: no output may point at a file
    /// that does not exist. A URI under another prefix is not this check's.
    #[tokio::test]
    async fn a_ghost_uri_under_the_runs_prefix_fails_a_task_that_declares_no_output() {
        let files = mem_files();
        let task = scoped(
            &files,
            serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"]}),
        );
        let prefix = prefix_of(&task);
        let ghost = |uri: String| CommandsResult {
            vars: serde_json::Map::from_iter([("f".to_string(), serde_json::json!(uri))]),
            ..CommandsResult::default()
        };
        let out = stored(Spy::ok(ghost(format!("{prefix}ghost.txt"))), files.clone())
            .run(&task)
            .await
            .expect("the author's failure, not a worker's");
        assert_eq!(out.state, RunState::Failed, "{out:?}");
        assert!(out.message.unwrap().contains("ghost.txt"));
        // The control: a URI of some other run is not this check's business.
        let other = prefix.replace(
            &task.run_id.to_string(),
            &uuid::Uuid::from_u128(9).to_string(),
        );
        assert_ne!(other, prefix);
        let out = stored(Spy::ok(ghost(format!("{other}ghost.txt"))), files)
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{out:?}");
    }

    /// **Fix round 1, I3 (spec §11): the store unreachable while the runner
    /// collects is egret's outage, at the handler too.** A runner's collect
    /// arm answers `Error::Internal` (`commands::tests::an_unreachable_store_while_collecting_*`
    /// pin that arm for Docker); the handler must pass it on as a worker
    /// `Err` -- no outcome, no attempt spent -- never fold it into a FAILED
    /// task, even for a task that declared outputs and a storage prefix.
    #[tokio::test]
    async fn an_unreachable_store_while_collecting_is_a_worker_error_not_a_failed_task() {
        let files = mem_files();
        let task = scoped(
            &files,
            serde_json::json!({"containerImage": "alpine:3.20", "commands": ["x"], "outputFiles": ["a.txt"]}),
        );
        let spy = Spy::new(Err(Error::Internal(anyhow::anyhow!(
            "{}abc-a.txt could not be stored: internal storage is unavailable: connection refused",
            prefix_of(&task)
        ))));
        let err = stored(spy, files)
            .run(&task)
            .await
            .expect_err("egret's outage, never the author's failed task");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(err.to_string().contains("abc-a.txt"), "{err}");
    }

    // ---- W9b Task 5: namespace files -----------------------------------------

    use crate::storage::nsfiles::stage::testing::{self as ns, contents, write, OWN};

    /// A script handler over `spy`, staging namespace files from `files` on `pool`.
    fn with_namespace_files(
        spy: Arc<Spy>,
        files: Arc<crate::storage::FileStore>,
        pool: Option<sqlx::PgPool>,
    ) -> Script {
        with_namespace_files_of(ScriptKind::ShellCommands, spy, files, pool)
    }

    fn with_namespace_files_of(
        kind: ScriptKind,
        spy: Arc<Spy>,
        files: Arc<crate::storage::FileStore>,
        pool: Option<sqlx::PgPool>,
    ) -> Script {
        Script::new(
            kind,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: spy,
                plugin_configurations: allow(RunnerKind::Docker, &["*"]),
                features: Arc::new(FixedFeatureState(true)),
                files: Some(files),
                pool,
            }),
        )
    }

    /// A task of flow namespace [`OWN`] in workspace `ws` whose render scope
    /// reaches namespace files (`fileURI`) on `pool` and resolves the
    /// execution URI [`staged_uri`] (14 bytes).
    fn ns_task(
        files: &Arc<crate::storage::FileStore>,
        pool: &sqlx::PgPool,
        ws: uuid::Uuid,
        spec: serde_json::Value,
    ) -> (TaskInput, Arc<crate::worker::handlers::testing::Recorder>) {
        ns_task_of(scripts::SHELL_COMMANDS, files, pool, ws, spec)
    }

    fn ns_task_of(
        type_id: &str,
        files: &Arc<crate::storage::FileStore>,
        pool: &sqlx::PgPool,
        ws: uuid::Uuid,
        spec: serde_json::Value,
    ) -> (TaskInput, Arc<crate::worker::handlers::testing::Recorder>) {
        let uri = staged_uri();
        let snapshot = crate::storage::scope::Snapshot::from_parts(
            BTreeMap::from([(uri.clone(), Ok(indexed(&uri, 14)))]),
            files.clone(),
            ws,
            None,
        )
        .with_namespace_files(
            Some(OWN.to_string()),
            Some(Arc::new((*pool.connect_options()).clone())),
        );
        let mut cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "outputs": {"w": {"uri": uri}},
        }))
        .expect("an object")
        .with_files(snapshot.into_scope());
        cx.insert("flow", serde_json::json!({"id": "stage", "namespace": OWN}));
        let (mut task, rec) = recorded(type_id, spec);
        task.workspace_id = ws;
        task.render = cx;
        (task, rec)
    }

    fn shell(extra: serde_json::Value) -> serde_json::Value {
        let mut spec = serde_json::json!({"containerImage": "alpine:3.20", "commands": ["ls"]});
        spec.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        spec
    }

    /// Every staged input's bytes, by its name in the working directory.
    async fn staged_contents(
        files: &crate::storage::FileStore,
        cmds: &TaskCommands,
    ) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = cmds
            .input_files
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (k, v) in &cmds.stored_inputs {
            out.insert(k.clone(), contents(files, &v.object_uri).await);
        }
        out
    }

    /// Spec §7, the user's ruling, w9b `staging.input_files_over_namespace_file`:
    /// under EVERY `ifExists`, the `inputFiles` entry `x.txt` overwrites the
    /// namespace file of that name, and FAIL does not fail for it. All four
    /// runs are made before anything is asserted, so a mutant shows which
    /// modes it breaks. A WARN collision between namespaces is logged.
    #[tokio::test(flavor = "multi_thread")]
    async fn namespace_files_are_staged_as_stored_inputs_and_input_files_overwrite_them_under_every_if_exists(
    ) {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let doc = crate::storage::nsfiles::stage::tests::fixture();
        let fixture = &doc["egret_expected"]["staging"]["input_files_over_namespace_file"];
        let mut broken = Vec::new();
        for mode in ["OVERWRITE", "FAIL", "WARN", "IGNORE"] {
            let spy = Spy::ok(CommandsResult::default()).staging(files.clone());
            let (task, _) = ns_task(
                &files,
                &pool,
                ws,
                shell(serde_json::json!({
                    "namespaceFiles": {"namespaces": ["w9b.s1"], "ifExists": mode},
                    "inputFiles": {"x.txt": "from-input"},
                })),
            );
            let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
                .run(&task)
                .await
                .expect("no worker error");
            if out.state != RunState::Success {
                broken.push(format!("{mode}: {:?}", out.message));
                continue;
            }
            let cmds = spy.only();
            let got = staged_contents(&files, &cmds).await;
            let want: BTreeMap<String, String> = fixture[mode]["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| {
                    (
                        k.trim_start_matches("./").to_string(),
                        v.as_str().unwrap().to_string(),
                    )
                })
                .collect();
            if got != want || cmds.stored_inputs.contains_key("x.txt") {
                broken.push(format!(
                    "{mode}: staged {got:?}, stored {:?}",
                    cmds.stored_inputs.keys()
                ));
            }
        }
        assert!(
            broken.is_empty(),
            "inputFiles must win under every ifExists: {broken:#?}"
        );

        // WARN between namespaces: the first kept, one WARN line naming the path.
        let spy = Spy::ok(CommandsResult::default());
        let (task, rec) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({
                "namespaceFiles": {"namespaces": ["w9b.s1", "w9b.s2"], "ifExists": "WARN"},
            })),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(
            staged_contents(&files, &spy.only()).await["x.txt"],
            "from-s1"
        );
        let warned: Vec<String> = rec
            .texts()
            .into_iter()
            .filter(|(l, _)| *l == LogLevel::Warn)
            .map(|(_, m)| m)
            .collect();
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains("'x.txt'"), "{warned:?}");
    }

    /// Spec §11: the database unreachable while staging is egret's outage --
    /// a worker `Err`, no runner call -- and so is the store unreachable
    /// while the runner streams a staged namespace file.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_store_outage_while_staging_is_a_worker_error() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let spec = shell(serde_json::json!({"namespaceFiles": {"namespaces": ["w9b.s1"]}}));

        // The store: listed from the database, streamed from a store that is down.
        let down = Arc::new(crate::storage::FileStore {
            store: Arc::new(DownStore),
            ..(*files).clone()
        });
        let spy = Spy::ok(CommandsResult::default()).staging(down.clone());
        let (task, _) = ns_task(&down, &pool, ws, spec.clone());
        let err = with_namespace_files(spy, down, Some(pool.clone()))
            .run(&task)
            .await
            .expect_err("an unreachable store is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");

        // The database: a closed pool.
        let closed = ns::pool().await.unwrap();
        closed.close().await;
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = ns_task(&files, &pool, ws, spec.clone());
        let err = with_namespace_files(spy.clone(), files.clone(), Some(closed))
            .run(&task)
            .await
            .expect_err("an unreachable database is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(err.to_string().contains("staging namespace files"), "{err}");
        assert!(spy.seen.lock().unwrap().is_empty(), "no runner call");

        // The control: the same task, both up, runs.
        let spy = Spy::ok(CommandsResult::default()).staging(files.clone());
        let out = with_namespace_files(spy, files, Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
    }

    /// `enabled: false` stages nothing and needs no database; the runner runs.
    /// With no database, an enabled block fails naming `[storage]`.
    #[tokio::test(flavor = "multi_thread")]
    async fn enabled_false_stages_nothing_and_calls_the_runner() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"namespaceFiles": {"enabled": "{{ 1 == 2 }}"}})),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), None)
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert!(spy.only().stored_inputs.is_empty());

        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"namespaceFiles": {"enabled": true}})),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), None)
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(out.message.unwrap().contains("[storage]"));
        assert!(spy.seen.lock().unwrap().is_empty());

        // The control: the block enabled, with the database: own.txt and x.txt.
        let spy = Spy::ok(CommandsResult::default());
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(
            staged_contents(&files, &spy.only()).await,
            BTreeMap::from([
                ("own.txt".to_string(), "own".to_string()),
                ("x.txt".to_string(), "from-stage".to_string()),
            ])
        );
    }

    /// The user's ruling on item 9: `nsfile:///cfg/a.txt` and
    /// `{{ fileURI('cfg/a.txt') }}` (a `kestra:///…/_files/…v2` URI) are
    /// staged under their `inputFiles` names, from the namespace object, with
    /// the row's size; an execution URI beside them keeps W9a's path.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_input_files_nsfile_uri_of_the_own_namespace_is_staged_under_its_input_name() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        write(&pool, &files, ws, OWN, "/cfg/a.txt", "one").await;
        assert_eq!(
            write(&pool, &files, ws, OWN, "/cfg/a.txt", "second").await,
            2
        );
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"inputFiles": {
                "a.txt": "nsfile:///cfg/a.txt",
                "b.txt": "{{ fileURI('cfg/a.txt') }}",
                "exec.txt": "{{ outputs.w.uri }}",
                "plain.txt": "cfg/a.txt",
            }})),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        let grant = crate::storage::nsfiles::access::authorize(
            ws,
            crate::storage::nsfiles::access::Actor::Api,
            OWN,
            crate::storage::nsfiles::access::Access::Read,
        )
        .unwrap();
        let object = files.namespace_object_uri(
            &grant,
            &crate::storage::nsfiles::path::NsPath::parse_file("/cfg/a.txt").unwrap(),
            2,
        );
        assert_eq!(
            cmds.stored_inputs["a.txt"],
            StoredInput {
                uri: "nsfile:///cfg/a.txt".into(),
                object_uri: object.clone(),
                size: 6
            }
        );
        assert_eq!(
            cmds.stored_inputs["b.txt"],
            StoredInput {
                uri: "kestra:///w9b/stage/_files/cfg/a.txt.v2".into(),
                object_uri: object,
                size: 6
            }
        );
        let exec = staged_uri();
        assert_eq!(cmds.stored_inputs["exec.txt"].uri, exec);
        assert_eq!(
            cmds.stored_inputs["exec.txt"].object_uri,
            files.object_uri(ws, &exec).unwrap()
        );
        // A bare path stays contents, as W5a/W9a.
        assert_eq!(cmds.input_files["plain.txt"], "cfg/a.txt");
        assert_eq!(
            contents(&files, &cmds.stored_inputs["a.txt"].object_uri).await,
            "second"
        );
    }

    /// Any namespace of the workspace: `nsfile://w9b.s1/x.txt` and
    /// `kestra:///w9b/s2/_files/x.txt` (w9b `staging.input_files_namespace_uris`).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_input_files_uri_of_another_namespace_of_the_workspace_is_staged() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let doc = crate::storage::nsfiles::stage::tests::fixture();
        let measured = &doc["egret_expected"]["staging"]["input_files_namespace_uris"];
        assert_eq!(measured["values"]["a.txt"], "nsfile://w9b.s1/x.txt");
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"inputFiles": measured["values"].clone()})),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let got = staged_contents(&files, &spy.only()).await;
        let want: BTreeMap<String, String> = measured["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    k.trim_start_matches("./").to_string(),
                    v.as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(got, want);
    }

    /// A namespace with a `_files` segment (or an empty one) named by an
    /// `inputFiles` URI is refused by `authorize`, naming it, before any
    /// runner -- though the flow's own namespace holds a file of that path.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_input_files_uri_of_a_files_segment_namespace_is_refused_by_name_before_the_runner()
    {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        for (uri, named, rule) in [
            ("nsfile://a._files/x.txt", "'a._files'", "'_files'"),
            ("nsfile://w9b..s1/x.txt", "'w9b..s1'", "empty segment"),
        ] {
            let spy = Spy::ok(CommandsResult::default());
            let (task, _) = ns_task(
                &files,
                &pool,
                ws,
                shell(serde_json::json!({"inputFiles": {"in.txt": uri}})),
            );
            let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
                .run(&task)
                .await
                .unwrap();
            assert_eq!(out.state, RunState::Failed, "{uri}");
            let m = out.message.unwrap();
            assert!(
                m.contains("\"in.txt\"") && m.contains(named) && m.contains(rule),
                "{uri}: {m}"
            );
            assert!(spy.seen.lock().unwrap().is_empty(), "{uri}: no runner call");
        }
    }

    /// A missing path, a soft-deleted path read as latest, and another
    /// workspace's identical (namespace, path) all fail the task naming the
    /// path, before the runner, with the same "missing" answer; a
    /// `kestra:///…/_files/p` URI of the soft-deleted path names revision 1
    /// and IS staged (R1).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_input_files_uri_of_a_missing_path_fails_the_task_naming_it_before_the_runner() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let (ws, other) = (ns::workspace(&pool).await, ns::workspace(&pool).await);
        write(&pool, &files, ws, OWN, "/gone.txt", "was here").await;
        ns::delete(&pool, ws, OWN, "/gone.txt").await;
        write(&pool, &files, other, OWN, "/theirs.txt", "not yours").await;
        let run = |uri: &'static str| {
            let (files, pool) = (files.clone(), pool.clone());
            async move {
                let spy = Spy::ok(CommandsResult::default());
                let (task, _) = ns_task(
                    &files,
                    &pool,
                    ws,
                    shell(serde_json::json!({"inputFiles": {"in.txt": uri}})),
                );
                let out = with_namespace_files(spy.clone(), files, Some(pool))
                    .run(&task)
                    .await
                    .unwrap();
                let called = !spy.seen.lock().unwrap().is_empty();
                (out, called, spy)
            }
        };
        for (uri, path) in [
            ("nsfile:///nope.txt", "'/nope.txt'"),
            ("nsfile:///gone.txt", "'/gone.txt'"),
            ("nsfile:///theirs.txt", "'/theirs.txt'"),
            ("kestra:///w9b/stage/_files/nope.txt", "'/nope.txt'"),
        ] {
            let (out, called, _) = run(uri).await;
            assert_eq!(out.state, RunState::Failed, "{uri}");
            let m = out.message.unwrap();
            assert!(
                m.contains("\"in.txt\"") && m.contains(path) && m.contains("does not exist"),
                "{uri}: {m}"
            );
            assert!(!called, "{uri}: the runner was called");
        }
        let (out, called, spy) = run("kestra:///w9b/stage/_files/gone.txt").await;
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert!(called);
        assert_eq!(
            staged_contents(&files, &spy.only()).await["in.txt"],
            "was here"
        );
    }

    /// The merge rule, whatever the value's kind: an `inputFiles` namespace
    /// URI overwrites a `namespaceFiles` entry of the same name.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_input_files_namespace_uri_overwrites_a_namespace_files_entry_of_the_same_name() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        for mode in ["OVERWRITE", "FAIL"] {
            let spy = Spy::ok(CommandsResult::default());
            let (task, _) = ns_task(
                &files,
                &pool,
                ws,
                shell(serde_json::json!({
                    "namespaceFiles": {"namespaces": ["w9b.s1"], "ifExists": mode},
                    "inputFiles": {"x.txt": "nsfile://w9b.s2/x.txt"},
                })),
            );
            let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
                .run(&task)
                .await
                .unwrap();
            assert_eq!(out.state, RunState::Success, "{mode}: {:?}", out.message);
            let cmds = spy.only();
            assert_eq!(cmds.stored_inputs["x.txt"].uri, "nsfile://w9b.s2/x.txt");
            assert_eq!(
                staged_contents(&files, &cmds).await,
                BTreeMap::from([
                    ("only1.txt".to_string(), "only1".to_string()),
                    ("x.txt".to_string(), "from-s2".to_string()),
                ]),
                "{mode}"
            );
        }
    }

    // ---- W9b Task 5 fix round 1 -----------------------------------------------

    /// What a runner unpacks: the archive the shipping `write_tar` writes for
    /// `cmds`, read back by the shipping `TarReader` -- each file's bytes by
    /// its name in the working directory.
    async fn unpacked(
        files: &crate::storage::FileStore,
        cmds: &TaskCommands,
    ) -> BTreeMap<String, String> {
        use crate::storage::transfer::{write_tar, EntryKind, TarReader};
        use tokio::io::AsyncReadExt;
        let sources = cmds.tar_sources().expect("the sources");
        let mut archive: Vec<u8> = Vec::new();
        write_tar(&mut archive, &sources, Some(files))
            .await
            .expect("the archive is written");
        let mut bytes: &[u8] = &archive;
        let mut reader = TarReader::new(&mut bytes);
        let mut out = BTreeMap::new();
        while let Some(entry) = reader.next_entry().await.expect("an entry") {
            if entry.kind != EntryKind::File {
                continue;
            }
            let mut body = Vec::new();
            reader
                .body()
                .read_to_end(&mut body)
                .await
                .expect("its contents");
            out.insert(entry.name, String::from_utf8(body).expect("utf-8"));
        }
        out
    }

    /// I2 (the user's ruling of 2026-10-06): a namespace file whose name holds
    /// glob characters (`[`, `*`; legal for `NsPath`) stages as in Kestra --
    /// with no include and with an include that matches it -- and its bytes
    /// arrive under that literal name, through the shipping archive.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_namespace_file_whose_name_holds_glob_characters_stages_as_in_kestra() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        write(&pool, &files, ws, OWN, "/reports/q[1].csv", "q,1\n").await;
        write(&pool, &files, ws, OWN, "/a*b.txt", "star").await;
        for block in [
            serde_json::json!({}),
            serde_json::json!({"include": ["reports/*.csv"]}),
            // A `\` escapes only under `glob:`: unprefixed, Kestra turns it into
            // `/` (`PathMatcherPredicate.java:96-103`).
            serde_json::json!({"include": [r"glob:/reports/q\[1\].csv"]}),
        ] {
            let spy = Spy::ok(CommandsResult::default()).staging(files.clone());
            let (task, _) = ns_task(
                &files,
                &pool,
                ws,
                shell(serde_json::json!({ "namespaceFiles": block })),
            );
            let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
                .run(&task)
                .await
                .expect("no worker error");
            assert_eq!(out.state, RunState::Success, "{block}: {:?}", out.message);
            let got = unpacked(&files, &spy.only()).await;
            assert_eq!(got["reports/q[1].csv"], "q,1\n", "{block}: {got:?}");
            if block.as_object().unwrap().is_empty() {
                assert_eq!(got["a*b.txt"], "star", "{got:?}");
            } else {
                assert!(!got.contains_key("a*b.txt"), "{block}: {got:?}");
            }
        }
        // The shared rule is unchanged for every other caller: an
        // `inputFiles` KEY that looks like a glob is still refused at render.
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"inputFiles": {"q[1].csv": "x"}})),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(out.message.unwrap().contains("looks like a glob"));
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// M1: `max_task_bytes` counts what the runner streams, AFTER the merge:
    /// a namespace file an `inputFiles` name replaced does not count, and an
    /// `inputFiles` namespace-file URI does.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_byte_cap_counts_the_merged_stored_inputs() {
        let Some(pool) = ns::pool().await else { return };
        let caps = |bytes: u64| crate::storage::Caps {
            max_task_bytes: bytes,
            ..crate::storage::Caps::default()
        };
        let files = ns::mem_files(caps(1 << 30));
        let ws = ns::workspace(&pool).await;
        write(&pool, &files, ws, OWN, "/big.txt", "0123456789").await;
        write(&pool, &files, ws, OWN, "/small.txt", "abcd").await;
        let at = |bytes: u64| {
            Arc::new(crate::storage::FileStore {
                caps: caps(bytes),
                ..(*files).clone()
            })
        };
        let run = |files: Arc<crate::storage::FileStore>, spec: serde_json::Value| {
            let pool = pool.clone();
            async move {
                let spy = Spy::ok(CommandsResult::default());
                let (task, _) = ns_task(&files, &pool, ws, shell(spec));
                let out = with_namespace_files(spy.clone(), files, Some(pool))
                    .run(&task)
                    .await
                    .expect("no worker error");
                let called = !spy.seen.lock().unwrap().is_empty();
                (out, called)
            }
        };
        // A replaced namespace file does not count: 10 + 4 listed, but
        // `big.txt` is an inline input, so 4 bytes stream; a cap of 4 passes.
        let replaced = serde_json::json!({
            "namespaceFiles": {}, "inputFiles": {"big.txt": "inline"},
        });
        let (out, called) = run(at(4), replaced.clone()).await;
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert!(called);
        let (out, called) = run(at(3), replaced).await;
        assert_eq!(out.state, RunState::Failed);
        assert!(!called);
        let m = out.message.unwrap();
        assert!(
            m.contains("4 bytes") && m.contains("workflow.storage.max_task_bytes"),
            "{m}"
        );
        // An `inputFiles` namespace URI counts: 4 + 10 under a cap of 13 is
        // refused naming it, and 14 passes.
        let uri = serde_json::json!({
            "namespaceFiles": {"include": ["small.txt"]},
            "inputFiles": {"in.txt": "nsfile:///big.txt"},
        });
        let (out, called) = run(at(13), uri.clone()).await;
        assert_eq!(out.state, RunState::Failed, "{:?}", out.message);
        assert!(!called);
        let m = out.message.unwrap();
        assert!(
            m.contains("14 bytes") && m.contains("more than the 13"),
            "{m}"
        );
        let (out, called) = run(at(14), uri).await;
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert!(called);
    }

    /// M4 (the user's ruling of 2026-10-06): `ifExists`, `include`, `exclude`
    /// and `namespaces` as expressions, rendered by the task's own render
    /// (`render_str`) at run, then type-checked. A bad rendered `ifExists`
    /// fails the task naming the field and the value, before any runner.
    #[tokio::test(flavor = "multi_thread")]
    async fn expressions_in_if_exists_include_exclude_and_namespaces_render_at_run() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let task_with = |block: serde_json::Value, inputs: serde_json::Value| {
            let (mut task, rec) = ns_task(
                &files,
                &pool,
                ws,
                shell(serde_json::json!({ "namespaceFiles": block })),
            );
            task.render.insert("inputs", inputs);
            (task, rec)
        };
        let block = serde_json::json!({
            "namespaces": "{{ inputs.nss }}",
            "ifExists": "{{ inputs.mode }}",
            "include": "{{ inputs.inc }}",
            "exclude": "{{ inputs.exc }}",
        });
        let inputs = serde_json::json!({
            "nss": ["w9b.s1", "w9b.s2"], "mode": "WARN",
            "inc": ["**.txt"], "exc": ["/sub/**"],
        });
        let spy = Spy::ok(CommandsResult::default());
        let (task, rec) = task_with(block.clone(), inputs.clone());
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        // The same block written as literals stages the same files.
        let spy_literal = Spy::ok(CommandsResult::default());
        let (literal, _) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"namespaceFiles": {
                "namespaces": inputs["nss"], "ifExists": "WARN",
                "include": inputs["inc"], "exclude": inputs["exc"],
            }})),
        );
        let lit = with_namespace_files(spy_literal.clone(), files.clone(), Some(pool.clone()))
            .run(&literal)
            .await
            .unwrap();
        assert_eq!(lit.state, RunState::Success, "{:?}", lit.message);
        let (got, want) = (
            staged_contents(&files, &spy.only()).await,
            staged_contents(&files, &spy_literal.only()).await,
        );
        assert_eq!(got, want);
        // WARN came from the expression: s1's x.txt kept, one WARN line.
        assert_eq!(got["x.txt"], "from-s1", "{got:?}");
        assert_eq!(got["only1.txt"], "only1", "{got:?}");
        assert!(!got.contains_key("sub/y.txt"), "excluded: {got:?}");
        let warned = rec
            .texts()
            .into_iter()
            .filter(|(l, _)| *l == LogLevel::Warn)
            .count();
        assert_eq!(warned, 1);

        // A bad rendered ifExists: failed, named, no runner.
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = task_with(
            serde_json::json!({"ifExists": "{{ inputs.mode }}"}),
            serde_json::json!({"mode": "SOMETIMES"}),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool.clone()))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        let m = out.message.unwrap();
        assert!(
            m.contains("namespaceFiles.ifExists") && m.contains("\"SOMETIMES\""),
            "{m}"
        );
        assert!(spy.seen.lock().unwrap().is_empty());
        // A list expression that renders to a string, not a list: named.
        let spy = Spy::ok(CommandsResult::default());
        let (task, _) = task_with(
            serde_json::json!({"namespaces": "{{ inputs.one }}"}),
            serde_json::json!({"one": "w9b.s1"}),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        let m = out.message.unwrap();
        assert!(
            m.contains("namespaceFiles.namespaces") && m.contains("\"w9b.s1\""),
            "{m}"
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// M5: with no `[storage]` configured, a block with no `namespaces` fails
    /// naming `[storage]` (not a missing flow namespace), before any runner.
    #[tokio::test]
    async fn no_storage_names_the_storage_section_for_a_block_without_namespaces() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Script::new(
            ScriptKind::ShellCommands,
            Arc::new(ScriptRuntime {
                runner_kind: RunnerKind::Docker,
                runner: spy.clone(),
                plugin_configurations: allow(RunnerKind::Docker, &["*"]),
                features: Arc::new(FixedFeatureState(true)),
                files: None,
                pool: None,
            }),
        );
        let (task, _) = recorded(
            scripts::SHELL_COMMANDS,
            shell(serde_json::json!({"namespaceFiles": {}})),
        );
        let out = h.run(&task).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        let m = out.message.unwrap();
        assert!(m.contains("[storage]"), "{m}");
        assert!(!m.contains("no flow namespace"), "{m}");
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// M6 (the user's ruling of 2026-10-06, spec §7): for `python.Script`, a
    /// namespace file `/main.py` collides with the task's own script file;
    /// the script wins, and one WARN line names the skipped namespace file.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_tasks_script_wins_over_a_namespace_main_py_and_one_warn_names_it() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        write(
            &pool,
            &files,
            ws,
            "w9b.py",
            "/main.py",
            "print('namespace')",
        )
        .await;
        write(&pool, &files, ws, "w9b.py", "/lib.py", "x = 1").await;
        let spy = Spy::ok(CommandsResult::default()).staging(files.clone());
        let (task, rec) = ns_task_of(
            scripts::PYTHON_SCRIPT,
            &files,
            &pool,
            ws,
            serde_json::json!({
                "containerImage": "python:3.13-slim",
                "script": "print('task')",
                "namespaceFiles": {"namespaces": ["w9b.py"]},
            }),
        );
        let out = with_namespace_files_of(
            ScriptKind::PythonScript,
            spy.clone(),
            files.clone(),
            Some(pool.clone()),
        )
        .run(&task)
        .await
        .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        assert_eq!(
            unpacked(&files, &cmds).await,
            BTreeMap::from([
                ("lib.py".to_string(), "x = 1".to_string()),
                ("main.py".to_string(), "print('task')".to_string()),
            ])
        );
        let warned: Vec<String> = rec
            .texts()
            .into_iter()
            .filter(|(l, _)| *l == LogLevel::Warn)
            .map(|(_, m)| m)
            .collect();
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(
            warned[0].contains("'main.py'")
                && warned[0].contains("'w9b.py'")
                && warned[0].contains("kestra:///w9b/py/_files/main.py"),
            "{warned:?}"
        );
        // The control: shell.Commands has no script file, so the namespace
        // `main.py` is staged and nothing is logged.
        let spy = Spy::ok(CommandsResult::default());
        let (task, rec) = ns_task(
            &files,
            &pool,
            ws,
            shell(serde_json::json!({"namespaceFiles": {"namespaces": ["w9b.py"]}})),
        );
        let out = with_namespace_files(spy.clone(), files.clone(), Some(pool))
            .run(&task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(
            staged_contents(&files, &spy.only()).await["main.py"],
            "print('namespace')"
        );
        assert!(rec.texts().iter().all(|(l, _)| *l != LogLevel::Warn));
    }
}
