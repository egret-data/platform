//! `WorkingDirectory`'s snapshot through a real Docker daemon (spec §5.2, plan
//! Task 6 Step 1 and Step 4): child A writes a file into the block's working
//! directory, child B reads it. The copy-out after A and the copy-in before B
//! are the Docker runner's `carry_out_snapshot` and `copy_in_snapshot`.
//!
//! # Needs a daemon and a database, and FAILS without them
//!
//! It does not self-skip (the lesson of `container_docker_live.rs`): without a
//! daemon, `docker info` fails and the test panics naming it. The database is
//! the `#[sqlx::test]` one (`DATABASE_URL`, exported, a private database).
//!
//! The object store is this process's `MemStore` (`install_mem_for_tests`). No
//! RustFS or S3 bucket is touched, and the shared dev stack is not used.
//!
//! Image: `alpine:3.20`, which must be present locally. The test does not pull.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use egret_workflow::worker::commands::DockerRunner;
use egret_workflow::worker::handlers::script::{register, PgFeatureState, ScriptRuntime};
use egret_workflow::worker::{tick_with, Handlers, WorkerTiming};
use sqlx::PgPool;

const IMAGE: &str = "alpine:3.20";

fn require_docker() {
    let up = std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(
        up,
        "the Docker daemon does not answer `docker info`; this test needs it and does not skip"
    );
    let present = std::process::Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(
        present,
        "the image {IMAGE} is not present locally; `docker pull` it first"
    );
}

/// The Docker runner, over the process's private store, with containers
/// allowed in this workspace.
fn registry(pool: &PgPool) -> Handlers {
    use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
    let files = egret_workflow::storage::install_mem_for_tests();
    let mut reg = Handlers::with_core_tasks(Some(files.clone()));
    register(
        &mut reg,
        Arc::new(ScriptRuntime {
            runner_kind: egret_workflow::scripts::RunnerKind::Docker,
            runner: Arc::new(DockerRunner::new().with_files(files.clone())),
            plugin_configurations: PluginConfigurations(vec![PluginConfiguration {
                r#type: egret_workflow::scripts::DOCKER_RUNNER.to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["*".into(), "*/*".into(), "*/*/*".into()]),
                    allowed_types: None,
                },
            }]),
            features: Arc::new(PgFeatureState::new(pool.clone())),
            files: Some(files),
            pool: None,
        }),
    );
    reg
}

/// One child: a shell command on the Docker runner.
fn child(id: &str, commands: &[&str]) -> String {
    let cmds: String = commands
        .iter()
        .map(|c| format!("          - '{c}'\n"))
        .collect();
    format!(
        "      - id: {id}\n        type: io.kestra.plugin.scripts.shell.Commands\n        taskRunner:\n          type: io.kestra.plugin.scripts.runner.docker.Docker\n        containerImage: {IMAGE}\n        commands:\n{cmds}"
    )
}

fn block(children: &[String]) -> String {
    format!(
        "id: wd-live\nnamespace: w6.live\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n{}",
        children.concat()
    )
}

/// Tick until the block is terminal; returns the rounds used.
async fn settle(pool: &PgPool, reg: &Handlers, ex: uuid::Uuid, rounds: usize) {
    for _ in 0..rounds {
        tick_with(pool, 8, reg, WorkerTiming::default())
            .await
            .expect("a tick");
        let state: String = sqlx::query_scalar(
            "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
        )
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("the block");
        if matches!(state.as_str(), "SUCCESS" | "FAILED" | "WARNING") {
            return;
        }
    }
    panic!("the block did not settle in {rounds} ticks");
}

async fn state_of(pool: &PgPool, ex: uuid::Uuid, task: &str) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("the run of {task}: {e}"))
}

/// **Child A writes a file; child B reads it.** The file exists in B's
/// container only if the snapshot carried it: A's copy-out, the stored
/// archive, and B's copy-in. B's `test` fails otherwise and the block with it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_file_written_by_one_child_is_read_by_the_next_through_docker(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);

    let source = block(&[
        child(
            "writer",
            &[
                "echo hello > a.txt",
                "mkdir -p sub && echo deep > sub/b.txt",
            ],
        ),
        child(
            "reader",
            &[
                "test \"$(cat a.txt)\" = hello",
                "test \"$(cat sub/b.txt)\" = deep",
            ],
        ),
    ]);
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    settle(&pool, &reg, ex, 200).await;

    assert_eq!(state_of(&pool, ex, "block").await.0, "SUCCESS");
    assert_eq!(state_of(&pool, ex, "writer").await.0, "SUCCESS");
    let (reader, message) = state_of(&pool, ex, "reader").await;
    assert_eq!(reader, "SUCCESS", "{message:?}");
}

/// **A child that writes a symbolic link fails the child, not the worker, and
/// names it.** The snapshot refuses a link; the block stops (spec §5.2, §5.3).
#[sqlx::test(migrations = "../../migrations")]
async fn a_symbolic_link_in_the_directory_fails_the_child_by_name(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);

    let source = block(&[
        child("linker", &["ln -s /etc/hostname link"]),
        child("after", &["echo never > after.txt"]),
    ]);
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    settle(&pool, &reg, ex, 200).await;

    let (state, message) = state_of(&pool, ex, "linker").await;
    assert_eq!(state, "FAILED");
    let message = message.expect("the reason");
    assert!(message.contains("symbolic link"), "{message}");
    assert_eq!(state_of(&pool, ex, "block").await.0, "FAILED");
}

/// Measurement for plan Task 6 Step 4 and spec §7.3, not a pass/fail gate: the
/// wall time of a child that leaves a 1 MB file (the baseline) and of one
/// that leaves 100 MB, and of the child that reads it. The times include the
/// container's create and start, so the copy cost is the difference from the
/// baseline. Run with `--nocapture` to see the numbers.
#[sqlx::test(migrations = "../../migrations")]
async fn the_snapshot_cost_of_a_hundred_megabyte_directory_is_measured(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);

    for (label, size_mb) in [("1 MB", 1u64), ("100 MB", 100u64)] {
        let source = block(&[
            child(
                "writer",
                &[&format!(
                    "dd if=/dev/zero of=big.bin bs=1M count={size_mb} 2>/dev/null"
                )],
            ),
            child("reader", &["test -s big.bin"]),
        ]);
        let started = Instant::now();
        let ex = egret_workflow::worker::start_execution(
            &pool,
            ws,
            None,
            &source,
            serde_json::json!({}),
        )
        .await
        .expect("start");
        settle(&pool, &reg, ex, 400).await;
        let total: Duration = started.elapsed();
        assert_eq!(state_of(&pool, ex, "reader").await.0, "SUCCESS", "{label}");
        let spans: Vec<(String, f64)> = sqlx::query_as(
            "SELECT task_id, extract(epoch FROM (updated_at - created_at))::float8 \
               FROM workflow.task_runs WHERE execution_id = $1 AND task_id IN ('writer', 'reader') \
              ORDER BY task_id",
        )
        .bind(ex)
        .fetch_all(&pool)
        .await
        .expect("spans");
        eprintln!(
            "snapshot cost [{label}]: block {:.2}s wall; per child, created to last update (s): {:?}",
            total.as_secs_f64(),
            spans
        );
    }
}

/// A block with `props` (indented four spaces, as its own keys) and `children`.
fn block_with(props: &str, children: &[String]) -> String {
    format!(
        "id: wd-live\nnamespace: w6.live\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n{props}    tasks:\n{}",
        children.concat()
    )
}

/// The `outputs` column of the block, as the finish claim wrote it.
async fn block_outputs(pool: &PgPool, ex: uuid::Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
    )
    .bind(ex)
    .fetch_one(pool)
    .await
    .expect("the block's run")
}

/// The bytes of the file a block output names.
async fn collected_bytes(ws: uuid::Uuid, uri: &str) -> Vec<u8> {
    let fs = egret_workflow::storage::install_mem_for_tests();
    let object = fs.object_uri(ws, uri).expect("the object of the URI");
    fs.store.get(&object).await.expect("the collected bytes")
}

/// **`outputFiles` is collected from the directory the last child left** (spec
/// §5.5): the file a child wrote, and a `*` match, are the block's outputs with
/// the bytes the child wrote.
#[sqlx::test(migrations = "../../migrations")]
async fn output_files_are_collected_after_the_last_child_through_docker(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);

    let source = block_with(
        "    outputFiles:\n      - out.txt\n      - \"*.log\"\n",
        &[
            child("writer", &["echo hello > out.txt", "echo one > a.log"]),
            child("reader", &["test \"$(cat out.txt)\" = hello"]),
        ],
    );
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    settle(&pool, &reg, ex, 200).await;

    assert_eq!(state_of(&pool, ex, "block").await.0, "SUCCESS");
    let outputs = block_outputs(&pool, ex).await.expect("the block's outputs");
    let files = outputs["outputFiles"].as_object().expect("a map");
    assert_eq!(files.len(), 2, "{outputs}");
    let out = files["out.txt"].as_str().expect("a URI");
    assert_eq!(collected_bytes(ws, out).await, b"hello\n");
    let log = files["a.log"].as_str().expect("a URI");
    assert_eq!(collected_bytes(ws, log).await, b"one\n");
}

/// **A failed child still leaves its outputs** (V2, measured on the image): the
/// child writes `out.txt` and fails; the block stops, and its `outputFiles` are
/// collected from the directory the failed child left.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_child_still_leaves_its_output_files(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);

    let source = block_with(
        "    outputFiles:\n      - out.txt\n",
        &[
            child("failing", &["echo kept > out.txt", "exit 1"]),
            child("after", &["echo never > after.txt"]),
        ],
    );
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    settle(&pool, &reg, ex, 200).await;

    assert_eq!(state_of(&pool, ex, "failing").await.0, "FAILED");
    assert_eq!(state_of(&pool, ex, "block").await.0, "FAILED");
    let outputs = block_outputs(&pool, ex).await.expect("the block's outputs");
    let uri = outputs["outputFiles"]["out.txt"].as_str().expect("a URI");
    assert_eq!(collected_bytes(ws, uri).await, b"kept\n");
}

/// **A declared output that is a directory fails the child that left it, by
/// name** (spec §5.3): the block stops, and the next child gets no run.
#[sqlx::test(migrations = "../../migrations")]
async fn a_declared_output_that_is_a_directory_fails_the_child_by_name(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);

    let source = block_with(
        "    outputFiles:\n      - sub\n",
        &[
            child("maker", &["mkdir -p sub && echo x > sub/x.txt"]),
            child("after", &["echo never > after.txt"]),
        ],
    );
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    settle(&pool, &reg, ex, 200).await;

    let (state, message) = state_of(&pool, ex, "maker").await;
    assert_eq!(state, "FAILED");
    let message = message.expect("the reason");
    assert!(
        message.contains("sub") && message.contains("DIRECTORY"),
        "{message}"
    );
    assert_eq!(state_of(&pool, ex, "block").await.0, "FAILED");
    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'after'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(runs, 0, "the sibling after the failed child gets no run");
}

/// **Task 2.3b, live: a block inside a `WaitFor` is prepared again each
/// iteration, so its Docker child runs in both, each time in a fresh
/// directory.** The child refuses a directory that already holds `a.txt`,
/// then writes it: the second iteration passes only if it starts from a new
/// prepare rather than the first iteration's directory. Before the fix the
/// second iteration's block ended at once and `child` never ran again (its
/// last row had no history).
#[sqlx::test(migrations = "../../migrations")]
async fn a_block_inside_a_wait_for_runs_its_docker_child_each_iteration(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    require_docker();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let reg = registry(&pool);
    // The block and its child, two levels deeper than `block()` puts them.
    let inner: String = child("child", &["test ! -e a.txt", "echo once > a.txt"])
        .lines()
        .map(|l| format!("    {l}\n"))
        .collect();
    let source = format!(
        "id: wd-loop-live\nnamespace: w6.live\ntasks:\n  - id: until\n    type: io.kestra.plugin.core.flow.WaitFor\n    condition: \"false\"\n    checkFrequency:\n      maxIterations: 1\n      interval: PT0S\n    failOnMaxReached: false\n    tasks:\n      - id: block\n        type: io.kestra.plugin.core.flow.WorkingDirectory\n        tasks:\n{inner}"
    );
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .unwrap_or_else(|e| panic!("start: {e}\n{source}"));
    let mut runs = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let end = loop {
        tick_with(&pool, 1, &reg, WorkerTiming::default())
            .await
            .expect("a tick");
        let row: Option<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'child'",
        )
        .bind(ex)
        .fetch_optional(&pool)
        .await
        .expect("the child");
        if let Some((id, state)) = row {
            if state != "CREATED" && runs.last() != Some(&(id, state.clone())) {
                runs.push((id, state));
            }
        }
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
                .bind(ex)
                .fetch_one(&pool)
                .await
                .expect("the execution");
        if matches!(state.as_str(), "SUCCESS" | "FAILED" | "WARNING" | "KILLED") {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "not ended within 120 s: {runs:?}"
        );
    };
    let done: Vec<_> = runs.iter().filter(|(_, s)| s == "SUCCESS").collect();
    let (child, message) = state_of(&pool, ex, "child").await;
    assert_eq!(end, "SUCCESS", "child {child}: {message:?}; {runs:?}");
    assert_eq!(
        done.len(),
        2,
        "the child succeeded once per iteration, in a fresh directory: {runs:?}"
    );
    assert_ne!(
        done[0].0, done[1].0,
        "each iteration's child has its own id"
    );
}
