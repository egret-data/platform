//! View governance B3 (Task 9): the lineage sweep, and the view drop.
//!
//! Each test drives the REAL embedded lakekeeper (on its in-memory storage)
//! with a fake [`ExplainRunner`] whose answer can change between sweeps. Views
//! are created straight on lakekeeper, with no hold in front, which is what a
//! view committed on a profile with no classified column looks like (Task 8:
//! its lineage is missing). The drop goes through the hold.
//!
//! Each sweep is scoped to the test's own warehouse: the tests of this binary
//! share one database.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-catalog-engine --test lineage_sweep_db

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, run_lakekeeper_migrations, spawn_classification_sweeps, LineageSweep,
    SweepReport, ViewHold,
};
use egret_metastore::Db;
use egret_perm_compile::lineage::{self, ExplainRunner, RecordedLineage, ViewRef};
use egret_policy::PolicyStore;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

async fn lakekeeper_ready(dsn: &str) {
    LAKEKEEPER
        .get_or_init(|| async {
            run_lakekeeper_migrations(dsn)
                .await
                .expect("lakekeeper migrations");
            let (state, _registry) = build_state(dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
}

/// Answers the EXPLAIN with the tables `orders` and `pii` that the SQL
/// mentions, minus the `hidden` ones: a branch the optimizer pruned (R6). A
/// SQL containing any `failing` marker is an engine error.
struct FakeRunner {
    catalog: String,
    hidden: Mutex<BTreeSet<String>>,
    failing: Mutex<BTreeSet<String>>,
}

#[async_trait::async_trait]
impl ExplainRunner for FakeRunner {
    async fn explain_io(&self, catalog: &str, schema: &str, sql: &str) -> Result<Value, String> {
        assert_eq!(catalog, self.catalog);
        assert_eq!(schema, "s");
        if sql.contains("SLOW") {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        if self.failing.lock().unwrap().iter().any(|m| sql.contains(m)) {
            return Err("engine trino-1: Access Denied: boom".into());
        }
        let hidden = self.hidden.lock().unwrap().clone();
        let inputs: Vec<Value> = ["orders", "pii"]
            .iter()
            .filter(|t| sql.contains(*t) && !hidden.contains(**t))
            .map(|t| {
                json!({"table": {"catalog": self.catalog, "schemaTable": {"schema": "s", "table": t}},
                       "constraint": {"none": false, "columnConstraints": []}, "estimate": {}})
            })
            .collect();
        Ok(json!({"inputTableColumnInfos": inputs, "estimate": {}}))
    }
}

struct Env {
    db: Db,
    /// Lakekeeper with the hold in front.
    app: axum::Router,
    /// Lakekeeper with nothing in front.
    raw: axum::Router,
    prefix: String,
    warehouse_id: Uuid,
    catalog: String,
    profile_id: i64,
    runner: Arc<FakeRunner>,
    sweep: Arc<LineageSweep>,
    /// The clearance-bearing tag on `s.pii.ssn`.
    clearance: Uuid,
}

async fn new_tag(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("ls-tag"))
        .bind(unique("LsTag"))
        .fetch_one(&db.pool)
        .await
        .expect("tag")
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    lakekeeper_ready(&dsn).await;
    let (state, _registry) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("lswh"))
        .await
        .expect("lakekeeper warehouse");

    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    let profile_name = unique("trino_ls");
    let tag_service = format!("{profile_name}_tag");
    policy
        .create_service(&tag_service, "tag", &json!({}))
        .await
        .expect("tag service");
    let profile = policy
        .create_service(&profile_name, "trino", &json!({"tagService": tag_service}))
        .await
        .expect("profile");
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'LS') RETURNING id")
            .bind(unique("ls-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-ls-test','AKIATEST') RETURNING id",
    )
    .bind(unique("ls-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(unique("ls_wh"))
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret', \
                 '{\"trinoConfig\": {\"delegationMode\": \"jwt\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("ls-eng"))
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("ls_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    let clearance = new_tag(&db).await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'s','pii','ssn')",
    )
    .bind(clearance)
    .bind(warehouse_id)
    .execute(pool)
    .await
    .expect("assign");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(clearance)
    .bind(vec![unique("ls-role")])
    .execute(pool)
    .await
    .expect("gate");

    let runner = Arc::new(FakeRunner {
        catalog: catalog.clone(),
        hidden: Mutex::default(),
        failing: Mutex::new(BTreeSet::from(["FAIL".to_string()])),
    });
    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let hold = ViewHold::new(db.clone(), runner.clone());
    let app = front_catalog_router(
        catalog_engine_router(state.clone()),
        None,
        Some(Arc::new(hold)),
    );
    let sweep = Arc::new(LineageSweep::new(
        db.clone(),
        state,
        SYSTEM_USER,
        runner.clone(),
    ));
    let env = Env {
        db,
        app,
        raw,
        prefix,
        warehouse_id,
        catalog,
        profile_id: profile.id,
        runner,
        sweep,
        clearance,
    };
    let (status, body) = env
        .call_raw("POST", "namespaces", Some(json!({"namespace": ["s"]})))
        .await;
    assert_eq!(status, StatusCode::OK, "create namespace: {body}");
    Some(env)
}

fn schema(id: i64, columns: &[&str]) -> Value {
    let fields: Vec<Value> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| json!({"id": i + 1, "name": c, "type": "string", "required": false}))
        .collect();
    json!({"type": "struct", "schema-id": id, "fields": fields})
}

fn version(id: i64, schema_id: i64, sql: &str) -> Value {
    json!({
        "version-id": id,
        "schema-id": schema_id,
        "timestamp-ms": chrono::Utc::now().timestamp_millis(),
        "summary": {"engine-name": "trino", "engine-version": "483"},
        "default-namespace": ["s"],
        "representations": [{"type": "sql", "sql": sql, "dialect": "trino"}]
    })
}

impl Env {
    async fn send(
        app: &axum::Router,
        prefix: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = axum::http::Request::builder()
            .method(method)
            .uri(format!("/{prefix}/{path}"))
            .header("content-type", "application/json")
            .body(match body {
                Some(b) => Body::from(b.to_string()),
                None => Body::empty(),
            })
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        Self::send(&self.app, &self.prefix, method, path, body).await
    }

    async fn call_raw(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        Self::send(&self.raw, &self.prefix, method, path, body).await
    }

    /// A view with columns `id`, `ssn`, created with no hold.
    async fn create_raw(&self, name: &str, sql: &str) {
        let body = json!({
            "name": name,
            "schema": schema(0, &["id", "ssn"]),
            "view-version": version(1, 0, sql),
            "properties": {}
        });
        let (status, body) = self
            .call_raw("POST", "namespaces/s/views", Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "create {name}: {body}");
    }

    /// `CREATE OR REPLACE` with no hold: a new current version reading `sql`,
    /// with columns `id`, `ssn`.
    async fn replace_raw(&self, name: &str, sql: &str) {
        let (status, view) = self
            .call_raw("GET", &format!("namespaces/s/views/{name}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let uuid = view["metadata"]["view-uuid"].as_str().expect("view-uuid");
        let body = json!({
            "requirements": [{"type": "assert-view-uuid", "uuid": uuid}],
            "updates": [
                {"action": "add-schema", "schema": schema(1, &["id", "ssn"])},
                {"action": "add-view-version", "view-version": version(2, -1, sql)},
                {"action": "set-current-view-version", "view-version-id": -1}
            ]
        });
        let (status, body) = self
            .call_raw("POST", &format!("namespaces/s/views/{name}"), Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "replace {name}: {body}");
    }

    fn view_ref(&self, name: &str) -> ViewRef {
        ViewRef {
            catalog: self.catalog.clone(),
            schema: "s".into(),
            name: name.into(),
        }
    }

    async fn lineage(&self, name: &str) -> RecordedLineage {
        let mut conn = self.db.pool.acquire().await.unwrap();
        lineage::recorded_lineage(&mut conn, &self.view_ref(name))
            .await
            .unwrap()
    }

    fn computed(&self, tables: &[&str]) -> RecordedLineage {
        RecordedLineage::Computed(
            tables
                .iter()
                .map(|t| (self.catalog.clone(), "s".to_string(), t.to_string()))
                .collect(),
        )
    }

    /// `(tag, column, from_lineage)` of every row on view `name`.
    async fn rows(&self, name: &str) -> BTreeSet<(Uuid, String, bool)> {
        let rows: Vec<(Uuid, String, bool)> = sqlx::query_as(
            "SELECT tag_id, column_name, from_lineage FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 AND schema_name = 's' AND table_name = $2",
        )
        .bind(self.warehouse_id)
        .bind(name)
        .fetch_all(&self.db.pool)
        .await
        .unwrap();
        rows.into_iter().collect()
    }

    fn derived(&self, columns: &[&str]) -> BTreeSet<(Uuid, String, bool)> {
        columns
            .iter()
            .map(|c| (self.clearance, c.to_string(), true))
            .collect()
    }

    /// An admin's tag on view `name`'s column `column` (not lineage-derived).
    async fn manual_tag(&self, name: &str, column: &str) -> Uuid {
        let tag = new_tag(&self.db).await;
        sqlx::query(
            "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
             column_name) VALUES ($1,$2,'s',$3,$4)",
        )
        .bind(tag)
        .bind(self.warehouse_id)
        .bind(name)
        .bind(column)
        .execute(&self.db.pool)
        .await
        .unwrap();
        tag
    }

    async fn tag_version(&self) -> i64 {
        sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(self.profile_id)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn sweep(&self) -> SweepReport {
        self.sweep
            .sweep_warehouses(&[self.warehouse_id])
            .await
            .expect("sweep")
    }
}

/// Spec R6: a branch the optimizer pruned at commit (here, one arm of a
/// `UNION`) starts reading the classified table. The next sweep sees it and
/// tags every column of the view, in one bump. The view had no lineage at all
/// before the first sweep (committed with no hold), which the sweep computes.
#[tokio::test]
async fn the_sweep_tags_a_view_whose_pruned_branch_now_reads_a_classified_table() {
    let Some(env) = env().await else { return };
    let v = unique("v_union");
    env.runner.hidden.lock().unwrap().insert("pii".into());
    env.create_raw(
        &v,
        "SELECT id, ssn FROM orders UNION ALL SELECT id, ssn FROM pii \
         WHERE current_date > DATE '2099-01-01'",
    )
    .await;
    assert_eq!(env.lineage(&v).await, RecordedLineage::Missing);

    let first = env.sweep().await;
    assert_eq!(
        (first.views, first.computed, first.retagged),
        (1, 1, 0),
        "{first:?}"
    );
    assert!(first.failures.is_empty(), "{first:?}");
    assert_eq!(env.lineage(&v).await, env.computed(&["orders"]));
    assert!(env.rows(&v).await.is_empty());
    let before = env.tag_version().await;

    // The predicate turned true: the engine now reports the branch.
    env.runner.hidden.lock().unwrap().clear();
    let second = env.sweep().await;
    assert_eq!((second.computed, second.retagged), (1, 1), "{second:?}");
    assert_eq!(env.lineage(&v).await, env.computed(&["orders", "pii"]));
    assert_eq!(env.rows(&v).await, env.derived(&["id", "ssn"]));
    assert_eq!(env.tag_version().await, before + 1);

    // A sweep that changes nothing bumps nothing.
    let third = env.sweep().await;
    assert_eq!((third.computed, third.retagged), (1, 0), "{third:?}");
    assert_eq!(env.tag_version().await, before + 1);
}

/// The sweep recomputes every view and names each one it could not judge.
/// A failed view keeps its tags; one whose lineage was computed before also
/// keeps that lineage, so B3's re-tag still follows its bases.
#[tokio::test]
async fn the_sweep_recomputes_lineage_and_reports_the_failures() {
    let Some(env) = env().await else { return };
    let good = unique("v_good");
    let bad = unique("v_bad");
    let flaky = unique("v_flaky");
    env.create_raw(&good, "SELECT id, ssn FROM pii").await;
    env.create_raw(&bad, "SELECT id, ssn FROM pii WHERE FAIL")
        .await;
    env.create_raw(&flaky, "SELECT id, ssn AS flaky FROM pii")
        .await;

    let first = env.sweep().await;
    assert_eq!((first.views, first.computed), (3, 2), "{first:?}");
    assert_eq!(first.failures.len(), 1, "{first:?}");
    let failure = &first.failures[0];
    assert_eq!(failure.view.as_deref(), Some(bad.as_str()));
    assert_eq!(failure.schema.as_deref(), Some("s"));
    assert_eq!(failure.catalog, env.catalog);
    assert!(failure.reason.contains("boom"), "{failure:?}");
    assert!(
        matches!(env.lineage(&bad).await, RecordedLineage::Failed { detail } if detail.contains("boom"))
    );
    assert!(env.rows(&bad).await.is_empty());
    for v in [&good, &flaky] {
        assert_eq!(env.lineage(v).await, env.computed(&["pii"]));
        assert_eq!(env.rows(v).await, env.derived(&["id", "ssn"]));
    }

    // The engine now fails for `flaky`, and `bad` keeps failing.
    env.runner.failing.lock().unwrap().insert("flaky".into());
    let second = env.sweep().await;
    let failed: BTreeSet<Option<String>> = second.failures.iter().map(|f| f.view.clone()).collect();
    assert_eq!(
        failed,
        BTreeSet::from([Some(bad.clone()), Some(flaky.clone())]),
        "{second:?}"
    );
    assert_eq!(second.computed, 1, "{second:?}");
    assert_eq!(
        env.lineage(&flaky).await,
        env.computed(&["pii"]),
        "a failed recompute keeps the lineage the engine last reported"
    );
    assert_eq!(
        env.rows(&flaky).await,
        env.derived(&["id", "ssn"]),
        "a failed recompute never removes a tag"
    );
}

/// Task 8 M6: a `CREATE OR REPLACE` that narrows the view (the hold only adds
/// rows) leaves its old tags. The sweep's re-tag removes the derived ones, and
/// keeps the tag an admin put on the view by hand.
#[tokio::test]
async fn the_sweep_removes_the_tags_a_narrowing_replace_left_and_keeps_manual_ones() {
    let Some(env) = env().await else { return };
    let v = unique("v_narrow");
    env.create_raw(&v, "SELECT id, ssn FROM pii").await;
    env.sweep().await;
    assert_eq!(env.rows(&v).await, env.derived(&["id", "ssn"]));
    let manual = env.manual_tag(&v, "id").await;
    env.replace_raw(&v, "SELECT id, ssn FROM orders").await;
    let before = env.tag_version().await;

    let report = env.sweep().await;

    assert_eq!((report.computed, report.retagged), (1, 1), "{report:?}");
    assert_eq!(env.lineage(&v).await, env.computed(&["orders"]));
    assert_eq!(
        env.rows(&v).await,
        BTreeSet::from([(manual, "id".to_string(), false)]),
        "the derived tags go, the manual one stays"
    );
    assert_eq!(env.tag_version().await, before + 1);
}

/// Spec B3: dropping a view deletes its lineage and its lineage-derived tags,
/// once lakekeeper has answered 2xx, in one bump. An admin's tag stays. A drop
/// lakekeeper refuses leaves everything.
#[tokio::test]
async fn dropping_a_view_deletes_its_lineage_and_its_derived_tags() {
    let Some(env) = env().await else { return };
    let v = unique("v_drop");
    env.create_raw(&v, "SELECT id, ssn FROM pii").await;
    env.sweep().await;
    assert_eq!(env.lineage(&v).await, env.computed(&["pii"]));
    assert_eq!(env.rows(&v).await, env.derived(&["id", "ssn"]));
    let manual = env.manual_tag(&v, "ssn").await;
    let before = env.tag_version().await;

    let (status, body) = env
        .call("DELETE", &format!("namespaces/s/views/{v}"), None)
        .await;

    assert!(status.is_success(), "drop: {status} {body}");
    assert_eq!(env.lineage(&v).await, RecordedLineage::Missing);
    assert_eq!(
        env.rows(&v).await,
        BTreeSet::from([(manual, "ssn".to_string(), false)])
    );
    assert_eq!(env.tag_version().await, before + 1);

    // A drop lakekeeper refuses (no such view) leaves the lineage and tags.
    let ghost = unique("v_ghost");
    {
        let mut conn = env.db.pool.acquire().await.unwrap();
        let tables = BTreeSet::from([(env.catalog.clone(), "s".to_string(), "pii".to_string())]);
        lineage::write_lineage(&mut conn, &env.view_ref(&ghost), &tables)
            .await
            .unwrap();
    }
    let (status, _) = env
        .call("DELETE", &format!("namespaces/s/views/{ghost}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(env.lineage(&ghost).await, env.computed(&["pii"]));
}

/// Task 9 fix round 1 (amended ruling): after a classification write commits,
/// a sweep scoped to the affected profiles' views with no computed lineage
/// runs, so a view committed before the profile had any classified column is
/// tagged without waiting for the schedule. Here a full sweep is still running
/// when the classification commits, so the scoped one first meets `Busy` and
/// must run again rather than be dropped.
#[tokio::test]
async fn a_classification_sweeps_the_views_with_no_lineage_even_behind_a_running_sweep() {
    let Some(env) = env().await else { return };
    let listener = spawn_classification_sweeps(env.sweep.clone(), env.db.clone());
    env.create_raw(&unique("v_slow"), "SELECT id, ssn FROM orders WHERE SLOW")
        .await;
    let running = {
        let sweep = env.sweep.clone();
        let wh = env.warehouse_id;
        tokio::spawn(async move { sweep.sweep_warehouses(&[wh]).await })
    };
    // The running sweep has listed its views and is in the slow EXPLAIN.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let v = unique("v_missing");
    env.create_raw(&v, "SELECT id, ssn FROM orders").await;
    assert_eq!(env.lineage(&v).await, RecordedLineage::Missing);

    assert!(
        !running.is_finished(),
        "the full sweep must still be running when the classification commits"
    );
    // A clearance-bearing tag classifies `orders`.
    let y = new_tag(&env.db).await;
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(y)
    .bind(vec![unique("ls-role")])
    .execute(&env.db.pool)
    .await
    .unwrap();
    egret_bi::catalog_tags::CatalogTagRepo::new(env.db.clone())
        .assign_catalog(y, env.warehouse_id, "s", "orders", "id", None)
        .await
        .expect("classify orders");

    let want: BTreeSet<(Uuid, String, bool)> = ["id", "ssn"]
        .iter()
        .map(|c| (y, c.to_string(), true))
        .collect();
    let mut tagged = false;
    for _ in 0..150 {
        if env.rows(&v).await == want {
            tagged = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        tagged,
        "the view was never swept: lineage {:?}, rows {:?}",
        env.lineage(&v).await,
        env.rows(&v).await
    );
    assert_eq!(env.lineage(&v).await, env.computed(&["orders"]));
    let full = running.await.unwrap().expect("the full sweep");
    assert_eq!(full.views, 1, "the full sweep listed only v_slow: {full:?}");
    listener.abort();
}
