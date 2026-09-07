//! `egret.container` — run a command in a container image.
//!
//! # This handler ships with NO oracle coverage, and that is measured
//!
//! `oracle-exec/corpus/` holds **345** flows and **zero** of them declare a
//! script, container, shell or command task type — measured with
//! `grep -rhoE 'type:\s*\S+' corpus | sort -u | grep -iE
//! 'script|container|shell|command|docker|bash|python|node'`, which matches
//! nothing. Kestra's own equivalents live in plugins (`plugin-script-*`,
//! `plugin-docker`) that the corpus never reaches.
//!
//! **So the execution ratchet cannot move for this file, in either direction,
//! and a green ratchet says nothing about it.** The instruments are the unit
//! tests at the bottom of this module — which drive a fake
//! [`TaskRunner`](crate::worker::commands::TaskRunner) and therefore judge the
//! handler and not the daemon — and `tests/container_docker_live.rs`, which
//! runs against a real Docker and **fails** when there is none. That split is
//! `handlers::sql`'s exactly, and it exists for the reason that file gives: a
//! fake decides what the outside world does, which is the class of defect it
//! cannot see.
//!
//! # What it corresponds to in Kestra, and what it deliberately is not
//!
//! `io.kestra.plugin.scripts.shell.Commands` over the `Docker` task runner —
//! `AbstractExecScript` (`commands`, `beforeCommands`, `env`, `outputFiles`,
//! `interpreter`, `failFast`) plus `Docker` (`image`). It is **not** a port of
//! the whole of either: `namespaceFiles`, `inputFiles`, `outputDirectory`,
//! `targetOS`, `warningOnStdErr`, `pullPolicy`, `credentials` and the resource
//! knobs are absent, each because it needs a seam this crate has not got, and
//! `crate::worker::commands`'s module doc lists them one by one.
//!
//! # The type id is `egret.container` and not a Kestra FQCN
//!
//! The four core handlers register under Kestra's own class names because a
//! corpus flow writes those and `executor::build_plan` puts them in
//! `TaskKind::Runnable::type_id` verbatim. Nothing in the corpus writes this
//! one, so there is no name to match; `egret.sql` set the precedent for egret's
//! own short spelling and this follows it.
//!
//! **`crate::spec::registry::TASK_TYPES` does not hold it**, so a flow using
//! this type cannot be published through egret's own publish path today. That
//! is the same disagreement `worker::registry::Handlers` documents for the
//! other four and it is reported rather than papered over: deciding egret's
//! public task vocabulary is a spec-level choice, and adding a row to that
//! table would also have to decide what a `RefField` means for an image name.

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_core::{Error, Result};
use serde::Deserialize;

use super::super::commands::{CommandsResult, DockerRunner, TaskCommands, TaskRunner};
use super::super::{Outcome, TaskHandler, TaskInput};
use super::{default_sink, LogSink};
use crate::runners::render_str;

/// The `type:` a flow writes.
pub const TYPE_ID: &str = "egret.container";

/// `egret.container`'s own properties.
///
/// `deny_unknown_fields` for [`handlers::sql`](super::sql)'s reason: a
/// misspelled `outputfiles:` that was ignored would run the command and produce
/// no files, and the author would be debugging an empty `{{ outputs.c.files }}`
/// rather than a typo.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    /// `Docker.image`. Required — `Docker.run` refuses without it and so does
    /// [`DockerRunner::run`], with the artifact's own message.
    image: String,
    /// `AbstractExecScript.commands`. One shell script, joined with `"\n"` by
    /// [`TaskCommands::argv`].
    commands: Vec<String>,
    /// `AbstractExecScript.beforeCommands`.
    #[serde(default, rename = "beforeCommands")]
    before_commands: Vec<String>,
    /// `AbstractExecScript.env`.
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// `AbstractExecScript.outputFiles`, literal names only.
    #[serde(default, rename = "outputFiles")]
    output_files: Vec<String>,
    /// `AbstractExecScript.interpreter`, defaulting to `["/bin/sh", "-c"]`
    /// inside [`TaskCommands::argv`] rather than here, so the default lives in
    /// one place and both handlers get it.
    #[serde(default)]
    interpreter: Vec<String>,
    /// `AbstractExecScript.failFast` — measured default **true**
    /// (`$default$failFast` is `Property.ofValue(true)`), which prepends
    /// `set -e` so a script's first failing line ends it.
    ///
    /// Without it a three-line script whose first line fails still exits 0 on
    /// the third, and the run is a green that ran none of what it claimed.
    #[serde(default = "yes", rename = "failFast")]
    fail_fast: bool,
}

fn yes() -> bool {
    true
}

/// `egret.container`, over a [`TaskRunner`].
pub struct Container {
    runner: Arc<dyn TaskRunner>,
    sink: Arc<dyn LogSink>,
}

impl std::fmt::Debug for Container {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Container")
            .field("runner", &self.runner)
            .finish_non_exhaustive()
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Container {
    /// The shipping handler: a [`DockerRunner`] and `tracing`.
    pub fn new() -> Self {
        Self::with_runner(Arc::new(DockerRunner::new()))
    }

    /// The same handler over another runner — a Kubernetes one later, a fake
    /// in a test.
    pub fn with_runner(runner: Arc<dyn TaskRunner>) -> Self {
        Self {
            runner,
            sink: default_sink(),
        }
    }

    /// The constructor a test uses to see what was logged. A container's
    /// stdout is most of what it produces, so a test that could not read the
    /// log lines could only assert that it ran.
    pub fn with_sink(mut self, sink: Arc<dyn LogSink>) -> Self {
        self.sink = sink;
        self
    }
}

/// `set -e`, `CommandsWrapper`'s own literal.
pub(super) const EXIT_ON_ERROR: &str = "set -e";

/// The outputs one finished command produces.
///
/// `ScriptOutput`'s shape minus what egret has no storage for: `vars`,
/// `exitCode`, `outputFiles`, `stdOutLineCount`, `stdErrLineCount`. The keys
/// are Kestra's own spellings so an author who has read its docs is not
/// surprised.
///
/// **`outputFiles` maps a name to its CONTENTS, where Kestra's maps a name to a
/// URI.** `crate::worker::commands`'s module doc carries the reason and the
/// cap; it is the one place this handler's outputs are shaped differently from
/// the artifact's, and it is not silent.
pub(super) fn outputs_for(result: &CommandsResult) -> serde_json::Value {
    serde_json::json!({
        "exitCode": result.exit_code,
        "vars": serde_json::Value::Object(result.vars.clone()),
        "outputFiles": result
            .output_files
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect::<serde_json::Map<_, _>>(),
        "stdOutLineCount": result.stdout_line_count,
        "stdErrLineCount": result.stderr_line_count,
    })
}

/// What a finished command means, as an [`Outcome`].
///
/// **A non-zero exit fails the task and KEEPS its outputs**, which is the
/// artifact's own arrangement rather than a convenience: `Docker.run` throws
/// `new TaskException(exitCode, logConsumer)`, `CommandsWrapper.run` catches it
/// and rethrows `RunnableTaskException(e, ScriptOutput)` — an exception
/// carrying an `Output`. `apply_outcome` writes `outputs` from the outcome
/// whatever its state, so `{{ outputs.c.exitCode }}` on a failed task resolves
/// here as it does there.
pub(super) fn outcome_for(type_id: &str, result: &CommandsResult) -> Outcome {
    let outputs = outputs_for(result);
    if result.exit_code == 0 {
        return Outcome::succeeded_with(outputs);
    }
    Outcome {
        state: crate::executor::RunState::Failed,
        outputs: Some(outputs),
        message: Some(format!(
            "{type_id}: the command exited with code {}",
            result.exit_code
        )),
    }
}

/// One runner failure, split the way [`handlers::sql`](super::sql) splits a
/// port failure.
///
/// * [`Error::Internal`] — the daemon could not be reached. A **worker**
///   failure: it aborts the tick and leaves the execution where it was, because
///   the flow is not wrong and failing it would blame the author for egret's
///   outage. `egret.sql` reasons identically about a metastore that is down.
/// * anything else — the image does not exist, an `outputFiles` entry was not
///   produced, a declared file is over the cap. A **failed task**, message and
///   all, because that text is the one useful thing in an authoring loop and
///   the author's own `retry:` can act on it.
pub(super) fn mapped(type_id: &str, e: Error) -> Result<Outcome> {
    if matches!(e, Error::Internal(_)) {
        return Err(Error::Internal(anyhow::anyhow!(
            "{type_id}: the container runner failed for a reason that is not the \
             author's flow: {e}"
        )));
    }
    Ok(Outcome::failed_with(format!("{type_id}: {e}")))
}

/// Render every string in a list, or say which one did not.
pub(super) fn render_all(
    type_id: &str,
    what: &str,
    items: &[String],
    cx: &crate::runners::RenderContext,
) -> std::result::Result<Vec<String>, Outcome> {
    items
        .iter()
        .map(|s| {
            render_str(s, cx).map_err(|e| {
                Outcome::failed_with(format!("{type_id}: `{what}` did not render: {e}"))
            })
        })
        .collect()
}

/// Render a map's values, keeping its keys literal.
///
/// The keys are environment variable names; a `{{ … }}` in one would be a
/// name computed at run time, which nothing in Kestra allows either (`env` is
/// `Property<Map<String, String>>`, rendered as a whole, and a rendered key
/// that collided with another would silently drop one of them).
pub(super) fn render_values(
    type_id: &str,
    what: &str,
    items: &BTreeMap<String, String>,
    cx: &crate::runners::RenderContext,
) -> std::result::Result<BTreeMap<String, String>, Outcome> {
    items
        .iter()
        .map(|(k, v)| {
            render_str(v, cx).map(|v| (k.clone(), v)).map_err(|e| {
                Outcome::failed_with(format!("{type_id}: `{what}` did not render: {e}"))
            })
        })
        .collect()
}

#[async_trait::async_trait]
impl TaskHandler for Container {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };

        let image = match render_str(&spec.image, &task.render) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Outcome::failed_with(format!(
                    "{TYPE_ID}: `image` did not render: {e}"
                )))
            }
        };
        let commands = match render_all(TYPE_ID, "commands", &spec.commands, &task.render) {
            Ok(v) => v,
            Err(o) => return Ok(o),
        };
        let mut before = match render_all(
            TYPE_ID,
            "beforeCommands",
            &spec.before_commands,
            &task.render,
        ) {
            Ok(v) => v,
            Err(o) => return Ok(o),
        };
        if spec.fail_fast {
            // `AbstractExecScript.mayAddExitOnErrorCommands` prepends it to the
            // BEFORE commands, so it applies to those too.
            before.insert(0, EXIT_ON_ERROR.to_string());
        }
        let env = match render_values(TYPE_ID, "env", &spec.env, &task.render) {
            Ok(v) => v,
            Err(o) => return Ok(o),
        };
        let output_files =
            match render_all(TYPE_ID, "outputFiles", &spec.output_files, &task.render) {
                Ok(v) => v,
                Err(o) => return Ok(o),
            };
        let interpreter = match render_all(TYPE_ID, "interpreter", &spec.interpreter, &task.render)
        {
            Ok(v) => v,
            Err(o) => return Ok(o),
        };

        let cmds = TaskCommands {
            container_image: image,
            interpreter,
            before_commands: before,
            commands,
            env,
            input_files: BTreeMap::new(),
            output_files,
        };

        match self.runner.run(task, &cmds, self.sink.as_ref()).await {
            Ok(result) => Ok(outcome_for(TYPE_ID, &result)),
            Err(e) => mapped(TYPE_ID, e),
        }
    }

    /// **This handler CAN cancel, and does** — the first in the crate that
    /// can.
    ///
    /// `handlers::sql` refuses because `QueryExecPort` has no cancellation verb
    /// and a dropped future tells a MySQL-protocol engine nothing. A container
    /// is different in exactly the way that matters: the daemon is a second
    /// channel to the work, addressable after the future that started it is
    /// gone. `TaskHandler::cancel`'s doc calls the four core handlers' `Ok(())`
    /// truthful because they hold nothing; this one is truthful because it
    /// actually stops the container.
    ///
    /// The address is [`container_name`](crate::worker::commands::container_name),
    /// which carries this worker's **lease**. That is what stops a reaped
    /// worker from killing its replacement's container, and it is the whole of
    /// why [`TaskInput::claimed_at`] exists.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        self.runner.cancel(task).await
    }
}

#[cfg(test)]
pub(super) mod testing {
    //! A [`TaskRunner`] that answers however the test built it, and records
    //! what it was asked.
    //!
    //! Shared with `handlers::python`'s tests, which need the same thing.

    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Default)]
    pub struct Spy {
        pub seen: Mutex<Vec<TaskCommands>>,
        pub cancelled: Mutex<Vec<String>>,
        answer: Mutex<Option<Result<CommandsResult>>>,
    }

    impl Spy {
        pub fn new(answer: Result<CommandsResult>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(Some(answer)),
                ..Self::default()
            })
        }
        pub fn ok(result: CommandsResult) -> Arc<Self> {
            Self::new(Ok(result))
        }
        pub fn only(&self) -> TaskCommands {
            let seen = self.seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "expected exactly one run");
            seen[0].clone()
        }
    }

    #[async_trait::async_trait]
    impl TaskRunner for Spy {
        async fn run(
            &self,
            _task: &TaskInput,
            commands: &TaskCommands,
            sink: &dyn LogSink,
        ) -> Result<CommandsResult> {
            self.seen.lock().unwrap().push(commands.clone());
            // A real runner streams; this one emits the one line the argv it
            // was given would print, so a test can assert the sink is wired
            // without a daemon.
            sink.log(
                _task,
                super::super::Level::Info,
                &commands.argv().join(" \u{2423} "),
            );
            self.answer
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Ok(CommandsResult::default()))
        }

        async fn cancel(&self, task: &TaskInput) -> Result<()> {
            self.cancelled
                .lock()
                .unwrap()
                .push(crate::worker::commands::container_name(task));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Spy;
    use super::*;
    use crate::executor::RunState;
    use crate::worker::handlers::testing::{input, Recorder};
    use crate::worker::handlers::Level;

    fn spec() -> serde_json::Value {
        serde_json::json!({
            "image": "alpine:3.20",
            "commands": ["echo one", "echo two"],
        })
    }

    /// The spec becomes a [`TaskCommands`], and `failFast` puts `set -e` at the
    /// front of the joined script.
    #[tokio::test]
    async fn the_spec_becomes_one_shell_script_with_set_e_in_front() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy.clone());
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success);

        let cmds = spy.only();
        assert_eq!(cmds.container_image, "alpine:3.20");
        assert_eq!(
            cmds.argv(),
            vec!["/bin/sh", "-c", "set -e\necho one\necho two"],
            "`failFast` is Kestra's own default true, and without `set -e` a \
             script whose first line fails still exits 0 on its last"
        );
    }

    /// `failFast: false` takes it away, and nothing else changes.
    #[tokio::test]
    async fn fail_fast_false_drops_the_set_e_and_only_that() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy.clone());
        let mut s = spec();
        s["failFast"] = serde_json::json!(false);
        h.run(&input(TYPE_ID, s)).await.expect("no worker error");
        assert_eq!(
            spy.only().argv(),
            vec!["/bin/sh", "-c", "echo one\necho two"]
        );
    }

    /// A non-zero exit is a FAILED task that still carries its outputs — the
    /// artifact's `RunnableTaskException(e, ScriptOutput)`.
    #[tokio::test]
    async fn a_non_zero_exit_fails_the_task_and_keeps_its_outputs() {
        let spy = Spy::ok(CommandsResult {
            exit_code: 3,
            stdout_line_count: 2,
            stderr_line_count: 1,
            vars: serde_json::json!({"n": 7})
                .as_object()
                .expect("an object")
                .clone(),
            ..CommandsResult::default()
        });
        let h = Container::with_runner(spy);
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");

        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            out.message.as_deref(),
            Some("egret.container: the command exited with code 3")
        );
        let outputs = out.outputs.expect("a failed command still carries outputs");
        assert_eq!(outputs["exitCode"], serde_json::json!(3));
        assert_eq!(outputs["vars"], serde_json::json!({"n": 7}));
        assert_eq!(outputs["stdOutLineCount"], serde_json::json!(2));
        assert_eq!(outputs["stdErrLineCount"], serde_json::json!(1));
    }

    /// A daemon that is not there is a WORKER failure; anything else fails the
    /// task. The two are the same call with a different error, so they are
    /// asserted together.
    #[tokio::test]
    async fn an_unreachable_daemon_aborts_the_tick_and_a_bad_image_fails_the_task() {
        let spy = Spy::new(Err(Error::Internal(anyhow::anyhow!(
            "Cannot connect to the Docker daemon"
        ))));
        let h = Container::with_runner(spy);
        let err = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect_err("egret's own outage must not fail the author's flow");
        assert!(
            err.to_string().contains("not the author's flow"),
            "got {err}"
        );

        let spy = Spy::new(Err(Error::InvalidInput(
            "docker create: Unable to find image 'nope:1' locally".into(),
        )));
        let h = Container::with_runner(spy);
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .expect("a message")
                .contains("Unable to find image"),
            "the engine's own words are the useful half in an authoring loop"
        );
    }

    /// A spec that does not parse fails the task rather than the worker, and
    /// an unknown field is refused rather than ignored.
    #[tokio::test]
    async fn a_misspelled_property_fails_the_task_and_says_so() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy.clone());
        let out = h
            .run(&input(
                TYPE_ID,
                serde_json::json!({"image": "alpine", "commands": ["true"], "outputfiles": ["a"]}),
            ))
            .await
            .expect("a bad spec is the author's flow, not a worker failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .expect("a message")
                .contains("outputfiles"),
            "got {:?}",
            out.message
        );
        assert!(
            spy.seen.lock().unwrap().is_empty(),
            "a spec that did not parse must not reach the runner"
        );
    }

    /// Every string property renders, and a template that cannot fails the
    /// task with the property's name in it.
    #[tokio::test]
    async fn the_properties_render_and_a_broken_one_names_itself() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy.clone());
        let cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "inputs": {"tag": "3.20", "who": "world"}
        }))
        .expect("an object");
        let task = TaskInput {
            render: cx,
            ..input(
                TYPE_ID,
                serde_json::json!({
                    "image": "alpine:{{ inputs.tag }}",
                    "commands": ["echo {{ inputs.who }}"],
                    "env": {"WHO": "{{ inputs.who }}"},
                }),
            )
        };
        h.run(&task).await.expect("no worker error");
        let cmds = spy.only();
        assert_eq!(cmds.container_image, "alpine:3.20");
        assert_eq!(cmds.commands, vec!["echo world"]);
        assert_eq!(cmds.env.get("WHO").map(String::as_str), Some("world"));

        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy.clone());
        let out = h
            .run(&input(
                TYPE_ID,
                serde_json::json!({"image": "alpine", "commands": ["echo {{ nope.missing }}"]}),
            ))
            .await
            .expect("an unrenderable template is a task failure, not a worker one");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .expect("a message")
                .contains("`commands`"),
            "got {:?} -- the author has to be told WHICH property",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// The log sink is wired: a container's stdout is most of what it
    /// produces, and a handler that streamed to nowhere would still pass every
    /// assertion about its outputs.
    #[tokio::test]
    async fn the_log_sink_receives_the_runners_lines() {
        let rec = Recorder::new();
        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy).with_sink(rec.clone());
        h.run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        let lines = rec.lines();
        assert_eq!(lines.len(), 1, "got {lines:?}");
        assert_eq!(lines[0].0, Level::Info);
        assert!(lines[0].1.contains("echo one"), "got {:?}", lines[0].1);
    }

    /// **`cancel` reaches the runner, addressed by the lease.**
    #[tokio::test]
    async fn cancel_asks_the_runner_to_stop_the_container_this_lease_owns() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Container::with_runner(spy.clone());
        let task = TaskInput {
            run_id: uuid::Uuid::from_u128(9),
            claimed_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid"),
            ..input(TYPE_ID, spec())
        };
        h.cancel(&task).await.expect("a container can be stopped");
        assert_eq!(
            spy.cancelled.lock().unwrap().as_slice(),
            ["egret-run-00000000000000000000000000000009-1700000000000000"],
            "the address carries the lease, not only the run"
        );
    }
}
