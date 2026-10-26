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
//! `oracle-exec/corpus/` holds **351** flows and **zero** of them declare a
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
/// `ScriptOutput`'s shape: `vars`, `exitCode`, `outputFiles`,
/// `stdOutLineCount`, `stdErrLineCount` (`taskRunner` is left out: nothing
/// in egret reads it). The keys are Kestra's own spellings so an author who
/// has read its docs is not surprised.
///
/// **`outputFiles` maps a file's relative path to its `kestra://` URI**, as
/// Kestra's does (`w9a-storage.json` `egret_expected.script_output_files_map`)
/// -- W9a spec §6.2, a breaking change from W5a, where the value was the
/// file's CONTENTS. Content is read with `{{ read(outputs.t.outputFiles['a.txt']) }}`.
pub(super) fn outputs_for(result: &CommandsResult) -> serde_json::Value {
    serde_json::json!({
        "exitCode": result.exit_code,
        "vars": serde_json::Value::Object(result.vars.clone()),
        "outputFiles": result
            .output_files
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.uri.clone())))
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
///
/// **The collected files ride along whatever the state** (W9a Task 6): the
/// outputs name them, so they are indexed with the outcome. Then, with the
/// run's `task_prefix`, [`check_outputs_collected`] runs over the final
/// outputs -- a `::{"outputs": …}::` marker naming a URI under this run's
/// prefix that was never collected fails the task (spec §5: no output points
/// at nothing). The script handler passes the run's prefix whether or not it
/// declared an output (fix round 1, I2); `None` is a run with no prefix at
/// all. Two outputs stored at one URI also fail the task, and the URI rides
/// once ([`distinct_files`], fix round 1, I1).
///
/// [`check_outputs_collected`]: crate::storage::collect::check_outputs_collected
pub(super) fn outcome_for(
    type_id: &str,
    result: &CommandsResult,
    task_prefix: Option<&str>,
) -> Outcome {
    let outputs = outputs_for(result);
    let (files, shared) = distinct_files(result);
    // **[`CommandsResult::failure`] first**, and it carries the outputs too:
    // fix round 1's Important 1 was that an output-file problem discarded the
    // exit code and the marker vars, contradicting fact 7 which this very file
    // quotes as its principle. `RunnableTaskException(e, ScriptOutput)` is an
    // exception CARRYING an output; so is this.
    let failure = result.failure.clone().or(shared).or_else(|| {
        task_prefix.and_then(|prefix| {
            crate::storage::collect::check_outputs_collected(&outputs, prefix, &files).err()
        })
    });
    if let Some(why) = failure {
        return Outcome {
            state: crate::executor::RunState::Failed,
            outputs: Some(outputs),
            message: Some(format!("{type_id}: {why}")),
            files,
        };
    }
    if result.exit_code == 0 {
        return Outcome {
            files,
            ..Outcome::succeeded_with(outputs)
        };
    }
    Outcome {
        state: crate::executor::RunState::Failed,
        outputs: Some(outputs),
        message: Some(format!("{type_id}: {}", command_failed(result.exit_code))),
        files,
    }
}

/// The collected files, each URI ONCE, and -- when two outputs share one --
/// the author's failure naming them (W9a Task 6 fix round 1, I1).
///
/// The commit indexes `Outcome::files` with one `INSERT … ON CONFLICT DO
/// UPDATE`, which cannot affect one row twice: a URI listed twice would be an
/// `Internal` error at every commit, the lease lapsing and the run re-claimed
/// and re-collected for ever with no attempt spent. The runners refuse the
/// one way an author can make it (`collect_directory`); this is the backstop
/// for any runner, and it keeps the commit possible whatever reached it.
pub(super) fn distinct_files(
    result: &CommandsResult,
) -> (Vec<crate::storage::StoredFile>, Option<String>) {
    let mut by_uri: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    let mut files = Vec::new();
    for (rel, f) in &result.output_files {
        let keys = by_uri.entry(f.uri.as_str()).or_default();
        if keys.is_empty() {
            files.push(f.clone());
        }
        keys.push(rel.as_str());
    }
    let shared = by_uri
        .iter()
        .find(|(_, keys)| keys.len() > 1)
        .map(|(uri, keys)| {
            format!(
                "the outputs {} are all stored at {uri}: two outputs cannot share one file",
                keys.iter()
                    .map(|k| format!("{k:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
    (files, shared)
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
///
/// `Err` is the failed task's whole message; the caller makes the
/// `Outcome` (an `Outcome` as the error made every `Result` here as large as
/// it, clippy `result_large_err`).
pub(super) fn render_all(
    type_id: &str,
    what: &str,
    items: &[String],
    cx: &crate::runners::RenderContext,
) -> std::result::Result<Vec<String>, String> {
    items
        .iter()
        .map(|s| render_str(s, cx).map_err(|e| format!("{type_id}: `{what}` did not render: {e}")))
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
) -> std::result::Result<BTreeMap<String, String>, String> {
    items
        .iter()
        .map(|(k, v)| {
            render_str(v, cx)
                .map(|v| (k.clone(), v))
                .map_err(|e| format!("{type_id}: `{what}` did not render: {e}"))
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
        /// The `spec.delete` every `cancel` was given (debt D1).
        pub cancel_deletes: Mutex<Vec<bool>>,
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
        /// When set, `run` writes the input archive from this store into a
        /// sink first, through the shipping `write_tar` and `staging_error`
        /// -- what a real runner does before it creates anything that runs.
        stage_from: Mutex<Option<Arc<crate::storage::FileStore>>>,
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
        /// Stage the inputs from `files` as a real runner does (W9a Task 5).
        pub fn staging(self: Arc<Self>, files: Arc<crate::storage::FileStore>) -> Arc<Self> {
            *self.stage_from.lock().unwrap() = Some(files);
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
            let stage_from = self.stage_from.lock().unwrap().clone();
            if let Some(files) = stage_from {
                let sources = commands.tar_sources()?;
                crate::storage::transfer::write_tar(&mut tokio::io::sink(), &sources, Some(&files))
                    .await
                    .map_err(crate::worker::commands::staging_error)?;
            }
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

        async fn cancel(
            &self,
            task: &TaskInput,
            grace: std::time::Duration,
            spec: &crate::scripts::RunnerSpec,
        ) -> Result<()> {
            self.cancel_graces.lock().unwrap().push(grace);
            self.cancel_deletes.lock().unwrap().push(spec.delete);
            self.cancelled
                .lock()
                .unwrap()
                .push(crate::worker::commands::container_name(task));
            Ok(())
        }

        async fn adoptable(&self, _task: &TaskInput) -> Result<bool> {
            Ok(self.adoptable.lock().unwrap().is_some())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoredFile;

    const PREFIX: &str = "kestra:///w9a/storage/out-filescase/executions/00000000-0000-0000-0000-0000000000e1/tasks/shell-task/00000000-0000-0000-0000-000000000041/";

    fn result_with(files: &[(&str, &str)]) -> CommandsResult {
        CommandsResult {
            output_files: files
                .iter()
                .map(|(rel, name)| {
                    (
                        rel.to_string(),
                        StoredFile {
                            uri: format!("{PREFIX}{name}"),
                            size: 1,
                        },
                    )
                })
                .collect(),
            ..CommandsResult::default()
        }
    }

    /// What follows the measured task run id in one of Kestra's URIs.
    fn tail_of(v: &serde_json::Value) -> String {
        v.as_str()
            .unwrap()
            .rsplit_once("/6ngIlvhFCotzCT8Em70NT4/")
            .unwrap()
            .1
            .to_string()
    }

    /// `w9a-storage.json` `script_output_files_map`: `outputFiles` maps each
    /// file's relative path to its URI -- the declared ones under Kestra's
    /// `{hash}-{name}` and the output directory's verbatim -- and nothing
    /// else.
    #[test]
    fn output_files_are_uris_keyed_by_relative_path() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/w9a-storage.json");
        let fx: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let want = fx["egret_expected"]["script_output_files_map"]["outputs"]["outputFiles"]
            .as_object()
            .expect("the measured map")
            .clone();
        assert_eq!(want.len(), 6, "a zero beside a number that can disagree");
        // Kestra's map, with egret's execution and run ids under the same
        // tails: the tail of each measured URI after its task run id.
        let tails: Vec<(String, String)> =
            want.iter().map(|(k, v)| (k.clone(), tail_of(v))).collect();
        let result = result_with(
            &tails
                .iter()
                .map(|(k, t)| (k.as_str(), t.as_str()))
                .collect::<Vec<_>>(),
        );
        let outputs = outputs_for(&result);
        let got = outputs["outputFiles"].as_object().unwrap();
        assert_eq!(got.len(), want.len());
        for (k, v) in &want {
            let tail = tail_of(v);
            assert_eq!(
                got[k],
                serde_json::json!(format!("{PREFIX}{tail}")),
                "{k}: a URI, never the contents"
            );
        }
        let mut keys: Vec<&String> = outputs.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "exitCode",
                "outputFiles",
                "stdErrLineCount",
                "stdOutLineCount",
                "vars"
            ]
        );
        let o = outcome_for("t", &result, Some(PREFIX));
        assert_eq!(o.state, crate::executor::RunState::Success, "{o:?}");
        assert_eq!(
            o.files.len(),
            6,
            "every collected file rides with the outcome"
        );
    }

    /// A marker naming a URI under this run's prefix that was not collected
    /// fails the task, keeping the outputs and the files; without a prefix
    /// (a handler with no storage) there is nothing to check.
    #[test]
    fn a_marker_pointing_at_an_uncollected_file_fails_the_task() {
        let mut result = result_with(&[("a.txt", "x-a.txt")]);
        result.vars.insert(
            "fake".into(),
            serde_json::json!(format!("{PREFIX}never.txt")),
        );
        let o = outcome_for("t", &result, Some(PREFIX));
        assert_eq!(o.state, crate::executor::RunState::Failed);
        assert!(o.message.as_deref().unwrap().contains("never.txt"), "{o:?}");
        assert_eq!(o.files.len(), 1);
        assert!(o.outputs.is_some());
        // Fix round 1, I2: `None` is a run with no prefix at all (no flow, no
        // storage) -- nothing under a prefix can exist to be checked. The
        // script handler passes the run's prefix whether or not it declared
        // an output (`script::tests::a_ghost_uri_under_the_runs_prefix_fails_a_task_that_declares_no_output`).
        let o = outcome_for("t", &result, None);
        assert_eq!(o.state, crate::executor::RunState::Success);
    }

    /// **Fix round 1, I1: two outputs naming ONE URI fail the task, and the
    /// URI rides with the outcome once.** Whatever runner produced them, the
    /// commit's single `INSERT … ON CONFLICT DO UPDATE` cannot affect one row
    /// twice: that would be an `Internal` error at every commit, the lease
    /// lapsing and the run re-claimed for ever with no attempt spent. Here it
    /// is the author's failed task, naming the URI and both outputs.
    #[test]
    fn two_outputs_naming_one_uri_fail_the_task_and_carry_the_uri_once() {
        let result = result_with(&[("a.txt", "h-a.txt"), ("h-a.txt", "h-a.txt"), ("b", "b")]);
        let o = outcome_for("t", &result, Some(PREFIX));
        assert_eq!(o.state, crate::executor::RunState::Failed, "{o:?}");
        let msg = o.message.as_deref().unwrap();
        assert!(
            msg.contains(&format!("{PREFIX}h-a.txt"))
                && msg.contains("\"a.txt\"")
                && msg.contains("\"h-a.txt\""),
            "{msg}"
        );
        let mut uris: Vec<&str> = o.files.iter().map(|f| f.uri.as_str()).collect();
        uris.sort();
        assert_eq!(
            uris,
            [format!("{PREFIX}b"), format!("{PREFIX}h-a.txt")],
            "each URI once"
        );
        assert!(o.outputs.is_some(), "the outputs are kept");
    }
}
