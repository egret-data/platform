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
//! | [`TaskHandler`](super::TaskHandler) | `RunnableTask` | one task **type** — `egret.container`, `egret.script.python` |
//! | [`TaskRunner`] | `io.kestra.core.models.tasks.runners.TaskRunner` | **where** a command runs — a container today, a pod later |
//! | [`TaskCommands`] | `io.kestra.core.models.tasks.runners.TaskCommands` | **what** to run, as data |
//!
//! A handler builds a [`TaskCommands`] and hands it to a [`TaskRunner`]; it
//! never speaks to a daemon itself. That is the seam Kestra has and it is worth
//! having for the same reason: `egret.script.python` and `egret.container` differ only
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
//! 3. **`TaskCommands`** declares **twelve** methods: `getContainerImage`,
//!    `getLogConsumer`, `getInterpreter`, `getBeforeCommands`, `getCommands`,
//!    `getAdditionalVars`, `getWorkingDirectory`, `getOutputDirectory`,
//!    `getEnv`, `getEnableOutputDirectory`, `getTimeout`, `getTargetOS`.
//!    [`TaskCommands`] below is that list minus the parts egret has no seam
//!    for, each named in its own doc. Review Minor 5: this list used to give
//!    ten of the twelve, dropping `getAdditionalVars` and
//!    `getEnableOutputDirectory` without their absence being stated anywhere —
//!    `getAdditionalVars` is Kestra's `{{ workingDir }}` / `{{ outputDir }}`
//!    render vars, which this crate's renderer does not expose to a task, and
//!    `getEnableOutputDirectory` gates the separate output directory named
//!    below.
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
    /// `egret.script.python` writing the author's `script:` to a `.py` file, exactly
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
/// implementation of this and not a second `egret.script.python`.
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
    /// `Err` is a **worker** failure — the daemon is unreachable, egret's own
    /// outage. The run's own outcome is not applied, so it stays `RUNNING` and
    /// leased until [`reap`](crate::worker::reap) returns it. (`tick` keeps
    /// `first_err` and carries on with the rest of the batch — see
    /// `claim::tick`; "aborts the tick" is the loose phrase this crate used and
    /// it overstates what happens to the other runs.)
    ///
    /// Everything the author can act on (a bad image, a command that exited
    /// non-zero) is `Ok` and the handler decides what it means, exactly as
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
/// exist, which `docker rm --force` reports by exiting **0** (measured), so
/// [`DockerRunner::cancel`] treats it as nothing to do — the artifact's own
/// answer (fact 9: `NotFoundException` is caught and silently ignored).
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
    //
    // **There is deliberately no `if result.failure.is_none()` guard around
    // this**, and fix round 1's mutation round is why. There was one; mutating
    // it to `if true` was killed by **nothing** in the whole crate, and the
    // reason is that it could not be: `vars_bytes` only ever grows, so once the
    // cap has fired every later marker re-fires it on its first key and clears
    // the map again before inserting anything. The guard was unobservable, so
    // it went — a guard nobody can see fail is a guard nobody has seen, and
    // this package has refused ten of them.
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

    // **Every component is `lstat`ed and a symlink is refused, leaf or not.**
    // See [`SYMLINK_REFUSAL`] for what this is defending against and why the
    // check is here rather than a `canonicalize` that would resolve the very
    // link it is trying to catch.
    let mut path = staged_root.to_path_buf();
    let mut walked = String::new();
    let mut meta = None;
    for segment in rel.split('/') {
        path.push(segment);
        if !walked.is_empty() {
            walked.push('/');
        }
        walked.push_str(segment);

        // `symlink_metadata` is `lstat`: it describes the entry itself and does
        // NOT follow it. `metadata` here would stat the TARGET, which for
        // `leaked.txt -> /etc/passwd` is the worker host's file, and the check
        // would pass on the very thing it exists to stop.
        let m = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // **A declared output that is not there fails the task.** The
                // alternative is a key missing from `outputs`, which nothing
                // notices until a later `{{ outputs.x.f }}` fails somewhere
                // else with a message about the wrong task.
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
        if m.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map(|t| t.display().to_string())
                .unwrap_or_else(|_| "?".to_string());
            return Err(format!(
                "`outputFiles` declares {rel:?}, and {walked:?} is a SYMLINK (to \
                 {target:?}). {SYMLINK_REFUSAL}"
            ));
        }
        meta = Some(m);
    }
    let meta = meta.expect("`relative_problem` refuses an empty name, so there is a segment");

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

/// What a symlink in the staged output tree means, and why it is refused
/// outright.
///
/// # This is a sandbox escape, measured, and fix round 1 made half of it
///
/// `docker cp` out of a container **recreates a symlink as a symlink** on the
/// host — it does not resolve it and it does not materialise its target.
/// Anything that then opens that path with an ordinary `read`/`stat` is opening
/// it **on the worker host**, outside the container the author was confined to.
/// Measured on this machine (daemon 28.4.0), with
/// `ln -s /etc/passwd leaked.txt` inside an `alpine:3.20` working directory:
///
/// ```text
/// host      /etc/passwd   9344 bytes
/// alpine    /etc/passwd    702 bytes
/// staged    leaked.txt    ->/etc/passwd   (lstat: islink=true)
/// read back                9344 bytes   <- the WORKER HOST's file, as a SUCCESS
/// ```
///
/// Two legs, and they have different histories:
///
/// * **the leaf** — a declared name that is itself a symlink. Present since the
///   handler was written, under both copy strategies;
/// * **an intermediate component** — `outputFiles: ["etcdir/passwd"]` with
///   `ln -s /etc etcdir`. **Introduced by fix round 1**, which replaced the
///   per-file copy with one whole-directory copy: measured, a per-file
///   `docker cp c:<wd>/etcdir/passwd` resolves `etcdir` *inside the container*
///   and yields alpine's 702-byte file, while the directory copy brings
///   `etcdir` over as a host symlink and yields the host's 9344.
///
/// # What the artifact does, and why egret cannot just copy it
///
/// `Docker.downloadOutputFiles` reads the working directory as a **TAR stream**
/// (`copyArchiveFromContainerCmd`) and, per entry, calls
/// `Files.copy(InputStream, Path)` — it materialises the entry's *content* into
/// a regular file. A tar symlink entry carries no content, so Kestra's host
/// filesystem never acquires a symlink and never follows one. The protection is
/// structural rather than a check.
///
/// egret uses `docker cp`, which is the same daemon endpoint without the tar
/// parsing (and without a tar dependency), so it must reach the same end state
/// by refusing instead: `symlink_metadata` on every component, no symlink
/// tolerated anywhere in a declared path.
///
/// # What is left after the fix, stated rather than found later
///
/// The per-file copy resolves an intermediate component **inside the
/// container**, so `outputFiles: ["etcdir/passwd"]` with `ln -s /etc etcdir`
/// now returns *alpine's* `/etc/passwd` as a regular file — the container's
/// own, which the task could have printed anyway. There is no link on the host
/// and nothing for this guard to see, and it **succeeds**.
///
/// That is deliberate and it is not the defect: the boundary that matters is
/// the worker host, and the task already owns its container. The residual is
/// that a declared name can still name a file outside the working directory
/// *within the container*. What Kestra does instead differs **per leg**, and
/// fix round 3 corrects this paragraph, which had one answer for both:
///
/// * **leaf** (`leaked.txt`) — the tar entry for a link carries no content, so
///   `Files.copy(InputStream, Path)` materialises an **empty regular file**,
///   which `Files.walkFileTree` then matches. Kestra returns an empty file.
/// * **intermediate** (`etcdir/passwd`) — there is no such entry to materialise.
///   `FilesService.outputFiles` resolves declared names through
///   `WorkingDir.findAllFilesMatching`, which is a `MatcherFileVisitor` over
///   `Files.walkFileTree` (`LocalWorkingDir`), so an unmatched name yields **no
///   entry at all**. Kestra returns *nothing*, not an empty file.
///
/// Either way both are inside the sandbox and neither is the host, so the
/// judgement is unchanged — only its justification was wrong.
/// `a_symlinked_output_file_cannot_read_the_worker_hosts_files` drives both legs
/// and asserts the host's file appears on neither.
///
/// # Why a refusal rather than "resolve it and check it stays inside"
///
/// Because the check would have to resolve the link to test it, and on the host
/// the resolution **is** the escape — `canonicalize` on `leaked.txt` returns
/// `/etc/passwd` having already left the staging directory. A containment test
/// is also the wrong question: the author's link was written inside the
/// container, where `/etc/passwd` is alpine's; it only became the host's by
/// being carried out. So the honest answer is that a link cannot be carried out
/// at all, and the author names the file it points at.
const SYMLINK_REFUSAL: &str = "A symlink is refused rather than followed: `docker cp` \
     recreates it as a symlink on the WORKER HOST, so reading it would read the host's \
     file rather than the container's, outside the sandbox the task ran in. Name the \
     file itself instead";

/// **egret does NOT classify a docker failure by reading its stderr**, and the
/// whole-branch re-review's Critical is why.
///
/// There were two substring tables here — `DAEMON_ERROR_MARKERS`
/// (`"docker.sock"`, `"no such file or directory"`, `"Cannot connect to the
/// Docker daemon"`, Kestra's own three) and the narrower `DAEMON_SOCKET_MARKERS`
/// — and every failure was classified by testing the daemon's stderr against
/// them. **An author can put text into that stderr**, so an author could choose
/// egret's classification. Proven live on daemon 28.4.0, through the shipping
/// handler:
///
/// ```text
/// outputFiles: ["docker.sock"]
///   → Error response from daemon: Could not find the file
///     /egret/working-dir/docker.sock in container egret-run-…
///   → contains "docker.sock" → Error::Internal → WORKER failure
/// image: "docker.sock/nope:1"
///   → Unable to find image 'docker.sock/nope:1' locally …
///   → contains "docker.sock" → Error::Internal → WORKER failure
/// ```
///
/// Both wedge the run `RUNNING` and leased, blaming a daemon that had just
/// answered three calls in the same run.
///
/// # A THIRD flow, and the measurement that wrongly ruled it out
///
/// My taint table recorded `env` as *"not echoed — the failure names the image,
/// not the env"*. **That row was false**, and how it got there is worth more
/// than the row: I measured a **well-formed** pair (`--env "docker.sock=1"`)
/// against a nonexistent image, so docker failed on the *pull* and never had to
/// parse the env at all. The re-review measured a **malformed** one and the CLI's
/// own flag parser rejects it, echoing the author's string **twice** in one
/// line:
///
/// ```text
/// docker create -e "=docker.sock" alpine:3.20 true
///   invalid argument "=docker.sock" for "-e, --env" flag:
///     invalid environment variable: =docker.sock      exit 125
/// ```
///
/// `env` is author-supplied and unvalidated (`handlers::container::Spec::env`),
/// so that is a third tainted flow into the old classification. **I asked a
/// narrower question than the one the rule needs**: "does this value appear in
/// stderr?" answered for the input I happened to choose, where the rule needs
/// "can this value *ever* appear?" — and a single well-formed example cannot
/// answer that.
///
/// **The structural fix closes it anyway, and that is the argument for having
/// made it structural.** Sanitising the value the reviewer reported would have
/// left this one live and undiscovered; a classification that reads no stderr at
/// all is not a list of values to sanitise, so a flow nobody enumerated cannot
/// defeat it. The enumeration is how you know what you fixed; it is not what
/// does the fixing.
///
/// **Sanitising the one value was rejected.** The suggested one-liner
/// (`stderr.replace(rel, "")`) fixes `outputFiles` and leaves `image:`, which is
/// the second flow — and it leaves the *shape*: an author's string being tested
/// against markers at all. So the classification no longer reads the failing
/// call's stderr. It **asks the daemon**:
/// [`DockerRunner::daemon_answers`] runs `docker version --format
/// {{.Server.Version}}`, whose command line and output contain **no author
/// input of any kind**.
///
/// # Why this diverges from the artifact — narrowed to what was established
///
/// `Docker.isDockerSocketAccessError` *is* a substring test over exactly those
/// three strings, applied to a `Throwable`.
///
/// **The claim that used to stand here was too wide.** It said Kestra is safe
/// because it tests "the client's own `DockerException` … never a message built
/// from the user's declared paths". That is what I established, and it is true
/// of the **copy** path only. Re-checked with `javap` on the pinned image, the
/// one call site sits at bytecode offset **1723** of `Docker.run`, and
/// **`pullImage` is called at offset 412 of the same method** — inside the same
/// `catch` — so an exception carrying the author's **`image:`** reaches that
/// substring test too. Kestra has one of egret's two tainted flows upstream of
/// where I looked.
///
/// So the honest statement is narrower and the conclusion is unchanged: the
/// artifact's test is unsafe **for any value the user supplies that can reach
/// the exception message**, egret's shelling out puts more such values there,
/// and a probe removes the question rather than bounding it.
///
/// The cost is one extra `docker version` on the failure path only.
///
/// [`DockerRunner::daemon_answers`]: DockerRunner::daemon_answers
const _CLASSIFICATION_IS_A_PROBE_NOT_A_SUBSTRING_TEST: () = ();

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
/// its message text. It does **not** inherit the artifact's substring
/// classification, though: [`_CLASSIFICATION_IS_A_PROBE_NOT_A_SUBSTRING_TEST`]
/// records why an author could otherwise choose egret's own error type, and
/// [`DockerRunner::daemon_answers`] is what replaced it.
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
            // **`kill_on_drop`, so a dropped future does not orphan a `docker`
            // process** — the re-review's NEW Minor 2. `timeout::guard` drops
            // the whole future when a task-level budget expires, and without
            // this the `docker cp`/`inspect` in flight at that moment keeps
            // running with nobody waiting on it. The leak test made it visible
            // by leaving a `sleep 300` behind on every run; the production case
            // is the same shape.
            .kill_on_drop(true)
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
    async fn classify(&self, what: &str, stderr: &str) -> Error {
        let stderr = stderr.trim();
        if !self.daemon_answers().await {
            return Error::Internal(anyhow::anyhow!(
                "the Docker daemon could not be reached, so `{what}` did not run: {stderr}"
            ));
        }
        Error::InvalidInput(format!("docker {what}: {stderr}"))
    }

    /// Ask the daemon whether it is there — **the classification's only input,
    /// and one no author can reach.**
    ///
    /// `docker version --format {{.Server.Version}}` and not `docker --version`:
    /// the second prints the CLI's own version without talking to anything, so
    /// it is green on a machine with a client and no daemon. The `--format`
    /// asks for a **server** field, so it fails unless the server answered.
    ///
    /// Neither this command line nor its output contains any author-supplied
    /// value, which is the whole point: see
    /// [`_CLASSIFICATION_IS_A_PROBE_NOT_A_SUBSTRING_TEST`].
    ///
    /// # Its cost, stated rather than discovered
    ///
    /// This runs **once per failed docker call**, and in
    /// [`collect_output_files`](DockerRunner::collect_output_files) that loop is
    /// over `outputFiles`, which the author writes and **nothing caps**. So a
    /// flow declaring a thousand absent files makes a thousand extra `docker
    /// version` calls in one run — bounded work per element and no daemon state,
    /// but not free.
    ///
    /// It is left uncapped deliberately: capping the probe means caching "the
    /// daemon is up" across a loop that can outlive the daemon, which trades a
    /// known cost for a wrong answer. The list itself is what should be bounded,
    /// and that is a `spec::validate` rule rather than a runner's — recorded
    /// here so whoever adds it knows this is one of the reasons.
    async fn daemon_answers(&self) -> bool {
        self.cli(&["version", "--format", "{{.Server.Version}}"])
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
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
            return Err(self
                .classify("create", &String::from_utf8_lossy(&created.stderr))
                .await);
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
        // **There is deliberately no "No such container" branch**, and the
        // whole-branch review's Minor 2 is why: `docker rm --force` on a name
        // that does not exist exits **0** (measured, daemon 28.4.0), so that
        // branch was unreachable — a guard nobody could see fire, which is the
        // shape this package has refused throughout. The absent container is
        // therefore handled by the success arm above, which is also the
        // artifact's answer (`Docker.kill` catches `NotFoundException` and
        // returns without logging).
        Err(self
            .classify("rm", &String::from_utf8_lossy(&out.stderr))
            .await)
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
            // **[`StagingDir`] removes itself however this scope ends** —
            // including when `timeout::guard` drops the whole future mid-copy,
            // which is the path an explicit cleanup could never cover (review
            // Minor 3). Review Minor 2's `?`-before-cleanup leak is closed by
            // the same thing, so the closure that used to force a single exit
            // is gone with it.
            // # `input_files` KEYS ARE NOT AUTHOR-SUPPLIED TODAY, AND THE NEXT
            // # HANDLER AUTHOR MUST KEEP IT THAT WAY
            //
            // The `create_dir_all(...).map_err(io_err)?` below is the **closed
            // Critical's exact shape**: an author-derived path handed to the
            // filesystem, with an `io::Error` becoming `Error::Internal` and
            // therefore a worker failure. It is unreachable only because **no
            // handler lets an author name a key**: `handlers::python` writes the
            // single constant `SCRIPT_FILE` and `handlers::container` passes an
            // empty map. Nothing in the type system says so — `TaskCommands` is
            // `pub` and its `input_files` is a plain `BTreeMap`.
            //
            // A handler that ever maps an author's value into a key here
            // reintroduces the Critical in its original form. It should route
            // through [`stage_destination`], whose `String` error cannot express
            // a worker failure, exactly as the output path does.
            let staging = tempdir(task)?;
            for (rel, contents) in &commands.input_files {
                check_relative(rel)?;
                let path = staging.path().join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(io_err)?;
                }
                std::fs::write(&path, contents).map_err(io_err)?;
            }
            let src = format!("{}/.", staging.path().display());
            let dst = format!("{name}:{WORKING_DIR}");
            let out = self.cli(&["cp", &src, &dst]).await?;
            if !out.status.success() {
                return Err(self
                    .classify("cp", &String::from_utf8_lossy(&out.stderr))
                    .await);
            }
        }

        // ---- start, and stream --------------------------------------------
        let mut child = Command::new(&self.binary)
            .args(["start", "--attach", name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The attach is the long one, so it is the one most likely to be
            // dropped mid-flight. `kill_on_drop` reaps the CLI; the container
            // itself is the daemon's and is stopped by `cancel`.
            .kill_on_drop(true)
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
            return Err(self
                .classify("inspect", &String::from_utf8_lossy(&inspected.stderr))
                .await);
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
            self.collect_output_files(name, commands, staging.path(), &mut result)
                .await?;
        }

        Ok(result)
    }

    /// Copy **only the declared files** out, then resolve each one with
    /// [`ingest_output_file`].
    ///
    /// # Fix round 2 moved this back to a per-file copy, and that is the fix
    ///
    /// Fix round 1 replaced N per-file copies with one whole-directory copy, on
    /// the reasoning that it is what `Docker.run` does and that it lets
    /// [`ingest_output_file`] answer *"is the author's file there?"* from the
    /// filesystem instead of from a daemon error string. The second half was
    /// right and is kept. The first half was wrong twice:
    ///
    /// * **it moved symlink resolution from inside the container to the worker
    ///   host**, which is [`SYMLINK_REFUSAL`]'s intermediate-component leg — a
    ///   sandbox escape that per-file copies do not have, because
    ///   `docker cp c:<wd>/etcdir/passwd` resolves `etcdir` container-side;
    /// * **it copied the whole working directory regardless of what was
    ///   declared** — measured, 64 MiB of undeclared scratch in 2.66s. The
    ///   output caps bound what reaches Postgres; nothing bounded what reached
    ///   the worker's disk.
    ///
    /// And the appeal to the artifact was only half true: `downloadOutputFiles`
    /// does copy the whole directory, but as a **tar stream it materialises**
    /// (`Files.copy(InputStream, Path)` per entry), never as a filesystem copy —
    /// which is exactly why Kestra's host never ends up holding a symlink. The
    /// shape mattered more than the granularity, and the granularity was the
    /// part that got copied.
    ///
    /// # The daemon-free property is preserved, deliberately
    ///
    /// The copy's exit status is **ignored except to ask the daemon whether it
    /// is still there**. A file that did not copy simply is not in the staging
    /// tree, and `ingest_output_file` says so from `symlink_metadata` — so the
    /// merge condition still holds: the missing file, the directory, the
    /// symlink and both caps are all decided with no daemon. Nothing about the
    /// author's declared name is read out of stderr any more; the only input to
    /// the worker-versus-task decision is
    /// [`daemon_answers`](DockerRunner::daemon_answers).
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
        let mut total = 0usize;
        for (i, rel) in commands.output_files.iter().enumerate() {
            // The name is vetted BEFORE it is interpolated into a `docker cp`
            // argument, so `..` never reaches the daemon and the staged path
            // cannot land outside `staging`.
            if let Some(problem) = relative_problem(rel) {
                result.failure = Some(problem);
                return Ok(());
            }
            // **One private root per declared name** — see
            // [`stage_destination`] for the whole argument. Nothing an author
            // declares can reach another declared name's staging tree.
            let root = staging.join(i.to_string());
            let dst = match stage_destination(&root, rel) {
                Ok(dst) => dst,
                Err(problem) => {
                    result.failure = Some(problem);
                    return Ok(());
                }
            };
            let src = format!("{name}:{WORKING_DIR}/{rel}");
            let dst_s = dst.display().to_string();
            let out = self.cli(&["cp", &src, &dst_s]).await?;
            if !out.status.success() && !self.daemon_answers().await {
                // **The daemon is asked, not the stderr.** `docker cp` echoes
                // the author's own path into its error message, so a declared
                // `outputFiles: ["docker.sock"]` used to be read as the socket
                // being unreachable and wedged the run. See
                // [`_CLASSIFICATION_IS_A_PROBE_NOT_A_SUBSTRING_TEST`].
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return Err(Error::Internal(anyhow::anyhow!(
                    "the Docker daemon could not be reached, so {rel:?} could not be \
                     copied back out of {name}: {stderr}"
                )));
            }
            // A copy that failed while the daemon is up is left to the
            // filesystem: the file is simply not staged, and
            // `ingest_output_file` produces the author-facing message.
            if let Err(problem) = ingest_output_file(
                result,
                &root,
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

/// Make the staging destination for **one** declared output name, under a root
/// that is private to that name.
///
/// # The general statement, which is what the whole-branch review asked for
///
/// **Nothing computed from an author's `outputFiles` string may produce a
/// worker failure.** Only egret's own environment may — the `docker` binary not
/// being spawnable, and the daemon socket not answering. That is the class, and
/// it has now been violated twice from two different code paths:
///
/// * Task 5 review's Critical 1 — `fs::read` on a declared **directory** gave
///   `Error::Internal`, so `outputFiles: ["a-dir"]` aborted the tick;
/// * the whole-branch review's Critical — `create_dir_all(staging/a)` gave
///   `AlreadyExists` when the container wrote a **regular file** `a` and the
///   author declared `["a", "a/b.txt"]`, so that flow left the run `RUNNING` and leased with
///   no outcome applied, and repeated for ever at every reap. Measured:
///   `File exists (os error 17)`.
///
/// The second was reintroduced by fix round 2 restoring the per-file copy, which
/// is why patching the measured case is not the fix: the *shape* keeps coming
/// back. Two things enforce the class instead of a branch:
///
/// 1. **This function's error type is `String`, not [`Error`]** — the same
///    discipline [`ingest_output_file`] already has (it contains zero
///    `Error::Internal`). An author's path is now handled end to end by
///    functions that *cannot* express a worker failure, so the classification
///    happens once, at the type, rather than at each `?`.
/// 2. **One private root per declared name.** `staging/<i>/<rel>`, so no
///    declared name can create, occupy or block a path another declared name
///    needs. `["a", "a/b.txt"]` cannot collide because they never share a tree.
///
/// # Exactly what the type-level half covers, which is less than it used to say
///
/// This sentence used to read *"the only `?` left on the author's route … Both
/// are egret's environment"*, and the re-review is right that it was **false as
/// written**: [`DockerRunner::collect_output_files`] returns
/// `Result<(), Error>` and builds an `Error::Internal` inline, so the route is
/// not type-proof end to end. A rule this task wrote and this task falsified —
/// the category I named last round.
///
/// Stated precisely instead:
///
/// * **[`stage_destination`] and [`ingest_output_file`] cannot express a worker
///   failure**, by their signatures — `Result<_, String>`. Everything decided
///   *about the author's declared name* happens in those two.
/// * **[`DockerRunner::collect_output_files`] can**, and does, for exactly one
///   reason: the daemon stopped answering. Its `Error` values are now built
///   solely from [`daemon_answers`](DockerRunner::daemon_answers), which takes
///   no author input — so an author still cannot *cause* one, which is the
///   property the class actually needs. The type does not carry that claim; the
///   probe does.
fn stage_destination(
    root: &std::path::Path,
    rel: &str,
) -> std::result::Result<std::path::PathBuf, String> {
    let dst = root.join(rel);
    // **Parents are created here, by egret, as real directories.** That is the
    // other half of the intermediate-component defence: nothing the container
    // wrote can become a path component of the staged tree.
    // `ingest_output_file` still `lstat`s every component, so the guard is one
    // a test can plant a link under and watch fail.
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "`outputFiles` declares {rel:?}, and egret could not make a place to \
                 copy it into: {e}"
            )
        })?;
    }
    Ok(dst)
}

/// A staging directory that removes itself **however** its scope ends —
/// including when the future holding it is dropped.
///
/// The whole-branch review's Minor 3, and it is cross-task in the way that makes
/// it easy to miss: `worker::timeout::guard` enforces a task-level `timeout:`
/// by **dropping the future** (Task 2), and the directory is mine (Task 5). An
/// explicit `remove_dir_all` after the `await` never runs on that path, so a
/// timed-out `egret.script.python` left `$TMPDIR/egret-workflow-*` behind
/// holding the author's **rendered `script:`** — which may carry whatever a
/// `{{ … }}` resolved to, in a directory that outlives the run.
///
/// `Drop` is the fix rather than another cleanup call, because the whole point
/// is that no call site runs: dropping the future drops its locals, and this
/// runs then. `remove_dir_all`'s result is discarded for the reason every
/// destructor discards one — there is nobody left to tell, and the run has
/// already failed on its budget.
struct StagingDir(std::path::PathBuf);

impl StagingDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A private staging directory for one run's file copies.
///
/// Named after the run so two concurrent runs cannot collide, and created under
/// the process temp dir rather than in the repo.
fn tempdir(task: &TaskInput) -> Result<StagingDir> {
    // **0700 and unpredictable, and review Minor 3 is only half the reason.**
    // The stated half: this directory holds the author's rendered `script:`,
    // which may carry whatever a `{{ … }}` resolved to, and 0755 under a
    // typical umask makes that world-readable on a shared host.
    //
    // The half that makes it a correctness issue and not only a disclosure one:
    // the name used to be a pure function of `(run_id, claimed_at)`, both of
    // which an author can see, in a world-writable directory. Anyone able to
    // guess it could pre-create it **as a symlink to a directory** and
    // `create_dir_all` would happily succeed through it — putting the staging
    // tree, and every `docker cp` destination under it, somewhere else. A
    // random component removes the guess and `create_dir` (not `create_dir_all`)
    // refuses a path that already exists, so a pre-created anything is an error
    // rather than a silent redirection.
    let dir = std::env::temp_dir().join(format!(
        "egret-workflow-{}-{}",
        task.run_id.simple(),
        uuid::Uuid::new_v4().simple()
    ));
    create_private_dir(&dir)?;
    Ok(StagingDir(dir))
}

/// Create one directory, private to this user, refusing a path that is already
/// there.
///
/// **Split out of [`tempdir`] so the properties can be asserted against THIS
/// code.** Fix round 3's NEW Minor 1: the test used to check that
/// `std::fs::DirBuilder::new().create(existing)` errors, which is a fact about
/// `std` — it passed with this implementation reverted to `create_dir_all`, and
/// a test that passes with the implementation reverted is not a test. It cannot
/// be asserted through `tempdir` directly either, because `tempdir`'s whole
/// point is that its path is random and therefore never already there.
///
/// `create_dir` and not `create_dir_all`: the recursive form **succeeds** on a
/// path that already exists, including one an attacker pre-created as a symlink
/// to a directory, which would silently relocate every `docker cp` destination
/// underneath it.
fn create_private_dir(dir: &std::path::Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    // Deliberately NOT `.recursive(true)`.
    builder.create(dir).map_err(io_err)
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
    // `.` and `./x` and a trailing `/` are how a declared name asks for a
    // DIRECTORY, and `docker cp c:<wd>/. dst` copies the whole subtree before
    // anything can refuse it — fix round 3's NEW Minor 2. The refusal for a
    // directory that is only discovered after the copy still happens in
    // `ingest_output_file`; this closes the spellings that ask for one by name.
    if rel.split('/').any(|seg| seg == ".") || rel.ends_with('/') {
        return Some(format!(
            "{rel:?} names a DIRECTORY rather than a file. An output is JSON and egret \
             has no internal storage to hand back an archive URI for (W9 owns it), so \
             name the files inside it instead"
        ));
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
        // A name that ASKS for a directory is refused before any copy happens
        // -- fix round 3's NEW Minor 2, because `docker cp c:<wd>/. dst` would
        // otherwise put the whole subtree on the worker's disk first.
        for (bad, needle) in [
            (".", "DIRECTORY"),
            ("./out.txt", "DIRECTORY"),
            ("sub/./out.txt", "DIRECTORY"),
            ("sub/", "DIRECTORY"),
        ] {
            let err = check_relative(bad).expect_err("a directory spelling must be refused");
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

        // **And a later MARKER must not refill what the cap just emptied.**
        // Fix round 1's mutation round found this leg missing: the guard that
        // stops collecting once the cap has fired survived its mutation,
        // because the only line sent after the flood was an ordinary one. A
        // cap that clears the vars and then lets the next marker repopulate
        // them bounds nothing at all.
        ingest_line(
            &mut r,
            &mut bytes,
            32,
            r#"::{"outputs":{"sneaked":"in"}}::"#,
            false,
        );
        assert!(
            r.vars.is_empty(),
            "a marker after the cap fired must contribute nothing: {:?}",
            r.vars
        );
        assert_eq!(
            r.stdout_line_count, 2,
            "it is still counted and still logged -- only its payload is dropped"
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

    /// **A declared output that is a SYMLINK is refused, not followed.**
    ///
    /// Fix round 2's Critical. `docker cp` recreates a symlink as a symlink on
    /// the worker host, so following it reads the HOST's file — outside the
    /// container the task was confined to. The reviewer measured it end to end:
    /// `ln -s /etc/passwd leaked.txt` returned the worker's 9344-byte
    /// `/etc/passwd` as a **successful** task output, where alpine's own is 702.
    ///
    /// This plants a real symlink rather than a regular file, which is the
    /// review's caveat applied: `ingest_output_file` traded daemon-dependence
    /// for host-filesystem-dependence, and a test that only ever writes regular
    /// files cannot see what the filesystem does with a link.
    #[test]
    fn a_declared_output_that_is_a_symlink_is_refused_rather_than_followed() {
        let dir = staging("symlink-leaf");
        // Something outside the staging tree, with content this test can
        // recognise: if the guard ever regresses, the assertion below sees the
        // secret rather than an absence.
        let outside = dir.parent().expect("a parent").join(format!(
            "egret-w4t5-secret-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&outside, "SECRET-FROM-OUTSIDE-THE-STAGING-TREE").expect("write");
        std::os::unix::fs::symlink(&outside, dir.join("leaked.txt")).expect("symlink");

        let mut r = CommandsResult::default();
        let mut total = 0usize;
        let err = ingest_output_file(&mut r, &dir, "leaked.txt", 1_000_000, 1_000_000, &mut total)
            .expect_err("a symlink must be refused");

        assert!(err.contains("SYMLINK"), "{err}");
        assert!(
            err.contains("leaked.txt"),
            "the declared name must be named: {err}"
        );
        assert!(
            err.contains("WORKER HOST"),
            "the refusal must say what the hazard is: {err}"
        );
        assert!(
            r.output_files.is_empty(),
            "nothing may be read out of a symlink"
        );
        assert_eq!(
            total, 0,
            "and it must not be counted against the caps either"
        );

        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **An INTERMEDIATE component that is a symlink is refused too.**
    ///
    /// The second leg, and the one fix round 1 introduced by copying the whole
    /// working directory: `outputFiles: ["etcdir/passwd"]` with
    /// `ln -s /etc etcdir`. A guard that only checked the leaf would pass this,
    /// which is the shape this programme keeps shipping — so it is asserted
    /// separately rather than assumed to follow from the leaf case.
    #[test]
    fn a_symlinked_intermediate_component_is_refused_as_well_as_a_symlinked_leaf() {
        let dir = staging("symlink-mid");
        let outside = dir
            .parent()
            .expect("a parent")
            .join(format!("egret-w4t5-etc-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&outside).expect("mkdir");
        std::fs::write(
            outside.join("passwd"),
            "SECRET-VIA-AN-INTERMEDIATE-COMPONENT",
        )
        .expect("write");
        std::os::unix::fs::symlink(&outside, dir.join("etcdir")).expect("symlink");

        let mut r = CommandsResult::default();
        let mut total = 0usize;
        let err = ingest_output_file(
            &mut r,
            &dir,
            "etcdir/passwd",
            1_000_000,
            1_000_000,
            &mut total,
        )
        .expect_err("a symlinked path COMPONENT must be refused");

        assert!(err.contains("SYMLINK"), "{err}");
        assert!(
            err.contains("\"etcdir\""),
            "the offending COMPONENT must be named, not just the declared path: {err}"
        );
        assert!(r.output_files.is_empty());

        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Even a symlink that points back INSIDE the staging tree is refused**,
    /// and a dangling one is refused as a symlink rather than reported missing.
    ///
    /// Both directions matter. The first says the rule is "no links", not "no
    /// links that escape" — a containment test would have to resolve the link
    /// to judge it, and on the host the resolution IS the escape. The second
    /// says the refusal is decided by `lstat` and not by whether the target
    /// happens to exist, so a link planted at a moment when its target is
    /// absent cannot slip through and be followed later.
    #[test]
    fn a_symlink_is_refused_even_pointing_inside_the_tree_or_at_nothing() {
        let dir = staging("symlink-inside");
        std::fs::write(dir.join("real.txt"), "legitimate").expect("write");
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("alias.txt")).expect("symlink");
        std::os::unix::fs::symlink(dir.join("gone.txt"), dir.join("dangling.txt"))
            .expect("symlink");

        let mut r = CommandsResult::default();
        let mut total = 0usize;

        let err = ingest_output_file(&mut r, &dir, "alias.txt", 1_000_000, 1_000_000, &mut total)
            .expect_err("a link inside the tree is still a link");
        assert!(err.contains("SYMLINK"), "{err}");

        let err = ingest_output_file(
            &mut r,
            &dir,
            "dangling.txt",
            1_000_000,
            1_000_000,
            &mut total,
        )
        .expect_err("a dangling link is refused as a link");
        assert!(
            err.contains("SYMLINK"),
            "a dangling link must not be reported as a missing file -- `lstat` sees \
             the link whatever its target does: {err}"
        );

        // The file the link pointed at is nameable directly, so the refusal is
        // about the link and not about the content being unreachable.
        ingest_output_file(&mut r, &dir, "real.txt", 1_000_000, 1_000_000, &mut total)
            .expect("the real file is a fine output");
        assert_eq!(
            r.output_files.get("real.txt").map(String::as_str),
            Some("legitimate")
        );

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

    /// **Only the DECLARED files are copied out of the container.**
    ///
    /// Fix round 2's NEW Important 1: fix round 1 copied the whole working
    /// directory regardless of what was declared, and the reviewer measured
    /// 64 MiB of undeclared scratch reaching the worker's disk in 2.66s. The
    /// output caps bound what reaches Postgres; nothing bounded that.
    ///
    /// Asserted on the **argv the runner actually issues**, through the
    /// `with_binary` seam, so it needs no daemon and cannot pass by the copy
    /// happening to be small. A run that copied `…/working-dir/.` would put that
    /// path in the log and fail here.
    #[tokio::test]
    async fn only_the_declared_output_files_are_copied_out_of_the_container() {
        let dir = staging("argv");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        // A `docker` that records what it was asked and fakes just enough:
        // `create` prints an id, `start` prints nothing, `inspect` prints 0,
        // and `cp` materialises a file at the destination so the pure seam has
        // something to ingest.
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> {log}\n\
                 case \"$1\" in\n\
                 \x20 create) echo fake-container-id ;;\n\
                 \x20 inspect) echo 0 ;;\n\
                 \x20 cp) case \"$3\" in *?) printf 'staged' > \"$3\" ;; esac ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string());
        let task = leased(1_700_000_000);
        let out = runner
            .run(
                &task,
                &TaskCommands {
                    container_image: "alpine:3.20".into(),
                    commands: vec!["true".into()],
                    output_files: vec!["wanted.txt".into(), "nested/also.txt".into()],
                    ..TaskCommands::default()
                },
                &super::super::handlers::TracingSink,
            )
            .await
            .expect("the shim is a well-behaved daemon");
        assert_eq!(out.failure, None, "{out:?}");
        assert_eq!(out.output_files.len(), 2, "{out:?}");

        let argv = std::fs::read_to_string(&log).expect("the shim logged");
        let cps: Vec<&str> = argv
            .lines()
            .filter(|l| l.starts_with("cp ") && l.contains(':'))
            .collect();
        assert_eq!(
            cps.len(),
            2,
            "one copy per DECLARED file, and no others: {argv}"
        );
        assert!(
            cps.iter()
                .any(|l| l.contains("/egret/working-dir/wanted.txt"))
                && cps
                    .iter()
                    .any(|l| l.contains("/egret/working-dir/nested/also.txt")),
            "each copy names its declared file: {argv}"
        );
        assert!(
            !argv.contains("/egret/working-dir/."),
            "the WHOLE working directory must not be copied -- that is what put \
             64 MiB of undeclared scratch on the worker's disk, and what moved \
             symlink resolution onto the host: {argv}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The staging directory is private and unguessable.**
    ///
    /// Review Minor 3, and the leg that makes it more than a disclosure issue:
    /// the name used to be a pure function of `(run_id, claimed_at)` in a
    /// world-writable directory, so it could be pre-created as a symlink to
    /// somewhere else and `create_dir_all` would follow it — putting every
    /// `docker cp` destination under an attacker's path.
    #[test]
    fn the_staging_directory_is_private_and_not_predictable() {
        let task = leased(1_700_000_000);
        let a_guard = tempdir(&task).expect("staging");
        let b_guard = tempdir(&task).expect("staging");
        let a = a_guard.path().to_path_buf();
        let b = b_guard.path().to_path_buf();
        assert_ne!(
            a, b,
            "two runs of the SAME task must not agree on the path -- a guessable \
             name in a world-writable directory can be pre-created as a symlink"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).expect("stat").permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "the author's rendered `script:` is written here, so it is not \
                 world-readable: got {mode:o}"
            );
        }

        // **And an existing path is an error rather than a silent reuse —
        // asserted against `create_private_dir`, which is THIS code.**
        // Fix round 3's NEW Minor 1: this leg used to call
        // `std::fs::DirBuilder::new().create(..)` itself, which is a fact about
        // `std` and passed with `tempdir` reverted to `create_dir_all`.
        assert!(
            create_private_dir(&a).is_err(),
            "`create_dir` must refuse a path that is already there -- the recursive \
             form succeeds, including through a symlink somebody pre-created"
        );

        // The same property where it actually bites: a directory standing in
        // for the staging path must not be adopted.
        let squatted = a.parent().expect("a parent").join(format!(
            "egret-w4t5-squat-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&squatted).expect("pre-create");
        assert!(
            create_private_dir(&squatted).is_err(),
            "a path somebody else created first must be refused, not reused"
        );
        let _ = std::fs::remove_dir_all(&squatted);

        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    /// **A parent and a child declared together is a TASK failure, never a
    /// worker one.**
    ///
    /// The whole-branch review's Critical: `outputFiles: ["a", "a/b.txt"]`
    /// where the container writes a regular file `a`. Staging `a` first left a
    /// regular file where `a/b.txt`'s parent had to go, `create_dir_all` gave
    /// `AlreadyExists` and `io_err` made it `Error::Internal` — so no outcome
    /// was applied and the run stayed `RUNNING` and leased, unclaimable for an
    /// hour, then repeating for ever. (`tick` keeps going with the rest of the
    /// batch; it is this run that is lost. See `claim::tick`.) Measured live: `File exists (os error 17)`.
    ///
    /// The reviewer's finding was that **no test declared a parent and a child
    /// together**. This one does, with no daemon: the shim fails the `cp` for
    /// the child exactly as Docker does when `a` is a file, so the run reaches
    /// the same state by the same route.
    ///
    /// `expect` on the `Result` **is** the assertion — an `Err` here is the bug.
    #[tokio::test]
    async fn a_parent_and_a_child_declared_together_never_wedges_the_run() {
        let dir = staging("parent-child");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> {log}\n\
                 case \"$1\" in\n\
                 \x20 create) echo fake-container-id ;;\n\
                 \x20 inspect) echo 0 ;;\n\
                 \x20 cp) case \"$2\" in\n\
                 \x20        *a/b.txt) echo 'Error response from daemon: not a directory' >&2; exit 1 ;;\n\
                 \x20        *) printf 'regular-file' > \"$3\" ;;\n\
                 \x20      esac ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string());
        let out = runner
            .run(
                &leased(1_700_000_000),
                &TaskCommands {
                    container_image: "alpine:3.20".into(),
                    commands: vec!["true".into()],
                    output_files: vec!["a".into(), "a/b.txt".into()],
                    ..TaskCommands::default()
                },
                &super::super::handlers::TracingSink,
            )
            .await
            .expect(
                "an author declaring a parent and a child must never wedge the run -- \
                 a worker failure here applies no outcome, so the run stays RUNNING and \
                 leased for an hour, then repeats for ever",
            );

        let why = out
            .failure
            .clone()
            .expect("the child cannot be produced, so the task fails");
        assert!(
            why.contains("a/b.txt"),
            "the failure must name the child, not egret's plumbing: {why}"
        );
        assert!(
            !why.contains("File exists") && !why.contains("os error 17"),
            "and it must not be a staging-collision message -- each declared name \
             gets its own root precisely so they cannot collide: {why}"
        );
        assert_eq!(
            out.output_files.get("a").map(String::as_str),
            Some("regular-file"),
            "the parent still came back; only the child failed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Two declared names never share a staging tree — asserted through the
    /// wiring, not beside it.**
    ///
    /// The re-review's NEW Minor 1: this test used to call `stage_destination`
    /// twice with roots it chose itself, so it **passed with the per-name root
    /// removed from `collect_output_files`** — it never touched the wiring it
    /// was named for. A test that passes with the thing it names deleted is the
    /// defect this programme opens with.
    ///
    /// It now drives the real `collect_output_files` through the shim and reads
    /// the destinations the runner actually issued, so removing the per-name
    /// root makes it fail.
    #[tokio::test]
    async fn each_declared_output_gets_a_staging_root_of_its_own() {
        let dir = staging("roots");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                 \x20 create) echo id ;;\n\
                 \x20 version) exit 0 ;;\n\
                 \x20 inspect) echo 0 ;;\n\
                 \x20 cp) echo \"$3\" >> {log}; printf x > \"$3\" ;;\n\
                 esac\nexit 0\n",
                log = log.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string());
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["true".into()],
            output_files: vec!["a".into(), "a/b.txt".into()],
            ..TaskCommands::default()
        };
        let sink = super::super::handlers::TracingSink;
        runner
            .run(&leased(1_700_000_000), &cmds, &sink)
            .await
            .expect("no worker failure");

        let logged = std::fs::read_to_string(&log).expect("the shim logged");
        let dests: Vec<&str> = logged
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(dests.len(), 2, "one copy per declared name: {dests:?}");
        let root_of = |d: &str| {
            // `<staging>/<i>/<rel>` — the root is the component before `rel`.
            let p = std::path::Path::new(d);
            let depth = p.components().count();
            p.components()
                .take(depth.saturating_sub(1))
                .collect::<std::path::PathBuf>()
        };
        assert_ne!(
            root_of(dests[0]),
            root_of(dests[1])
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default(),
            "the two names must not be staged under one root: {dests:?}"
        );
        assert!(
            dests[0] != dests[1] && !dests[1].starts_with(dests[0]),
            "`a/b.txt` must not be staged underneath `a` -- that is the collision \
             the Critical came from: {dests:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A dropped future leaves no staging directory behind.**
    ///
    /// The whole-branch review's Minor 3, and it is cross-task: `timeout::guard`
    /// enforces a task-level `timeout:` by **dropping the future** (Task 2), and
    /// the directory holding the author's rendered `script:` is Task 5's. An
    /// explicit cleanup after the `await` cannot run on that path, so
    /// [`StagingDir`] removes itself on `Drop`.
    ///
    /// # This test counted a GLOBAL directory once, and the full suite caught it
    ///
    /// The first version counted `$TMPDIR/egret-workflow-*` before and after,
    /// which is how the reviewer measured the leak by hand. It passed alone and
    /// **failed in `cargo test --workspace`** — `left: 66, right: 65` — because
    /// the lib tests run in parallel and a *different* test's staging directory
    /// appeared between the two counts. A global counter is not an observation
    /// of this run.
    ///
    /// So the shim reports the path it was handed, and the assertion is about
    /// **that** directory: immune to whatever else is running, and a stronger
    /// claim than a count besides.
    #[tokio::test]
    async fn a_dropped_run_future_leaves_no_staging_directory_behind() {
        let dir = staging("drop-leak");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        // `cp` records the staging path it was given and then never returns, so
        // the future is guaranteed to be dropped inside the window between
        // creating that directory and removing it.
        //
        // **`exec sleep`, and the word is load-bearing.** `kill_on_drop` reaps
        // the process this crate spawned — the shim's `/bin/sh` — and a plain
        // `sleep 300` is that shell's *child*, which survives it and reparents
        // to init. Measured at the previous HEAD: 0 orphans before the test, 1
        // after, on every run. `exec` replaces the shell with the sleep so
        // there is one process and `kill_on_drop` reaches it. The production
        // path is not affected either way (nothing egret spawns forks a child),
        // which is why this was the test leaking and not the runner.
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                 \x20 create) echo id ;;\n\
                 \x20 cp) echo \"$2\" >> {log}; exec sleep 300 ;;\n\
                 \x20 inspect) echo 0 ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string());
        let task = leased(1_700_000_000);
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["true".into()],
            input_files: std::collections::BTreeMap::from([(
                "main.py".to_string(),
                "SECRET-RENDERED-SCRIPT".to_string(),
            )]),
            ..TaskCommands::default()
        };
        let sink = super::super::handlers::TracingSink;
        let fut = runner.run(&task, &cmds, &sink);
        // Exactly what `timeout::guard` does to a handler that outruns its
        // budget: drop the future.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
                .await
                .is_err(),
            "the shim hangs, so the future must still have been running"
        );

        let logged = std::fs::read_to_string(&log)
            .expect("the shim was reached, so it logged the staging path");
        let src = logged
            .lines()
            .next()
            .expect("one `cp` happened")
            .trim()
            .to_string();
        let staged = src
            .strip_suffix("/.")
            .expect("the input-file copy is `<staging>/.`");
        assert!(
            staged.contains("egret-workflow-"),
            "sanity: {staged} should be a staging directory"
        );
        assert!(
            !std::path::Path::new(staged).exists(),
            "dropping the future left {staged} behind, holding the author's \
             rendered `script:` -- an explicit cleanup after the `await` never \
             runs on this path, which is why `StagingDir` uses `Drop`"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **An author cannot choose egret's error type by spelling a marker.**
    ///
    /// The whole-branch re-review's Critical. `docker cp` and `docker create`
    /// echo the author's own strings into stderr, and the classification used
    /// to be `stderr.contains("docker.sock")` — so `outputFiles: ["docker.sock"]`
    /// **and** `image: "docker.sock/nope:1"` both became worker failures,
    /// wedging the run while the daemon was answering. Proven live on both
    /// flows before the fix.
    ///
    /// The classification asks the daemon now, so this drives the daemon's
    /// answer (via the shim's `version`) and puts every marker string in the
    /// stderr, both ways round. If any of it were still read from stderr, the
    /// `daemon up` half would come back `Internal`.
    #[tokio::test]
    async fn the_classification_asks_the_daemon_and_never_reads_the_authors_string() {
        let dir = staging("classify");
        let mk = |name: &str, version_ok: bool| {
            let shim = dir.join(name);
            std::fs::write(
                &shim,
                format!(
                    "#!/bin/sh\ncase \"$1\" in\n  version) exit {} ;;\nesac\nexit 0\n",
                    if version_ok { 0 } else { 1 }
                ),
            )
            .expect("write shim");
            let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
            {
                use std::os::unix::fs::PermissionsExt;
                perms.set_mode(0o700);
            }
            std::fs::set_permissions(&shim, perms).expect("chmod");
            DockerRunner::with_binary(shim.display().to_string())
        };

        // Every string an author could spell to imitate a daemon outage, plus
        // the two real messages from the live reproduction.
        let authors_strings = [
            "docker.sock",
            "no such file or directory",
            "Cannot connect to the Docker daemon",
            "Error response from daemon: Could not find the file \
             /egret/working-dir/docker.sock in container egret-run-1",
            "Unable to find image 'docker.sock/nope:1' locally",
        ];

        let up = mk("up.sh", true);
        for stderr in authors_strings {
            let e = up.classify("cp", stderr).await;
            assert!(
                matches!(e, Error::InvalidInput(_)),
                "the daemon answered, so this is the AUTHOR's flow whatever their \
                 string says -- got {e:?} for {stderr:?}"
            );
        }

        // And when the daemon really is gone it is egret's outage, even for a
        // stderr that mentions nothing at all.
        let down = mk("down.sh", false);
        let e = down.classify("cp", "something entirely unrelated").await;
        assert!(
            matches!(e, Error::Internal(_)),
            "the daemon did not answer, so this is egret's outage: {e:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The same property end to end**: a declared `outputFiles` naming a
    /// marker is a task failure, not a worker one.
    #[tokio::test]
    async fn an_output_file_named_like_a_daemon_marker_fails_the_task() {
        let dir = staging("marker-output");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            "#!/bin/sh\n\
             case \"$1\" in\n\
             \x20 create) echo id ;;\n\
             \x20 version) exit 0 ;;\n\
             \x20 inspect) echo 0 ;;\n\
             \x20 cp) echo \"Error response from daemon: Could not find the file \
             /egret/working-dir/docker.sock in container c\" >&2; exit 1 ;;\n\
             esac\nexit 0\n",
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string());
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["true".into()],
            output_files: vec!["docker.sock".into()],
            ..TaskCommands::default()
        };
        let sink = super::super::handlers::TracingSink;
        let out = runner
            .run(&leased(1_700_000_000), &cmds, &sink)
            .await
            .expect(
                "an author naming a file `docker.sock` must not be able to make egret \
                 report its own daemon as unreachable",
            );
        let why = out.failure.clone().expect("the file is not there");
        assert!(why.contains("docker.sock"), "{why}");
        assert!(
            why.contains("did not leave"),
            "and it is the ordinary missing-output message: {why}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
