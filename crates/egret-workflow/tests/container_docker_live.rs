//! **`egret.container` and `egret.script.python` against a real Docker daemon —
//! the only instrument either handler has that judges the outside world.**
//!
//! # Why this file is not optional
//!
//! `tests/executor.rs` compares execution traces against Kestra's own corpus.
//! That corpus holds **345** flows and **zero** of them declare a script,
//! container, shell or command task type — measured, not assumed:
//!
//! ```bash
//! cd crates/egret-workflow/oracle-exec
//! find corpus -name '*.yaml' -o -name '*.yml' | wc -l          # 345
//! grep -rhoE 'type:\s*\S+' corpus | sed 's/.*type:[[:space:]]*//' | sort -u \
//!   | grep -icE 'script|container|shell|command|docker|bash|python|node'   # 0
//! ```
//!
//! So the execution ratchet cannot move for these handlers in either direction.
//! The unit tests in `worker::commands`, `worker::handlers::container` and
//! `worker::handlers::python` judge everything that can be decided without a
//! daemon — fix round 1 moved two pure seams (`ingest_line`,
//! `ingest_output_file`) out of `DockerRunner::run_created` precisely so that
//! set is as large as it can be. What is left here is what genuinely needs a
//! daemon: *was a process actually started*, and *does the daemon agree about
//! which container this worker owns*.
//!
//! # It FAILS without Docker, and the skip has to be typed
//!
//! `sql_starrocks_live.rs`'s own history is the argument: it self-skipped, and
//! the two runs were indistinguishable except by the clock — `7 passed; 0
//! failed; 0 ignored` in `0.00s` against the same line in `0.88s`.
//!
//! # This file runs WITHOUT libtest, and that is the second half of the same rule
//!
//! libtest has no stable way for a test to report itself **ignored** at run
//! time, so every self-skipping suite in this workspace reports its skipped
//! tests as **passed**. A test that did not run must not be counted as passed:
//! that alone would have made the `sql_starrocks_live` defect unmisreadable the
//! day it shipped. So this target is `harness = false` (see `Cargo.toml`) and
//! [`main`] prints libtest's own line format with the counts telling the truth:
//!
//! | invocation | what it prints |
//! |---|---|
//! | daemon + images | `N passed; 0 failed; 0 ignored` |
//! | no daemon, no opt-out | `1 passed; N-1 failed; 0 ignored` and exit 101 |
//! | no daemon, `EGRET_ALLOW_DOCKER_SKIP=1` | `1 passed; 0 failed; N-1 ignored` |
//! | daemon, image missing, opt-out typed | the same, naming the `docker pull` |
//!
//! What the hand-rolled runner does **not** implement, stated rather than
//! discovered: parallelism (it is sequential, and the whole suite is ~5s),
//! `--exact`, `--list`, `--format json`, and `#[should_panic]`. A bare
//! positional argument filters by substring, which is what `cargo test -p
//! egret-workflow --test container_docker_live <name>` needs.
//!
//! # Two axes, and the image axis is one of them
//!
//! Fix round 1's Important 3: the opt-out used to cover the daemon and not the
//! images, so a machine with Docker running and nothing pulled hard-failed 12
//! tests through `just check`. Both are now the same verdict function, driven
//! in both directions by a test that needs neither.
//!
//! # Every assertion here is written so it could not pass against a fake
//!
//! The numbers are computed **inside the container** by a program this file
//! sends it — `4950` is `sum(range(100))` and appears nowhere else in this
//! crate — and the container-identity property is asserted by asking the daemon
//! what is running, not by asking egret what it did.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use egret_workflow::worker::commands::{container_name, DockerRunner, TaskCommands, TaskRunner};
use egret_workflow::worker::handlers::{container, python, Level, LogSink, TracingSink};
use egret_workflow::worker::{Outcome, TaskHandler, TaskInput};
use uuid::Uuid;

/// The image the container tests run in. Small, has `/bin/sh`, and is not
/// Python — so an `egret.script.python` assertion cannot pass by accident here.
const SHELL_IMAGE: &str = "alpine:3.20";

/// `python::DEFAULT_IMAGE`, named again so the pre-flight check can tell the
/// reader which pull is missing.
const PYTHON_IMAGE: &str = python::DEFAULT_IMAGE;

/// `sum(range(100))`. It exists only if a real CPython computed it: this file
/// never writes it, and neither does any fixture in this crate.
const SUM_TO_100: &str = "4950";

/// The opt-out, and it has to be **typed into the invocation**.
pub const ALLOW_DOCKER_SKIP: &str = "EGRET_ALLOW_DOCKER_SKIP";

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// What a run should do, as a pure function of the three facts that decide it.
///
/// Pure for `egret_test_db::skip_verdict`'s reason: `std::env::set_var` is
/// `unsafe` and racy beside other threads' reads, so a test that flipped the
/// real variable would be a worse test than no test. This one is driven
/// directly, in every direction, by
/// [`the_docker_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed`].
///
/// `Ok(true)` run, `Ok(false)` skip quietly because the operator asked,
/// `Err(msg)` fail with `msg`.
fn docker_verdict(
    has_daemon: bool,
    missing_images: &[&str],
    allow_skip: bool,
) -> Result<bool, String> {
    if has_daemon && missing_images.is_empty() {
        return Ok(true);
    }
    if allow_skip {
        return Ok(false);
    }
    let what = if !has_daemon {
        "No Docker daemon answered, so there is nothing to run `egret.container` \
         or `egret.script.python` in."
            .to_string()
    } else {
        format!(
            "The Docker daemon answered, but {} {} not on this machine. Pulling \
             inside a test would make its duration a measurement of the network, \
             so this suite refuses instead.",
            missing_images.join(" and "),
            if missing_images.len() == 1 {
                "is"
            } else {
                "are"
            }
        )
    };
    Err(format!(
        "{what}\n\
         \n\
         This suite does NOT skip on its own. `tests/sql_starrocks_live.rs` used \
         to, and the whole-branch review measured what that looked like from \
         outside: `7 passed; 0 failed; 0 ignored` in 0.00s against the identical \
         line in 0.88s. These two handlers reach ZERO of the 345 corpus flows — \
         measured — so the differential oracle can never judge them and an empty \
         green here would be most of their evidence about the daemon.\n\
         \n\
         Pick one:\n\
         \n\
         \x20 1. Start a daemon and pull the two images this suite uses:\n\
         \x20      # macOS: Docker Desktop, or `brew install colima && colima start`\n\
         \x20      # Linux: `sudo systemctl start docker`, and be in the `docker` group\n\
         \x20      docker pull {SHELL_IMAGE}\n\
         \x20      docker pull {PYTHON_IMAGE}\n\
         \x20      just test-workflow-docker      # does both, and refuses the opt-out\n\
         \n\
         \x20 2. Deliberately skip it, visibly, in the invocation:\n\
         \x20      {ALLOW_DOCKER_SKIP}=1 cargo test -p egret-workflow --test container_docker_live\n\
         \n\
         A skipped run reports `0 failed; N ignored`, never `N passed`: a test \
         that did not run is not a test that passed."
    ))
}

/// Does a daemon answer?
///
/// `docker version` and not `docker --version`: the second prints the CLI's own
/// version without talking to anything, so it is green on a machine with a
/// client and no daemon — which is precisely the machine this gate is for.
/// `--format {{.Server.Version}}` fails unless the **server** answered.
fn daemon_answers() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn image_present(image: &str) -> bool {
    std::process::Command::new("docker")
        .args(["image", "inspect", image])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// **The gate is a real gate, in every direction.**
///
/// The one case that needs neither a daemon nor an image, so it runs on every
/// machine: it drives the pure verdict rather than the environment.
fn the_docker_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed() {
    assert_eq!(
        docker_verdict(true, &[], false),
        Ok(true),
        "a daemon and the images is enough"
    );
    assert_eq!(
        docker_verdict(true, &[], true),
        Ok(true),
        "the opt-out does not turn a working daemon off"
    );
    assert_eq!(
        docker_verdict(false, &[], true),
        Ok(false),
        "a typed opt-out skips quietly, which is the operator's own choice"
    );
    assert_eq!(
        docker_verdict(true, &[SHELL_IMAGE], true),
        Ok(false),
        "the IMAGE axis is under the same opt-out as the daemon axis -- fix \
         round 1's Important 3 was that it was not, so a machine with Docker \
         running and nothing pulled hard-failed through `just check`"
    );

    let msg = docker_verdict(false, &[], false)
        .expect_err("no daemon and no typed opt-out must FAIL, not skip");
    assert!(
        msg.contains(ALLOW_DOCKER_SKIP),
        "the failure has to say what to type: {msg}"
    );
    assert!(
        msg.contains("docker pull alpine:3.20") && msg.contains("colima"),
        "more machines lack a daemon than lack a database, so the message says \
         what to install and what to pull: {msg}"
    );

    let msg = docker_verdict(true, &[PYTHON_IMAGE], false)
        .expect_err("a missing image with no typed opt-out must FAIL too");
    assert!(
        msg.contains(PYTHON_IMAGE),
        "the failure has to name the image that is missing: {msg}"
    );
    assert!(
        msg.contains("measurement of the network"),
        "and why it will not just pull it: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A recording [`LogSink`]. `handlers::testing::Recorder` is `pub(crate)`, so
/// an integration test cannot reach it.
#[derive(Debug, Default)]
struct Recorder {
    lines: Mutex<Vec<(Level, String)>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn lines(&self) -> Vec<(Level, String)> {
        self.lines.lock().unwrap().clone()
    }
    fn texts(&self) -> Vec<String> {
        self.lines().into_iter().map(|(_, t)| t).collect()
    }
}

impl LogSink for Recorder {
    fn log(&self, _task: &TaskInput, level: Level, message: &str) {
        self.lines
            .lock()
            .unwrap()
            .push((level, message.to_string()));
    }
}

/// A [`TaskInput`] with a **fresh** run id and a lease of `now`, so two tests
/// can never address the same container.
fn task(type_id: &str, spec: serde_json::Value) -> TaskInput {
    TaskInput {
        workspace_id: Uuid::new_v4(),
        execution_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        task_id: "t".to_string(),
        type_id: type_id.to_string(),
        value: None,
        attempt: 1,
        spec,
        render: egret_workflow::runners::RenderContext::new(),
        timeout: None,
        principal: None,
        claimed_at: chrono::Utc::now(),
    }
}

/// Ask the daemon, not egret, whether a container is there.
fn container_exists(name: &str) -> bool {
    std::process::Command::new("docker")
        .args(["inspect", "--type", "container", name])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn container_running(name: &str) -> bool {
    let out = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", name])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("docker inspect");
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true"
}

fn force_remove(name: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rm", "--force", name])
        .stdin(std::process::Stdio::null())
        .output();
}

fn outputs(out: &Outcome) -> serde_json::Value {
    out.outputs.clone().expect("this task produces outputs")
}

fn failed(out: &Outcome) -> String {
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Failed,
        "expected a FAILED task, got {out:?}"
    );
    out.message
        .clone()
        .expect("a failed task carries a message")
}

// ---------------------------------------------------------------------------
// The assertions
// ---------------------------------------------------------------------------

/// **A container runs, and every line it printed reached the sink, on the
/// stream it was printed to.**
///
/// The level itself is `worker::commands`'s unit business now; what only a
/// daemon can say is that a **process** ran and that its two pipes were both
/// read.
async fn a_container_runs_and_both_of_its_streams_reach_the_sink() {
    let rec = Recorder::new();
    let h = container::Container::new().with_sink(rec.clone());
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["echo out-one", "echo out-two", "echo err-one 1>&2"],
        }),
    );

    let out = h
        .run(&t)
        .await
        .expect("a real daemon is not a worker failure");
    assert_eq!(out.state, egret_workflow::executor::RunState::Success);

    let o = outputs(&out);
    assert_eq!(o["exitCode"], serde_json::json!(0));
    assert_eq!(o["stdOutLineCount"], serde_json::json!(2));
    assert_eq!(o["stdErrLineCount"], serde_json::json!(1));

    let texts = rec.texts();
    assert!(
        texts.contains(&"out-one".to_string())
            && texts.contains(&"out-two".to_string())
            && texts.contains(&"err-one".to_string()),
        "every line the container printed must reach the sink: {texts:?}"
    );
    assert_eq!(
        rec.lines()
            .into_iter()
            .find(|(_, t)| t == "err-one")
            .map(|(l, _)| l),
        Some(Level::Warn),
        "and the stream it came from must survive the trip"
    );
    assert!(
        !container_exists(&container_name(&t)),
        "a finished run removes its container -- `$default$delete` is true"
    );
}

/// **The `::{…}::` marker becomes task outputs**, and an ordinary line does
/// not.
async fn an_output_marker_printed_by_the_container_becomes_task_outputs() {
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            // The value is computed by the shell, so it exists only if the
            // container ran: 6 * 7.
            "commands": [
                "echo not-a-marker",
                "echo \"::{\\\"outputs\\\":{\\\"answer\\\":$((6*7))}}::\"",
            ],
        }),
    );
    let out = h.run(&t).await.expect("no worker failure");
    assert_eq!(out.state, egret_workflow::executor::RunState::Success);
    assert_eq!(
        outputs(&out)["vars"],
        serde_json::json!({"answer": 42}),
        "the ordinary line beside it must contribute nothing"
    );
}

/// **An output file comes back with its contents, and a declared file the
/// command did not produce fails the task WITHOUT losing what it did
/// produce.**
///
/// The second half is fix round 1's Important 1 seen end to end: the exit code
/// and the marker vars survive the failure, because
/// `RunnableTaskException(e, ScriptOutput)` is an exception carrying an Output.
async fn an_output_file_comes_back_and_a_missing_one_fails_without_losing_the_rest() {
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": [
                "seq 1 5 | tr '\\n' ',' > counted.csv",
                "mkdir -p sub && echo deep > sub/inner.txt",
            ],
            "outputFiles": ["counted.csv", "sub/inner.txt"],
        }),
    );
    let out = h.run(&t).await.expect("no worker failure");
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{out:?}"
    );
    let files = &outputs(&out)["outputFiles"];
    assert_eq!(
        files["counted.csv"],
        serde_json::json!("1,2,3,4,5,"),
        "the contents come back, computed by `seq` inside the container"
    );
    assert_eq!(files["sub/inner.txt"], serde_json::json!("deep\n"));

    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["echo '::{\"outputs\":{\"printed\":1}}::'"],
            "outputFiles": ["never-written.txt"],
        }),
    );
    let out = h
        .run(&t)
        .await
        .expect("a missing output file is the author's flow, never a worker failure");
    let msg = failed(&out);
    assert!(
        msg.contains("never-written.txt"),
        "a declared output that is absent must fail the task NAMING it: {msg}"
    );
    let o = outputs(&out);
    assert_eq!(
        o["exitCode"],
        serde_json::json!(0),
        "the exit code survives the output-file failure"
    );
    assert_eq!(
        o["vars"],
        serde_json::json!({"printed": 1}),
        "and so do the vars the command printed before it"
    );
}

/// **A DIRECTORY named as an output file fails the task, not the worker.**
///
/// Fix round 1's Critical 1, driven where it was measured: `fs::read` on a
/// directory used to give an `Error::Internal`, which the handler correctly
/// reads as egret's own outage — so one typo'd flow aborted the tick every time
/// it was claimed, for ever, blaming egret. `expect` on the `Result` is the
/// assertion: an `Err` here IS the bug.
async fn a_directory_named_as_an_output_file_does_not_wedge_the_worker() {
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["mkdir -p results && echo x > results/inner"],
            "outputFiles": ["results"],
        }),
    );
    let out = h
        .run(&t)
        .await
        .expect("a typo'd `outputFiles` must not abort the tick -- see Critical 1");
    let msg = failed(&out);
    assert!(msg.contains("DIRECTORY"), "{msg}");
    assert!(msg.contains("results"), "{msg}");
}

/// **A non-zero exit fails the task on a real daemon**, and `set -e` is what
/// makes the first failing line the one that decides it.
async fn a_non_zero_exit_fails_the_task_and_set_e_stops_at_the_first_failure() {
    let rec = Recorder::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["echo before", "exit 7", "echo after"],
        }),
    );
    let out = container::Container::new()
        .with_sink(rec.clone())
        .run(&t)
        .await
        .expect("a command that failed is the author's flow");
    assert_eq!(
        failed(&out),
        "egret.container: the command exited with code 7"
    );
    assert_eq!(outputs(&out)["exitCode"], serde_json::json!(7));
    let texts = rec.texts();
    assert!(texts.contains(&"before".to_string()), "{texts:?}");
    assert!(
        !texts.contains(&"after".to_string()),
        "`set -e` ends the script at the first failing line: {texts:?}"
    );
    assert!(!container_exists(&container_name(&t)));
}

/// **`egret.script.python` runs the author's program in the artifact's own
/// default image**, streams it, and its outputs are computed by CPython.
async fn python_runs_the_authors_script_and_its_numbers_come_from_cpython() {
    let rec = Recorder::new();
    let h = python::Python::new().with_sink(rec.clone());
    let t = task(
        python::TYPE_ID,
        serde_json::json!({
            "script": "import json, sys\n\
                       total = sum(range(100))\n\
                       print('python', sys.version_info[0], flush=True)\n\
                       print('::' + json.dumps({'outputs': {'total': total}}) + '::')\n\
                       open('total.txt', 'w').write(str(total))\n",
            "outputFiles": ["total.txt"],
        }),
    );
    let out = h.run(&t).await.expect("no worker failure");
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{out:?}"
    );

    let o = outputs(&out);
    assert_eq!(
        o["vars"]["total"],
        serde_json::json!(4950),
        "the marker's payload was serialised by `json.dumps` inside the container"
    );
    assert_eq!(o["outputFiles"]["total.txt"], serde_json::json!(SUM_TO_100));
    assert!(
        rec.texts().iter().any(|l| l.starts_with("python 3")),
        "the interpreter that answered is the image's own: {:?}",
        rec.texts()
    );
}

/// **A dropped future does NOT stop a container, and `cancel` is what does.**
///
/// This is the property the whole of `TaskInput::claimed_at` exists for, and it
/// is the one no unit test can see: `worker::timeout::guard` enforces a
/// task-level `timeout:` by **dropping the future**, which is all a `Sleep`
/// needs and all `egret.sql` gets. A container outlives that drop, because the
/// daemon is holding it and not this process.
async fn a_dropped_future_leaves_the_container_running_and_cancel_is_what_stops_it() {
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({"image": SHELL_IMAGE, "commands": ["sleep 120"]}),
    );
    let name = container_name(&t);

    let dropped = tokio::time::timeout(std::time::Duration::from_millis(2500), h.run(&t)).await;
    assert!(dropped.is_err(), "the handler must still have been running");

    assert!(
        container_running(&name),
        "dropping the future is exactly as forceful as dropping a future: the \
         container is the daemon's, and this is the over-acceptance that `cancel` \
         exists to close"
    );

    h.cancel(&t).await.expect("a container CAN be stopped");
    assert!(
        !container_exists(&name),
        "`cancel` is `docker rm --force`, which is `killContainerCmd` + delete — \
         Kestra's two shipped defaults ($default$killGracePeriod = ZERO, \
         $default$delete = true)"
    );

    h.cancel(&t)
        .await
        .expect("a container that is not there is nothing to stop, not a refusal");
}

/// **A reaped worker's `cancel` cannot reach its replacement's container.**
///
/// `TaskHandler::cancel`'s hazard note, driven against the daemon. Worker B
/// holds the run under lease `t2`; worker A — reaped, still inside its handler,
/// about to have its own budget expire — calls `cancel` with the same `run_id`
/// and its **stale** lease `t1`. `attempt` is identical, because `reap` leaves
/// the attempt open.
///
/// Two mutations discriminate here and they are different ones: dropping the
/// lease from the name is caught by the vacuity guard below, and addressing the
/// container by its `run_id` **label** — the design this one was chosen over —
/// is caught by the final assertion and by nothing else in the workspace.
async fn a_reaped_workers_cancel_cannot_reach_its_replacements_container() {
    let h = container::Container::new();

    let run_id = Uuid::new_v4();
    let spec = serde_json::json!({"image": SHELL_IMAGE, "commands": ["sleep 120"]});

    let stale = TaskInput {
        run_id,
        claimed_at: chrono::Utc::now() - chrono::Duration::seconds(30),
        ..task(container::TYPE_ID, spec.clone())
    };
    let live = TaskInput {
        claimed_at: chrono::Utc::now(),
        ..stale.clone()
    };
    assert_eq!(stale.run_id, live.run_id);
    assert_eq!(stale.attempt, live.attempt);
    assert_ne!(
        container_name(&stale),
        container_name(&live),
        "if these are equal the rest of this test is vacuous"
    );

    let running = tokio::time::timeout(std::time::Duration::from_millis(2500), h.run(&live)).await;
    assert!(running.is_err(), "B must still be inside its handler");
    let b_name = container_name(&live);
    assert!(container_running(&b_name), "B's container must be up");

    h.cancel(&stale)
        .await
        .expect("a stale worker's kill finds nothing, which is not a refusal");

    assert!(
        container_running(&b_name),
        "a reaped worker's cancel reached its REPLACEMENT's container. The lease \
         token is what stops that (see `TaskInput::claimed_at`), and a cancel that \
         found the container by its `run_id` LABEL instead would fail exactly here"
    );

    force_remove(&b_name);
}

/// **An image that does not exist fails the task, and does not abort the
/// tick.**
async fn an_image_that_does_not_exist_fails_the_task_rather_than_the_worker() {
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": "egret.invalid/there-is-no-such-image:0",
            "commands": ["true"],
        }),
    );
    let out = h
        .run(&t)
        .await
        .expect("an image that does not exist is the author's flow, not egret's outage");
    assert!(
        failed(&out).contains("egret.container:"),
        "{:?}",
        out.message
    );
    assert!(
        !container_exists(&container_name(&t)),
        "a create that failed leaves nothing behind"
    );
}

/// **The runner refuses an empty image with the artifact's own sentence**, and
/// refuses it before it creates anything.
async fn the_runner_refuses_an_empty_image_with_the_artifacts_own_message() {
    let runner = DockerRunner::new();
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let err = runner
        .run(
            &t,
            &TaskCommands {
                container_image: String::new(),
                commands: vec!["true".into()],
                ..TaskCommands::default()
            },
            &TracingSink,
        )
        .await
        .expect_err("no image is a refusal");
    assert_eq!(
        err.to_string(),
        "invalid input: This task runner needs the `containerImage` property to be set",
        "`Docker.run`'s own first act and its own string"
    );
    assert!(!container_exists(&container_name(&t)));
}

/// **A path that escapes the working directory is refused on both channels** —
/// the one a handler writes and the one it reads back — and neither refusal is
/// a worker failure.
async fn a_path_that_escapes_the_working_directory_is_refused_by_the_runner() {
    let runner = DockerRunner::new();

    // **After `docker create`, not before it** — review Minor 1, which said
    // this exact sentence was false when it was a test heading, and which I
    // then reproduced verbatim as this comment. `check_relative` for
    // `input_files` runs inside `run_created`, i.e. once the container exists;
    // the refusal is an `Err` because nothing has RUN yet and there is no exit
    // code or var to preserve, which is a different fact from when it happens.
    // The container is removed on the way out, which the assertion at the end
    // of this test is what checks.
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let err = runner
        .run(
            &t,
            &TaskCommands {
                container_image: SHELL_IMAGE.to_string(),
                commands: vec!["true".into()],
                input_files: BTreeMap::from([("../escaped.txt".to_string(), "x".to_string())]),
                ..TaskCommands::default()
            },
            &TracingSink,
        )
        .await
        .expect_err("`..` must be refused");
    assert!(err.to_string().contains("climbs out"), "got {err}");

    // Output files are refused AFTER it ran, so the refusal rides on the
    // result and the exit code survives.
    let t2 = task(container::TYPE_ID, serde_json::json!({}));
    let out = runner
        .run(
            &t2,
            &TaskCommands {
                container_image: SHELL_IMAGE.to_string(),
                commands: vec!["true".into()],
                output_files: vec!["/etc/passwd".to_string()],
                ..TaskCommands::default()
            },
            &TracingSink,
        )
        .await
        .expect("an absolute output path is the author's flow, not egret's outage");
    assert!(
        out.failure
            .as_deref()
            .expect("the task must fail")
            .contains("absolute path"),
        "got {:?}",
        out.failure
    );
    assert_eq!(out.exit_code, 0, "the exit code survives the refusal");

    for t in [&t, &t2] {
        assert!(
            !container_exists(&container_name(t)),
            "a refused run removes whatever it created"
        );
    }
}

/// **A symlinked output file cannot read the worker host's files.**
///
/// Fix round 2's Critical, driven exactly where the reviewer found it. The
/// boundary that matters is the **worker host**: the task already owns its
/// container and can `cat` anything in it, so the defect was never "a file
/// outside the working directory" — it was *the worker's* file arriving as a
/// successful task output. Both legs are driven, and they now end differently,
/// which is the finding this test records:
///
/// * **leaf** — `ln -s /etc/passwd leaked.txt`. `docker cp` recreates the link
///   on the host, so this is the escape, and it is **refused**.
/// * **intermediate** — `ln -s /etc etcdir`, declared as `etcdir/passwd`. With
///   the per-file copy restored, `docker cp` resolves `etcdir` **inside the
///   container**, so what arrives is a regular file holding *alpine's*
///   `/etc/passwd` — the container's own, which the task could have printed
///   anyway. No host file, no link on the host, nothing for the guard to see.
///
/// So the second leg **succeeds**, and the assertion that matters for it is
/// that the content is the container's and not the worker's. The two are
/// distinguishable byte for byte: alpine writes `root:x:0:0`, macOS writes
/// `root:*:0:0`, and the sizes differed 702 against 9344 on the machine this
/// was found on.
///
/// The residual is stated rather than hidden: a declared name can still reach
/// outside the working directory *within the container*. Kestra's tar-stream
/// copy would return an **empty** file there (a link entry carries no content
/// for `Files.copy` to materialise); egret returns the container's file. Both
/// are inside the sandbox; neither is the host.
async fn a_symlinked_output_file_cannot_read_the_worker_hosts_files() {
    let h = container::Container::new();

    // Whatever the worker host's own passwd file says, no task output may
    // contain it. Read here so the control is the real file and not a guess
    // about which OS this is.
    let host_passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let host_marker = host_passwd
        .lines()
        .find(|l| l.starts_with("root:"))
        .unwrap_or("root:*:0:0")
        .to_string();

    // ---- leaf: the escape, refused ----
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["ln -s /etc/passwd leaked.txt", "echo '::{\"outputs\":{\"ran\":1}}::'"],
            "outputFiles": ["leaked.txt"],
        }),
    );
    let out = h
        .run(&t)
        .await
        .expect("a symlink is the author's flow, never a worker failure");
    let msg = failed(&out);
    assert!(
        msg.contains("SYMLINK") && msg.contains("leaked.txt"),
        "a declared output that is a symlink must be refused, naming it: {msg}"
    );
    let o = outputs(&out);
    assert_eq!(
        o["outputFiles"],
        serde_json::json!({}),
        "nothing may be read through a link: {o}"
    );
    assert_eq!(
        o["vars"],
        serde_json::json!({"ran": 1}),
        "the guard must fire on a run that otherwise succeeded: {o}"
    );
    assert!(
        !o.to_string().contains(&host_marker),
        "the WORKER HOST's passwd file reached the task outputs -- this is the \
         sandbox escape fix round 2 exists to close"
    );

    // ---- intermediate: resolved container-side, so it is the container's ----
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["ln -s /etc etcdir", "echo '::{\"outputs\":{\"ran\":1}}::'"],
            "outputFiles": ["etcdir/passwd"],
        }),
    );
    let out = h.run(&t).await.expect("no worker failure");
    let o = outputs(&out);
    let got = o["outputFiles"]["etcdir/passwd"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !got.is_empty(),
        "the per-file copy resolves the component inside the container, so this \
         leg returns the CONTAINER's file: {o}"
    );
    assert!(
        got.contains("root:x:0:0"),
        "and it must be alpine's, which writes `root:x:0:0`: {got}"
    );
    assert!(
        !got.contains(&host_marker) && !o.to_string().contains(&host_marker),
        "the worker host's passwd file must not appear on this leg either -- \
         that is what the whole-directory copy did, and what restoring the \
         per-file copy undoes"
    );
}

/// **An output file over the cap fails the task rather than being truncated**,
/// end to end. The per-file and total caps themselves are
/// `worker::commands`'s unit business; what this adds is that a real
/// `docker cp` of a real file goes through the same seam.
async fn an_output_file_over_the_cap_fails_the_task_rather_than_being_truncated() {
    let runner = DockerRunner::new().with_limits(64, 1_048_576);
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let cmds = TaskCommands {
        container_image: SHELL_IMAGE.to_string(),
        commands: vec!["printf 'x%.0s' $(seq 1 100) > big.txt".into()],
        output_files: vec!["big.txt".into()],
        ..TaskCommands::default()
    };
    let out = runner
        .run(&t, &cmds, &TracingSink)
        .await
        .expect("a file over the cap is the author's flow");
    let why = out.failure.clone().expect("the cap must fail the task");
    assert!(why.contains("100 bytes"), "the SIZE must be named: {why}");
    assert!(why.contains("over the 64"), "the CAP must be named: {why}");
    assert!(
        out.output_files.is_empty(),
        "and nothing came back truncated"
    );

    // The same file under the cap comes back whole, so the refusal above is
    // the cap and not the copy failing.
    let runner = DockerRunner::new().with_limits(1024, 1_048_576);
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let out = runner
        .run(&t, &cmds, &TracingSink)
        .await
        .expect("100 bytes is under a 1024-byte cap");
    assert_eq!(out.failure, None, "{out:?}");
    assert_eq!(out.output_files["big.txt"].len(), 100);
}

/// **A container that floods the marker channel is refused rather than
/// stored**, and the run still finishes.
async fn a_flood_of_output_markers_is_refused_rather_than_stored() {
    let runner = DockerRunner::new().with_limits(1_048_576, 32);
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let out = runner
        .run(
            &t,
            &TaskCommands {
                container_image: SHELL_IMAGE.to_string(),
                commands: vec![
                    "echo \"::{\\\"outputs\\\":{\\\"a\\\":\\\"$(printf 'y%.0s' $(seq 1 200))\\\"}}::\"".into(),
                    "echo after-the-flood".into(),
                ],
                ..TaskCommands::default()
            },
            &TracingSink,
        )
        .await
        .expect("a flooding container is the author's flow");
    let why = out.failure.clone().expect("the cap must fail the task");
    assert!(why.contains("markers"), "the channel must be named: {why}");
    assert!(out.vars.is_empty(), "and none of it was kept");
    assert_eq!(
        out.stdout_line_count, 2,
        "the stream keeps being read after the cap fires, so the container is \
         never left blocked on a full pipe"
    );
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

type Case = fn() -> Pin<Box<dyn Future<Output = ()> + Send>>;

macro_rules! case {
    ($f:ident) => {
        (stringify!($f), (|| Box::pin($f())) as Case)
    };
}

/// The cases that need a daemon, in one list so the gate decides once.
fn daemon_cases() -> Vec<(&'static str, Case)> {
    vec![
        case!(a_container_runs_and_both_of_its_streams_reach_the_sink),
        case!(a_directory_named_as_an_output_file_does_not_wedge_the_worker),
        case!(a_dropped_future_leaves_the_container_running_and_cancel_is_what_stops_it),
        case!(a_flood_of_output_markers_is_refused_rather_than_stored),
        case!(a_non_zero_exit_fails_the_task_and_set_e_stops_at_the_first_failure),
        case!(a_path_that_escapes_the_working_directory_is_refused_by_the_runner),
        case!(a_reaped_workers_cancel_cannot_reach_its_replacements_container),
        case!(a_symlinked_output_file_cannot_read_the_worker_hosts_files),
        case!(an_image_that_does_not_exist_fails_the_task_rather_than_the_worker),
        case!(an_output_file_comes_back_and_a_missing_one_fails_without_losing_the_rest),
        case!(an_output_file_over_the_cap_fails_the_task_rather_than_being_truncated),
        case!(an_output_marker_printed_by_the_container_becomes_task_outputs),
        case!(python_runs_the_authors_script_and_its_numbers_come_from_cpython),
        case!(the_runner_refuses_an_empty_image_with_the_artifacts_own_message),
    ]
}

/// libtest's line format, by hand, so a skip can report `ignored` — which
/// libtest has no stable way to do at run time. See this module's doc.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let matches = |name: &str| filter.as_ref().is_none_or(|f| name.contains(f.as_str()));

    let gate_name = "the_docker_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed";
    let cases = daemon_cases();
    let total = cases.len() + 1;
    let selected: Vec<&(&str, Case)> = cases.iter().filter(|(n, _)| matches(n)).collect();
    let run_gate = matches(gate_name);
    let filtered_out = total - selected.len() - usize::from(run_gate);

    let missing: Vec<&str> = [SHELL_IMAGE, PYTHON_IMAGE]
        .into_iter()
        .filter(|i| !image_present(i))
        .collect();
    let verdict = docker_verdict(
        daemon_answers(),
        &missing,
        std::env::var_os(ALLOW_DOCKER_SKIP).is_some(),
    );

    let started = std::time::Instant::now();
    println!("\nrunning {} tests", selected.len() + usize::from(run_gate));

    let (mut passed, mut ignored) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();

    if run_gate {
        // The one case that needs nothing, so it runs whatever the verdict is.
        match std::panic::catch_unwind(
            the_docker_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed,
        ) {
            Ok(()) => {
                println!("test {gate_name} ... ok");
                passed += 1;
            }
            Err(_) => {
                println!("test {gate_name} ... FAILED");
                failures.push(gate_name.to_string());
            }
        }
    }

    match &verdict {
        Err(why) => {
            eprintln!("\n{why}\n");
            for (name, _) in &selected {
                println!("test {name} ... FAILED");
                failures.push((*name).to_string());
            }
        }
        Ok(false) => {
            eprintln!(
                "skipping: {ALLOW_DOCKER_SKIP} was typed, so {} tests did not run and are \
                 reported IGNORED rather than passed",
                selected.len()
            );
            for (name, _) in &selected {
                println!("test {name} ... ignored");
                ignored += 1;
            }
        }
        Ok(true) => {
            let rt = tokio::runtime::Runtime::new().expect("a tokio runtime");
            for (name, case) in &selected {
                let outcome = rt.block_on(async { tokio::spawn(case()).await });
                match outcome {
                    Ok(()) => {
                        println!("test {name} ... ok");
                        passed += 1;
                    }
                    Err(_) => {
                        println!("test {name} ... FAILED");
                        failures.push((*name).to_string());
                    }
                }
            }
        }
    }

    if !failures.is_empty() {
        println!("\nfailures:\n");
        for f in &failures {
            println!("    {f}");
        }
    }
    println!(
        "\ntest result: {}. {passed} passed; {} failed; {ignored} ignored; 0 measured; \
         {filtered_out} filtered out; finished in {:.2}s\n",
        if failures.is_empty() { "ok" } else { "FAILED" },
        failures.len(),
        started.elapsed().as_secs_f64()
    );
    if !failures.is_empty() {
        std::process::exit(101);
    }
}
