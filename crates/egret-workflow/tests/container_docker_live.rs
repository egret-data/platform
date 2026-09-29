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

use egret_workflow::logs::{LogLevel, LogLimits, LogLine, RunIdentity, RunLog};
use egret_workflow::scripts::{RunnerKind, RunnerSpec};
use egret_workflow::worker::commands::{
    container_name, DockerRunner, NoObserver, TaskCommands, TaskRunner,
};
use egret_workflow::worker::handlers::script::{FixedFeatureState, Script, ScriptRuntime};
use egret_workflow::worker::handlers::{container, python, LogSink};
use egret_workflow::worker::{Outcome, TaskHandler, TaskInput};
use uuid::Uuid;

/// The one script handler (W5a Task 5) for `type_id`, over a real
/// [`DockerRunner`], with the gate open: the feature on and every image
/// allowed. The gate itself is judged in `tests/scripts_gate.rs`; this file
/// judges the daemon.
fn handler(type_id: &str) -> Script {
    use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
    Script::new(
        egret_workflow::scripts::kind_of(type_id).expect("a script type"),
        Arc::new(ScriptRuntime {
            runner_kind: RunnerKind::Docker,
            runner: Arc::new(DockerRunner::new()),
            plugin_configurations: PluginConfigurations(vec![PluginConfiguration {
                r#type: RunnerKind::Docker.fqcn().to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["*".into(), "*/*".into(), "*/*/*".into()]),
                    allowed_types: None,
                },
            }]),
            features: Arc::new(FixedFeatureState(true)),
        }),
    )
}

/// `egret.container`, W4's spelling, now an alias of `shell.Commands`.
fn shell() -> Script {
    handler(container::TYPE_ID)
}

/// `egret.script.python`, now an alias of `python.Script`.
fn py() -> Script {
    handler(python::TYPE_ID)
}

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
    lines: Mutex<Vec<LogLine>>,
}

impl Recorder {
    fn lines(&self) -> Vec<(LogLevel, String)> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .map(|l| (l.level, l.message.clone()))
            .collect()
    }
    fn texts(&self) -> Vec<String> {
        self.lines().into_iter().map(|(_, t)| t).collect()
    }
}

#[async_trait::async_trait]
impl LogSink for Recorder {
    async fn write(&self, line: LogLine) {
        self.lines.lock().unwrap().push(line);
    }
}

/// [`task`], with its `log` recording into the returned [`Recorder`].
fn recorded(type_id: &str, spec: serde_json::Value) -> (TaskInput, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    let mut t = task(type_id, spec);
    t.log = RunLog::new(
        RunIdentity {
            workspace_id: t.workspace_id,
            execution_id: t.execution_id,
            task_run_id: t.run_id,
            task_id: t.task_id.clone(),
            attempt: RunIdentity::stored_attempt(t.attempt),
        },
        rec.clone(),
        LogLimits::default(),
    );
    (t, rec)
}

/// This process's workspace id, on every task it runs: the label the suite's
/// cleanup removes by. Since final review I3 a handler's `run` leaves its
/// container for the post-commit `release`, which a test calling `run`
/// directly does not reach.
fn suite_workspace() -> Uuid {
    static WS: std::sync::OnceLock<Uuid> = std::sync::OnceLock::new();
    *WS.get_or_init(Uuid::new_v4)
}

/// Remove every container this process created (its workspace label), and
/// nothing else on the daemon.
fn remove_this_suites_containers() {
    let out = std::process::Command::new("docker")
        .args([
            "ps",
            "-aq",
            "--filter",
            &format!("label=egret.io/workspace-id={}", suite_workspace()),
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("docker ps");
    for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
        force_remove(id);
    }
}

/// A [`TaskInput`] with a **fresh** run id, so two tests can never address
/// the same container.
fn task(type_id: &str, spec: serde_json::Value) -> TaskInput {
    TaskInput {
        workspace_id: suite_workspace(),
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
        log: RunLog::discard(),
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

fn container_id(name: &str) -> String {
    let out = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.Id}}", name])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("docker inspect");
    assert!(out.status.success(), "{name} must exist");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every container, running or not, labelled with `run`'s task-run id.
fn containers_of(run: Uuid) -> Vec<String> {
    let out = std::process::Command::new("docker")
        .args([
            "ps",
            "--all",
            "--filter",
            &format!("label=egret.io/taskrun-id={run}"),
            "--format",
            "{{.Names}}",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("docker ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn force_remove(name: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rm", "--force", name])
        .stdin(std::process::Stdio::null())
        .output();
}

/// The `taskRunner:` a flow with none gets on this runner.
fn docker_spec() -> RunnerSpec {
    RunnerSpec::defaults(RunnerKind::Docker)
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
    let h = shell();
    let (t, rec) = recorded(
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
        Some(LogLevel::Error),
        "and the stream it came from must survive the trip: stderr is ERROR (W11 Task 1)"
    );
    // Final review I3: `run` removes nothing; the worker's post-commit
    // `release` does, and only then.
    assert!(
        container_exists(&container_name(&t)),
        "the container outlives `run`, so a failed commit can re-attach to it"
    );
    h.release(&t).await.expect("released");
    assert!(
        !container_exists(&container_name(&t)),
        "release removes the container -- `$default$delete` is true"
    );
}

/// **The `::{…}::` marker becomes task outputs**, and an ordinary line does
/// not.
async fn an_output_marker_printed_by_the_container_becomes_task_outputs() {
    let h = shell();
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

/// **An output file comes back with its contents; a declared file the command
/// did not produce is IGNORED; a non-recursive glob keys what it matches** --
/// Kestra's `outputFiles`, case for case (`w5a-scripts.json`
/// `egret_expected.output_files`, W5a Task 5 fix round 1, the user's decision).
async fn output_files_follow_kestra_literal_missing_and_glob() {
    let h = shell();
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

    // The fixture's glob case: `*.csv` over a.csv, b.csv, sub/c.csv, x.txt
    // keys a.csv and b.csv; plus its missing case, ignored.
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": [
                "echo a > a.csv", "echo b > b.csv", "echo x > x.txt",
                "mkdir -p sub dir.csv && echo c > sub/c.csv",
                "echo '::{\"outputs\":{\"printed\":1}}::'",
            ],
            "outputFiles": ["*.csv", "never.txt"],
        }),
    );
    let out = h
        .run(&t)
        .await
        .expect("a missing output is never a worker failure");
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "a declared file the command did not write is ignored: {out:?}"
    );
    let o = outputs(&out);
    assert_eq!(
        o["outputFiles"],
        serde_json::json!({"a.csv": "a\n", "b.csv": "b\n"}),
        "non-recursive, files only"
    );
    assert_eq!(o["vars"], serde_json::json!({"printed": 1}));
}

/// **A DIRECTORY named as an output file fails the task, not the worker.**
///
/// Fix round 1's Critical 1, driven where it was measured: `fs::read` on a
/// directory used to give an `Error::Internal`, which the handler correctly
/// reads as egret's own outage — so one typo'd flow aborted the tick every time
/// it was claimed, for ever, blaming egret. `expect` on the `Result` is the
/// assertion: an `Err` here IS the bug.
async fn a_directory_named_as_an_output_file_does_not_wedge_the_worker() {
    let h = shell();
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
        .expect("a typo'd `outputFiles` must not wedge the run -- see Critical 1");
    let msg = failed(&out);
    assert!(msg.contains("DIRECTORY"), "{msg}");
    assert!(msg.contains("results"), "{msg}");
}

/// **A non-zero exit fails the task on a real daemon**, and `set -e` is what
/// makes the first failing line the one that decides it.
async fn a_non_zero_exit_fails_the_task_and_set_e_stops_at_the_first_failure() {
    let (t, rec) = recorded(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["echo before", "exit 7", "echo after"],
        }),
    );
    let out = shell()
        .run(&t)
        .await
        .expect("a command that failed is the author's flow");
    assert_eq!(
        failed(&out),
        "egret.container: Command failed with exit code 7"
    );
    assert_eq!(outputs(&out)["exitCode"], serde_json::json!(7));
    let texts = rec.texts();
    assert!(texts.contains(&"before".to_string()), "{texts:?}");
    assert!(
        !texts.contains(&"after".to_string()),
        "`set -e` ends the script at the first failing line: {texts:?}"
    );
    shell().release(&t).await.expect("released");
    assert!(!container_exists(&container_name(&t)));
}

/// **`egret.script.python` runs the author's program in the artifact's own
/// default image**, streams it, and its outputs are computed by CPython.
async fn python_runs_the_authors_script_and_its_numbers_come_from_cpython() {
    let h = py();
    let (t, rec) = recorded(
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
/// The one property no unit test can see: a future dropped -- by a lost fence
/// (W5a Task 3) or anything else -- leaves the container to the daemon, which
/// is what lets the next holder re-attach to it; `cancel` (a kill, or the live
/// holder's own expired budget, through `cancel::guarded`) is what stops it.
async fn a_dropped_future_leaves_the_container_running_and_cancel_is_what_stops_it() {
    let h = shell();
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

/// **A new holder of the attempt RE-ATTACHES to the old holder's container
/// instead of starting a second one** (W5a Task 5, spec §5), and the old
/// holder's dropped future removed nothing.
///
/// Worker A's handler future is dropped mid-run -- what a lost fence does
/// (W5a Task 3) -- so the container keeps running. Worker B holds the same
/// attempt under a NEW lease and runs the same task: it must find A's
/// container by name, follow its log to the end and report the outcome A's
/// container produced. `delete: false` keeps the container afterwards so the
/// daemon can be asked how many there ever were: one, with A's id.
///
/// Before W5a the name carried the lease, so B created a second container
/// while A's ran on (Task 3's measured orphan); that mutant fails the id
/// assertion here.
async fn a_new_holder_re_attaches_to_the_attempts_container_instead_of_starting_another() {
    let h = shell();
    let spec = serde_json::json!({
        "image": SHELL_IMAGE,
        "commands": [
            "echo first",
            "sleep 4",
            "echo second",
            "echo \"::{\\\"outputs\\\":{\\\"n\\\":$((6*7))}}::\"",
        ],
        "taskRunner": {
            "type": egret_workflow::scripts::DOCKER_RUNNER,
            "delete": false,
        },
    });
    let a = task(container::TYPE_ID, spec);
    let name = container_name(&a);

    let dropped = tokio::time::timeout(std::time::Duration::from_millis(2000), h.run(&a)).await;
    assert!(
        dropped.is_err(),
        "A must still have been inside its handler"
    );
    assert!(
        container_running(&name),
        "dropping A's future must not remove the container: the new holder re-attaches to it"
    );
    let a_id = container_id(&name);

    let (b, rec) = {
        let (t, rec) = recorded(container::TYPE_ID, serde_json::json!({}));
        (
            TaskInput {
                spec: a.spec.clone(),
                log: t.log,
                ..a.clone()
            },
            rec,
        )
    };
    // B is a different holder (its own log), and `TaskInput` carries no
    // lease at all: the name can only be the (run, attempt) re-attach key.
    assert_eq!(a.attempt, b.attempt, "of the same attempt");

    let out = h
        .run(&b)
        .await
        .expect("re-attaching is not a worker failure");
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{out:?}"
    );
    assert_eq!(
        outputs(&out)["vars"],
        serde_json::json!({"n": 42}),
        "B reports what A's container computed"
    );
    assert!(
        rec.texts().contains(&"second".to_string()),
        "B follows the container's log to its end: {:?}",
        rec.texts()
    );
    assert_eq!(
        containers_of(b.run_id),
        vec![name.clone()],
        "one container for the attempt, ever"
    );
    assert_eq!(container_id(&name), a_id, "and it is A's, not a new one");
    assert!(
        !container_running(&name),
        "`delete: false` keeps the container, exited -- the artifact's measured behaviour"
    );
    force_remove(&name);
}

/// **An image that does not exist fails the task, and does not abort the
/// tick.**
async fn an_image_that_does_not_exist_fails_the_task_rather_than_the_worker() {
    let h = shell();
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
            &docker_spec(),
            &NoObserver,
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

    // **Before `docker create`** since W5a Task 5: `inputFiles` keys are the
    // author's now, so `check_relative` runs at the top of `run`, before a
    // container exists. The refusal is an `Err` because nothing has RUN yet
    // and there is no exit code or var to preserve. The assertion at the end
    // of this test checks nothing was left behind.
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
            &docker_spec(),
            &NoObserver,
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
            &docker_spec(),
            &NoObserver,
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

    // The input refusal came before `docker create`: nothing exists.
    assert!(
        !container_exists(&container_name(&t)),
        "a run refused before `create` leaves nothing"
    );
    // The output refusal came after the command ran: the container stays for
    // the post-commit `release` (final review I3), which removes it.
    assert!(container_exists(&container_name(&t2)));
    runner.release(&t2, &docker_spec()).await.expect("release");
    assert!(
        !container_exists(&container_name(&t2)),
        "release removes what the refused run created"
    );
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
/// outside the working directory *within the container*, and egret returns the
/// container's file. Kestra's answer differs **per leg** — an empty file for the
/// leaf (the link's tar entry has no content for `Files.copy` to materialise),
/// and **no entry at all** for the intermediate one (`FilesService.outputFiles`
/// matches declared names with `Files.walkFileTree`, so an unmatched name
/// yields nothing). Both are inside the sandbox; neither is the host.
async fn a_symlinked_output_file_cannot_read_the_worker_hosts_files() {
    let h = shell();

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

/// **A parent and a child declared together never wedges the run.**
///
/// The whole-branch review's Critical, against a real daemon. The container
/// writes a **regular file** `a`; the author declares `["a", "a/b.txt"]`. Before
/// the fix, staging `a` left a regular file where `a/b.txt`'s parent had to go,
/// `create_dir_all` gave `AlreadyExists`, and it surfaced as
/// `egret.container: the container runner failed for a reason that is not the
/// author's flow: File exists (os error 17)` — a **worker** failure, so no outcome
/// was applied: the run stayed `RUNNING` and leased and every later claim
/// repeated it for ever. (`tick` itself carries on with the rest of the batch —
/// the phrase "aborts the tick" overstates it.)
///
/// `expect` on the `Result` **is** the assertion: an `Err` here is the bug, and
/// it is the same shape as `a_directory_named_as_an_output_file_does_not_wedge_
/// the_worker` because it is the same class — an author's `outputFiles` must
/// never become an engine failure.
async fn a_parent_and_a_child_declared_together_never_wedges_the_run() {
    let h = shell();
    let t = task(
        container::TYPE_ID,
        serde_json::json!({
            "image": SHELL_IMAGE,
            "commands": ["echo regular-file > a", "echo '::{\"outputs\":{\"ran\":1}}::'"],
            "outputFiles": ["a", "a/b.txt"],
        }),
    );
    let out = h.run(&t).await.expect(
        "an author declaring a parent and a child must never wedge the run -- a worker \
         failure applies no outcome, so it stays RUNNING and leased for an hour",
    );

    // The child cannot exist (its parent is a file), so it is a declared file
    // the command did not leave: IGNORED since W5a Task 5 fix round 1
    // (Kestra's `missing_declared_file`), and never egret's staging collision.
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{out:?}"
    );
    let o = outputs(&out);
    assert_eq!(
        o["outputFiles"],
        serde_json::json!({"a": "regular-file\n"}),
        "the parent came back; the impossible child is simply absent: {o}"
    );
    assert_eq!(o["vars"], serde_json::json!({"ran": 1}), "{o}");
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
        .run(&t, &cmds, &docker_spec(), &NoObserver)
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
        .run(&t, &cmds, &docker_spec(), &NoObserver)
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
            &docker_spec(),
            &NoObserver,
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
        case!(a_parent_and_a_child_declared_together_never_wedges_the_run),
        case!(a_path_that_escapes_the_working_directory_is_refused_by_the_runner),
        case!(a_new_holder_re_attaches_to_the_attempts_container_instead_of_starting_another),
        case!(a_symlinked_output_file_cannot_read_the_worker_hosts_files),
        case!(an_image_that_does_not_exist_fails_the_task_rather_than_the_worker),
        case!(output_files_follow_kestra_literal_missing_and_glob),
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
            remove_this_suites_containers();
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
