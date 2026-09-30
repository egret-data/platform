//! **The Kestra plugin handler** (W5b spec §7): every `type: kestra.<FQCN>`
//! in [`crate::plugins::catalog`], run by the plugin host image
//! (`plugin-host/`, `deploy/docker/plugin-host.Dockerfile`) on W5a's
//! configured [`TaskRunner`].
//!
//! # What one run does, in order
//!
//! 1. **parse** the leaf spec with [`plugins::parse`] -- the function publish
//!    uses, so a draft that never passed publish is refused here by the same
//!    named errors;
//! 2. **render** the parsed properties ONCE with
//!    [`render_with_secrets`], resolving `secret()` in the execution's
//!    workspace. It is handed the author's UNRENDERED properties: it renders
//!    once (Kestra's `recursiveRendering=false`) and collects literal secret
//!    names from that source text. A render error is already masked by it;
//! 3. **gate** with [`gate::check`] -- the one gate publish also calls -- with
//!    `workflow_kestra_plugins` and `workflow_containers` **read now**
//!    ([`FeatureState`]);
//! 4. **build** the [`TaskCommands`] ([`KestraRuntime::commands`]): the
//!    configured host image, `/bin/sh -c "exec java …"`, the one input file
//!    `task.json` (`{"type": FQCN, "properties": {…rendered…}}`), no output
//!    files, `host_protocol: true`;
//! 5. **mask**: the runner is handed a [`TaskInput`] whose log is
//!    `task.log.masked(values)`, so every line the host prints reaches
//!    `RunLog` with each resolved value replaced by `******`;
//! 6. **run** -- or, when the gate refused, **adopt** a container or pod that
//!    already exists for this (run, attempt), exactly as the script handler
//!    does; `adopt` never creates;
//! 7. **map** the exit code ([`outcome_of`]).
//!
//! # Where a secret's value goes, and where it does not
//!
//! Into `task.json`'s `properties` only -- an input file, `docker cp` from a
//! 0700 staging directory that removes itself however its scope ends
//! (`commands::StagingDir`), or an in-memory tar over `pods/exec` on
//! Kubernetes (`k8s::transfer::pack`). **Never** into the argv, the env,
//! the image, or the pod spec: the command reads the file by
//! `$WORKING_DIR`. Every FAILED reason this handler writes is passed
//! through [`crate::logs::mask`] with the resolved values: the host's
//! `egretFailure` message can echo a connection string. **Outputs are not
//! masked** (spec §12, as Kestra): a query that selects a secret returns it.
//!
//! # The exit codes (`EgretPluginHost`)
//!
//! `0` success, the `::{"outputs": …}::` marker's map as the outputs, as it
//! is; `2` the host refused the task (unknown type, invalid properties);
//! `3` the plugin threw; `4` the plugin asked for something egret does not
//! provide yet -- internal storage, KV, namespace files (W9), and also
//! `localPath`/`decrypt`/`sdk`/`services`, which is why the reason does not
//! say "storage" alone. Any other code (137: the JVM was OOM-killed) is
//! `the plugin host exited with <n>`.
//!
//! # A timeout is the budget's outcome, whatever the host printed
//!
//! `worker::timeout::guard` drops this handler's future when the task's
//! budget (its `timeout:`, or `workflow.container_default_timeout_s` when it
//! has none) runs out, then `cancel::guarded` calls [`TaskHandler::cancel`],
//! which asks the runner to stop the container with grace
//! `workflow.plugin_host.kill_grace_s`: SIGTERM reaches the JVM (PID 1,
//! through `exec`), whose shutdown hook calls the plugin's `kill()`. The run
//! is FAILED with W11's `Timeout after …`.
//!
//! **Known and accepted:** after SIGTERM the host prints `WARN killed:
//! timeout` and, when `kill()` made `run()` throw, an exit-3 `egretFailure`
//! marker. Nobody reads those lines: the runner's future was dropped before
//! `cancel` was called, so no pump is attached during the grace. They are
//! not stored in W11's log and do not change the outcome.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use egret_config::{PluginConfigurations, PluginHostConfig};
use egret_core::Result;
use serde_json::{Map, Value};

use super::super::commands::{CommandsResult, TaskCommands, TaskRunner, DEFAULT_INTERPRETER};
use super::super::{Handlers, Outcome, TaskHandler, TaskInput};
use super::container::mapped;
use super::script::FeatureState;
use crate::logs::mask;
use crate::plugins::{self, gate};
use crate::runners::secrets::{render_with_secrets, SecretSource};
use crate::scripts::{RunnerKind, RunnerSpec};
use crate::worker::commands::NoObserver;

/// The one input file: what the host reads (`EgretPluginHost run <file>`).
pub const TASK_JSON: &str = "task.json";

/// The host's classpath inside the image
/// (`deploy/docker/plugin-host.Dockerfile`: Kestra core and the host jar in
/// `lib/`, the three locked plugin jars in `plugins/`).
pub const HOST_CLASSPATH: &str = "/opt/egret-plugin-host/lib/*:/opt/egret-plugin-host/plugins/*";

/// The host's main class.
pub const HOST_MAIN: &str = "io.kestra.core.runners.EgretPluginHost";

/// The env var the host's shutdown hook reads: how long it waits for `run()`
/// to return after calling `kill()`.
pub const ENV_KILL_WAIT_MS: &str = "EGRET_KILL_WAIT_MS";

/// Everything a [`Kestra`] handler needs besides the task: built once per
/// process (`runtime::handlers_for`) and shared by every catalog type.
pub struct KestraRuntime {
    /// The one runner this deployment has -- the script handler's `Arc`.
    pub runner: Arc<dyn TaskRunner>,
    /// Which runner [`runner`](Self::runner) is.
    pub runner_kind: RunnerKind,
    /// The workspace half of the gate.
    pub features: Arc<dyn FeatureState>,
    /// `workflow.plugin_configurations`: the `kestra` entry's
    /// `allowed-types`.
    pub plugin_configurations: PluginConfigurations,
    /// `workflow.plugin_host`.
    pub host: PluginHostConfig,
    /// Where `secret()` reads, in the execution's workspace.
    pub secrets: Arc<dyn SecretSource>,
}

impl std::fmt::Debug for KestraRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KestraRuntime")
            .field("runner_kind", &self.runner_kind)
            .field("runner", &self.runner)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl KestraRuntime {
    /// The shell command: `exec java …`.
    ///
    /// **`exec` is load-bearing**: without it `/bin/sh` is PID 1, and a
    /// SIGTERM on a timeout never reaches the JVM, so the plugin's `kill()`
    /// never runs and the query outlives its task. The interpreter must be a
    /// shell `-c` for the `$WORKING_DIR` expansion, and on Kubernetes because
    /// a non-shell interpreter merges stderr into stdout (W5b Task 5).
    pub fn command(&self) -> String {
        format!(
            "exec java -Xmx{heap} -cp '{HOST_CLASSPATH}' {HOST_MAIN} run \"${env}/{TASK_JSON}\"",
            heap = self.host.jvm_max_heap,
            env = super::super::commands::ENV_WORKING_DIR,
        )
    }

    /// The host's kill wait: the grace less 2 s, so `run()` returning after
    /// `kill()` still leaves the JVM time to exit before SIGKILL (spec §7.1).
    fn kill_wait_ms(&self) -> u64 {
        self.host.kill_grace_s.saturating_sub(2) * 1000
    }

    /// Step 4 of the module doc. `properties` is the RENDERED map; it goes
    /// into `task.json` and nowhere else.
    pub fn commands(&self, fqcn: &str, properties: Map<String, Value>) -> TaskCommands {
        let task_json = serde_json::json!({
            "type": fqcn,
            "properties": Value::Object(properties),
        });
        TaskCommands {
            container_image: self.host.image.clone().unwrap_or_default(),
            interpreter: DEFAULT_INTERPRETER.iter().map(|s| s.to_string()).collect(),
            before_commands: Vec::new(),
            commands: vec![self.command()],
            env: BTreeMap::from([(
                ENV_KILL_WAIT_MS.to_string(),
                self.kill_wait_ms().to_string(),
            )]),
            input_files: BTreeMap::from([(TASK_JSON.to_string(), task_json.to_string())]),
            output_files: Vec::new(),
            host_protocol: true,
        }
    }
}

/// Register one handler per catalog class, under `kestra.<FQCN>`
/// ([`plugins::prefixed_types`]). With `Handlers::register`, not
/// `add_publishable`: these types have no `TASK_TYPES` row, publish accepts
/// them through the catalog. Called only when a container runner exists.
pub fn register(handlers: &mut Handlers, rt: Arc<KestraRuntime>) {
    for type_id in plugins::prefixed_types() {
        handlers.register(
            type_id,
            Arc::new(Kestra {
                type_id,
                rt: rt.clone(),
            }),
        );
    }
}

/// One catalog type over the shared [`KestraRuntime`].
#[derive(Debug)]
pub struct Kestra {
    type_id: &'static str,
    rt: Arc<KestraRuntime>,
}

/// Step 7: what a finished host means. Every message is masked with
/// `secrets`; the outputs are not (spec §12).
pub fn outcome_of(type_id: &str, result: &CommandsResult, secrets: &[String]) -> Outcome {
    let failed = |why: String| Outcome::failed_with(format!("{type_id}: {}", mask(&why, secrets)));
    // A problem egret found after the host ran (the outputs over W5a's
    // 1 MiB cap) fails the task whatever the exit code.
    if let Some(why) = &result.failure {
        return failed(why.clone());
    }
    let said = || {
        result
            .host_failure
            .as_ref()
            .map(|f| f.message.clone())
            .unwrap_or_else(|| "(the host printed no reason)".to_string())
    };
    match result.exit_code {
        0 => Outcome::succeeded_with(Value::Object(result.vars.clone())),
        2 => failed(format!("the plugin host refused the task: {}", said())),
        3 => failed(format!("the plugin failed: {}", said())),
        4 => failed(format!(
            "the plugin needs something egret does not provide yet (internal storage, KV, \
             namespace files — W9): {}",
            said()
        )),
        n => failed(format!("the plugin host exited with {n}")),
    }
}

#[async_trait::async_trait]
impl TaskHandler for Kestra {
    fn type_id(&self) -> &'static str {
        self.type_id
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let type_id = task.type_id.as_str();

        // 1. parse -- publish's own function.
        let parsed = match plugins::parse(type_id, &task.spec) {
            Ok(p) => p,
            Err(refusals) => {
                let why: Vec<&str> = refusals.iter().map(|r| r.message.as_str()).collect();
                return Ok(Outcome::failed_with(format!(
                    "{type_id}: {}",
                    why.join("; ")
                )));
            }
        };

        // 2. render once, resolving secret(), from the UNRENDERED properties.
        let (rendered, resolved) = match render_with_secrets(
            &parsed.properties,
            &task.render,
            task.workspace_id,
            self.rt.secrets.as_ref(),
        )
        .await
        {
            Ok(r) => r,
            // Already masked by `render_with_secrets`.
            Err(message) => return Ok(Outcome::failed_with(format!("{type_id}: {message}"))),
        };
        let secrets = resolved.values;

        // 3. the gate, with both features read now.
        let feature_enabled = self
            .rt
            .features
            .kestra_plugins_enabled(task.workspace_id)
            .await?;
        let containers_enabled = self
            .rt
            .features
            .containers_enabled(task.workspace_id)
            .await?;
        let refused = gate::check(&gate::GateInput {
            feature_enabled,
            containers_enabled,
            plugin_configurations: &self.rt.plugin_configurations,
            runner: Some(self.rt.runner_kind),
            fqcn: parsed.fqcn,
        })
        .err();

        // 4. the commands; 5. the masked log.
        let cmds = self.rt.commands(parsed.fqcn, rendered);
        let masked = TaskInput {
            log: task.log.masked(&secrets),
            ..task.clone()
        };
        let spec = RunnerSpec::defaults(self.rt.runner_kind);

        // 6. run, or adopt what already exists.
        let ran = match refused {
            Some(refusal) => match self
                .rt
                .runner
                .adopt(&masked, &cmds, &spec, &NoObserver)
                .await
            {
                Ok(Some(result)) => Ok(result),
                Ok(None) => return Ok(Outcome::failed_with(format!("{type_id}: {refusal}"))),
                Err(e) => Err(e),
            },
            None => self.rt.runner.run(&masked, &cmds, &spec, &NoObserver).await,
        };

        // 7. map.
        match ran {
            Ok(result) => Ok(outcome_of(type_id, &result, &secrets)),
            Err(e) => match mapped(type_id, e) {
                Ok(o) => Ok(Outcome {
                    message: o.message.map(|m| mask(&m, &secrets).into_owned()),
                    ..o
                }),
                Err(e) => Err(e),
            },
        }
    }

    /// **A graceful cancel**: the runner stops the attempt's container or pod
    /// with grace `workflow.plugin_host.kill_grace_s`, and returns only once
    /// it stopped (or grace + 5 s passed) -- so the plugin's `kill()` runs
    /// before the outcome commits and before `release` removes the pod. See
    /// the module doc for the host's lines during the grace.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        self.rt
            .runner
            .cancel(task, Duration::from_secs(self.rt.host.kill_grace_s))
            .await
    }

    /// **After the outcome committed**: the runner deletes the attempt's
    /// container or pod. A plugin task has no `taskRunner:` (publish refuses
    /// it), so the spec is always the configured runner's defaults
    /// (`delete: true`). A task that does not parse never reached a runner.
    async fn release(&self, task: &TaskInput) -> Result<()> {
        if plugins::parse(task.type_id.as_str(), &task.spec).is_err() {
            return Ok(());
        }
        self.rt
            .runner
            .release(task, &RunnerSpec::defaults(self.rt.runner_kind))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::container::testing::Spy;
    use super::*;
    use crate::executor::RunState;
    use crate::logs::LogLevel;
    use crate::runners::secrets::SecretError;
    use crate::worker::commands::HostFailure;
    use crate::worker::handlers::script::FixedFeatureState;
    use crate::worker::handlers::testing::{input, recorded};
    use egret_config::{PluginConfiguration, PluginConfigurationValues};

    const QUERY: &str = "kestra.io.kestra.plugin.jdbc.postgresql.Query";
    const FQCN: &str = "io.kestra.plugin.jdbc.postgresql.Query";
    const IMAGE: &str = "registry.example/egret-plugin-host:test";
    const PASSWORD: &str = "pa55-w0rd!xyz";

    /// `secret('db', subkey='password')` answers [`PASSWORD`], in any
    /// workspace; every read is recorded.
    #[derive(Default)]
    struct OneSecret {
        reads: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl SecretSource for OneSecret {
        async fn get(
            &self,
            _workspace: uuid::Uuid,
            name: &str,
        ) -> std::result::Result<BTreeMap<String, String>, SecretError> {
            self.reads.lock().unwrap().push(name.to_string());
            if name == "db" {
                Ok(BTreeMap::from([
                    ("username".to_string(), "admin".to_string()),
                    ("password".to_string(), PASSWORD.to_string()),
                ]))
            } else {
                Err(SecretError::NotFound { name: name.into() })
            }
        }
    }

    fn runtime(spy: Arc<Spy>, feature: bool, configs: PluginConfigurations) -> Arc<KestraRuntime> {
        Arc::new(KestraRuntime {
            runner: spy,
            runner_kind: RunnerKind::Docker,
            features: Arc::new(FixedFeatureState(feature)),
            plugin_configurations: configs,
            host: PluginHostConfig {
                image: Some(IMAGE.to_string()),
                ..PluginHostConfig::default()
            },
            secrets: Arc::new(OneSecret::default()),
        })
    }

    /// The handler for [`QUERY`], everything allowed.
    fn handler(spy: Arc<Spy>) -> Kestra {
        Kestra {
            type_id: QUERY,
            rt: runtime(spy, true, PluginConfigurations::default()),
        }
    }

    fn spec() -> Value {
        serde_json::json!({
            "url": "jdbc:postgresql://pg:5432/w5b",
            "username": "w5b",
            "password": "{{ secret('db', subkey='password') }}",
            "sql": "SELECT {{ inputs.n }} AS one",
            "fetchType": "FETCH_ONE",
        })
    }

    fn task() -> TaskInput {
        TaskInput {
            render: crate::runners::RenderContext::from_json_object(
                &serde_json::json!({"inputs": {"n": 1}}),
            )
            .expect("an object"),
            ..input(QUERY, spec())
        }
    }

    fn task_json(cmds: &TaskCommands) -> Value {
        serde_json::from_str(cmds.input_files.get(TASK_JSON).expect("task.json")).expect("json")
    }

    fn ok(vars: Value) -> CommandsResult {
        CommandsResult {
            exit_code: 0,
            vars: vars.as_object().cloned().expect("an object"),
            ..CommandsResult::default()
        }
    }

    fn message(out: &Outcome) -> &str {
        out.message.as_deref().unwrap_or_default()
    }

    // ---- the run-time gate --------------------------------------------------

    /// **A disabled feature fails the run and no runner creates anything**:
    /// the one runner call is `adopt`, which never creates.
    #[tokio::test]
    async fn a_disabled_feature_fails_the_run_and_calls_no_runner() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), false, PluginConfigurations::default()),
        };
        let out = h
            .run(&task())
            .await
            .expect("a refusal is the task's failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            message(&out).contains("'workflow_kestra_plugins' feature is disabled"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty(), "`run` never called");
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 1);
    }

    /// **A type outside `allowed-types` is refused at run** by the same gate.
    #[tokio::test]
    async fn a_type_outside_allowed_types_fails_the_run_and_calls_no_runner() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let configs = PluginConfigurations(vec![PluginConfiguration {
            r#type: plugins::CONFIG_TYPE.to_string(),
            values: PluginConfigurationValues {
                allowed_images: None,
                allowed_types: Some(vec!["io.kestra.plugin.jdbc.trino.*".into()]),
            },
        }]);
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), true, configs),
        };
        let out = h.run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("allowed-types"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// **A refused run whose container already exists is adopted and
    /// finishes**; with nothing to adopt the same refusal fails, and `run`
    /// (create) is never called either way.
    #[tokio::test]
    async fn a_refused_run_adopts_its_existing_container_and_never_creates_one() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        *spy.adoptable.lock().unwrap() = Some(ok(serde_json::json!({"size": 1})));
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), false, PluginConfigurations::default()),
        };
        let out = h.run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(out.outputs, Some(serde_json::json!({"size": 1})));
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 1);
        assert!(spy.seen.lock().unwrap().is_empty(), "`run` never called");

        // An admitted run is run, never adopted.
        let spy = Spy::ok(ok(serde_json::json!({})));
        handler(spy.clone()).run(&task()).await.expect("ran");
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
        assert_eq!(spy.seen.lock().unwrap().len(), 1);
    }

    /// An unreadable feature state is egret's outage: `Err`, no runner call.
    #[tokio::test]
    async fn an_unreadable_feature_state_is_a_worker_failure_and_calls_no_runner() {
        #[derive(Debug)]
        struct Down;
        #[async_trait::async_trait]
        impl FeatureState for Down {
            async fn containers_enabled(&self, _: uuid::Uuid) -> Result<bool> {
                Ok(true)
            }
            async fn kestra_plugins_enabled(&self, _: uuid::Uuid) -> Result<bool> {
                Err(egret_core::Error::Internal(anyhow::anyhow!(
                    "database down"
                )))
            }
        }
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut rt = runtime(spy.clone(), true, PluginConfigurations::default());
        Arc::get_mut(&mut rt).expect("unshared").features = Arc::new(Down);
        let h = Kestra { type_id: QUERY, rt };
        assert!(h.run(&task()).await.is_err());
        assert!(spy.seen.lock().unwrap().is_empty());
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
    }

    /// A property publish refuses is refused at run, by name, before any
    /// secret is read or any runner called (a draft that skipped publish).
    #[tokio::test]
    async fn a_property_publish_refuses_is_refused_at_run() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let h = handler(spy.clone());
        let mut s = spec();
        s["fetchType"] = serde_json::json!("STORE");
        let out = h.run(&input(QUERY, s)).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("W9"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
    }

    /// A secret that does not exist fails the task naming it, with no
    /// runner call.
    #[tokio::test]
    async fn a_missing_secret_fails_the_task_and_calls_no_runner() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut s = spec();
        s["password"] = serde_json::json!("{{ secret('nope') }}");
        let out = handler(spy.clone())
            .run(&input(QUERY, s))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("'nope'"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    // ---- the commands --------------------------------------------------------

    /// **The host image, `exec java`, and `task.json` as the only input.**
    #[tokio::test]
    async fn the_commands_run_the_host_image_with_exec_java_and_task_json_as_the_only_input() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut s = spec();
        // Keys egret's Task model reads never reach the plugin.
        s["timeout"] = serde_json::json!("PT5S");
        s["retry"] = serde_json::json!({"type": "constant", "maxAttempts": 2});
        let out = handler(spy.clone())
            .run(&TaskInput { spec: s, ..task() })
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        assert_eq!(cmds.container_image, IMAGE);
        assert_eq!(
            cmds.argv(),
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "exec java -Xmx384m -cp '/opt/egret-plugin-host/lib/*:/opt/egret-plugin-host/\
                 plugins/*' io.kestra.core.runners.EgretPluginHost run \"$WORKING_DIR/task.json\""
                    .to_string(),
            ]
        );
        assert!(cmds.argv().join(" ").contains("exec java -Xmx384m"));
        assert_eq!(
            cmds.input_files.keys().collect::<Vec<_>>(),
            vec![TASK_JSON],
            "task.json is the only input"
        );
        assert!(cmds.output_files.is_empty());
        assert!(cmds.host_protocol);
        assert_eq!(
            cmds.env,
            BTreeMap::from([(ENV_KILL_WAIT_MS.to_string(), "13000".to_string())]),
            "the one env var is the host's kill wait: grace 15 s less 2 s"
        );
        // task.json: the FQCN without `kestra.`, the RENDERED properties,
        // and nothing egret's Task model consumes.
        let doc = task_json(&cmds);
        assert_eq!(doc["type"], FQCN);
        assert_eq!(
            doc["properties"],
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w5b",
                "username": "w5b",
                "password": PASSWORD,
                "sql": "SELECT 1 AS one",
                "fetchType": "FETCH_ONE",
            })
        );
        // No rendered property value in the env.
        let props = doc["properties"].as_object().expect("a map");
        for v in props.values().filter_map(Value::as_str) {
            assert!(
                cmds.env.values().all(|e| !e.contains(v)),
                "{v} is in the env"
            );
        }
    }

    /// The configured heap and grace reach the command and the env.
    #[tokio::test]
    async fn the_configured_heap_and_grace_reach_the_command() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut rt = runtime(spy.clone(), true, PluginConfigurations::default());
        {
            let rt = Arc::get_mut(&mut rt).expect("unshared");
            rt.host.jvm_max_heap = "1g".into();
            rt.host.kill_grace_s = 30;
        }
        Kestra { type_id: QUERY, rt }
            .run(&task())
            .await
            .expect("ran");
        let cmds = spy.only();
        assert!(
            cmds.argv()[2].starts_with("exec java -Xmx1g -cp "),
            "{:?}",
            cmds.argv()
        );
        assert_eq!(cmds.env[ENV_KILL_WAIT_MS], "28000");
    }

    /// **A secret is in `task.json` and nowhere in the argv, the env or the
    /// image**; and the log line and the FAILED reason that echo it are
    /// masked.
    #[tokio::test]
    async fn a_secret_is_in_task_json_and_nowhere_in_the_commands_or_env() {
        let spy = Spy::ok(CommandsResult {
            exit_code: 3,
            host_failure: Some(HostFailure {
                code: 3,
                message: format!("PSQLException: password {PASSWORD} rejected"),
            }),
            ..CommandsResult::default()
        })
        .streaming(vec![
            (false, format!("INFO connecting with {PASSWORD}")),
            (true, format!("stderr {PASSWORD}")),
        ]);
        let (mut t, rec) = recorded(QUERY, spec());
        t.render = task().render;
        let out = handler(spy.clone()).run(&t).await.expect("no worker error");

        let cmds = spy.only();
        assert!(
            cmds.input_files[TASK_JSON].contains(PASSWORD),
            "the one place the value goes"
        );
        assert!(!cmds.argv().join("\n").contains(PASSWORD));
        assert!(cmds
            .env
            .iter()
            .all(|(k, v)| !k.contains(PASSWORD) && !v.contains(PASSWORD)));
        assert!(!cmds.container_image.contains(PASSWORD));

        assert_eq!(
            rec.texts(),
            vec![
                (LogLevel::Info, "connecting with ******".to_string()),
                (LogLevel::Error, "stderr ******".to_string()),
            ]
        );
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            message(&out),
            format!("{QUERY}: the plugin failed: PSQLException: password ****** rejected")
        );
    }

    /// A runner failure's text is masked too.
    #[tokio::test]
    async fn a_runner_failure_is_masked() {
        let spy = Spy::ok(CommandsResult {
            exit_code: 0,
            failure: Some(format!("outputs over the cap near {PASSWORD}")),
            ..CommandsResult::default()
        });
        let out = handler(spy).run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(!message(&out).contains(PASSWORD), "{:?}", out.message);
        assert!(message(&out).contains("******"), "{:?}", out.message);

        let spy = Spy::new(Err(egret_core::Error::InvalidInput(format!(
            "no such image near {PASSWORD}"
        ))));
        let out = handler(spy).run(&task()).await.expect("a task failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(!message(&out).contains(PASSWORD), "{:?}", out.message);
    }

    // ---- the outcome ---------------------------------------------------------

    /// **Each host exit code is a named reason.**
    #[tokio::test]
    async fn exit_codes_map_to_named_reasons() {
        let cases: [(i32, Option<&str>, String); 5] = [
            (
                2,
                Some("invalid io.kestra.plugin.jdbc.postgresql.Query: sql: must not be null"),
                format!(
                    "{QUERY}: the plugin host refused the task: invalid \
                     io.kestra.plugin.jdbc.postgresql.Query: sql: must not be null"
                ),
            ),
            (
                3,
                Some("org.postgresql.util.PSQLException: boom"),
                format!("{QUERY}: the plugin failed: org.postgresql.util.PSQLException: boom"),
            ),
            (
                4,
                Some("storage() needs internal storage (W9)"),
                format!(
                    "{QUERY}: the plugin needs something egret does not provide yet (internal \
                     storage, KV, namespace files — W9): storage() needs internal storage (W9)"
                ),
            ),
            (
                3,
                None,
                format!("{QUERY}: the plugin failed: (the host printed no reason)"),
            ),
            (
                137,
                None,
                format!("{QUERY}: the plugin host exited with 137"),
            ),
        ];
        for (code, said, want) in cases {
            let spy = Spy::ok(CommandsResult {
                exit_code: code,
                host_failure: said.map(|m| HostFailure {
                    code,
                    message: m.to_string(),
                }),
                ..CommandsResult::default()
            });
            let out = handler(spy).run(&task()).await.expect("no worker error");
            assert_eq!(out.state, RunState::Failed, "exit {code}");
            assert_eq!(message(&out), want, "exit {code}");
            assert_eq!(
                out.outputs, None,
                "exit {code}: a plugin that threw has no Output"
            );
        }
    }

    /// **Exit 0: the `outputs` marker's map is the outputs, as it is** --
    /// no `exitCode`/`vars` wrapping, no masking.
    #[tokio::test]
    async fn outputs_are_the_markers_outputs_as_they_are() {
        let vars = serde_json::json!({
            "row": {"one": 1, "pw": PASSWORD},
            "size": 1,
        });
        let spy = Spy::ok(ok(vars.clone()));
        let out = handler(spy).run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Success);
        assert_eq!(out.outputs, Some(vars), "not masked, spec §12");
        assert_eq!(out.message, None);
    }

    // ---- cancel, release, the image, registration ---------------------------

    /// **`cancel` asks the runner with the configured grace**, not W5a's 0.
    #[tokio::test]
    async fn cancel_passes_the_configured_grace() {
        let spy = Spy::ok(CommandsResult::default());
        let h = handler(spy.clone());
        h.cancel(&task()).await.expect("cancelled");
        let mut rt = runtime(spy.clone(), true, PluginConfigurations::default());
        Arc::get_mut(&mut rt).expect("unshared").host.kill_grace_s = 7;
        Kestra { type_id: QUERY, rt }
            .cancel(&task())
            .await
            .expect("cancelled");
        assert_eq!(
            spy.cancel_graces.lock().unwrap().as_slice(),
            [Duration::from_secs(15), Duration::from_secs(7)]
        );
        assert_eq!(
            spy.cancelled.lock().unwrap().as_slice(),
            [
                crate::worker::commands::container_name(&task()),
                crate::worker::commands::container_name(&task()),
            ]
        );
    }

    /// `release` delegates with the configured runner's defaults; a task that
    /// does not parse never reached a runner.
    #[tokio::test]
    async fn release_delegates_to_the_configured_runner() {
        let spy = Spy::ok(CommandsResult::default());
        let h = handler(spy.clone());
        h.release(&task()).await.expect("released");
        h.release(&input(QUERY, serde_json::json!({"sqll": "x"})))
            .await
            .expect("nothing to release");
        assert_eq!(
            spy.released.lock().unwrap().as_slice(),
            [(crate::worker::commands::container_name(&task()), true)]
        );
    }

    /// **The host image is not matched against `allowed-images`**: no entry
    /// anywhere (which allows NO image to a script task) and an entry that
    /// allows only `python:*` both run the host image.
    #[tokio::test]
    async fn the_host_image_is_not_matched_against_allowed_images() {
        for configs in [
            PluginConfigurations::default(),
            PluginConfigurations(vec![PluginConfiguration {
                r#type: crate::scripts::DOCKER_RUNNER.to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["python:*".into()]),
                    allowed_types: None,
                },
            }]),
        ] {
            let spy = Spy::ok(ok(serde_json::json!({})));
            let h = Kestra {
                type_id: QUERY,
                rt: runtime(spy.clone(), true, configs),
            };
            let out = h.run(&task()).await.expect("no worker error");
            assert_eq!(out.state, RunState::Success, "{:?}", out.message);
            assert_eq!(spy.only().container_image, IMAGE);
        }
    }

    /// **Registration**: exactly one key per catalog class, `kestra.<FQCN>`,
    /// each serving a plugin handler named by that key.
    #[test]
    fn register_serves_every_catalog_type_with_the_prefix_and_nothing_else() {
        let mut reg = Handlers::new();
        register(
            &mut reg,
            runtime(
                Spy::ok(CommandsResult::default()),
                false,
                PluginConfigurations::default(),
            ),
        );
        let want: Vec<String> = plugins::catalog()
            .classes
            .keys()
            .map(|fqcn| format!("kestra.{fqcn}"))
            .collect();
        assert_eq!(want.len(), 5, "the three jars' five runnable classes");
        assert_eq!(
            reg.type_ids(),
            want.iter().map(String::as_str).collect::<Vec<_>>()
        );
        for id in reg.type_ids() {
            assert_eq!(reg.get(id).expect("served").type_id(), id);
            assert!(crate::spec::registry::known_type(id).is_none());
        }
    }
}
