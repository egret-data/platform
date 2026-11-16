//! Delete an ended execution and its family through the worker entry point
//! (workflow package 7d, spec
//! `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md` Q1, Q2,
//! Q5, D1-D3; plan corrections S2, S4, S5, S8, S9; Kestra
//! `ExecutionService.delete`, `ES:747-767`).
//!
//! Every delete test owns its store: a `MemStore` built here, never the
//! installed global. Every test is `#[sqlx::test]`: the worker's `claim` and
//! `tick` are global, so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test delete_labels

use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::{MemStore, ObjectStore, StorageError};
use egret_workflow::storage::{Caps, FileStore};
use egret_workflow::worker::{
    delete_execution, set_execution_labels, DeleteRefused, DeleteRequest, Handlers, LabelsRefused,
    LabelsSet, Registry,
};
use uuid::Uuid;

mod common;

// ── Fixtures (from `tests/pause_resume.rs` and `tests/pause_resume_routes.rs`)

fn one_task(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

fn subflow_parent(id: &str, target: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
         flowId: {target}\n{extra}  - id: after\n    \
         type: io.kestra.plugin.core.log.Log\n    message: after\n"
    )
}

/// A `Loop` of two iterations, then a top-level task.
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

async fn file_flow(db: &Db, ws: Uuid, by: egret_core::UserId, source: &str) -> Uuid {
    let flow = egret_workflow::spec::parse(source).expect("the fixture flow parses");
    let input = egret_workflow::CreateFlowInput {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        source: source.to_string(),
        plan: serde_json::to_value(&flow).expect("a parsed flow serialises"),
        folder_id: None,
    };
    egret_workflow::FlowRepo::new(db.clone())
        .create(ws, by, &input)
        .await
        .expect("file the flow")
        .id
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

async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    (db, ws, user)
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("state")
}

/// The core tasks run `Log` only here; nothing they do touches a store.
fn handlers() -> Handlers {
    Handlers::with_core_tasks(egret_workflow::storage::current())
}

/// What the store holds under `prefix` (an `s3://bucket/...` URI, as
/// `FileStore`'s prefixes are): `list` takes the key, without the bucket.
async fn listed_under(fs: &FileStore, prefix: &str) -> egret_storage::Listing {
    let key = prefix
        .strip_prefix("s3://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, k)| k)
        .expect("an s3:// URI");
    fs.store.list(key, None).await.unwrap()
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn start(
    pool: &sqlx::PgPool,
    db: &Db,
    ws: Uuid,
    user: egret_core::UserId,
    src: &str,
) -> Uuid {
    let flow = file_flow(db, ws, user, src).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), src, serde_json::json!({}))
        .await
        .expect("start")
}

/// This test's own store (never the installed one).
fn mem_fs(store: Arc<dyn ObjectStore>) -> FileStore {
    FileStore {
        store,
        bucket: "egret-7d".to_string(),
        prefix: "workflow/".to_string(),
        caps: Caps::default(),
        retention_days: 30,
    }
}

/// A store whose `delete_prefix` is down; everything else is a `MemStore`.
struct NoDelete(MemStore);

#[async_trait::async_trait]
impl ObjectStore for NoDelete {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.0.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.0.list(prefix, delimiter).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        Err(StorageError::S3(format!(
            "{uri}: dispatch failure: connection refused"
        )))
    }
}

fn req(ws: Uuid, id: Uuid, delete_storage: bool) -> DeleteRequest {
    DeleteRequest {
        workspace: ws,
        id,
        delete_storage,
    }
}

async fn exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workflow.executions WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Each member's (id, flow_key): the root and its iterations.
async fn members(pool: &sqlx::PgPool, root: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as(
        "SELECT id, flow_key FROM workflow.executions WHERE id = $1 OR parent_execution_id = $1 ORDER BY id",
    )
    .bind(root)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// One object under the member's own prefix and one under its `_wd/` root.
async fn seed_objects(fs: &FileStore, ws: Uuid, id: Uuid, flow_key: &str) -> (String, String) {
    let own = fs.execution_prefix(ws, "t", flow_key, id);
    let wd = fs.working_dir_root(ws, id);
    fs.store
        .put(&format!("{own}tasks/a/f.txt"), b"x".to_vec())
        .await
        .unwrap();
    fs.store
        .put(&format!("{wd}blk/f.txt"), b"x".to_vec())
        .await
        .unwrap();
    (own, wd)
}

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_ended_loop_root_is_deleted_with_its_family_rows_and_objects(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let root = start(&pool, &db, ws, user, LOOPY).await;
    let other = start(&pool, &db, ws, user, &one_task("other")).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let family = members(&pool, root).await;
    assert_eq!(family.len(), 3, "the root and two iterations");
    let mut prefixes = Vec::new();
    for (id, flow_key) in &family {
        prefixes.push(seed_objects(&fs, ws, *id, flow_key).await);
    }
    let (kept, _) = seed_objects(&fs, ws, other, "other").await;

    // Act
    let done = delete_execution(&pool, Some(&fs), req(ws, root, true))
        .await
        .expect("delete")
        .expect("not refused");

    // Assert
    assert_eq!(
        (done.executions, done.state.as_str(), done.flow_key.as_str()),
        (3, "SUCCESS", "loopy")
    );
    for (id, _) in &family {
        assert!(!exists(&pool, *id).await, "{id} is gone");
    }
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = ANY($1)")
            .bind(family.iter().map(|(id, _)| *id).collect::<Vec<_>>())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(runs, 0);
    for (own, wd) in &prefixes {
        assert!(listed_under(&fs, own).await.objects.is_empty(), "{own}");
        assert!(listed_under(&fs, wd).await.objects.is_empty(), "{wd}");
    }
    assert_eq!(
        listed_under(&fs, &kept).await.objects.len(),
        1,
        "another execution's files stay"
    );
    assert!(exists(&pool, other).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_storage_false_keeps_the_objects(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    let (own, wd) = seed_objects(&fs, ws, ex, "one").await;

    delete_execution(&pool, Some(&fs), req(ws, ex, false))
        .await
        .unwrap()
        .unwrap();

    assert!(!exists(&pool, ex).await);
    assert_eq!(listed_under(&fs, &own).await.objects.len(), 1);
    assert_eq!(listed_under(&fs, &wd).await.objects.len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_store_failure_deletes_no_row_and_can_be_repeated(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let down = mem_fs(Arc::new(NoDelete(MemStore::new())));
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    seed_objects(&down, ws, ex, "one").await;

    // Act
    let err = delete_execution(&pool, Some(&down), req(ws, ex, true))
        .await
        .unwrap_err();

    // Assert
    assert!(matches!(err, egret_core::Error::Internal(_)), "{err}");
    assert!(exists(&pool, ex).await, "the row stays");
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(runs, 1, "and its runs");
    // Repeated against a working store, it goes.
    let up = mem_fs(Arc::new(MemStore::new()));
    delete_execution(&pool, Some(&up), req(ws, ex, true))
        .await
        .unwrap()
        .unwrap();
    assert!(!exists(&pool, ex).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_running_execution_is_not_idle_and_is_kept(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await; // not ticked: RUNNING
    let got = delete_execution(&pool, None, req(ws, ex, true))
        .await
        .unwrap();
    assert_eq!(
        got,
        Err(DeleteRefused::NotIdle {
            execution: ex,
            state: "RUNNING".into()
        })
    );
    assert!(exists(&pool, ex).await);
}

/// Idle, not terminal (Q1): an ended execution with a live run (an
/// `afterExecution:` run, an `Exit` sibling) is refused, naming its state.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ended_execution_with_a_live_run_is_not_idle(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'RUNNING', ended_at = NULL WHERE execution_id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    let got = delete_execution(&pool, None, req(ws, ex, true))
        .await
        .unwrap();
    assert_eq!(
        got,
        Err(DeleteRefused::NotIdle {
            execution: ex,
            state: "SUCCESS".into()
        })
    );
}

/// The family is checked, not the root alone (S2).
#[sqlx::test(migrations = "../../migrations")]
async fn a_live_iteration_keeps_its_root(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let root = start(&pool, &db, ws, user, LOOPY).await;
    settle_everything(&pool, &handlers()).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 ORDER BY loop_index LIMIT 1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'RUNNING', ended_at = NULL WHERE execution_id = $1",
    )
    .bind(iteration)
    .execute(&pool)
    .await
    .unwrap();
    let got = delete_execution(&pool, None, req(ws, root, true))
        .await
        .unwrap();
    assert_eq!(
        got,
        Err(DeleteRefused::NotIdle {
            execution: iteration,
            state: "SUCCESS".into()
        })
    );
    assert!(exists(&pool, root).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_is_not_a_root(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let root = start(&pool, &db, ws, user, LOOPY).await;
    settle_everything(&pool, &handlers()).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 LIMIT 1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    let got = delete_execution(&pool, None, req(ws, iteration, true))
        .await
        .unwrap();
    assert_eq!(got, Err(DeleteRefused::NotARoot { root }));
    assert!(exists(&pool, iteration).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_parent_has_its_child_as_a_dependent_until_the_child_goes(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    file_flow(&db, ws, user, &one_task("child")).await;
    let parent = start(&pool, &db, ws, user, &subflow_parent("parent", "child", "")).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, parent).await, "SUCCESS");
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Act + Assert: refused, naming the child; the child first; then the parent.
    let got = delete_execution(&pool, None, req(ws, parent, true))
        .await
        .unwrap();
    assert_eq!(
        got,
        Err(DeleteRefused::HasDependents {
            executions: vec![child],
            total: 1
        })
    );
    assert!(exists(&pool, parent).await);
    delete_execution(&pool, None, req(ws, child, true))
        .await
        .unwrap()
        .unwrap();
    delete_execution(&pool, None, req(ws, parent, true))
        .await
        .unwrap()
        .unwrap();
    assert!(!exists(&pool, parent).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_is_a_dependent_of_what_it_replayed(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let src = one_task("one");
    let original = start(&pool, &db, ws, user, &src).await;
    settle_everything(&pool, &handlers()).await;
    let replay = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: original,
            source: &src,
            kind: egret_workflow::worker::ReplayKind::Whole,
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: user.0,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");
    settle_everything(&pool, &handlers()).await;

    // Act + Assert
    assert_eq!(
        delete_execution(&pool, None, req(ws, original, true))
            .await
            .unwrap(),
        Err(DeleteRefused::HasDependents {
            executions: vec![replay],
            total: 1
        })
    );
    assert!(exists(&pool, original).await);
    delete_execution(&pool, None, req(ws, replay, true))
        .await
        .unwrap()
        .unwrap();
    delete_execution(&pool, None, req(ws, original, true))
        .await
        .unwrap()
        .unwrap();
    assert!(!exists(&pool, original).await);
}

/// S9 (user decision (a)).
#[sqlx::test(migrations = "../../migrations")]
async fn s9_a_pending_flow_trigger_event_refuses_the_delete(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    sqlx::query(
        "INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state) \
         VALUES ($1, $2, 'SUCCESS')",
    )
    .bind(ex)
    .bind(ws)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        delete_execution(&pool, None, req(ws, ex, true))
            .await
            .unwrap(),
        Err(DeleteRefused::ListenersPending)
    );
    sqlx::query("DELETE FROM workflow.flow_trigger_events WHERE source_execution_id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    delete_execution(&pool, None, req(ws, ex, true))
        .await
        .unwrap()
        .unwrap();
}

/// S8 (user decision (b)): a Flow-trigger fire is not a dependent. The
/// source is deleted; the fire is kept, its `trigger_execution_id` cleared,
/// and its trigger record still names the source as data.
#[sqlx::test(migrations = "../../migrations")]
async fn s8_a_flow_trigger_fire_is_detached_not_a_dependent(pool: sqlx::PgPool) {
    // Arrange: a listener that fires on `producer`'s end (the fixtures of
    // tests/flow_trigger_fire.rs:47-73; `seed_flow` registers the listener).
    let (db, ws, user) = setup(&pool).await;
    let producer_src = "id: producer\nnamespace: w13\ntasks:\n  \
                        - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let listener_src = "id: listener\nnamespace: w13\ntriggers:\n  \
                        - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
                        when: \"{{ flow.id == 'producer' }}\"\ntasks:\n  \
                        - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let producer = common::seed_flow(&db, ws, user, producer_src, "draft").await;
    common::seed_flow(&db, ws, user, listener_src, "draft").await;
    let source = egret_workflow::worker::start_execution(
        &pool,
        ws,
        Some(producer),
        producer_src,
        serde_json::json!({}),
    )
    .await
    .expect("start the source");
    settle_everything(&pool, &handlers()).await;
    let report = egret_workflow::flow_trigger::process_events(&pool)
        .await
        .expect("a pass");
    assert_eq!(report.fired, 1, "control: the listener fired");
    let fired: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE trigger_execution_id = $1 AND flow_trigger_event_id IS NOT NULL",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .expect("the fire");
    settle_everything(&pool, &handlers()).await;
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.flow_trigger_events WHERE source_execution_id = $1",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        left, 0,
        "control: the fire consumed the source's event (else S9 would refuse)"
    );

    // Act
    let done = delete_execution(&pool, None, req(ws, source, true))
        .await
        .unwrap()
        .expect("a fire does not block the delete");

    // Assert
    assert_eq!(done.detached, 1);
    assert!(!exists(&pool, source).await);
    assert!(exists(&pool, fired).await, "the fire is kept");
    let (link, record): (Option<Uuid>, serde_json::Value) = sqlx::query_as(
        "SELECT trigger_execution_id, execution_trigger FROM workflow.executions WHERE id = $1",
    )
    .bind(fired)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(link, None);
    assert!(
        record.to_string().contains(&source.to_string()),
        "the record still names the source: {record}"
    );
}

/// Control for S8: the detach is rolled back when another dependent refuses
/// the delete (a subflow child here), so nothing is half-done.
#[sqlx::test(migrations = "../../migrations")]
async fn s8_a_refused_delete_keeps_the_fires_link(pool: sqlx::PgPool) {
    // Arrange: the S8 test's listener, listening to a subflow parent.
    let (db, ws, user) = setup(&pool).await;
    file_flow(&db, ws, user, &one_task("child")).await;
    let parent_src = subflow_parent("parent", "child", "");
    let parent_flow = file_flow(&db, ws, user, &parent_src).await;
    let listener_src = "id: listener\nnamespace: t\ntriggers:\n  \
                        - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
                        when: \"{{ flow.id == 'parent' }}\"\ntasks:\n  \
                        - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    common::seed_flow(&db, ws, user, listener_src, "draft").await;
    let parent = egret_workflow::worker::start_execution(
        &pool,
        ws,
        Some(parent_flow),
        &parent_src,
        serde_json::json!({}),
    )
    .await
    .unwrap();
    settle_everything(&pool, &handlers()).await;
    egret_workflow::flow_trigger::process_events(&pool)
        .await
        .unwrap();
    settle_everything(&pool, &handlers()).await;
    let child: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE trigger_execution_id = $1 AND flow_trigger_event_id IS NULL",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Act
    let got = delete_execution(&pool, None, req(ws, parent, true))
        .await
        .unwrap();

    // Assert: refused for the child alone; the fire still points at the parent.
    assert_eq!(
        got,
        Err(DeleteRefused::HasDependents {
            executions: vec![child],
            total: 1
        })
    );
    let still: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions \
          WHERE trigger_execution_id = $1 AND flow_trigger_event_id IS NOT NULL",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_cannot_delete(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    let err = delete_execution(&pool, None, req(other_ws, ex, true))
        .await
        .unwrap_err();
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err}");
    assert!(exists(&pool, ex).await);
}

/// The family includes a `CREATE_NEW_EXECUTION` retry replacement (S2):
/// it is not a root, it cascades with the original, and its objects go too.
#[sqlx::test(migrations = "../../migrations")]
async fn a_retry_replacement_goes_with_its_root(pool: sqlx::PgPool) {
    // Arrange: tests/execution_timestamps.rs:469-498's flow and its `Fails`
    // registry (copied below), so the original fails and is replaced once.
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let src = "id: cne\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
               \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
               \x20     type: constant\n      interval: PT1S\n      maxAttempts: 2\n";
    let original = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &Fails).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    egret_workflow::worker::wake(&pool, 16).await.expect("wake");
    settle_everything(&pool, &Fails).await;
    let replacement: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(original)
    .fetch_one(&pool)
    .await
    .expect("the replacement exists");
    let (own_o, _) = seed_objects(&fs, ws, original, "cne").await;
    let (own_r, _) = seed_objects(&fs, ws, replacement, "cne").await;

    // Act + Assert: the replacement alone is refused, naming the original.
    assert_eq!(
        delete_execution(&pool, Some(&fs), req(ws, replacement, true))
            .await
            .unwrap(),
        Err(DeleteRefused::NotARoot { root: original })
    );
    let done = delete_execution(&pool, Some(&fs), req(ws, original, true))
        .await
        .unwrap()
        .expect("deleted");
    assert_eq!(done.executions, 2);
    assert!(!exists(&pool, original).await && !exists(&pool, replacement).await);
    assert!(listed_under(&fs, &own_o).await.objects.is_empty());
    assert!(listed_under(&fs, &own_r).await.objects.is_empty());
}

/// The family is locked to a fixpoint (fix round 1): a member inserted under
/// a not-yet-locked member while the delete reads the family is found, so
/// it is idle-checked, its objects go, and it is counted.
///
/// Deterministic: a second transaction holds the replacement `Y1`'s row
/// (as the wake that settles it does) and inserts `Y2` under it,
/// uncommitted. The delete locks the root, reads the family without `Y2`,
/// and waits on `Y1`'s lock; the test sees it waiting in `pg_stat_activity`
/// (this test's database only) and only then commits `Y2`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_member_inserted_while_the_family_is_read_is_deleted_with_it(pool: sqlx::PgPool) {
    // Arrange: R -> Y1 (a CREATE_NEW_EXECUTION replacement), all ended.
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let src = "id: cne\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
               \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
               \x20     type: constant\n      interval: PT1S\n      maxAttempts: 2\n";
    let root = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &Fails).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    egret_workflow::worker::wake(&pool, 16).await.expect("wake");
    settle_everything(&pool, &Fails).await;
    let y1: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .expect("the replacement exists");
    let y2 = Uuid::new_v4();
    let (own_y2, wd_y2) = seed_objects(&fs, ws, y2, "cne").await;

    // The "wake": Y1's row held, Y2 inserted under it (a copy of Y1's row
    // with Y1 as its parent), not committed.
    let mut wake = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR NO KEY UPDATE")
        .bind(y1)
        .execute(&mut *wake)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workflow.executions \
         SELECT (jsonb_populate_record(e, jsonb_build_object('id', $2::uuid, 'parent_execution_id', $1::uuid))).* \
           FROM workflow.executions e WHERE e.id = $1",
    )
    .bind(y1)
    .bind(y2)
    .execute(&mut *wake)
    .await
    .expect("insert Y2 under Y1");

    // Act: the delete runs until it waits on Y1's lock.
    let deleting = {
        let pool = pool.clone();
        let fs = mem_fs(fs.store.clone());
        tokio::spawn(async move { delete_execution(&pool, Some(&fs), req(ws, root, true)).await })
    };
    let mut waited = false;
    for _ in 0..500 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND wait_event_type = 'Lock' \
                AND query LIKE '%FOR UPDATE%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if n > 0 {
            waited = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(waited, "control: the delete waited on Y1's lock");
    wake.commit().await.unwrap();
    let done = deleting
        .await
        .unwrap()
        .expect("delete")
        .expect("not refused");

    // Assert
    assert_eq!(done.executions, 3, "Y2 is a member");
    for id in [root, y1, y2] {
        assert!(!exists(&pool, id).await, "{id} is gone");
    }
    assert!(
        listed_under(&fs, &own_y2).await.objects.is_empty(),
        "{own_y2}"
    );
    assert!(
        listed_under(&fs, &wd_y2).await.objects.is_empty(),
        "{wd_y2}"
    );
}

/// tests/execution_timestamps.rs:56-70, without the sleep.
struct Fails;

#[async_trait::async_trait]
impl Registry for Fails {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        Ok(egret_workflow::worker::Outcome::failed_with(
            "this fixture always fails",
        ))
    }
    async fn cancel(&self, task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing here has a timeout, so {} is never killed",
            task.task_id
        );
    }
}

// ── Labels (T4) ─────────────────────────────────────────────────────────────

const LABELLED: &str = "id: labelled\nnamespace: t\nlabels:\n  team: data\n  Team2: x\ntasks:\n  \
     - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

fn label(k: &str, v: &str) -> egret_workflow::models::Label {
    egret_workflow::models::Label {
        key: k.into(),
        value: v.into(),
    }
}

async fn stored(pool: &sqlx::PgPool, ex: Uuid) -> (Option<serde_json::Value>, bool) {
    sqlx::query_as("SELECT labels, labels_replaced FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replace_removes_a_flow_label_and_keeps_the_system_ones(pool: sqlx::PgPool) {
    // Arrange: ended, and restarted once (the engine's own label).
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    sqlx::query("UPDATE workflow.executions SET labels = $2 WHERE id = $1")
        .bind(ex)
        .bind(serde_json::json!([{"key": "system.restarted", "value": "true"}]))
        .execute(&pool)
        .await
        .unwrap();

    // Act: `team` is left out, `Team2` (a flow key outside Kestra's pattern) resent.
    let got = set_execution_labels(&pool, ws, ex, &[label("Team2", "x"), label("env", "dev")])
        .await
        .unwrap();

    // Assert
    assert_eq!(
        got,
        Ok(LabelsSet {
            keys_before: vec!["team".into(), "Team2".into(), "system.restarted".into()],
            keys_after: vec!["Team2".into(), "env".into(), "system.restarted".into()],
        })
    );
    assert_eq!(
        stored(&pool, ex).await,
        (
            Some(serde_json::json!([
                {"key": "Team2", "value": "x"},
                {"key": "env", "value": "dev"},
                {"key": "system.restarted", "value": "true"}
            ])),
            true
        )
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_replace_works_on_the_replaced_list(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    set_execution_labels(&pool, ws, ex, &[label("env", "dev")])
        .await
        .unwrap()
        .unwrap();
    // `team` is no longer visible, so a new key `Team` must match the pattern.
    let got = set_execution_labels(&pool, ws, ex, &[label("Team", "x")])
        .await
        .unwrap();
    assert!(
        matches!(got, Err(LabelsRefused::Invalid(ref m)) if m.contains("Team")),
        "{got:?}"
    );
    let got = set_execution_labels(&pool, ws, ex, &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.keys_before, vec!["env".to_string()]);
    assert_eq!(stored(&pool, ex).await, (Some(serde_json::json!([])), true));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_running_execution_cannot_be_relabelled(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await; // RUNNING
    let got = set_execution_labels(&pool, ws, ex, &[label("env", "dev")])
        .await
        .unwrap();
    assert_eq!(got, Err(LabelsRefused::NotIdle("RUNNING".into())));
    assert_eq!(stored(&pool, ex).await, (None, false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_changed_or_new_system_label_is_refused_and_nothing_is_written(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    for body in [
        vec![label("system.restarted", "true")],
        vec![label("system.replay", "x")],
    ] {
        let got = set_execution_labels(&pool, ws, ex, &body).await.unwrap();
        assert!(
            matches!(got, Err(LabelsRefused::Invalid(ref m)) if m.contains(&body[0].key)),
            "{got:?}"
        );
    }
    assert_eq!(stored(&pool, ex).await, (None, false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_cannot_relabel(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    let err = set_execution_labels(&pool, other_ws, ex, &[])
        .await
        .unwrap_err();
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err}");
}

/// Fix round 1: the idle check runs after the lock wait, on a fresh snapshot.
/// A transaction holds the row and makes it RUNNING (a restart); the labels
/// call waits on the lock, then must see RUNNING and refuse.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_committed_during_the_lock_wait_makes_the_labels_refuse(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR NO KEY UPDATE")
        .bind(ex)
        .execute(&mut *holder)
        .await
        .unwrap();
    let setting = {
        let pool = pool.clone();
        tokio::spawn(
            async move { set_execution_labels(&pool, ws, ex, &[label("env", "dev")]).await },
        )
    };
    let mut waited = false;
    for _ in 0..500 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND wait_event_type = 'Lock' \
                AND query LIKE '%FOR NO KEY UPDATE%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if n > 0 {
            waited = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        waited,
        "control: set_execution_labels waited on the row lock"
    );
    sqlx::query("UPDATE workflow.executions SET state = 'RUNNING', ended_at = NULL WHERE id = $1")
        .bind(ex)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();

    let got = setting.await.unwrap().unwrap();

    assert_eq!(got, Err(LabelsRefused::NotIdle("RUNNING".into())));
    assert_eq!(stored(&pool, ex).await, (None, false));
}
