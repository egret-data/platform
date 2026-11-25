//! Package 10b Task 2: `secret()` in a script task's `env:`, through the REAL
//! worker and the REAL log writer. Requires DATABASE_URL.
//!
//! The secret-bearing entry reaches the runner through `secret_env` (never the
//! inline `env`), the plain entry stays inline, `secret_masks` holds the
//! resolved value, and a line the script echoes is stored masked.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
use egret_workflow::logs::{spawn_writer, LogLevel, LogLimits, LogRepo, WriterConfig};
use egret_workflow::runners::secrets::{self, SecretError, SecretSource};
use egret_workflow::scripts::{RunnerKind, RunnerSpec};
use egret_workflow::worker::commands::{CommandsResult, LineObserver, TaskCommands, TaskRunner};
use egret_workflow::worker::handlers::script::{self, FixedFeatureState, ScriptRuntime};
use egret_workflow::worker::{start_execution, tick, Handlers, TaskInput};
use uuid::Uuid;

const SENTINEL: &str = "hunter2-SENTINEL";

struct OneSecret;

#[async_trait::async_trait]
impl SecretSource for OneSecret {
    async fn get(
        &self,
        _workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError> {
        if name == "API" {
            Ok(BTreeMap::from([(
                "value".to_string(),
                SENTINEL.to_string(),
            )]))
        } else {
            Err(SecretError::NotFound { name: name.into() })
        }
    }
}

/// Records the commands it is handed and "echoes $TOK" the way a runner does:
/// through a log masked with the commands' `secret_masks`.
#[derive(Debug, Default)]
struct Echo {
    seen: Mutex<Vec<TaskCommands>>,
}

#[async_trait::async_trait]
impl TaskRunner for Echo {
    async fn run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        _spec: &RunnerSpec,
        _observer: &dyn LineObserver,
    ) -> egret_core::Result<CommandsResult> {
        self.seen.lock().unwrap().push(commands.clone());
        let tok = commands.secret_env.get("TOK").cloned().unwrap_or_default();
        task.log
            .masked(&commands.secret_masks)
            .line(LogLevel::Info, &format!("tok={tok}"))
            .await;
        Ok(CommandsResult::default())
    }

    async fn adopt(
        &self,
        _task: &TaskInput,
        _commands: &TaskCommands,
        _spec: &RunnerSpec,
        _observer: &dyn LineObserver,
    ) -> egret_core::Result<Option<CommandsResult>> {
        Ok(None)
    }

    async fn cancel(
        &self,
        _task: &TaskInput,
        _grace: std::time::Duration,
        _spec: &RunnerSpec,
    ) -> egret_core::Result<()> {
        Ok(())
    }

    async fn release(&self, _task: &TaskInput, _spec: &RunnerSpec) -> egret_core::Result<()> {
        Ok(())
    }
}

const FLOW: &str = "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    \
     type: io.kestra.plugin.scripts.shell.Commands\n    \
     containerImage: alpine:3.20\n    commands: [\"echo $TOK\"]\n    \
     env:\n      TOK: \"{{ secret('API') }}\"\n      PLAIN: hi\n";

/// Run `flow` through the real worker; the one script task's commands and the
/// stored log lines. `None` when there is no database.
async fn run_flow(flow: &str) -> Option<(TaskCommands, Vec<String>)> {
    // Both tests tick the same database: one at a time, so neither claims
    // the other's run with its own runner.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _turn = SERIAL.lock().await;
    let db = common::db().await?;
    let pool = db.pool.clone();
    let (ws, _user) = common::seed_workspace(&db).await;
    let _ = secrets::install(Arc::new(OneSecret));
    let ex = start_execution(&pool, ws, None, flow, serde_json::json!({}))
        .await
        .expect("start");

    let runner = Arc::new(Echo::default());
    let cancel = tokio_util::sync::CancellationToken::new();
    let (sink, writer) = spawn_writer(
        LogRepo::new(pool.clone()),
        WriterConfig::default(),
        cancel.clone(),
    );
    let mut reg =
        Handlers::with_core_tasks(None).with_log_sink(Arc::new(sink), LogLimits::default());
    script::register(
        &mut reg,
        Arc::new(ScriptRuntime {
            runner_kind: RunnerKind::Docker,
            runner: runner.clone(),
            plugin_configurations: PluginConfigurations(vec![PluginConfiguration {
                r#type: RunnerKind::Docker.fqcn().to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["*".into(), "*/*".into()]),
                    allowed_types: None,
                },
            }]),
            features: Arc::new(FixedFeatureState(true)),
            files: None,
            pool: None,
        }),
    );
    for _ in 0..16 {
        if tick(&pool, 8, &reg).await.expect("tick") == 0 {
            break;
        }
    }
    drop(reg);
    cancel.cancel();
    writer.await.unwrap();

    let (state, message): (String, Option<String>) = sqlx::query_as(
        "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 's'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "SUCCESS", "{message:?}");

    let seen = runner.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the runner ran once");
    let cmds = seen[0].clone();

    let lines: Vec<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    Some((cmds, lines))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_env_resolves_secret_into_the_secure_channel_and_masks_it() {
    let Some((cmds, lines)) = run_flow(FLOW).await else {
        return;
    };
    assert_eq!(
        cmds.secret_env.get("TOK").map(String::as_str),
        Some(SENTINEL)
    );
    assert!(!cmds.env.contains_key("TOK"), "the secret is never inline");
    assert_eq!(cmds.env.get("PLAIN").map(String::as_str), Some("hi"));
    assert_eq!(cmds.secret_masks, vec![SENTINEL.to_string()]);
    assert!(
        lines.iter().any(|l| l == "tok=******"),
        "the masked form is stored: {lines:?}"
    );
    assert!(
        lines.iter().all(|l| !l.contains(SENTINEL)),
        "no stored line holds the value"
    );
}

/// A secret env key that shadows a seeded python default lives ONLY in
/// `secret_env`: the two maps are disjoint, the secret wins.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_env_key_shadowing_a_python_default_is_only_in_secret_env() {
    let flow = "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    \
         type: io.kestra.plugin.scripts.python.Commands\n    \
         containerImage: python:3.12\n    commands: [\"echo hi\"]\n    \
         env:\n      PYTHONUNBUFFERED: \"{{ secret('API') }}\"\n";
    let Some((cmds, _)) = run_flow(flow).await else {
        return;
    };
    assert_eq!(
        cmds.secret_env.get("PYTHONUNBUFFERED").map(String::as_str),
        Some(SENTINEL)
    );
    assert!(
        !cmds.env.contains_key("PYTHONUNBUFFERED"),
        "neither the default nor the secret stays inline: {:?}",
        cmds.env
    );
    assert!(cmds.env.keys().all(|k| !cmds.secret_env.contains_key(k)));
}
