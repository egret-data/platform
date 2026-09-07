//! `TaskCommands` and `TaskRunner` — the shell-command seam, ported.
//!
//! # The two names, and why both exist here and nowhere else
//!
//! This crate's task abstraction is [`TaskHandler`](super::TaskHandler), and
//! `worker/mod.rs` says why it is **not** called `TaskRunner`: in Kestra that
//! name means a *shell-command executor*, and reusing it would recreate the
//! confusion the programme spec §6.4 had to clear up. W4 Task 5 is the task
//! where egret acquires an actual shell-command executor, so **both** names now
//! exist and the distinction is structural rather than a convention:
//!
//! | egret | Kestra | what it is |
//! |---|---|---|
//! | [`TaskHandler`](super::TaskHandler) | `RunnableTask` | one task **type** — `egret.container`, `egret.python` |
//! | [`TaskRunner`] | `io.kestra.core.models.tasks.runners.TaskRunner` | **where** a command runs — a container today, a pod later |
//! | [`TaskCommands`] | `io.kestra.core.models.tasks.runners.TaskCommands` | **what** to run, as data |
//!
//! A handler builds a [`TaskCommands`] and hands it to a [`TaskRunner`]; it
//! never speaks to a daemon itself. That is the seam Kestra has and it is worth
//! having for the same reason: `egret.python` and `egret.container` differ only
//! in how they fill the struct, and a Kubernetes runner is a second
//! implementation rather than a second copy of every handler.
//!
//! # The contract, measured
//!
//! Read off the pinned image
//! `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`
//! with `javap -p -c -constants`, from `/app/kestra` (the fat jar — core AND
//! the bundled script/docker plugins are both in it; the per-plugin jars under
//! `/app/plugins` carry the language plugins). Each row is a fact this module
//! either follows or diverges from **on the record**:
//!
//! 1. **`TaskRunner.run(RunContext, TaskCommands, List<String>)`** is the whole
//!    interface (`TaskRunner.class`), plus `kill()`, `onKill(Runnable)` and
//!    `checkKilled()`. `TaskRunner extends WorkerJobLifecycle`, so a runner is
//!    killable in its own right.
//! 2. **`TaskRunner.kill()`** is `isKilled.compareAndSet(false, true)` and then
//!    `killable.get().run()` — a per-instance `AtomicReference<Runnable>`. It is
//!    a **handle**, not an identity, and that difference is this module's
//!    central hazard: see [`container_name`].
//! 3. **`TaskCommands`** declares `getContainerImage`, `getLogConsumer`,
//!    `getInterpreter`, `getBeforeCommands`, `getCommands`, `getEnv`,
//!    `getWorkingDirectory`, `getOutputDirectory`, `getTimeout`, `getTargetOS`.
//!    [`TaskCommands`] below is that list minus the parts egret has no seam for,
//!    each named in its own doc.
//! 4. **The argv is `interpreter ++ [ (beforeCommands ++ commands).join(lineSeparator) ]`** —
//!    `ScriptService.scriptCommands`, which builds `new ArrayList<>(interpreter)`
//!    and adds ONE element made by `Collectors.joining(targetOS.lineSeparator)`.
//!    `TargetOS.LINUX.lineSeparator` is `"\n"`. So a list of commands is one
//!    shell script, not one argument each. [`TaskCommands::argv`] is that
//!    function.
//! 5. **The container is created UNNAMED and identified by labels.**
//!    `Docker.configure` calls `createContainerCmd(image).withLabels(…)`,
//!    `withEnv`, `withWorkingDir`, `withEntrypoint`, `withCmd` — there is no
//!    `withName` anywhere in the class. The labels come from
//!    `ScriptService.labels`: `kestra.io/namespace`, `flow-id`, `task-id`,
//!    `execution-id`, `taskrun-id`, `taskrun-attempt`. **egret names the
//!    container**, which is the one deliberate structural divergence in this
//!    module, and [`container_name`] carries the argument for it.
//! 6. **Files move as archives, not as bind mounts.**
//!    `Docker.$default$fileHandlingStrategy` is `VOLUME`, and the class copies
//!    `inputFiles.tar` in and `downloadOutputFiles` out. [`DockerRunner`] uses
//!    `docker cp`, which is the same daemon endpoint, and therefore needs no
//!    host path to be shared with the daemon — the reason it works identically
//!    on Docker Desktop, in CI, and against a remote daemon.
//! 7. **A non-zero exit is `new TaskException(exitCode, logConsumer)`**
//!    (`Docker.run`), which `CommandsWrapper.run` catches and rethrows as
//!    `RunnableTaskException(e, ScriptOutput)` — i.e. the task **fails and
//!    still carries its outputs**. [`super::Outcome`] can express that, and the
//!    handlers do.
//! 8. **Outputs come out of the log stream**, matched by
//!    `TaskLogLineMatcher.LOG_DATA_SYNTAX = Pattern.compile("^::(\\{.*})::$")`
//!    and merged by `PluginUtilsService.parseOut`. [`parse_marker`] is that
//!    regex, hand-rolled — see its doc for why it is not a `regex::Regex`.
//! 9. **`kill` is `docker kill`, not `docker stop`, by default.**
//!    `Docker.kill(client, id, logger)` inspects the container and, if
//!    `State.Running` is `TRUE`, calls `stopContainerCmd(id).withTimeout(n)`
//!    when `killGracePeriod.isPositive()` and `killContainerCmd(id)` otherwise.
//!    `$default$killGracePeriod` returns **`Duration.ZERO`**, so the shipped
//!    default is `killContainerCmd` — SIGKILL. A `NotFoundException` is caught
//!    and **silently** ignored; any other exception is `logger.error(…)` and
//!    **not rethrown**. `$default$delete` is `true`, so the container is removed
//!    afterwards. [`DockerRunner::cancel`] is `docker rm --force`, which is
//!    exactly those two defaults in one call.
//!
//! # What this module deliberately does NOT have
//!
//! * **No internal storage.** Kestra's `ScriptOutput.outputFiles` is a
//!   `Map<String, URI>` into Kestra's own storage. egret's object store belongs
//!   to W9 and this crate cannot reach it — the same wall
//!   [`handlers::sql`](super::handlers::sql)'s missing `fetch: STORE` hit — so
//!   an output file's **contents** come back inline under
//!   [`MAX_OUTPUT_FILE_BYTES`], and a file over the cap fails the task rather
//!   than being truncated. A prefix is not an answer.
//! * **No `namespaceFiles`, no `inputFiles` from storage, no `outputDirectory`,
//!   no `targetOS`.** Each needs a seam this crate does not have (storage,
//!   namespace files, Windows workers). [`TaskCommands::input_files`] carries
//!   the one case a handler here actually needs — a script written by the
//!   handler itself.
//! * **No image pulling policy, no registry credentials, no resource limits.**
//!   `Docker` has `pullPolicy`, `credentials`, `cpu`, `memory`, `shmSize`,
//!   `deviceRequests`, `privileged`, `networkMode`. Adding any of them is a
//!   property with a caller; none has one yet, and this package's standing rule
//!   is that a field with no reader does not ship.

use std::collections::BTreeMap;
use std::process::Stdio;

use egret_core::{Error, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::handlers::{Level, LogSink};
use super::TaskInput;

/// The directory a command runs in, inside the container.
///
/// Kestra's is `/kestra/working-dir` (`ScriptService`'s `workingDir` variable,
/// bound by the `RunContext`'s temp dir); egret's is its own namespace for the
/// same reason egret does not write `kestra.io/` labels.
pub const WORKING_DIR: &str = "/egret/working-dir";

/// The env var naming [`WORKING_DIR`] to the command.
///
/// `ScriptService.ENV_WORKING_DIR = "WORKING_DIR"`, verbatim — an author who
/// has written a Kestra script does not have to learn a second name.
pub const ENV_WORKING_DIR: &str = "WORKING_DIR";

/// What runs the joined command when a handler names no interpreter.
///
/// `AbstractExecScript.$default$interpreter` is `["/bin/sh", "-c"]` and
/// `CommandsWrapper` carries the same literal `/bin/sh`.
pub const DEFAULT_INTERPRETER: [&str; 2] = ["/bin/sh", "-c"];

/// The most bytes ONE output file may carry back, and — summed —
/// the most all of them may.
///
/// Not a performance number, and the same argument as
/// [`handlers::sql::MAX_FETCH_ROWS`](super::handlers::sql::MAX_FETCH_ROWS):
/// an output is written to `task_runs.outputs` (JSONB) and re-read into the
/// render context of **every** later task in the execution. Kestra does not
/// need a cap because its equivalent is a URI into internal storage; egret has
/// no such storage here, so the contents are inline and a cap is the only thing
/// standing between a flow and an unbounded Postgres row.
///
/// **It bounds the total as well as each file, and fix round 1's Important 2 is
/// why that sentence is here.** `outputFiles:` is an unbounded `Vec`, so a
/// per-file cap alone bounds nothing: a thousand files a byte under the cap is
/// a gigabyte, and this doc used to claim it was *the* bound.
/// [`ingest_output_file`] carries a running total and refuses on either.
pub const MAX_OUTPUT_FILE_BYTES: usize = 1_048_576;

/// The most bytes the `::{…}::` markers may contribute in total.
///
/// Same argument as [`MAX_OUTPUT_FILE_BYTES`], applied to the other channel:
/// a container that prints a marker in a loop would otherwise grow the row
/// without bound.
pub const MAX_VARS_BYTES: usize = 1_048_576;

/// What to run, as data — egret's `TaskCommands`.
///
/// Built by a handler, consumed by a [`TaskRunner`]. A struct rather than
/// Kestra's interface because there is one shape and no plugin author to let
/// implement a second.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskCommands {
    /// `TaskCommands.getContainerImage()`. Required — `Docker.run` refuses with
    /// *"This task runner needs the `containerImage` property to be set"*, and
    /// [`DockerRunner::run`] refuses in the same place for the same reason.
    pub container_image: String,
    /// `TaskCommands.getInterpreter()`, defaulting to [`DEFAULT_INTERPRETER`].
    pub interpreter: Vec<String>,
    /// `TaskCommands.getBeforeCommands()` — joined ahead of [`Self::commands`],
    /// which is what makes `set -e` (and a `pip install`) a *prefix* of the
    /// same shell script rather than a separate process.
    pub before_commands: Vec<String>,
    /// `TaskCommands.getCommands()`.
    pub commands: Vec<String>,
    /// `TaskCommands.getEnv()`. `BTreeMap` so the argv this produces is ordered
    /// and therefore assertable.
    pub env: BTreeMap<String, String>,
    /// Files the handler itself writes into [`WORKING_DIR`] before the command
    /// starts, keyed by their name relative to it.
    ///
    /// **Not Kestra's `inputFiles`**, which reads from internal storage and
    /// from namespace files. This carries the one case a handler here has:
    /// `egret.python` writing the author's `script:` to a `.py` file, exactly
    /// as `io.kestra.plugin.scripts.python.Script` does (measured: it writes a
    /// `.py`, then runs it).
    pub input_files: BTreeMap<String, String>,
    /// `AbstractExecScript.outputFiles` — names relative to [`WORKING_DIR`],
    /// read back after the command exits.
    ///
    /// **Literal names only, no globs.** Kestra's `CommandsWrapper` treats
    /// `*`, `?` and `[` as a glob; egret refuses them, because a glob that
    /// matched nothing would produce an output map missing a key the author
    /// declared, and that absence is invisible until a later `{{ … }}` fails
    /// somewhere else.
    pub output_files: Vec<String>,
}

impl TaskCommands {
    /// The argv a container is created with: `interpreter ++ [ joined ]`.
    ///
    /// `ScriptService.scriptCommands` (fact 4 on this module's doc), ported
    /// literally: **one** element after the interpreter, made by joining
    /// `before_commands ++ commands` with `"\n"` — `TargetOS.LINUX`'s line
    /// separator. Joining with a space instead would break `set -e` and every
    /// multi-line script; giving each command its own argv slot would make
    /// `/bin/sh -c a b` run `a` with `b` as `$0`.
    pub fn argv(&self) -> Vec<String> {
        let mut out: Vec<String> = if self.interpreter.is_empty() {
            DEFAULT_INTERPRETER.iter().map(|s| s.to_string()).collect()
        } else {
            self.interpreter.clone()
        };
        let script: Vec<&str> = self
            .before_commands
            .iter()
            .chain(self.commands.iter())
            .map(String::as_str)
            .collect();
        out.push(script.join("\n"));
        out
    }
}

/// What one command run produced — egret's `ScriptOutput`, minus the parts
/// that need internal storage.
///
/// `ScriptOutput` is `{vars, exitCode, outputFiles, stdOutLineCount,
/// stdErrLineCount, taskRunner}`; `taskRunner` is a
/// `TaskRunnerDetailResult` (the container id and such) and is left out
/// because egret has nothing that reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandsResult {
    /// The container's exit status. `0` is a success; anything else fails the
    /// task, and the handler is what decides that (fact 7).
    pub exit_code: i32,
    /// `ScriptOutput.stdOutLineCount` — `AbstractLogConsumer.stdOutCount`.
    pub stdout_line_count: usize,
    /// `ScriptOutput.stdErrLineCount`.
    pub stderr_line_count: usize,
    /// `ScriptOutput.vars` — everything a `::{"outputs": {…}}::` line carried.
    pub vars: serde_json::Map<String, serde_json::Value>,
    /// Declared output files, name → contents.
    pub output_files: BTreeMap<String, String>,
    /// Why the task must fail even though the command may have exited `0`.
    ///
    /// **This channel exists because fix round 1's Important 1 found the code
    /// contradicting the principle its own module doc quotes.** Fact 7 is that
    /// `Docker.run` throws `TaskException(exitCode, logConsumer)` and
    /// `CommandsWrapper.run` rethrows it as `RunnableTaskException(e,
    /// ScriptOutput)` — an exception **carrying an Output**. An output-file
    /// problem used to be an `Err` out of the runner, which threw away the exit
    /// code and every `::{…}::` var the command had already printed; measured,
    /// the run reached `outputs: None`.
    ///
    /// So a problem the AUTHOR caused after the command ran is recorded here
    /// and the result is still returned. `Err` out of [`TaskRunner::run`] is
    /// now reserved for egret's own outage, which is the split
    /// [`handlers::container::mapped`](super::handlers::container) reads.
    pub failure: Option<String>,
}

/// **Where** a command runs.
///
/// One implementation today ([`DockerRunner`]); the trait exists because
/// Kestra's does and for the same reason — a Kubernetes runner is a second
/// implementation of this and not a second `egret.python`.
///
/// # `cancel` has no default body, for [`TaskHandler::cancel`]'s reason
///
/// A default `Ok(())` would answer for a runner that holds a pod, and nothing
/// would say so. Every implementation states what stopping means to it.
///
/// [`TaskHandler::cancel`]: super::TaskHandler::cancel
#[async_trait::async_trait]
pub trait TaskRunner: Send + Sync + std::fmt::Debug {
    /// Run `commands` for `task`, streaming every log line to `sink` as it
    /// arrives.
    ///
    /// `Err` is a **worker** failure and aborts the tick without moving the
    /// execution — the daemon is unreachable, egret's own outage. Everything
    /// the author can act on (a bad image, a command that exited non-zero) is
    /// `Ok` and the handler decides what it means, exactly as
    /// [`Registry::run`](super::Registry) splits the two.
    async fn run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        sink: &dyn LogSink,
    ) -> Result<CommandsResult>;

    /// Stop whatever `task` is running, addressed by **identity**.
    ///
    /// The caller is [`cancel::guarded`](super::TaskHandler::cancel) by way of
    /// a handler, and it has no handle to pass: see [`container_name`].
    async fn cancel(&self, task: &TaskInput) -> Result<()>;
}

/// The container this run owns, named from its **lease**.
///
/// # This is the answer to `TaskHandler::cancel`'s hazard note
///
/// That method's doc carries a section headed *READ THIS BEFORE IMPLEMENTING A
/// HANDLER THAT HOLDS AN EXTERNAL RESOURCE*, written by W4 Task 3 for this
/// task. Restated: [`cancel`](TaskRunner::cancel) is addressed by **run id**,
/// not by a handle to the work, whereas Kestra's `TaskRunner.kill()` runs a
/// `Runnable` captured over *this instance's* container id (fact 2). So a
/// worker A whose lease [`reap`](super::reap) cleared — leaving
/// `state = 'RUNNING'`, which is claimable — can still be inside its handler
/// when worker B claims the same `run_id`, and A's own budget then expires and
/// calls `cancel`. **If the container were named `egret-run-<run_id>`, A would
/// kill B's container.** `attempt` does not disambiguate: the reaper leaves the
/// attempt open, so A and B share it.
///
/// The disambiguator is the lease token, [`TaskInput::claimed_at`] — the value
/// the claim's own `UPDATE … SET claimed_at = now()` wrote and the value every
/// [`apply_outcome`](super::apply_outcome) write is fenced on. A name built
/// from `(run_id, claimed_at)` makes A's kill address a container that does not
/// exist, which the daemon reports as "No such container" and
/// [`DockerRunner::cancel`] treats as nothing to do — the artifact's own answer
/// (fact 9: `NotFoundException` is caught and silently ignored).
///
/// # Why not a handle registry inside the runner
///
/// Because a registry keyed by `run_id` has the identical bug — B overwrites
/// A's entry in the same process — and a registry keyed by
/// `(run_id, claimed_at)` is this function with more moving parts. More
/// decisively, in production A and B are **different processes**: a registry
/// cannot see across them, and the daemon can, because it is shared. The name
/// is the only disambiguator that works in the case the hazard is actually
/// about.
///
/// # The format
///
/// `egret-run-<run id, hyphenless>-<lease, microseconds since the epoch>`.
/// Microseconds because that is Postgres `timestamptz`'s own resolution, so the
/// token is carried without loss; a coarser unit would let two claims of one
/// run collide, which is the whole failure this exists to prevent. Every
/// character is in Docker's `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
///
/// `-` for a negative instant is in that set too, and a pre-epoch lease is
/// unreachable in practice — `claimed_at` is `now()`. It is spelled `_` anyway
/// rather than relying on that, because a name that fails to parse would make
/// the failure a daemon error at run time rather than at review time.
pub fn container_name(task: &TaskInput) -> String {
    let micros = task.claimed_at.timestamp_micros();
    let lease = if micros < 0 {
        format!("_{}", micros.unsigned_abs())
    } else {
        micros.to_string()
    };
    format!("egret-run-{}-{}", task.run_id.simple(), lease)
}

/// The labels a container carries, so an operator can find one this worker
/// left behind.
///
/// `ScriptService.labels` writes `kestra.io/{namespace,flow-id,task-id,
/// execution-id,taskrun-id,taskrun-attempt}` (fact 5). egret writes its own
/// namespace and the same identifiers, plus the one Kestra has no equivalent
/// of and this module depends on: the lease.
///
/// **The labels are diagnostics, not identity.** Nothing in this module looks a
/// container up by them — [`container_name`] is the address — and that is
/// deliberate: a lookup by label would be a second way to name the same thing,
/// able to disagree with the first.
pub fn labels(task: &TaskInput) -> Vec<(String, String)> {
    vec![
        (
            "egret.io/workspace-id".into(),
            task.workspace_id.to_string(),
        ),
        (
            "egret.io/execution-id".into(),
            task.execution_id.to_string(),
        ),
        ("egret.io/taskrun-id".into(), task.run_id.to_string()),
        ("egret.io/task-id".into(), task.task_id.clone()),
        ("egret.io/taskrun-attempt".into(), task.attempt.to_string()),
        ("egret.io/lease".into(), task.claimed_at.to_rfc3339()),
    ]
}

/// **Take one log line: count it, harvest its outputs, and say how to log it.**
///
/// A free function over `&mut CommandsResult` rather than a block inside
/// [`DockerRunner::run_created`], and that is fix round 1's merge condition
/// rather than a tidy-up. The review measured what the shape cost: mutations to
/// the line-handling logic were killed **only** by the live suite, because the
/// unit tests drive a fake [`TaskRunner`] and a mutation *inside* the real one
/// is invisible to them by construction. A faithful enough fake of a container
/// runner **is** [`DockerRunner`], so the seam that had to move was the pure
/// logic out of the async function — which the trait boundary was never going
/// to give.
///
/// Everything here is decided without a daemon, a process or a pipe: the counts,
/// the marker, the vars cap, and the level. `run_created` keeps only the parts
/// that genuinely need one.
///
/// **The cap sets [`CommandsResult::failure`] and clears the vars rather than
/// returning an error.** Returning one mid-stream abandoned the exit code and
/// left `docker start`'s pipes unread, which stalls a container that is still
/// writing; and the vars are cleared rather than kept because the whole point
/// of the cap is that this much is not going into `task_runs.outputs`.
pub(crate) fn ingest_line(
    result: &mut CommandsResult,
    vars_bytes: &mut usize,
    max_vars_bytes: usize,
    line: &str,
    is_err: bool,
) -> Level {
    if is_err {
        result.stderr_line_count += 1;
    } else {
        result.stdout_line_count += 1;
    }

    // The marker is consumed as an output AND logged, which is what
    // `DefaultLogConsumer` does: it calls `parseOut` and then logs the line
    // either way.
    if result.failure.is_none() {
        if let Some(outputs) = parse_marker(line) {
            for (k, v) in outputs {
                *vars_bytes += k.len() + v.to_string().len();
                if *vars_bytes > max_vars_bytes {
                    result.vars.clear();
                    result.failure = Some(format!(
                        "the command emitted more than {max_vars_bytes} bytes of \
                         `::{{\"outputs\": …}}::` markers, and every byte of that would be \
                         stored on the task run and re-rendered by every later task in \
                         the execution"
                    ));
                    break;
                }
                result.vars.insert(k, v);
            }
        }
    }

    // stdout is INFO and stderr is WARN. Kestra's knob for the second half is
    // `AbstractExecScript.warningOnStdErr`; egret does not offer it, because a
    // property with no reader does not ship and nothing here reads a level
    // back.
    if is_err {
        Level::Warn
    } else {
        Level::Info
    }
}

/// **Resolve one declared output file inside the copied working directory.**
///
/// The second pure seam, and the reason the whole working directory is copied
/// back in one `docker cp` rather than one per declared file: *whether a
/// declared file is there* becomes a question for the **filesystem** instead of
/// a question about the wording of a daemon error string.
///
/// That is fix round 1's Critical 1 and Important 5 answered together, because
/// they were one bug seen from two ends:
///
/// * **C1** — `outputFiles: ["some-dir"]` copied fine, then `fs::read` on a
///   directory gave an `io::Error`, which became `Error::Internal`, which
///   `handlers::container::mapped` correctly reads as **egret's outage** and
///   turns into a tick-aborting worker failure. One typo'd flow wedged the
///   worker for ever, blaming egret. A directory is now named and refused here.
/// * **I5** — the old code asked "is this the daemon?" by looking for
///   `"no such file or directory"` in the copy's stderr **before** concluding
///   the author's file was missing, so a CLI wording change would have silently
///   converted an author error into C1's failure mode. There is no longer a
///   string test on that path at all: an absent file is `ErrorKind::NotFound`
///   on a `std::fs::metadata` of a path in egret's own staging directory.
///
/// `Err` is the message the **task** fails with — never a worker failure. The
/// caller puts it in [`CommandsResult::failure`], so the exit code and the vars
/// the command printed survive it.
pub(crate) fn ingest_output_file(
    result: &mut CommandsResult,
    staged_root: &std::path::Path,
    rel: &str,
    max_file_bytes: usize,
    max_total_bytes: usize,
    total: &mut usize,
) -> std::result::Result<(), String> {
    if let Some(problem) = relative_problem(rel) {
        return Err(problem);
    }
    let path = staged_root.join(rel);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // **A declared output that is not there fails the task.** The
            // alternative is a key missing from `outputs`, which nothing
            // notices until a later `{{ outputs.x.f }}` fails somewhere else
            // with a message about the wrong task.
            return Err(format!(
                "`outputFiles` declares {rel:?}, which the command did not leave in \
                 {WORKING_DIR}"
            ));
        }
        Err(e) => {
            return Err(format!(
                "`outputFiles` declares {rel:?}, which could not be read back out of \
                 {WORKING_DIR}: {e}"
            ))
        }
    };
    if meta.is_dir() {
        return Err(format!(
            "`outputFiles` declares {rel:?}, which is a DIRECTORY. An output is JSON \
             and egret has no internal storage to hand back an archive URI for (W9 \
             owns it), so name the files inside it instead"
        ));
    }
    let len = meta.len() as usize;
    if len > max_file_bytes {
        return Err(format!(
            "the output file {rel:?} is {len} bytes, over the {max_file_bytes} an \
             output may carry. egret has no internal storage for a workflow yet (W9 \
             owns it), so an output file's CONTENTS are stored on the task run and \
             re-rendered by every later task in the execution; a prefix is not an \
             answer"
        ));
    }
    *total += len;
    if *total > max_total_bytes {
        return Err(format!(
            "the output files declared by this task come to {total} bytes, over the \
             {max_total_bytes} one task may carry. `outputFiles:` is an unbounded \
             list, so a per-file cap alone bounds nothing"
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| {
        format!("the output file {rel:?} could not be read out of the staging copy: {e}")
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "the output file {rel:?} is not UTF-8, and an output is JSON. egret has no \
             internal storage to hand back a URI to (W9 owns it), so a binary artefact \
             cannot be an output of this task today"
        )
    })?;
    result.output_files.insert(rel.to_string(), text);
    Ok(())
}

/// `TaskLogLineMatcher.LOG_DATA_SYNTAX`, applied to one log line.
///
/// The artifact's regex is `^::(\{.*})::$`, compiled once and matched against
/// every line of both streams; a match is parsed as JSON and its `outputs` key
/// merged into the task's `vars` (`PluginUtilsService.parseOut`).
///
/// **Hand-rolled rather than a `regex::Regex`.** `regex` is already a
/// dependency of this crate, so that is not the reason: the reason is that
/// `.*` against an arbitrary container's stdout is a line-length-proportional
/// backtrack on every line of every run, and this predicate is *prefix and
/// suffix*, which is two `str` comparisons. The unit tests below pin the two
/// against each other on the cases that distinguish them (a `::` line that is
/// not JSON, a line with `::` in the middle, an empty payload).
///
/// Returns the JSON **object** the line carried, or `None` when the line is an
/// ordinary log line — including when it looks like a marker and does not
/// parse, which is Kestra's behaviour too (`parseOut` logs *"Invalid outputs
/// '{}'"* and carries on).
pub fn parse_marker(line: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let line = line.trim_end_matches(['\r', '\n']);
    let inner = line.strip_prefix("::")?.strip_suffix("::")?;
    // `\{.*}` — the payload must itself be brace-delimited. A `::foo::` line is
    // an ordinary line, not a broken marker.
    if !(inner.starts_with('{') && inner.ends_with('}')) {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(inner).ok()?;
    let obj = parsed.as_object()?;
    // `parseOut` merges the `outputs` key and nothing else.
    match obj.get("outputs") {
        Some(serde_json::Value::Object(outputs)) => Some(outputs.clone()),
        _ => None,
    }
}

/// The three substrings Kestra reads as "the daemon is not there".
///
/// `Docker.DOCKER_SOCKET_ERROR_MESSAGES = List.of("docker.sock",
/// "no such file or directory", "Cannot connect to the Docker daemon")`,
/// verbatim from the class's static initialiser, used by
/// `Docker.isDockerSocketAccessError`.
///
/// It is what separates a **worker** failure from a **task** failure here: a
/// daemon that is not running is egret's outage and must not fail the author's
/// flow, while an image that does not exist is the author's and must.
const DAEMON_ERROR_MARKERS: [&str; 3] = [
    "docker.sock",
    "no such file or directory",
    "Cannot connect to the Docker daemon",
];

/// The two of [`DAEMON_ERROR_MARKERS`] that name the **socket** rather than any
/// missing path.
///
/// Fix round 1's Important 5: on a `docker cp` out of a container, the generic
/// `"no such file or directory"` is far more likely to be the author's own file
/// than the daemon's socket, and reading it as the daemon turns an author error
/// into a tick-aborting worker failure. Kestra never applies its list to a copy
/// — `isDockerSocketAccessError` is used on the client's own `DockerException`
/// on the connect path — so narrowing here is applying the artifact's own scope,
/// not diverging from it.
///
/// `collect_output_files` is the only user, and the ordering hazard is gone
/// besides: the question it used to answer with a string is now answered by
/// [`ingest_output_file`] against the filesystem.
const DAEMON_SOCKET_MARKERS: [&str; 2] = ["docker.sock", "Cannot connect to the Docker daemon"];

/// A [`TaskRunner`] over the local Docker daemon, driven through the `docker`
/// CLI.
///
/// # Why the CLI and not a client library
///
/// Kestra uses `docker-java`. The Rust equivalent is `bollard`, which is not in
/// this workspace's tree and would bring `hyper`'s unix-socket stack, `bytes`,
/// `tar` and a `serde` model of the whole Docker API for the five verbs used
/// here (`create`, `cp`, `start`, `inspect`, `rm`). The CLI is already the
/// dependency — `oracle-exec/docker-compose.yml` requires it, and the daemon it
/// talks to is the same one — and `docker cp` in particular does the tar
/// framing that would otherwise be a dependency of its own.
///
/// The cost is stated: this shells out, so it inherits the CLI's exit codes and
/// its message text, and [`DAEMON_ERROR_MARKERS`] is a **substring** test on
/// stderr rather than a typed error. That is exactly what
/// `Docker.isDockerSocketAccessError` does with the same three strings, so it
/// is the artifact's own precision and not a shortcut past a better one.
#[derive(Clone, Debug)]
pub struct DockerRunner {
    /// The binary. A field so a test can point it at a script that is not
    /// Docker — which is how [`DockerRunner::run`]'s daemon-versus-task split
    /// is driven without breaking the machine's daemon.
    binary: String,
    /// [`MAX_OUTPUT_FILE_BYTES`], as a field.
    max_output_bytes: usize,
    /// [`MAX_VARS_BYTES`], as a field.
    max_vars_bytes: usize,
}

impl Default for DockerRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerRunner {
    /// The shipping runner: the `docker` on `PATH`, [`MAX_OUTPUT_FILE_BYTES`]
    /// and [`MAX_VARS_BYTES`].
    pub fn new() -> Self {
        Self {
            binary: "docker".to_string(),
            max_output_bytes: MAX_OUTPUT_FILE_BYTES,
            max_vars_bytes: MAX_VARS_BYTES,
        }
    }

    /// The same runner over another binary. For a test, and for an operator
    /// whose CLI is not on `PATH`.
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            ..Self::new()
        }
    }

    /// The same runner with smaller caps.
    ///
    /// `handlers::sql::Sql::with_limits`'s reason, applied to the other two
    /// unbounded channels: *"so a test can drive the cap without materialising
    /// ten thousand rows to do it"*. A cap nobody has watched refuse is a cap
    /// nobody has watched, and a megabyte of container output inside a test is
    /// a slow way to learn that.
    pub fn with_limits(mut self, max_output_bytes: usize, max_vars_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self.max_vars_bytes = max_vars_bytes;
        self
    }

    /// Run one `docker` subcommand to completion, capturing both streams.
    async fn cli(&self, args: &[&str]) -> Result<std::process::Output> {
        Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "could not run `{} {}`: {e}",
                    self.binary,
                    args.first().copied().unwrap_or("")
                ))
            })
    }

    /// Split one failed `docker` invocation into egret's outage and the
    /// author's mistake.
    fn classify(&self, what: &str, stderr: &str) -> Error {
        let stderr = stderr.trim();
        if DAEMON_ERROR_MARKERS.iter().any(|m| stderr.contains(m)) {
            return Error::Internal(anyhow::anyhow!(
                "the Docker daemon could not be reached, so `{what}` did not run: {stderr}"
            ));
        }
        Error::InvalidInput(format!("docker {what}: {stderr}"))
    }
}

#[async_trait::async_trait]
impl TaskRunner for DockerRunner {
    async fn run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        sink: &dyn LogSink,
    ) -> Result<CommandsResult> {
        // `Docker.run`'s own first act, and its own message.
        if commands.container_image.trim().is_empty() {
            return Err(Error::InvalidInput(
                "This task runner needs the `containerImage` property to be set".to_string(),
            ));
        }

        let name = container_name(task);
        let argv = commands.argv();

        // ---- create ----------------------------------------------------
        let mut create: Vec<String> = vec![
            "create".into(),
            "--name".into(),
            name.clone(),
            "--workdir".into(),
            WORKING_DIR.into(),
        ];
        for (k, v) in labels(task) {
            create.push("--label".into());
            create.push(format!("{k}={v}"));
        }
        create.push("--env".into());
        create.push(format!("{ENV_WORKING_DIR}={WORKING_DIR}"));
        for (k, v) in &commands.env {
            create.push("--env".into());
            create.push(format!("{k}={v}"));
        }
        // `withEntrypoint(interpreter)` + `withCmd(joined)`. The CLI takes one
        // string for `--entrypoint`, so the interpreter's head is the
        // entrypoint and its tail leads the CMD — which produces the identical
        // argv, because the daemon concatenates the two.
        create.push("--entrypoint".into());
        create.push(argv[0].clone());
        create.push(commands.container_image.clone());
        create.extend(argv[1..].iter().cloned());

        let borrowed: Vec<&str> = create.iter().map(String::as_str).collect();
        let created = self.cli(&borrowed).await?;
        if !created.status.success() {
            return Err(self.classify("create", &String::from_utf8_lossy(&created.stderr)));
        }

        // Everything from here on owns a container, so every exit runs through
        // `remove`. `?` would leak one.
        let result = self.run_created(task, commands, sink, &name).await;
        let removed = self.cli(&["rm", "--force", &name]).await;
        match removed {
            Ok(out) if !out.status.success() => tracing::warn!(
                run = %task.run_id,
                container = %name,
                "could not remove the container after the run: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => tracing::warn!(run = %task.run_id, container = %name, "{e}"),
            Ok(_) => {}
        }
        result
    }

    /// [`container_name`]'s whole argument, applied: this addresses the
    /// container by the name that carries **this worker's lease**, so a worker
    /// whose lease was reaped names a container that no longer exists.
    ///
    /// `docker rm --force` is `killContainerCmd` + delete, which are Kestra's
    /// two shipped defaults (`$default$killGracePeriod` = `Duration.ZERO`,
    /// `$default$delete` = `true`) in one call. A grace period is deliberately
    /// not offered: it is a property with no reader, and the artifact's default
    /// is not to have one.
    ///
    /// **A container that is not there is `Ok(())`**, which is the artifact's
    /// answer too — `Docker.kill`'s exception table catches
    /// `NotFoundException` and returns without logging. It is also the only
    /// answer that is true of the two ways it happens: the run already
    /// finished, or this worker's lease is stale and the container belongs to
    /// nobody by this name.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        let name = container_name(task);
        let out = self.cli(&["rm", "--force", &name]).await?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("No such container") {
            return Ok(());
        }
        Err(self.classify("rm", &stderr))
    }
}

impl DockerRunner {
    /// The half of [`TaskRunner::run`] that runs with a container in
    /// existence.
    ///
    /// Split out so the caller can remove the container on **every** path
    /// without a `Drop` guard that would have to block on an async call.
    async fn run_created(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        sink: &dyn LogSink,
        name: &str,
    ) -> Result<CommandsResult> {
        // ---- input files ------------------------------------------------
        //
        // `docker cp` into a container that has never started, which is
        // Kestra's `copyArchiveToContainer` and the reason the working
        // directory does not have to exist first: measured on this machine's
        // daemon (28.4.0), a copy into `/egret/working-dir` creates it.
        if !commands.input_files.is_empty() {
            let staging = tempdir(task)?;
            for (rel, contents) in &commands.input_files {
                check_relative(rel)?;
                let path = staging.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(io_err)?;
                }
                std::fs::write(&path, contents).map_err(io_err)?;
            }
            let src = format!("{}/.", staging.display());
            let dst = format!("{name}:{WORKING_DIR}");
            let out = self.cli(&["cp", &src, &dst]).await;
            let _ = std::fs::remove_dir_all(&staging);
            let out = out?;
            if !out.status.success() {
                return Err(self.classify("cp", &String::from_utf8_lossy(&out.stderr)));
            }
        }

        // ---- start, and stream --------------------------------------------
        let mut child = Command::new(&self.binary)
            .args(["start", "--attach", name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "could not run `{} start`: {e}",
                    self.binary
                ))
            })?;

        let mut out_lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
        let mut err_lines = BufReader::new(child.stderr.take().expect("stderr piped")).lines();

        let mut result = CommandsResult::default();
        let mut vars_bytes = 0usize;
        let mut out_open = true;
        let mut err_open = true;
        while out_open || err_open {
            // Both `next_line` futures are documented cancel-safe, which is
            // what makes this loop correct rather than lossy: the branch that
            // loses the select has read nothing.
            let (line, is_err) = tokio::select! {
                l = out_lines.next_line(), if out_open => match l.map_err(io_err)? {
                    Some(l) => (l, false),
                    None => { out_open = false; continue }
                },
                l = err_lines.next_line(), if err_open => match l.map_err(io_err)? {
                    Some(l) => (l, true),
                    None => { err_open = false; continue }
                },
            };

            // **Every decision about this line is [`ingest_line`]'s**, and
            // this loop keeps only the pipes. See that function for why the
            // split is a merge condition and not a tidy-up.
            let level = ingest_line(
                &mut result,
                &mut vars_bytes,
                self.max_vars_bytes,
                &line,
                is_err,
            );
            sink.log(task, level, &line);
        }

        let status = child.wait().await.map_err(io_err)?;

        // ---- the exit code, read from the container and not from the CLI ---
        //
        // `docker start --attach` propagates the container's status, but it
        // also has statuses of its own (a daemon that went away mid-stream),
        // and the two are indistinguishable from the outside. `inspect` asks
        // the container.
        let inspected = self
            .cli(&["inspect", "--format", "{{.State.ExitCode}}", name])
            .await?;
        if !inspected.status.success() {
            return Err(self.classify("inspect", &String::from_utf8_lossy(&inspected.stderr)));
        }
        let raw = String::from_utf8_lossy(&inspected.stdout)
            .trim()
            .to_string();
        result.exit_code = raw.parse::<i32>().map_err(|_| {
            Error::Internal(anyhow::anyhow!(
                "docker inspect returned {raw:?} for the exit code of {name}, \
                 and `start` exited {status}"
            ))
        })?;

        // ---- output files ---------------------------------------------------
        if !commands.output_files.is_empty() && result.failure.is_none() {
            let staging = tempdir(task)?;
            let collected = self
                .collect_output_files(name, commands, &staging, &mut result)
                .await;
            let _ = std::fs::remove_dir_all(&staging);
            collected?;
        }

        Ok(result)
    }

    /// Copy the working directory back **once**, then resolve every declared
    /// file inside it with [`ingest_output_file`].
    ///
    /// # One copy, not one per file, and that is the fix and not an optimisation
    ///
    /// It is also what the artifact does: `Docker.run` carries the string
    /// *"Unable to sync the working directory back from the container; the task
    /// declares no output files, so continuing without failing the run."*, so
    /// Kestra syncs the **directory** and decides about individual files
    /// afterwards. Doing the same means "is the author's file there?" is
    /// answered by `std::fs::metadata` on egret's own staging copy rather than
    /// by the wording of a `docker cp` error — which is what Important 5 was
    /// about, and what made Critical 1 reachable.
    ///
    /// So the only failure this can attribute to the daemon is the ONE copy,
    /// and even there the generic `"no such file or directory"` marker is not
    /// consulted: [`DAEMON_SOCKET_MARKERS`] is the narrower pair, because at
    /// this call site the generic string is far more likely to be the author's
    /// container than the socket.
    ///
    /// `Err` is a **worker** failure and reaches only the daemon case; an
    /// author's problem lands in [`CommandsResult::failure`] with the exit code
    /// and the vars intact.
    async fn collect_output_files(
        &self,
        name: &str,
        commands: &TaskCommands,
        staging: &std::path::Path,
        result: &mut CommandsResult,
    ) -> Result<()> {
        std::fs::create_dir_all(staging).map_err(io_err)?;
        let src = format!("{name}:{WORKING_DIR}/.");
        let dst = staging.display().to_string();
        let out = self.cli(&["cp", &src, &dst]).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if DAEMON_SOCKET_MARKERS.iter().any(|m| stderr.contains(m)) {
                return Err(Error::Internal(anyhow::anyhow!(
                    "the Docker daemon could not be reached, so the working directory \
                     of {name} could not be copied back: {stderr}"
                )));
            }
            result.failure = Some(format!(
                "the working directory could not be copied back out of the container, \
                 so `outputFiles:` cannot be answered: {stderr}"
            ));
            return Ok(());
        }

        let mut total = 0usize;
        for rel in &commands.output_files {
            if let Err(problem) = ingest_output_file(
                result,
                staging,
                rel,
                self.max_output_bytes,
                self.max_output_bytes,
                &mut total,
            ) {
                result.failure = Some(problem);
                break;
            }
        }
        Ok(())
    }
}

/// A private staging directory for one run's file copies.
///
/// Named after the run so two concurrent runs cannot collide, and created under
/// the process temp dir rather than in the repo.
fn tempdir(task: &TaskInput) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "egret-workflow-{}-{}",
        task.run_id.simple(),
        task.claimed_at.timestamp_micros()
    ));
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    Ok(dir)
}

/// A file name a handler may name, relative to [`WORKING_DIR`].
///
/// Kestra's own check is a character allow-list with the message *"Output file
/// '%s' contains unsupported characters. Allowed: letters, digits, hyphens,
/// dots, underscores, plus, tilde, percent, equals, slashes, commas, colons,
/// semicolons."* (`ScriptService`). egret refuses on the three properties that
/// matter to a `docker cp` argument rather than on a character class: an
/// absolute path, a `..` segment, and a glob. The character class would also
/// have to be *widened* here — a `docker cp` destination is a path, not a
/// pattern — and a rule that has to be widened is a rule that stops being read.
fn check_relative(rel: &str) -> Result<()> {
    match relative_problem(rel) {
        None => Ok(()),
        Some(problem) => Err(Error::InvalidInput(problem)),
    }
}

/// The same rule as a **message or nothing**, so [`ingest_output_file`] can use
/// it without an [`Error`] it would only have to unwrap again.
fn relative_problem(rel: &str) -> Option<String> {
    if rel.is_empty() {
        return Some("a file name must not be empty".to_string());
    }
    if rel.starts_with('/') {
        return Some(format!(
            "{rel:?} is an absolute path; a file is named relative to {WORKING_DIR}"
        ));
    }
    if rel.split('/').any(|seg| seg == "..") {
        return Some(format!("{rel:?} climbs out of {WORKING_DIR} with `..`"));
    }
    if rel.contains(['*', '?', '[']) {
        return Some(format!(
            "{rel:?} looks like a glob, and this task takes literal names only. A glob \
             that matched nothing would leave a declared output absent from the outputs \
             map, which nothing notices until a later expression fails"
        ));
    }
    None
}

fn io_err(e: std::io::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::handlers::testing::input;

    fn leased(secs: i64) -> TaskInput {
        TaskInput {
            run_id: uuid::Uuid::from_u128(7),
            claimed_at: chrono::DateTime::from_timestamp(secs, 0).expect("a valid instant"),
            ..input("egret.container", serde_json::json!({}))
        }
    }

    /// **The lease is in the name, and this is the assertion that would fail if
    /// it were not.**
    ///
    /// Two inputs that agree on everything the old `TaskInput` carried —
    /// `run_id`, `execution_id`, `task_id`, `attempt` — and differ only in the
    /// lease. If [`container_name`] dropped `claimed_at`, these two would be
    /// equal, and a reaped worker's `cancel` would address its replacement's
    /// container. The unit test says the names differ; the live suite's
    /// `a_reaped_workers_cancel_cannot_reach_its_replacements_container` says
    /// the daemon agrees.
    #[test]
    fn the_container_name_carries_the_lease_and_not_only_the_run() {
        let a = leased(1_700_000_000);
        let b = leased(1_700_000_001);
        assert_eq!(a.run_id, b.run_id, "the two differ ONLY in the lease");
        assert_eq!(a.attempt, b.attempt);
        assert_ne!(
            container_name(&a),
            container_name(&b),
            "a name that ignores `claimed_at` lets a reaped worker kill its \
             replacement's container -- see `TaskHandler::cancel`'s hazard note"
        );
        assert_eq!(
            container_name(&a),
            "egret-run-00000000000000000000000000000007-1700000000000000",
            "the format is pinned: a change to it is a change to what `cancel` \
             addresses, and every running container's name at once"
        );
    }

    /// Docker's own rule for a container name:
    /// `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
    #[test]
    fn every_name_this_produces_is_a_legal_container_name() {
        for secs in [0_i64, 1, 1_700_000_000, -1, -1_700_000_000] {
            let name = container_name(&leased(secs));
            let mut chars = name.chars();
            let first = chars.next().expect("non-empty");
            assert!(
                first.is_ascii_alphanumeric(),
                "{name} starts with {first:?}"
            );
            assert!(
                chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
                "{name} holds a character docker will refuse"
            );
        }
    }

    /// `ScriptService.scriptCommands`: the interpreter, then **one** element.
    #[test]
    fn the_argv_is_the_interpreter_then_one_joined_script() {
        let cmds = TaskCommands {
            container_image: "alpine".into(),
            before_commands: vec!["set -e".into()],
            commands: vec!["echo a".into(), "echo b".into()],
            ..TaskCommands::default()
        };
        assert_eq!(
            cmds.argv(),
            vec!["/bin/sh", "-c", "set -e\necho a\necho b"],
            "a list of commands is ONE shell script joined by the line separator, \
             not one argv slot each -- `/bin/sh -c a b` runs `a` with `b` as $0"
        );
    }

    /// An interpreter the handler chose replaces the default whole.
    #[test]
    fn a_named_interpreter_replaces_the_default() {
        let cmds = TaskCommands {
            interpreter: vec!["/usr/bin/env".into(), "python3".into(), "-c".into()],
            commands: vec!["print(1)".into()],
            ..TaskCommands::default()
        };
        assert_eq!(
            cmds.argv(),
            vec!["/usr/bin/env", "python3", "-c", "print(1)"]
        );
    }

    /// `^::(\{.*})::$`, and the lines that distinguish it from a looser test.
    #[test]
    fn the_output_marker_is_the_artifacts_own_syntax() {
        let hit = parse_marker(r#"::{"outputs":{"a":1,"b":"x"}}::"#).expect("a marker");
        assert_eq!(hit.get("a"), Some(&serde_json::json!(1)));
        assert_eq!(hit.get("b"), Some(&serde_json::json!("x")));

        // A trailing newline is not part of the line, and a reader that split
        // on `\n` keeps the `\r` of a CRLF stream.
        assert!(parse_marker("::{\"outputs\":{\"a\":1}}::\r\n").is_some());

        for ordinary in [
            "hello",
            "::not json::",
            "prefix ::{\"outputs\":{}}::",
            "::{\"outputs\":{}}:: suffix",
            "::{\"outputs\":{\"a\":1}}",
            "{\"outputs\":{\"a\":1}}",
            // A marker carrying something other than `outputs` contributes
            // nothing: `parseOut` merges that key and no other.
            "::{\"metrics\":{\"a\":1}}::",
            // Broken JSON is an ordinary line, not a failure -- `parseOut`
            // logs "Invalid outputs '{}'" and carries on.
            "::{\"outputs\":}::",
        ] {
            assert!(
                parse_marker(ordinary).is_none(),
                "{ordinary:?} must not be read as an output marker"
            );
        }
    }

    /// The three refusals, each with the reason in its message.
    #[test]
    fn a_file_name_that_escapes_the_working_directory_is_refused() {
        for (bad, needle) in [
            ("", "must not be empty"),
            ("/etc/passwd", "absolute path"),
            ("../../etc/passwd", "climbs out"),
            ("a/../../b", "climbs out"),
            ("*.csv", "glob"),
            ("out[0].txt", "glob"),
        ] {
            let err = check_relative(bad).expect_err("{bad:?} must be refused");
            assert!(
                err.to_string().contains(needle),
                "{bad:?} was refused with {err}, which does not say {needle:?}"
            );
        }
        for good in ["out.txt", "nested/out.txt", "a.b-c_d+e"] {
            check_relative(good).unwrap_or_else(|e| panic!("{good:?} must be allowed: {e}"));
        }
    }

    /// A staging directory this test owns, prefixed so parallel agents sharing
    /// one temp dir cannot collide.
    fn staging(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "egret-w4t5-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("staging");
        dir
    }

    /// **Both streams are counted, and the level says which one it was.**
    ///
    /// This is one of the four properties fix round 1 moved out of
    /// `run_created` so a machine with no daemon can judge them. Before the
    /// split, the only test that could see the level was a live one.
    #[test]
    fn a_line_is_counted_on_its_own_stream_and_logged_at_its_own_level() {
        let mut r = CommandsResult::default();
        let mut bytes = 0usize;
        assert_eq!(
            ingest_line(&mut r, &mut bytes, 1024, "out", false),
            Level::Info
        );
        assert_eq!(
            ingest_line(&mut r, &mut bytes, 1024, "err", true),
            Level::Warn,
            "stderr is WARN and stdout is INFO -- a logger that took the level from \
             nowhere would still have the text"
        );
        assert_eq!((r.stdout_line_count, r.stderr_line_count), (1, 1));
        assert!(r.vars.is_empty() && r.failure.is_none());
    }

    /// **A marker becomes vars; an ordinary line does not.**
    #[test]
    fn a_marker_line_is_harvested_and_still_logged() {
        let mut r = CommandsResult::default();
        let mut bytes = 0usize;
        ingest_line(&mut r, &mut bytes, 1024, "ordinary", false);
        ingest_line(
            &mut r,
            &mut bytes,
            1024,
            r#"::{"outputs":{"a":1}}::"#,
            false,
        );
        assert_eq!(r.vars.get("a"), Some(&serde_json::json!(1)));
        assert_eq!(
            r.stdout_line_count, 2,
            "a marker is a line too -- `DefaultLogConsumer` parses it AND logs it"
        );
    }

    /// **The vars cap sets a failure, clears what it had, and stops
    /// collecting** — it does not abandon the stream.
    #[test]
    fn the_vars_cap_fails_the_task_and_keeps_reading() {
        let mut r = CommandsResult::default();
        let mut bytes = 0usize;
        let big = "y".repeat(200);
        ingest_line(
            &mut r,
            &mut bytes,
            32,
            &format!(r#"::{{"outputs":{{"a":"{big}"}}}}::"#),
            false,
        );
        assert!(
            r.vars.is_empty(),
            "the point of the cap is that this much does NOT go into task_runs.outputs"
        );
        let why = r.failure.clone().expect("the cap must fail the task");
        assert!(why.contains("32 bytes"), "the cap must be named: {why}");
        assert!(why.contains("markers"), "the channel must be named: {why}");

        // And the stream keeps being read, so the container is not left
        // blocked on a full pipe and the line counts still complete.
        assert_eq!(
            ingest_line(&mut r, &mut bytes, 32, "later", true),
            Level::Warn
        );
        assert_eq!(r.stderr_line_count, 1);
        assert_eq!(
            r.failure.as_deref(),
            Some(why.as_str()),
            "the first reason stands; a later line must not overwrite it"
        );
    }

    /// **A declared output that is not in the copied working directory fails
    /// the task, naming it** — and it is a `std::fs` answer now, not a guess at
    /// a daemon's wording (fix round 1's Important 5).
    #[test]
    fn a_declared_output_that_is_absent_fails_the_task_and_names_itself() {
        let dir = staging("absent");
        let mut r = CommandsResult::default();
        let mut total = 0usize;
        let err = ingest_output_file(&mut r, &dir, "gone.txt", 1024, 4096, &mut total)
            .expect_err("an absent output must not be skipped");
        assert!(err.contains("gone.txt"), "{err}");
        assert!(err.contains("did not leave"), "{err}");
        assert!(r.output_files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A directory named as an output file fails the TASK, not the worker.**
    ///
    /// Fix round 1's Critical 1: `fs::read` on a directory gave an
    /// `io::Error`, which became `Error::Internal`, which
    /// `handlers::container::mapped` reads as egret's outage and turns into a
    /// tick-aborting worker failure. One typo'd flow wedged the worker for
    /// ever, blaming egret. This function cannot produce a worker failure at
    /// all — its `Err` is the task's message — and the directory is now named.
    #[test]
    fn a_directory_named_as_an_output_file_fails_the_task_rather_than_the_worker() {
        let dir = staging("dir");
        std::fs::create_dir_all(dir.join("adir")).expect("mkdir");
        std::fs::write(dir.join("adir/inner"), "x").expect("write");
        let mut r = CommandsResult::default();
        let mut total = 0usize;
        let err = ingest_output_file(&mut r, &dir, "adir", 1024, 4096, &mut total)
            .expect_err("a directory is not an output");
        assert!(err.contains("DIRECTORY"), "{err}");
        assert!(err.contains("adir"), "{err}");
        // The file inside it is nameable, so the refusal is about the shape
        // and not about the path being unreachable.
        ingest_output_file(&mut r, &dir, "adir/inner", 1024, 4096, &mut total)
            .expect("a file inside a directory is a fine output");
        assert_eq!(
            r.output_files.get("adir/inner").map(String::as_str),
            Some("x")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The per-file cap and the TOTAL cap both refuse**, and the second is
    /// fix round 1's Important 2: `outputFiles:` is an unbounded list, so a
    /// per-file cap alone bounds nothing.
    #[test]
    fn both_the_per_file_cap_and_the_total_cap_refuse() {
        let dir = staging("caps");
        std::fs::write(dir.join("a"), "x".repeat(100)).expect("write");
        std::fs::write(dir.join("b"), "y".repeat(100)).expect("write");
        std::fs::write(dir.join("c"), "z".repeat(100)).expect("write");

        // Per file.
        let mut r = CommandsResult::default();
        let mut total = 0usize;
        let err = ingest_output_file(&mut r, &dir, "a", 64, 1_000_000, &mut total)
            .expect_err("100 bytes is over a 64-byte per-file cap");
        assert!(err.contains("100 bytes"), "{err}");
        assert!(err.contains("over the 64"), "{err}");
        assert!(err.contains("a prefix is not an answer"), "{err}");

        // Total: each file is UNDER the per-file cap, and together they are
        // over the total. A per-file-only bound passes this.
        let mut r = CommandsResult::default();
        let mut total = 0usize;
        ingest_output_file(&mut r, &dir, "a", 150, 250, &mut total).expect("a fits");
        ingest_output_file(&mut r, &dir, "b", 150, 250, &mut total).expect("b fits");
        let err = ingest_output_file(&mut r, &dir, "c", 150, 250, &mut total)
            .expect_err("three 100-byte files are over a 250-byte total");
        assert!(err.contains("300 bytes"), "the TOTAL must be named: {err}");
        assert!(err.contains("unbounded list"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A binary artefact cannot be an output, and says why.
    #[test]
    fn a_non_utf8_output_file_fails_the_task_saying_why() {
        let dir = staging("binary");
        std::fs::write(dir.join("b.bin"), [0xff_u8, 0xfe, 0x00]).expect("write");
        let mut r = CommandsResult::default();
        let mut total = 0usize;
        let err = ingest_output_file(&mut r, &dir, "b.bin", 1024, 4096, &mut total)
            .expect_err("not UTF-8");
        assert!(err.contains("not UTF-8"), "{err}");
        assert!(
            err.contains("W9"),
            "the reason is the missing storage seam: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The name rule reaches the output path too, and before the filesystem
    /// does.
    #[test]
    fn an_escaping_output_name_is_refused_before_the_filesystem_is_touched() {
        let dir = staging("escape");
        let mut r = CommandsResult::default();
        let mut total = 0usize;
        for (bad, needle) in [
            ("../secret", "climbs out"),
            ("/etc/passwd", "absolute path"),
            ("*.csv", "glob"),
            ("", "must not be empty"),
        ] {
            let err = ingest_output_file(&mut r, &dir, bad, 1024, 4096, &mut total)
                .expect_err("an escaping name must be refused");
            assert!(err.contains(needle), "{bad:?} was refused with {err}");
        }
        assert_eq!(total, 0, "a refused name must not have been measured");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The daemon's absence is egret's outage; anything else is the author's
    /// flow. The split is a substring test on stderr, so it is asserted on the
    /// artifact's own three strings.
    #[test]
    fn a_daemon_that_is_not_there_is_a_worker_failure_and_a_bad_image_is_not() {
        let runner = DockerRunner::new();
        for daemon in [
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
            "error during connect: ... /var/run/docker.sock: no such file or directory",
        ] {
            assert!(
                matches!(runner.classify("create", daemon), Error::Internal(_)),
                "{daemon:?} is egret's outage and must not fail the author's flow"
            );
        }
        assert!(
            matches!(
                runner.classify("create", "Unable to find image 'nope:1' locally"),
                Error::InvalidInput(_)
            ),
            "an image that does not exist is the author's flow, not egret's outage"
        );
    }
}
