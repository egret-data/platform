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
//! | [`TaskRunner`] | `io.kestra.core.models.tasks.runners.TaskRunner` | **where** a command runs — a Docker container ([`DockerRunner`]) or a Kubernetes pod ([`KubernetesRunner`](super::k8s::KubernetesRunner)) |
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
//!    render vars, which the script handler binds itself (`outputDir` since
//!    W9a Task 6), and `getEnableOutputDirectory` is
//!    [`TaskCommands::output_directory`].
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
//! * **Internal storage since W9a (2026-10-01).** Kestra's
//!   `ScriptOutput.outputFiles` is a `Map<String, URI>` into its own storage,
//!   and so is egret's now: every declared output (and, with
//!   `outputDirectory: true`, every file under [`OUTPUT_DIR`]) is streamed out
//!   of the container into the object store ([`crate::storage::collect`])
//!   and its value is the `kestra://` URI. W5a carried the **contents**
//!   inline under a 1 MiB cap, UTF-8 only; both limits are gone (W9a spec
//!   §6.2) -- `workflow.storage.max_file_bytes` and `max_task_bytes` bound
//!   what a task stores. An `inputFiles` value that is a `kestra://` URI is
//!   staged from the store ([`TaskCommands::stored_inputs`]).
//! * **No `namespaceFiles`, no `targetOS`.** Each needs a seam this crate
//!   does not have (namespace files are W9b, Windows workers none).
//!   [`TaskCommands::input_files`] carries the inline case -- a script
//!   written by the handler itself, or an author's literal contents.
//! * **No registry credentials, no `shmSize`, `deviceRequests`, `privileged`,
//!   `networkMode`, volumes.** W5a Task 5 implements `pullPolicy`, `cpu.cpus`,
//!   `memory.memory` and `delete` (see [`create_args`]); every other Kestra
//!   `Docker` property is refused at save by `scripts::parse`, never ignored.

use std::collections::BTreeMap;
use std::process::Stdio;

use egret_core::{Error, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::TaskInput;
use crate::logs::LogLevel;
use crate::scripts::RunnerSpec;

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

/// The most bytes a directory LISTING for `outputFiles` globs may be -- the
/// output of `docker diff` (Docker) or `ls -A1p` (Kubernetes). Past it the
/// task fails by name; nothing is parsed (W5a Task 5 fix round 2: an author
/// who creates 300 000 files must not be able to make the worker read, hold
/// or scan an unbounded listing inside its handler, where the lease renewal,
/// the budget and the self-fence cannot run).
pub const MAX_LISTING_BYTES: usize = 1_048_576;

/// A pure DoS guard on `docker diff` (W5a Task 5 fix round 3): the most
/// bytes read of the WHOLE container's diff, most of which is outside the
/// working directory (`pip install`, `apt-get`) and is skipped, not counted
/// toward [`MAX_LISTING_BYTES`]. Past it, or past [`DIFF_TIMEOUT`], the task
/// fails saying the container changed too much to list.
pub const MAX_DIFF_BYTES: usize = 64 * 1_048_576;

/// How long reading `docker diff` may take (the same DoS guard).
pub const DIFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The most names `outputFiles` may resolve to -- literal names and glob
/// matches together -- before the task fails by name. Each one is a copy or
/// an exec, and a key in the outputs map.
pub const MAX_OUTPUT_NAMES: usize = 1_000;

/// The most archive ENTRIES `outputDirectory`'s archive may hold -- files,
/// directories, links, the root itself (W9a Task 6 fix round 1, M1). Files
/// are also held to [`MAX_OUTPUT_NAMES`]; this bounds what is not a file, so
/// an archive of endless empty directories cannot keep the worker reading
/// headers with nothing ever crossing a byte or name cap.
pub const MAX_OUTPUT_DIR_ENTRIES: usize = 10_000;

/// The most distinct glob PATTERNS one task's `outputFiles` may declare
/// (W5a Task 5 fix round 3): refused at save, and again at run for a draft.
pub const MAX_OUTPUT_PATTERNS: usize = 64;

/// The longest a pattern's FILE NAME may be, in bytes -- `NAME_MAX`: a longer
/// pattern with one literal character could match no file. Refused at save
/// and at run.
pub const MAX_PATTERN_BYTES: usize = 255;

/// The most matcher steps resolving one task's `outputFiles` globs may take
/// (`scripts::gate::glob_match_within`). With the caps above the worst case
/// is `MAX_OUTPUT_PATTERNS` x the listing (at most [`MAX_LISTING_BYTES`] of
/// names) x `MAX_PATTERN_BYTES` -- about 1.7e10 steps, too many to run inside
/// a handler -- so the work is ALSO bounded by this budget, independent of
/// the input: past it the task fails by name. 200 M steps leaves room for
/// every realistic case (64 simple patterns x ~47k names x ~20 steps is
/// ~60 M) and is measured in `the_glob_worst_case_is_bounded_by_the_step_budget`.
pub const GLOB_STEP_BUDGET: u64 = 200_000_000;

/// The most bytes the `::{…}::` markers may contribute in total.
///
/// The markers are written to `task_runs.outputs` (JSONB) and re-read into
/// the render context of every later task in the execution, so a container
/// that prints a marker in a loop would otherwise grow the row without bound.
/// (W5a's output-file cap made the same argument for file contents; since
/// W9a an output file is a URI into internal storage, under
/// `workflow.storage.max_file_bytes` and `max_task_bytes`.)
pub const MAX_VARS_BYTES: usize = 1_048_576;

/// What to run, as data — egret's `TaskCommands`.
///
/// Built by a handler, consumed by a [`TaskRunner`]. A struct rather than
/// Kestra's interface because there is one shape and no plugin author to let
/// implement a second.
#[derive(Clone, Default, PartialEq, Eq)]
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
    /// Files staged from internal storage: name relative to WORKING_DIR -> where egret reads them.
    /// Never contents; the Debug impl prints names and sizes only.
    ///
    /// W9a spec §5: an `inputFiles` value that is a `kestra://` URI, resolved
    /// readable by the render snapshot. The runner streams each one from the
    /// store into the container ([`write_tar`](crate::storage::transfer::write_tar));
    /// no name may be in both this and [`Self::input_files`].
    pub stored_inputs: BTreeMap<String, StoredInput>,
    /// `AbstractExecScript.outputFiles` — names relative to [`WORKING_DIR`],
    /// read back after the command exits.
    ///
    /// **As Kestra (W5a Task 5 fix round 1, the user's decision):** a literal
    /// name, or a non-recursive glob -- `*` and `?` in the file name, matched
    /// within one directory (`*.csv`, `sub/*.csv`) -- keying every FILE it
    /// matches; and a declared file the command did not write is IGNORED (no
    /// key, no failure). Measured: `w5a-scripts.json`
    /// `egret_expected.output_files`. `**`, `[...]` and a wildcard directory
    /// are refused by name ([`output_problem`]). W4 took literal names only
    /// and failed a missing one.
    ///
    /// **W9a (spec §6.2): each file is streamed into internal storage** at
    /// [`Self::collect_to`], and its value in the outputs is the
    /// `kestra://` URI -- no longer its contents. W5a's 1 MiB and UTF-8
    /// limits are gone; `workflow.storage.max_file_bytes` and
    /// `max_task_bytes` replace them.
    pub output_files: Vec<String>,
    /// `outputDirectory: true` (W9a Task 6): the command gets
    /// [`OUTPUT_DIR`] (the env var [`ENV_OUTPUT_DIR`], the render variable
    /// `outputDir`), created before it starts, and every file under it is
    /// collected as if declared, keyed by its path relative to it and stored
    /// verbatim under the task prefix -- Kestra's measured keying
    /// (`w9a-storage.json` `egret_expected.output_directory`).
    pub output_directory: bool,
    /// Where collected files go. `None` with any output declared fails the task naming
    /// `[storage]`: W5a's contents-in-outputs path no longer exists.
    pub collect_to: Option<CollectTo>,
    /// **The plugin host's stdout protocol** (W5b): `false` for every script
    /// task, which keeps W5a's and W11's line rules byte for byte.
    ///
    /// `true` only from the `kestra.*` handler, whose image runs
    /// `EgretPluginHost` with logback's `%level %msg%n` on stdout. Then
    /// [`ingest_line`]:
    ///
    /// * levels a stdout line that starts with `TRACE`, `DEBUG`, `INFO`,
    ///   `WARN` or `ERROR` and ONE space, and strips that prefix;
    /// * gives a stdout line with no prefix -- a Java stack trace's `\tat …`
    ///   and `Caused by: …` -- the level of the stdout line before it (INFO,
    ///   or DEBUG inside a bracket, when there is none);
    /// * harvests a `::{"egretFailure": {"code": n, "message": "…"}}::`
    ///   marker into [`CommandsResult::host_failure`].
    ///
    /// stderr stays ERROR, and the debug bracket and the `outputs` marker
    /// rules are unchanged. The argv and env are built the same way whatever
    /// this says; **the pod differs in one line**: a plugin pod's `files-out`
    /// helper exits on SIGTERM, so a graceful cancel ends when `main` does
    /// ([`files_out_script`](super::k8s::pod_spec::files_out_script)). A
    /// script pod (`false`) is W5a's byte for byte.
    pub host_protocol: bool,
}

/// Where a task run's collected files go (W9a spec §5): its workspace, and
/// its prefix `kestra:///…/executions/{the run's own execution}/tasks/{task}/{taskRunId}/`
/// (an iteration's id in a `Loop`, spec §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectTo {
    pub workspace: uuid::Uuid,
    pub task_prefix: String,
    pub naming: Naming,
}

/// How a declared output's URI is named under [`CollectTo::task_prefix`].
/// The output directory's files are always [`Naming::UnderPrefixVerbatim`]
/// (Kestra's measured `output_directory.uri_tail`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    /// `IdUtils.from({WORKING_DIR}/{rel}) + "-" + filename`, spaces as `+`
    /// (`FilesService.java:94-97`). The scheme is Kestra's; the hash cannot
    /// equal Kestra's, whose working directory is a random temp dir.
    KestraOutputName,
    /// `{task_prefix}{rel}` -- the plugin bridge's `.egret/out/` (Task 9).
    UnderPrefixVerbatim,
}

/// The output directory of `outputDirectory: true`, inside [`WORKING_DIR`]
/// so the `files-out` helper of a Kubernetes pod sees it (the working
/// directory is the pod's shared volume).
///
/// Kestra's is a fresh `IdUtils.create()` under its working directory,
/// random per run (`w9a-storage.json` `egret_expected.output_directory`);
/// egret's is fixed, because a re-claim that adopts a running container
/// rebuilds the commands and must name the directory the container was
/// given. Its name never reaches a URI or a key -- both are relative to it.
pub const OUTPUT_DIR: &str = "/egret/working-dir/.egret/out";

/// [`OUTPUT_DIR`] relative to [`WORKING_DIR`].
pub const OUTPUT_DIR_REL: &str = ".egret/out";

/// The plugin host's scratch, relative to [`WORKING_DIR`] (W9a Task 9 fix
/// round 1): Kestra's `LocalWorkingDir` and `EgretStorage`'s `.part` files.
/// Created world-writable for every `host_protocol` run -- `docker cp`
/// leaves the working directory itself root-owned and the host image runs as
/// `USER 65532` -- and kept under the working directory so it stays bounded
/// by the working directory's `emptyDir.sizeLimit` on Kubernetes. Never
/// collected: only [`OUTPUT_DIR_REL`] is.
pub const SCRATCH_DIR_REL: &str = ".egret/tmp";

/// The env var naming [`OUTPUT_DIR`]: Kestra's `OUTPUT_DIR`, measured.
pub const ENV_OUTPUT_DIR: &str = "OUTPUT_DIR";

/// One file staged from internal storage (W9a spec §5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredInput {
    /// The `kestra://` URI the author named, for messages.
    pub uri: String,
    /// `FileStore::object_uri` of it, in the running execution's workspace.
    pub object_uri: String,
    /// The size its index row holds; the archive entry declares it, and an
    /// object of any other size fails the task.
    pub size: u64,
}

impl std::fmt::Debug for TaskCommands {
    /// Every field but [`TaskCommands::input_files`]' contents, which are
    /// printed as their names and byte counts (W5b fix round 1, M5): the
    /// plugin handler's `task.json` holds resolved secrets. The stored inputs
    /// are printed as names and sizes the same way.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let files: BTreeMap<&str, String> = self
            .input_files
            .iter()
            .map(|(k, v)| (k.as_str(), format!("<{} bytes>", v.len())))
            .collect();
        let stored: BTreeMap<&str, String> = self
            .stored_inputs
            .iter()
            .map(|(k, v)| (k.as_str(), format!("<{} bytes stored>", v.size)))
            .collect();
        f.debug_struct("TaskCommands")
            .field("container_image", &self.container_image)
            .field("interpreter", &self.interpreter)
            .field("before_commands", &self.before_commands)
            .field("commands", &self.commands)
            .field("env", &self.env)
            .field("input_files", &files)
            .field("stored_inputs", &stored)
            .field("output_files", &self.output_files)
            .field("output_directory", &self.output_directory)
            .field("collect_to", &self.collect_to)
            .field("host_protocol", &self.host_protocol)
            .finish()
    }
}

impl TaskCommands {
    /// Whether anything is collected out of the container after the command.
    pub fn has_outputs(&self) -> bool {
        !self.output_files.is_empty() || self.output_directory
    }

    /// Whether anything goes into the working directory before the command.
    pub fn has_inputs(&self) -> bool {
        !self.input_files.is_empty()
            || !self.stored_inputs.is_empty()
            || self.output_directory
            || self.host_protocol
    }

    /// Every input file, by name, as the archive writes it. A name in both
    /// maps is refused: it would be staged twice.
    pub fn tar_sources(&self) -> Result<BTreeMap<String, crate::storage::transfer::TarSource<'_>>> {
        use crate::storage::transfer::TarSource;
        let mut out: BTreeMap<String, TarSource<'_>> = self
            .input_files
            .iter()
            .map(|(k, v)| (k.clone(), TarSource::Contents(v.as_str())))
            .collect();
        for (k, v) in &self.stored_inputs {
            if out.insert(k.clone(), TarSource::Stored(v)).is_some() {
                return Err(Error::InvalidInput(format!(
                    "the input file {k:?} is declared twice, as contents and as a stored file"
                )));
            }
        }
        if self.output_directory {
            if let Some(clash) = out
                .keys()
                .find(|k| *k == OUTPUT_DIR_REL || k.starts_with(&format!("{OUTPUT_DIR_REL}/")))
            {
                return Err(Error::InvalidInput(format!(
                    "the input file {clash:?} is inside the output directory {OUTPUT_DIR}, \
                     which egret creates empty for `outputDirectory: true`"
                )));
            }
            out.insert(OUTPUT_DIR_REL.to_string(), TarSource::Directory);
        }
        if self.host_protocol {
            if let Some(clash) = out
                .keys()
                .find(|k| *k == SCRATCH_DIR_REL || k.starts_with(&format!("{SCRATCH_DIR_REL}/")))
            {
                return Err(Error::InvalidInput(format!(
                    "the input file {clash:?} is inside {SCRATCH_DIR_REL}, the plugin host's \
                     scratch directory, which egret creates empty"
                )));
            }
            out.insert(SCRATCH_DIR_REL.to_string(), TarSource::Directory);
        }
        Ok(out)
    }

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
    /// Every collected output file -- declared, globbed or in the output
    /// directory -- by its path relative to where it was found, as stored.
    /// `outputs.outputFiles` maps the same keys to the URIs (W9a spec §6.2).
    pub output_files: BTreeMap<String, crate::storage::StoredFile>,
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
    /// The plugin host's `::{"egretFailure": {"code": n, "message": "…"}}::`
    /// line, **harvested only under [`TaskCommands::host_protocol`]** -- for a
    /// script the key is ignored like every non-`outputs` key (W11's rule).
    ///
    /// The runner records it and decides nothing: the handler maps the exit
    /// code and puts the message in the FAILED reason (and on a timeout the
    /// outcome is the budget's whatever the host printed, `cancel.rs` fact 2).
    /// When the host prints more than one, the last one stands.
    pub host_failure: Option<HostFailure>,
}

/// What the plugin host said about its own failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFailure {
    /// The host's exit code as it printed it (`2`, `3`, `4`).
    pub code: i32,
    /// At most [`MAX_HOST_FAILURE_BYTES`], cut at a char boundary.
    pub message: String,
}

/// The longest an `egretFailure` message is kept, in bytes (W5b global
/// constraints: 4 KiB). It becomes a task run's FAILED reason.
pub const MAX_HOST_FAILURE_BYTES: usize = 4096;

/// **Where** a command runs.
///
/// Two implementations: [`DockerRunner`] (local development and tests) and,
/// since W5a Task 4, [`KubernetesRunner`](super::k8s::KubernetesRunner) (a pod
/// per task-run attempt, production). The trait exists because Kestra's does
/// and for the same reason: the Kubernetes runner is a second implementation
/// of this, not a second copy of every script handler.
///
/// # `cancel` has no default body, for [`TaskHandler::cancel`]'s reason
///
/// A default `Ok(())` would answer for a runner that holds a pod, and nothing
/// would say so. Every implementation states what stopping means to it.
///
/// [`TaskHandler::cancel`]: super::TaskHandler::cancel
#[async_trait::async_trait]
pub trait TaskRunner: Send + Sync + std::fmt::Debug {
    /// Run `commands` for `task`, writing every log line to `task.log` as it
    /// arrives, through [`stream_line`], after showing it to `observer`.
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
    ///
    /// `spec` is the task's parsed `taskRunner:` (`pullPolicy`, `delete`,
    /// `resources`, …) — **where** the command runs, per task, which is why it
    /// is an argument and not a field of the runner (W5a Task 4). A runner
    /// reads the properties it implements; a handler with no `taskRunner:`
    /// passes [`RunnerSpec::defaults`].
    ///
    /// # Re-attach (W5a spec §5)
    ///
    /// A runner that names its work deterministically per **(run, attempt)**
    /// — [`KubernetesRunner`](super::k8s::KubernetesRunner) — first looks the
    /// work up by that name and re-attaches to it if it exists, instead of
    /// creating a second one. That is what makes a dropped `run` future (the
    /// worker lost its lease, W5a Task 3) safe: dropping never deletes, and
    /// the next holder of the lease finds the same pod. [`DockerRunner`] does
    /// the same with its container since W5a Task 5.
    ///
    /// # `run` never deletes what it ran (W5a final review I3)
    ///
    /// Whatever `run` returns, the container or pod is left in place:
    /// [`release`](Self::release) deletes it, and `claim::run_one` calls that
    /// (through the handler) only **after the outcome committed**. A runner
    /// that deleted here would turn a failed commit into a second execution.
    async fn run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        spec: &RunnerSpec,
        observer: &dyn LineObserver,
    ) -> Result<CommandsResult>;

    /// [`run`](Self::run), **but only if the work for `task`'s (run, attempt)
    /// already exists and was started** -- a re-attach, never a create.
    /// `Ok(None)` when there is nothing to adopt; nothing is created then.
    ///
    /// This is how the script handler keeps W5a spec §4's promise that "runs
    /// already in a pod finish" when the gate would now refuse the task (the
    /// workspace turned `workflow_containers` off, or the allowlist changed,
    /// after the pod was created): the pod passed the gate when it was
    /// created, and a re-claim adopts it rather than failing the run and
    /// leaving the pod for the sweep. Because this cannot create, the gate is
    /// never bypassed for new work.
    async fn adopt(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        spec: &RunnerSpec,
        observer: &dyn LineObserver,
    ) -> Result<Option<CommandsResult>>;

    /// Whether [`adopt`](Self::adopt) would find started work for `task`'s
    /// (run, attempt). Lets a handler that refuses a run decide whether it
    /// must prepare for an adoption -- the plugin handler resolves secrets
    /// only then, to mask what the adopted container prints (W5b fix round 1,
    /// M2). Read-only; a runner that cannot tell cheaply answers `true`, and
    /// `adopt` still decides.
    async fn adoptable(&self, _task: &TaskInput) -> Result<bool> {
        Ok(true)
    }

    /// Stop whatever `task` is running, addressed by **identity**.
    ///
    /// The caller is [`cancel::guarded`](super::TaskHandler::cancel) by way of
    /// a handler, and it has no handle to pass: see [`container_name`].
    ///
    /// # `grace` (W5b)
    ///
    /// `Duration::ZERO` is W5a's cancel, unchanged: kill and remove at once
    /// (Kestra's `killGracePeriod` default). The script handler passes it.
    ///
    /// A non-zero `grace` asks the container to stop: SIGTERM to its PID 1,
    /// SIGKILL after `grace`. **`cancel` then returns only once the container
    /// has stopped, or `grace` + [`CANCEL_WAIT_MARGIN`] has passed** -- the
    /// outcome commits after `cancel`, and [`release`](Self::release) then
    /// removes the container with grace 0, which would otherwise cut the
    /// plugin host's `kill()` short (spec §7.1). A container still running at
    /// the bound is a WARN and `Ok(())`: the outcome is the budget's either
    /// way (`cancel.rs` fact 2).
    async fn cancel(&self, task: &TaskInput, grace: std::time::Duration) -> Result<()>;

    /// Free `task`'s container or pod once its outcome has **committed**
    /// (`TaskHandler::release`): delete it when `spec.delete` (the default),
    /// otherwise leave it, as the author asked -- the Kubernetes runner lets
    /// its `files-out` sidecar exit so the kept pod completes. A container or
    /// pod that is not there is `Ok(())`.
    ///
    /// No default body, for `cancel`'s reason: every runner says what freeing
    /// means to it.
    async fn release(&self, task: &TaskInput, spec: &RunnerSpec) -> Result<()>;
}

/// How long past its `grace` a [`TaskRunner::cancel`] waits for the container
/// to stop before giving up with a WARN.
pub const CANCEL_WAIT_MARGIN: std::time::Duration = std::time::Duration::from_secs(5);

/// Something that sees each line a runner writes, before it goes to
/// `task.log` — `handlers::python`'s `NameHintSink`, which connects a
/// `NameError` to its cause.
///
/// An observer and not a sink: it cannot drop, reorder or re-level a line,
/// and the line reaches the log whatever it does.
pub trait LineObserver: Send + Sync {
    fn observe(&self, level: LogLevel, line: &str);
}

/// The observer for a handler with nothing to watch for.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoObserver;

impl LineObserver for NoObserver {
    fn observe(&self, _level: LogLevel, _line: &str) {}
}

/// `DefaultLogConsumer`'s per-command state: the bytes of markers harvested
/// so far (for the vars cap) and whether a `##kestra:log:debug##` bracket is
/// open. One per command, shared by both streams — which is why a marker on
/// either stream toggles it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StreamState {
    pub vars_bytes: usize,
    pub debug: bool,
    /// Under [`TaskCommands::host_protocol`]: the level of the last stdout
    /// line that carried a prefix, which a line with none (a stack trace's
    /// continuation) takes. `None` until the first one.
    pub last_level: Option<LogLevel>,
}

/// The line that opens a DEBUG bracket: `DefaultLogConsumer.LOG_DEBUG_MARKER`.
pub const LOG_DEBUG_MARKER: &str = "##kestra:log:debug##";
/// The line that closes it: `DefaultLogConsumer.LOG_INFO_MARKER`.
pub const LOG_INFO_MARKER: &str = "##kestra:log:info##";

/// One line from a command's stream: decide it with [`ingest_line`], show it
/// to `observer`, and write it to `task.log`.
///
/// The async half of line handling, shared by [`DockerRunner`] and every fake
/// runner in the tests, so a test driving a fake exercises the same levels and
/// the same destination the real runner does.
// Eight plain arguments, each [`ingest_line`]'s or the sink's; a struct for
// one call shape shared by three runners and a fake would only rename them.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_line(
    task: &TaskInput,
    observer: &dyn LineObserver,
    result: &mut CommandsResult,
    state: &mut StreamState,
    max_vars_bytes: usize,
    line: &str,
    is_err: bool,
    host_protocol: bool,
) {
    if let Some((level, text)) =
        ingest_line(result, state, max_vars_bytes, line, is_err, host_protocol)
    {
        observer.observe(level, &text);
        task.log.line(level, &text).await;
    }
}

/// [`stream_line`] for a re-attach: a line the previous holder already
/// stored (`replayed`) is decided like any other -- counted, its marker
/// harvested, its level carried to the next continuation line -- but not
/// written again.
///
/// **Compared as stored** (W5b fix round 1, M1): `replayed` holds stored
/// text, which under `host_protocol` is the message without its level prefix
/// and, on a masked log, with each secret replaced. The raw line matched
/// neither, so the comparison is made after [`ingest_line`] and
/// `RunLog::stored_form`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_replayed_line(
    task: &TaskInput,
    observer: &dyn LineObserver,
    result: &mut CommandsResult,
    state: &mut StreamState,
    max_vars_bytes: usize,
    line: &str,
    is_err: bool,
    host_protocol: bool,
    replayed: &mut crate::worker::k8s::runner::Replayed,
) {
    if let Some((level, text)) =
        ingest_line(result, state, max_vars_bytes, line, is_err, host_protocol)
    {
        if replayed.already_stored(&task.log.stored_form(&text)) {
            return;
        }
        observer.observe(level, &text);
        task.log.line(level, &text).await;
    }
}

/// The container this run attempt owns: `egret-run-<run id, hyphenless>-<attempt>`
/// -- the Kubernetes runner's [`pod_name`](super::k8s::pod_name), exactly.
///
/// # W5a Task 5 moved the key from the LEASE to the ATTEMPT, and why that is
/// # now safe
///
/// W4 named the container after `(run_id, claimed_at)` so that a worker A,
/// reaped while still inside its handler, could not kill the container of the
/// worker B that claimed the same run: A's own budget would expire and call
/// `cancel` with a name that no longer existed. Two things changed in W5a:
///
/// 1. **A worker renews its lease while its handler runs, under a fence, and
///    drops the handler's future when the fence moves** (Task 3,
///    `worker::claim::under_lease`) -- **or when no renewal has succeeded for
///    `working_timeout`, the reaper's own threshold** (Task 5 fix round 1).
///    That second half is what makes this key safe, and the first version of
///    this doc claimed it without it: a renewal that ERRORS used to be retried
///    for ever, so a worker cut off from Postgres but not from Docker or the
///    API server stayed inside its handler after being reaped, and could
///    kill (its own budget's `cancel`) the container the new holder had
///    re-attached to. Now the worker gives the run up no later than the
///    reaper could take it, with its future -- budget and all -- dropped, so
///    no `cancel` is issued. (Removing the container on finish, the other
///    half of that fear, is gone too: `release` runs only after a COMMITTED
///    outcome, which a worker that lost the run cannot make -- final review
///    I3.)
///    *Residual, stated:* a worker whose process is frozen (not merely cut
///    off) past the deadline resumes inside a poll of the handler before the
///    fence is re-checked; the window is one poll.
/// 2. **The new holder must FIND the old holder's container** to re-attach to
///    it (spec §5), which a lease-keyed name makes impossible: every claim
///    would create a second container while the first kept running (Task 3's
///    measured orphan). The attempt is the re-attach key because `reap`
///    leaves the attempt open, so A and B compute the same name -- which is
///    now the point.
///
/// A `cancel` for the run (a kill, a budget that expired in the live holder)
/// reaches the container whoever started it, which is what a kill means.
pub fn container_name(task: &TaskInput) -> String {
    super::k8s::pod_name(task.run_id, super::k8s::pod_spec::attempt_of(task))
}

/// The label that marks a container the author asked to keep
/// (`taskRunner.delete: false`); the orphan sweep leaves it alone, as the pod
/// sweep leaves a pod with the same annotation.
pub const DELETE_LABEL: &str = super::k8s::pod_spec::DELETE_ANNOTATION;

/// The labels a container carries, so an operator can find one this worker
/// left behind.
///
/// `ScriptService.labels` writes `kestra.io/{namespace,flow-id,task-id,
/// execution-id,taskrun-id,taskrun-attempt}` (fact 5). egret writes its own
/// namespace and the same identifiers -- the pod's four
/// (`egret.io/{workspace-id,execution-id,taskrun-id,attempt}`, W5a spec §5)
/// plus the task id. W4's `egret.io/lease` label is gone with the lease-keyed
/// name ([`container_name`]): after a re-attach it would name the wrong holder.
///
/// **The labels are diagnostics, not identity.** Nothing in this module looks a
/// container up by them — [`container_name`] is the address — and that is
/// deliberate: a lookup by label would be a second way to name the same thing,
/// able to disagree with the first.
///
/// *W5a Task 4:* the one reader is [`OrphanSweep`], which **lists** by
/// [`TASKRUN_LABEL`] to find the run each container belongs to. It never
/// addresses a single container's work by label; it removes by name.
pub fn labels(task: &TaskInput) -> Vec<(String, String)> {
    vec![
        (
            super::k8s::pod_spec::WORKSPACE_LABEL.into(),
            task.workspace_id.to_string(),
        ),
        (
            super::k8s::pod_spec::EXECUTION_LABEL.into(),
            task.execution_id.to_string(),
        ),
        (TASKRUN_LABEL.into(), task.run_id.to_string()),
        ("egret.io/task-id".into(), task.task_id.clone()),
        (
            super::k8s::pod_spec::ATTEMPT_LABEL.into(),
            super::k8s::pod_spec::attempt_of(task).to_string(),
        ),
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
/// the marker, the vars cap, the debug bracket and the level. `run_created`
/// keeps only the parts that genuinely need one.
///
/// Returns what to store, `None` for a line that is not stored at all. It is
/// `DefaultLogConsumer.accept` + `PluginUtilsService.parseOut`, in their
/// order, and every level is W11 Task 1's measurement
/// (`oracle-exec/w11-logs.json`):
///
/// 1. `##kestra:log:debug##` / `##kestra:log:info##` toggle the bracket, on
///    either stream, and are neither stored nor counted (`accept` returns
///    before counting).
/// 2. A **whole-line** `::{…}::` marker is not stored, on either stream: its
///    `outputs` are harvested, any other key (`metrics`) is ignored, and one
///    that does not parse is stored as one WARN `Invalid outputs '<error>'`
///    (the error text is serde's; Kestra's DEBUG stack line is not written).
///    A line that merely contains a marker is an ordinary line.
/// 3. stderr is **ERROR** — measured with `warningOnStdErr` default, `false`
///    and `true` alike, and `parseOut` tests the stream before the bracket, so
///    also inside one. stdout is DEBUG inside a bracket and INFO outside.
/// 4. **Under `host_protocol` only** (W5b, [`TaskCommands::host_protocol`]):
///    a stdout line `LEVEL message` is stored at `LEVEL` as `message`, a
///    stdout line with no prefix at the previous prefixed line's level (rule
///    3's when there is none), and an `egretFailure` marker fills
///    [`CommandsResult::host_failure`]. With it off, none of this happens.
///
/// **The cap sets [`CommandsResult::failure`] and clears the vars rather than
/// returning an error.** Returning one mid-stream abandoned the exit code and
/// left `docker start`'s pipes unread, which stalls a container that is still
/// writing; and the vars are cleared rather than kept because the whole point
/// of the cap is that this much is not going into `task_runs.outputs`.
pub(crate) fn ingest_line<'a>(
    result: &mut CommandsResult,
    state: &mut StreamState,
    max_vars_bytes: usize,
    line: &'a str,
    is_err: bool,
    host_protocol: bool,
) -> Option<(LogLevel, std::borrow::Cow<'a, str>)> {
    let bare = line.trim_end_matches(['\r', '\n']);
    if bare == LOG_DEBUG_MARKER {
        state.debug = true;
        return None;
    }
    if bare == LOG_INFO_MARKER {
        state.debug = false;
        return None;
    }

    if is_err {
        result.stderr_line_count += 1;
    } else {
        result.stdout_line_count += 1;
    }

    // **There is deliberately no `if result.failure.is_none()` guard around
    // the harvest**, and fix round 1's mutation round is why. There was one;
    // mutating it to `if true` was killed by **nothing** in the whole crate,
    // and the reason is that it could not be: `vars_bytes` only ever grows, so
    // once the cap has fired every later marker re-fires it on its first key
    // and clears the map again before inserting anything. The guard was
    // unobservable, so it went.
    match classify_marker(line) {
        Marker::Ordinary => {}
        Marker::Object(obj) => {
            // The host's failure line: read only under the protocol, so a
            // script's own `egretFailure` key stays ignored (W11's rule).
            if host_protocol {
                if let Some(failure) = host_failure_of(&obj) {
                    result.host_failure = Some(failure);
                }
            }
            if let Some(serde_json::Value::Object(outputs)) = obj.get("outputs") {
                for (k, v) in outputs {
                    state.vars_bytes += k.len() + v.to_string().len();
                    if state.vars_bytes > max_vars_bytes {
                        result.vars.clear();
                        result.failure = Some(format!(
                            "the command emitted more than {max_vars_bytes} bytes of \
                             `::{{\"outputs\": …}}::` markers, and every byte of that would be \
                             stored on the task run and re-rendered by every later task in \
                             the execution"
                        ));
                        break;
                    }
                    result.vars.insert(k.clone(), v.clone());
                }
            }
            return None;
        }
        Marker::Invalid(error) => {
            return Some((
                LogLevel::Warn,
                std::borrow::Cow::Owned(format!("Invalid outputs '{error}'")),
            ));
        }
    }

    let unprefixed = if state.debug {
        LogLevel::Debug
    } else {
        LogLevel::Info
    };
    if is_err {
        return Some((LogLevel::Error, std::borrow::Cow::Borrowed(line)));
    }
    if host_protocol {
        // `%level %msg%n`: the prefix is the level and is not stored. A line
        // without one (a stack trace's `\tat …`, `Caused by: …`) belongs to
        // the log event before it, so it takes that event's level.
        return Some(match host_prefix(line) {
            Some((level, message)) => {
                state.last_level = Some(level);
                (level, std::borrow::Cow::Borrowed(message))
            }
            None => (
                state.last_level.unwrap_or(unprefixed),
                std::borrow::Cow::Borrowed(line),
            ),
        });
    }
    Some((unprefixed, std::borrow::Cow::Borrowed(line)))
}

/// `LEVEL message` -> `(LEVEL, message)`: one of the five names, exactly as
/// logback's `%level` prints them, then ONE space. Anything else is `None`.
fn host_prefix(line: &str) -> Option<(LogLevel, &str)> {
    let (head, message) = line.split_once(' ')?;
    LogLevel::parse(head).map(|level| (level, message))
}

/// `{"egretFailure": {"code": n, "message": "…"}}` -> a [`HostFailure`], the
/// message cut to [`MAX_HOST_FAILURE_BYTES`]. A payload of any other shape
/// is ignored: the exit code still fails the task, and the handler names it.
fn host_failure_of(obj: &serde_json::Map<String, serde_json::Value>) -> Option<HostFailure> {
    let failure = obj.get("egretFailure")?.as_object()?;
    let code = i32::try_from(failure.get("code")?.as_i64()?).ok()?;
    let message = failure
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Some(HostFailure {
        code,
        message: truncate_on_char_boundary(message, MAX_HOST_FAILURE_BYTES).to_string(),
    })
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a char
/// boundary. One slice, no re-encoding: the cut moves back at most three
/// bytes (a UTF-8 char is at most four).
pub(crate) fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// What [`classify_marker`] makes of a line.
pub(crate) enum Marker {
    /// An ordinary line.
    Ordinary,
    /// A whole-line `::{…}::` whose payload parsed.
    Object(serde_json::Map<String, serde_json::Value>),
    /// A whole-line `::{…}::` whose payload did not parse: the parser's error.
    Invalid(String),
}

/// `TaskLogLineMatcher.LOG_DATA_SYNTAX` — `^::(\{.*})::$` — and then Jackson.
pub(crate) fn classify_marker(line: &str) -> Marker {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(inner) = line.strip_prefix("::").and_then(|l| l.strip_suffix("::")) else {
        return Marker::Ordinary;
    };
    // `\{.*}` — the payload must itself be brace-delimited. A `::foo::` line is
    // an ordinary line, not a broken marker.
    if !(inner.starts_with('{') && inner.ends_with('}')) {
        return Marker::Ordinary;
    }
    match serde_json::from_str::<serde_json::Value>(inner) {
        Ok(serde_json::Value::Object(obj)) => Marker::Object(obj),
        // Brace-delimited and valid is always an object; kept total anyway.
        Ok(other) => Marker::Invalid(format!("not a JSON object: {other}")),
        Err(e) => Marker::Invalid(e.to_string()),
    }
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
/// Returns the `outputs` object the line carried, or `None` — for an ordinary
/// line, for a marker without `outputs`, and for one that does not parse. Whether
/// the line is **stored** is a different question, answered by [`ingest_line`]:
/// every whole-line marker is consumed, and a broken one becomes a WARN.
pub fn parse_marker(line: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    // `parseOut` merges the `outputs` key and nothing else.
    match classify_marker(line) {
        Marker::Object(obj) => match obj.get("outputs") {
            Some(serde_json::Value::Object(outputs)) => Some(outputs.clone()),
            _ => None,
        },
        Marker::Ordinary | Marker::Invalid(_) => None,
    }
}

/// **What a symlink declared as an output means, and why it is refused by
/// name** -- the history, kept because each fix was measured.
///
/// # W5a: a sandbox escape through the host staging directory
///
/// `docker cp` out of a container to a host path **recreates a symlink as a
/// symlink** there; anything that then opened that path read it **on the
/// worker host**. Measured on daemon 28.4.0 with `ln -s /etc/passwd leaked.txt`
/// in an `alpine:3.20` working directory:
///
/// ```text
/// host      /etc/passwd   9344 bytes
/// alpine    /etc/passwd    702 bytes
/// staged    leaked.txt    ->/etc/passwd   (lstat: islink=true)
/// read back                9344 bytes   <- the WORKER HOST's file, as a SUCCESS
/// ```
///
/// Two legs: the leaf (a declared name that is itself a link) and an
/// intermediate component (`etcdir/passwd` with `ln -s /etc etcdir`, which a
/// whole-directory copy brought over as a host link). W5a refused both by
/// `lstat`ing every component of the staged path, and copied per file so an
/// intermediate link resolved **inside the container** (alpine's 702 bytes).
///
/// # W9a Task 6 (2026-10-01): no host path at all
///
/// Output files are now read as a **stream** -- `docker cp c:<path> -` into
/// [`TarReader`](crate::storage::transfer::TarReader) -- and only a regular
/// file's contents are ever taken, straight into internal storage. A link is
/// an archive entry with **no contents**: there is nothing on the host to
/// resolve, and it is refused by name rather than stored empty (Kestra's
/// `Files.copy` of a link entry makes an empty file, an empty output the
/// author did not write). The per-file copy still resolves an intermediate
/// component inside the container, so `etcdir/passwd` returns the
/// container's own file, which the task could have printed anyway; the
/// boundary that matters, the worker host, is not reachable from either leg.
/// `a_symlinked_output_file_cannot_read_the_worker_hosts_files` (live) drives
/// both.
const _SYMLINKS_ARE_REFUSED_BY_NAME: () = ();

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
    /// [`MAX_VARS_BYTES`], as a field.
    max_vars_bytes: usize,
    /// [`MAX_LISTING_BYTES`], as a field (a test drives the cap).
    max_listing_bytes: usize,
    /// [`MAX_DIFF_BYTES`], as a field (a test drives the cap).
    max_diff_bytes: usize,
    /// Where a re-attach reads the attempt's newest stored line from, so it
    /// follows the container's log from there rather than from its start
    /// (W5a Task 5). `None`: a re-attach replays the whole log.
    logs: Option<crate::logs::LogRepo>,
    /// Internal storage, which stored inputs are streamed from (W9a Task 5)
    /// and output files into (Task 6). `None`: either fails the task naming
    /// `[storage]`.
    files: Option<std::sync::Arc<crate::storage::FileStore>>,
}

impl Default for DockerRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerRunner {
    /// The shipping runner: the `docker` on `PATH` and [`MAX_VARS_BYTES`].
    pub fn new() -> Self {
        Self {
            binary: "docker".to_string(),
            max_vars_bytes: MAX_VARS_BYTES,
            max_listing_bytes: MAX_LISTING_BYTES,
            max_diff_bytes: MAX_DIFF_BYTES,
            logs: None,
            files: None,
        }
    }

    /// The same runner, streaming stored inputs from `files` -- what
    /// production builds when `[storage]` is configured (`runtime::handlers_for`).
    pub fn with_files(mut self, files: std::sync::Arc<crate::storage::FileStore>) -> Self {
        self.files = Some(files);
        self
    }

    /// The same runner, reading the re-attach window from `logs` -- what
    /// production builds (`runtime::handlers_for`). See [`DockerRunner::run`].
    pub fn with_logs(mut self, logs: crate::logs::LogRepo) -> Self {
        self.logs = Some(logs);
        self
    }

    /// The same runner over another binary. For a test, and for an operator
    /// whose CLI is not on `PATH`.
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            ..Self::new()
        }
    }

    /// The same runner with a smaller [`MAX_VARS_BYTES`].
    ///
    /// `handlers::sql::Sql::with_limits`'s reason: *"so a test can drive the
    /// cap without materialising ten thousand rows to do it"*. The output
    /// files' caps are internal storage's (`workflow.storage.max_file_bytes`,
    /// `max_task_bytes`), read from the [`FileStore`](crate::storage::FileStore)
    /// this runner is given ([`with_files`](Self::with_files)).
    pub fn with_vars_cap(mut self, max_vars_bytes: usize) -> Self {
        self.max_vars_bytes = max_vars_bytes;
        self
    }

    /// The same runner with a smaller [`MAX_LISTING_BYTES`], for a test.
    pub fn with_listing_cap(mut self, max_listing_bytes: usize) -> Self {
        self.max_listing_bytes = max_listing_bytes;
        self
    }

    /// The same runner with a smaller [`MAX_DIFF_BYTES`], for a test.
    pub fn with_diff_cap(mut self, max_diff_bytes: usize) -> Self {
        self.max_diff_bytes = max_diff_bytes;
        self
    }

    /// The entries of each directory in `dirs`, from `docker diff`, **streamed**
    /// (W5a Task 5 fix round 3): only lines under the working directory are
    /// kept, and only they count toward the listing cap -- so a container that
    /// installed a package tree outside it is not "too many files". The whole
    /// diff is still bounded, by [`MAX_DIFF_BYTES`] and [`DIFF_TIMEOUT`], as a
    /// DoS guard with its own message. The process is killed at either cap.
    ///
    /// `Ok(Err)` is the task's failure; `Err` is egret's (the daemon).
    async fn diff_listing(
        &self,
        name: &str,
        dirs: &std::collections::BTreeSet<String>,
    ) -> Result<std::result::Result<std::collections::BTreeMap<String, Vec<String>>, String>> {
        use tokio::io::AsyncReadExt;
        let mut child = Command::new(&self.binary)
            .args(["diff", name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!("could not run `{} diff`: {e}", self.binary))
            })?;
        let stdout = child.stdout.take().expect("stdout piped");
        let limit = u64::try_from(self.max_diff_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut reader = BufReader::new(stdout.take(limit));
        let prefix = format!("A {WORKING_DIR}/");
        let prefix_c = format!("C {WORKING_DIR}/");
        let deadline = tokio::time::Instant::now() + DIFF_TIMEOUT;
        let (mut total, mut in_wd) = (0usize, 0usize);
        let mut kept = String::new();
        let mut line = Vec::new();
        let too_changed = |what: String| {
            format!(
                "the container changed {what} of its filesystem, so egret cannot list its \
                 outputs to resolve the `outputFiles` glob; name the output files instead"
            )
        };
        let failure = loop {
            line.clear();
            let n = match tokio::time::timeout_at(deadline, reader.read_until(b'\n', &mut line))
                .await
            {
                Err(_) => break Some(too_changed(format!("too much to read in {DIFF_TIMEOUT:?}"))),
                Ok(r) => r.map_err(io_err)?,
            };
            if n == 0 {
                break None;
            }
            total += n;
            if total > self.max_diff_bytes {
                break Some(too_changed(format!(
                    "more than {} bytes",
                    self.max_diff_bytes
                )));
            }
            if line.starts_with(prefix.as_bytes()) || line.starts_with(prefix_c.as_bytes()) {
                in_wd += n;
                if in_wd > self.max_listing_bytes {
                    break Some(listing_too_big(self.max_listing_bytes));
                }
                kept.push_str(&String::from_utf8_lossy(&line));
            }
        };
        if let Some(why) = failure {
            let _ = child.kill().await;
            return Ok(Err(why));
        }
        let mut err = Vec::new();
        if let Some(stderr) = child.stderr.take() {
            let _ = stderr.take(64 * 1024).read_to_end(&mut err).await;
        }
        let status = child.wait().await.map_err(io_err)?;
        if !status.success() {
            return Err(self.classify("diff", &String::from_utf8_lossy(&err)).await);
        }
        Ok(Ok(parse_docker_diff(&kept, dirs)))
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

/// What `docker inspect --format {{.State.Status}}` says about a task's
/// container, as far as [`DockerRunner::run`] cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    /// No container by that name.
    Absent,
    /// Created and never started: nothing of the author's has run in it.
    Created,
    /// Being removed, or dead: a `cancel` got there first.
    Going,
    /// Running, paused, restarting or exited: its work happened, or is
    /// happening, and a new holder re-attaches to it.
    Started,
}

#[async_trait::async_trait]
impl TaskRunner for DockerRunner {
    /// Reads `spec.pull_policy` (`docker create --pull`), `spec.cpu`
    /// (`--cpus`), `spec.memory` (`--memory`) and `spec.delete` (W5a Task 5);
    /// `resources` is the Kubernetes runner's.
    ///
    /// # Re-attach, and what ends a container (W5a spec §5, as the pod)
    ///
    /// The container is looked up by [`container_name`] -- **(run, attempt)**
    /// -- before anything is created:
    ///
    /// * **absent**: create it, copy the input files in, `start --attach`;
    /// * **created, never started** (a holder died between `create` and
    ///   `start`): nothing of the author's ran, so it is removed and created
    ///   afresh -- the input copy may have been cut short;
    /// * **running or exited**: this call adopts it, follows `docker logs
    ///   --follow` from the attempt's newest stored line minus the margin
    ///   ([`with_logs`](DockerRunner::with_logs); without it, from the start,
    ///   and lines repeat), waits for the exit code and collects the outputs;
    /// * **being removed**: a cancel won; the task fails saying so.
    ///
    /// Then, exactly as the Kubernetes runner: **`run` removes nothing**.
    /// [`release`](TaskRunner::release) removes the container (`delete`,
    /// default `true`) once the outcome has committed (final review I3); a
    /// worker failure (`Error::Internal`, the daemon went away) leaves it for
    /// the next claim to re-attach to; and **dropping this future removes
    /// nothing** -- there is no `Drop` on this path, so a worker whose lease was
    /// taken (W5a Task 3) leaves the container running for the new holder.
    async fn run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        spec: &RunnerSpec,
        observer: &dyn LineObserver,
    ) -> Result<CommandsResult> {
        // `Docker.run`'s own first act, and its own message.
        if commands.container_image.trim().is_empty() {
            return Err(Error::InvalidInput(
                "This task runner needs the `containerImage` property to be set".to_string(),
            ));
        }
        // An input name is the author's: refused before a container exists.
        for rel in commands
            .input_files
            .keys()
            .chain(commands.stored_inputs.keys())
        {
            check_relative(rel)?;
        }

        let name = container_name(task);
        let result = match self.find(&name).await? {
            Found::Absent => {
                self.create_and_run(task, commands, spec, observer, &name)
                    .await
            }
            Found::Created => {
                let out = self.cli(&["rm", "--force", &name]).await?;
                if !out.status.success() {
                    return Err(self
                        .classify("rm", &String::from_utf8_lossy(&out.stderr))
                        .await);
                }
                self.create_and_run(task, commands, spec, observer, &name)
                    .await
            }
            Found::Going => Err(Error::InvalidInput(format!(
                "the task's container {name} is being removed (the run was cancelled)"
            ))),
            Found::Started => {
                tracing::info!(run = %task.run_id, container = %name, "re-attaching to the task's existing container");
                self.reattach(task, commands, observer, &name).await
            }
        };

        if let Err(Error::Internal(e)) = &result {
            tracing::warn!(
                run = %task.run_id,
                container = %name,
                "the task container is kept for a later claim to re-attach to: {e}"
            );
        }
        // Nothing is removed here, whatever the result: `release`, after the
        // outcome commits (final review I3).
        result
    }

    /// The container, only if one was **started** for this (run, attempt):
    /// followed to its end exactly as [`run`](TaskRunner::run)'s re-attach.
    /// A container that was created and never started ran nothing of the
    /// author's, so there is nothing to finish: `None`, and `release` removes
    /// it once the refusal commits.
    async fn adoptable(&self, task: &TaskInput) -> Result<bool> {
        Ok(matches!(
            self.find(&container_name(task)).await?,
            Found::Started
        ))
    }

    async fn adopt(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        _spec: &RunnerSpec,
        observer: &dyn LineObserver,
    ) -> Result<Option<CommandsResult>> {
        let name = container_name(task);
        match self.find(&name).await? {
            Found::Started => {
                tracing::info!(
                    run = %task.run_id,
                    container = %name,
                    "re-attaching to the task's existing container, which passed the gate \
                     when it was created"
                );
                let result = self.reattach(task, commands, observer, &name).await;
                if let Err(Error::Internal(e)) = &result {
                    tracing::warn!(run = %task.run_id, container = %name, "the task container is kept for a later claim to re-attach to: {e}");
                }
                result.map(Some)
            }
            Found::Absent | Found::Created | Found::Going => Ok(None),
        }
    }

    /// Remove the attempt's container: `docker rm --force`, which is
    /// `killContainerCmd` + delete, Kestra's two shipped defaults
    /// (`$default$killGracePeriod` = `Duration.ZERO`, `$default$delete` =
    /// `true`) in one call. A grace period is deliberately not offered.
    ///
    /// **Addressed by (run, attempt), as the Kubernetes runner's pod** -- see
    /// [`container_name`] for why that no longer lets a reaped worker kill its
    /// replacement's work.
    ///
    /// **A container that is not there is `Ok(())`**, which is the artifact's
    /// answer too -- `Docker.kill`'s exception table catches
    /// `NotFoundException` and returns without logging.
    ///
    /// # With a `grace` (W5b): `docker stop -t <grace>` first
    ///
    /// The plugin host's shutdown hook needs its SIGTERM and the time to run
    /// the plugin's `kill()` (spec §7.1). `docker stop` sends SIGTERM to PID
    /// 1, SIGKILL after `grace` (whole seconds, rounded up), and **blocks
    /// until the container has stopped** -- which is what makes `cancel`
    /// return only then. Its wait is bounded by `grace` +
    /// [`CANCEL_WAIT_MARGIN`] in case the daemon never answers. Then W5a's
    /// `rm --force`, unchanged. A grace of zero is W5a's path exactly.
    async fn cancel(&self, task: &TaskInput, grace: std::time::Duration) -> Result<()> {
        let name = container_name(task);
        if !grace.is_zero() {
            // A stop that failed must not leave the container running: log
            // it and remove anyway. `rm --force` surfaces its own error if
            // the daemon is really gone.
            if let Err(e) = self.stop(task, &name, grace).await {
                tracing::warn!(
                    run = %task.run_id,
                    container = %name,
                    "`docker stop` failed, removing the task container anyway: {e}"
                );
            }
        }
        let out = self.cli(&["rm", "--force", &name]).await?;
        if out.status.success() {
            return Ok(());
        }
        // **There is deliberately no "No such container" branch**, and the
        // whole-branch review's Minor 2 is why: `docker rm --force` on a name
        // that does not exist exits **0** (measured, daemon 28.4.0), so that
        // branch was unreachable. The absent container is handled by the
        // success arm above.
        Err(self
            .classify("rm", &String::from_utf8_lossy(&out.stderr))
            .await)
    }

    /// `docker rm --force` when `spec.delete` (the default); with
    /// `delete: false` the container is kept, exited -- measured on the
    /// artifact -- and the orphan sweep leaves it too (its label).
    async fn release(&self, task: &TaskInput, spec: &RunnerSpec) -> Result<()> {
        if !spec.delete {
            return Ok(());
        }
        // Grace 0: the outcome has committed, so there is nothing left to
        // let finish.
        self.cancel(task, std::time::Duration::ZERO).await
    }
}

impl DockerRunner {
    /// `docker stop -t <grace>` on `name`, for [`TaskRunner::cancel`]: blocks
    /// until the container has stopped, bounded by `grace` +
    /// [`CANCEL_WAIT_MARGIN`] (a WARN and `Ok(())` past it -- the outcome is
    /// the budget's either way). A container that is not there, or is already
    /// being removed, is `Ok(())`, as `rm --force`'s answer is.
    async fn stop(&self, task: &TaskInput, name: &str, grace: std::time::Duration) -> Result<()> {
        let secs = (grace.as_secs() + u64::from(grace.subsec_nanos() > 0)).to_string();
        let bound = grace + CANCEL_WAIT_MARGIN;
        let Ok(out) = tokio::time::timeout(bound, self.cli(&["stop", "-t", &secs, name])).await
        else {
            tracing::warn!(
                run = %task.run_id,
                container = %name,
                "the task container did not stop within {bound:?} of its SIGTERM; removing it"
            );
            return Ok(());
        };
        let out = out?;
        if out.status.success() {
            return Ok(());
        }
        // `docker stop` on a name that does not exist exits 1, unlike `rm
        // --force`; the probe decides, not the stderr.
        if matches!(self.find(name).await?, Found::Absent | Found::Going) {
            return Ok(());
        }
        Err(self
            .classify("stop", &String::from_utf8_lossy(&out.stderr))
            .await)
    }

    /// The state of the container called `name`, per [`Found`].
    ///
    /// A failed `inspect` is **absent** when the daemon answers the probe and
    /// egret's outage when it does not -- the probe, not the stderr, decides
    /// ([`_CLASSIFICATION_IS_A_PROBE_NOT_A_SUBSTRING_TEST`]).
    async fn find(&self, name: &str) -> Result<Found> {
        let out = self
            .cli(&[
                "inspect",
                "--type",
                "container",
                "--format",
                "{{.State.Status}}",
                name,
            ])
            .await?;
        if !out.status.success() {
            if self.daemon_answers().await {
                return Ok(Found::Absent);
            }
            return Err(Error::Internal(anyhow::anyhow!(
                "the Docker daemon could not be reached, so the task's container could not be \
                 looked up: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(match String::from_utf8_lossy(&out.stdout).trim() {
            "created" => Found::Created,
            "removing" | "dead" => Found::Going,
            _ => Found::Started,
        })
    }

    /// `docker create` with the task's name, labels, env, runner properties
    /// and argv, then [`run_created`](Self::run_created).
    async fn create_and_run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        spec: &RunnerSpec,
        observer: &dyn LineObserver,
        name: &str,
    ) -> Result<CommandsResult> {
        let borrowed_args = create_args(task, commands, spec, name);
        let borrowed: Vec<&str> = borrowed_args.iter().map(String::as_str).collect();
        let created = self.cli(&borrowed).await?;
        if !created.status.success() {
            return Err(self
                .classify("create", &String::from_utf8_lossy(&created.stderr))
                .await);
        }
        self.run_created(task, commands, observer, name).await
    }

    /// Adopt a container a previous holder started: follow its log from the
    /// re-attach window, wait for its exit code, collect its outputs.
    async fn reattach(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        observer: &dyn LineObserver,
        name: &str,
    ) -> Result<CommandsResult> {
        let (since, mut replayed) = match &self.logs {
            Some(logs) => crate::worker::k8s::runner::reattach_window(logs, task).await?,
            None => (None, crate::worker::k8s::runner::Replayed::default()),
        };
        let mut args: Vec<String> = vec!["logs".into(), "--follow".into()];
        if let Some(since) = since {
            args.push("--since".into());
            args.push(since.timestamp().to_string());
        }
        args.push(name.to_string());
        let mut child = Command::new(&self.binary)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!("could not run `{} logs`: {e}", self.binary))
            })?;
        let mut result = self
            .pump(
                task,
                observer,
                &mut child,
                Some(&mut replayed),
                commands.host_protocol,
            )
            .await?;
        child.wait().await.map_err(io_err)?;

        // `docker wait` rather than `inspect`: the log stream can close a
        // moment before the daemon records the exit, and `wait` returns the
        // code once it has (immediately, for a container already exited).
        let waited = self.cli(&["wait", name]).await?;
        if !waited.status.success() {
            return Err(self
                .classify("wait", &String::from_utf8_lossy(&waited.stderr))
                .await);
        }
        let raw = String::from_utf8_lossy(&waited.stdout).trim().to_string();
        result.exit_code = raw.parse::<i32>().map_err(|_| {
            Error::Internal(anyhow::anyhow!(
                "docker wait returned {raw:?} for the exit code of {name}"
            ))
        })?;

        if commands.has_outputs() && result.failure.is_none() {
            self.collect_output_files(name, commands, &mut result)
                .await?;
        }
        Ok(result)
    }

    /// Read both of `child`'s pipes to their end, every line through
    /// [`stream_line`]; a line the previous holder already stored
    /// (`replayed`) is dropped once.
    async fn pump(
        &self,
        task: &TaskInput,
        observer: &dyn LineObserver,
        child: &mut tokio::process::Child,
        mut replayed: Option<&mut crate::worker::k8s::runner::Replayed>,
        host_protocol: bool,
    ) -> Result<CommandsResult> {
        let mut out_lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
        let mut err_lines = BufReader::new(child.stderr.take().expect("stderr piped")).lines();

        let mut result = CommandsResult::default();
        let mut state = StreamState::default();
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
            match replayed.as_deref_mut() {
                Some(r) => {
                    stream_replayed_line(
                        task,
                        observer,
                        &mut result,
                        &mut state,
                        self.max_vars_bytes,
                        &line,
                        is_err,
                        host_protocol,
                        r,
                    )
                    .await
                }
                None => {
                    stream_line(
                        task,
                        observer,
                        &mut result,
                        &mut state,
                        self.max_vars_bytes,
                        &line,
                        is_err,
                        host_protocol,
                    )
                    .await
                }
            }
        }
        Ok(result)
    }

    /// The input files into `name`'s working directory: `docker cp -`, fed
    /// by [`write_tar`](crate::storage::transfer::write_tar) on its stdin.
    ///
    /// The split (spec §11): an author's name or object problem fails the
    /// task; the store being unreachable is egret's outage (`Internal`); and a
    /// `docker cp` that stopped reading or failed is classified like every
    /// other docker call -- the daemon answering makes it the task's.
    async fn copy_in(&self, commands: &TaskCommands, name: &str) -> Result<()> {
        use crate::storage::transfer::{write_tar, TarError};
        let sources = commands.tar_sources()?;
        let dst = format!("{name}:{WORKING_DIR}");
        let mut child = Command::new(&self.binary)
            .args(["cp", "-", &dst])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // As `cli`: a dropped future (`timeout::guard`) must not orphan
            // the `docker cp`.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!("could not run `{} cp`: {e}", self.binary))
            })?;
        // stderr is read BESIDE the write (fix round 1, M3): a `docker cp`
        // that filled its stderr pipe before reading stdin would otherwise
        // block, and the write with it. Kept to 64 KiB, the rest drained.
        let stderr = child.stderr.take().expect("stderr piped");
        let errs = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut kept = Vec::new();
            let mut stderr = stderr;
            let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut kept).await;
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            kept
        });
        let mut stdin = child.stdin.take().expect("stdin piped");
        let wrote = write_tar(&mut stdin, &sources, self.files.as_deref()).await;
        if let Err(e @ (TarError::Author(_) | TarError::Store(_))) = wrote {
            // OUR side stopped part-way. `docker cp` is killed and reaped
            // BEFORE its stdin closes: an end of file would hand it a
            // cut-short archive to extract.
            let _ = child.kill().await;
            drop(stdin);
            return Err(staging_error(e));
        }
        // EOF for `docker cp`. After a WRITE failure, `docker cp` stopped
        // reading: its own exit, classified, says why.
        drop(stdin);
        let status = child.wait().await.map_err(io_err)?;
        let err = errs.await.unwrap_or_default();
        if !status.success() {
            return Err(self.classify("cp", &String::from_utf8_lossy(&err)).await);
        }
        match wrote {
            // `docker cp` exited 0 having not read the whole archive.
            Err(e) => Err(staging_error(e)),
            Ok(()) => Ok(()),
        }
    }

    /// The half of a fresh run that runs with a container in existence:
    /// input files in, `start --attach`, exit code, output files out.
    async fn run_created(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        observer: &dyn LineObserver,
        name: &str,
    ) -> Result<CommandsResult> {
        // ---- input files ------------------------------------------------
        //
        // `docker cp` into a container that has never started, which is
        // Kestra's `copyArchiveToContainer` and the reason the working
        // directory does not have to exist first: measured on this machine's
        // daemon (28.4.0), a copy into `/egret/working-dir` creates it.
        if commands.has_inputs() {
            // **Streamed (W9a spec §5)**: `docker cp -` reads a ustar archive
            // from its stdin, written by [`write_tar`] as the stored inputs
            // are read from the store. Nothing is staged on the worker's
            // disk any more -- W5a's staging directory held the author's
            // rendered `script:` and the plugin's `task.json` -- and no file
            // is held whole in memory.
            //
            // # `input_files` KEYS ARE AUTHOR-SUPPLIED SINCE W5a
            //
            // [`check_relative`] (in `run`) refuses `..`, absolute paths, `.`
            // and globs, and `write_tar` refuses a name that is also another's
            // parent (`{"a": .., "a/b": ..}`) before writing a byte -- both
            // the TASK's failure, never a worker failure.
            //
            // [`write_tar`]: crate::storage::transfer::write_tar
            self.copy_in(commands, name).await?;
        }

        // ---- start, and stream --------------------------------------------
        let mut child = Command::new(&self.binary)
            .args(["start", "--attach", name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The attach is the long one, so it is the one most likely to be
            // dropped mid-flight. `kill_on_drop` reaps the CLI; the container
            // itself is the daemon's and keeps running -- the next holder
            // re-attaches to it, and `cancel` is what stops it.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "could not run `{} start`: {e}",
                    self.binary
                ))
            })?;

        let mut result = self
            .pump(task, observer, &mut child, None, commands.host_protocol)
            .await?;
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
        if commands.has_outputs() && result.failure.is_none() {
            self.collect_output_files(name, commands, &mut result)
                .await?;
        }

        Ok(result)
    }

    /// Stream **only the declared files** out, one `docker cp c:<path> -` per
    /// name, each read entry by entry ([`TarReader`]) straight into internal
    /// storage ([`collect_declared`]); then, with `outputDirectory: true`,
    /// the whole [`OUTPUT_DIR`] the same way ([`collect_directory`]).
    ///
    /// # W9a Task 6 (2026-10-01): no host staging directory any more
    ///
    /// W5a copied each declared file to a staging directory on the worker and
    /// then had to refuse symlinks component by component, because a link
    /// `docker cp` recreated there resolved against the WORKER HOST
    /// ([`_SYMLINKS_ARE_REFUSED_BY_NAME`]). Read as a stream, a link is an archive
    /// entry with no contents: it is refused by name and nothing is followed,
    /// on any host. `docker cp c:<wd>/etcdir/passwd -` still resolves an
    /// intermediate link inside the container, so that leg returns the
    /// container's own file, as before.
    ///
    /// # Whose error
    ///
    /// An author's problem (a cap, a directory, a link, a name no URI can
    /// carry) lands in [`CommandsResult::failure`] with the exit code and the
    /// vars intact. `Err` is egret's: the store did not answer (an outage,
    /// the run is re-claimed with no attempt spent), or the copy broke off.
    /// Whether the daemon is there is asked with
    /// [`daemon_answers`](DockerRunner::daemon_answers), never read from
    /// stderr ([`_CLASSIFICATION_IS_A_PROBE_NOT_A_SUBSTRING_TEST`]): a copy
    /// that failed while the daemon answers is a file that is not there,
    /// IGNORED as Kestra ignores it.
    ///
    /// [`TarReader`]: crate::storage::transfer::TarReader
    /// [`collect_declared`]: crate::storage::collect::collect_declared
    /// [`collect_directory`]: crate::storage::collect::collect_directory
    async fn collect_output_files(
        &self,
        name: &str,
        commands: &TaskCommands,
        result: &mut CommandsResult,
    ) -> Result<()> {
        use crate::storage::collect::{collect_declared, collect_directory, CollectError};
        use crate::storage::transfer::TarReader;
        // Every declared name is vetted BEFORE anything is interpolated into
        // a `docker` argument, so `..` never reaches the daemon.
        for rel in &commands.output_files {
            if let Some(problem) = output_problem(rel) {
                result.failure = Some(problem);
                return Ok(());
            }
        }
        let (fs, to) = match collection_target(self.files.as_ref(), commands) {
            Ok(both) => both,
            Err(problem) => {
                result.failure = Some(problem);
                return Ok(());
            }
        };
        // Patterns are resolved against `docker diff` -- the paths the
        // container added, which works on a stopped container and moves no
        // file contents -- then read back one file at a time like a literal.
        let dirs = pattern_dirs(&commands.output_files);
        let listed = if dirs.is_empty() {
            std::collections::BTreeMap::new()
        } else {
            match self.diff_listing(name, &dirs).await? {
                Ok(listed) => listed,
                Err(problem) => {
                    result.failure = Some(problem);
                    return Ok(());
                }
            }
        };
        let names = match expand_outputs(&commands.output_files, &listed, MAX_OUTPUT_NAMES) {
            Ok(n) => n,
            Err(problem) => {
                result.failure = Some(problem);
                return Ok(());
            }
        };
        let mut so_far = 0u64;
        let mut names_left = MAX_OUTPUT_NAMES.saturating_sub(names.len());
        for (rel, from_glob) in &names {
            let src = format!("{name}:{WORKING_DIR}/{rel}");
            let mut copy = self.copy_out(&src).await?;
            let got = {
                let mut reader = TarReader::new(&mut copy.stdout);
                collect_declared(&fs, &to, rel, *from_glob, &mut reader, &mut so_far).await
            };
            let finished = copy.finish(got.is_ok()).await;
            match got {
                Ok(Some(file)) => {
                    result.output_files.insert(rel.clone(), file);
                }
                Ok(None) => {
                    if !finished && !self.daemon_answers().await {
                        return Err(Error::Internal(anyhow::anyhow!(
                            "the Docker daemon could not be reached, so {rel:?} could not be \
                             copied out of {name}: {}",
                            copy.stderr().await
                        )));
                    }
                }
                Err(CollectError::Author(problem)) => {
                    result.failure = Some(problem);
                    return Ok(());
                }
                Err(e) => return Err(self.collect_failure(name, rel, e, &mut copy).await),
            }
        }
        if commands.output_directory {
            let src = format!("{name}:{OUTPUT_DIR}");
            let mut copy = self.copy_out(&src).await?;
            let got = {
                let mut reader = TarReader::new(&mut copy.stdout);
                collect_directory(
                    &fs,
                    &to,
                    &mut reader,
                    &mut so_far,
                    &mut names_left,
                    &mut result.output_files,
                )
                .await
            };
            let finished = copy.finish(got.is_ok()).await;
            match got {
                Ok(()) => {
                    if !finished && !self.daemon_answers().await {
                        return Err(Error::Internal(anyhow::anyhow!(
                            "the Docker daemon could not be reached, so {OUTPUT_DIR} could not \
                             be copied out of {name}: {}",
                            copy.stderr().await
                        )));
                    }
                }
                Err(CollectError::Author(problem)) => result.failure = Some(problem),
                Err(e) => return Err(self.collect_failure(name, OUTPUT_DIR, e, &mut copy).await),
            }
        }
        Ok(())
    }

    /// `docker cp <src> -`: the archive on its stdout, its stderr read beside
    /// it (64 KiB kept) so a chatty CLI cannot block the copy.
    async fn copy_out(&self, src: &str) -> Result<CopyOut> {
        let mut child = Command::new(&self.binary)
            .args(["cp", src, "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!("could not run `{} cp`: {e}", self.binary))
            })?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let errs = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut kept = Vec::new();
            let mut stderr = stderr;
            let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut kept).await;
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            kept
        });
        Ok(CopyOut {
            child,
            stdout,
            errs: Some(errs),
            kept: None,
        })
    }

    /// An `Err` out of collection that is not the author's: the store (an
    /// outage), or the copy breaking off -- the daemon gone, or a stream egret
    /// could not read. Either way egret's, and the run is re-claimed.
    async fn collect_failure(
        &self,
        name: &str,
        what: &str,
        e: crate::storage::collect::CollectError,
        copy: &mut CopyOut,
    ) -> Error {
        use crate::storage::collect::CollectError;
        match e {
            CollectError::Store(m) => Error::Internal(anyhow::anyhow!("{m}")),
            CollectError::Source(m) if !self.daemon_answers().await => {
                Error::Internal(anyhow::anyhow!(
                    "the Docker daemon could not be reached, so {what:?} could not be copied \
                     out of {name}: {m} ({})",
                    copy.stderr().await
                ))
            }
            CollectError::Source(m) | CollectError::Author(m) => Error::Internal(anyhow::anyhow!(
                "copying {what:?} out of {name} broke off: {m} ({})",
                copy.stderr().await
            )),
        }
    }
}

/// One `docker cp <src> -` in flight ([`DockerRunner::copy_out`]).
struct CopyOut {
    child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    errs: Option<tokio::task::JoinHandle<Vec<u8>>>,
    kept: Option<String>,
}

impl CopyOut {
    /// End the copy. After a clean read (`drain`), what is left of the
    /// archive (the end blocks) is read, at most 1 MiB, and the CLI's own
    /// exit is waited for: `true` when it succeeded. Otherwise -- a refusal
    /// part-way, or a declared directory whose whole tree is not wanted --
    /// it is killed, and `false`.
    async fn finish(&mut self, drain: bool) -> bool {
        use tokio::io::AsyncReadExt;
        if drain {
            let left = tokio::io::copy(
                &mut (&mut self.stdout).take(1 << 20),
                &mut tokio::io::sink(),
            )
            .await
            .unwrap_or(0);
            if left < 1 << 20 {
                return self.child.wait().await.is_ok_and(|s| s.success());
            }
        }
        let _ = self.child.kill().await;
        false
    }

    /// The CLI's stderr, trimmed (read once).
    async fn stderr(&mut self) -> String {
        if let Some(errs) = self.errs.take() {
            let bytes = errs.await.unwrap_or_default();
            self.kept = Some(String::from_utf8_lossy(&bytes).trim().to_string());
        }
        self.kept.clone().unwrap_or_default()
    }
}

/// The store and the target for a run's output files, or the task's failure
/// naming `[storage]` (W9a spec §4.6, §6.2): W5a's contents-in-outputs path
/// no longer exists, so without internal storage nothing can be collected.
pub(crate) fn collection_target(
    files: Option<&std::sync::Arc<crate::storage::FileStore>>,
    commands: &TaskCommands,
) -> std::result::Result<(std::sync::Arc<crate::storage::FileStore>, CollectTo), String> {
    match (files, &commands.collect_to) {
        (Some(fs), Some(to)) => Ok((fs.clone(), to.clone())),
        _ => Err(no_storage_for_outputs()),
    }
}

/// The message a declared output fails with when egret has no internal
/// storage to put it in.
pub(crate) fn no_storage_for_outputs() -> String {
    "`outputFiles` and `outputDirectory` need internal storage, and egret has none \
     configured (the `[storage]` section of its configuration): since W9a an output \
     file is a kestra:// URI, not its contents"
        .to_string()
}

/// The label every egret task container **and** task pod carries with its
/// `task_runs.id` — what an orphan sweep reads to find the run a piece of
/// work belongs to.
pub const TASKRUN_LABEL: &str = "egret.io/taskrun-id";

/// The prefix of every task container's and task pod's name.
pub const RUN_NAME_PREFIX: &str = "egret-run-";

/// **Clean up the work a runner left behind for runs that have ended** —
/// one interface for both runners (W5a Task 4, carrying Task 3's finding).
///
/// Task 3 measured that a Docker container whose worker lost its lease keeps
/// running, and until re-attach exists the new holder starts a second one; a
/// pod is left behind the same way when egret dies between downloading a
/// task's outputs and deleting its pod (spec §5, *Orphans*). Neither is
/// anybody's to delete while its run is live, and both are garbage once the
/// run is terminal. The reaper's tick calls this (Task 5 wires the call site).
///
/// Returns how many were removed. `Err` is the runner's backend being
/// unreachable; one item that fails to delete is logged and not counted.
#[async_trait::async_trait]
pub trait OrphanSweep: Send + Sync {
    async fn sweep_orphans(&self, pool: &sqlx::PgPool) -> Result<u64>;
}

/// Of `ids`, which `task_runs` rows are terminal, and which have no row at
/// all — the one question both halves of [`OrphanSweep`] ask the database.
///
/// Terminal is [`RunState::is_terminal`](crate::executor::RunState::is_terminal),
/// Kestra's `isTerminated()`, spelled here by the enum rather than a copied
/// list so the two cannot drift.
pub(crate) async fn run_liveness(pool: &sqlx::PgPool, ids: &[uuid::Uuid]) -> Result<RunLiveness> {
    let terminal: Vec<&str> = crate::executor::RunState::ALL
        .iter()
        .filter(|s| s.is_terminal())
        .map(|s| s.wire())
        .collect();
    let rows: Vec<(uuid::Uuid, bool)> =
        sqlx::query_as("SELECT id, state = ANY($2) FROM workflow.task_runs WHERE id = ANY($1)")
            .bind(ids)
            .bind(&terminal)
            .fetch_all(pool)
            .await
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "reading run states for the orphan sweep: {e}"
                ))
            })?;
    let mut out = RunLiveness::default();
    let mut seen = std::collections::HashSet::new();
    for (id, is_terminal) in rows {
        seen.insert(id);
        if is_terminal {
            out.terminal.insert(id);
        }
    }
    for id in ids {
        if !seen.contains(id) {
            out.missing.insert(*id);
        }
    }
    Ok(out)
}

/// [`run_liveness`]'s answer.
#[derive(Debug, Default)]
pub(crate) struct RunLiveness {
    /// A row exists and its state is terminal.
    pub terminal: std::collections::HashSet<uuid::Uuid>,
    /// No row at all (the execution was deleted, cascading its runs — or the
    /// work belongs to ANOTHER egret sharing the backend).
    pub missing: std::collections::HashSet<uuid::Uuid>,
}

/// **The Docker half of the sweep.**
///
/// Lists every container carrying [`TASKRUN_LABEL`] whose name starts with
/// [`RUN_NAME_PREFIX`], and force-removes those whose run is **terminal**.
///
/// # A container whose run has NO row is left alone, unlike a pod
///
/// A Docker daemon is shared: a developer's machine runs the dev stack, the
/// test suites and anything else against one daemon, each with its own
/// database. A container whose run id is absent from *this* database is far
/// more likely to be another egret's live work than an orphan of ours, so
/// only a row this database can see as terminal authorises the removal. The
/// pod sweep differs on purpose: the task namespace is egret's alone
/// ([`k8s::sweep_orphans`](super::k8s::sweep_orphans)).
#[async_trait::async_trait]
impl OrphanSweep for DockerRunner {
    async fn sweep_orphans(&self, pool: &sqlx::PgPool) -> Result<u64> {
        let label_filter = format!("label={TASKRUN_LABEL}");
        let format = format!(
            "{{{{.Names}}}}\t{{{{.Label \"{TASKRUN_LABEL}\"}}}}\t{{{{.Label \"{DELETE_LABEL}\"}}}}"
        );
        let out = self
            .cli(&[
                "ps",
                "--all",
                "--filter",
                &label_filter,
                "--format",
                &format,
            ])
            .await?;
        if !out.status.success() {
            return Err(self
                .classify("ps", &String::from_utf8_lossy(&out.stderr))
                .await);
        }
        let listed = parse_docker_ps(&String::from_utf8_lossy(&out.stdout));
        if listed.is_empty() {
            return Ok(0);
        }
        let ids: Vec<uuid::Uuid> = listed.iter().map(|(_, id)| *id).collect();
        let live = run_liveness(pool, &ids).await?;
        let mut removed = 0u64;
        for (name, id) in &listed {
            if !live.terminal.contains(id) {
                continue;
            }
            match self.cli(&["rm", "--force", name]).await {
                Ok(o) if o.status.success() => removed += 1,
                Ok(o) => tracing::warn!(
                    container = %name,
                    "orphan sweep could not remove the container: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => tracing::warn!(container = %name, "orphan sweep: {e}"),
            }
        }
        Ok(removed)
    }
}

/// `docker ps --format '{{.Names}}\t{{.Label "egret.io/taskrun-id"}}\t{{.Label "egret.io/delete"}}'`,
/// parsed: the egret task containers only (name prefix AND a label that is a
/// UUID), **minus those the author asked to keep** (`delete: false`, the
/// [`DELETE_LABEL`] -- the pod sweep's rule, W5a Task 5).
fn parse_docker_ps(stdout: &str) -> Vec<(String, uuid::Uuid)> {
    stdout
        .lines()
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let name = cols.next()?;
            let id = cols.next()?;
            let keep = cols.next().is_some_and(|d| d.trim() == "false");
            if !name.starts_with(RUN_NAME_PREFIX) || keep {
                return None;
            }
            Some((name.to_string(), uuid::Uuid::parse_str(id.trim()).ok()?))
        })
        .collect()
}

/// `docker create`'s argv for one task: name, working directory, labels, env,
/// the `taskRunner:` properties the Docker runner implements, entrypoint,
/// image and CMD. Pure, so the mapping is asserted without a daemon.
///
/// * `pullPolicy` → `--pull`: `IF_NOT_PRESENT` is `missing` (the CLI's own
///   default, written out), `ALWAYS` is `always`, `NEVER` is `never`.
/// * `cpu.cpus` → `--cpus`, `memory.memory` → `--memory`, as written; a value
///   the daemon cannot parse fails `create`, i.e. the task, with its words.
/// * `delete: false` → the [`DELETE_LABEL`] label, which the orphan sweep reads.
///
/// **No disk bound** (final review I5, a known limit): Docker has no portable
/// per-container disk limit -- `--storage-opt size=` works only on overlay2
/// over xfs with `pquota` -- so a task on this runner can fill the daemon's
/// disk. The Kubernetes runner bounds it (`ephemeral-storage` and
/// `emptyDir.sizeLimit`); this runner is for local development and tests.
pub(crate) fn create_args(
    task: &TaskInput,
    commands: &TaskCommands,
    spec: &RunnerSpec,
    name: &str,
) -> Vec<String> {
    let argv = commands.argv();
    let mut create: Vec<String> = vec![
        "create".into(),
        "--name".into(),
        name.to_string(),
        "--workdir".into(),
        WORKING_DIR.into(),
        "--pull".into(),
        match spec.pull_policy {
            crate::scripts::PullPolicy::Always => "always",
            crate::scripts::PullPolicy::IfNotPresent => "missing",
            crate::scripts::PullPolicy::Never => "never",
        }
        .into(),
    ];
    if let Some(cpus) = &spec.cpu {
        create.push("--cpus".into());
        create.push(cpus.clone());
    }
    if let Some(memory) = &spec.memory {
        create.push("--memory".into());
        create.push(memory.clone());
    }
    for (k, v) in labels(task) {
        create.push("--label".into());
        create.push(format!("{k}={v}"));
    }
    if !spec.delete {
        create.push("--label".into());
        create.push(format!("{DELETE_LABEL}=false"));
    }
    create.push("--env".into());
    create.push(format!("{ENV_WORKING_DIR}={WORKING_DIR}"));
    if commands.output_directory {
        create.push("--env".into());
        create.push(format!("{ENV_OUTPUT_DIR}={OUTPUT_DIR}"));
    }
    for (k, v) in &commands.env {
        create.push("--env".into());
        create.push(format!("{k}={v}"));
    }
    // `withEntrypoint(interpreter)` + `withCmd(joined)`. The CLI takes one
    // string for `--entrypoint`, so the interpreter's head is the entrypoint
    // and its tail leads the CMD — which produces the identical argv, because
    // the daemon concatenates the two.
    create.push("--entrypoint".into());
    create.push(argv[0].clone());
    // `--` ends `docker create`'s flags, so the image can only ever be the
    // IMAGE positional. Without it an image spelled like a flag
    // (`--privileged`) WAS a flag, and the author's interpreter tail then
    // supplied the rest (`-v /:/host alpine …`) -- final review I1. The gate
    // refuses such an image first (`gate::image_ref_problem`); this is the
    // second line, and it holds on its own.
    create.push("--".into());
    create.push(commands.container_image.clone());
    create.extend(argv[1..].iter().cloned());
    create
}

/// What a failed [`write_tar`](crate::storage::transfer::write_tar) is to a
/// runner (spec §11): the author's problem fails the task
/// (`InvalidInput`); the store or the transfer failing is egret's
/// (`Internal`: the lease lapses, the run is re-claimed, no attempt is spent).
pub(crate) fn staging_error(e: crate::storage::transfer::TarError) -> Error {
    use crate::storage::transfer::TarError;
    match e {
        TarError::Author(m) => Error::InvalidInput(m),
        e @ (TarError::Store(_) | TarError::Write(_)) => Error::Internal(anyhow::anyhow!("{e}")),
    }
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

/// The same rule as a **message or nothing**, so the collectors can use
/// it without an [`Error`] it would only have to unwrap again.
pub(crate) fn relative_problem(rel: &str) -> Option<String> {
    if rel.is_empty() {
        return Some("a file name must not be empty".to_string());
    }
    if rel.starts_with('/') {
        return Some(format!(
            "{rel:?} is an absolute path; a file is named relative to {WORKING_DIR}"
        ));
    }
    // W9a Task 5 fix round 1, M1: a NUL ends the name for every C-string
    // reader (`tar`, the kernel) -- in a ustar prefix it would make
    // `x\0/../..` read as something else than egret checked -- and no
    // control character belongs in a file name a task is asked to read.
    if rel.chars().any(char::is_control) {
        return Some(format!(
            "{rel:?} contains a control character; a file name may not"
        ));
    }
    if rel.split('/').any(|seg| seg == "..") {
        return Some(format!("{rel:?} climbs out of {WORKING_DIR} with `..`"));
    }
    // `.` and `./x` and a trailing `/` are how a declared name asks for a
    // DIRECTORY, and `docker cp c:<wd>/. dst` copies the whole subtree before
    // anything can refuse it — fix round 3's NEW Minor 2. The refusal for a
    // directory that is only discovered after the copy still happens in
    // collection; this closes the spellings that ask for one by name.
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

/// Is a declared output a pattern (`*` or `?` in its file name) rather than a
/// literal name.
pub(crate) fn is_pattern(rel: &str) -> bool {
    rel.contains(['*', '?'])
}

/// [`relative_problem`] for a DECLARED output, which may be a non-recursive
/// glob (W5a Task 5 fix round 1, following `w5a-scripts.json`
/// `egret_expected.output_files`: `*.csv` keys `a.csv`, `b.csv` and not
/// `sub/c.csv`).
///
/// `*` (any run of characters) and `?` (one character) are accepted in the
/// FILE NAME -- the last segment -- and match within the one directory the
/// rest of the name gives (`sub/*.csv`). Refused by name, never ignored: a
/// wildcard in a directory segment, `**`, and `[...]` classes (not
/// implemented; `scripts::parse` refuses a literal one at save).
pub(crate) fn output_problem(rel: &str) -> Option<String> {
    if rel.contains('[') || rel.contains("**") {
        return Some(format!(
            "{rel:?}: `**` and `[...]` in `outputFiles` are not implemented by egret \
             (W5a); `*` and `?` in the file name are"
        ));
    }
    let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel));
    if is_pattern(file) && file.len() > MAX_PATTERN_BYTES {
        return Some(format!(
            "an `outputFiles` pattern's file name is {} bytes, over the {MAX_PATTERN_BYTES} \
             a file name can have",
            file.len()
        ));
    }
    if is_pattern(dir) {
        return Some(format!(
            "{rel:?}: a wildcard in a directory of `outputFiles` is not implemented by \
             egret (W5a); only the file name may be a pattern"
        ));
    }
    let literal = file.replace(['*', '?'], "x");
    let probe = if dir.is_empty() {
        literal
    } else {
        format!("{dir}/{literal}")
    };
    relative_problem(&probe).map(|p| p.replace(&format!("{probe:?}"), &format!("{rel:?}")))
}

/// The limits on the declared LIST (W5a Task 5 fix round 3), judged at save
/// by `scripts::parse` and again at run: at most [`MAX_OUTPUT_NAMES`]
/// entries, and at most [`MAX_OUTPUT_PATTERNS`] distinct patterns.
pub(crate) fn output_list_problem<'a>(
    declared: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let mut entries = 0usize;
    let mut patterns = std::collections::HashSet::new();
    for rel in declared {
        entries += 1;
        if is_pattern(rel) {
            patterns.insert(rel);
        }
    }
    if entries > MAX_OUTPUT_NAMES {
        return Some(format!(
            "`outputFiles` declares {entries} entries, over the {MAX_OUTPUT_NAMES} one task \
             may read back"
        ));
    }
    if patterns.len() > MAX_OUTPUT_PATTERNS {
        return Some(format!(
            "`outputFiles` declares {} distinct patterns, over the {MAX_OUTPUT_PATTERNS} one \
             task may; match fewer, wider patterns",
            patterns.len()
        ));
    }
    None
}

/// Resolve the declared outputs to the names to read back, given the entries
/// directly inside each directory a pattern names (`listed[dir]`, `""` for
/// the working directory). Literal names are kept, in declared order,
/// whether or not they exist (a missing one is ignored when read); each
/// pattern contributes its matches, sorted. A name is returned once. The
/// `bool` is "from a glob". Pure: both runners list, then call this.
///
/// `Err` (the task's message) when the names exceed `max_names` -- per
/// pattern or in total ([`MAX_OUTPUT_NAMES`]).
pub(crate) fn expand_outputs(
    declared: &[String],
    listed: &std::collections::BTreeMap<String, Vec<String>>,
    max_names: usize,
) -> std::result::Result<Vec<(String, bool)>, String> {
    let too_many = |what: &str| {
        format!(
            "`outputFiles` {what} more than {max_names} names; egret reads back at most \
             {max_names} output files per task (each is a copy and an output key, stored on \
             the task run: W9 owns storage)"
        )
    };
    if let Some(problem) = output_list_problem(declared.iter().map(String::as_str)) {
        return Err(problem);
    }
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut seen_patterns = std::collections::HashSet::new();
    let mut budget = GLOB_STEP_BUDGET;
    for rel in declared {
        if !is_pattern(rel) {
            if seen.insert(rel.clone()) {
                out.push((rel.clone(), false));
            }
            continue;
        }
        // A pattern declared twice is matched once.
        if !seen_patterns.insert(rel.as_str()) {
            continue;
        }
        let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel));
        let mut matched: Vec<String> = Vec::new();
        for name in listed.get(dir).into_iter().flatten() {
            match crate::scripts::gate::glob_match_within(file, name, &mut budget) {
                None => {
                    return Err(format!(
                        "matching the `outputFiles` patterns against the working directory \
                         took more than {GLOB_STEP_BUDGET} steps; declare fewer or simpler \
                         patterns, or fewer files"
                    ))
                }
                Some(false) => {}
                Some(true) => {
                    if matched.len() == max_names {
                        return Err(too_many(&format!("pattern {rel:?} matches")));
                    }
                    matched.push(if dir.is_empty() {
                        name.clone()
                    } else {
                        format!("{dir}/{name}")
                    });
                }
            }
        }
        matched.sort();
        for m in matched {
            if seen.insert(m.clone()) {
                out.push((m, true));
            }
        }
    }
    if out.len() > max_names {
        return Err(too_many("resolves to"));
    }
    Ok(out)
}

/// The task's message when a glob's directory listing is over
/// [`MAX_LISTING_BYTES`].
pub(crate) fn listing_too_big(cap: usize) -> String {
    format!(
        "the working directory's listing, needed to resolve the `outputFiles` glob, is \
         over {cap} bytes (too many files); name the output files, or write fewer files"
    )
}

/// The directories whose entries [`expand_outputs`] needs listed.
pub(crate) fn pattern_dirs(declared: &[String]) -> std::collections::BTreeSet<String> {
    declared
        .iter()
        .filter(|rel| is_pattern(rel))
        .map(|rel| rel.rsplit_once('/').map_or("", |(d, _)| d).to_string())
        .collect()
}

/// `docker diff` lines (`A /egret/working-dir/sub/c.csv`), as the entries of
/// each directory in `dirs` that are not a parent of another entry -- i.e.
/// its files (and empty directories, which collection skips for a
/// glob).
fn parse_docker_diff(
    stdout: &str,
    dirs: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let prefix = format!("{WORKING_DIR}/");
    let paths: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.get(2..))
        .filter_map(|p| p.strip_prefix(prefix.as_str()))
        .filter(|p| !p.is_empty())
        .collect();
    let mut out: std::collections::BTreeMap<String, Vec<String>> =
        dirs.iter().map(|d| (d.clone(), Vec::new())).collect();
    // Every proper ancestor of every path, once: linear in the listing's
    // length (fix round 2 -- the first version compared every path with
    // every other, quadratic in a listing the author controls).
    let mut parents: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for p in &paths {
        for (i, b) in p.bytes().enumerate() {
            if b == b'/' {
                parents.insert(&p[..i]);
            }
        }
    }
    for p in &paths {
        if parents.contains(p) {
            continue;
        }
        let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
        if let Some(v) = out.get_mut(dir) {
            v.push(name.to_string());
        }
    }
    out
}

fn io_err(e: std::io::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::fixture;
    use crate::worker::handlers::testing::input;

    /// A task input for run 7; `task` is the task id, so two inputs can
    /// differ in everything but the run and the attempt.
    fn leased(task: i64) -> TaskInput {
        TaskInput {
            run_id: uuid::Uuid::from_u128(7),
            task_id: format!("t{task}"),
            ..input("egret.container", serde_json::json!({}))
        }
    }

    /// **The name is the (run, attempt) re-attach key, and nothing else**
    /// (W5a Task 5; see [`container_name`] for the argument). `TaskInput`
    /// carries no lease since the final review (I2), so a lease-keyed name is
    /// not even expressible.
    ///
    /// Two holders of one attempt compute the same name, which is what lets
    /// the second find the first's container; a retry (the next attempt) gets
    /// a different one; and the name is the pod name, so the two runners
    /// address their work identically.
    #[test]
    fn the_container_name_is_the_attempt_and_not_the_lease() {
        let a = leased(1_700_000_000);
        let b = leased(1_700_000_001);
        assert_eq!(a.run_id, b.run_id, "one run, one attempt, two holders");
        assert_eq!(
            container_name(&a),
            container_name(&b),
            "a new holder of the same attempt must find the old holder's container"
        );
        assert_eq!(
            container_name(&a),
            "egret-run-00000000000000000000000000000007-1",
            "the format is pinned: a change to it is a change to what `cancel` and \
             re-attach address, and every running container's name at once"
        );
        let retry = TaskInput {
            attempt: 2,
            ..a.clone()
        };
        assert_ne!(container_name(&a), container_name(&retry));
        assert_eq!(
            container_name(&a),
            crate::worker::k8s::pod_name(a.run_id, 1),
            "the Docker and Kubernetes runners name an attempt's work the same way"
        );
    }

    /// Docker's own rule for a container name:
    /// `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
    #[test]
    fn every_name_this_produces_is_a_legal_container_name() {
        for attempt in [0_usize, 1, 7, usize::MAX] {
            let name = container_name(&TaskInput {
                attempt,
                ..leased(1_700_000_000)
            });
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

    /// **`docker create` carries the `taskRunner:` properties the Docker runner
    /// implements** -- `pullPolicy`, `cpu.cpus`, `memory.memory`, `delete` --
    /// and a default spec carries only `--pull missing`.
    #[test]
    fn create_args_carry_the_docker_runner_properties() {
        let task = leased(1_700_000_000);
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["echo hi".into()],
            ..TaskCommands::default()
        };
        let pair = |args: &[String], flag: &str| -> Option<String> {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };

        let d = create_args(
            &task,
            &cmds,
            &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
            "n",
        );
        assert_eq!(pair(&d, "--pull").as_deref(), Some("missing"));
        assert_eq!(pair(&d, "--cpus"), None);
        assert_eq!(pair(&d, "--memory"), None);
        assert!(!d.iter().any(|a| a.starts_with(DELETE_LABEL)), "{d:?}");
        // The image is followed by the CMD tail, and nothing after it is a flag.
        let at = d
            .iter()
            .position(|a| a == "alpine:3.20")
            .expect("the image");
        assert_eq!(&d[at + 1..], ["-c".to_string(), "echo hi".to_string()]);
        assert_eq!(d[at - 1], "--", "flags end before the image: {d:?}");

        for (policy, want) in [
            (crate::scripts::PullPolicy::Always, "always"),
            (crate::scripts::PullPolicy::IfNotPresent, "missing"),
            (crate::scripts::PullPolicy::Never, "never"),
        ] {
            let spec = RunnerSpec {
                pull_policy: policy,
                cpu: Some("1.5".into()),
                memory: Some("256m".into()),
                delete: false,
                ..RunnerSpec::defaults(crate::scripts::RunnerKind::Docker)
            };
            let a = create_args(&task, &cmds, &spec, "n");
            assert_eq!(pair(&a, "--pull").as_deref(), Some(want));
            assert_eq!(pair(&a, "--cpus").as_deref(), Some("1.5"));
            assert_eq!(pair(&a, "--memory").as_deref(), Some("256m"));
            assert!(
                a.contains(&format!("{DELETE_LABEL}=false")),
                "`delete: false` must be visible to the orphan sweep: {a:?}"
            );
        }
    }

    /// **An image spelled like a flag stays the IMAGE** (final review I1): the
    /// only `--` in the argv ends the flags immediately before it, so
    /// `--privileged` and the author's interpreter tail (`-v /:/host …`) are
    /// the positional image and CMD, never `docker create` options. The gate
    /// refuses such an image first; this pins the runner's own defence.
    #[test]
    fn a_flag_shaped_image_is_never_a_docker_create_flag() {
        let task = leased(1_700_000_000);
        let cmds = TaskCommands {
            container_image: "--privileged".into(),
            interpreter: vec![
                "/bin/sh".into(),
                "-v".into(),
                "/:/host".into(),
                "alpine:3.20".into(),
                "sh".into(),
                "-c".into(),
            ],
            commands: vec!["id".into()],
            ..TaskCommands::default()
        };
        let a = create_args(
            &task,
            &cmds,
            &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
            "n",
        );
        let ends: Vec<usize> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| x.as_str() == "--")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(ends.len(), 1, "exactly one end-of-flags marker: {a:?}");
        let end = ends[0];
        assert_eq!(
            a[end + 1],
            "--privileged",
            "the image is the first positional"
        );
        assert!(
            !a[..end].iter().any(|x| x == "--privileged" || x == "-v"),
            "nothing the author wrote is before `--`: {a:?}"
        );
    }

    /// The sweep's parser keeps only egret task containers and drops the ones
    /// the author asked to keep.
    #[test]
    fn the_docker_sweep_parses_names_and_leaves_delete_false_alone() {
        let a = uuid::Uuid::from_u128(1);
        let b = uuid::Uuid::from_u128(2);
        let out = format!(
            "egret-run-x-1\t{a}\t\negret-run-y-1\t{b}\tfalse\nsomething-else\t{a}\t\negret-run-z-1\tnot-a-uuid\t\n"
        );
        assert_eq!(
            parse_docker_ps(&out),
            vec![("egret-run-x-1".to_string(), a)]
        );
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
            // Broken JSON carries no outputs (it is not STORED either --
            // `ingest_line` turns it into one WARN; see its tests).
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

    /// Internal storage over a `MemStore`, default caps (W9a Task 6).
    fn mem_files() -> std::sync::Arc<crate::storage::FileStore> {
        std::sync::Arc::new(crate::storage::FileStore {
            store: std::sync::Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: crate::storage::Caps::default(),
            retention_days: 30,
        })
    }

    /// Where `task`'s collected files go.
    fn collect_to(task: &TaskInput) -> CollectTo {
        CollectTo {
            workspace: task.workspace_id,
            task_prefix: crate::storage::uri::task_prefix(
                "t",
                "f",
                task.execution_id,
                &task.task_id,
                task.run_id,
            )
            .unwrap(),
            naming: Naming::KestraOutputName,
        }
    }

    /// What `docker cp c:<file> -` writes for one regular file.
    fn file_tar(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut out = crate::storage::transfer::header(
            name,
            contents.len() as u64,
            b'0',
            0o644,
            1_700_000_000,
        )
        .unwrap()
        .to_vec();
        out.extend_from_slice(contents);
        out.resize(out.len().next_multiple_of(512) + 1024, 0);
        out
    }

    /// What `docker cp c:<dir> -` writes for a directory with one file.
    fn dir_tar(name: &str) -> Vec<u8> {
        let mut out = crate::storage::transfer::header(&format!("{name}/"), 0, b'5', 0o755, 0)
            .unwrap()
            .to_vec();
        out.extend_from_slice(&file_tar(&format!("{name}/inner"), b"x"));
        out
    }

    /// The bytes stored for `rel`'s URI.
    async fn stored_bytes(
        fs: &crate::storage::FileStore,
        task: &TaskInput,
        r: &CommandsResult,
        rel: &str,
    ) -> Vec<u8> {
        fs.store
            .get(
                &fs.object_uri(task.workspace_id, &r.output_files[rel].uri)
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// `ingest_line` over a fresh state, one line.
    fn one(
        r: &mut CommandsResult,
        st: &mut StreamState,
        max: usize,
        line: &str,
        is_err: bool,
    ) -> Option<(LogLevel, String)> {
        ingest_line(r, st, max, line, is_err, false).map(|(l, t)| (l, t.into_owned()))
    }

    /// Every stdout line of `lines` through [`ingest_line`] with
    /// `host_protocol`, over one state: what is stored, in order.
    fn stored(
        r: &mut CommandsResult,
        host_protocol: bool,
        lines: &[&str],
    ) -> Vec<(LogLevel, String)> {
        let mut st = StreamState::default();
        lines
            .iter()
            .filter_map(|l| {
                ingest_line(r, &mut st, 1 << 20, l, false, host_protocol)
                    .map(|(l, t)| (l, t.into_owned()))
            })
            .collect()
    }

    /// **`Debug` never prints an input file's contents** (W5b fix round 1,
    /// M5): the plugin handler's `task.json` holds resolved secrets, and a
    /// `{:?}` in a `tracing` event or a panic message would write them out.
    #[test]
    fn debug_names_the_input_files_but_never_prints_their_contents() {
        let cmds = TaskCommands {
            container_image: "img".into(),
            input_files: BTreeMap::from([(
                "task.json".to_string(),
                r#"{"properties":{"password":"pa55-s3cret"}}"#.to_string(),
            )]),
            ..TaskCommands::default()
        };
        let debug = format!("{cmds:?} {cmds:#?}");
        assert!(!debug.contains("pa55-s3cret"), "{debug}");
        assert!(debug.contains("task.json"), "{debug}");
        assert!(debug.contains("img"), "{debug}");
    }

    /// **A re-attach drops the lines the previous holder stored -- compared
    /// as stored** (W5b fix round 1, M1). Under `host_protocol` the stored
    /// text is the message without its level prefix, masked; the raw line
    /// re-read from the container is neither. Comparing the raw line with the
    /// stored text never matched, so every overlap line was stored twice on
    /// each re-attach of a plugin pod.
    #[tokio::test]
    async fn a_reattach_drops_an_overlap_line_compared_as_it_was_stored() {
        let (mut task, rec) = crate::worker::handlers::testing::recorded(
            "kestra.io.kestra.plugin.jdbc.postgresql.Query",
            serde_json::json!({}),
        );
        task.log = task.log.masked(&["pa55".to_string()]);
        // What the previous holder stored for its last two lines.
        let mut replayed = crate::worker::k8s::runner::Replayed::new(vec![
            "connecting with ******".to_string(),
            "\tat Foo.bar".to_string(),
        ]);
        let mut child = tokio::process::Command::new("/bin/sh")
            .args([
                "-c",
                "printf 'INFO connecting with pa55\\n\\tat Foo.bar\\nINFO new line\\n'",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh");
        DockerRunner::new()
            .pump(&task, &NoObserver, &mut child, Some(&mut replayed), true)
            .await
            .expect("pumped");
        child.wait().await.expect("exited");
        assert_eq!(
            rec.texts(),
            vec![(LogLevel::Info, "new line".to_string())],
            "only the line the previous holder did not store is stored"
        );
    }

    /// **The host's `%level %msg%n`, parsed** (W5b Task 5): the prefix is the
    /// level and is stripped, and a line with none -- a stack trace's
    /// continuation -- keeps the level of the line before it.
    #[test]
    fn host_protocol_levels_stdout_and_a_continuation_keeps_the_previous_level() {
        let mut r = CommandsResult::default();
        let got = stored(
            &mut r,
            true,
            &[
                "WARN a",
                "\tat x.y(Z.java:1)",
                "INFO b",
                "ERROR boom: java.lang.IllegalStateException: no",
                "Caused by: java.io.IOException: gone",
                "\t... 3 more",
                "DEBUG d",
                "TRACE t",
                "INFO  two spaces keep the second",
            ],
        );
        assert_eq!(
            got,
            vec![
                (LogLevel::Warn, "a".to_string()),
                (LogLevel::Warn, "\tat x.y(Z.java:1)".to_string()),
                (LogLevel::Info, "b".to_string()),
                (
                    LogLevel::Error,
                    "boom: java.lang.IllegalStateException: no".to_string()
                ),
                (
                    LogLevel::Error,
                    "Caused by: java.io.IOException: gone".to_string()
                ),
                (LogLevel::Error, "\t... 3 more".to_string()),
                (LogLevel::Debug, "d".to_string()),
                (LogLevel::Trace, "t".to_string()),
                (LogLevel::Info, " two spaces keep the second".to_string()),
            ]
        );
        assert_eq!(r.stdout_line_count, 9, "every line is still counted");

        // No line before it: INFO -- and DEBUG inside a bracket, whose rule
        // is unchanged.
        let mut r = CommandsResult::default();
        assert_eq!(
            stored(&mut r, true, &["\tat first"]),
            vec![(LogLevel::Info, "\tat first".to_string())]
        );
        let mut r = CommandsResult::default();
        assert_eq!(
            stored(&mut r, true, &[LOG_DEBUG_MARKER, "bare", "WARN w", "bare2"]),
            vec![
                (LogLevel::Debug, "bare".to_string()),
                (LogLevel::Warn, "w".to_string()),
                (LogLevel::Warn, "bare2".to_string()),
            ]
        );

        // Only the five names, case-sensitively, and only with the space.
        let mut r = CommandsResult::default();
        assert_eq!(
            stored(
                &mut r,
                true,
                &["ERROR x", "warn y", "WARNING z", "WARN", "FATAL f"]
            ),
            vec![
                (LogLevel::Error, "x".to_string()),
                (LogLevel::Error, "warn y".to_string()),
                (LogLevel::Error, "WARNING z".to_string()),
                (LogLevel::Error, "WARN".to_string()),
                (LogLevel::Error, "FATAL f".to_string()),
            ]
        );

        // stderr stays ERROR, and does not become the continuation's level.
        let mut r = CommandsResult::default();
        let mut st = StreamState::default();
        let mut go = |line: &str, is_err: bool| {
            ingest_line(&mut r, &mut st, 1 << 20, line, is_err, true)
                .map(|(l, t)| (l, t.into_owned()))
        };
        assert_eq!(go("DEBUG q", false), Some((LogLevel::Debug, "q".into())));
        assert_eq!(
            go("INFO on stderr", true),
            Some((LogLevel::Error, "INFO on stderr".into()))
        );
        assert_eq!(go("cont", false), Some((LogLevel::Debug, "cont".into())));
    }

    /// **Without the protocol a level prefix is ordinary text** -- a script's
    /// `echo "WARN disk"` is W11's INFO line, text unchanged.
    #[test]
    fn without_host_protocol_a_level_prefix_is_ordinary_text() {
        let mut r = CommandsResult::default();
        assert_eq!(
            stored(&mut r, false, &["WARN a", "\tat x", "ERROR e"]),
            vec![
                (LogLevel::Info, "WARN a".to_string()),
                (LogLevel::Info, "\tat x".to_string()),
                (LogLevel::Info, "ERROR e".to_string()),
            ]
        );
    }

    /// **`egretFailure` is the host's, so only the host protocol reads it**:
    /// for a script it is ignored like any non-`outputs` key, and the marker
    /// line is not stored either way.
    #[test]
    fn the_egret_failure_marker_is_harvested_only_under_host_protocol() {
        let line =
            r#"::{"egretFailure": {"code": 3, "message": "java.lang.RuntimeException: é boom"}}::"#;
        for host in [false, true] {
            let mut r = CommandsResult::default();
            assert_eq!(stored(&mut r, host, &[line]), vec![], "host={host}");
            assert!(r.vars.is_empty(), "host={host}: {r:?}");
            assert_eq!(r.failure, None, "host={host}");
            let want = host.then(|| HostFailure {
                code: 3,
                message: "java.lang.RuntimeException: é boom".to_string(),
            });
            assert_eq!(r.host_failure, want, "host={host}");
        }

        // The success marker still fills the vars under the protocol, and a
        // later failure marker (the SIGTERM path) replaces an earlier one.
        let mut r = CommandsResult::default();
        stored(
            &mut r,
            true,
            &[
                r#"::{"outputs": {"size": 2}}::"#,
                r#"::{"egretFailure": {"code": 2, "message": "first"}}::"#,
                r#"::{"egretFailure": {"code": 3, "message": "second"}}::"#,
            ],
        );
        assert_eq!(r.vars.get("size"), Some(&serde_json::json!(2)));
        assert_eq!(
            r.host_failure,
            Some(HostFailure {
                code: 3,
                message: "second".into()
            })
        );
    }

    /// **The message is capped at 4 KiB, cut at a char boundary** -- it
    /// becomes the FAILED reason, and a multi-byte char straddling the cap
    /// must not panic or be split.
    #[test]
    fn an_egret_failure_message_over_4_kib_is_truncated_on_a_char_boundary() {
        assert_eq!(MAX_HOST_FAILURE_BYTES, 4096);
        // 4095 ASCII bytes, then a 2-byte `é` that would end at byte 4097.
        let message = format!("{}é{}", "a".repeat(4095), "b".repeat(100));
        let line = format!(
            "::{}::",
            serde_json::json!({"egretFailure": {"code": 3, "message": message}})
        );
        let mut r = CommandsResult::default();
        stored(&mut r, true, &[line.as_str()]);
        let got = r.host_failure.expect("harvested").message;
        assert_eq!(got.len(), 4095, "the straddling char is dropped whole");
        assert_eq!(got, "a".repeat(4095));

        // Exactly at the cap is kept whole; one byte under too.
        for n in [4096usize, 4094] {
            let m = "é".repeat(n / 2);
            let line = format!(
                "::{}::",
                serde_json::json!({"egretFailure": {"code": 4, "message": m}})
            );
            let mut r = CommandsResult::default();
            stored(&mut r, true, &[line.as_str()]);
            assert_eq!(r.host_failure.expect("harvested").message, m, "{n}");
        }

        // A huge message stays linear: 8 MiB of 3-byte chars truncates at once.
        let m = "€".repeat(8 << 20 >> 2);
        let line = format!(
            "::{}::",
            serde_json::json!({"egretFailure": {"code": 3, "message": m}})
        );
        let mut r = CommandsResult::default();
        stored(&mut r, true, &[line.as_str()]);
        let got = r.host_failure.expect("harvested").message;
        assert_eq!(got.len(), 4095, "1365 whole `€`s");
        assert!(got.chars().all(|c| c == '€'));
    }

    /// **Both streams are counted, and the level says which one it was** —
    /// Task 1's `stdout_level` and `stderr_level`, typed.
    ///
    /// This is one of the four properties fix round 1 moved out of
    /// `run_created` so a machine with no daemon can judge them. Before the
    /// split, the only test that could see the level was a live one.
    #[test]
    fn a_line_is_counted_on_its_own_stream_and_logged_at_its_own_level() {
        let out = fixture::level(&fixture::expected("stdout_level")["stdout_level"]);
        let err = fixture::level(&fixture::expected("stderr_level")["stderr_level"]);
        assert_eq!((out, err), (LogLevel::Info, LogLevel::Error));
        let table = &fixture::w11()["level_table"];
        assert_eq!(fixture::level(&table["stdout"]), out);
        assert_eq!(fixture::level(&table["stderr"]), err);

        let mut r = CommandsResult::default();
        let mut st = StreamState::default();
        assert_eq!(
            one(&mut r, &mut st, 1024, "out", false),
            Some((out, "out".into()))
        );
        assert_eq!(
            one(&mut r, &mut st, 1024, "err", true),
            Some((err, "err".into())),
            "stderr is ERROR (measured, whatever warningOnStdErr says) and stdout INFO"
        );
        assert_eq!((r.stdout_line_count, r.stderr_line_count), (1, 1));
        assert!(r.vars.is_empty() && r.failure.is_none());
    }

    /// Task 1's marker `cases` (both `outputs_marker_consumed` and
    /// `other_marker_shapes`), typed: a whole-line `::{json}::` is not stored
    /// on either stream and adds its `outputs`; a line merely containing one
    /// is stored at its stream's level and adds nothing.
    #[test]
    fn a_whole_line_marker_is_harvested_and_not_stored() {
        let mut cases = Vec::new();
        for id in ["outputs_marker_consumed", "other_marker_shapes"] {
            cases.extend(fixture::expected(id)["cases"].as_array().unwrap().clone());
        }
        assert_eq!(cases.len(), 6);
        for case in &cases {
            let line = case["line"].as_str().unwrap();
            let is_err = match case["stream"].as_str().unwrap() {
                "stdout" => false,
                "stderr" => true,
                s => panic!("stream {s}"),
            };
            let mut r = CommandsResult::default();
            let mut st = StreamState::default();
            let got = one(&mut r, &mut st, 1 << 20, line, is_err);
            let invalid = matches!(classify_marker(line), Marker::Invalid(_));
            if case["stored"] == true {
                assert_eq!(
                    got,
                    Some((fixture::level(&case["level"]), line.to_string())),
                    "{case}"
                );
            } else if invalid {
                // Not stored as itself; the WARN is asserted below.
                assert_ne!(got.as_ref().map(|g| g.1.as_str()), Some(line), "{case}");
            } else {
                assert_eq!(got, None, "{case}");
            }
            let want = case["outputs"].as_object().unwrap();
            let have: serde_json::Map<String, serde_json::Value> =
                r.vars.clone().into_iter().collect();
            assert_eq!(&have, want, "{case}");
            assert_eq!(
                (r.stdout_line_count, r.stderr_line_count),
                if is_err { (0, 1) } else { (1, 0) },
                "a marker line is still counted on its stream: {case}"
            );
        }
    }

    /// `invalid_marker_warning`, typed: one WARN, `Invalid outputs '<serde
    /// error>'`, no stack line.
    #[test]
    fn an_invalid_marker_is_one_warn_naming_the_error() {
        let w = &fixture::expected("other_marker_shapes")["invalid_marker_warning"];
        assert_eq!(w["count"], 1);
        assert_eq!(w["stack_trace_line_count"], 0);
        let prefix = w["message_prefix"].as_str().unwrap();
        let suffix = w["message_suffix"].as_str().unwrap();
        let mut r = CommandsResult::default();
        let mut st = StreamState::default();
        let (level, text) = one(&mut r, &mut st, 1024, "::{not json}::", false).expect("a WARN");
        assert_eq!(level, fixture::level(&w["level"]));
        assert_eq!(
            fixture::level(&fixture::w11()["level_table"]["invalid_marker_warning"]),
            level
        );
        let error = serde_json::from_str::<serde_json::Value>("{not json}")
            .unwrap_err()
            .to_string();
        assert_eq!(text, format!("{prefix}{error}{suffix}"));
        assert!(r.vars.is_empty());
    }

    /// The debug bracket's `sequence`, typed: stored lines and levels in input
    /// order, marker lines not stored, stderr ERROR inside the bracket; and a
    /// marker on stderr toggles it too (`toggled_by_stderr_marker`).
    #[test]
    fn the_debug_bracket_follows_task_1s_sequence() {
        let e = fixture::expected("debug_bracket");
        assert_eq!(e["bracket_marker_lines_stored"], false);
        let seq = &e["sequence"];
        let mut r = CommandsResult::default();
        let mut st = StreamState::default();
        let mut stored = Vec::new();
        for pair in seq["input"].as_array().unwrap() {
            let is_err = pair[0] == "stderr";
            if let Some(x) = one(&mut r, &mut st, 1024, pair[1].as_str().unwrap(), is_err) {
                stored.push(x);
            }
        }
        let want: Vec<(LogLevel, String)> = seq["stored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (fixture::level(&p[0]), p[1].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(stored, want);
        let table = &fixture::w11()["level_table"];
        assert_eq!(
            fixture::level(&table["stdout_inside_debug_bracket"]),
            LogLevel::Debug
        );
        assert_eq!(
            fixture::level(&table["stderr_inside_debug_bracket"]),
            LogLevel::Error
        );

        assert_eq!(e["toggled_by_stderr_marker"], true);
        let mut st = StreamState::default();
        assert_eq!(one(&mut r, &mut st, 1024, LOG_DEBUG_MARKER, true), None);
        assert_eq!(
            one(&mut r, &mut st, 1024, "x", false),
            Some((LogLevel::Debug, "x".into()))
        );
        assert_eq!(one(&mut r, &mut st, 1024, LOG_INFO_MARKER, true), None);
        assert_eq!(
            one(&mut r, &mut st, 1024, "y", false),
            Some((LogLevel::Info, "y".into()))
        );
    }

    /// **The vars cap sets a failure, clears what it had, and stops
    /// collecting** — it does not abandon the stream.
    #[test]
    fn the_vars_cap_fails_the_task_and_keeps_reading() {
        let mut r = CommandsResult::default();
        let mut st = StreamState::default();
        let big = "y".repeat(200);
        one(
            &mut r,
            &mut st,
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
            one(&mut r, &mut st, 32, "later", true),
            Some((LogLevel::Error, "later".into()))
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
        one(
            &mut r,
            &mut st,
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
            "it is still counted -- only its payload is dropped"
        );
    }

    /// **`outputFiles` follows the artifact, case for case** (W5a Task 5 fix
    /// round 1, the user's decision): every `egret_expected.output_files` case
    /// of `w5a-scripts.json` -- a literal file, a non-recursive `*.csv` that
    /// keys `a.csv` and `b.csv` but not `sub/c.csv` or `x.txt`, and a declared
    /// file the command never wrote, which is IGNORED (no key, no failure).
    /// Driven through the two pure seams both runners use: `expand_outputs`
    /// over the working directory's listing, then `ingest_output` per name.
    #[tokio::test]
    async fn output_files_follow_the_fixtures_cases() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w5a-scripts.json");
        let fx: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let spec = &fx["egret_expected"]["output_files"];
        assert_eq!(spec["missing_declared_file"], "ignored");
        assert_eq!(spec["glob_descends_into_subdirectories"], false);
        let strings = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        let cases = spec["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 3);
        let fs = mem_files();
        let task = leased(1_700_000_000);
        let to = collect_to(&task);
        for case in cases {
            let declared = strings(&case["declared"]);
            let written = strings(&case["written"]);
            // The container's working directory: each written file, and the
            // directories they imply.
            let is_dir = |rel: &str| written.iter().any(|w| w.starts_with(&format!("{rel}/")));
            // What a listing of each pattern's directory returns: its direct
            // entries (a subdirectory included, which a glob then skips).
            let mut listed = std::collections::BTreeMap::new();
            for d in pattern_dirs(&declared) {
                let under = if d.is_empty() {
                    String::new()
                } else {
                    format!("{d}/")
                };
                let names: std::collections::BTreeSet<String> = written
                    .iter()
                    .filter_map(|w| w.strip_prefix(&under))
                    .map(|rest| rest.split('/').next().unwrap().to_string())
                    .collect();
                listed.insert(d, names.into_iter().collect::<Vec<_>>());
            }
            let mut r = CommandsResult::default();
            let mut so_far = 0;
            for (rel, from_glob) in expand_outputs(&declared, &listed, MAX_OUTPUT_NAMES).unwrap() {
                // The archive `docker cp c:<wd>/<rel> -` would write.
                let archive = if written.contains(&rel) {
                    file_tar(
                        rel.rsplit('/').next().unwrap(),
                        format!("contents of {rel}").as_bytes(),
                    )
                } else if is_dir(&rel) {
                    dir_tar(&rel)
                } else {
                    Vec::new()
                };
                let mut src = &archive[..];
                let mut reader = crate::storage::transfer::TarReader::new(&mut src);
                if let Some(f) = crate::storage::collect::collect_declared(
                    &fs,
                    &to,
                    &rel,
                    from_glob,
                    &mut reader,
                    &mut so_far,
                )
                .await
                .unwrap_or_else(|e| panic!("{case}: {rel}: {e}"))
                {
                    r.output_files.insert(rel.clone(), f);
                }
            }
            let keys: Vec<String> = r.output_files.keys().cloned().collect();
            assert_eq!(keys, strings(&case["keys"]), "{case}");
            assert_eq!(case["state"], "SUCCESS");
            for k in &keys {
                assert_eq!(
                    stored_bytes(&fs, &task, &r, k).await,
                    format!("contents of {k}").into_bytes()
                );
            }
        }
    }

    /// **Fix round 2: an author-sized listing cannot wedge the worker.** An
    /// 8 MB `docker diff` (a shim; 8 MB rather than endless so the unmutated
    /// control's memory stays bounded) is cut at a 64 KiB listing cap and the task FAILS by name, promptly -- rather than the
    /// handler reading, holding and scanning it while its lease renewal,
    /// budget and self-fence cannot run.
    #[tokio::test]
    async fn a_listing_over_the_cap_fails_the_task_in_bounded_time() {
        let dir = staging("endless-diff");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            "#!/bin/sh\n\
             case \"$1\" in\n\
             \x20 create) echo id ;;\n\
             \x20 version) exit 0 ;;\n\
             \x20 inspect) case \"$*\" in *State.Status*) exit 1 ;; esac; echo 0 ;;\n\
             \x20 diff) yes 'A /egret/working-dir/f.x' | head -c 8000000 ;;\n\
             esac\nexit 0\n",
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string())
            .with_listing_cap(64 * 1024)
            .with_files(mem_files());
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["true".into()],
            output_files: vec!["*.x".into()],
            collect_to: Some(collect_to(&leased(1_700_000_000))),
            ..TaskCommands::default()
        };
        let started = std::time::Instant::now();
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            runner.run(
                &leased(1_700_000_000),
                &cmds,
                &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
                &NoObserver,
            ),
        )
        .await
        .expect("an endless listing must be cut at the cap, not read to its end")
        .expect("an author's listing is never a worker failure");
        let why = out.failure.expect("over the cap fails the task");
        assert!(why.contains("listing") && why.contains("65536"), "{why}");
        assert!(out.output_files.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The listing is parsed in LINEAR time: 200 000 paths well inside a
    /// second-scale bound (the first version was quadratic).
    #[test]
    fn a_large_listing_parses_in_linear_time() {
        let mut diff = String::from("A /egret/working-dir\nA /egret/working-dir/sub\n");
        for i in 0..200_000 {
            diff.push_str(&format!("A /egret/working-dir/sub/f{i}.x\n"));
        }
        let dirs = std::collections::BTreeSet::from(["sub".to_string()]);
        let started = std::time::Instant::now();
        let listed = parse_docker_diff(&diff, &dirs);
        assert_eq!(listed["sub"].len(), 200_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    /// Too many names -- one pattern's matches, or all names together -- fail
    /// the task by name.
    #[test]
    fn too_many_output_names_fail_by_name() {
        let listed = std::collections::BTreeMap::from([(
            String::new(),
            (0..11).map(|i| format!("f{i}.x")).collect::<Vec<_>>(),
        )]);
        let err = expand_outputs(&["*.x".to_string()], &listed, 10).unwrap_err();
        assert!(err.contains("\"*.x\" matches more than 10"), "{err}");
        let ok = expand_outputs(&["*.x".to_string()], &listed, 11).expect("at the cap");
        assert_eq!(ok.len(), 11);
        let declared: Vec<String> = (0..6).map(|i| format!("lit{i}")).collect();
        let small = std::collections::BTreeMap::from([(
            String::new(),
            (0..6).map(|i| format!("g{i}.x")).collect::<Vec<_>>(),
        )]);
        let mut all = declared.clone();
        all.push("*.x".into());
        let err = expand_outputs(&all, &small, 10).unwrap_err();
        assert!(err.contains("resolves to more than 10"), "{err}");
    }

    /// A `docker` shim whose `diff` prints `diff_script`'s output and whose
    /// `cp … -` writes an archive of one file holding the word `staged`.
    fn diff_shim(tag: &str, diff_script: &str) -> (std::path::PathBuf, String) {
        let dir = staging(tag);
        let tar = dir.join("staged.tar");
        std::fs::write(&tar, file_tar("a.csv", b"staged")).expect("write tar");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                 \x20 create) echo id ;;\n\
                 \x20 version) exit 0 ;;\n\
                 \x20 inspect) case \"$*\" in *State.Status*) exit 1 ;; esac; echo 0 ;;\n\
                 \x20 diff) {diff_script} ;;\n\
                 \x20 cp) cat {tar} ;;\n\
                 esac\nexit 0\n",
                tar = tar.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");
        (dir, shim.display().to_string())
    }

    async fn run_glob(runner: &DockerRunner) -> CommandsResult {
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["true".into()],
            output_files: vec!["*.csv".into()],
            collect_to: Some(collect_to(&leased(1_700_000_000))),
            ..TaskCommands::default()
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            runner.run(
                &leased(1_700_000_000),
                &cmds,
                &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
                &NoObserver,
            ),
        )
        .await
        .expect("bounded")
        .expect("never a worker failure")
    }

    /// **Fix round 3: only the working directory counts toward the listing
    /// cap.** A container that changed ~400 KB of paths OUTSIDE it (a package
    /// install) under a 64 KiB listing cap still lists its one output.
    #[tokio::test]
    async fn a_diff_mostly_outside_the_working_dir_still_lists_the_outputs() {
        let (dir, shim) = diff_shim(
            "outside-diff",
            "yes 'A /usr/lib/python3/site-packages/torch/x.py' | head -n 10000; \
             echo 'A /egret/working-dir'; echo 'A /egret/working-dir/a.csv'",
        );
        let fs = mem_files();
        let runner = DockerRunner::with_binary(shim)
            .with_listing_cap(64 * 1024)
            .with_files(fs.clone());
        let out = run_glob(&runner).await;
        assert_eq!(out.failure, None, "{out:?}");
        assert_eq!(out.output_files.keys().collect::<Vec<_>>(), ["a.csv"]);
        assert_eq!(
            stored_bytes(&fs, &leased(1_700_000_000), &out, "a.csv").await,
            b"staged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole diff still has a hard DoS cap, with its own honest message.
    #[tokio::test]
    async fn a_diff_over_the_hard_cap_fails_saying_the_container_changed_too_much() {
        let (dir, shim) = diff_shim(
            "huge-diff",
            "yes 'A /usr/lib/python3/site-packages/torch/x.py' | head -n 10000",
        );
        let runner = DockerRunner::with_binary(shim)
            .with_diff_cap(100_000)
            .with_files(mem_files());
        let out = run_glob(&runner).await;
        let why = out.failure.expect("over the hard cap fails the task");
        assert!(
            why.contains("the container changed more than 100000 bytes"),
            "{why}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The glob worst case at the caps is bounded by the step budget.**
    /// 64 adversarial 255-byte patterns (`*aaa…ab`, the backtracking worst
    /// case) against a full 1 MiB-class listing of `aaa…a` names: the budget
    /// is spent and the task fails by name, in bounded time, rather than the
    /// ~1e10 steps the caps alone allow. The control -- 64 ordinary patterns
    /// against the same listing -- resolves within the budget.
    #[test]
    fn the_glob_worst_case_is_bounded_by_the_step_budget() {
        let names: Vec<String> = (0..47_000)
            .map(|i| format!("{}{i}", "a".repeat(15)))
            .collect();
        let listed = BTreeMap::from([(String::new(), names)]);
        let adversarial: Vec<String> = (0..MAX_OUTPUT_PATTERNS)
            .map(|i| format!("*{}{i}b", "a".repeat(MAX_PATTERN_BYTES - 4)))
            .collect();
        let started = std::time::Instant::now();
        let err = expand_outputs(&adversarial, &listed, MAX_OUTPUT_NAMES).unwrap_err();
        let spent = started.elapsed();
        assert!(err.contains("steps"), "{err}");
        assert!(spent < std::time::Duration::from_secs(30), "{spent:?}");
        eprintln!("glob worst case (budget spent): {spent:?}");

        let ordinary: Vec<String> = (0..MAX_OUTPUT_PATTERNS)
            .map(|i| format!("*.csv{i}"))
            .collect();
        let started = std::time::Instant::now();
        let ok = expand_outputs(&ordinary, &listed, MAX_OUTPUT_NAMES).expect("within budget");
        assert!(ok.is_empty());
        eprintln!("64 ordinary patterns x 47k names: {:?}", started.elapsed());
    }

    /// More than [`MAX_OUTPUT_PATTERNS`] distinct patterns is refused;
    /// duplicates count once.
    #[test]
    fn the_pattern_count_is_capped_and_duplicates_count_once() {
        let listed = BTreeMap::new();
        let many: Vec<String> = (0..=MAX_OUTPUT_PATTERNS)
            .map(|i| format!("*.{i}"))
            .collect();
        let err = expand_outputs(&many, &listed, MAX_OUTPUT_NAMES).unwrap_err();
        assert!(err.contains("distinct patterns"), "{err}");
        let dupes: Vec<String> = (0..500).map(|_| "*.csv".to_string()).collect();
        expand_outputs(&dupes, &listed, MAX_OUTPUT_NAMES).expect("one distinct pattern");
        assert!(output_problem(&format!("*{}", "a".repeat(MAX_PATTERN_BYTES))).is_some());
        assert!(output_problem(&format!("*{}", "a".repeat(MAX_PATTERN_BYTES - 1))).is_none());
    }

    /// The pattern rules: `*`/`?` in the file name only; `**`, `[...]` and a
    /// wildcard directory are refused by name; the literal rules still hold.
    /// W9a Task 5 fix round 1, M1: a NUL (or any control character) in a
    /// name is refused -- a C-string reader stops at the NUL, so
    /// `x\0/../../etc/passwd` would not be the name egret checked.
    #[test]
    fn a_control_character_in_a_file_name_is_refused() {
        for bad in [
            "a\0b",
            "x\0/../../etc/passwd",
            "a\nb.txt",
            "tab\there",
            "del\u{7f}",
        ] {
            let problem =
                relative_problem(bad).unwrap_or_else(|| panic!("{bad:?} must be refused"));
            assert!(problem.contains("control character"), "{problem}");
        }
        assert_eq!(relative_problem("plain name-1.txt"), None);
        assert_eq!(relative_problem("d/caf\u{e9}.csv"), None);
    }

    #[test]
    fn an_output_pattern_is_a_file_name_glob_and_nothing_wider() {
        assert_eq!(output_problem("*.csv"), None);
        assert_eq!(output_problem("sub/data-?.csv"), None);
        assert_eq!(output_problem("out.txt"), None);
        for bad in ["**/x", "a/**", "[ab].csv", "*/x.csv", "../*.csv", "/tmp/*"] {
            assert!(output_problem(bad).is_some(), "{bad} must be refused");
        }
        // An input name is still literal.
        assert!(relative_problem("*.csv").is_some());
        // `docker diff` parsing: files of the named directories only, no
        // parent directories.
        let diff = "C /egret\nA /egret/working-dir\nA /egret/working-dir/a.csv\n\
                    A /egret/working-dir/sub\nA /egret/working-dir/sub/c.csv\nA /tmp/x\n";
        let dirs = std::collections::BTreeSet::from(["".to_string(), "sub".to_string()]);
        let listed = parse_docker_diff(diff, &dirs);
        assert_eq!(listed[""], vec!["a.csv".to_string()]);
        assert_eq!(listed["sub"], vec!["c.csv".to_string()]);
    }

    /// The name rule reaches the output path too, before any `docker`
    /// argument is built.
    #[tokio::test]
    async fn an_escaping_output_name_is_refused_before_anything_is_copied() {
        for (bad, needle) in [
            ("../secret", "climbs out"),
            ("/etc/passwd", "absolute path"),
            ("**/x.csv", "not implemented"),
            ("[ab].csv", "not implemented"),
            ("*/x.csv", "not implemented"),
            ("", "must not be empty"),
        ] {
            let err = output_problem(bad).expect("an escaping name must be refused");
            assert!(err.contains(needle), "{bad:?} was refused with {err}");
            // Through the runner, over a `docker` that does not exist: an
            // attempt to copy would be a worker failure, not this refusal.
            let runner = DockerRunner::with_binary("/nonexistent-docker").with_files(mem_files());
            let mut r = CommandsResult::default();
            runner
                .collect_output_files(
                    "c",
                    &TaskCommands {
                        output_files: vec![bad.to_string()],
                        collect_to: Some(collect_to(&leased(1))),
                        ..TaskCommands::default()
                    },
                    &mut r,
                )
                .await
                .expect("an author's name is never a worker failure");
            assert!(r.failure.unwrap().contains(needle), "{bad:?}");
        }
    }

    /// **No internal storage, no outputs**: W5a's contents-in-outputs path
    /// is gone, so a declared output fails the task naming `[storage]` --
    /// whether the runner has no store or the commands no target.
    #[tokio::test]
    async fn an_output_without_storage_fails_the_task_naming_the_storage_section() {
        let task = leased(1);
        for (runner, to) in [
            (
                DockerRunner::with_binary("/nonexistent-docker"),
                Some(collect_to(&task)),
            ),
            (
                DockerRunner::with_binary("/nonexistent-docker").with_files(mem_files()),
                None,
            ),
        ] {
            let mut r = CommandsResult::default();
            runner
                .collect_output_files(
                    "c",
                    &TaskCommands {
                        output_files: vec!["a.txt".into()],
                        collect_to: to,
                        ..TaskCommands::default()
                    },
                    &mut r,
                )
                .await
                .expect("the task's failure, not a worker's");
            assert!(r.failure.unwrap().contains("[storage]"));
        }
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
    /// **A `docker stop` that fails still removes the container** (W5b Task
    /// 5 fix round 1): the shim's daemon answers, the container is running,
    /// and `stop` refuses; `cancel` must still reach `rm --force`, in that
    /// order, and report `rm`'s answer.
    #[tokio::test]
    async fn a_failed_stop_still_removes_the_container() {
        let dir = staging("stop-fails");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> {log}\n\
                 case \"$1\" in\n\
                 \x20 stop) echo 'Error response from daemon: boom' >&2; exit 1 ;;\n\
                 \x20 inspect) echo running ;;\n\
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
        runner
            .cancel(&task, std::time::Duration::from_secs(1))
            .await
            .expect("rm --force succeeded, so the cancel did");
        let argv = std::fs::read_to_string(&log).expect("the shim logged");
        let name = container_name(&task);
        let calls: Vec<&str> = argv.lines().collect();
        let stop = calls
            .iter()
            .position(|l| *l == format!("stop -t 1 {name}"))
            .unwrap_or_else(|| panic!("stop was asked: {calls:?}"));
        let rm = calls
            .iter()
            .position(|l| *l == format!("rm --force {name}"))
            .unwrap_or_else(|| {
                panic!("the container was removed after the failed stop: {calls:?}")
            });
        assert!(stop < rm, "{calls:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn only_the_declared_output_files_are_copied_out_of_the_container() {
        let dir = staging("argv");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        // A `docker` that records what it was asked and fakes just enough:
        // `create` prints an id, `start` prints nothing, `inspect` prints 0
        // (and finds no container by name, so the run creates one),
        // and `cp … -` writes an archive of one file to its stdout, as
        // `docker cp c:<file> -` does.
        let tar = dir.join("staged.tar");
        std::fs::write(&tar, file_tar("f", b"staged")).expect("write tar");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> {log}\n\
                 case \"$1\" in\n\
                 \x20 create) echo fake-container-id ;;\n\
                 \x20 inspect) case \"$*\" in *State.Status*) exit 1 ;; esac; echo 0 ;;\n\
                 \x20 cp) case \"$3\" in -) cat {tar} ;; esac ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display(),
                tar = tar.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let runner = DockerRunner::with_binary(shim.display().to_string()).with_files(mem_files());
        let task = leased(1_700_000_000);
        let out = runner
            .run(
                &task,
                &TaskCommands {
                    container_image: "alpine:3.20".into(),
                    commands: vec!["true".into()],
                    output_files: vec!["wanted.txt".into(), "nested/also.txt".into()],
                    collect_to: Some(collect_to(&task)),
                    ..TaskCommands::default()
                },
                &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
                &NoObserver,
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
        let tar = dir.join("a.tar");
        std::fs::write(&tar, file_tar("a", b"regular-file")).expect("write tar");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> {log}\n\
                 case \"$1\" in\n\
                 \x20 create) echo fake-container-id ;;\n\
                 \x20 inspect) case \"$*\" in *State.Status*) exit 1 ;; esac; echo 0 ;;\n\
                 \x20 cp) case \"$2\" in\n\
                 \x20        *a/b.txt) echo 'Error response from daemon: not a directory' >&2; exit 1 ;;\n\
                 \x20        *) cat {tar} ;;\n\
                 \x20      esac ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display(),
                tar = tar.display()
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");

        let fs = mem_files();
        let runner = DockerRunner::with_binary(shim.display().to_string()).with_files(fs.clone());
        let task = leased(1_700_000_000);
        let out = runner
            .run(
                &task,
                &TaskCommands {
                    container_image: "alpine:3.20".into(),
                    commands: vec!["true".into()],
                    output_files: vec!["a".into(), "a/b.txt".into()],
                    collect_to: Some(collect_to(&task)),
                    ..TaskCommands::default()
                },
                &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
                &NoObserver,
            )
            .await
            .expect(
                "an author declaring a parent and a child must never wedge the run -- \
                 a worker failure here applies no outcome, so the run stays RUNNING and \
                 leased for an hour, then repeats for ever",
            );

        // The child cannot exist (its parent is a file), so it is a declared
        // file the command did not leave: IGNORED since W5a Task 5 fix round 1
        // (Kestra's `missing_declared_file`) -- and never a staging
        // collision, because each declared name gets its own root.
        assert_eq!(out.failure, None, "{out:?}");
        assert_eq!(
            stored_bytes(&fs, &task, &out, "a").await,
            b"regular-file",
            "the parent came back"
        );
        assert!(!out.output_files.contains_key("a/b.txt"), "{out:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A dropped future leaves no input file on the worker's disk and no
    /// `docker cp` behind.**
    ///
    /// W5a's form of this test (the whole-branch review's Minor 3) proved the
    /// staging directory holding the author's rendered `script:` was removed
    /// when `timeout::guard` dropped the future mid-copy. Since W9a Task 5 the
    /// input files stream through `docker cp -`'s stdin and are never on the
    /// worker's disk, so the claim is now stronger and the test says it: the
    /// copy's source is `-`, and dropping the future kills the `docker cp`
    /// (`kill_on_drop`), so nothing keeps reading the rendered script.
    #[tokio::test]
    async fn a_dropped_run_future_leaves_no_staging_directory_behind() {
        let dir = staging("drop-leak");
        let log = dir.join("argv.log");
        let shim = dir.join("docker-shim.sh");
        // `cp` records its source and PID and then never returns, so the
        // future is guaranteed to be dropped while the copy is running.
        // **`exec sleep`**: one process, the one `kill_on_drop` reaches (a
        // plain `sleep` would be the shell's child and survive it).
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                 \x20 create) echo id ;;\n\
                 \x20 cp) echo \"$2 $$\" >> {log}; exec sleep 300 ;;\n\
                 \x20 inspect) case \"$*\" in *State.Status*) exit 1 ;; esac; echo 0 ;;\n\
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
        let sink = NoObserver;
        let spec = RunnerSpec::defaults(crate::scripts::RunnerKind::Docker);
        let fut = runner.run(&task, &cmds, &spec, &sink);
        // Exactly what `timeout::guard` does to a handler that outruns its
        // budget: drop the future.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
                .await
                .is_err(),
            "the shim hangs, so the future must still have been running"
        );

        let logged = std::fs::read_to_string(&log).expect("the shim was reached, so it logged");
        let (src, pid) = logged
            .lines()
            .next()
            .expect("one `cp` happened")
            .trim()
            .split_once(' ')
            .expect("source and pid");
        assert_eq!(
            src, "-",
            "the input files go through `docker cp -`'s stdin, never a path on the worker's disk"
        );
        // `kill_on_drop` sends SIGKILL; give the kernel a moment to reap.
        let mut alive = true;
        for _ in 0..50 {
            alive = std::process::Command::new("kill")
                .args(["-0", pid])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            !alive,
            "the dropped future left `docker cp` (pid {pid}) running"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- W9a Task 5 fix round 1: `docker cp -` --------------------------

    /// A `docker` shim whose `cp` runs `cp_body` and whose `version` answers
    /// (the daemon is up, so `classify` makes a failed `cp` the task's).
    fn cp_shim(dir: &std::path::Path, cp_body: &str) -> String {
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                 \x20 version) echo 28.4.0 ;;\n\
                 \x20 cp) {cp_body} ;;\n\
                 esac\n"
            ),
        )
        .expect("write shim");
        let mut perms = std::fs::metadata(&shim).expect("stat").permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        std::fs::set_permissions(&shim, perms).expect("chmod");
        shim.display().to_string()
    }

    /// An object store that answers every read, after `delay`, with an
    /// unreachable-S3 error.
    struct LateDown(std::time::Duration);

    #[async_trait::async_trait]
    impl egret_storage::ObjectStore for LateDown {
        async fn put(
            &self,
            _: &str,
            _: Vec<u8>,
        ) -> std::result::Result<(), egret_storage::StorageError> {
            unreachable!()
        }
        async fn get(&self, _: &str) -> std::result::Result<Vec<u8>, egret_storage::StorageError> {
            unreachable!()
        }
        async fn delete(&self, _: &str) -> std::result::Result<(), egret_storage::StorageError> {
            unreachable!()
        }
        async fn copy(
            &self,
            _: &str,
            _: &str,
        ) -> std::result::Result<(), egret_storage::StorageError> {
            unreachable!()
        }
        async fn list(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, egret_storage::StorageError> {
            unreachable!()
        }
        async fn get_stream(
            &self,
            _: &str,
        ) -> std::result::Result<
            Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
            egret_storage::StorageError,
        > {
            tokio::time::sleep(self.0).await;
            Err(egret_storage::StorageError::S3("connection refused".into()))
        }
        async fn put_stream(
            &self,
            uri: &str,
            body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            _: Option<u64>,
        ) -> std::result::Result<u64, egret_storage::StorageError> {
            let mut first = [0u8; 1];
            let _ = tokio::io::AsyncReadExt::read(body, &mut first).await;
            tokio::time::sleep(self.0).await;
            Err(egret_storage::StorageError::S3(format!(
                "{uri}: connection refused"
            )))
        }
    }

    // ---- W9a Task 6: `docker cp c:<path> -` -----------------------------

    /// **An unreachable store while collecting is a worker error, not a
    /// failed task** (spec §11): the run is re-claimed and no attempt is
    /// spent. The archive arrived whole; only the store failed.
    #[tokio::test]
    async fn an_unreachable_store_while_collecting_is_a_worker_error_not_a_failed_task() {
        let dir = staging("collect-down");
        let tar = dir.join("a.tar");
        std::fs::write(&tar, file_tar("a.txt", b"abc")).unwrap();
        let shim = cp_shim(&dir, &format!("cat {}", tar.display()));
        let task = leased(1);
        let down = std::sync::Arc::new(crate::storage::FileStore {
            store: std::sync::Arc::new(LateDown(std::time::Duration::ZERO)),
            ..std::sync::Arc::into_inner(mem_files()).unwrap()
        });
        let mut r = CommandsResult::default();
        let err = DockerRunner::with_binary(shim)
            .with_files(down)
            .collect_output_files(
                "c",
                &TaskCommands {
                    output_files: vec!["a.txt".into()],
                    collect_to: Some(collect_to(&task)),
                    ..TaskCommands::default()
                },
                &mut r,
            )
            .await
            .expect_err("the store is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("kestra:///") && msg.contains("a.txt"), "{msg}");
        assert_eq!(r.failure, None, "never the task's failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Fix round 1, I3: the same for the output directory's arm.** The
    /// store fails while `docker cp c:<OUTPUT_DIR> -`'s files are collected:
    /// an `Error::Internal` naming the URI, never `result.failure` (which
    /// would spend an attempt on egret's outage, spec §11).
    #[tokio::test]
    async fn an_unreachable_store_while_collecting_the_output_directory_is_a_worker_error() {
        let dir = staging("collect-dir-down");
        let mut tree = crate::storage::transfer::header("out/", 0, b'5', 0o755, 0)
            .unwrap()
            .to_vec();
        tree.extend_from_slice(&file_tar("out/od.txt", b"in outdir"));
        let tree_path = dir.join("tree.tar");
        std::fs::write(&tree_path, tree).unwrap();
        let shim = cp_shim(&dir, &format!("cat {}", tree_path.display()));
        let task = leased(1);
        let down = std::sync::Arc::new(crate::storage::FileStore {
            store: std::sync::Arc::new(LateDown(std::time::Duration::ZERO)),
            ..std::sync::Arc::into_inner(mem_files()).unwrap()
        });
        let mut r = CommandsResult::default();
        let err = DockerRunner::with_binary(shim)
            .with_files(down)
            .collect_output_files(
                "c",
                &TaskCommands {
                    output_directory: true,
                    collect_to: Some(collect_to(&task)),
                    ..TaskCommands::default()
                },
                &mut r,
            )
            .await
            .expect_err("the store is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("kestra:///") && msg.contains("od.txt"),
            "{msg}"
        );
        assert_eq!(r.failure, None, "never the task's failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`outputDirectory: true`**: the whole directory is one
    /// `docker cp c:<OUTPUT_DIR> -`, every file in it keyed by its path
    /// relative to it and stored verbatim; the declared names keep Kestra's
    /// hashed names (`w9a-storage.json` `script_output_files_map`).
    #[tokio::test]
    async fn the_output_directory_is_one_copy_and_its_files_keep_their_paths() {
        let dir = staging("outdir");
        let log = dir.join("argv.log");
        let file = dir.join("f.tar");
        std::fs::write(&file, file_tar("a b.txt", b"hello a b")).unwrap();
        let mut tree = crate::storage::transfer::header("out/", 0, b'5', 0o755, 0)
            .unwrap()
            .to_vec();
        tree.extend_from_slice(
            &crate::storage::transfer::header("out/deep/", 0, b'5', 0o755, 0).unwrap(),
        );
        tree.extend_from_slice(&file_tar("out/deep/d.txt", b"deep"));
        let tree_path = dir.join("tree.tar");
        std::fs::write(&tree_path, tree).unwrap();
        let shim = cp_shim(
            &dir,
            &format!(
                "echo \"$2\" >> {log}; case \"$2\" in *.egret/out) cat {tree} ;; *) cat {file} ;; esac",
                log = log.display(),
                tree = tree_path.display(),
                file = file.display()
            ),
        );
        let fs = mem_files();
        let task = leased(1);
        let mut r = CommandsResult::default();
        DockerRunner::with_binary(shim)
            .with_files(fs.clone())
            .collect_output_files(
                "c",
                &TaskCommands {
                    output_files: vec!["a b.txt".into()],
                    output_directory: true,
                    collect_to: Some(collect_to(&task)),
                    ..TaskCommands::default()
                },
                &mut r,
            )
            .await
            .expect("collected");
        assert_eq!(r.failure, None, "{r:?}");
        let prefix = collect_to(&task).task_prefix;
        let uris: BTreeMap<&str, String> = r
            .output_files
            .iter()
            .map(|(k, v)| (k.as_str(), v.uri.clone()))
            .collect();
        assert_eq!(
            uris,
            BTreeMap::from([
                ("a b.txt", format!("{prefix}3a3goS5hpDPIJpezcLagQ0-a+b.txt")),
                ("deep/d.txt", format!("{prefix}deep/d.txt")),
            ])
        );
        assert_eq!(stored_bytes(&fs, &task, &r, "deep/d.txt").await, b"deep");
        assert_eq!(
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            [
                "c:/egret/working-dir/a b.txt",
                "c:/egret/working-dir/.egret/out"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A store failure part-way kills and reaps `docker cp` before its
    /// stdin closes**: it never sees the end of a cut-short archive, and no
    /// `docker cp` (nor its zombie) outlives the call.
    #[tokio::test]
    async fn a_store_failure_mid_copy_kills_docker_cp_before_its_stdin_closes() {
        let dir = staging("cp-kill");
        let pid = dir.join("pid");
        let log = dir.join("log");
        let shim = cp_shim(
            &dir,
            &format!(
                "echo $$ > {pid}; cat > /dev/null; echo eof >> {log}",
                pid = pid.display(),
                log = log.display()
            ),
        );
        let files = std::sync::Arc::new(crate::storage::FileStore {
            store: std::sync::Arc::new(LateDown(std::time::Duration::from_millis(500))),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: crate::storage::Caps::default(),
            retention_days: 30,
        });
        let runner = DockerRunner::with_binary(shim).with_files(files);
        let cmds = TaskCommands {
            stored_inputs: BTreeMap::from([(
                "in.bin".to_string(),
                StoredInput {
                    uri: "kestra:///a/f/executions/x/in.bin".into(),
                    object_uri: "s3://egret-test/x".into(),
                    size: 3,
                },
            )]),
            ..TaskCommands::default()
        };
        let err = runner
            .copy_in(&cmds, "egret-run-x-1")
            .await
            .expect_err("the store is down");
        assert!(matches!(err, Error::Internal(_)), "an outage: {err:?}");
        let pid = std::fs::read_to_string(&pid).expect("the shim started");
        let pid = pid.trim();
        // Synchronously, before the runtime can reap anything else.
        let ps = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .expect("ps");
        let state = String::from_utf8_lossy(&ps.stdout).trim().to_string();
        assert!(
            state.is_empty(),
            "`docker cp` (pid {pid}) outlived copy_in, state {state:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !log.exists(),
            "`docker cp` saw the end of a cut-short archive: {:?}",
            std::fs::read_to_string(&log)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A `docker cp` that stops reading and fails is classified, not an
    /// outage**: the daemon answered, so its message is the task's.
    #[tokio::test]
    async fn a_docker_cp_that_fails_mid_archive_is_classified_like_any_docker_call() {
        let dir = staging("cp-fail");
        let shim = cp_shim(&dir, "echo 'Error: no space left on device' >&2; exit 1");
        let runner = DockerRunner::with_binary(shim);
        let cmds = TaskCommands {
            // Over a pipe buffer, so the write itself fails.
            input_files: BTreeMap::from([("big.txt".to_string(), "x".repeat(1 << 20))]),
            ..TaskCommands::default()
        };
        let err = runner
            .copy_in(&cmds, "egret-run-x-1")
            .await
            .expect_err("cp failed");
        assert!(
            matches!(&err, Error::InvalidInput(m) if m.contains("no space left on device")),
            "{err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fix round 1, M3: a `docker cp` that writes a lot to stderr before it
    /// reads stdin does not deadlock the copy.
    #[tokio::test]
    async fn a_chatty_docker_cp_does_not_deadlock_the_copy() {
        let dir = staging("cp-chatty");
        let shim = cp_shim(
            &dir,
            "head -c 300000 /dev/zero | tr '\\0' x >&2; cat > /dev/null",
        );
        let runner = DockerRunner::with_binary(shim);
        let cmds = TaskCommands {
            input_files: BTreeMap::from([("big.txt".to_string(), "x".repeat(1 << 20))]),
            ..TaskCommands::default()
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            runner.copy_in(&cmds, "egret-run-x-1"),
        )
        .await
        .expect("the copy must not deadlock on a full stderr pipe")
        .expect("cp exited 0");
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
    /// marker is the author's (missing) file, never a worker failure.
    #[tokio::test]
    async fn an_output_file_named_like_a_daemon_marker_is_the_authors_missing_file() {
        let dir = staging("marker-output");
        let shim = dir.join("docker-shim.sh");
        std::fs::write(
            &shim,
            "#!/bin/sh\n\
             case \"$1\" in\n\
             \x20 create) echo id ;;\n\
             \x20 version) exit 0 ;;\n\
             \x20 inspect) case \"$*\" in *State.Status*) exit 1 ;; esac; echo 0 ;;\n\
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

        let runner = DockerRunner::with_binary(shim.display().to_string()).with_files(mem_files());
        let cmds = TaskCommands {
            container_image: "alpine:3.20".into(),
            commands: vec!["true".into()],
            output_files: vec!["docker.sock".into()],
            collect_to: Some(collect_to(&leased(1_700_000_000))),
            ..TaskCommands::default()
        };
        let sink = NoObserver;
        let out = runner
            .run(
                &leased(1_700_000_000),
                &cmds,
                &RunnerSpec::defaults(crate::scripts::RunnerKind::Docker),
                &sink,
            )
            .await
            .expect(
                "an author naming a file `docker.sock` must not be able to make egret \
                 report its own daemon as unreachable",
            );
        // A file the daemon could not copy while it answers is simply not
        // there -- the ordinary missing output, IGNORED since W5a Task 5 fix
        // round 1 (Kestra's `missing_declared_file`), and still no worker
        // failure.
        assert_eq!(out.failure, None, "{out:?}");
        assert!(out.output_files.is_empty(), "{out:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
