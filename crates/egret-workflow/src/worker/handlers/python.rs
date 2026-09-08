//! `egret.script.python` — run an inline Python script in a container.
//!
//! # This handler was called `egret.python` for one commit, and that name was
//! # already taken
//!
//! Fix round 1's Critical 2. `crate::spec::registry::TASK_TYPES` has held an
//! `egret.python` since W1 (commit `228173f9`) declaring a **required**
//! `RefField { field: "notebook", kind: RefKind::Notebook }`, and the programme
//! spec's §6.4 table says what it is for: *"`egret.python` | `KernelLauncher`;
//! cần port `egret-kernel-api` mới"* — **run a notebook through the kernel
//! launcher**, needing a port that does not exist yet. `src/models/flow.rs:417`
//! and `spec/validate.rs`'s fixtures both write `type: egret.python` with a
//! `notebook:` field, and `tests/common/mod.rs:199` has a `FakeRefs` arm for it.
//!
//! A container that runs an inline `script:` is a different task. The collision
//! was live in both directions and neither was a compile error:
//!
//! * a `script:` flow **fails publish**, because publish resolves the declared
//!   `notebook` reference and there is none;
//! * a `notebook:` flow that somehow published would **fail at run time**, on
//!   this module's `deny_unknown_fields`.
//!
//! **The existing entry is not bent to fit.** It is a shipped, spec-declared
//! meaning with fixtures behind it; this handler is the new thing, so this
//! handler is what renamed. `egret.script.python` follows the vocabulary that is
//! already there (`egret.dataapi.call`, `egret.dataset.refresh`,
//! `egret.report.send` all namespace before the verb) and reads as what it is —
//! a script task, in Python. Nothing outside `crates/egret-workflow` referenced
//! either name; measured with a repo-wide grep over `*.rs`, `*.ts`, `*.tsx`,
//! `*.md`, `*.yaml`, `*.json`.
//!
//! `TASK_TYPES` gains a `egret.script.python` row with `references: &[]` in the
//! same change, because publish refuses a type absent from that table and a
//! handler nothing can publish to is dead surface.
//! `handlers::container::tests::the_publish_table_and_these_handlers_agree`
//! pins all of it — including that this handler's id is **not** `egret.python`,
//! so the collision cannot come back quietly.
//!
//! # No oracle coverage either, and for the same measured reason
//!
//! Zero of the 345 corpus flows declare a script task type
//! ([`handlers::container`](super::container) carries the measurement). This
//! file's instruments are its own unit tests over a fake
//! [`TaskRunner`](crate::worker::commands::TaskRunner) and
//! `tests/container_docker_live.rs` against a real daemon.
//!
//! # What it corresponds to, measured on the pinned image
//!
//! `io.kestra.plugin.scripts.python.Script`, from
//! `/app/plugins/io_kestra_plugin__plugin-script-python__1_9_8.jar`. What that
//! class does, read with `javap -p -c -constants`:
//!
//! * writes the author's `script:` to a file whose suffix is the literal
//!   `".py"`, then runs it;
//! * the interpreter binary is the literal `"python"`
//!   (`PythonEnvironmentManager`);
//! * `AbstractPythonExecScript`'s container image default is the literal
//!   `"python:3.13-slim"`;
//! * it sets `PYTHONUNBUFFERED=true`, `PIP_ROOT_USER_ACTION=ignore` and
//!   `PIP_DISABLE_PIP_VERSION_CHECK=1` (and a `PYTHONPATH`, which depends on a
//!   virtualenv this handler does not build).
//!
//! [`ENV_DEFAULTS`] is the third of those, verbatim, and [`DEFAULT_IMAGE`] and
//! [`SCRIPT_FILE`] the others.
//!
//! # Two measured behaviours this handler deliberately does NOT copy
//!
//! * **The `true = True` preamble.** `Script.class` prepends the literal
//!   `"true = True\nfalse = False\nnull = None\n"` to the script, so that a
//!   `{{ … }}` rendering a JSON boolean into Python source still parses. egret
//!   does not, because it shifts every line number in every traceback by three
//!   and the author never wrote those lines — a `NameError` on line 1 reported
//!   as line 4 is a debugging cost paid on every run to rescue a template that
//!   should have used `{{ … | lower }}`. The divergence is stated here rather
//!   than discovered from a stack trace.
//! * **The dependency resolver.** Plugin 1.9.8 carries a whole
//!   `PythonEnvironmentManager` that installs `uv` (it holds a
//!   `uvInstallerVersion` and a `uvInstallerSha256`) and builds a virtualenv
//!   per run. egret runs the image's own `python`; an author who needs packages
//!   names an image that has them, or installs them in `beforeCommands`. Porting
//!   the resolver means downloading and checksumming a binary inside a task,
//!   which is a supply-chain decision and not a handler's.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egret_core::Result;
use serde::Deserialize;

use super::super::commands::{DockerRunner, TaskCommands, TaskRunner};
use super::super::{Outcome, TaskHandler, TaskInput};
use super::container::{mapped, outcome_for, render_all, render_values, EXIT_ON_ERROR};
use super::{default_sink, LogSink};
use crate::runners::render_str;

/// The `type:` a flow writes.
///
/// **Not `egret.python`** — that name is the notebook-kernel task, declared in
/// `spec::registry::TASK_TYPES` since W1. See this module's doc.
pub const TYPE_ID: &str = "egret.script.python";

/// `AbstractPythonExecScript`'s own container image default, verbatim.
pub const DEFAULT_IMAGE: &str = "python:3.13-slim";

/// Where the author's `script:` is written, relative to
/// [`WORKING_DIR`](crate::worker::commands::WORKING_DIR).
///
/// Kestra generates a random name with the `".py"` suffix; egret fixes the
/// name, because a random one would appear in every traceback the author reads
/// and in none of the two runs the same. The suffix is what makes `python` treat
/// it as a module for the purposes of an import beside it.
pub const SCRIPT_FILE: &str = "main.py";

/// The environment `Script.class` sets, verbatim and in its own spellings.
///
/// `PYTHONUNBUFFERED` is the load-bearing one: without it CPython buffers
/// stdout when it is a pipe, which is exactly what
/// [`DockerRunner`](crate::worker::commands::DockerRunner) gives it, and every
/// `print()` — including a `::{"outputs": …}::` marker — would arrive in one
/// burst at exit instead of streaming.
pub const ENV_DEFAULTS: [(&str, &str); 3] = [
    ("PYTHONUNBUFFERED", "true"),
    ("PIP_ROOT_USER_ACTION", "ignore"),
    ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
];

/// `egret.script.python`'s own properties.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    /// The program. Written to [`SCRIPT_FILE`] and run by `python`.
    script: String,
    /// The image. [`DEFAULT_IMAGE`] when the author names none.
    #[serde(default)]
    image: Option<String>,
    /// Shell lines run before `python` — `pip install`, a `mkdir`.
    #[serde(default, rename = "beforeCommands")]
    before_commands: Vec<String>,
    /// Author env, applied **over** [`ENV_DEFAULTS`].
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default, rename = "outputFiles")]
    output_files: Vec<String>,
    #[serde(default = "yes", rename = "failFast")]
    fail_fast: bool,
}

fn yes() -> bool {
    true
}

/// The Python names Kestra's `Script` predefines and egret deliberately does
/// not.
///
/// `Script.class` prepends the literal `"true = True\nfalse = False\nnull = None\n"`
/// so a `{{ … }}` rendering a JSON boolean into Python source still parses.
/// [This module's doc](self) says why egret does not copy it: three invisible
/// lines shift every traceback the author reads.
///
/// **Review Minor 7 is that the divergence lived only in rustdoc**, so an author
/// porting a Kestra script got a `NameError` with nothing to connect it to. The
/// repo has no author-facing workflow task reference to put it in — measured, no
/// document in `docs/` mentions `outputFiles` or `beforeCommands` at all — so it
/// is connected at the place the author actually meets it instead:
/// [`NameHintSink`] watches the script's own stderr for exactly these names and
/// [`Python::run`] appends the explanation to the failure message.
const KESTRA_PREDEFINED: [&str; 3] = ["true", "false", "null"];

/// A [`LogSink`] that passes every line through and remembers whether Python
/// complained about one of [`KESTRA_PREDEFINED`].
///
/// A decorator rather than a field on `CommandsResult`, because this is one
/// handler's authoring hint and not part of what a command produces — the
/// runner has no business knowing that `null` means something to Python.
#[derive(Debug)]
struct NameHintSink {
    inner: Arc<dyn LogSink>,
    seen: Mutex<Option<String>>,
}

impl NameHintSink {
    fn new(inner: Arc<dyn LogSink>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            seen: Mutex::new(None),
        })
    }

    /// The name CPython said was undefined, if it was one of Kestra's three.
    ///
    /// Matched on CPython's own wording — `NameError: name 'true' is not
    /// defined` — rather than on the bare word, so an ordinary log line
    /// mentioning `null` cannot produce the hint.
    fn undefined_name(line: &str) -> Option<&'static str> {
        let rest = line.split("NameError: name '").nth(1)?;
        let name = rest.split('\'').next()?;
        KESTRA_PREDEFINED.iter().copied().find(|k| *k == name)
    }

    fn hint(&self) -> Option<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl LogSink for NameHintSink {
    fn log(&self, task: &TaskInput, level: super::Level, message: &str) {
        if let Some(name) = Self::undefined_name(message) {
            let mut seen = self.seen.lock().unwrap();
            if seen.is_none() {
                *seen = Some(format!(
                    "`{name}` is not a Python name. Kestra's `python.Script` \
                     silently prepends `true = True`, `false = False` and \
                     `null = None` to every script; `{TYPE_ID}` deliberately does \
                     not, because those three invisible lines shift every line \
                     number in every traceback. If a `{{{{ … }}}}` rendered a JSON \
                     boolean or null into your source, render it as Python \
                     (`{{{{ … | lower }}}}` is not enough for `null`) or define \
                     the names yourself"
                ));
            }
        }
        self.inner.log(task, level, message);
    }
}

/// `egret.script.python`, over a [`TaskRunner`].
pub struct Python {
    runner: Arc<dyn TaskRunner>,
    sink: Arc<dyn LogSink>,
}

impl std::fmt::Debug for Python {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Python")
            .field("runner", &self.runner)
            .finish_non_exhaustive()
    }
}

impl Default for Python {
    fn default() -> Self {
        Self::new()
    }
}

impl Python {
    /// The shipping handler: a [`DockerRunner`] and `tracing`.
    pub fn new() -> Self {
        Self::with_runner(Arc::new(DockerRunner::new()))
    }

    pub fn with_runner(runner: Arc<dyn TaskRunner>) -> Self {
        Self {
            runner,
            sink: default_sink(),
        }
    }

    pub fn with_sink(mut self, sink: Arc<dyn LogSink>) -> Self {
        self.sink = sink;
        self
    }
}

#[async_trait::async_trait]
impl TaskHandler for Python {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };

        let script = match render_str(&spec.script, &task.render) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Outcome::failed_with(format!(
                    "{TYPE_ID}: `script` did not render: {e}"
                )))
            }
        };
        let image = match spec.image.as_deref() {
            None => DEFAULT_IMAGE.to_string(),
            Some(raw) => match render_str(raw, &task.render) {
                Ok(v) => v,
                Err(e) => {
                    return Ok(Outcome::failed_with(format!(
                        "{TYPE_ID}: `image` did not render: {e}"
                    )))
                }
            },
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
            before.insert(0, EXIT_ON_ERROR.to_string());
        }
        let author_env = match render_values(TYPE_ID, "env", &spec.env, &task.render) {
            Ok(v) => v,
            Err(o) => return Ok(o),
        };
        let output_files =
            match render_all(TYPE_ID, "outputFiles", &spec.output_files, &task.render) {
                Ok(v) => v,
                Err(o) => return Ok(o),
            };

        // The defaults first, the author's over them: a flow that sets
        // `PYTHONUNBUFFERED: "false"` on purpose gets it, and one that sets
        // nothing gets the streaming this handler's log wiring depends on.
        let mut env: BTreeMap<String, String> = ENV_DEFAULTS
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        env.extend(author_env);

        let cmds = TaskCommands {
            container_image: image,
            interpreter: Vec::new(),
            before_commands: before,
            // `python main.py`, where the file is written into the working
            // directory the container is created with — so the command is the
            // same whatever the image's own WORKDIR is.
            commands: vec![format!("python {SCRIPT_FILE}")],
            env,
            input_files: BTreeMap::from([(SCRIPT_FILE.to_string(), script)]),
            output_files,
        };

        // The sink is wrapped for the run so an authoring mistake Kestra hides
        // is connected to its cause here — see [`NameHintSink`] and review
        // Minor 7.
        let watcher = NameHintSink::new(self.sink.clone());
        match self.runner.run(task, &cmds, watcher.as_ref()).await {
            Ok(result) => {
                let mut outcome = outcome_for(TYPE_ID, &result);
                if let (Some(hint), Some(message)) = (watcher.hint(), outcome.message.as_mut()) {
                    message.push_str(". ");
                    message.push_str(&hint);
                }
                Ok(outcome)
            }
            Err(e) => mapped(TYPE_ID, e),
        }
    }

    /// The same real cancellation
    /// [`Container`](super::container::Container)'s carries, for the same
    /// reason and through the same lease-addressed name — this handler holds a
    /// container too.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        self.runner.cancel(task).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::container::testing::Spy;
    use super::*;
    use crate::executor::RunState;
    use crate::worker::commands::CommandsResult;
    use crate::worker::handlers::testing::input;

    fn spec() -> serde_json::Value {
        serde_json::json!({"script": "print('hi')"})
    }

    /// The script becomes a file, the file is what `python` is given, and the
    /// image is the artifact's own default.
    #[tokio::test]
    async fn the_script_is_written_to_a_file_and_run_by_python() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Python::with_runner(spy.clone());
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success);

        let cmds = spy.only();
        assert_eq!(cmds.container_image, DEFAULT_IMAGE);
        assert_eq!(
            cmds.input_files.get(SCRIPT_FILE).map(String::as_str),
            Some("print('hi')"),
            "the author's program goes in verbatim -- Kestra prepends \
             `true = True\\nfalse = False\\nnull = None\\n` and this handler \
             deliberately does not, so line 1 of a traceback is line 1 of the flow"
        );
        assert_eq!(
            cmds.argv(),
            vec!["/bin/sh", "-c", "set -e\npython main.py"],
            "the interpreter binary is the literal `python`, the same one \
             `PythonEnvironmentManager` uses"
        );
    }

    /// The measured environment is set, and the author's own entries win.
    #[tokio::test]
    async fn the_measured_python_environment_is_set_and_the_author_can_override_it() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Python::with_runner(spy.clone());
        let mut s = spec();
        s["env"] = serde_json::json!({"PYTHONUNBUFFERED": "false", "MINE": "1"});
        h.run(&input(TYPE_ID, s)).await.expect("no worker error");

        let env = spy.only().env;
        assert_eq!(
            env.get("PIP_ROOT_USER_ACTION").map(String::as_str),
            Some("ignore")
        );
        assert_eq!(
            env.get("PIP_DISABLE_PIP_VERSION_CHECK").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            env.get("PYTHONUNBUFFERED").map(String::as_str),
            Some("false"),
            "the author's own value wins over the default"
        );
        assert_eq!(env.get("MINE").map(String::as_str), Some("1"));

        // And with no `env:` at all, all three defaults are there -- a test
        // that only checked the override would pass against a handler that set
        // nothing.
        let spy = Spy::ok(CommandsResult::default());
        let h = Python::with_runner(spy.clone());
        h.run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        let env = spy.only().env;
        for (k, v) in ENV_DEFAULTS {
            assert_eq!(env.get(k).map(String::as_str), Some(v), "{k} is not set");
        }
    }

    /// A named image replaces the default, rendered.
    #[tokio::test]
    async fn a_named_image_replaces_the_default_and_renders() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Python::with_runner(spy.clone());
        let cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "inputs": {"tag": "3.12-slim"}
        }))
        .expect("an object");
        let task = TaskInput {
            render: cx,
            ..input(
                TYPE_ID,
                serde_json::json!({"script": "pass", "image": "python:{{ inputs.tag }}"}),
            )
        };
        h.run(&task).await.expect("no worker error");
        assert_eq!(spy.only().container_image, "python:3.12-slim");
    }

    /// A non-zero exit fails the task, carrying the marker outputs the script
    /// printed before it died.
    #[tokio::test]
    async fn a_failing_script_fails_the_task_and_keeps_what_it_printed() {
        let spy = Spy::ok(CommandsResult {
            exit_code: 1,
            vars: serde_json::json!({"before": "the crash"})
                .as_object()
                .expect("an object")
                .clone(),
            ..CommandsResult::default()
        });
        let h = Python::with_runner(spy);
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            out.message.as_deref(),
            Some("egret.script.python: the command exited with code 1")
        );
        assert_eq!(
            out.outputs.expect("outputs survive a failure")["vars"],
            serde_json::json!({"before": "the crash"})
        );
    }

    /// A spec with no `script:` is the author's flow, not a worker failure.
    #[tokio::test]
    async fn a_spec_without_a_script_fails_the_task() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Python::with_runner(spy.clone());
        let out = h
            .run(&input(
                TYPE_ID,
                serde_json::json!({"image": "python:3.13-slim"}),
            ))
            .await
            .expect("a bad spec is a task failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .expect("a message")
                .contains("script"),
            "got {:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// **A `NameError` on one of Kestra's three predefined names is explained
    /// where the author meets it.**
    ///
    /// Review Minor 7. Both directions: the hint fires on CPython's own wording
    /// and nothing else, so an ordinary line mentioning `null` cannot produce
    /// it.
    #[tokio::test]
    async fn a_kestra_predefined_name_error_is_connected_to_its_cause() {
        use crate::worker::commands::{CommandsResult, TaskCommands, TaskRunner};

        /// A runner that prints the traceback a ported Kestra script produces,
        /// then fails the way the container would.
        #[derive(Debug)]
        struct Traceback(&'static str);

        #[async_trait::async_trait]
        impl TaskRunner for Traceback {
            async fn run(
                &self,
                task: &TaskInput,
                _c: &TaskCommands,
                sink: &dyn LogSink,
            ) -> Result<CommandsResult> {
                sink.log(task, super::super::Level::Warn, self.0);
                Ok(CommandsResult {
                    exit_code: 1,
                    stderr_line_count: 1,
                    ..CommandsResult::default()
                })
            }
            async fn cancel(&self, _task: &TaskInput) -> Result<()> {
                Ok(())
            }
        }

        let h = Python::with_runner(Arc::new(Traceback("NameError: name 'null' is not defined")));
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        let msg = out.message.expect("a failed run carries a message");
        assert!(
            msg.starts_with("egret.script.python: the command exited with code 1"),
            "the failure itself still leads: {msg}"
        );
        assert!(
            msg.contains("Kestra's `python.Script` silently prepends"),
            "and the divergence is explained where the author meets it: {msg}"
        );

        // A line that merely mentions the word must not trigger it.
        let h = Python::with_runner(Arc::new(Traceback("loaded null values from the source")));
        let out = h
            .run(&input(TYPE_ID, spec()))
            .await
            .expect("no worker error");
        let msg = out.message.expect("a message");
        assert!(
            !msg.contains("silently prepends"),
            "the hint keys on CPython's own NameError wording, not on the word: {msg}"
        );
    }

    /// `cancel` reaches the runner here too — a `Python` holds a container
    /// exactly as a `Container` does, and inheriting `Ok(())` would be the
    /// silent lie `TaskHandler::cancel` has no default body to prevent.
    #[tokio::test]
    async fn cancel_asks_the_runner_to_stop_the_container() {
        let spy = Spy::ok(CommandsResult::default());
        let h = Python::with_runner(spy.clone());
        let task = TaskInput {
            run_id: uuid::Uuid::from_u128(4),
            claimed_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid"),
            ..input(TYPE_ID, spec())
        };
        h.cancel(&task).await.expect("a container can be stopped");
        assert_eq!(
            spy.cancelled.lock().unwrap().as_slice(),
            ["egret-run-00000000000000000000000000000004-1700000000000000"]
        );
    }
}
