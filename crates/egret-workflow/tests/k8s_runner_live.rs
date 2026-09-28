//! **`KubernetesRunner` against a real cluster** (W5a Task 4) — the only
//! instrument that judges the pod lifecycle, the exec + tar transfer, the log
//! follow and the orphan sweep. The pod *spec* is judged without a cluster by
//! `worker::k8s::pod_spec`'s unit tests.
//!
//! # The gate: `EGRET_TEST_KIND=1`
//!
//! Without it every case is reported **IGNORED** (never passed), with a loud
//! line saying so — the same hand-rolled `main` as `container_docker_live`,
//! for its reason: libtest cannot report a run-time skip as ignored. With it,
//! a cluster that does not answer is a FAILURE, not a skip.
//!
//! What it needs (all local, nothing pulled at test time):
//!
//! ```bash
//! docker build -f deploy/docker/task-helper.Dockerfile -t egret-task-helper:dev deploy/docker
//! kind load docker-image egret-task-helper:dev --name egret
//! printf 'FROM alpine:3.20\n' | docker build -t egret-w5a-test-shell:dev -
//! kind load docker-image egret-w5a-test-shell:dev --name egret
//! EGRET_TEST_KIND=1 DATABASE_URL=… cargo test -p egret-workflow --test k8s_runner_live
//! ```
//!
//! # It touches ONE namespace, and deletes it
//!
//! The kind cluster `egret` is shared with the dev stack. Every object this
//! suite makes is in [`NS`], which it creates with Pod Security `restricted`
//! **enforced** — so the admission controller itself judges the pod's
//! hardening on every case — and deletes at the end, whatever happened.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_workflow::logs::{LogLevel, LogLimits, LogLine, LogRepo, RunIdentity, RunLog};
use egret_workflow::scripts::{ResourceAmounts, Resources, RunnerKind, RunnerSpec};
use egret_workflow::worker::commands::{
    CommandsResult, DockerRunner, NoObserver, OrphanSweep, TaskCommands, TaskRunner,
};
use egret_workflow::worker::handlers::LogSink;
use egret_workflow::worker::k8s::{pod_spec, sweep_orphans, KubernetesRunner, PodConfig};
use egret_workflow::worker::TaskInput;
use k8s_openapi::api::core::v1::{Namespace, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use uuid::Uuid;

mod common;

/// The only namespace this suite creates or touches.
const NS: &str = "egret-w5a-tasks-test";
/// The gate.
const GATE: &str = "EGRET_TEST_KIND";
const CONTEXT: &str = "kind-egret";
const HELPER_IMAGE: &str = "egret-task-helper:dev";
/// `alpine:3.20`, retagged so `kind load` has a single-platform image.
const SHELL_IMAGE: &str = "egret-w5a-test-shell:dev";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

async fn client() -> kube::Client {
    let opts = kube::config::KubeConfigOptions {
        context: Some(CONTEXT.to_string()),
        ..Default::default()
    };
    let cfg = kube::Config::from_kubeconfig(&opts)
        .await
        .unwrap_or_else(|e| panic!("no kubeconfig context {CONTEXT}: {e}"));
    kube::Client::try_from(cfg).expect("a kube client")
}

fn pod_config() -> PodConfig {
    PodConfig {
        namespace: NS.to_string(),
        run_as_user: 1000,
        helper_image: HELPER_IMAGE.to_string(),
        default_resources: Resources {
            request: ResourceAmounts {
                cpu: Some("50m".into()),
                memory: Some("32Mi".into()),
            },
            limit: ResourceAmounts {
                cpu: Some("500m".into()),
                memory: Some("128Mi".into()),
            },
        },
        max_resources: Resources {
            request: ResourceAmounts::default(),
            limit: ResourceAmounts {
                cpu: Some("1".into()),
                memory: Some("256Mi".into()),
            },
        },
        image_pull_secrets: vec![],
    }
}

async fn pool() -> sqlx::PgPool {
    common::db()
        .await
        .expect("the kind suite needs DATABASE_URL (the runner reads task_logs)")
        .pool
}

async fn runner() -> KubernetesRunner {
    KubernetesRunner::new(client().await, pod_config(), LogRepo::new(pool().await))
}

fn pods(c: &kube::Client) -> Api<Pod> {
    Api::namespaced(c.clone(), NS)
}

fn k8s_spec() -> RunnerSpec {
    RunnerSpec::defaults(RunnerKind::Kubernetes)
}

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

fn task_for(ws: Uuid, execution: Uuid, run: Uuid) -> (TaskInput, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    let t = TaskInput {
        workspace_id: ws,
        execution_id: execution,
        run_id: run,
        task_id: "t".to_string(),
        type_id: egret_workflow::scripts::SHELL_COMMANDS.to_string(),
        value: None,
        attempt: 1,
        spec: serde_json::Value::Null,
        render: egret_workflow::runners::RenderContext::new(),
        timeout: None,
        principal: None,
        claimed_at: chrono::Utc::now(),
        log: RunLog::new(
            RunIdentity {
                workspace_id: ws,
                execution_id: execution,
                task_run_id: run,
                task_id: "t".to_string(),
                attempt: 0,
            },
            rec.clone(),
            LogLimits::default(),
        ),
    };
    (t, rec)
}

/// A task with fresh ids and no database rows.
fn task() -> (TaskInput, Arc<Recorder>) {
    task_for(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
}

fn shell(commands: &[&str]) -> TaskCommands {
    TaskCommands {
        container_image: SHELL_IMAGE.to_string(),
        commands: commands.iter().map(|s| s.to_string()).collect(),
        ..TaskCommands::default()
    }
}

fn name_of(t: &TaskInput) -> String {
    pod_spec::pod_name(t.run_id, pod_spec::attempt_of(t))
}

/// Gone, or on its way (a deletion timestamp): either way no longer running
/// the task. Polls, because a grace-0 delete is asynchronous.
async fn wait_gone(c: &kube::Client, name: &str, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match pods(c).get_opt(name).await.expect("get pod") {
            None => return true,
            Some(p) if p.metadata.deletion_timestamp.is_some() => return true,
            Some(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_main_running(c: &kube::Client, name: &str) {
    for _ in 0..240 {
        if let Some(p) = pods(c).get_opt(name).await.expect("get pod") {
            let running = p
                .status
                .as_ref()
                .and_then(|s| s.container_statuses.as_ref())
                .into_iter()
                .flatten()
                .any(|cs| {
                    cs.name == "main" && cs.state.as_ref().is_some_and(|s| s.running.is_some())
                });
            if running {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("pod {name}'s main never started running");
}

/// Every pod UID the cluster ever showed for `run`, collected in the
/// background until the returned handle is dropped. Counting UIDs is asking
/// the API server how many pods were CREATED, not asking egret.
fn watch_uids(
    c: kube::Client,
    run: Uuid,
) -> (Arc<Mutex<BTreeSet<String>>>, tokio::task::JoinHandle<()>) {
    let seen = Arc::new(Mutex::new(BTreeSet::new()));
    let s = seen.clone();
    let h = tokio::spawn(async move {
        let lp = ListParams::default().labels(&format!("egret.io/taskrun-id={run}"));
        loop {
            if let Ok(list) = pods(&c).list(&lp).await {
                for p in list.items {
                    if let Some(uid) = p.metadata.uid {
                        s.lock().unwrap().insert(uid);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    (seen, h)
}

async fn insert_execution(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history) \
         VALUES ($1, 'a.b', 'f', 'id: f\ntasks: []\n', 'RUNNING', '[\"CREATED\",\"RUNNING\"]') \
         RETURNING id",
    )
    .bind(ws)
    .fetch_one(pool)
    .await
    .expect("insert execution")
}

async fn insert_run(pool: &sqlx::PgPool, execution: Uuid, ix: i32, state: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         VALUES ($1, $2, $2, $3, 'io.kestra.plugin.scripts.shell.Commands', $4, '[]'::jsonb) \
         RETURNING id",
    )
    .bind(execution)
    .bind(ix)
    .bind(format!("t{ix}"))
    .bind(state)
    .fetch_one(pool)
    .await
    .expect("insert task run")
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// A command runs in a pod, its lines reach the attempt's log, its marker
/// becomes vars — and the pod is gone afterwards.
async fn a_shell_command_runs_in_a_pod_and_the_pod_is_deleted() {
    let c = client().await;
    let r = runner().await;
    let (t, rec) = task();
    let (uids, watcher) = watch_uids(c.clone(), t.run_id);
    let out = r
        .run(
            &t,
            &shell(&[
                "echo \"hello from $(id -u) in $PWD\"",
                "echo to-stderr >&2",
                "echo '::{\"outputs\":{\"n\":42}}::'",
                "echo second-stderr 1>&2",
            ]),
            &k8s_spec(),
            &NoObserver,
        )
        .await
        .expect("no worker failure");
    watcher.abort();

    assert_eq!(out.exit_code, 0, "{out:?}");
    assert_eq!(out.failure, None);
    assert_eq!(out.vars.get("n"), Some(&serde_json::json!(42)));
    let lines = rec.lines();
    assert!(
        lines.contains(&(
            LogLevel::Info,
            "hello from 1000 in /egret/working-dir".to_string()
        )),
        "the line was computed in the pod, as uid 1000, in the working dir: {lines:?}"
    );
    for err in ["to-stderr", "second-stderr"] {
        assert!(
            lines.contains(&(LogLevel::Error, err.to_string())),
            "stderr is carried through files-out and logged at ERROR, as W11 measured: \
             {lines:?}"
        );
    }
    assert!(
        !lines
            .iter()
            .any(|(l, t)| *l != LogLevel::Error && t.contains("stderr")),
        "no stderr line also arrives on the stdout stream: {lines:?}"
    );
    assert_eq!(out.stdout_line_count, 2, "hello + the marker line: {out:?}");
    assert_eq!(out.stderr_line_count, 2, "{out:?}");
    assert!(
        !lines.iter().any(|(_, t)| t.contains("##egret:stderr:end")),
        "the end sentinel is not a line: {lines:?}"
    );
    assert!(
        !rec.texts().iter().any(|l| l.contains("::{")),
        "the marker is not stored"
    );
    assert_eq!(uids.lock().unwrap().len(), 1, "exactly one pod was created");
    assert!(
        wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await,
        "`delete: true` (the default) removes the pod when the run returns"
    );
}

/// A non-shell interpreter cannot have egret's redirect prepended: its stderr
/// stays merged (INFO, counted as stdout), and one WARN line says so.
async fn a_non_shell_interpreter_keeps_the_merged_stream_and_says_so() {
    let r = runner().await;
    let (t, rec) = task();
    let mut cmds = shell(&["echo out-line", "echo err-line >&2"]);
    // busybox's sh, reached through a program that is not a known shell name.
    cmds.interpreter = vec!["/bin/busybox".into(), "sh".into(), "-c".into()];
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!(out.exit_code, 0, "{out:?}");
    let lines = rec.lines();
    assert!(
        lines.contains(&(LogLevel::Info, "out-line".to_string())),
        "{lines:?}"
    );
    assert!(
        lines.contains(&(LogLevel::Info, "err-line".to_string())),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|(l, t)| *l == LogLevel::Warn && t.contains("not a shell")),
        "{lines:?}"
    );
    assert_eq!((out.stdout_line_count, out.stderr_line_count), (2, 0));
}

/// Inline input files arrive in the working dir by exec + tar; declared output
/// files come back by exec + tar.
async fn input_and_output_files_round_trip() {
    let c = client().await;
    let r = runner().await;
    let (t, _rec) = task();
    let mut cmds = shell(&[
        "cat in/data.txt > out.txt",
        "printf %s \"$WORKING_DIR\" > wd.txt",
        "wc -c < b.txt | tr -d ' ' > n.txt",
    ]);
    cmds.input_files = BTreeMap::from([
        ("in/data.txt".to_string(), "abc\nline two\n".to_string()),
        ("b.txt".to_string(), "x".repeat(700)),
    ]);
    cmds.output_files = vec!["out.txt".into(), "wd.txt".into(), "n.txt".into()];
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!(out.exit_code, 0, "{out:?}");
    assert_eq!(out.failure, None, "{out:?}");
    assert_eq!(
        out.output_files,
        BTreeMap::from([
            ("out.txt".to_string(), "abc\nline two\n".to_string()),
            ("wd.txt".to_string(), "/egret/working-dir".to_string()),
            ("n.txt".to_string(), "700\n".to_string()),
        ])
    );
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);

    // A declared file the command did not write fails the task, by name.
    let (t, _rec) = task();
    let mut cmds = shell(&["echo x > there.txt"]);
    cmds.output_files = vec!["there.txt".into(), "missing.txt".into()];
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!(
        out.output_files.get("there.txt").map(String::as_str),
        Some("x\n")
    );
    let why = out.failure.expect("a missing output fails the task");
    assert!(
        why.contains("missing.txt") && why.contains("did not leave"),
        "{why}"
    );
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);
}

/// A non-zero exit is the exit code, with the vars and the files it left.
async fn a_nonzero_exit_fails_and_keeps_outputs() {
    let c = client().await;
    let r = runner().await;
    let (t, _rec) = task();
    let mut cmds = shell(&[
        "echo '::{\"outputs\":{\"k\":\"v\"}}::'",
        "echo partial > o.txt",
        "exit 3",
    ]);
    cmds.output_files = vec!["o.txt".into()];
    let out: CommandsResult = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("a non-zero exit is not a worker failure");
    assert_eq!(out.exit_code, 3);
    assert_eq!(out.vars.get("k"), Some(&serde_json::json!("v")));
    assert_eq!(
        out.output_files.get("o.txt").map(String::as_str),
        Some("partial\n")
    );
    assert_eq!(out.failure, None);
    assert!(
        wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await,
        "deleted on failure too"
    );
}

/// `cancel` deletes the pod (grace 0), and the `run` in flight returns.
async fn cancel_deletes_the_pod() {
    let c = client().await;
    let r = Arc::new(runner().await);
    let (t, _rec) = task();
    let name = name_of(&t);
    let (r2, t2) = (r.clone(), t.clone());
    let running = tokio::spawn(async move {
        r2.run(&t2, &shell(&["sleep 300"]), &k8s_spec(), &NoObserver)
            .await
    });
    wait_main_running(&c, &name).await;
    let started = std::time::Instant::now();
    r.cancel(&t).await.expect("cancel");
    assert!(
        wait_gone(&c, &name, Duration::from_secs(20)).await,
        "cancel deletes the pod"
    );
    let ended = tokio::time::timeout(Duration::from_secs(30), running)
        .await
        .expect("the run in flight returns once its pod is gone")
        .expect("join");
    assert!(
        !matches!(&ended, Ok(o) if o.exit_code == 0),
        "a cancelled `sleep 300` did not succeed: {ended:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(60));
    // A second cancel of a pod that is not there is Ok.
    r.cancel(&t).await.expect("cancelling nothing is Ok");
}

/// The next holder of a lease re-attaches to the pod its predecessor left —
/// one pod ever — and follows the log from the last stored line.
async fn a_second_run_call_for_the_same_attempt_reattaches_not_recreates() {
    let c = client().await;
    let pool = pool().await;
    let (ws, _user) = common::seed_workspace(&common::db().await.unwrap()).await;
    let execution = insert_execution(&pool, ws).await;
    let run = insert_run(&pool, execution, 0, "RUNNING").await;
    let r = runner().await;
    let cmds = shell(&["echo one", "sleep 8", "echo two"]);

    let (uids, watcher) = watch_uids(c.clone(), run);
    let (t1, rec1) = task_for(ws, execution, run);
    let name = name_of(&t1);
    {
        // The first holder: dropped mid-run, exactly as a lost fence drops it.
        let spec = k8s_spec();
        let first = r.run(&t1, &cmds, &spec, &NoObserver);
        tokio::pin!(first);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::select! {
                res = &mut first => panic!("the first call finished early: {res:?}"),
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
            if rec1.texts().iter().any(|l| l == "one") {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "never saw `one`");
        }
        // Past the second boundary of `one`, so the floored sinceTime is after it.
        tokio::time::sleep(Duration::from_millis(2100)).await;
    } // <- dropped
    assert!(
        pods(&c)
            .get_opt(&name)
            .await
            .unwrap()
            .is_some_and(|p| p.metadata.deletion_timestamp.is_none()),
        "dropping the run future must NOT delete the pod"
    );
    // What the first holder's log writer had stored for this attempt.
    sqlx::query(
        "INSERT INTO workflow.task_logs \
           (workspace_id, execution_id, task_run_id, task_id, attempt, level, ts, message) \
         VALUES ($1, $2, $3, 't', 0, 2, now(), 'one')",
    )
    .bind(ws)
    .bind(execution)
    .bind(run)
    .execute(&pool)
    .await
    .expect("store the first holder's line");

    let (t2, rec2) = task_for(ws, execution, run);
    let out = r
        .run(&t2, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("the second holder finishes the run");
    watcher.abort();
    assert_eq!(out.exit_code, 0);
    let texts = rec2.texts();
    assert!(texts.contains(&"two".to_string()), "{texts:?}");
    assert!(
        !texts.contains(&"one".to_string()),
        "sinceTime = the stored line's ts: `one` is not replayed: {texts:?}"
    );
    assert_eq!(
        uids.lock().unwrap().len(),
        1,
        "ONE pod ever: the second call re-attached rather than recreated"
    );
    assert!(wait_gone(&c, &name, Duration::from_secs(20)).await);
}

/// A line written on the node just before the reclaim, and never stored, is
/// not lost — even when the previous holder's stored `ts` (egret's clock) is
/// past the next second boundary. The overlap that WAS stored is not repeated.
async fn a_line_written_just_before_the_reclaim_is_not_lost() {
    let c = client().await;
    let pool = pool().await;
    let (ws, _user) = common::seed_workspace(&common::db().await.unwrap()).await;
    let execution = insert_execution(&pool, ws).await;
    let run = insert_run(&pool, execution, 0, "RUNNING").await;
    let r = runner().await;
    let cmds = shell(&["echo one", "sleep 3", "echo almost", "sleep 5", "echo two"]);
    let (t1, rec1) = task_for(ws, execution, run);
    let name = name_of(&t1);
    {
        let spec = k8s_spec();
        let first = r.run(&t1, &cmds, &spec, &NoObserver);
        tokio::pin!(first);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::select! {
                res = &mut first => panic!("the first call finished early: {res:?}"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
            if rec1.texts().iter().any(|l| l == "one") {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "never saw `one`");
        }
    } // <- dropped right after `one`, before `almost` is written
      // `almost` is written (~3 s after `one`) while nobody is attached.
    tokio::time::sleep(Duration::from_millis(3600)).await;
    // The previous holder stored `one` with egret's clock 1.5 s AHEAD of the
    // node: ⌊that⌋ is after `almost` was written, which is exactly the
    // window a margin-less sinceTime loses.
    sqlx::query(
        "INSERT INTO workflow.task_logs \
           (workspace_id, execution_id, task_run_id, task_id, attempt, level, ts, message) \
         VALUES ($1, $2, $3, 't', 0, 2, now() + interval '1.5 seconds', 'one')",
    )
    .bind(ws)
    .bind(execution)
    .bind(run)
    .execute(&pool)
    .await
    .expect("store the first holder's line");

    let (t2, rec2) = task_for(ws, execution, run);
    let out = r
        .run(&t2, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("the second holder finishes the run");
    assert_eq!(out.exit_code, 0);
    let texts = rec2.texts();
    assert!(
        texts.contains(&"almost".to_string()),
        "the unstored line before the reclaim is replayed: {texts:?}"
    );
    assert!(texts.contains(&"two".to_string()), "{texts:?}");
    assert!(
        !texts.contains(&"one".to_string()),
        "the stored overlap is dropped by exact text: {texts:?}"
    );
    assert!(wait_gone(&c, &name, Duration::from_secs(20)).await);
}

/// The sweep deletes pods of terminal runs only: a live run's pod, a
/// `delete: false` pod, and a pod whose run this database does not know
/// (another egret's, or another test suite's) all survive.
async fn the_orphan_sweep_deletes_pods_of_terminal_runs_only() {
    let c = client().await;
    let pool = pool().await;
    let (ws, _user) = common::seed_workspace(&common::db().await.unwrap()).await;
    let execution = insert_execution(&pool, ws).await;
    let done = insert_run(&pool, execution, 0, "SUCCESS").await;
    let live = insert_run(&pool, execution, 1, "RUNNING").await;
    let kept = insert_run(&pool, execution, 2, "FAILED").await;
    let rowless = Uuid::new_v4();
    let cfg = pod_config();

    let mut names = BTreeMap::new();
    for (run, delete) in [(done, true), (live, true), (kept, false), (rowless, true)] {
        let (t, _) = task_for(ws, execution, run);
        let mut spec = k8s_spec();
        spec.delete = delete;
        let pod = pod_spec::build(&t, &shell(&["sleep 300"]), &spec, &cfg).unwrap();
        pods(&c)
            .create(&PostParams::default(), &pod)
            .await
            .expect("create");
        names.insert(run, name_of(&t));
    }
    let listed = pods(&c).list(&ListParams::default()).await.unwrap();
    let standing = listed
        .items
        .iter()
        .filter(|p| p.metadata.deletion_timestamp.is_none())
        .count();
    assert_eq!(standing, 4, "the namespace holds only this case's pods");

    let n = sweep_orphans(&c, &cfg, &pool).await.expect("sweep");
    assert_eq!(n, 1, "only the terminal run's pod");
    assert!(wait_gone(&c, &names[&done], Duration::from_secs(20)).await);
    for run in [live, kept, rowless] {
        let p = pods(&c).get_opt(&names[&run]).await.unwrap();
        assert!(
            p.is_some_and(|p| p.metadata.deletion_timestamp.is_none()),
            "{} must survive the sweep",
            names[&run]
        );
    }
    for run in [live, kept, rowless] {
        let _ = pods(&c)
            .delete(&names[&run], &DeleteParams::default().grace_period(0))
            .await;
    }
}

/// The Docker half behind the same interface: a container of a terminal run is
/// removed; one of a live run, and one whose run this database does not know,
/// are left.
async fn the_docker_orphan_sweep_removes_containers_of_terminal_runs_only() {
    let pool = pool().await;
    let (ws, _user) = common::seed_workspace(&common::db().await.unwrap()).await;
    let execution = insert_execution(&pool, ws).await;
    let done = insert_run(&pool, execution, 0, "SUCCESS").await;
    let live = insert_run(&pool, execution, 1, "RUNNING").await;
    let foreign = Uuid::new_v4();
    let mut names = BTreeMap::new();
    for run in [done, live, foreign] {
        let name = format!("egret-run-{}-1", run.simple());
        let out = std::process::Command::new("docker")
            .args([
                "create",
                "--name",
                &name,
                "--label",
                &format!("egret.io/taskrun-id={run}"),
                "alpine:3.20",
                "sleep",
                "300",
            ])
            .output()
            .expect("docker create");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        names.insert(run, name);
    }
    let exists = |n: &str| {
        std::process::Command::new("docker")
            .args(["inspect", "--type", "container", n])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let n = DockerRunner::new()
        .sweep_orphans(&pool)
        .await
        .expect("sweep");
    let (d, l, f) = (
        exists(&names[&done]),
        exists(&names[&live]),
        exists(&names[&foreign]),
    );
    for name in names.values() {
        let _ = std::process::Command::new("docker")
            .args(["rm", "--force", name])
            .output();
    }
    assert_eq!(n, 1, "only the terminal run's container");
    assert!(!d, "the terminal run's container is removed");
    assert!(l, "a live run's container is left");
    assert!(
        f,
        "a container this database does not know is left (shared daemon)"
    );
}

// ---------------------------------------------------------------------------
// The namespace, and the harness
// ---------------------------------------------------------------------------

async fn namespaces() -> Api<Namespace> {
    Api::all(client().await)
}

async fn create_namespace() {
    let api = namespaces().await;
    // A namespace left terminating by an earlier run must finish first.
    for _ in 0..240 {
        match api.get_opt(NS).await.expect("read namespace") {
            None => break,
            Some(n) if n.metadata.deletion_timestamp.is_some() => {
                tokio::time::sleep(Duration::from_millis(500)).await
            }
            Some(_) => panic!(
                "namespace {NS} already exists and is not terminating: a previous run did not \
                 clean up. Inspect it, then `kubectl --context {CONTEXT} delete ns {NS}`"
            ),
        }
    }
    let ns = Namespace {
        metadata: ObjectMeta {
            name: Some(NS.to_string()),
            labels: Some(BTreeMap::from([
                (
                    "pod-security.kubernetes.io/enforce".to_string(),
                    "restricted".to_string(),
                ),
                (
                    "pod-security.kubernetes.io/enforce-version".to_string(),
                    "latest".to_string(),
                ),
                (
                    "app.kubernetes.io/managed-by".to_string(),
                    "egret-w5a-task4-tests".to_string(),
                ),
            ])),
            ..Default::default()
        },
        ..Default::default()
    };
    api.create(&PostParams::default(), &ns)
        .await
        .expect("create namespace");
    // The default ServiceAccount is created asynchronously; a pod needs it.
    let sa: Api<k8s_openapi::api::core::v1::ServiceAccount> = Api::namespaced(client().await, NS);
    for _ in 0..120 {
        if sa.get_opt("default").await.ok().flatten().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("the default service account never appeared in {NS}");
}

async fn delete_namespace() -> bool {
    let api = namespaces().await;
    let _ = api.delete(NS, &DeleteParams::default()).await;
    for _ in 0..480 {
        if api.get_opt(NS).await.ok().flatten().is_none() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

type Case = fn() -> Pin<Box<dyn Future<Output = ()> + Send>>;

macro_rules! case {
    ($f:ident) => {
        (stringify!($f), (|| Box::pin($f())) as Case)
    };
}

fn cases() -> Vec<(&'static str, Case)> {
    vec![
        case!(a_shell_command_runs_in_a_pod_and_the_pod_is_deleted),
        case!(input_and_output_files_round_trip),
        case!(a_nonzero_exit_fails_and_keeps_outputs),
        case!(cancel_deletes_the_pod),
        case!(a_second_run_call_for_the_same_attempt_reattaches_not_recreates),
        case!(a_line_written_just_before_the_reclaim_is_not_lost),
        case!(a_non_shell_interpreter_keeps_the_merged_stream_and_says_so),
        case!(the_orphan_sweep_deletes_pods_of_terminal_runs_only),
        case!(the_docker_orphan_sweep_removes_containers_of_terminal_runs_only),
    ]
}

fn main() {
    // `egret-bin`'s `main` does the same: the tree enables two providers.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let matches = |name: &str| filter.as_ref().is_none_or(|f| name.contains(f.as_str()));
    let all = cases();
    let selected: Vec<&(&str, Case)> = all.iter().filter(|(n, _)| matches(n)).collect();
    let filtered_out = all.len() - selected.len();
    let started = std::time::Instant::now();
    println!("\nrunning {} tests", selected.len());

    let (mut passed, mut ignored) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();

    if std::env::var(GATE).as_deref() != Ok("1") {
        eprintln!(
            "\n*** SKIPPING the Kubernetes runner's live suite: {GATE}=1 is not set. ***\n\
             *** {} tests did NOT run and are reported IGNORED, not passed.       ***\n\
             See this file's doc for the images to load into the kind cluster.\n",
            selected.len()
        );
        for (name, _) in &selected {
            println!("test {name} ... ignored");
            ignored += 1;
        }
    } else if !selected.is_empty() {
        let rt = tokio::runtime::Runtime::new().expect("a tokio runtime");
        rt.block_on(create_namespace());
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
        if rt.block_on(delete_namespace()) {
            eprintln!("namespace {NS} deleted");
        } else {
            eprintln!("namespace {NS} did NOT finish deleting");
            failures.push("namespace cleanup".to_string());
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
