//! Package #4 Task C5 (spec D10): `egret.sql` is served on every deployment.
//!
//! Publish resolves its `database:` through the real `RefKind::Database`
//! resolver (`WorkflowRefs` over `egret_query::DatabaseRepo`, the repo
//! egret-bin hands it), and a run goes through the API start and the real
//! worker over the registry `runtime::handlers_for` builds for
//! `container_runner = none`, against a stub `QueryExecPort`. The real port
//! against a real engine is `tests/sql_starrocks_live.rs`'s.
//!
//! Its own binary, because it installs the `none` set. DB-gated; self-skips
//! ONLY when `EGRET_ALLOW_DB_SKIP` is set.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egret_core::UserId;
use egret_metastore::Db;
use egret_query_api::{ColumnMeta, Limits, Param, QueryExecPort, TabularResult};
use egret_workflow::runners::secrets::{SecretError, SecretSource};
use serde_json::{json, Value};
use uuid::Uuid;

/// Ticks are global over the database, so the tests that tick take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn served_none() -> &'static egret_workflow::runtime::Served {
    common::install_served_none()
}

struct NoSecrets;

#[async_trait::async_trait]
impl SecretSource for NoSecrets {
    async fn get(&self, _: Uuid, name: &str) -> Result<BTreeMap<String, String>, SecretError> {
        Err(SecretError::NotFound { name: name.into() })
    }
}

/// What the stub port was asked: `(ws, database, principal, sql, params)`.
type Call = (Uuid, Uuid, UserId, String, Vec<Param>);

/// A `QueryExecPort` that records each call and answers one row, `n = 42`.
#[derive(Default)]
struct Stub {
    calls: Mutex<Vec<Call>>,
}

#[async_trait::async_trait]
impl QueryExecPort for Stub {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> egret_core::Result<Vec<ColumnMeta>> {
        unreachable!("egret.sql never describes")
    }
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _limits: Limits,
    ) -> egret_core::Result<TabularResult> {
        unreachable!("egret.sql always goes through `execute_as_with_params`")
    }
    async fn execute_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        params: &[Param],
        _limits: Limits,
    ) -> egret_core::Result<TabularResult> {
        self.calls.lock().unwrap().push((
            ws,
            database_id,
            principal,
            sql.to_string(),
            params.to_vec(),
        ));
        Ok(TabularResult {
            columns: vec![ColumnMeta {
                name: "n".into(),
                ty: "bigint".into(),
            }],
            rows: vec![vec![json!(42)]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }
}

/// A `query.databases` row owned by `ws`. Never connected to: the run test's
/// port is the stub.
async fn register_database(db: &Db, ws: Uuid, by: UserId) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, options, delegated, created_by) \
         VALUES ($1, $2, 'starrocks', '127.0.0.1', 1, '{}'::jsonb, false, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(by.0)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

/// The real resolver egret-bin wires: `RefKind::Database` over
/// `egret_query::DatabaseRepo`.
fn real_refs(db: &Db) -> egret_workflow::WorkflowRefs {
    let databases = egret_query::DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let mut refs = common::workflow_refs();
    refs.databases = Arc::new(databases);
    refs
}

fn sql_flow(ns: &str, database: &str) -> String {
    format!(
        "id: q\nnamespace: {ns}\ntasks:\n  - id: q\n    type: egret.sql\n    \
         database: \"{database}\"\n    fetch: FETCH_ONE\n    \
         sql: \"SELECT n FROM t WHERE ns = {{{{ flow.namespace }}}}\"\n  \
         - id: after\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ outputs.q.row.n }}}}\"\n"
    )
}

fn fresh_ns() -> String {
    format!("ns{}", Uuid::new_v4().simple())
}

/// The `none` set serves `egret.sql`, and so does the registry
/// `handlers_for` builds for it -- the drift test's premise, measured here
/// on the integration side.
#[tokio::test]
async fn the_none_deployment_serves_egret_sql() {
    let s = served_none();
    assert!(s.serves("egret.sql"));
    assert!(!s.serves("egret.report.send"), "control: a deferred row");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap();
    let (h, runner) = egret_workflow::runtime::handlers_for(
        &egret_config::Config::default().workflow,
        pool,
        Arc::new(NoSecrets),
        Arc::new(Stub::default()),
    )
    .await
    .expect("the shipping registry builds");
    assert!(runner.is_none());
    assert_eq!(&egret_workflow::runtime::Served::from_handlers(&h), s);
}

/// Publish, through the real resolver: a database of this workspace resolves
/// and is frozen into the snapshot; another workspace's database, an id that
/// does not exist and a name all fail the same way (`NotFound`), so publish is
/// no probe for which databases exist elsewhere.
#[tokio::test]
async fn publish_resolves_the_database_through_the_real_repo() {
    served_none();
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let own = register_database(&db, ws, user).await;
    let foreign = register_database(&db, other_ws, other_user).await;
    let refs = real_refs(&db);
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let ns = fresh_ns();
    let f = common::draft(&repo, ws, user, &sql_flow(&ns, &own.to_string())).await;
    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect("a database of this workspace publishes");
    assert!(
        v.snapshot.to_string().contains(&own.to_string()),
        "the snapshot carries the resolved id: {}",
        v.snapshot
    );

    for (what, database) in [
        ("another workspace's database", foreign.to_string()),
        ("an id nobody holds", Uuid::new_v4().to_string()),
        ("a name", "warehouse".to_string()),
    ] {
        let ns = fresh_ns();
        let f = common::draft(&repo, ws, user, &sql_flow(&ns, &database)).await;
        let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
            .await
            .expect_err(what);
        assert!(
            matches!(err, egret_core::Error::NotFound(_)),
            "{what}: {err:?}"
        );
    }
}

/// The API start accepts an `egret.sql` flow (no `task_type_not_served`),
/// and the worker over `handlers_for`'s registry runs it: the port is asked
/// for this workspace's database, as the flow's owner, with every `{{ }}`
/// bound; the row lands in the outputs and a later task reads it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_egret_sql_flow_starts_and_runs_against_the_port() {
    served_none();
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let database = register_database(&db, ws, user).await;
    let ns = fresh_ns();
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &sql_flow(&ns, &database.to_string()),
        "draft",
    )
    .await;
    let app = common::app(&db).await;

    let started = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
    )
    .await;
    assert_eq!(started.status(), 201, "the start gate serves egret.sql");
    let ex: Uuid = common::json(started).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let stub = Arc::new(Stub::default());
    let (h, _) = egret_workflow::runtime::handlers_for(
        &egret_config::Config::default().workflow,
        db.pool.clone(),
        Arc::new(NoSecrets),
        stub.clone(),
    )
    .await
    .unwrap();
    for _ in 0..64 {
        let n = egret_workflow::worker::tick(&db.pool, 16, &h)
            .await
            .expect("no worker error");
        if n == 0 {
            break;
        }
    }

    let runs: Vec<(String, String, Option<Value>, Option<String>)> = sqlx::query_as(
        "SELECT task_id, state, outputs, message FROM workflow.task_runs \
          WHERE execution_id = $1 ORDER BY task_id",
    )
    .bind(ex)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let by_id = |id: &str| {
        runs.iter()
            .find(|r| r.0 == id)
            .unwrap_or_else(|| panic!("no run of {id}: {runs:?}"))
            .clone()
    };
    let (_, state, outputs, message) = by_id("q");
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs, Some(json!({"size": 1, "row": {"n": 42}})));
    let (_, state, outputs, message) = by_id("after");
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["value"], json!("42"));

    let calls = stub.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![(
            ws,
            database,
            user,
            "SELECT n FROM t WHERE ns = ?".to_string(),
            vec![Param::Str(ns.clone())],
        )],
        "this workspace's database, as the flow's owner, the value bound"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(state, "SUCCESS");
}

/// A second member of `ws` with the flow permissions: the editor of final
/// review I1, who is not the flow's creator.
async fn editor(db: &Db, ws: Uuid) -> UserId {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 'e', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("e_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed editor");
    let user = UserId(uid);
    common::grant(db, ws, user, &["workflow:read", "workflow:manage"]).await;
    user
}

/// Spec D10, user decision 2026-10-28 (final review I1): `egret.sql` runs as
/// the flow's creator, so an editor who is not the creator cannot publish a
/// flow containing one, and the creator can.
#[tokio::test]
async fn only_the_creator_publishes_an_egret_sql_flow() {
    served_none();
    let Some(db) = common::db().await else { return };
    let (ws, creator) = common::seed_workspace(&db).await;
    let other = editor(&db, ws).await;
    let database = register_database(&db, ws, creator).await;
    let refs = real_refs(&db);
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        creator,
        &sql_flow(&fresh_ns(), &database.to_string()),
    )
    .await;

    let err = egret_workflow::publish(&repo, &refs, ws, other, f.id, "")
        .await
        .expect_err("an editor who is not the creator");
    match err {
        egret_core::Error::InvalidInput(m) => {
            assert!(m.contains("only its creator can publish"), "{m}")
        }
        e => panic!("{e:?}"),
    }
    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.flow_versions WHERE flow_id = $1")
            .bind(f.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(versions, 0, "the refused publish wrote no version");

    egret_workflow::publish(&repo, &refs, ws, creator, f.id, "")
        .await
        .expect("the creator publishes");
}

/// I1 at the API start: a start runs the DRAFT, which any editor can change,
/// so only the creator may start a flow containing `egret.sql`.
#[tokio::test]
async fn only_the_creator_starts_an_egret_sql_flow() {
    served_none();
    let Some(db) = common::db().await else { return };
    let (ws, creator) = common::seed_workspace(&db).await;
    common::grant(&db, ws, creator, &["workflow:read", "workflow:manage"]).await;
    let other = editor(&db, ws).await;
    let database = register_database(&db, ws, creator).await;
    let flow = common::seed_flow(
        &db,
        ws,
        creator,
        &sql_flow(&fresh_ns(), &database.to_string()),
        "draft",
    )
    .await;
    let app = common::app(&db).await;
    let path = format!("/api/ws/{ws}/workflows/flows/{flow}/executions");

    let refused = common::post_no_body(&app, &path, other).await;
    assert_eq!(refused.status(), 400);
    let text = common::body_text(refused).await;
    assert!(text.contains("only its creator can start it"), "{text}");
    assert_eq!(common::execution_count(&db, ws).await, 0);

    let started = common::post_no_body(&app, &path, creator).await;
    assert_eq!(started.status(), 201, "the creator starts the draft");
}

/// I1 for a start no person makes (a schedule, a webhook, a Flow trigger):
/// the source must be one the creator published. A draft never published, a
/// version published by somebody else and a draft edited after the creator's
/// publish are all refused; the creator's published source starts.
#[tokio::test]
async fn a_trigger_start_needs_a_source_the_creator_published() {
    served_none();
    let Some(db) = common::db().await else { return };
    let (ws, creator) = common::seed_workspace(&db).await;
    let other = editor(&db, ws).await;
    let database = register_database(&db, ws, creator).await;
    let refs = real_refs(&db);
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let source = sql_flow(&fresh_ns(), &database.to_string());
    let f = common::draft(&repo, ws, creator, &source).await;
    let start = |src: String| {
        let pool = db.pool.clone();
        async move {
            egret_workflow::worker::start_execution(&pool, ws, Some(f.id), &src, json!({})).await
        }
    };
    let refused = |r: egret_core::Result<Uuid>, what: &str| match r {
        Err(egret_core::Error::InvalidInput(m)) => {
            assert!(
                m.contains("not a version its creator published"),
                "{what}: {m}"
            )
        }
        other => panic!("{what}: {other:?}"),
    };

    refused(start(source.clone()).await, "never published");

    // A version holding this exact source, but published by the editor.
    sqlx::query(
        "INSERT INTO workflow.flow_versions (flow_id, version, snapshot, created_by) \
         VALUES ($1, 99, jsonb_build_object('source', $2::text), $3)",
    )
    .bind(f.id)
    .bind(&source)
    .bind(other.0)
    .execute(&db.pool)
    .await
    .unwrap();
    refused(start(source.clone()).await, "published by somebody else");

    egret_workflow::publish(&repo, &refs, ws, creator, f.id, "")
        .await
        .expect("the creator publishes");
    start(source.clone())
        .await
        .expect("the creator's published source starts");

    let edited = format!("{source}description: edited by another editor\n");
    refused(start(edited).await, "a draft edited after the publish");
}

/// I1 for a `Subflow` child: another user's flow cannot call a flow that
/// runs `egret.sql`; a parent with the same creator can.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subflow_child_with_egret_sql_needs_the_same_creator() {
    served_none();
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let (ws, creator) = common::seed_workspace(&db).await;
    let other = editor(&db, ws).await;
    let database = register_database(&db, ws, creator).await;
    let refs = real_refs(&db);
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let ns = fresh_ns();
    let child = common::draft(&repo, ws, creator, &sql_flow(&ns, &database.to_string())).await;
    egret_workflow::publish(&repo, &refs, ws, creator, child.id, "")
        .await
        .expect("the creator publishes the child");
    let parent_source = |id: &str| {
        format!(
            "id: {id}\nnamespace: {ns}\ntasks:\n  - id: sub\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: {ns}\n    flowId: q\n"
        )
    };
    let (h, _) = egret_workflow::runtime::handlers_for(
        &egret_config::Config::default().workflow,
        db.pool.clone(),
        Arc::new(NoSecrets),
        Arc::new(Stub::default()),
    )
    .await
    .unwrap();
    let mut outcome = Vec::new();
    for (by, id) in [(other, "foreign"), (creator, "own")] {
        let src = parent_source(id);
        let parent = common::draft(&repo, ws, by, &src).await;
        let ex =
            egret_workflow::worker::start_execution(&db.pool, ws, Some(parent.id), &src, json!({}))
                .await
                .expect("the parent runs no egret.sql");
        for _ in 0..64 {
            if egret_workflow::worker::tick(&db.pool, 16, &h)
                .await
                .unwrap()
                == 0
            {
                break;
            }
        }
        let (state, message): (String, Option<String>) = sqlx::query_as(
            "SELECT state, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'sub'",
        )
        .bind(ex)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        outcome.push((id, state, message));
    }
    let (_, state, message) = &outcome[0];
    assert_eq!(state, "FAILED", "{message:?}");
    assert!(
        message
            .as_deref()
            .unwrap_or("")
            .contains("only a flow with the same creator"),
        "{message:?}"
    );
    let (_, state, message) = &outcome[1];
    assert_eq!(state, "SUCCESS", "{message:?}");
}
