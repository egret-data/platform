//! KI-112: a flow input declared `type: SECRET` is never returned in clear,
//! and the run still renders the real value.
//!
//! Every test starts through the real start route, runs the REAL runtime
//! (`spawn_with_logs`, the production core handlers, the database log
//! writer), and reads back through the real read routes. `#[sqlx::test]`
//! because the claim is global (`tests/task_logs.rs`'s reason).
//!
//! The run's proof that it saw the real value is a length: the `Log` writes
//! `len={{ inputs.pw | length }}`, and [`PLAIN`] is longer than the mask, so
//! a run that rendered the masked string would log another number.
//!
//! The read paths, by route:
//! - `POST .../flows/{id}/executions` (the start's own answer)
//! - `GET .../executions/{id}`, `GET .../executions` (and `?flow_id=`)
//! - `GET .../executions/{id}/graph`
//! - `GET .../executions/{id}/logs`
//! - `POST .../executions/{id}/replay` and `.../resume` (their answers)
//!
//! and the stored row: `workflow.executions.inputs` and `secret_inputs`,
//! `workflow.task_runs.message`, `workflow.task_logs.message`.

mod common;

use std::time::Duration;

use egret_metastore::Db;
use egret_workflow::logs::LogLimits;
use egret_workflow::runtime::{spawn_with_logs, RuntimeConfig};
use egret_workflow::worker::Handlers;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The value posted for the SECRET input: distinctive, and 18 characters
/// where the mask is 6.
const PLAIN: &str = "hunter2-ki112-0123";
const MASK: &str = "******";

fn source(id: &str, tasks: &str) -> String {
    format!(
        "id: {id}\nnamespace: ki112\ninputs:\n  - id: pw\n    type: SECRET\n  \
         - id: who\n    type: STRING\ntasks:\n{tasks}"
    )
}

/// A `Log` that prints the secret, its length and the plain input.
const LOG_TASK: &str = "  - id: say\n    type: io.kestra.plugin.core.log.Log\n    \
    message: \"who={{ inputs.who }} pw={{ inputs.pw }} len={{ inputs.pw | length }}\"\n";

/// What [`LOG_TASK`] logs for a run that saw the real value.
fn logged() -> String {
    format!("who=world pw={MASK} len={}", PLAIN.len())
}

struct Fx {
    db: Db,
    pool: sqlx::PgPool,
    ws: Uuid,
    user: egret_core::UserId,
    app: common::App,
}

async fn fx(pool: sqlx::PgPool) -> Fx {
    egret_test_db::assert_database_url_is_exported();
    egret_workflow::secret_inputs::install_for_tests();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    Fx {
        db,
        pool,
        ws,
        user,
        app,
    }
}

impl Fx {
    async fn get(&self, path: &str) -> String {
        let res = common::get(&self.app, &format!("/api/ws/{}{path}", self.ws), self.user).await;
        assert_eq!(res.status(), 200, "GET {path}");
        common::body_text(res).await
    }

    async fn start(&self, flow: Uuid, inputs: Value) -> (Uuid, String) {
        let res = common::post(
            &self.app,
            &format!("/api/ws/{}/workflows/flows/{flow}/executions", self.ws),
            self.user,
            json!({ "inputs": inputs }),
        )
        .await;
        assert_eq!(res.status(), 201);
        let text = common::body_text(res).await;
        let dto: Value = serde_json::from_str(&text).unwrap();
        (dto["id"].as_str().unwrap().parse().unwrap(), text)
    }

    /// Run the real runtime until `exec` is in `state`.
    async fn run_until(&self, exec: Uuid, state: &str) {
        self.run_until_row(
            "SELECT state FROM workflow.executions WHERE id = $1",
            exec,
            state,
        )
        .await
    }

    /// Run the real runtime until the row `query` finds for `exec` is in
    /// `state` (no row yet counts as not yet).
    async fn run_until_row(&self, query: &'static str, exec: Uuid, state: &str) {
        let cancel = CancellationToken::new();
        let rt = spawn_with_logs(
            self.pool.clone(),
            Handlers::with_core_tasks(egret_workflow::storage::current()),
            RuntimeConfig {
                enabled: true,
                tick_interval: Duration::from_millis(50),
                batch: 8,
                reaper_interval: Duration::from_secs(3600),
                working_timeout: Duration::from_secs(3600),
                container_default_timeout: egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
                kill_poll: egret_workflow::worker::DEFAULT_KILL_POLL,
                scheduler_enabled: false,
                trigger_interval: Duration::from_secs(3600),
                log_limits: LogLimits::default(),
                log_retention_days: 30,
            },
            cancel.clone(),
        );
        let started = std::time::Instant::now();
        loop {
            let now: Option<String> = sqlx::query_scalar(query)
                .bind(exec)
                .fetch_optional(&self.pool)
                .await
                .unwrap();
            if now.as_deref() == Some(state) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "timed out: {now:?} for {exec}, waiting for {state}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(15), rt.join())
            .await
            .expect("the runtime stops");
        // The log route serves a line 2 s after it was stored: move the rows
        // back rather than sleep.
        sqlx::query(
            "UPDATE workflow.task_logs SET inserted_at = inserted_at - interval '3 seconds' \
              WHERE execution_id IN (SELECT id FROM workflow.executions WHERE workspace_id = $1)",
        )
        .bind(self.ws)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn log_messages(&self, exec: Uuid) -> Vec<String> {
        let page: Value = serde_json::from_str(
            &self
                .get(&format!("/workflows/executions/{exec}/logs?limit=100"))
                .await,
        )
        .unwrap();
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|l| l["message"].as_str().unwrap().to_string())
            .collect()
    }

    /// The stored row, as text: nothing a backup or a SELECT shows may hold it.
    async fn assert_row_holds_no_plaintext(&self, exec: Uuid) {
        let (inputs, sealed): (Value, Option<Vec<u8>>) =
            sqlx::query_as("SELECT inputs, secret_inputs FROM workflow.executions WHERE id = $1")
                .bind(exec)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        assert_eq!(inputs["pw"], MASK, "inputs holds the mask: {inputs}");
        let sealed = sealed.expect("the secret is sealed beside the row");
        assert!(
            !sealed.windows(PLAIN.len()).any(|w| w == PLAIN.as_bytes()),
            "secret_inputs is ciphertext"
        );
        let elsewhere: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM workflow.task_runs \
                      WHERE execution_id = $1 AND message LIKE '%' || $2 || '%') \
                  + (SELECT count(*) FROM workflow.task_logs \
                      WHERE execution_id = $1 AND message LIKE '%' || $2 || '%')",
        )
        .bind(exec)
        .bind(PLAIN)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        assert_eq!(elsewhere, 0, "no run message and no log line holds it");
    }
}

fn assert_masked(what: &str, text: &str) {
    assert!(!text.contains(PLAIN), "{what} carries the secret: {text}");
}

/// The start's answer, the detail, both lists, the graph and the logs are
/// masked; the run logged the real value's length; a STRING input is
/// unchanged everywhere.
#[sqlx::test(migrations = "../../migrations")]
async fn a_secret_input_is_masked_in_every_read_and_the_run_sees_the_real_value(
    pool: sqlx::PgPool,
) {
    // Arrange
    let f = fx(pool).await;
    let flow = common::seed_flow(&f.db, f.ws, f.user, &source("reads", LOG_TASK), "draft").await;

    // Act
    let (exec, started) = f.start(flow, json!({"pw": PLAIN, "who": "world"})).await;
    f.run_until(exec, "SUCCESS").await;

    // Assert
    assert_masked("the start's answer", &started);
    let detail = f.get(&format!("/workflows/executions/{exec}")).await;
    assert_masked("the execution detail", &detail);
    let dto: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(dto["inputs"], json!({"pw": MASK, "who": "world"}));
    assert_masked("the list", &f.get("/workflows/executions").await);
    assert_masked(
        "the flow's list",
        &f.get(&format!("/workflows/executions?flow_id={flow}"))
            .await,
    );
    assert_masked(
        "the graph",
        &f.get(&format!("/workflows/executions/{exec}/graph")).await,
    );
    let logs = f.log_messages(exec).await;
    assert_masked("the logs", &logs.join("\n"));
    assert_eq!(logs, vec![logged()], "the run rendered the real value");
    f.assert_row_holds_no_plaintext(exec).await;
}

/// The replay route starts a new execution from the stored one: it must run
/// with the stored secret, not with the mask the row shows.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_runs_with_the_stored_secret(pool: sqlx::PgPool) {
    // Arrange
    let f = fx(pool).await;
    let flow = common::seed_flow(&f.db, f.ws, f.user, &source("replay", LOG_TASK), "draft").await;
    let (first, _) = f.start(flow, json!({"pw": PLAIN, "who": "world"})).await;
    f.run_until(first, "SUCCESS").await;

    // Act
    let res = common::post_no_body(
        &f.app,
        &format!("/api/ws/{}/workflows/executions/{first}/replay", f.ws),
        f.user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let text = common::body_text(res).await;
    let replay: Uuid = serde_json::from_str::<Value>(&text).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    f.run_until(replay, "SUCCESS").await;

    // Assert
    assert_masked("the replay's answer", &text);
    assert_eq!(f.log_messages(replay).await, vec![logged()]);
    f.assert_row_holds_no_plaintext(replay).await;
}

/// A resumed execution is the same row: the task after the pause renders
/// the real value.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resumed_execution_still_sees_the_secret(pool: sqlx::PgPool) {
    // Arrange
    let f = fx(pool).await;
    let tasks = format!(
        "  - id: wait\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1H\n{LOG_TASK}"
    );
    let flow = common::seed_flow(&f.db, f.ws, f.user, &source("resume", &tasks), "draft").await;
    let (exec, _) = f.start(flow, json!({"pw": PLAIN, "who": "world"})).await;
    f.run_until(exec, "PAUSED").await;

    // Act
    let res = common::post_no_body(
        &f.app,
        &format!("/api/ws/{}/workflows/executions/{exec}/resume", f.ws),
        f.user,
    )
    .await;
    assert_eq!(res.status(), 200);
    assert_masked("the resume's answer", &common::body_text(res).await);
    f.run_until(exec, "SUCCESS").await;

    // Assert
    assert_eq!(f.log_messages(exec).await, vec![logged()]);
    f.assert_row_holds_no_plaintext(exec).await;
}

/// The two copies the worker makes of an execution's inputs -- a `Loop`
/// iteration and a `CREATE_NEW_EXECUTION` retry's replacement -- carry the
/// sealed secret, and a failed run's message is masked.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_and_a_retry_replacement_see_the_secret(pool: sqlx::PgPool) {
    // Arrange
    let f = fx(pool).await;
    let body = LOG_TASK
        .replace("\n    ", "\n        ")
        .replace("  - id", "      - id");
    let tasks = format!(
        "  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks:\n{body}\
         \x20 - id: boom\n    type: io.kestra.plugin.core.execution.Fail\n    \
         errorMessage: \"bad {{{{ inputs.pw }}}}\"\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n      \
         type: constant\n      interval: PT0.1S\n      maxAttempts: 2\n"
    );
    let flow = common::seed_flow(&f.db, f.ws, f.user, &source("copies", &tasks), "draft").await;

    // Act
    let (exec, _) = f.start(flow, json!({"pw": PLAIN, "who": "world"})).await;
    // A `CREATE_NEW_EXECUTION` retry ends the original `RETRIED` and starts
    // the replacement after its interval; the replacement fails again.
    f.run_until_row(
        "SELECT state FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
        exec,
        "FAILED",
    )
    .await;
    let replacement: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(exec)
    .fetch_one(&f.pool)
    .await
    .expect("the retry made a replacement");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM workflow.executions WHERE id = $1")
            .bind(exec)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        "RETRIED"
    );

    // Assert
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index = 0",
    )
    .bind(exec)
    .fetch_one(&f.pool)
    .await
    .expect("one iteration");
    assert_eq!(f.log_messages(iteration).await, vec![logged()]);
    f.assert_row_holds_no_plaintext(iteration).await;
    for ex in [exec, replacement] {
        let logs = f.log_messages(ex).await;
        assert!(
            logs.iter().any(|l| l.contains(&format!("bad {MASK}"))),
            "the failure is logged masked: {logs:?}"
        );
        f.assert_row_holds_no_plaintext(ex).await;
        assert_masked(
            "the detail",
            &f.get(&format!("/workflows/executions/{ex}")).await,
        );
    }
}

/// A `Subflow` child is a different flow: it seals the inputs ITS
/// declaration calls SECRET, here the value the parent renders into it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_seals_its_own_secret_input(pool: sqlx::PgPool) {
    // Arrange
    let f = fx(pool).await;
    common::seed_flow(&f.db, f.ws, f.user, &source("child", LOG_TASK), "draft").await;
    let parent = "id: parent\nnamespace: ki112\ninputs:\n  - id: pw\n    type: SECRET\n\
                  tasks:\n  - id: call\n    type: io.kestra.plugin.core.flow.Subflow\n    \
                  namespace: ki112\n    flowId: child\n    inputs:\n      \
                  pw: \"{{ inputs.pw }}\"\n      who: world\n";
    let flow = common::seed_flow(&f.db, f.ws, f.user, parent, "draft").await;

    // Act
    let (exec, _) = f.start(flow, json!({"pw": PLAIN})).await;
    f.run_until(exec, "SUCCESS").await;

    // Assert
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(exec)
            .fetch_one(&f.pool)
            .await
            .expect("one child");
    assert_eq!(f.log_messages(child).await, vec![logged()]);
    f.assert_row_holds_no_plaintext(child).await;
    assert_masked(
        "the child's detail",
        &f.get(&format!("/workflows/executions/{child}")).await,
    );
}

/// An execution stored before migration 0203 holds its secret in clear: the
/// startup backfill masks and seals it, keeps the STRING input, and a second
/// pass changes nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn the_backfill_masks_and_seals_a_secret_stored_in_clear(pool: sqlx::PgPool) {
    // Arrange
    let f = fx(pool).await;
    let old: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, inputs) \
         VALUES ($1, 'ki112', 'old', $2, 'SUCCESS', '[\"CREATED\",\"SUCCESS\"]'::jsonb, $3) \
         RETURNING id",
    )
    .bind(f.ws)
    .bind(source("old", LOG_TASK))
    .bind(json!({"pw": PLAIN, "who": "world"}))
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // Act
    let changed = egret_workflow::secret_inputs::backfill(&f.pool)
        .await
        .unwrap();
    let again = egret_workflow::secret_inputs::backfill(&f.pool)
        .await
        .unwrap();

    // Assert
    assert_eq!((changed, again), (1, 0));
    let (inputs, sealed): (Value, Option<Vec<u8>>) =
        sqlx::query_as("SELECT inputs, secret_inputs FROM workflow.executions WHERE id = $1")
            .bind(old)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(inputs, json!({"pw": MASK, "who": "world"}));
    let (revealed, _) = egret_workflow::secret_inputs::reveal(inputs, sealed.as_deref()).unwrap();
    assert_eq!(revealed, json!({"pw": PLAIN, "who": "world"}));
    assert_masked(
        "the old execution's detail",
        &f.get(&format!("/workflows/executions/{old}")).await,
    );
}

/// A STRING input is stored and returned exactly as posted, and a flow with
/// no SECRET value seals nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_without_a_secret_value_is_unchanged(pool: sqlx::PgPool) {
    // Arrange
    let f = fx(pool).await;
    let flow = common::seed_flow(&f.db, f.ws, f.user, &source("plain", LOG_TASK), "draft").await;

    // Act
    let (exec, started) = f.start(flow, json!({"who": "world", "pw": null})).await;

    // Assert
    let dto: Value = serde_json::from_str(&started).unwrap();
    assert_eq!(dto["inputs"], json!({"who": "world", "pw": null}));
    let sealed: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT secret_inputs FROM workflow.executions WHERE id = $1")
            .bind(exec)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(sealed, None);
}
