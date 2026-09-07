//! **`egret.sql` against a real engine — the only instrument this handler has.**
//!
//! # Why this file is not optional
//!
//! Every other handler in `worker::handlers` is judged by `tests/executor.rs`
//! against Kestra's own execution corpus. `egret.sql` reaches **zero** corpus
//! entries — W3b Task 1 measured it, and the spec (§3.1) says why: Kestra has no
//! `egret.sql`, so no golden entry can ever exercise it. **The ratchet being
//! green says nothing at all about this handler**, and the coverage statement in
//! `oracle-exec/README.md` separates the two for that reason.
//!
//! So this is the instrument, and it is modelled on
//! `crates/egret-dataapi/tests/serving_starrocks_live.rs`, which exists because
//! an audit found that every StarRocks-backed Data API endpoint returned a bare
//! 500 for a period while a fake `QueryExecPort` reported everything as working.
//! A fake decides what an engine does; that is the class of defect it cannot
//! see, and the class this file is written against.
//!
//! # Running it
//!
//! ```bash
//! just up                                    # StarRocks on :9030
//! ./deploy/seed-starrocks-workflow.sh        # the fixture table and its five rows
//! export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//! export EGRET_TEST_STARROCKS_URL=mysql://localhost:9030
//! cargo test -p egret-workflow --test sql_starrocks_live
//! ```
//!
//! # It self-skips without the engine, and a skipped run proves nothing
//!
//! `DATABASE_URL` is fail-closed workspace-wide; the ENGINE gate is a separate
//! axis, and CI has no StarRocks, so it follows the existing opt-in-behind-an-
//! env-var convention (`EGRET_TEST_STARROCKS_URL`, `EGRET_TEST_TRINO_URL`,
//! `EGRET_TEST_S3_ENDPOINT`). Every assertion below is written so it could not
//! pass against a fake: the numbers are SUMs StarRocks computed over rows this
//! file never wrote, and `91`/`13` appear nowhere in the flow source or in any
//! fixture in this crate.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use egret_metastore::Db;
use egret_query::{DatabaseRepo, EngineState, JobManager, QueryStore};
use egret_query_api::{Limits, QueryExecPort};
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::worker::handlers::sql::{Fetch, Sql, TYPE_ID};
use egret_workflow::worker::{self, Handlers, TaskHandler, TaskInput};
use uuid::Uuid;

/// The fixture `deploy/seed-starrocks-workflow.sh` writes.
const SEED_DB: &str = "egret_workflow_live";
const SEED_TABLE: &str = "shipments";

/// The GROUPED result of the four ordinary rows: `('EU', 40), ('EU', 51)` and
/// `('US', 6), ('US', 7)`.
///
/// Asserted as the sum because a sum is something the engine COMPUTED. `91`
/// exists only if StarRocks aggregated two rows this file never sent it.
const EU_UNITS: i64 = 91;

/// The fifth row's `region`, which is the literal text of a classic injection.
///
/// It is in the fixture so the injection assertion has a positive control: bound,
/// `WHERE region = ?` matches **exactly this row** (1 of 5); interpolated,
/// `WHERE region = 'EU' OR '1'='1'` matches **all five**. Both are non-zero, so
/// the assertion cannot pass by the statement being broken.
const INJECTION_TEXT: &str = "EU' OR '1'='1";
const ALL_ROWS: i64 = 5;

// ---------------------------------------------------------------------------
// The parts of `EngineState` this path does not use
// ---------------------------------------------------------------------------

/// `egret.sql` returns rows in an output; it never materialises a result set to
/// object storage. `get` is `Err` rather than `Ok(vec![])` so a future change
/// that DID read through it fails loudly instead of seeing an empty object.
#[derive(Default)]
struct MemStore;

#[async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, _uri: &str, _bytes: Vec<u8>) -> Result<(), StorageError> {
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        Err(StorageError::NotFound(uri.into()))
    }
    async fn delete(&self, _uri: &str) -> Result<(), StorageError> {
        Ok(())
    }
    async fn copy(&self, src: &str, _dst: &str) -> Result<(), StorageError> {
        Err(StorageError::NotFound(src.into()))
    }
    async fn list(
        &self,
        _prefix: &str,
        _delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        Ok(egret_storage::Listing {
            folders: Vec::new(),
            objects: Vec::new(),
        })
    }
}

/// `JobManager`'s factory is unused — `execute_as*` builds its connector
/// directly. It errors rather than returning a stub so a path that started using
/// it could not silently succeed against nothing.
struct UnusedFactory;

#[async_trait]
impl egret_query::execution::connector::ConnectorFactory for UnusedFactory {
    async fn build(
        &self,
        _db: &egret_query::Database,
        _secret: Option<String>,
        _session_user: Option<String>,
        _bearer_token: Option<String>,
    ) -> Result<Box<dyn egret_query::execution::connector::Connector>, egret_query::EngineError>
    {
        Err(egret_query::EngineError::Connect("unused (test)".into()))
    }
}

/// The connection under test is not delegated, so nothing should ask the broker.
/// Refusing keeps that a fact rather than an assumption.
struct NoMinter;

#[async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Live {
    db: Db,
    port: Arc<dyn QueryExecPort>,
    ws: Uuid,
    /// A second workspace, existing only so the scoping assertion has something
    /// to be refused. A single-workspace fixture cannot tell "scoped" from
    /// "there was nothing else to reach".
    other_ws: Uuid,
    database_id: Uuid,
    other_database_id: Uuid,
    owner: egret_core::UserId,
    flow_id: Uuid,
}

fn parse_engine_url() -> Option<(String, i32)> {
    let raw = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&raw).expect("EGRET_TEST_STARROCKS_URL must be a URL");
    Some((u.host_str()?.to_string(), u.port().unwrap_or(9030) as i32))
}

/// `Err` names the gate that was actually closed — the two are different axes
/// and a reader has to be able to tell them apart.
async fn live() -> Result<Live, String> {
    let Some((host, port)) = parse_engine_url() else {
        return Err("EGRET_TEST_STARROCKS_URL unset".to_string());
    };
    let Some(db) = common::db().await else {
        return Err(format!(
            "DATABASE_URL unset, with {} set",
            egret_test_db::ALLOW_SKIP_VAR
        ));
    };

    let (ws, owner) = common::seed_workspace(&db).await;
    let (other_ws, _other_owner) = common::seed_workspace(&db).await;

    let database_id = register_starrocks(&db, ws, owner, &host, port).await;
    let other_database_id = register_starrocks(&db, other_ws, owner, &host, port).await;

    // The flow row exists for ONE reason: `worker::claim` reads the principal
    // from `workflow.flows.created_by` through `executions.flow_id`, so an
    // execution with no flow has no identity and `egret.sql` refuses. See
    // `TaskInput::principal`.
    let flow_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan, created_by) \
         VALUES ($1, 'egret.test', $2, '', '{}'::jsonb, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("live-{}", Uuid::new_v4().simple()))
    .bind(owner.0)
    .fetch_one(&db.pool)
    .await
    .expect("insert flow");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let store = QueryStore::new(db.clone());
    let databases = DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore);
    let limits = Limits {
        max_rows: 100_000,
        timeout: std::time::Duration::from_secs(60),
    };
    let engine = EngineState {
        store: store.clone(),
        jobs: JobManager::new(
            store,
            databases.clone(),
            Arc::new(UnusedFactory),
            storage.clone(),
            "s3://warehouse".into(),
            "query-results".into(),
            limits.clone(),
            100,
        ),
        databases,
        storage,
        bucket_uri: "s3://warehouse".into(),
        results_prefix: "query-results".into(),
        limits,
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
    };

    Ok(Live {
        db,
        // NOT a fake. `egret_query`'s own implementation, the one `egret-bin` wires.
        port: Arc::new(engine),
        ws,
        other_ws,
        database_id,
        other_database_id,
        owner,
        flow_id,
    })
}

async fn register_starrocks(
    db: &Db,
    ws: Uuid,
    by: egret_core::UserId,
    host: &str,
    port: i32,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, options, delegated, created_by) \
         VALUES ($1, $2, 'starrocks', $3, $4, $5, false, $6) RETURNING id",
    )
    .bind(ws)
    .bind(format!("sr-{}", Uuid::new_v4().simple()))
    .bind(host)
    .bind(port)
    .bind(serde_json::json!({ "user": "root" }))
    .bind(by.0)
    .fetch_one(&db.pool)
    .await
    .expect("insert starrocks database")
}

/// A [`TaskInput`] carrying `spec`, `cx` and an identity.
fn input(f: &Live, spec: serde_json::Value, cx: serde_json::Value) -> TaskInput {
    TaskInput {
        workspace_id: f.ws,
        execution_id: Uuid::nil(),
        run_id: Uuid::nil(),
        task_id: "q".to_string(),
        type_id: TYPE_ID.to_string(),
        value: None,
        attempt: 1,
        spec,
        render: egret_workflow::runners::RenderContext::from_json_object(&cx)
            .expect("the render context is an object"),
        principal: Some(f.owner),
    }
}

/// Tick until **this** execution is terminal, and return its state.
///
/// Deliberately not "tick until `tick` returns 0". The claim is *global* by
/// design — a worker serves every workspace, which is the whole point of
/// `SKIP LOCKED` — and the two worker-driven tests in this file run on separate
/// threads against one process-private database, so each one's ticks can and do
/// claim the *other's* runs. Measured: a first version of these tests looped on
/// `tick(..) == 0` and one of them read back `"RUNNING"` because its budget had
/// been spent finishing the other's execution.
///
/// Ticking on behalf of another test is harmless — both register the same
/// handler — so the fix is to stop when the execution under test has finished
/// rather than when this thread happens to find nothing to do. That is also the
/// difference between asserting about the run you started and asserting about
/// whichever run went first.
async fn drive(f: &Live, reg: &Handlers, ex: Uuid) -> String {
    for _ in 0..64 {
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
                .bind(ex)
                .fetch_one(&f.db.pool)
                .await
                .expect("read the execution state");
        if state != "CREATED" && state != "RUNNING" {
            return state;
        }
        worker::tick(&f.db.pool, 1, reg).await.expect("tick");
    }
    panic!("execution {ex} never reached a terminal state");
}

macro_rules! fixture {
    () => {
        match live().await {
            Ok(f) => f,
            Err(why) => {
                eprintln!("skipping: {why}");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// The assertions
// ---------------------------------------------------------------------------

/// **The property the handler exists for, proved by the engine rather than by
/// reading the formatter.**
///
/// The bound value is the literal text of an injection, and the fixture holds a
/// row whose `region` IS that text. Bound, the statement matches that one row;
/// interpolated, `WHERE region = 'EU' OR '1'='1'` matches all five. The two
/// answers are `1` and `5` — both non-zero, so this cannot pass by the statement
/// having been broken, which is the failure mode an "expect zero rows" version of
/// this test would have.
#[tokio::test]
async fn a_bound_value_is_data_to_the_engine_and_not_sql() {
    let f = fixture!();
    let out = Sql::new(f.port.clone())
        .run(&input(
            &f,
            serde_json::json!({
                "database": f.database_id.to_string(),
                "sql": format!(
                    "SELECT count(*) AS n FROM {SEED_DB}.{SEED_TABLE} WHERE region = {{{{ inputs.r }}}}"
                ),
                "fetch": "FETCH_ONE",
            }),
            serde_json::json!({ "inputs": { "r": INJECTION_TEXT } }),
        ))
        .await
        .expect("a bound execution is not a worker failure");

    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{:?}",
        out.message
    );
    let n = out.outputs.as_ref().unwrap()["row"]["n"]
        .as_i64()
        .expect("count(*) is a number");
    assert_eq!(
        n, 1,
        "the injection text must have matched the ONE row that holds it. \
         {ALL_ROWS} would mean the value reached the engine as SQL"
    );
    assert_ne!(
        n, ALL_ROWS,
        "the interpolated reading of the same statement, named so the two \
         answers this test distinguishes are both written down"
    );
}

/// The rows come back, and the numbers are ones StarRocks computed.
///
/// `91` is the sum of two rows this file never sent; the `Spy` port every other
/// test in this crate drives could not invent it.
#[tokio::test]
async fn fetch_one_returns_a_row_the_engine_aggregated() {
    let f = fixture!();
    let out = Sql::new(f.port.clone())
        .run(&input(
            &f,
            serde_json::json!({
                "database": f.database_id.to_string(),
                "sql": format!(
                    "SELECT region, sum(units) AS units FROM {SEED_DB}.{SEED_TABLE} \
                     WHERE region = {{{{ inputs.r }}}} GROUP BY region"
                ),
                "fetch": "FETCH_ONE",
            }),
            serde_json::json!({ "inputs": { "r": "EU" } }),
        ))
        .await
        .unwrap();

    assert_eq!(
        out.state,
        egret_workflow::executor::RunState::Success,
        "{:?}",
        out.message
    );
    let outputs = out.outputs.expect("a successful FETCH_ONE has an output");
    assert_eq!(outputs["size"], serde_json::json!(1));
    assert_eq!(outputs["row"]["region"], serde_json::json!("EU"));
    assert_eq!(
        outputs["row"]["units"].as_i64(),
        Some(EU_UNITS),
        "the sum of two rows, computed by StarRocks: {outputs}"
    );
}

/// `fetch: NONE` produces `{}` — present and empty — **and the statement still
/// ran**. The second half is what makes the first mean anything, and it is
/// asserted by running a statement the engine refuses: a handler that skipped
/// execution for `NONE` would answer `{}` there too.
#[tokio::test]
async fn fetch_none_is_an_empty_map_and_the_statement_still_reached_the_engine() {
    let f = fixture!();
    let handler = Sql::new(f.port.clone());

    let ok = handler
        .run(&input(
            &f,
            serde_json::json!({
                "database": f.database_id.to_string(),
                "sql": format!("SELECT 1 FROM {SEED_DB}.{SEED_TABLE} LIMIT 1"),
            }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(ok.state, egret_workflow::executor::RunState::Success);
    assert_eq!(
        ok.outputs,
        Some(serde_json::json!({})),
        "`{{{{ outputs.q }}}}` must render, so the map is present and empty"
    );

    let bad = handler
        .run(&input(
            &f,
            serde_json::json!({
                "database": f.database_id.to_string(),
                "sql": format!("SELECT nosuchcolumn FROM {SEED_DB}.{SEED_TABLE}"),
            }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        bad.state,
        egret_workflow::executor::RunState::Failed,
        "a `NONE` over a statement the engine refuses must fail, which is what \
         shows the statement was executed rather than skipped"
    );
    assert!(
        bad.message
            .as_deref()
            .unwrap_or("")
            .contains("nosuchcolumn"),
        "and the engine's own words reach the author: {:?}",
        bad.message
    );
}

/// **The port's workspace scoping, exercised rather than trusted.**
///
/// The same handler, the same statement, the same engine — only the pair
/// `(workspace, database)` differs. The control is the first half: without it,
/// a refusal proves nothing, because a handler that refused everything would
/// pass.
#[tokio::test]
async fn a_connection_from_another_workspace_is_refused() {
    let f = fixture!();
    let handler = Sql::new(f.port.clone());
    let stmt = format!("SELECT 1 FROM {SEED_DB}.{SEED_TABLE} LIMIT 1");

    let control = handler
        .run(&input(
            &f,
            serde_json::json!({ "database": f.database_id.to_string(), "sql": stmt }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        control.state,
        egret_workflow::executor::RunState::Success,
        "the control must succeed, or the refusal below means nothing: {:?}",
        control.message
    );

    assert_ne!(f.ws, f.other_ws);
    let refused = handler
        .run(&input(
            &f,
            serde_json::json!({ "database": f.other_database_id.to_string(), "sql": stmt }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        refused.state,
        egret_workflow::executor::RunState::Failed,
        "a `database` registered in another workspace must not be reachable"
    );
    assert!(
        !refused.message.as_deref().unwrap_or("").contains("host"),
        "and the refusal must not describe the connection it declined to open: {:?}",
        refused.message
    );
}

/// **The output is written to `task_runs.outputs` and read back out of
/// Postgres**, driven through the shipping worker rather than by calling the
/// handler.
///
/// This exists because of a hole W3b already paid for once: Task 3's guard on
/// `core.debug.Return`'s output died only to in-process tests, because no
/// `task_runs.outputs` column existed yet, and nothing durable compared it.
/// Asserting on the value this test reads back with SQL is what keeps
/// `egret.sql` from having the same gap.
#[tokio::test]
async fn the_output_reaches_postgres_through_the_real_worker() {
    let f = fixture!();
    let source = format!(
        "id: sql-live\nnamespace: egret.test\ninputs:\n  - id: r\n    type: STRING\n\
         tasks:\n  - id: q\n    type: {TYPE_ID}\n    database: {db}\n    fetch: FETCH_ONE\n    \
         sql: \"SELECT region, sum(units) AS units FROM {SEED_DB}.{SEED_TABLE} \
         WHERE region = {{{{ inputs.r }}}} GROUP BY region\"\n",
        db = f.database_id
    );

    let ex = worker::start_execution(
        &f.db.pool,
        f.ws,
        Some(f.flow_id),
        &source,
        serde_json::json!({ "r": "EU" }),
    )
    .await
    .expect("start the execution");

    let mut reg = Handlers::new();
    reg.add(Arc::new(Sql::new(f.port.clone())));
    let state = drive(&f, &reg, ex).await;

    let outputs: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'q'",
    )
    .bind(ex)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();

    assert_eq!(state, "SUCCESS", "outputs on disk: {outputs:?}");
    let outputs = outputs.expect("`task_runs.outputs` must hold the handler's output");
    assert_eq!(
        outputs["row"]["units"].as_i64(),
        Some(EU_UNITS),
        "the value StarRocks computed, read back out of Postgres: {outputs}"
    );
}

/// An execution with **no owner** refuses, driven through the worker so the
/// principal really comes from `workflow.flows.created_by` rather than from a
/// field a test set.
///
/// The control is the test above, which is the same flow with a `flow_id`.
#[tokio::test]
async fn an_execution_with_no_flow_has_no_owner_and_the_task_fails() {
    let f = fixture!();
    let source = format!(
        "id: sql-noowner\nnamespace: egret.test\ntasks:\n  - id: q\n    type: {TYPE_ID}\n    \
         database: {db}\n    sql: \"SELECT 1\"\n",
        db = f.database_id
    );
    let ex = worker::start_execution(&f.db.pool, f.ws, None, &source, serde_json::json!({}))
        .await
        .expect("start the execution");

    let mut reg = Handlers::new();
    reg.add(Arc::new(Sql::new(f.port.clone())));
    assert_eq!(
        drive(&f, &reg, ex).await,
        "FAILED",
        "the execution as a whole"
    );

    let (state, outputs): (String, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT state, outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'q'",
    )
    .bind(ex)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        state, "FAILED",
        "an execution with no flow has no identity to run as, and the refusal is \
         `egret-automation`'s rule: stop rather than fall back to the service account"
    );
    assert!(
        outputs.is_none(),
        "a failed run produces no output: {outputs:?}"
    );
}

/// The `Fetch` spellings are the ones an author writes. A rename here is a
/// silent behaviour change for every published flow, so it is pinned at the
/// wire, not at the enum.
#[test]
fn the_fetch_spellings_are_the_ones_a_flow_writes() {
    for (text, want) in [
        ("NONE", Fetch::None),
        ("FETCH_ONE", Fetch::One),
        ("FETCH", Fetch::All),
    ] {
        assert_eq!(
            serde_json::from_value::<Fetch>(serde_json::json!(text)).unwrap(),
            want
        );
    }
    assert!(
        serde_json::from_value::<Fetch>(serde_json::json!("STORE")).is_err(),
        "`STORE` writes a result set to internal storage, which W9 owns; it must \
         be refused rather than silently treated as `NONE`"
    );
}
