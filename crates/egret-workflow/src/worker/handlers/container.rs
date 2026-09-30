//! The outcome and rendering helpers every script task shares, and the
//! `egret.container` alias's name.
//!
//! # W5a Task 5: the handler that lived here is `handlers::script::Script`
//!
//! W4 shipped `egret.container` as a handler of its own, with its own
//! `deny_unknown_fields` property set (`image`, `commands`, ...). W5a made it
//! an **alias** of `io.kestra.plugin.scripts.shell.Commands`
//! (`spec::registry::TASK_TYPES`, W5a spec §3), parsed by `scripts::parse` --
//! which still accepts W4's `image` as a synonym of `containerImage` on the
//! aliases, so a flow saved before W5a keeps working -- and served by the one
//! script handler, [`super::script`], which gates it at run time like every
//! other script task. What stayed here is what that handler shares with the
//! W4 contract: [`outcome_for`] (a failed command keeps its outputs,
//! `RunnableTaskException(e, ScriptOutput)`), [`mapped`] (egret's outage vs
//! the author's error), and the renderers.
//!
//! # No oracle coverage, measured
//!
//! `oracle-exec/corpus/` holds **345** flows and **zero** of them declare a
//! script, container, shell or command task type (`grep -rhoE 'type:\s*\S+'
//! corpus | sort -u | grep -iE 'script|container|shell|command|docker|bash|python|node'`
//! matches nothing). The execution ratchet cannot move for script tasks in
//! either direction; the instruments are the unit tests over a fake
//! [`TaskRunner`](crate::worker::commands::TaskRunner) ([`testing::Spy`]) and
//! `tests/container_docker_live.rs` / `tests/k8s_runner_live.rs`.

use std::collections::BTreeMap;

use egret_core::{Error, Result};

use super::super::commands::CommandsResult;
use super::super::Outcome;
use crate::runners::render_str;

/// W4's `type:`, now an alias of `io.kestra.plugin.scripts.shell.Commands`
/// ([`crate::scripts::ALIAS_CONTAINER`]).
pub const TYPE_ID: &str = crate::scripts::ALIAS_CONTAINER;

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
    // **[`CommandsResult::failure`] first**, and it carries the outputs too:
    // fix round 1's Important 1 was that an output-file problem discarded the
    // exit code and the marker vars, contradicting fact 7 which this very file
    // quotes as its principle. `RunnableTaskException(e, ScriptOutput)` is an
    // exception CARRYING an output; so is this.
    if let Some(why) = &result.failure {
        return Outcome {
            state: crate::executor::RunState::Failed,
            outputs: Some(outputs),
            message: Some(format!("{type_id}: {why}")),
        };
    }
    if result.exit_code == 0 {
        return Outcome::succeeded_with(outputs);
    }
    Outcome {
        state: crate::executor::RunState::Failed,
        outputs: Some(outputs),
        message: Some(format!("{type_id}: {}", command_failed(result.exit_code))),
    }
}

/// Kestra's own words for a non-zero exit, measured (`w5a-scripts.json`
/// `fail_fast.failure_log`: ERROR `Command failed with exit code <n>`). The
/// script handler logs this line at ERROR and the outcome's message carries
/// it after the type.
pub(super) fn command_failed(exit_code: i32) -> String {
    format!("Command failed with exit code {exit_code}")
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

#[cfg(test)]
pub(super) mod testing {
    //! A [`TaskRunner`] that answers however the test built it, and records
    //! what it was asked.
    //!
    //! Used by `handlers::script`'s tests.

    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::worker::commands::{TaskCommands, TaskRunner};
    use crate::worker::TaskInput;

    #[derive(Debug, Default)]
    pub struct Spy {
        pub seen: Mutex<Vec<TaskCommands>>,
        /// The `taskRunner:` spec each run was given.
        pub specs: Mutex<Vec<crate::scripts::RunnerSpec>>,
        pub cancelled: Mutex<Vec<String>>,
        /// The `grace` every `cancel` was given.
        pub cancel_graces: Mutex<Vec<std::time::Duration>>,
        /// `(container name, delete)` for every `release`.
        pub released: Mutex<Vec<(String, bool)>>,
        /// What `adopt` answers: `Some` = a started container exists for the
        /// attempt and finished with this; `None` (the default) = nothing.
        pub adoptable: Mutex<Option<CommandsResult>>,
        /// How many times `adopt` was asked.
        pub adopt_calls: Mutex<usize>,
        answer: Mutex<Option<Result<CommandsResult>>>,
        /// `(is_stderr, line)` to stream, in order. `None` streams the one
        /// stdout line the argv would print.
        streams: Mutex<Option<Vec<(bool, String)>>>,
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
        /// Stream these `(is_stderr, line)` pairs, through the same
        /// `commands::stream_line` the real runner uses.
        pub fn streaming(self: Arc<Self>, lines: Vec<(bool, String)>) -> Arc<Self> {
            *self.streams.lock().unwrap() = Some(lines);
            self
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
            task: &TaskInput,
            commands: &TaskCommands,
            spec: &crate::scripts::RunnerSpec,
            observer: &dyn crate::worker::commands::LineObserver,
        ) -> Result<CommandsResult> {
            self.seen.lock().unwrap().push(commands.clone());
            self.specs.lock().unwrap().push(spec.clone());
            // A real runner streams; this one streams what the test gave it,
            // or the one line the argv would print, through the real runner's
            // own `stream_line` — so the levels, the markers and the
            // destination (`task.log`) are the shipping ones, without a daemon.
            let lines = self
                .streams
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| vec![(false, commands.argv().join(" \u{2423} "))]);
            let mut scratch = CommandsResult::default();
            let mut state = crate::worker::commands::StreamState::default();
            for (is_err, line) in lines {
                crate::worker::commands::stream_line(
                    task,
                    observer,
                    &mut scratch,
                    &mut state,
                    usize::MAX,
                    &line,
                    is_err,
                    commands.host_protocol,
                )
                .await;
            }
            self.answer
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Ok(CommandsResult::default()))
        }

        async fn cancel(&self, task: &TaskInput, grace: std::time::Duration) -> Result<()> {
            self.cancel_graces.lock().unwrap().push(grace);
            self.cancelled
                .lock()
                .unwrap()
                .push(crate::worker::commands::container_name(task));
            Ok(())
        }

        async fn adopt(
            &self,
            task: &TaskInput,
            commands: &TaskCommands,
            _spec: &crate::scripts::RunnerSpec,
            observer: &dyn crate::worker::commands::LineObserver,
        ) -> Result<Option<CommandsResult>> {
            *self.adopt_calls.lock().unwrap() += 1;
            let adopted = self.adoptable.lock().unwrap().clone();
            // A real re-attach reads the existing container's lines into
            // `task.log`; this one streams what the test set with
            // `streaming`, and nothing when it set nothing.
            let lines = self.streams.lock().unwrap().clone();
            if let (Some(_), Some(lines)) = (&adopted, lines) {
                let mut scratch = CommandsResult::default();
                let mut state = crate::worker::commands::StreamState::default();
                for (is_err, line) in lines {
                    crate::worker::commands::stream_line(
                        task,
                        observer,
                        &mut scratch,
                        &mut state,
                        usize::MAX,
                        &line,
                        is_err,
                        commands.host_protocol,
                    )
                    .await;
                }
            }
            Ok(adopted)
        }

        async fn release(&self, task: &TaskInput, spec: &crate::scripts::RunnerSpec) -> Result<()> {
            self.released
                .lock()
                .unwrap()
                .push((crate::worker::commands::container_name(task), spec.delete));
            Ok(())
        }
    }
}
