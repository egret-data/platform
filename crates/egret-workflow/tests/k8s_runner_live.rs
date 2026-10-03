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
//! `EGRET_TEST_KIND_CONTEXT=kind-<name>` runs it against another cluster
//! (a temporary one) instead of `kind-egret`; load the two images there.
//! `a_tar_x_that_dies_in_the_pod_fails_the_task` also needs
//! `egret-w9a-t5-badtar:dev`: `egret-task-helper:dev` plus a
//! `/usr/local/bin/tar` that, for `-x`, reads 4 KiB, prints
//! `tar: write error: No space left on device` and exits 1.
//!
//! # It touches ONE namespace, and deletes it
//!
//! The kind cluster `egret` is shared with the dev stack. Every object this
//! suite makes is in [`NS`], which it creates with Pod Security `restricted`
//! **enforced** — so the admission controller itself judges the pod's
//! hardening on every case — and deletes at the end, whatever happened.
//!
//! # The runner runs as the chart's Role, not as the admin
//!
//! Every [`KubernetesRunner`] and [`sweep_orphans`] call here goes through
//! [`runner_client`], which **impersonates** the ServiceAccount [`RUNNER_SA`]
//! in [`NS`]. That account is bound to a Role whose rules are **parsed from
//! the chart's `templates/rbac-tasks.yaml`** ([`chart_role_rules`]), so the
//! suite runs the runner with exactly the permissions a deployment grants.
//! Until W5a Task 6 fix round 1 the runner used the admin kubeconfig, and the
//! chart's `pods/exec` `create`-only Role — a 403 on every exec on
//! Kubernetes 1.33, where kube-rs's WebSocket exec is authorised as `get` —
//! passed this suite and failed only on a real install. The admin client
//! ([`client`]) is kept for what the test itself does: create the namespace
//! and the RBAC, and observe or plant pods.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_workflow::logs::{LogLevel, LogLimits, LogLine, LogRepo, RunIdentity, RunLog};
use egret_workflow::scripts::{ResourceAmounts, Resources, RunnerKind, RunnerSpec};
use egret_workflow::worker::commands::{
    CommandsResult, DockerRunner, NoObserver, OrphanSweep, StoredInput, TaskCommands, TaskRunner,
};
use egret_workflow::worker::handlers::LogSink;
use egret_workflow::worker::k8s::{pod_spec, sweep_orphans, KubernetesRunner, PodConfig};
use egret_workflow::worker::TaskInput;
use k8s_openapi::api::core::v1::{Namespace, Pod, ServiceAccount};
use k8s_openapi::api::rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use uuid::Uuid;

mod common;

/// The only namespace this suite creates or touches.
const NS: &str = "egret-w5a-tasks-test";
/// The gate.
const GATE: &str = "EGRET_TEST_KIND";
/// The kubeconfig context, `kind-egret` unless [`CONTEXT_VAR`] names another
/// -- a temporary cluster, so a run need not touch the shared one.
const CONTEXT: &str = "kind-egret";
const CONTEXT_VAR: &str = "EGRET_TEST_KIND_CONTEXT";

fn context() -> String {
    std::env::var(CONTEXT_VAR).unwrap_or_else(|_| CONTEXT.to_string())
}
const HELPER_IMAGE: &str = "egret-task-helper:dev";
/// `alpine:3.20`, retagged so `kind load` has a single-platform image.
const SHELL_IMAGE: &str = "egret-w5a-test-shell:dev";
/// The ServiceAccount the runner impersonates, bound to the chart's Role.
const RUNNER_SA: &str = "egret-task-runner-test";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

async fn client() -> kube::Client {
    let opts = kube::config::KubeConfigOptions {
        context: Some(context()),
        ..Default::default()
    };
    let cfg = kube::Config::from_kubeconfig(&opts)
        .await
        .unwrap_or_else(|e| panic!("no kubeconfig context {}: {e}", context()));
    kube::Client::try_from(cfg).expect("a kube client")
}

/// The admin client with `Impersonate-User` set to [`RUNNER_SA`]: every
/// request is authorised as that account, so as the chart's Role.
async fn runner_client() -> kube::Client {
    let opts = kube::config::KubeConfigOptions {
        context: Some(context()),
        ..Default::default()
    };
    let mut cfg = kube::Config::from_kubeconfig(&opts)
        .await
        .unwrap_or_else(|e| panic!("no kubeconfig context {}: {e}", context()));
    cfg.auth_info.impersonate = Some(format!("system:serviceaccount:{NS}:{RUNNER_SA}"));
    kube::Client::try_from(cfg).expect("an impersonating kube client")
}

/// The task Role's rules, read from the chart itself: the lines from
/// `rules:` to the document's end in `templates/rbac-tasks.yaml` (they hold
/// no template expression), parsed as Kubernetes `PolicyRule`s.
fn chart_role_rules() -> Vec<PolicyRule> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/helm/egret/templates/rbac-tasks.yaml"
    );
    let text = std::fs::read_to_string(path).expect("read the chart's rbac-tasks.yaml");
    let start = text.find("\nrules:\n").expect("the Role has `rules:`") + 1;
    let end = start + text[start..].find("\n---").expect("the Role ends at `---`");
    #[derive(serde::Deserialize)]
    struct Rules {
        rules: Vec<PolicyRule>,
    }
    let rules: Rules = serde_norway::from_str(&text[start..end]).expect("the rules parse");
    assert!(
        rules.rules.iter().any(|r| r
            .resources
            .as_deref()
            .unwrap_or_default()
            .contains(&"pods/exec".to_string())),
        "the chart's Role must be what was read: {:?}",
        rules.rules
    );
    rules.rules
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
                ..Default::default()
            },
            limit: ResourceAmounts {
                cpu: Some("500m".into()),
                memory: Some("128Mi".into()),
                ephemeral_storage: Some("1Gi".into()),
            },
        },
        max_resources: Resources {
            request: ResourceAmounts::default(),
            limit: ResourceAmounts {
                cpu: Some("1".into()),
                memory: Some("256Mi".into()),
                ..Default::default()
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
    KubernetesRunner::new(
        runner_client().await,
        pod_config(),
        LogRepo::new(pool().await),
    )
    .with_files(suite_files())
}

/// This process's internal storage (W9a Task 6): a `MemStore`; the transfer
/// is what is judged here.
fn suite_files() -> Arc<egret_workflow::storage::FileStore> {
    static FILES: std::sync::OnceLock<Arc<egret_workflow::storage::FileStore>> =
        std::sync::OnceLock::new();
    FILES
        .get_or_init(|| {
            Arc::new(egret_workflow::storage::FileStore {
                store: Arc::new(egret_storage::MemStore::new()),
                bucket: "egret-test".into(),
                prefix: "workflow/".into(),
                caps: egret_workflow::storage::Caps::default(),
                retention_days: 30,
            })
        })
        .clone()
}

/// `cmds` collecting into `t`'s own task prefix.
fn collecting(t: &TaskInput, mut cmds: TaskCommands) -> TaskCommands {
    cmds.collect_to = Some(egret_workflow::worker::commands::CollectTo {
        workspace: t.workspace_id,
        task_prefix: egret_workflow::storage::uri::task_prefix(
            "w9a.t6.k8s",
            "live",
            t.execution_id,
            &t.task_id,
            t.run_id,
        )
        .unwrap(),
        naming: egret_workflow::worker::commands::Naming::KestraOutputName,
    });
    cmds
}

/// What the store holds for each collected file, by its relative path.
async fn contents(
    files: &egret_workflow::storage::FileStore,
    t: &TaskInput,
    out: &CommandsResult,
) -> BTreeMap<String, String> {
    let mut got = BTreeMap::new();
    for (rel, f) in &out.output_files {
        let bytes = files
            .store
            .get(&files.object_uri(t.workspace_id, &f.uri).unwrap())
            .await
            .unwrap_or_else(|e| panic!("{} is in the store: {e}", f.uri));
        got.insert(rel.clone(), String::from_utf8(bytes).unwrap());
    }
    got
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
        pods(&c)
            .get_opt(&name_of(&t))
            .await
            .unwrap()
            .is_some_and(|p| p.metadata.deletion_timestamp.is_none()),
        "the pod outlives `run` (final review I3): a commit that fails after this point \
         leaves it for the next claim to re-attach to"
    );
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(
        wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await,
        "`delete: true` (the default): release removes the pod"
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
    r.release(&t, &k8s_spec()).await.expect("release");
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
    let cmds = collecting(&t, cmds);
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!(out.exit_code, 0, "{out:?}");
    assert_eq!(out.failure, None, "{out:?}");
    assert_eq!(
        contents(&suite_files(), &t, &out).await,
        BTreeMap::from([
            ("out.txt".to_string(), "abc\nline two\n".to_string()),
            ("wd.txt".to_string(), "/egret/working-dir".to_string()),
            ("n.txt".to_string(), "700\n".to_string()),
        ])
    );
    // Final review I3: `run` deletes nothing; `release` (after the commit) does.
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);

    // **Kestra's `outputFiles`, in a pod** (W5a Task 5 fix round 1): a
    // declared file the command did not write is IGNORED, and a
    // non-recursive glob keys each file it matches in its directory only
    // (`w5a-scripts.json` `egret_expected.output_files`).
    let (t, _rec) = task();
    let mut cmds = shell(&[
        "echo x > there.txt",
        "echo a > a.csv",
        "echo b > b.csv",
        "mkdir -p sub d.csv && echo c > sub/c.csv",
    ]);
    cmds.output_files = vec!["there.txt".into(), "missing.txt".into(), "*.csv".into()];
    let cmds = collecting(&t, cmds);
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!(out.failure, None, "a missing output is ignored: {out:?}");
    assert_eq!(
        contents(&suite_files(), &t, &out).await,
        BTreeMap::from([
            ("a.csv".to_string(), "a\n".to_string()),
            ("b.csv".to_string(), "b\n".to_string()),
            ("there.txt".to_string(), "x\n".to_string()),
        ]),
        "`*.csv` is non-recursive and skips the directory `d.csv`"
    );
    // Final review I3: `run` deletes nothing; `release` (after the commit) does.
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);
}

/// **W9a Task 5: a 12 MiB file in internal storage streams into the pod**
/// through the exec's `tar -x`, fed by `write_tar` as it reads the store, and
/// passes Kestra's `wc -c` verification; the command counts it in `main`.
async fn a_stored_input_file_streams_into_the_pod() {
    const SIZE: usize = 12 << 20;
    let c = client().await;
    let files = Arc::new(egret_workflow::storage::FileStore {
        store: Arc::new(egret_storage::MemStore::new()),
        bucket: "egret-test".into(),
        prefix: "workflow/".into(),
        caps: egret_workflow::storage::Caps::default(),
        retention_days: 30,
    });
    let r = runner().await.with_files(files.clone());
    let (t, _rec) = task();
    let uri = format!(
        "kestra:///w9a/t5/in-files/executions/{}/tasks/w/{}/big.bin",
        Uuid::new_v4(),
        Uuid::new_v4()
    );
    let object_uri = files.object_uri(t.workspace_id, &uri).unwrap();
    files
        .store
        .put(
            &object_uri,
            "0123456789abcdef".repeat(SIZE / 16).into_bytes(),
        )
        .await
        .unwrap();
    let mut cmds = shell(&[
        "wc -c < d/big.bin | tr -d ' ' > n.txt",
        "tr -d '0-9a-f' < d/big.bin | wc -c | tr -d ' ' > foreign.txt",
        "head -c 16 d/big.bin > head.txt",
        "cp literal.txt lit.txt",
        // Fix round 1, I1: real mtimes (W5a's pack wrote 0 here too).
        "stat -c %Y d/big.bin > mtime.txt",
    ]);
    cmds.input_files = BTreeMap::from([("literal.txt".to_string(), "just contents".to_string())]);
    cmds.stored_inputs = BTreeMap::from([(
        "d/big.bin".to_string(),
        StoredInput {
            uri,
            object_uri,
            size: SIZE as u64,
        },
    )]);
    cmds.output_files = vec![
        "n.txt".into(),
        "foreign.txt".into(),
        "head.txt".into(),
        "lit.txt".into(),
        "mtime.txt".into(),
    ];
    let cmds = collecting(&t, cmds);
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!(out.exit_code, 0, "{out:?}");
    assert_eq!(out.failure, None, "{out:?}");
    let mut files_out = contents(&files, &t, &out).await;
    let mtime: u64 = files_out
        .get("mtime.txt")
        .and_then(|m| m.trim().parse().ok())
        .unwrap_or_else(|| panic!("{out:?}"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        mtime >= 315_532_800 && mtime <= now + 60 && mtime + 3600 >= now,
        "the staged file carries the upload's time, not 1970: {mtime}"
    );
    files_out.remove("mtime.txt");
    assert_eq!(
        files_out,
        BTreeMap::from([
            ("n.txt".to_string(), "12582912\n".to_string()),
            ("foreign.txt".to_string(), "0\n".to_string()),
            ("head.txt".to_string(), "0123456789abcdef".to_string()),
            ("lit.txt".to_string(), "just contents".to_string()),
        ])
    );
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);
}

/// The helper image whose `tar -x` reads 4 KiB and dies saying the disk is
/// full (fix round 1, I2): `FROM egret-task-helper:dev` plus a
/// `/usr/local/bin/tar` shim, built and loaded by hand (see the report).
const BAD_TAR_HELPER: &str = "egret-w9a-t5-badtar:dev";

/// **Fix round 1, I2: a `tar -x` that dies in the pod is the TASK's
/// failure**, naming tar's words -- not an outage that re-uploads the same
/// archive for ever with no attempt spent.
async fn a_tar_x_that_dies_in_the_pod_fails_the_task() {
    const SIZE: usize = 12 << 20;
    let c = client().await;
    let files = Arc::new(egret_workflow::storage::FileStore {
        store: Arc::new(egret_storage::MemStore::new()),
        bucket: "egret-test".into(),
        prefix: "workflow/".into(),
        caps: egret_workflow::storage::Caps::default(),
        retention_days: 30,
    });
    let mut cfg = pod_config();
    cfg.helper_image = BAD_TAR_HELPER.to_string();
    let r = KubernetesRunner::new(runner_client().await, cfg, LogRepo::new(pool().await))
        .with_files(files.clone());
    let (t, _rec) = task();
    let uri = format!(
        "kestra:///w9a/t5/in-files/executions/{}/tasks/w/{}/big.bin",
        Uuid::new_v4(),
        Uuid::new_v4()
    );
    let object_uri = files.object_uri(t.workspace_id, &uri).unwrap();
    files
        .store
        .put(&object_uri, vec![b'x'; SIZE])
        .await
        .unwrap();
    let mut cmds = shell(&["echo never"]);
    cmds.stored_inputs = BTreeMap::from([(
        "big.bin".to_string(),
        StoredInput {
            uri,
            object_uri,
            size: SIZE as u64,
        },
    )]);
    let started = std::time::Instant::now();
    let got = tokio::time::timeout(
        Duration::from_secs(120),
        r.run(&t, &cmds, &k8s_spec(), &NoObserver),
    )
    .await
    .expect("a dead tar ends the run promptly");
    match &got {
        Err(egret_core::Error::InvalidInput(m)) => assert!(
            m.contains("No space left on device") && m.contains("tar -x"),
            "{m}"
        ),
        other => panic!("the task's failure expected, not {other:?}"),
    }
    eprintln!("extract failure in {:?}: {got:?}", started.elapsed());
    remove_pod(&c, &name_of(&t)).await;
}

/// **Fix round 1, M2: when OUR side stops part-way the exec is aborted**: an
/// object shorter than its index row is the author's failure, at once --
/// without the abort, `tar -x` waits for the rest of the archive until the
/// idle limit and the run becomes an outage.
async fn a_short_stored_object_aborts_the_upload_and_fails_the_task() {
    let c = client().await;
    let files = Arc::new(egret_workflow::storage::FileStore {
        store: Arc::new(egret_storage::MemStore::new()),
        bucket: "egret-test".into(),
        prefix: "workflow/".into(),
        caps: egret_workflow::storage::Caps::default(),
        retention_days: 30,
    });
    let r = runner().await.with_files(files.clone());
    let (t, _rec) = task();
    let uri = format!(
        "kestra:///w9a/t5/in-files/executions/{}/tasks/w/{}/short.bin",
        Uuid::new_v4(),
        Uuid::new_v4()
    );
    let object_uri = files.object_uri(t.workspace_id, &uri).unwrap();
    files
        .store
        .put(&object_uri, vec![b'x'; 1000])
        .await
        .unwrap();
    let mut cmds = shell(&["echo never"]);
    cmds.stored_inputs = BTreeMap::from([(
        "short.bin".to_string(),
        StoredInput {
            uri: uri.clone(),
            object_uri,
            size: 5000,
        },
    )]);
    let started = std::time::Instant::now();
    let got = tokio::time::timeout(
        Duration::from_secs(60),
        r.run(&t, &cmds, &k8s_spec(), &NoObserver),
    )
    .await
    .expect("the aborted upload ends well inside the 120 s idle limit");
    match &got {
        Err(egret_core::Error::InvalidInput(m)) => assert!(m.contains(&uri), "{m}"),
        other => panic!("the author's failure expected, not {other:?}"),
    }
    eprintln!("short object failed in {:?}", started.elapsed());
    remove_pod(&c, &name_of(&t)).await;
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
    let cmds = collecting(&t, cmds);
    let out: CommandsResult = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("a non-zero exit is not a worker failure");
    assert_eq!(out.exit_code, 3);
    assert_eq!(out.vars.get("k"), Some(&serde_json::json!("v")));
    assert_eq!(
        contents(&suite_files(), &t, &out)
            .await
            .get("o.txt")
            .map(String::as_str),
        Some("partial\n")
    );
    assert_eq!(out.failure, None);
    // Final review I3: `run` deletes nothing; `release` (after the commit) does.
    r.release(&t, &k8s_spec()).await.expect("release");
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
    r.cancel(&t, Duration::ZERO).await.expect("cancel");
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
    r.cancel(&t, Duration::ZERO)
        .await
        .expect("cancelling nothing is Ok");
}

/// **W5b Task 5: `cancel` with a grace deletes the pod with that grace and
/// returns only once `main` has terminated; `release` then removes it.**
///
/// `main`'s PID 1 traps SIGTERM and takes 3 s to exit. The grace is 30 s, so
/// a `cancel` back in under 25 s was ended by `main` stopping and not by the
/// grace running out; one back in under 3 s did not wait. A plugin pod's
/// `files-out` stops on SIGTERM (`pod_spec::files_out_script`, gated on
/// `host_protocol`), which is what lets the pod go
/// when `main` does: the kubelet reports no container terminated until all
/// have stopped (measured, see that function).
async fn k8s_cancel_with_grace_waits_for_termination_then_release_deletes() {
    let c = client().await;
    let r = Arc::new(runner().await);
    let (t, rec) = task();
    let name = name_of(&t);
    let (r2, t2) = (r.clone(), t.clone());
    let running = tokio::spawn(async move {
        // `host_protocol: true`: a plugin pod, whose `files-out` exits on
        // SIGTERM. A script pod's does not (W5a's pod, byte for byte).
        let cmds = TaskCommands {
            host_protocol: true,
            ..shell(&[
                "trap 'sleep 3; echo slow-term; sleep 1; exit 0' TERM",
                "echo ready",
                "while :; do sleep 1; done",
            ])
        };
        r2.run(&t2, &cmds, &k8s_spec(), &NoObserver).await
    });
    wait_main_running(&c, &name).await;
    let mut ready = false;
    for _ in 0..240 {
        if rec.texts().iter().any(|l| l == "ready") {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(ready, "main never printed `ready`: {:?}", rec.lines());

    let started = std::time::Instant::now();
    r.cancel(&t, Duration::from_secs(30))
        .await
        .expect("cancel with a grace");
    let took = started.elapsed();
    eprintln!("graceful cancel returned in {took:?}");
    assert!(
        took >= Duration::from_millis(2900),
        "cancel returned before main's 3 s trap had finished: {took:?}"
    );
    assert!(
        took < Duration::from_secs(25),
        "main exited at ~3 s; cancel must not sit out the grace: {took:?}"
    );
    if let Some(p) = pods(&c).get_opt(&name).await.expect("get pod") {
        let main_exit = p
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .into_iter()
            .flatten()
            .find(|cs| cs.name == "main")
            .and_then(|cs| cs.state.as_ref())
            .and_then(|s| s.terminated.as_ref())
            .map(|t| t.exit_code);
        assert_eq!(
            main_exit,
            Some(0),
            "when cancel returned the pod was gone, or main had terminated through its \
             trap's `exit 0`"
        );
    }

    let ended = tokio::time::timeout(Duration::from_secs(60), running)
        .await
        .expect("the run in flight returns once main terminated")
        .expect("join");
    assert!(
        rec.texts().iter().any(|l| l == "slow-term"),
        "SIGTERM reached main's trap: {:?} / {ended:?}",
        rec.lines()
    );

    let released = std::time::Instant::now();
    r.release(&t, &k8s_spec()).await.expect("release");
    let mut gone = false;
    while released.elapsed() < Duration::from_secs(15) {
        if pods(&c).get_opt(&name).await.expect("get pod").is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        gone,
        "after release the pod is gone well before its 30 s grace would have ended"
    );
    r.cancel(&t, Duration::from_secs(30))
        .await
        .expect("cancelling a pod that is not there is Ok, with a grace too");
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
    // Final review I3: `run` deletes nothing; `release` (after the commit) does.
    r.release(&t2, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name, Duration::from_secs(20)).await);
}

/// **W9a Task 6, in a pod**: `outputFiles` are streamed out of `files-out`
/// by `tar -c` straight into the store -- a `*.csv` glob, `sub/x.csv`, a
/// 12 MiB file (byte-identical by a SHA-256 computed in `main`) and
/// `outputDirectory: true` (`$OUTPUT_DIR`, keyed by relative path, stored
/// verbatim). A declared symlink is refused by name and never followed.
async fn output_files_are_streamed_out_of_the_pod_byte_identical() {
    use sha2::Digest;
    let c = client().await;
    let r = runner().await;
    let (t, rec) = task();
    let mut cmds = shell(&[
        "echo a > a.csv",
        "echo b > b.csv",
        "mkdir -p sub && printf 'x,y\\n1,2\\n' > sub/x.csv",
        "head -c 12582912 /dev/urandom > big.bin",
        "echo \"SHA=$(sha256sum big.bin | cut -d' ' -f1)\"",
        "echo \"OUTPUT_DIR=$OUTPUT_DIR\"",
        "printf 'in outdir' > \"$OUTPUT_DIR/od.txt\"",
        "mkdir -p \"$OUTPUT_DIR/deep\" && printf deep > \"$OUTPUT_DIR/deep/d.txt\"",
    ]);
    cmds.output_files = vec!["*.csv".into(), "sub/x.csv".into(), "big.bin".into()];
    cmds.output_directory = true;
    let cmds = collecting(&t, cmds);
    let prefix = cmds.collect_to.clone().unwrap().task_prefix;
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("no worker failure");
    assert_eq!((out.exit_code, out.failure.clone()), (0, None), "{out:?}");
    let texts = rec.texts();
    assert!(
        texts.contains(&"OUTPUT_DIR=/egret/working-dir/.egret/out".to_string()),
        "{texts:?}"
    );
    let sha = texts
        .iter()
        .find_map(|l| l.strip_prefix("SHA="))
        .expect("main printed the SHA")
        .to_string();
    let uris: BTreeMap<&str, &str> = out
        .output_files
        .iter()
        .map(|(k, v)| (k.as_str(), v.uri.as_str()))
        .collect();
    assert_eq!(
        uris.keys().copied().collect::<Vec<_>>(),
        [
            "a.csv",
            "b.csv",
            "big.bin",
            "deep/d.txt",
            "od.txt",
            "sub/x.csv"
        ]
    );
    assert_eq!(
        uris["sub/x.csv"],
        format!("{prefix}10ehIGUrkxwg9tBZYZJQ9K-x.csv")
    );
    assert_eq!(uris["deep/d.txt"], format!("{prefix}deep/d.txt"));
    let files = suite_files();
    let big = files
        .store
        .get(&files.object_uri(t.workspace_id, uris["big.bin"]).unwrap())
        .await
        .unwrap();
    assert_eq!(big.len(), 12 << 20);
    let got: String = sha2::Sha256::digest(&big)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(got, sha, "byte-identical to what main wrote");
    // big.bin is random bytes, judged by its SHA above; `contents` reads text.
    let mut text_only = out.clone();
    text_only.output_files.remove("big.bin");
    let small = contents(&suite_files(), &t, &text_only).await;
    assert_eq!(
        small,
        BTreeMap::from([
            ("a.csv".to_string(), "a\n".to_string()),
            ("b.csv".to_string(), "b\n".to_string()),
            ("deep/d.txt".to_string(), "deep".to_string()),
            ("od.txt".to_string(), "in outdir".to_string()),
            ("sub/x.csv".to_string(), "x,y\n1,2\n".to_string()),
        ])
    );
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);

    // A declared symlink: refused by name, the target never read.
    let (t, _rec) = task();
    let mut cmds = shell(&["ln -s /etc/passwd leaked.txt"]);
    cmds.output_files = vec!["leaked.txt".into()];
    let cmds = collecting(&t, cmds);
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("a link is the author's flow");
    let why = out.failure.clone().expect("refused");
    assert!(
        why.contains("SYMLINK") && why.contains("leaked.txt"),
        "{why}"
    );
    assert!(out.output_files.is_empty());
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);
}

/// An object store that answers nothing (W9a Task 6 fix round 1, I3).
struct Down;

#[async_trait::async_trait]
impl egret_storage::ObjectStore for Down {
    async fn put(&self, _: &str, _: Vec<u8>) -> Result<(), egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3("connection refused".into()))
    }
    async fn get(&self, _: &str) -> Result<Vec<u8>, egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3("connection refused".into()))
    }
    async fn delete(&self, _: &str) -> Result<(), egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3("connection refused".into()))
    }
    async fn copy(&self, _: &str, _: &str) -> Result<(), egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3("connection refused".into()))
    }
    async fn list(
        &self,
        _: &str,
        _: Option<&str>,
    ) -> Result<egret_storage::Listing, egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3("connection refused".into()))
    }
    async fn put_stream(
        &self,
        uri: &str,
        _: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        _: Option<u64>,
    ) -> Result<u64, egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3(format!(
            "{uri}: connection refused"
        )))
    }
}

/// **Fix round 1, I3: the store unreachable while the pod's files are
/// collected is egret's outage** (spec §11) -- both `download` arms, a
/// declared file and the output directory, answer `Error::Internal` naming
/// the URI, never `result.failure` (which would spend an attempt).
async fn an_unreachable_store_while_collecting_is_a_worker_error() {
    let c = client().await;
    let down = Arc::new(egret_workflow::storage::FileStore {
        store: Arc::new(Down),
        bucket: "egret-test".into(),
        prefix: "workflow/".into(),
        caps: egret_workflow::storage::Caps::default(),
        retention_days: 30,
    });
    let r = KubernetesRunner::new(
        runner_client().await,
        pod_config(),
        LogRepo::new(pool().await),
    )
    .with_files(down);
    for (declared, want) in [(true, "a.txt"), (false, "od.txt")] {
        let (t, _rec) = task();
        let mut cmds = shell(&["echo a > a.txt", "printf od > \"$OUTPUT_DIR/od.txt\""]);
        if declared {
            cmds.output_files = vec!["a.txt".into()];
        } else {
            cmds.output_directory = true;
        }
        let cmds = collecting(&t, cmds);
        let err = r
            .run(&t, &cmds, &k8s_spec(), &NoObserver)
            .await
            .expect_err("the store is egret's outage, not the task's failure");
        assert!(
            matches!(err, egret_core::Error::Internal(_)),
            "{declared}: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("kestra:///") && msg.contains(want), "{msg}");
        r.release(&t, &k8s_spec()).await.expect("release");
        assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);
    }
}

/// **A re-attached pod is collected under the same URIs** (spec §5
/// "Re-attach"): the first holder is dropped while `main` runs, the second
/// re-attaches and collects, a third re-attaches to the finished pod and
/// collects again -- the same names and sizes, the same object count.
async fn a_reattached_pod_is_collected_under_the_same_uris() {
    let c = client().await;
    let r = runner().await;
    let (t, rec) = task();
    let mut cmds = shell(&[
        "echo started",
        "sleep 6",
        "echo one > one.txt",
        "printf two > \"$OUTPUT_DIR/two.txt\"",
    ]);
    cmds.output_files = vec!["one.txt".into()];
    cmds.output_directory = true;
    let cmds = collecting(&t, cmds);
    {
        let spec = k8s_spec();
        let first = r.run(&t, &cmds, &spec, &NoObserver);
        tokio::pin!(first);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            tokio::select! {
                res = &mut first => panic!("the first call finished early: {res:?}"),
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
            if rec.texts().iter().any(|l| l == "started") {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "never saw `started`"
            );
        }
    } // <- dropped
    let b = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("the second holder re-attaches");
    let c2 = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("the third re-attaches to the finished pod");
    assert_eq!((b.failure.clone(), c2.failure.clone()), (None, None));
    assert_eq!(b.output_files, c2.output_files);
    assert_eq!(
        b.output_files
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["one.txt", "two.txt"]
    );
    let prefix = &cmds.collect_to.as_ref().unwrap().task_prefix;
    let files = suite_files();
    let under = files
        .object_uri(t.workspace_id, &format!("{prefix}x"))
        .unwrap();
    let key_prefix = under
        .strip_prefix("s3://egret-test/")
        .unwrap()
        .strip_suffix('x')
        .unwrap()
        .to_string();
    assert_eq!(
        files
            .store
            .list(&key_prefix, None)
            .await
            .unwrap()
            .objects
            .len(),
        2,
        "the third holder overwrote the second's objects"
    );
    r.release(&t, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name_of(&t), Duration::from_secs(20)).await);
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
    // Final review I3: `run` deletes nothing; `release` (after the commit) does.
    r.release(&t2, &k8s_spec()).await.expect("release");
    assert!(wait_gone(&c, &name, Duration::from_secs(20)).await);
}

/// A sink that STORES each line immediately (as a flushed `PgLogSink` batch
/// would) and records it for the assertions.
#[derive(Debug)]
struct StoringRecorder {
    repo: LogRepo,
    rec: Arc<Recorder>,
}

#[async_trait::async_trait]
impl LogSink for StoringRecorder {
    async fn write(&self, line: LogLine) {
        self.repo
            .insert_batch(std::slice::from_ref(&line))
            .await
            .expect("store the line");
        self.rec.write(line).await;
    }
}

/// Fix round 2: with a non-shell interpreter egret writes a WARN line of its
/// own. On a re-attach it must not be stored before the re-attach window is
/// read, or `sinceTime` becomes this holder's own "now" − margin and every
/// line written while nobody was attached is lost.
async fn a_non_shell_reattach_keeps_the_lines_written_before_the_reclaim() {
    let c = client().await;
    let pool = pool().await;
    let (ws, _user) = common::seed_workspace(&common::db().await.unwrap()).await;
    let execution = insert_execution(&pool, ws).await;
    let run = insert_run(&pool, execution, 0, "RUNNING").await;
    let r = runner().await;
    let mut cmds = shell(&["echo one", "sleep 1", "echo mid", "sleep 9", "echo two"]);
    cmds.interpreter = vec!["/bin/busybox".into(), "sh".into(), "-c".into()];
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
    } // <- dropped right after `one`
      // What the first holder had stored.
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
    // Reclaimed well over the margin later: `mid` is > 5 s old by then.
    tokio::time::sleep(Duration::from_secs(7)).await;

    let (mut t2, rec2) = task_for(ws, execution, run);
    t2.log = RunLog::new(
        t2.log.identity().clone(),
        Arc::new(StoringRecorder {
            repo: LogRepo::new(pool.clone()),
            rec: rec2.clone(),
        }),
        LogLimits::default(),
    );
    let out = r
        .run(&t2, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("the second holder finishes the run");
    assert_eq!(out.exit_code, 0);
    let texts = rec2.texts();
    assert!(
        texts.contains(&"mid".to_string()),
        "the line written while nobody was attached is replayed: {texts:?}"
    );
    assert!(texts.contains(&"two".to_string()), "{texts:?}");
    assert!(!texts.contains(&"one".to_string()), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("not a shell")), "{texts:?}");
    // Final review I3: `run` deletes nothing; `release` (after the commit) does.
    r.release(&t2, &k8s_spec()).await.expect("release");
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

    let n = sweep_orphans(&runner_client().await, &cfg, &pool)
        .await
        .expect("sweep");
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
                 clean up. Inspect it, then `kubectl --context {} delete ns {NS}`",
                context()
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
    let mut ready = false;
    for _ in 0..120 {
        if sa.get_opt("default").await.ok().flatten().is_some() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(ready, "the default service account never appeared in {NS}");
    create_runner_identity().await;
}

/// [`RUNNER_SA`], the chart's Role ([`chart_role_rules`]) and the binding,
/// in [`NS`] -- then proof that [`runner_client`] really is that account:
/// it may list pods in [`NS`] and may NOT list them in `default` (the admin
/// can do both, so a client that ignored the impersonation passes neither
/// half of this check).
async fn create_runner_identity() {
    let c = client().await;
    let pp = PostParams::default();
    let meta = |name: &str| ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some(NS.to_string()),
        ..Default::default()
    };
    Api::<ServiceAccount>::namespaced(c.clone(), NS)
        .create(
            &pp,
            &ServiceAccount {
                metadata: meta(RUNNER_SA),
                ..Default::default()
            },
        )
        .await
        .expect("create the runner's service account");
    Api::<Role>::namespaced(c.clone(), NS)
        .create(
            &pp,
            &Role {
                metadata: meta(RUNNER_SA),
                rules: Some(chart_role_rules()),
            },
        )
        .await
        .expect("create the chart's Role");
    Api::<RoleBinding>::namespaced(c.clone(), NS)
        .create(
            &pp,
            &RoleBinding {
                metadata: meta(RUNNER_SA),
                role_ref: RoleRef {
                    api_group: "rbac.authorization.k8s.io".into(),
                    kind: "Role".into(),
                    name: RUNNER_SA.into(),
                },
                subjects: Some(vec![Subject {
                    kind: "ServiceAccount".into(),
                    name: RUNNER_SA.into(),
                    namespace: Some(NS.into()),
                    ..Default::default()
                }]),
            },
        )
        .await
        .expect("bind the Role");
    let rc = runner_client().await;
    let mut allowed = false;
    for _ in 0..40 {
        if Api::<Pod>::namespaced(rc.clone(), NS)
            .list(&ListParams::default())
            .await
            .is_ok()
        {
            allowed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(allowed, "the runner's identity cannot list pods in {NS}");
    let elsewhere = Api::<Pod>::namespaced(rc, "default")
        .list(&ListParams::default())
        .await;
    assert!(
        matches!(&elsewhere, Err(kube::Error::Api(e)) if e.code == 403),
        "the runner's client must be the Role's account, not the admin: \
         listing pods in `default` gave {elsewhere:?}"
    );
}

/// A ServiceAccount in [`NS`] bound to a Role of exactly `rules`, and a
/// client impersonating it -- for a case that needs a DIFFERENT Role than the
/// chart's (a regression the operator could make).
async fn client_with_role(sa: &str, rules: Vec<PolicyRule>) -> kube::Client {
    let c = client().await;
    let pp = PostParams::default();
    let meta = |name: &str| ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some(NS.to_string()),
        ..Default::default()
    };
    Api::<ServiceAccount>::namespaced(c.clone(), NS)
        .create(
            &pp,
            &ServiceAccount {
                metadata: meta(sa),
                ..Default::default()
            },
        )
        .await
        .expect("create the service account");
    Api::<Role>::namespaced(c.clone(), NS)
        .create(
            &pp,
            &Role {
                metadata: meta(sa),
                rules: Some(rules),
            },
        )
        .await
        .expect("create the Role");
    Api::<RoleBinding>::namespaced(c.clone(), NS)
        .create(
            &pp,
            &RoleBinding {
                metadata: meta(sa),
                role_ref: RoleRef {
                    api_group: "rbac.authorization.k8s.io".into(),
                    kind: "Role".into(),
                    name: sa.into(),
                },
                subjects: Some(vec![Subject {
                    kind: "ServiceAccount".into(),
                    name: sa.into(),
                    namespace: Some(NS.into()),
                    ..Default::default()
                }]),
            },
        )
        .await
        .expect("bind the Role");
    let opts = kube::config::KubeConfigOptions {
        context: Some(context()),
        ..Default::default()
    };
    let mut cfg = kube::Config::from_kubeconfig(&opts)
        .await
        .unwrap_or_else(|e| panic!("no kubeconfig context {}: {e}", context()));
    cfg.auth_info.impersonate = Some(format!("system:serviceaccount:{NS}:{sa}"));
    let rc = kube::Client::try_from(cfg).expect("an impersonating kube client");
    for _ in 0..40 {
        if Api::<Pod>::namespaced(rc.clone(), NS)
            .list(&ListParams::default())
            .await
            .is_ok()
        {
            return rc;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{sa} never became able to list pods in {NS}");
}

/// Delete `name` as the admin and wait until it is gone.
async fn remove_pod(c: &kube::Client, name: &str) {
    let _ = pods(c)
        .delete(name, &DeleteParams::default().grace_period(0))
        .await;
    assert!(wait_gone(c, name, Duration::from_secs(30)).await, "{name}");
}

/// **A Role without `pods/exec` `get` FAILS the task, naming the missing
/// verb, in bounded time** (final review I4). Measured in W5a Task 6: with
/// `create` only, every exec on Kubernetes 1.33 is a 403 (WebSocket exec is
/// authorised as `get`). Before this fix a task with files re-attached for
/// ever, and one without files waited 15 s and succeeded with its stderr
/// stream unterminated; both now fail with the reason. The Role here is the
/// chart's minus that one verb; the control is every other case in this file,
/// which runs as the chart's full Role.
async fn an_exec_the_role_refuses_fails_the_task_naming_the_missing_verb() {
    let c = client().await;
    let rules: Vec<PolicyRule> = chart_role_rules()
        .into_iter()
        .map(|mut r| {
            if r.resources
                .as_deref()
                .unwrap_or_default()
                .contains(&"pods/exec".to_string())
            {
                r.verbs.retain(|v| v != "get");
            }
            r
        })
        .collect();
    assert!(
        rules.iter().any(|r| r
            .resources
            .as_deref()
            .unwrap_or_default()
            .contains(&"pods/exec".to_string())
            && r.verbs == vec!["create".to_string()]),
        "the Role under test is the chart's minus `pods/exec` `get`: {rules:?}"
    );
    let rc = client_with_role("egret-task-runner-noget", rules).await;
    let r = KubernetesRunner::new(rc, pod_config(), LogRepo::new(pool().await));

    let no_files = shell(&["echo hi", "echo oops >&2"]);
    let mut with_files = shell(&["cat in.txt > out.txt"]);
    with_files.input_files = BTreeMap::from([("in.txt".to_string(), "x".to_string())]);
    with_files.output_files = vec!["out.txt".into()];
    with_files.collect_to = Some(egret_workflow::worker::commands::CollectTo {
        workspace: Uuid::nil(),
        task_prefix: egret_workflow::storage::uri::task_prefix(
            "w9a.t6.k8s",
            "live",
            Uuid::nil(),
            "t",
            Uuid::nil(),
        )
        .unwrap(),
        naming: egret_workflow::worker::commands::Naming::KestraOutputName,
    });
    for (what, cmds) in [
        ("no files", no_files),
        ("input and output files", with_files),
    ] {
        let (t, _rec) = task();
        let started = std::time::Instant::now();
        let got = tokio::time::timeout(
            Duration::from_secs(120),
            r.run(&t, &cmds, &k8s_spec(), &NoObserver),
        )
        .await
        .unwrap_or_else(|_| panic!("{what}: the run must END, not re-attach for ever"));
        match &got {
            Err(egret_core::Error::InvalidInput(m)) => {
                assert!(
                    m.contains("`pods/exec` `get`") && m.contains(NS),
                    "{what}: the reason names the verb and the namespace: {m}"
                );
            }
            other => panic!("{what}: a task failure (not Internal, not Ok) expected: {other:?}"),
        }
        eprintln!("{what}: failed with the reason in {:?}", started.elapsed());
        remove_pod(&c, &name_of(&t)).await;
    }
}

/// **A helper image that cannot be pulled FAILS the task, naming egret's
/// setting, in bounded time** (final review I4). The chart's default
/// `egret-task-helper:dev` is not in any registry; before this fix a task
/// with files held its pod and re-attached every `working_timeout` for ever.
async fn an_unpullable_helper_image_fails_the_task_naming_the_setting() {
    let c = client().await;
    let mut cfg = pod_config();
    cfg.helper_image = "egret.invalid/egret-task-helper:no-such-tag".into();
    let r = KubernetesRunner::new(runner_client().await, cfg, LogRepo::new(pool().await));
    let (t, _rec) = task();
    let mut cmds = shell(&["cat in.txt"]);
    cmds.input_files = BTreeMap::from([("in.txt".to_string(), "x".to_string())]);
    let started = std::time::Instant::now();
    let got = tokio::time::timeout(
        Duration::from_secs(180),
        r.run(&t, &cmds, &k8s_spec(), &NoObserver),
    )
    .await
    .expect("the run must END, not re-attach for ever");
    match &got {
        Err(egret_core::Error::InvalidInput(m)) => {
            assert!(
                m.contains("workflow.kubernetes.helper_image")
                    && m.contains("egret.invalid/egret-task-helper:no-such-tag"),
                "{m}"
            );
        }
        other => panic!("a task failure naming the setting expected: {other:?}"),
    }
    eprintln!("helper pull failure reported in {:?}", started.elapsed());
    remove_pod(&c, &name_of(&t)).await;
}

/// **A task that writes past its ephemeral-storage limit is evicted, and the
/// task FAILS with the kubelet's reason** (final review I5) -- instead of
/// filling the node. The limit here is 16 MiB (the config's, so the working
/// directory's `sizeLimit` too); the task writes 64 MiB and then waits.
async fn a_task_that_fills_its_disk_is_evicted_and_fails_saying_so() {
    let c = client().await;
    let mut cfg = pod_config();
    cfg.default_resources.request.ephemeral_storage = Some("8Mi".into());
    cfg.default_resources.limit.ephemeral_storage = Some("16Mi".into());
    let r = KubernetesRunner::new(runner_client().await, cfg, LogRepo::new(pool().await));
    let (t, _rec) = task();
    let cmds = shell(&[
        "dd if=/dev/zero of=big bs=1048576 count=64 2>/dev/null || true",
        "echo wrote",
        "sleep 240",
    ]);
    let started = std::time::Instant::now();
    let got = tokio::time::timeout(
        Duration::from_secs(200),
        r.run(&t, &cmds, &k8s_spec(), &NoObserver),
    )
    .await
    .expect("the eviction must end the run well before `sleep 240` would");
    let why = match &got {
        Ok(out) => out.failure.clone().unwrap_or_else(|| panic!("{out:?}")),
        Err(egret_core::Error::InvalidInput(m)) => m.clone(),
        Err(e) => panic!("a task failure expected, not a worker failure: {e:?}"),
    };
    assert!(
        why.to_lowercase().contains("evicted") && why.contains("exceeds"),
        "the kubelet's own reason reaches the task: {why}"
    );
    eprintln!("evicted in {:?}: {why}", started.elapsed());
    remove_pod(&c, &name_of(&t)).await;
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

// ---------------------------------------------------------------------------
// W9a Task 9 fix round 1: the plugin storage bridge in a pod
// ---------------------------------------------------------------------------

/// The plugin host image (W5b), loaded into the cluster like the helper.
const PLUGIN_HOST_IMAGE: &str = "egret-plugin-host:dev";
const PG_IMAGE: &str =
    "postgres:18@sha256:4ef4dbc939d61acea57712655ddb4b4ab27419c913f94cca0cd57cb3ea3c2280";

fn docker_out(args: &[&str]) -> (bool, String) {
    let o = std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("docker on PATH");
    let mut text = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() {
        text.push_str(&String::from_utf8_lossy(&o.stderr));
    }
    (o.status.success(), text)
}

/// A throwaway Postgres on the kind clusters' Docker network, removed on drop.
struct KindPg {
    name: String,
    ip: String,
}

impl Drop for KindPg {
    fn drop(&mut self) {
        let _ = docker_out(&["rm", "--force", &self.name]);
    }
}

impl KindPg {
    async fn start() -> Self {
        let name = format!("egret-w9a-t9-k8s-pg-{}", Uuid::new_v4().simple());
        let (ok, out) = docker_out(&[
            "run",
            "-d",
            "--network",
            "kind",
            "--name",
            &name,
            "-e",
            "POSTGRES_USER=w9a",
            "-e",
            "POSTGRES_DB=w9a",
            "-e",
            "POSTGRES_PASSWORD=w9a-k8s",
            PG_IMAGE,
        ]);
        assert!(ok, "docker run postgres: {out}");
        let (ok, ip) = docker_out(&[
            "inspect",
            "-f",
            "{{.NetworkSettings.Networks.kind.IPAddress}}",
            &name,
        ]);
        assert!(ok && !ip.is_empty(), "the container's kind IP: {ip}");
        let pg = KindPg { name, ip };
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut ready = 0;
        while ready < 2 {
            if pg.psql_try("SELECT 1").0 {
                ready += 1;
                tokio::time::sleep(Duration::from_secs(2)).await;
            } else {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Postgres did not come up"
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        pg
    }

    fn psql_try(&self, sql: &str) -> (bool, String) {
        docker_out(&[
            "exec", &self.name, "psql", "-U", "w9a", "-d", "w9a", "-tAc", sql,
        ])
    }

    fn psql(&self, sql: &str) -> String {
        let (ok, out) = self.psql_try(sql);
        assert!(ok, "psql {sql}: {out}");
        out
    }

    fn url(&self) -> String {
        format!("jdbc:postgresql://{}:5432/w9a", self.ip)
    }
}

struct NoSecrets;

#[async_trait::async_trait]
impl egret_workflow::runners::secrets::SecretSource for NoSecrets {
    async fn get(
        &self,
        _: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, egret_workflow::runners::secrets::SecretError> {
        Err(egret_workflow::runners::secrets::SecretError::NotFound { name: name.into() })
    }
}

/// The production registry's plugin handlers over the cluster's runner, with
/// the suite's internal storage.
async fn plugin_handlers() -> egret_workflow::worker::Handlers {
    let mut h = egret_workflow::worker::Handlers::new();
    egret_workflow::worker::handlers::kestra::register(
        &mut h,
        Arc::new(egret_workflow::worker::handlers::kestra::KestraRuntime {
            runner: Arc::new(runner().await),
            runner_kind: RunnerKind::Kubernetes,
            features: Arc::new(egret_workflow::worker::handlers::script::FixedFeatureState(
                true,
            )),
            plugin_configurations: Default::default(),
            host: egret_config::PluginHostConfig {
                image: Some(PLUGIN_HOST_IMAGE.into()),
                ..Default::default()
            },
            secrets: Arc::new(NoSecrets),
            files: Some(suite_files()),
        }),
    );
    h
}

/// A plugin task run of `type_id`, in flow `w9a.t9.k8s`/`live`, whose snapshot
/// resolves `readable`.
fn plugin_task(
    ws: Uuid,
    execution: Uuid,
    type_id: &str,
    spec: serde_json::Value,
    readable: Vec<egret_workflow::storage::IndexedFile>,
) -> TaskInput {
    let run = Uuid::new_v4();
    let (mut t, _) = task_for(ws, execution, run);
    t.type_id = type_id.to_string();
    t.task_id = "load".into();
    t.spec = spec;
    let prefix =
        egret_workflow::storage::uri::task_prefix("w9a.t9.k8s", "live", execution, "load", run)
            .unwrap();
    let snapshot = egret_workflow::storage::scope::Snapshot::from_parts(
        readable
            .into_iter()
            .map(|f| (f.uri.clone(), Ok(f)))
            .collect(),
        suite_files(),
        ws,
        Some(prefix),
    );
    let mut cx = egret_workflow::runners::RenderContext::new().with_files(snapshot.into_scope());
    cx.insert(
        "flow",
        serde_json::json!({"namespace": "w9a.t9.k8s", "id": "live"}),
    );
    t.render = cx;
    t
}

/// **In a pod, as `run_as_user` (1000), not the image's `USER 65532`**: the
/// plugin host's `.egret/tmp` and `.egret/out` -- extracted by `files-in`'s
/// `tar -x` as a non-root user -- are directories that user can write.
async fn the_plugin_host_scratch_and_output_dirs_are_writable_as_run_as_user() {
    let (t, rec) = task();
    let mut cmds = shell(&[
        "id -u",
        "stat -c '%a %u %n' .egret/tmp .egret/out",
        "touch .egret/tmp/probe && echo tmp-writable",
        "touch .egret/out/probe && echo out-writable",
    ]);
    cmds.host_protocol = true;
    cmds.output_directory = true;
    let cmds = egret_workflow::worker::commands::TaskCommands {
        collect_to: Some(egret_workflow::worker::commands::CollectTo {
            workspace: t.workspace_id,
            task_prefix: egret_workflow::storage::uri::task_prefix(
                "w9a.t9.k8s",
                "live",
                t.execution_id,
                &t.task_id,
                t.run_id,
            )
            .unwrap(),
            naming: egret_workflow::worker::commands::Naming::UnderPrefixVerbatim,
        }),
        ..cmds
    };
    let r = runner().await;
    let out = r
        .run(&t, &cmds, &k8s_spec(), &NoObserver)
        .await
        .expect("ran");
    let texts = rec.texts();
    println!("    PROBE: exit {} {texts:?}", out.exit_code);
    assert_eq!(out.exit_code, 0, "{texts:?}");
    assert!(
        texts.contains(&"1000".to_string()),
        "run as run_as_user: {texts:?}"
    );
    assert!(texts.contains(&"tmp-writable".to_string()), "{texts:?}");
    assert!(texts.contains(&"out-writable".to_string()), "{texts:?}");
    assert!(
        out.output_files.contains_key("probe"),
        "the probe was collected"
    );
    r.release(&t, &k8s_spec()).await.expect("released");
}

/// **The plugin storage chain in a pod**: `postgresql.Query fetchType:
/// STORE` writes through the host's `EgretStorage` (scratch in
/// `.egret/tmp`, the file in `.egret/out`), the runner collects it; then
/// `postgresql.Batch` reads that URI, staged into the pod, into a table.
async fn a_plugin_stores_and_reads_storage_in_a_pod() {
    let pg = KindPg::start().await;
    pg.psql(
        "CREATE TABLE src (i int, t text); INSERT INTO src VALUES (20, 'p'), (21, 'q'), (22, 'r'); \
         CREATE TABLE dst (i int, t text)",
    );
    let h = plugin_handlers().await;
    let (ws, exec) = (Uuid::new_v4(), Uuid::new_v4());
    let conn = serde_json::json!({"url": pg.url(), "username": "w9a", "password": "w9a-k8s"});
    let with = |extra: serde_json::Value| {
        let mut m = conn.as_object().unwrap().clone();
        m.extend(extra.as_object().unwrap().clone());
        serde_json::Value::Object(m)
    };

    let query = "kestra.io.kestra.plugin.jdbc.postgresql.Query";
    let store = plugin_task(
        ws,
        exec,
        query,
        with(serde_json::json!({"sql": "SELECT i, t FROM src ORDER BY i", "fetchType": "STORE"})),
        vec![],
    );
    let out = h
        .get(query)
        .expect("served")
        .run(&store)
        .await
        .expect("no worker error");
    println!(
        "    STORE: {:?} {:?} {:?} files {:?}",
        out.state, out.outputs, out.message, out.files
    );
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{:?}",
        out.message
    );
    let uri = out.outputs.as_ref().unwrap()["uri"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(out.files.len(), 1);
    assert_eq!(
        out.files[0].uri, uri,
        "the URI the plugin returned is the file collected"
    );
    h.get(query)
        .unwrap()
        .release(&store)
        .await
        .expect("released");

    let batch = "kestra.io.kestra.plugin.jdbc.postgresql.Batch";
    let load = plugin_task(
        ws,
        exec,
        batch,
        with(serde_json::json!({"from": uri, "sql": "INSERT INTO dst (i, t) VALUES (?, ?)"})),
        vec![egret_workflow::storage::IndexedFile {
            uri: uri.clone(),
            execution_id: exec,
            task_run_id: Some(store.run_id),
            size: out.files[0].size,
        }],
    );
    let out = h
        .get(batch)
        .expect("served")
        .run(&load)
        .await
        .expect("no worker error");
    println!(
        "    BATCH: {:?} {:?} {:?}",
        out.state, out.outputs, out.message
    );
    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{:?}",
        out.message
    );
    assert_eq!(out.outputs.as_ref().unwrap()["rowCount"], 3);
    h.get(batch)
        .unwrap()
        .release(&load)
        .await
        .expect("released");
    assert_eq!(
        pg.psql("SELECT string_agg(i || ':' || t, ',' ORDER BY i) FROM dst")
            .trim(),
        "20:p,21:q,22:r"
    );
}

fn cases() -> Vec<(&'static str, Case)> {
    vec![
        case!(a_shell_command_runs_in_a_pod_and_the_pod_is_deleted),
        case!(input_and_output_files_round_trip),
        case!(output_files_are_streamed_out_of_the_pod_byte_identical),
        case!(a_reattached_pod_is_collected_under_the_same_uris),
        case!(an_unreachable_store_while_collecting_is_a_worker_error),
        case!(a_stored_input_file_streams_into_the_pod),
        case!(a_tar_x_that_dies_in_the_pod_fails_the_task),
        case!(a_short_stored_object_aborts_the_upload_and_fails_the_task),
        case!(a_nonzero_exit_fails_and_keeps_outputs),
        case!(cancel_deletes_the_pod),
        case!(k8s_cancel_with_grace_waits_for_termination_then_release_deletes),
        case!(a_second_run_call_for_the_same_attempt_reattaches_not_recreates),
        case!(a_line_written_just_before_the_reclaim_is_not_lost),
        case!(a_non_shell_interpreter_keeps_the_merged_stream_and_says_so),
        case!(a_non_shell_reattach_keeps_the_lines_written_before_the_reclaim),
        case!(the_orphan_sweep_deletes_pods_of_terminal_runs_only),
        case!(the_docker_orphan_sweep_removes_containers_of_terminal_runs_only),
        case!(an_exec_the_role_refuses_fails_the_task_naming_the_missing_verb),
        case!(an_unpullable_helper_image_fails_the_task_naming_the_setting),
        case!(a_task_that_fills_its_disk_is_evicted_and_fails_saying_so),
        case!(the_plugin_host_scratch_and_output_dirs_are_writable_as_run_as_user),
        case!(a_plugin_stores_and_reads_storage_in_a_pod),
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
