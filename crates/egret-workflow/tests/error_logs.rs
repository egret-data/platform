//! Package #5b Task B7 (spec D11): `errorLogs()` through the REAL worker, its
//! lines stored by the REAL writer (`PgLogSink`), read back by the prefetch
//! (`worker::claim::render_context`) before the task renders. Requires
//! DATABASE_URL.
//!
//! What is measured here, and only here:
//! - an `errors:` task reads the failed task's line on the FIRST read: the
//!   failed attempt's line is behind the writer's flush barrier before the
//!   outcome commits, so the bounded wait is not what makes it visible;
//! - an execution with no ERROR line waits the bound, about 1 s, and answers `[]`;
//! - the list is this execution's and this workspace's only (a line under
//!   this execution's id and another workspace's is not in it);
//! - an input that holds `errorLogs` but does not parse reads nothing;
//! - a SECRET input's value, masked when the line was stored, stays masked;
//! - a call reached only through `render(vars.…)` is read for, and answers;
//! - a call assembled at render is refused by egret's sentence, and an
//!   executor-side render (an `If`'s condition) reads nothing and fails.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use egret_metastore::Db;
use egret_workflow::logs::{spawn_writer, LogLevel, LogLimits, LogLine, LogRepo, WriterConfig};
use egret_workflow::worker::{error_log_reads_so_far, start_execution, tick, Handlers};
use serde_json::json;
use uuid::Uuid;

/// One test at a time: the read counter is the process's.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SECRET: &str = "hunter2-b7-errorlogs";

struct Fx {
    db: Db,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    egret_workflow::secret_inputs::install_for_tests();
    Some(Fx {
        db,
        _serial: serial,
    })
}

impl Fx {
    fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    async fn workspace(&self) -> Uuid {
        common::seed_workspace(&self.db).await.0
    }

    /// Start `source` and run it to the end on the real worker and writer.
    async fn run(&self, ws: Uuid, source: &str, inputs: serde_json::Value) -> Uuid {
        let p = self.pool();
        let ex = start_execution(p, ws, None, source, inputs).await.unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let (sink, writer) = spawn_writer(
            LogRepo::new(p.clone()),
            WriterConfig::default(),
            cancel.clone(),
        );
        let reg =
            Handlers::with_core_tasks(None).with_log_sink(Arc::new(sink), LogLimits::default());
        for _ in 0..256 {
            let ticked = tick(p, 16, &reg).await;
            assert!(ticked.is_ok(), "a worker error: {ticked:?}");
            if ticked.unwrap() == 0 {
                break;
            }
        }
        drop(reg);
        cancel.cancel();
        writer.await.unwrap();
        ex
    }

    /// `(state, message)` of `task` in execution `ex`.
    async fn run_of(&self, ex: Uuid, task: &str) -> (String, Option<String>) {
        sqlx::query_as(
            "SELECT state, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(ex)
        .bind(task)
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    /// The stored lines of `task` in `ex`, in order.
    async fn lines_of(&self, ex: Uuid, task: &str) -> Vec<(i16, String)> {
        sqlx::query_as(
            "SELECT level, message FROM workflow.task_logs \
              WHERE execution_id = $1 AND task_id = $2 ORDER BY id",
        )
        .bind(ex)
        .bind(task)
        .fetch_all(self.pool())
        .await
        .unwrap()
    }

    /// A terminal execution with one stored ERROR line, `message`.
    async fn other_execution_with(&self, ws: Uuid, message: &str) -> Uuid {
        let ex: Uuid = sqlx::query_scalar(
            "INSERT INTO workflow.executions \
               (workspace_id, namespace, flow_key, source, state, history) \
             VALUES ($1, 'b7', 'other', 'id: other\ntasks: []\n', 'FAILED', \
                     '[\"CREATED\",\"RUNNING\",\"FAILED\"]'::jsonb) \
             RETURNING id",
        )
        .bind(ws)
        .fetch_one(self.pool())
        .await
        .unwrap();
        let written = LogRepo::new(self.pool().clone())
            .insert_batch(&[LogLine {
                workspace_id: ws,
                execution_id: ex,
                task_run_id: None,
                task_id: Some("failed".into()),
                attempt: Some(0),
                level: LogLevel::Error,
                ts: chrono::Utc::now(),
                message: message.into(),
            }])
            .await
            .unwrap();
        assert_eq!(written, 1);
        ex
    }
}

/// `valids/errors`' shape: a `Fail`, then an `errors:` `Log` that lists the
/// error lines. Its `errorMessage` carries a SECRET input, and a `{{ … }}` of
/// its own, which must reach the log as text.
fn errors_flow() -> String {
    "id: errors\nnamespace: b7\ninputs:\n  - id: pw\n    type: SECRET\n\
     tasks:\n  - id: failed\n    type: io.kestra.plugin.core.execution.Fail\n    \
       errorMessage: \"boom pw={{ inputs.pw }} raw={% raw %}{{ 1 + 1 }}{% endraw %}\"\n\
     errors:\n  - id: t2\n    type: io.kestra.plugin.core.log.Log\n    \
       message: |\n      \
         {%- for log in errorLogs() -%}\n      \
         task={{ log.taskId }} level={{ log.level }} message={{ log.message }};\n      \
         {%- endfor -%}\n"
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_errors_task_reads_the_failed_tasks_line_on_its_first_read() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    // Another execution of the same workspace, and the same text under
    // another workspace: neither may reach the list.
    fx.other_execution_with(ws, "another execution's error")
        .await;
    let other_ws = fx.workspace().await;
    fx.other_execution_with(other_ws, "another workspace's error")
        .await;

    let reads_before = error_log_reads_so_far();
    let ex = fx.run(ws, &errors_flow(), json!({"pw": SECRET})).await;
    let reads = error_log_reads_so_far() - reads_before;

    let (state, message) = fx.run_of(ex, "failed").await;
    assert_eq!(state, "FAILED", "{message:?}");
    let (state, message) = fx.run_of(ex, "t2").await;
    assert_eq!(state, "SUCCESS", "{message:?}");

    let stored = fx.lines_of(ex, "failed").await;
    assert!(
        stored
            .iter()
            .any(|(level, m)| *level == 4 && m.contains("boom pw=******")),
        "the failed attempt's line is stored masked: {stored:?}"
    );
    let logged = fx.lines_of(ex, "t2").await;
    assert_eq!(logged.len(), 1, "{logged:?}");
    let line = &logged[0].1;
    assert!(
        line.starts_with("task=failed level=ERROR message=boom pw=******"),
        "{line}"
    );
    // The message's own `{{ 1 + 1 }}` is text, never rendered again.
    assert!(line.contains("raw={{ 1 + 1 }}"), "{line}");
    assert!(!line.contains(SECRET), "the secret leaked: {line}");
    assert!(
        !line.contains("another execution") && !line.contains("another workspace"),
        "a line of another execution reached the list: {line}"
    );
    // The measurement: one read answered it, no wait.
    assert_eq!(
        reads, 1,
        "the failed task's line was not visible on the first read"
    );

    // The read itself, scoped by both ids. Execution ids are unique, so an
    // execution of another workspace cannot tell the workspace predicate
    // apart (fix wave #5b, M8): a line stored under THIS execution's id and
    // another workspace's id can. The table's only key on the execution is
    // `execution_id` (`migrations/0159`), so the row is inserted directly:
    // `insert_batch` joins the execution on both ids and would drop it.
    let repo = LogRepo::new(fx.pool().clone());
    assert!(repo.error_lines(other_ws, ex).await.unwrap().is_empty());
    sqlx::query(
        "INSERT INTO workflow.task_logs (workspace_id, execution_id, task_id, attempt, level, ts, message) \
         VALUES ($1, $2, 'failed', 0, 4, now(), 'a cross-workspace line')",
    )
    .bind(other_ws)
    .bind(ex)
    .execute(fx.pool())
    .await
    .unwrap();
    let own = repo.error_lines(ws, ex).await.unwrap();
    assert!(!own.is_empty());
    assert!(
        own.iter().all(|l| l.message != "a cross-workspace line"),
        "the workspace predicate let another workspace's line through: {own:?}"
    );
    let theirs = repo.error_lines(other_ws, ex).await.unwrap();
    assert_eq!(theirs.len(), 1, "{theirs:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_error_line_the_read_waits_the_bound_and_answers_empty() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let source = "id: calm\nnamespace: b7\ntasks:\n  - id: say\n    \
        type: io.kestra.plugin.core.log.Log\n    \
        message: \"n={{ errorLogs() | length }} {{ errorLogs() }}\"\n";
    let reads_before = error_log_reads_so_far();
    let started = Instant::now();
    let ex = fx.run(ws, source, json!({})).await;
    let took = started.elapsed();
    let reads = error_log_reads_so_far() - reads_before;
    let (state, message) = fx.run_of(ex, "say").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(fx.lines_of(ex, "say").await, [(2, "n=0 []".to_string())]);
    assert!(reads >= 2, "an empty first read is read again: {reads}");
    assert!(
        took >= egret_workflow::worker::ERROR_LOGS_WAIT,
        "the wait is at least the bound: {took:?}"
    );
    assert!(
        took < egret_workflow::worker::ERROR_LOGS_WAIT + Duration::from_secs(10),
        "and bounded: {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_that_does_not_call_it_reads_nothing() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let source = "id: quiet\nnamespace: b7\ntasks:\n  - id: say\n    \
        type: io.kestra.plugin.core.log.Log\n    message: \"errorLogs() is not called\"\n";
    let reads_before = error_log_reads_so_far();
    let ex = fx.run(ws, source, json!({})).await;
    assert_eq!(fx.run_of(ex, "say").await.0, "SUCCESS");
    assert_eq!(error_log_reads_so_far(), reads_before);
}

/// Fix wave #5b, I3: an input that holds the name beside a broken `{{`
/// does not parse, so it calls nothing: no read and no wait. (It used to
/// answer `true`, and every task of the execution read and waited 1 s.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_input_that_does_not_parse_reads_nothing() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let source = "id: broken\nnamespace: b7\ninputs:\n  - id: x\n    type: STRING\n\
        tasks:\n  - id: say\n    type: io.kestra.plugin.core.log.Log\n    \
        message: \"n={{ inputs.x | length }}\"\n";
    let reads_before = error_log_reads_so_far();
    let ex = fx.run(ws, source, json!({"x": "errorLogs {{"})).await;
    let (state, message) = fx.run_of(ex, "say").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(fx.lines_of(ex, "say").await, [(2, "n=12".to_string())]);
    // The wait follows a read, so no read is no wait.
    assert_eq!(error_log_reads_so_far(), reads_before, "the worker read");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_assembled_call_and_an_executor_render_are_refused() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    // Assembled at render: the worker did not see a call, so egret's sentence.
    let source = "id: assembled\nnamespace: b7\ntasks:\n  - id: say\n    \
        type: io.kestra.plugin.core.log.Log\n    \
        message: \"{{ render('{{ error' ~ 'Logs() }}') }}\"\n";
    let ex = fx.run(ws, source, json!({})).await;
    let (state, message) = fx.run_of(ex, "say").await;
    assert_eq!(state, "FAILED");
    assert!(
        message
            .as_deref()
            .unwrap_or_default()
            .contains("The 'errorLogs' function was not loaded for this render"),
        "{message:?}"
    );
    // An `If`'s condition renders in `settle`, off the worker, so the worker
    // reads nothing for it and the render fails. `settle` stores no message for
    // a failed condition render (`worker::render::RenderSide::failures`), so
    // the sentence -- Kestra's own -- is pinned by
    // `runners::error_logs::tests::outside_the_worker_it_is_kestras_refusal`.
    let source = "id: offworker\nnamespace: b7\ntasks:\n  - id: branch\n    \
        type: io.kestra.plugin.core.flow.If\n    \
        condition: \"{{ errorLogs() | length == 0 }}\"\n    then:\n      \
        - id: say\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";
    let reads_before = error_log_reads_so_far();
    let ex = fx.run(ws, source, json!({})).await;
    let (state, message) = fx.run_of(ex, "branch").await;
    assert_eq!(state, "FAILED", "{message:?}");
    assert_eq!(
        error_log_reads_so_far(),
        reads_before,
        "settle read the log"
    );
    let ran: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'say'",
    )
    .bind(ex)
    .fetch_one(fx.pool())
    .await
    .unwrap();
    assert_eq!(ran, 0, "the branch was not taken");
}

/// `render(vars.tpl)` on the worker reaches the list, as Kestra's nested
/// render on the worker does: the worker reads for `vars` too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_render_of_a_variable_reads_the_list() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let source = "id: nested\nnamespace: b7\nvariables:\n  tpl: \"{{ errorLogs()[0].message }}\"\n\
        tasks:\n  - id: failed\n    type: io.kestra.plugin.core.execution.Fail\n    \
        errorMessage: deep\nerrors:\n  - id: t2\n    type: io.kestra.plugin.core.log.Log\n    \
        message: \"got={{ render(vars.tpl) }}\"\n";
    let ex = fx.run(ws, source, json!({})).await;
    let (state, message) = fx.run_of(ex, "t2").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(fx.lines_of(ex, "t2").await, [(2, "got=deep".to_string())]);
}
