//! The delete, labels, log download and log delete routes (workflow package
//! 7d, spec `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md`
//! D1, D4, D5, D7-D9, D12; plan corrections S3, S7, S8, S9, S13):
//!
//! ```text
//! DELETE /api/ws/{workspace}/workflows/executions/{id}
//! POST   /api/ws/{workspace}/workflows/executions/{id}/labels
//! GET    /api/ws/{workspace}/workflows/executions/{id}/logs/download
//! DELETE /api/ws/{workspace}/workflows/executions/{id}/logs
//! ```
//!
//! Every refusal is paired with an accepted control. Every test is
//! `#[sqlx::test]`: the worker's `claim` and `tick` are global, so each test
//! owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test delete_labels_logs_routes

use egret_metastore::Db;
use egret_workflow::worker::{Handlers, Registry};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";

// ── Fixtures (as `tests/pause_resume_routes.rs`) ───────────────────────────

/// T3 test 6's `Loop` flow.
const LOOPY: &str = r#"id: loopy
namespace: t
tasks:
  - id: each
    type: io.kestra.plugin.core.flow.Loop
    values: ["1", "2"]
    concurrencyLimit: 1
    tasks:
      - id: a
        type: io.kestra.plugin.core.log.Log
        message: "{{ item.value }}"
  - id: after
    type: io.kestra.plugin.core.log.Log
    message: done
"#;

async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId, common::App) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    (db, ws, user, app)
}

/// The flow row `source` is a draft of, created on first use (the
/// workspace's creator owns it).
async fn flow_of(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let parsed = egret_workflow::spec::parse(source).expect("parse");
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.flows WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3",
    )
    .bind(ws)
    .bind(&parsed.namespace)
    .bind(&parsed.id)
    .fetch_optional(pool)
    .await
    .expect("look up the flow");
    if let Some(id) = found {
        return id;
    }
    let owner: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .expect("workspace owner");
    let db = Db { pool: pool.clone() };
    common::seed_flow(&db, ws, egret_core::UserId(owner), source, "draft").await
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let flow = flow_of(pool, ws, source).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), source, json!({}))
        .await
        .expect("start execution")
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution state")
}

fn handlers() -> Handlers {
    Handlers::with_core_tasks(egret_workflow::storage::current())
}

async fn settle_everything(pool: &sqlx::PgPool, reg: &dyn Registry) {
    for _ in 0..64 {
        let ran = egret_workflow::worker::tick(pool, 16, reg)
            .await
            .expect("tick");
        if ran == 0 {
            return;
        }
    }
    panic!("the loop never settled in 64 rounds");
}

async fn expect_error(res: axum::response::Response, status: u16, code: Option<&str>) -> String {
    assert_eq!(res.status(), status);
    let body = common::json(res).await;
    assert_eq!(body["code"].as_str(), code, "{body}");
    body["error"].as_str().unwrap_or_default().to_string()
}

fn one_task(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\nlabels:\n  team: data\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n"
    )
}
fn exec_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}")
}
fn labels_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/labels")
}
fn logs_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/logs")
}
fn download_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/logs/download")
}
/// Ended, and with no log line of its own: a test's lines are the only ones.
async fn ended(pool: &sqlx::PgPool, ws: Uuid, src: &str) -> Uuid {
    let ex = start(pool, ws, src).await;
    settle_everything(pool, &handlers()).await;
    assert_eq!(exec_state(pool, ex).await, "SUCCESS");
    sqlx::query("DELETE FROM workflow.task_logs WHERE execution_id = $1")
        .bind(ex)
        .execute(pool)
        .await
        .unwrap();
    ex
}
async fn exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workflow.executions WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
/// A line at a fixed instant, so the download's text is exact.
async fn log_line(
    pool: &sqlx::PgPool,
    ws: Uuid,
    ex: Uuid,
    task: Option<&str>,
    run: Option<Uuid>,
    level: i16,
    msg: &str,
) {
    sqlx::query(
        "INSERT INTO workflow.task_logs (workspace_id, execution_id, task_id, task_run_id, attempt, level, ts, message) \
         VALUES ($1, $2, $3, $4, CASE WHEN $4::uuid IS NULL THEN NULL ELSE 0 END, $5, '2026-11-16T10:00:00.123Z', $6)",
    )
    .bind(ws)
    .bind(ex)
    .bind(task)
    .bind(run)
    .bind(level)
    .bind(msg)
    .execute(pool)
    .await
    .unwrap();
}
async fn lines_of(pool: &sqlx::PgPool, ex: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id")
        .bind(ex)
        .fetch_all(pool)
        .await
        .unwrap()
}
async fn text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_answers_204_and_is_audited_with_names_and_no_value(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;

    // Act
    let res = common::delete(&app, &exec_path(ws, ex), user).await;

    // Assert
    assert_eq!(res.status(), 204);
    assert!(!exists(&pool, ex).await);
    assert_eq!(
        common::get(&app, &exec_path(ws, ex), user).await.status(),
        404
    );
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:delete".to_string(),
            Some(ex.to_string()),
            json!({
                "namespace": "t", "flow_key": "one", "state": "SUCCESS",
                "delete_storage": true, "executions": 1, "files": 0, "detached": 0
            })
        )]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_flags_follow_kestra_and_delete_logs_false_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    let msg = expect_error(
        common::delete(
            &app,
            &format!("{}?deleteLogs=false", exec_path(ws, ex)),
            user,
        )
        .await,
        400,
        Some("delete_logs_required"),
    )
    .await;
    assert!(msg.contains("deleteLogs"), "{msg}");
    assert_eq!(
        common::delete(
            &app,
            &format!("{}?deleteStorage=maybe", exec_path(ws, ex)),
            user
        )
        .await
        .status(),
        400
    );
    assert!(exists(&pool, ex).await);
    let res = common::delete(
        &app,
        &format!(
            "{}?deleteStorage=false&deleteLogs=true&deleteMetrics=false",
            exec_path(ws, ex)
        ),
        user,
    )
    .await;
    assert_eq!(res.status(), 204);
    let audit = common::audit_mutations(&app);
    assert_eq!(audit.last().unwrap().2["delete_storage"], json!(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_refusals_are_coded_with_details(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    // not_terminated: RUNNING.
    let running = start(&pool, ws, &one_task("run")).await;
    let msg = expect_error(
        common::delete(&app, &exec_path(ws, running), user).await,
        409,
        Some("not_terminated"),
    )
    .await;
    assert!(msg.contains("RUNNING") && msg.contains("kill"), "{msg}");
    // not_a_root: a loop iteration, `details.root`.
    let root = ended(&pool, ws, LOOPY).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 LIMIT 1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    let res = common::delete(&app, &exec_path(ws, iteration), user).await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "not_a_root");
    assert_eq!(body["details"]["root"], json!(root));
    // has_dependents: a whole replay of `one`, `details.executions`.
    let one = ended(&pool, ws, &one_task("one")).await;
    let res = common::post_no_body(&app, &format!("{}/replay", exec_path(ws, one)), user).await;
    assert!(res.status().is_success(), "{}", res.status());
    let replay: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE replayed_from_execution_id = $1",
    )
    .bind(one)
    .fetch_one(&pool)
    .await
    .unwrap();
    settle_everything(&pool, &handlers()).await;
    let res = common::delete(&app, &exec_path(ws, one), user).await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "has_dependents");
    assert_eq!(body["details"], json!({"executions": [replay], "total": 1}));
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains(&replay.to_string()));
    // Nothing deleted, nothing audited beyond the replay; then the controls.
    assert!(
        exists(&pool, running).await && exists(&pool, iteration).await && exists(&pool, one).await
    );
    let actions: Vec<String> = common::audit_mutations(&app)
        .into_iter()
        .map(|(action, _, _)| action)
        .collect();
    assert_eq!(actions, vec!["workflow_execution:replay".to_string()]);
    assert_eq!(
        common::delete(&app, &exec_path(ws, replay), user)
            .await
            .status(),
        204
    );
    assert_eq!(
        common::delete(&app, &exec_path(ws, one), user)
            .await
            .status(),
        204
    );
    assert_eq!(
        common::delete(&app, &exec_path(ws, root), user)
            .await
            .status(),
        204
    );
    assert!(
        !exists(&pool, iteration).await,
        "the iteration went with its root"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_trashed_flow_does_not_block_a_delete(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE workspace_id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        common::delete(&app, &exec_path(ws, ex), user)
            .await
            .status(),
        204
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn workflow_read_cannot_delete_relabel_or_delete_logs_but_can_download(pool: sqlx::PgPool) {
    // Arrange
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    log_line(&pool, ws, ex, None, None, 2, "kept").await;

    // Act + Assert
    assert_eq!(
        common::delete(&app, &exec_path(ws, ex), user)
            .await
            .status(),
        403
    );
    assert_eq!(
        common::post(&app, &labels_path(ws, ex), user, json!([]))
            .await
            .status(),
        403
    );
    assert_eq!(
        common::delete(&app, &logs_path(ws, ex), user)
            .await
            .status(),
        403
    );
    assert_eq!(
        common::get(&app, &download_path(ws, ex), user)
            .await
            .status(),
        200
    );
    assert!(exists(&pool, ex).await);
    assert_eq!(lines_of(&pool, ex).await, vec!["kept"]);
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_gets_404_on_all_four(pool: sqlx::PgPool) {
    // Both grants before the app: its Cedar PDP loads them when built.
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let (other_ws, other) = common::seed_workspace(&db).await;
    common::grant(&db, other_ws, other, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    log_line(&pool, ws, ex, None, None, 2, "kept").await;
    assert_eq!(
        common::delete(&app, &exec_path(other_ws, ex), other)
            .await
            .status(),
        404
    );
    assert_eq!(
        common::post(&app, &labels_path(other_ws, ex), other, json!([]))
            .await
            .status(),
        404
    );
    assert_eq!(
        common::delete(&app, &logs_path(other_ws, ex), other)
            .await
            .status(),
        404
    );
    assert_eq!(
        common::get(&app, &download_path(other_ws, ex), other)
            .await
            .status(),
        404
    );
    assert!(exists(&pool, ex).await);
    assert_eq!(lines_of(&pool, ex).await, vec!["kept"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn labels_replace_the_visible_list_and_are_audited_by_key(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;

    // Act: the flow's `team` removed, `env` added.
    let res = common::post(
        &app,
        &labels_path(ws, ex),
        user,
        json!([{"key": "env", "value": "dev"}]),
    )
    .await;

    // Assert
    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(
        body["effective_labels"],
        json!([{"key": "env", "value": "dev"}])
    );
    assert_eq!(body["idle"], json!(true));
    let audit = common::audit_mutations(&app);
    assert_eq!(
        audit,
        vec![(
            "workflow_execution:labels".to_string(),
            Some(ex.to_string()),
            json!({"keys_before": ["team"], "keys_after": ["env"]})
        )]
    );
    assert!(
        !audit[0].2.to_string().contains("dev"),
        "no value in the trail"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn labels_refusals_are_coded(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let running = start(&pool, ws, &one_task("run")).await;
    expect_error(
        common::post(&app, &labels_path(ws, running), user, json!([])).await,
        409,
        Some("not_terminated"),
    )
    .await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    for (body, needle) in [
        (json!([{"key": "Env", "value": "dev"}]), "Env"),
        (json!([{"key": "env", "value": ""}]), "env"),
        (
            json!([{"key": "system.replay", "value": "true"}]),
            "system.replay",
        ),
        (
            json!([{"key": "a", "value": "1"}, {"key": "a", "value": "2"}]),
            "`a`",
        ),
    ] {
        let msg = expect_error(
            common::post(&app, &labels_path(ws, ex), user, body).await,
            400,
            Some("invalid_labels"),
        )
        .await;
        assert!(msg.contains(needle), "{msg} should name {needle}");
    }
    // A body that is not a list: 400, no code.
    expect_error(
        common::post(&app, &labels_path(ws, ex), user, json!({"key": "a"})).await,
        400,
        None,
    )
    .await;
    assert!(common::audit_mutations(&app).is_empty());
    // Control: the unchanged list passes.
    assert_eq!(
        common::post(
            &app,
            &labels_path(ws, ex),
            user,
            json!([{"key": "team", "value": "data"}])
        )
        .await
        .status(),
        200
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_detail_carries_idle(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let idle = |v: Value| v["idle"].clone();
    // Read before `ended`, whose settle would end this one too.
    let running = start(&pool, ws, &one_task("run")).await;
    assert_eq!(
        idle(common::json(common::get(&app, &exec_path(ws, running), user).await).await),
        json!(false)
    );
    let ex = ended(&pool, ws, &one_task("one")).await;
    assert_eq!(
        idle(common::json(common::get(&app, &exec_path(ws, ex), user).await).await),
        json!(true)
    );
    // Ended with a live run: not idle.
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'RUNNING', ended_at = NULL WHERE execution_id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        idle(common::json(common::get(&app, &exec_path(ws, ex), user).await).await),
        json!(false)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_download_is_kestras_text_with_filters_and_headers(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    let run = Uuid::new_v4();
    log_line(&pool, ws, ex, None, None, 2, "one").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 4, "two").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 1, "three").await;

    // Act
    let res = common::get(&app, &download_path(ws, ex), user).await;

    // Assert
    assert_eq!(res.status(), 200);
    let h = res.headers().clone();
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert_eq!(
        h["content-disposition"],
        format!("attachment; filename=\"{ex}.log\"")
    );
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert!(
        h.get("cache-control").is_none(),
        "an ended execution may be cached"
    );
    assert_eq!(
        text(res).await,
        "2026-11-16T10:00:00.123Z INFO one\n2026-11-16T10:00:00.123Z ERROR two\n2026-11-16T10:00:00.123Z DEBUG three"
    );
    let res = common::get(
        &app,
        &format!("{}?task_run_id={run}&min_level=WARN", download_path(ws, ex)),
        user,
    )
    .await;
    assert_eq!(text(res).await, "2026-11-16T10:00:00.123Z ERROR two");
    // Not idle: no-cache. An empty log: an empty body.
    let running = start(&pool, ws, &one_task("run")).await;
    let res = common::get(&app, &download_path(ws, running), user).await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(text(res).await, "");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_download_stops_at_100000_lines_and_says_so(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    sqlx::query(
        "INSERT INTO workflow.task_logs (workspace_id, execution_id, level, ts, message) \
         SELECT $1, $2, 2, '2026-11-16T10:00:00Z', 'l' || g FROM generate_series(1, 100001) g",
    )
    .bind(ws)
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    // Over the cap: 100 000 lines, then the truncation line.
    let body = text(common::get(&app, &download_path(ws, ex), user).await).await;
    let lines: Vec<&str> = body.split('\n').collect();
    assert_eq!(lines.len(), 100_001);
    assert_eq!(lines[0], "2026-11-16T10:00:00.000Z INFO l1");
    assert_eq!(lines[99_999], "2026-11-16T10:00:00.000Z INFO l100000");
    assert_eq!(lines[100_000], "… truncated at 100000 lines");
    // Exactly at the cap: no truncation line.
    sqlx::query("DELETE FROM workflow.task_logs WHERE execution_id = $1 AND message = 'l100001'")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let body = text(common::get(&app, &download_path(ws, ex), user).await).await;
    assert_eq!(body.split('\n').count(), 100_000);
    assert!(body.ends_with("INFO l100000"), "no trailing newline");
    assert!(!body.contains("truncated"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn log_delete_ands_its_filters_works_while_running_and_is_audited(pool: sqlx::PgPool) {
    // Arrange: a running execution (Q6).
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_task("run")).await;
    let run = Uuid::new_v4();
    log_line(&pool, ws, ex, None, None, 4, "exec-error").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 3, "a-warn").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 2, "a-info").await;

    // Act
    let res = common::delete(
        &app,
        &format!("{}?task_run_id={run}&min_level=WARN", logs_path(ws, ex)),
        user,
    )
    .await;

    // Assert
    assert_eq!(res.status(), 204);
    assert_eq!(lines_of(&pool, ex).await, vec!["exec-error", "a-info"]);
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:logs_delete".to_string(),
            Some(ex.to_string()),
            json!({"task_id": null, "task_run_id": run, "attempt": null, "min_level": "WARN", "deleted": 1})
        )]
    );
    assert_eq!(
        common::delete(&app, &logs_path(ws, ex), user)
            .await
            .status(),
        204
    );
    assert!(lines_of(&pool, ex).await.is_empty());
    assert_eq!(
        common::delete(&app, &format!("{}?min_level=LOUD", logs_path(ws, ex)), user)
            .await
            .status(),
        400
    );
}

/// tests/pause_resume.rs:75-82.
fn subflow_parent(id: &str, target: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
         flowId: {target}\n  - id: after\n    \
         type: io.kestra.plugin.core.log.Log\n    message: after\n"
    )
}

/// `has_dependents` for a subflow parent, through the route (D12).
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_parent_answers_has_dependents(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    flow_of(&pool, ws, &one_task("child")).await;
    let parent = ended(&pool, ws, &subflow_parent("parent", "child")).await;
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Act
    let res = common::delete(&app, &exec_path(ws, parent), user).await;

    // Assert
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "has_dependents");
    assert_eq!(body["details"], json!({"executions": [child], "total": 1}));
    assert!(exists(&pool, parent).await && exists(&pool, child).await);
    // Control: the child first, then the parent.
    assert_eq!(
        common::delete(&app, &exec_path(ws, child), user)
            .await
            .status(),
        204
    );
    assert_eq!(
        common::delete(&app, &exec_path(ws, parent), user)
            .await
            .status(),
        204
    );
}

/// The workflows entitlement gates the four routes (as `restart_routes.rs:221-231`).
#[sqlx::test(migrations = "../../migrations")]
async fn the_workflows_entitlement_gates_the_four_routes(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    log_line(&pool, ws, ex, None, None, 2, "kept").await;
    common::disable_feature(&db, ws, "workflows").await;

    assert_eq!(
        common::delete(&app, &exec_path(ws, ex), user)
            .await
            .status(),
        403
    );
    assert_eq!(
        common::post(&app, &labels_path(ws, ex), user, json!([]))
            .await
            .status(),
        403
    );
    assert_eq!(
        common::delete(&app, &logs_path(ws, ex), user)
            .await
            .status(),
        403
    );
    assert_eq!(
        common::get(&app, &download_path(ws, ex), user)
            .await
            .status(),
        403
    );
    assert!(exists(&pool, ex).await);
    assert_eq!(lines_of(&pool, ex).await, vec!["kept"]);
}
