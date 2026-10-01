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
//!   ignored -- [`TaskCommands::output_files`]), but its value is the file's
//!   CONTENTS under W4's 1 MiB per-file and total caps, not an
//!   internal-storage URI (W9 owns storage); spec §2's table says so.

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_config::PluginConfigurations;
use egret_core::{Error, Result};

use super::super::commands::{
    relative_problem, LineObserver, NoObserver, StoredInput, TaskCommands, TaskRunner,
    LOG_DEBUG_MARKER, LOG_INFO_MARKER, WORKING_DIR,
};
use super::super::{Handlers, Outcome, TaskHandler, TaskInput};
use super::container::{
    command_failed, mapped, outcome_for, render_all, render_values, EXIT_ON_ERROR,
};
use super::python::{NameHintSink, ENV_DEFAULTS, SCRIPT_FILE};
use crate::logs::LogLevel;
use crate::runners::render_str;
use crate::scripts::{self, gate, RunnerKind, RunnerSpec, ScriptKind, ScriptTask};

/// The render variable Kestra binds to the working directory (`{{ workingDir }}`).
pub const WORKING_DIR_VAR: &str = "workingDir";

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
    ) -> std::result::Result<TaskCommands, Outcome> {
        let mut cx = task.render.clone();
        cx.insert(WORKING_DIR_VAR, serde_json::json!(WORKING_DIR));

        let one = |what: &str, s: &str| {
            render_str(s, &cx).map_err(|e| {
                Outcome::failed_with(format!("{type_id}: `{what}` did not render: {e}"))
            })
        };
        let image = one("containerImage", p.image())?;
        let interpreter = match &p.interpreter {
            Some(i) => render_all(type_id, "interpreter", i, &cx)?,
            None => Vec::new(),
        };
        let before = render_all(type_id, "beforeCommands", &p.before_commands, &cx)?;
        let author_env = render_values(type_id, "env", &p.env, &cx)?;
        let rendered_inputs = render_values(type_id, "inputFiles", &p.input_files, &cx)?;
        let (mut input_files, stored_inputs) = self.stage_inputs(type_id, task, rendered_inputs)?;
        let output_files = render_all(type_id, "outputFiles", &p.output_files, &cx)?;
        let script = p.script.as_deref().unwrap_or_default();

        let commands = match p.kind {
            ScriptKind::ShellCommands | ScriptKind::PythonCommands => {
                render_all(type_id, "commands", &p.commands, &cx)?
            }
            ScriptKind::ShellScript => vec![one("script", script)?],
            ScriptKind::PythonScript => {
                let body = one("script", script)?;
                if input_files.contains_key(SCRIPT_FILE) || stored_inputs.contains_key(SCRIPT_FILE)
                {
                    return Err(Outcome::failed_with(format!(
                        "{type_id}: `inputFiles` declares {SCRIPT_FILE:?}, which is where egret \
                         writes this task's `script`; name the file something else"
                    )));
                }
                input_files.insert(SCRIPT_FILE.to_string(), body);
                vec![format!("python {SCRIPT_FILE}")]
            }
        };

        // An input file's NAME is the author's: a problem with it is this
        // task's failure, decided before the gate and before any runner.
        for rel in input_files.keys().chain(stored_inputs.keys()) {
            if let Some(problem) = relative_problem(rel) {
                return Err(Outcome::failed_with(format!(
                    "{type_id}: `inputFiles`: {problem}"
                )));
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

        Ok(TaskCommands {
            container_image: image,
            interpreter,
            before_commands: prefix(p.fail_fast, before),
            commands,
            env,
            input_files,
            stored_inputs,
            output_files,
            host_protocol: false,
        })
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
    ) -> std::result::Result<(BTreeMap<String, String>, BTreeMap<String, StoredInput>), Outcome>
    {
        let mut contents = BTreeMap::new();
        let mut stored = BTreeMap::new();
        for (name, value) in rendered {
            if !value.starts_with(STORAGE_SCHEME) {
                contents.insert(name, value);
                continue;
            }
            let fail = |why: &str| {
                Outcome::failed_with(format!("{type_id}: `inputFiles` {name:?}: {why}"))
            };
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
        Ok((contents, stored))
    }
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
        let cmds = match self.render(type_id, task, &parsed) {
            Ok(c) => c,
            Err(o) => return Ok(o),
        };

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
                let mut outcome = outcome_for(type_id, &result);
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
mod tests {
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

    /// Spec §11: the store unreachable while the runner stages is egret's
    /// outage -- a worker `Err`, no outcome -- while an object missing behind
    /// its index row is the author's failed task naming the URI.
    #[tokio::test]
    async fn an_unreachable_store_while_staging_is_a_worker_error_not_a_failed_task() {
        struct Down;
        #[async_trait::async_trait]
        impl egret_storage::ObjectStore for Down {
            async fn put(
                &self,
                _: &str,
                _: Vec<u8>,
            ) -> std::result::Result<(), egret_storage::StorageError> {
                Err(egret_storage::StorageError::S3("connection refused".into()))
            }
            async fn get(
                &self,
                _: &str,
            ) -> std::result::Result<Vec<u8>, egret_storage::StorageError> {
                Err(egret_storage::StorageError::S3("connection refused".into()))
            }
            async fn delete(
                &self,
                _: &str,
            ) -> std::result::Result<(), egret_storage::StorageError> {
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
            ) -> std::result::Result<egret_storage::Listing, egret_storage::StorageError>
            {
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
        let uri = staged_uri();
        let down = Arc::new(crate::storage::FileStore {
            store: Arc::new(Down),
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
        ];
        for why in cases {
            let spy = Spy::ok(CommandsResult::default());
            let out = stored(spy.clone(), files.clone())
                .run(&in_files_task(&files, &uri, Err(why.clone())))
                .await
                .expect("the author's failure, not the worker's");
            let calls = spy.seen.lock().unwrap().len();
            assert_eq!(calls, 0, "the runner was called {calls} time(s) for {why:?}");
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
            "flow": {"id": "q2_input_files_inline"},
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
        open(spy.clone()).run(&task).await.expect("no worker error");
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
}
