//! **`egret.container` and `egret.python` against a real Docker daemon — the
//! only instrument either handler has that judges the outside world.**
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
//! So the execution ratchet cannot move for these handlers in either direction,
//! and a green ratchet is not evidence about them. The unit tests in
//! `worker::handlers::container` and `worker::handlers::python` drive a **fake**
//! runner, which means they judge the handler and never the daemon: what image
//! naming does, whether a dropped future leaves a container running, whether
//! `docker cp` brings a file back. This file is the half that can see any of
//! that, and it is modelled on `tests/sql_starrocks_live.rs`, which exists
//! because the same gap was found in `egret.sql`.
//!
//! # It FAILS without Docker, and the skip has to be typed
//!
//! `sql_starrocks_live.rs`'s own history is the argument: it self-skipped, and
//! the two runs were indistinguishable except by the clock — `7 passed; 0
//! failed; 0 ignored` in `0.00s` against the same line in `0.88s`. The rule that
//! fixed it is applied here on its own axis: no daemon **fails**, and skipping
//! requires [`ALLOW_DOCKER_SKIP`] typed into the invocation.
//!
//! **A third variable rather than reusing `EGRET_ALLOW_ENGINE_SKIP`**, for the
//! reason that file gives for not reusing `EGRET_ALLOW_DB_SKIP`: the metastore,
//! the query engine and the container daemon are different axes and a machine
//! may honestly have one and not another. The cost is higher on this axis than
//! on the other two, and the failure message is written for it: more machines
//! lack a Docker daemon than lack a Postgres, so the message says exactly what
//! to install, what to pull, and what to type instead.
//!
//! # Running it
//!
//! ```bash
//! # Docker Desktop / colima / dockerd running, then:
//! docker pull alpine:3.20
//! docker pull python:3.13-slim
//! cargo test -p egret-workflow --test container_docker_live
//!
//! # or, deliberately and visibly, with no daemon:
//! EGRET_ALLOW_DOCKER_SKIP=1 cargo test -p egret-workflow --test container_docker_live
//! ```
//!
//! No `DATABASE_URL`: nothing here touches Postgres. These are handler-level
//! properties, and the worker-level ones (the claim, the fence, the reaper)
//! belong to `tests/worker.rs`, which owns that database.
//!
//! # Every assertion here is written so it could not pass against a fake
//!
//! The numbers are computed **inside the container** by a program this file
//! sends it — `4950` is `sum(range(100))` and appears nowhere else in this
//! crate — and the container-identity property is asserted by asking the daemon
//! what is running, not by asking egret what it did.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egret_workflow::worker::commands::{container_name, DockerRunner, TaskCommands, TaskRunner};
use egret_workflow::worker::handlers::{container, python, Level, LogSink};
use egret_workflow::worker::{Outcome, TaskHandler, TaskInput};
use uuid::Uuid;

/// The image the container tests run in. Small, has `/bin/sh`, and is not
/// Python — so a `egret.python` assertion cannot pass by accident here.
const SHELL_IMAGE: &str = "alpine:3.20";

/// `python::DEFAULT_IMAGE`, named again so the pre-flight check can tell the
/// reader which pull is missing.
const PYTHON_IMAGE: &str = python::DEFAULT_IMAGE;

/// `sum(range(100))`. It exists only if a real CPython computed it: this file
/// never writes it, and neither does any fixture in this crate.
const SUM_TO_100: &str = "4950";

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// The opt-out, and it has to be **typed into the invocation**.
pub const ALLOW_DOCKER_SKIP: &str = "EGRET_ALLOW_DOCKER_SKIP";

/// What a run with no daemon should do, as a pure function of the two facts
/// that decide it.
///
/// Pure for `egret_test_db::skip_verdict`'s reason: `std::env::set_var` is
/// `unsafe` and racy beside other threads' reads, so a test that flipped the
/// real variable would be a worse test than no test. This one is driven
/// directly, in both directions, by
/// [`the_docker_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed`].
fn docker_verdict(has_daemon: bool, allow_skip: bool) -> Result<(), String> {
    if has_daemon || allow_skip {
        return Ok(());
    }
    Err(format!(
        "No Docker daemon answered, so there is nothing to run `egret.container` \
         or `egret.python` in.\n\
         \n\
         This suite does NOT skip. `tests/sql_starrocks_live.rs` used to, and the \
         whole-branch review measured what that looked like from outside: `7 \
         passed; 0 failed; 0 ignored` in 0.00s against the identical line in \
         0.88s. These two handlers reach ZERO of the 345 corpus flows — measured \
         — so the differential oracle can never judge them and an empty green \
         here would be the whole of their evidence about the daemon.\n\
         \n\
         Pick one:\n\
         \n\
         \x20 1. Start a daemon and pull the two images this suite uses:\n\
         \x20      # macOS: Docker Desktop, or `brew install colima && colima start`\n\
         \x20      # Linux: `sudo systemctl start docker`, and be in the `docker` group\n\
         \x20      docker pull {SHELL_IMAGE}\n\
         \x20      docker pull {PYTHON_IMAGE}\n\
         \n\
         \x20 2. Deliberately skip it, visibly, in the invocation:\n\
         \x20      {ALLOW_DOCKER_SKIP}=1 cargo test -p egret-workflow --test container_docker_live"
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

/// Is `image` already local? A pull inside a test would make its duration a
/// network measurement, so the suite refuses instead and says which pull.
fn image_present(image: &str) -> bool {
    std::process::Command::new("docker")
        .args(["image", "inspect", image])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The gate, applied. Panics unless there is a daemon or somebody typed the
/// opt-out; returns `false` when the opt-out was typed, so the caller returns.
fn gate(images: &[&str]) -> bool {
    let has = daemon_answers();
    if let Err(msg) = docker_verdict(has, std::env::var_os(ALLOW_DOCKER_SKIP).is_some()) {
        panic!("{msg}");
    }
    if !has {
        eprintln!("skipping: no Docker daemon, and {ALLOW_DOCKER_SKIP} was typed");
        return false;
    }
    for image in images {
        assert!(
            image_present(image),
            "the image {image} is not on this machine. Pulling inside a test would \
             make its duration a measurement of the network, so this suite refuses \
             instead: run `docker pull {image}` and try again"
        );
    }
    true
}

macro_rules! fixture {
    ($($image:expr),+ $(,)?) => {
        if !gate(&[$($image),+]) {
            return;
        }
    };
}

/// **The gate is a real gate, in both directions.**
///
/// Runs on every machine, daemon or not: it drives the pure verdict rather than
/// the environment, so it is the one test here that cannot be affected by what
/// the machine has.
#[test]
fn the_docker_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed() {
    assert!(docker_verdict(true, false).is_ok(), "a daemon is enough");
    assert!(
        docker_verdict(true, true).is_ok(),
        "the opt-out does not turn a working daemon off"
    );
    assert!(
        docker_verdict(false, true).is_ok(),
        "a typed opt-out skips quietly, which is the operator's own choice"
    );
    let msg = docker_verdict(false, false)
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

// ---------------------------------------------------------------------------
// The assertions
// ---------------------------------------------------------------------------

/// **A container runs, and every line it printed reached the sink, on the
/// stream it was printed to.**
///
/// Log streaming is part of the contract and not an extra: a container's stdout
/// is most of what it produces, and a handler that streamed to nowhere would
/// still satisfy every assertion about its exit code.
#[tokio::test]
async fn a_container_runs_and_both_of_its_streams_reach_the_sink() {
    fixture!(SHELL_IMAGE);
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
        "stderr is WARN and stdout is INFO -- a sink that got the level from \
         nowhere would still have the text"
    );
    assert!(
        !container_exists(&container_name(&t)),
        "a finished run removes its container -- `$default$delete` is true"
    );
}

/// **The `::{…}::` marker becomes task outputs**, and an ordinary line does
/// not.
#[tokio::test]
async fn an_output_marker_printed_by_the_container_becomes_task_outputs() {
    fixture!(SHELL_IMAGE);
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
        "`TaskLogLineMatcher.LOG_DATA_SYNTAX` is `^::(\\{{.*}})::$` and the \
         ordinary line beside it must contribute nothing"
    );
}

/// **An output file comes back with its contents, and a declared file the
/// command did not produce fails the task.**
///
/// Both halves, because a collector that returned an empty map would pass the
/// first assertion of a weaker test.
#[tokio::test]
async fn an_output_file_comes_back_and_a_missing_one_fails_the_task() {
    fixture!(SHELL_IMAGE);
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["seq 1 5 | tr '\\n' ',' > counted.csv", "mkdir -p sub && echo deep > sub/inner.txt"],
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
            "commands": ["true"],
            "outputFiles": ["never-written.txt"],
        }),
    );
    let out = h
        .run(&t)
        .await
        .expect("a missing output file is the author's flow");
    assert_eq!(out.state, egret_workflow::executor::RunState::Failed);
    assert!(
        out.message
            .as_deref()
            .expect("a message")
            .contains("never-written.txt"),
        "a declared output that is absent must fail the task NAMING it, not \
         leave a key missing from the outputs map: {:?}",
        out.message
    );
}

/// **A non-zero exit fails the task on a real daemon**, and `set -e` is what
/// makes the first failing line the one that decides it.
#[tokio::test]
async fn a_non_zero_exit_fails_the_task_and_set_e_stops_at_the_first_failure() {
    fixture!(SHELL_IMAGE);
    let h = container::Container::new();
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
    assert_eq!(out.state, egret_workflow::executor::RunState::Failed);
    assert_eq!(
        out.message.as_deref(),
        Some("egret.container: the command exited with code 7")
    );
    assert_eq!(outputs(&out)["exitCode"], serde_json::json!(7));
    let texts = rec.texts();
    assert!(texts.contains(&"before".to_string()), "{texts:?}");
    assert!(
        !texts.contains(&"after".to_string()),
        "`set -e` ends the script at the first failing line: {texts:?}"
    );

    // The handler is the one under test above; this second one exists so the
    // unused binding is not a warning and the daemon is asked once more that
    // nothing was left behind.
    let _ = &h;
    assert!(!container_exists(&container_name(&t)));
}

/// **`egret.python` runs the author's program in the artifact's own default
/// image**, streams it, and its outputs are computed by CPython.
#[tokio::test]
async fn python_runs_the_authors_script_and_its_numbers_come_from_cpython() {
    fixture!(PYTHON_IMAGE);
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
/// is the one the unit tests cannot see: `worker::timeout::guard` enforces a
/// task-level `timeout:` by **dropping the future**, which is all a `Sleep`
/// needs and all `egret.sql` gets. A container outlives that drop, because the
/// daemon is holding it and not this process.
///
/// Both halves are asserted against the **daemon**, not against egret's own
/// return value.
#[tokio::test]
async fn a_dropped_future_leaves_the_container_running_and_cancel_is_what_stops_it() {
    fixture!(SHELL_IMAGE);
    let h = container::Container::new();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({"image": SHELL_IMAGE, "commands": ["sleep 120"]}),
    );
    let name = container_name(&t);

    // `timeout::guard`'s own mechanism: run it, and drop the future.
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

    // And a second cancel is not an error: `Docker.kill` catches
    // `NotFoundException` and returns without logging.
    h.cancel(&t)
        .await
        .expect("a container that is not there is nothing to stop, not a refusal");
}

/// **A reaped worker's `cancel` cannot reach its replacement's container.**
///
/// This is `TaskHandler::cancel`'s hazard note, driven against the daemon.
/// Worker B holds the run under lease `t2`; worker A — reaped, still inside its
/// handler, and about to have its own budget expire — calls `cancel` with the
/// same `run_id` and its **stale** lease `t1`. `attempt` is identical, because
/// `reap` leaves the attempt open.
///
/// **This test fails if the disambiguator is dropped.** With
/// `container_name` built from `run_id` alone the two names are equal, A's
/// `docker rm --force` reaches B's container, and the assertion that B is still
/// running is false. That is the mutation Step 4 records.
#[tokio::test]
async fn a_reaped_workers_cancel_cannot_reach_its_replacements_container() {
    fixture!(SHELL_IMAGE);
    let h = container::Container::new();

    let run_id = Uuid::new_v4();
    let spec = serde_json::json!({"image": SHELL_IMAGE, "commands": ["sleep 120"]});

    // Worker A's lease, and worker B's. Everything else is identical — the same
    // run, the same execution, the same attempt.
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

    // B starts its container and is left running.
    let running = tokio::time::timeout(std::time::Duration::from_millis(2500), h.run(&live)).await;
    assert!(running.is_err(), "B must still be inside its handler");
    let b_name = container_name(&live);
    assert!(container_running(&b_name), "B's container must be up");

    // A's budget expires. `cancel::guarded` calls this, addressed by identity.
    h.cancel(&stale)
        .await
        .expect("a stale worker's kill finds nothing, which is not a refusal");

    assert!(
        container_running(&b_name),
        "a reaped worker's cancel reached its REPLACEMENT's container. The lease \
         token is what stops that (see `TaskInput::claimed_at`), and without it \
         `egret-run-<run_id>` names whichever worker holds the run now"
    );

    force_remove(&b_name);
}

/// **An image that does not exist fails the task, and does not abort the
/// tick.**
///
/// The split matters: egret's own outage (no daemon) must leave the execution
/// alone, while the author's mistake must fail their task with the daemon's own
/// words. Only the second half is reachable here — a daemon that is not there
/// is what the gate above refuses to run without — and the first is asserted on
/// the classifier in `worker::commands`'s unit tests.
#[tokio::test]
async fn an_image_that_does_not_exist_fails_the_task_rather_than_the_worker() {
    fixture!(SHELL_IMAGE);
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
    assert_eq!(out.state, egret_workflow::executor::RunState::Failed);
    assert!(
        out.message
            .as_deref()
            .expect("a message")
            .contains("egret.container:"),
        "got {:?}",
        out.message
    );
    assert!(
        !container_exists(&container_name(&t)),
        "a create that failed leaves nothing behind"
    );
}

/// **An output file over the cap fails the task rather than being truncated.**
///
/// The cap is the only thing between a flow and an unbounded `task_runs.outputs`
/// row that every later render in the execution pays for, and until this test it
/// was a guard nobody had watched refuse. Driven through
/// [`DockerRunner::with_limits`] rather than by writing a real megabyte, which is
/// `handlers::sql::Sql::with_limits`'s own reason.
#[tokio::test]
async fn an_output_file_over_the_cap_fails_the_task_rather_than_being_truncated() {
    fixture!(SHELL_IMAGE);
    let runner = DockerRunner::new().with_limits(64, 1_048_576);
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let cmds = TaskCommands {
        container_image: SHELL_IMAGE.to_string(),
        // 100 bytes, computed by the container: over 64 and under a megabyte,
        // so this could not pass against the shipping cap either.
        commands: vec!["printf 'x%.0s' $(seq 1 100) > big.txt".into()],
        output_files: vec!["big.txt".into()],
        ..TaskCommands::default()
    };
    let err = runner
        .run(&t, &cmds, &egret_workflow::worker::handlers::TracingSink)
        .await
        .expect_err("a file over the cap must not come back truncated");
    let msg = err.to_string();
    assert!(msg.contains("100 bytes"), "the SIZE must be named: {msg}");
    assert!(msg.contains("over the 64"), "the CAP must be named: {msg}");
    assert!(msg.contains("a prefix is not an answer"), "{msg}");

    // And the same file under the cap comes back whole, so the refusal above is
    // the cap and not the copy failing.
    let runner = DockerRunner::new().with_limits(1024, 1_048_576);
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let out = runner
        .run(&t, &cmds, &egret_workflow::worker::handlers::TracingSink)
        .await
        .expect("100 bytes is under a 1024-byte cap");
    assert_eq!(out.output_files["big.txt"].len(), 100);
}

/// **A container that floods the marker channel is refused**, for the same
/// reason and on the other unbounded channel.
#[tokio::test]
async fn a_flood_of_output_markers_is_refused_rather_than_stored() {
    fixture!(SHELL_IMAGE);
    let runner = DockerRunner::new().with_limits(1_048_576, 32);
    let t = task(container::TYPE_ID, serde_json::json!({}));
    let err = runner
        .run(
            &t,
            &TaskCommands {
                container_image: SHELL_IMAGE.to_string(),
                commands: vec![
                    "echo \"::{\\\"outputs\\\":{\\\"a\\\":\\\"$(printf 'y%.0s' $(seq 1 200))\\\"}}::\"".into(),
                ],
                ..TaskCommands::default()
            },
            &egret_workflow::worker::handlers::TracingSink,
        )
        .await
        .expect_err("200 bytes of marker payload is over a 32-byte cap");
    assert!(
        err.to_string().contains("markers"),
        "the refusal must name the channel: {err}"
    );
}

/// **The runner refuses an empty image with the artifact's own sentence**, and
/// refuses it before it creates anything.
#[tokio::test]
async fn the_runner_refuses_an_empty_image_with_the_artifacts_own_message() {
    fixture!(SHELL_IMAGE);
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
            &egret_workflow::worker::handlers::TracingSink,
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

/// **A file name that climbs out of the working directory is refused before a
/// container exists**, on both channels — the one a handler writes and the one
/// it reads back.
#[tokio::test]
async fn a_path_that_escapes_the_working_directory_is_refused_by_the_runner() {
    fixture!(SHELL_IMAGE);
    let runner = DockerRunner::new();

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
            &egret_workflow::worker::handlers::TracingSink,
        )
        .await
        .expect_err("`..` must be refused");
    assert!(err.to_string().contains("climbs out"), "got {err}");

    let t2 = task(container::TYPE_ID, serde_json::json!({}));
    let err = runner
        .run(
            &t2,
            &TaskCommands {
                container_image: SHELL_IMAGE.to_string(),
                commands: vec!["true".into()],
                output_files: vec!["/etc/passwd".to_string()],
                ..TaskCommands::default()
            },
            &egret_workflow::worker::handlers::TracingSink,
        )
        .await
        .expect_err("an absolute output path must be refused");
    assert!(err.to_string().contains("absolute path"), "got {err}");

    for t in [&t, &t2] {
        assert!(
            !container_exists(&container_name(t)),
            "a refused run removes whatever it created"
        );
    }
}
