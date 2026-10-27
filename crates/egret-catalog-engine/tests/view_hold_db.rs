//! View governance B2: the hold that keeps a classified view's Iceberg commit
//! back until every Ranger plugin holds the view's classification tags.
//!
//! Each test drives the REAL embedded lakekeeper (on its in-memory storage)
//! through [`front_catalog_router`] with the hold installed, a fake
//! [`ExplainRunner`], and a fake Trino plugin: a task that polls
//! `download_tags` through egret's real Ranger router with an advancing
//! `lastKnownVersion`, or never polls. An observer layer between the hold and
//! lakekeeper records when each request reaches lakekeeper.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-catalog-engine --test view_hold_db

#[path = "common/plugins.rs"]
mod plugins;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, run_lakekeeper_migrations, view_lock_key, ViewHold, LINEAGE_REFUSAL,
    UNCONFIRMED_REFUSAL,
};
use egret_metastore::Db;
use egret_perm_compile::lineage::{
    recorded_lineage, ExplainRunner, RecordedLineage, ViewRef, EMPTY_LINEAGE_REASON,
};
use egret_policy::{PolicyStore, NO_PLUGIN_POLLED};
use http_body_util::BodyExt as _;
use plugins::Plugin;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";
/// The hold limit of every test that expects a confirmation.
const LIMIT: Duration = Duration::from_secs(20);
/// How often the fake plugin polls. Long enough that a commit that did not
/// wait lands before the plugin reports the new version.
const POLL: Duration = Duration::from_millis(300);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Final review F5: the timed tests run alone. The view-tag advisory lock is
/// global to the test database, so a test that holds it (or keeps the one
/// best-effort permit busy) for seconds stalls every hold running beside it,
/// and any parallel load slows the commits a timed test measures. A test
/// whose assertions bound a duration (a 3 s shared deadline, "returned within
/// 1.2 s") then fails on a stall that is not its own.
///
/// So every test that holds that lock or bounds a duration takes [`serial`]
/// (exclusive); every other test takes [`shared`], and those still run in
/// parallel with each other. `tokio::sync::RwLock` because each
/// `#[tokio::test]` has its own runtime and a panicking holder must not
/// poison the next. Timings are unchanged; only who runs beside whom is.
static SERIAL: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Run alone: holds the view-tag lock, or asserts a duration.
async fn serial() -> tokio::sync::RwLockWriteGuard<'static, ()> {
    SERIAL.write().await
}

/// Run beside other untimed tests, never beside a [`serial`] one.
async fn shared() -> tokio::sync::RwLockReadGuard<'static, ()> {
    SERIAL.read().await
}

/// lakekeeper's schema and default project, once per test process.
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

/// Answers the EXPLAIN by the table names the SQL mentions: `pii`, `orders`.
/// A SQL containing `FAIL` is an engine error, and one containing
/// `WHERE false` reports no input (spec R6).
struct FakeRunner {
    catalog: String,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ExplainRunner for FakeRunner {
    async fn explain_io(&self, catalog: &str, schema: &str, sql: &str) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            catalog, self.catalog,
            "the default catalog is the registry's"
        );
        assert_eq!(schema, "s");
        if sql.contains("SLOW15") {
            tokio::time::sleep(Duration::from_secs(15)).await;
        } else if sql.contains("SLOW1") {
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        if sql.contains("FAIL") {
            return Err("engine trino-1: Access Denied: boom".into());
        }
        let inputs: Vec<Value> = if sql.contains("WHERE false") {
            Vec::new()
        } else {
            ["orders", "pii"]
                .iter()
                .filter(|t| sql.contains(*t))
                .map(|t| {
                    json!({"table": {"catalog": self.catalog, "schemaTable": {"schema": "s", "table": t}},
                           "constraint": {"none": false, "columnConstraints": []}, "estimate": {}})
                })
                .collect()
        };
        Ok(json!({"inputTableColumnInfos": inputs, "estimate": {}}))
    }
}

type Seen = Arc<Mutex<Vec<(Instant, String)>>>;

/// A base classification to commit the moment the next request reaches
/// lakekeeper: `(tag, warehouse, table, column)`. It lands after the hold's
/// first read and before its after-commit record (Task 9 fix round 2, N6).
type OnReach = Arc<Mutex<Option<(Uuid, Uuid, String, String)>>>;

/// A request to stall right after lakekeeper answers it 2xx: `(what,
/// signal)`, `what` as [`Seen`] records it. The observer signals and then
/// sleeps [`STALL`] before the answer goes back to the hold, so a test can
/// drop the request between lakekeeper's commit and the hold's record.
type AfterCommit = Arc<Mutex<Option<(String, Arc<tokio::sync::Notify>)>>>;

/// How long an [`AfterCommit`] request stalls.
const STALL: Duration = Duration::from_millis(500);

/// Records when a request reaches lakekeeper (inside the hold), runs the
/// pending [`OnReach`] classification, if any, and stalls a matching
/// [`AfterCommit`] request once lakekeeper has answered it.
async fn observe(
    State((seen, on_reach, after_commit, db)): State<(Seen, OnReach, AfterCommit, Db)>,
    req: Request,
    next: Next,
) -> Response {
    let what = format!("{} {}", req.method(), req.uri().path());
    seen.lock().unwrap().push((Instant::now(), what.clone()));
    let pending = on_reach.lock().unwrap().take();
    if let Some((tag, warehouse, table, column)) = pending {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments \
                 (tag_id, warehouse_id, schema_name, table_name, column_name) \
             VALUES ($1, $2, 's', $3, $4)",
        )
        .bind(tag)
        .bind(warehouse)
        .bind(table)
        .bind(column)
        .execute(&db.pool)
        .await
        .expect("classify on reach");
    }
    let response = next.run(req).await;
    if response.status().is_success() {
        let armed = {
            let mut slot = after_commit.lock().unwrap();
            match slot.as_ref() {
                Some((w, _)) if *w == what => slot.take().map(|(_, signal)| signal),
                _ => None,
            }
        };
        if let Some(signal) = armed {
            signal.notify_one();
            tokio::time::sleep(STALL).await;
        }
    }
    response
}

struct Env {
    db: Db,
    app: axum::Router,
    /// The same lakekeeper with no hold in front, to build fixtures the hold
    /// would refuse.
    raw: axum::Router,
    tag_service: String,
    tag_service_id: i64,
    prefix: String,
    warehouse_id: Uuid,
    catalog: String,
    profile_id: i64,
    profile_name: String,
    policy: PolicyStore,
    runner: Arc<FakeRunner>,
    seen: Seen,
    on_reach: OnReach,
    after_commit: AfterCommit,
    /// The clearance-bearing tag on `s.pii.ssn` (when classified).
    clearance: Option<Uuid>,
}

async fn new_tag(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("vh-tag"))
        .bind(unique("VhTag"))
        .fetch_one(&db.pool)
        .await
        .expect("tag")
}

/// A clearance-bearing tag on `s.{table}.{column}`.
async fn classify(db: &Db, warehouse_id: Uuid, table: &str, column: &str) -> Uuid {
    let tag = new_tag(db).await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'s',$3,$4)",
    )
    .bind(tag)
    .bind(warehouse_id)
    .bind(table)
    .bind(column)
    .execute(&db.pool)
    .await
    .expect("assign");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(tag)
    .bind(vec![unique("vh-role")])
    .execute(&db.pool)
    .await
    .expect("gate");
    tag
}

/// One governed warehouse: a lakekeeper warehouse with namespace `s`, the
/// egret warehouse and registry catalog over it, one Trino engine attached
/// through a fresh profile (with its tag service). `classified` puts a
/// clearance on `s.pii.ssn`; `s.orders` is never classified here.
async fn env(limit: Duration, classified: bool) -> Option<Env> {
    env_with(Opts {
        limit,
        classified,
        ..Opts::default()
    })
    .await
}

struct Opts {
    limit: Duration,
    classified: bool,
    /// The type of the one engine attached to the catalog.
    engine_type: &'static str,
    max_holds: usize,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            limit: LIMIT,
            classified: true,
            engine_type: "trino",
            max_holds: 8,
        }
    }
}

async fn env_with(o: Opts) -> Option<Env> {
    let Opts {
        limit,
        classified,
        engine_type,
        max_holds,
    } = o;
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    lakekeeper_ready(&dsn).await;
    let (state, _registry) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("vhwh"))
        .await
        .expect("lakekeeper warehouse");

    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    let profile_name = unique("trino_vh");
    let tag_service = format!("{profile_name}_tag");
    let tag_service_id = policy
        .create_service(&tag_service, "tag", &json!({}))
        .await
        .expect("tag service")
        .id;
    let profile = policy
        .create_service(&profile_name, "trino", &json!({"tagService": tag_service}))
        .await
        .expect("profile");

    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'VH') RETURNING id")
            .bind(unique("vh-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-vh-test','AKIATEST') RETURNING id",
    )
    .bind(unique("vh-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(unique("vh_wh"))
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,$3,'egret', \
                 '{\"trinoConfig\": {\"delegationMode\": \"jwt\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("vh-eng"))
    .bind(profile.id)
    .bind(engine_type)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("vh_cat");
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
    let clearance = if classified {
        Some(classify(&db, warehouse_id, "pii", "ssn").await)
    } else {
        None
    };

    let runner = Arc::new(FakeRunner {
        catalog: catalog.clone(),
        calls: AtomicUsize::new(0),
    });
    let seen: Seen = Arc::default();
    let on_reach: OnReach = Arc::default();
    let after_commit: AfterCommit = Arc::default();
    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let inner = catalog_engine_router(state).layer(axum::middleware::from_fn_with_state(
        (
            seen.clone(),
            on_reach.clone(),
            after_commit.clone(),
            db.clone(),
        ),
        observe,
    ));
    let hold = ViewHold::new(db.clone(), runner.clone())
        .with_limit(limit)
        .with_max_holds(max_holds);
    // A pass-through guard in front, as in production: it reads a rename's
    // body, so the hold's rename runs on the bytes the mount put back.
    let guard: Arc<dyn egret_catalog_engine::CatalogGuard> = Arc::new(BodyReadingGuard);
    let app = front_catalog_router(inner, Some(guard), Some(Arc::new(hold)));

    let env = Env {
        db,
        app,
        raw,
        tag_service,
        tag_service_id,
        prefix,
        warehouse_id,
        catalog,
        profile_id: profile.id,
        profile_name,
        policy,
        runner,
        seen,
        on_reach,
        after_commit,
        clearance,
    };
    let (status, body) = env
        .call("POST", "namespaces", Some(json!({"namespace": ["s"]})))
        .await;
    assert_eq!(status, StatusCode::OK, "create namespace: {body}");
    Some(env)
}

impl Env {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        Self::send(&self.app, &self.prefix, method, path, body).await
    }

    /// The same request with no hold in front.
    async fn call_raw(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        Self::send(&self.raw, &self.prefix, method, path, body).await
    }

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
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    async fn create(&self, name: &str, sql: &str) -> (StatusCode, Value) {
        self.call("POST", "namespaces/s/views", Some(create_body(name, sql)))
            .await
    }

    async fn view(&self, name: &str) -> Option<Value> {
        let (status, body) = self
            .call("GET", &format!("namespaces/s/views/{name}"), None)
            .await;
        match status {
            StatusCode::OK => Some(body),
            StatusCode::NOT_FOUND => None,
            other => panic!("load view {name}: {other} {body}"),
        }
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
        recorded_lineage(&mut conn, &self.view_ref(name))
            .await
            .unwrap()
    }

    async fn assignments(&self, name: &str) -> BTreeSet<(Uuid, String)> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT tag_id, column_name FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 AND schema_name = 's' AND table_name = $2",
        )
        .bind(self.warehouse_id)
        .bind(name)
        .fetch_all(&self.db.pool)
        .await
        .unwrap();
        rows.into_iter().collect()
    }

    /// `from_lineage` of each row on view `name`, by column.
    async fn derived_flags(&self, name: &str) -> Vec<bool> {
        sqlx::query_scalar(
            "SELECT from_lineage FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 AND schema_name = 's' AND table_name = $2 \
              ORDER BY column_name",
        )
        .bind(self.warehouse_id)
        .bind(name)
        .fetch_all(&self.db.pool)
        .await
        .unwrap()
    }

    async fn columns(&self, name: &str) -> Option<Vec<String>> {
        let mut conn = self.db.pool.acquire().await.unwrap();
        egret_perm_compile::lineage::recorded_view_columns(&mut conn, &self.view_ref(name))
            .await
            .unwrap()
    }

    async fn tag_version(&self) -> i64 {
        sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(self.profile_id)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// When the view request reached lakekeeper, if it did. `what` is
    /// `"<METHOD> /<path after the prefix>"`.
    fn reached(&self, what: &str) -> Vec<Instant> {
        let (method, path) = what.split_once(' ').unwrap();
        let what = format!("{method} /{}{path}", self.prefix);
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, w)| *w == what)
            .map(|(t, _)| *t)
            .collect()
    }

    /// Drive `request` until lakekeeper has answered `what` 2xx, then drop
    /// it, as hyper drops the handler future of a client that disconnects.
    /// `what` is `"<METHOD> /<path after the prefix>"`.
    async fn cancel_after_commit<F>(&self, what: &str, request: F)
    where
        F: std::future::Future<Output = (StatusCode, Value)>,
    {
        let (method, path) = what.split_once(' ').unwrap();
        let signal = Arc::new(tokio::sync::Notify::new());
        *self.after_commit.lock().unwrap() =
            Some((format!("{method} /{}{path}", self.prefix), signal.clone()));
        // Boxed, so `drop` drops the handler future itself.
        let mut request = Box::pin(request);
        tokio::select! {
            r = &mut request => panic!("{what} answered before lakekeeper committed: {r:?}"),
            _ = signal.notified() => {}
        }
        drop(request);
        assert!(
            self.after_commit.lock().unwrap().is_none(),
            "{what} was stalled"
        );
    }

    fn expected_pii_tags(&self, columns: &[&str]) -> BTreeSet<(Uuid, String)> {
        let tag = self.clearance.expect("classified");
        columns.iter().map(|c| (tag, c.to_string())).collect()
    }

    /// The fake Trino plugin: polls `download_tags` every [`POLL`], reporting
    /// the version its last 200 carried.
    fn start_plugin(&self) -> Plugin {
        plugins::start_trino(self.policy.clone(), self.profile_name.clone(), POLL)
    }
}

/// Lets every request through. The mount hands it the body of exactly the
/// requests `GuardedBody` names; a rename's must be the rename it expects.
struct BodyReadingGuard;

#[async_trait::async_trait]
impl egret_catalog_engine::CatalogGuard for BodyReadingGuard {
    async fn check(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        _bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<egret_catalog_engine::CatalogCaller, (StatusCode, String)> {
        let kind = egret_catalog_engine::GuardedBody::of(method, path);
        assert_eq!(
            body.is_some(),
            kind.is_some(),
            "the body of a guarded request only: {method} {path}"
        );
        let rename = kind == Some(egret_catalog_engine::GuardedBody::Rename);
        if let (true, Some(b)) = (rename, body) {
            let json: Value = serde_json::from_slice(b).expect("a rename body is JSON");
            assert!(json.get("source").is_some(), "the rename body: {json}");
        }
        Ok(egret_catalog_engine::CatalogCaller {
            subject: "view-hold-test".into(),
            audience: "catalog".into(),
        })
    }
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

/// Live result §1 Q1's create body, in its shape.
fn create_body(name: &str, sql: &str) -> Value {
    json!({
        "name": name,
        "schema": schema(0, &["id", "ssn"]),
        "view-version": version(1, 0, sql),
        "properties": {"trino.run-as-owner": "vgadmin"}
    })
}

/// Live result §1 Q1's `CREATE OR REPLACE` commit body, in its shape.
fn replace_body(view: &Value, sql: &str) -> Value {
    let uuid = view["metadata"]["view-uuid"].as_str().expect("view-uuid");
    json!({
        "requirements": [{"type": "assert-view-uuid", "uuid": uuid}],
        "updates": [
            {"action": "set-properties", "updates": {"trino.run-as-owner": "vgadmin"}},
            {"action": "add-schema", "schema": schema(1, &["id", "ssn", "name"])},
            {"action": "add-view-version", "view-version": version(2, -1, sql)},
            {"action": "set-current-view-version", "view-version-id": -1}
        ]
    })
}

fn current_sql(view: &Value) -> String {
    let m = &view["metadata"];
    let current = m["current-version-id"].as_i64().unwrap();
    let v = m["versions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["version-id"].as_i64() == Some(current))
        .unwrap();
    v["representations"][0]["sql"].as_str().unwrap().to_string()
}

fn message(body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn computed(tables: &[&str], catalog: &str) -> RecordedLineage {
    RecordedLineage::Computed(
        tables
            .iter()
            .map(|t| (catalog.to_string(), "s".to_string(), t.to_string()))
            .collect(),
    )
}

/// Test 1: the view reaches lakekeeper only after the plugin's download
/// reports the new tag version.
#[tokio::test]
async fn a_classified_view_commits_only_after_the_plugin_holds_its_tags() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;

    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let version = env.tag_version().await;
    let reached = env.reached("POST /namespaces/s/views");
    assert_eq!(reached.len(), 1, "one create reached lakekeeper");
    let confirmed = plugin.report_of(version).await;
    assert!(
        confirmed <= reached[0],
        "the commit reached lakekeeper {:?} before the plugin reported version {version}",
        confirmed.duration_since(reached[0])
    );
    assert!(env.view("v1").await.is_some());
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "ssn"])
    );
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
    // Task 9: the rows are marked lineage-derived, and the columns are
    // recorded beside the lineage, so B3 can re-tag the view.
    assert_eq!(env.derived_flags("v1").await, vec![true, true]);
    assert_eq!(
        env.columns("v1").await,
        Some(vec!["id".into(), "ssn".into()])
    );
}

/// Test 2: no plugin ever polls. The request is refused within the limit plus
/// a margin, and nothing remains.
#[tokio::test]
async fn a_view_no_plugin_confirms_is_refused_and_nothing_remains() {
    let _serial = serial().await;
    let limit = Duration::from_secs(2);
    let Some(env) = env(limit, true).await else {
        return;
    };
    let before = env.tag_version().await;

    let start = Instant::now();
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    let took = start.elapsed();

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["type"], "CommitFailedException");
    let msg = message(&body);
    assert!(msg.contains(&format!("{UNCONFIRMED_REFUSAL} 2 s")), "{msg}");
    assert!(msg.contains(NO_PLUGIN_POLLED), "{msg}");
    assert!(took >= limit, "refused after {took:?}, before the limit");
    assert!(
        took < limit + Duration::from_secs(5),
        "refused after {took:?}"
    );

    assert!(env.reached("POST /namespaces/s/views").is_empty());
    assert!(env.view("v1").await.is_none(), "no view exists");
    assert!(
        env.assignments("v1").await.is_empty(),
        "no assignment remains"
    );
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
    assert_eq!(
        env.tag_version().await,
        before + 2,
        "bumped, then bumped back"
    );
}

/// Test 3: a view over unclassified tables commits with no wait, and its
/// lineage is recorded.
#[tokio::test]
async fn a_view_over_unclassified_tables_commits_with_no_wait() {
    let _serial = serial().await;
    // No plugin polls: a hold would time out at the limit.
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let before = env.tag_version().await;

    let start = Instant::now();
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );

    assert!(env.view("v1").await.is_some());
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v1").await, computed(&["orders"], &env.catalog));
    // Task 9: a later classification of `orders` re-tags these columns.
    assert_eq!(
        env.columns("v1").await,
        Some(vec!["id".into(), "ssn".into()])
    );
    assert_eq!(env.tag_version().await, before);
    assert_eq!(env.runner.calls.load(Ordering::SeqCst), 1);
}

/// Test 4: an EXPLAIN failure refuses, and so does an empty lineage (R6).
/// Nothing is written either way.
#[tokio::test]
async fn a_lineage_that_cannot_be_computed_refuses_and_writes_nothing() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let before = env.tag_version().await;

    for (sql, reason) in [
        ("SELECT id, ssn FROM pii -- FAIL", "Access Denied: boom"),
        ("SELECT id, ssn FROM pii WHERE false", EMPTY_LINEAGE_REASON),
    ] {
        let (status, body) = env.create("v1", sql).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let msg = message(&body);
        assert!(msg.contains(&format!("{LINEAGE_REFUSAL}: ")), "{msg}");
        assert!(msg.contains(reason), "{msg}");
    }

    assert!(env.reached("POST /namespaces/s/views").is_empty());
    assert!(env.view("v1").await.is_none());
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
    assert_eq!(env.tag_version().await, before);
}

/// Test 5: `CREATE OR REPLACE` (a commit carrying `add-view-version`) goes
/// through the same hold.
#[tokio::test]
async fn create_or_replace_goes_through_the_same_hold() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let plugin = env.start_plugin();
    plugin.polled().await;

    let view = env.view("v1").await.unwrap();
    let sql = "SELECT id, ssn, name FROM pii";
    let (status, body) = env
        .call(
            "POST",
            "namespaces/s/views/v1",
            Some(replace_body(&view, sql)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let version = env.tag_version().await;
    let reached = env.reached("POST /namespaces/s/views/v1");
    assert_eq!(reached.len(), 1);
    let confirmed = plugin.report_of(version).await;
    assert!(confirmed <= reached[0], "the replace did not wait");
    assert_eq!(current_sql(&env.view("v1").await.unwrap()), sql);
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "name", "ssn"])
    );
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
}

/// Test 6: a commit with no new view version passes untouched.
#[tokio::test]
async fn a_commit_with_no_new_view_version_passes_untouched() {
    let _serial = serial().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = env.tag_version().await;
    let calls = env.runner.calls.load(Ordering::SeqCst);

    // `COMMENT ON VIEW`, then a replace with the same SQL and schema.
    for body in [
        json!({"updates": [{"action": "set-properties", "updates": {"comment": "hello"}}]}),
        json!({"requirements": [], "updates": []}),
    ] {
        let start = Instant::now();
        let (status, resp) = env.call("POST", "namespaces/s/views/v1", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{resp}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    assert_eq!(env.reached("POST /namespaces/s/views/v1").len(), 2);
    assert_eq!(env.runner.calls.load(Ordering::SeqCst), calls, "no EXPLAIN");
    assert_eq!(env.tag_version().await, before);
    assert!(env.assignments("v1").await.is_empty());
    let view = env.view("v1").await.unwrap();
    assert_eq!(view["metadata"]["properties"]["comment"], "hello");
}

/// Test 7: lakekeeper answers non-2xx after the hold. Nothing written by
/// that hold remains, and what was there before it does.
#[tokio::test]
async fn a_commit_lakekeeper_refuses_after_the_hold_leaves_nothing_behind() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tags_before = env.assignments("v1").await;
    let lineage_before = env.lineage("v1").await;

    // A second create of the same name, reading a newly classified table:
    // the hold writes the new tag and lineage, then lakekeeper refuses.
    let second = classify(&env.db, env.warehouse_id, "orders", "name").await;
    let (status, body) = env
        .create("v1", "SELECT id, ssn FROM pii JOIN orders USING (id)")
        .await;
    assert!(
        !status.is_success(),
        "lakekeeper must refuse: {status} {body}"
    );
    assert_eq!(
        env.reached("POST /namespaces/s/views").len(),
        2,
        "the hold let it through"
    );

    assert_eq!(env.assignments("v1").await, tags_before);
    assert!(env
        .assignments("v1")
        .await
        .iter()
        .all(|(t, _)| *t != second));
    assert_eq!(env.lineage("v1").await, lineage_before);
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
    assert_eq!(
        current_sql(&env.view("v1").await.unwrap()),
        "SELECT id, ssn FROM pii"
    );
}

/// Test 8: a rename moves the lineage and the tag assignments.
#[tokio::test]
async fn a_rename_moves_the_lineage_and_the_tags() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tags = env.assignments("v1").await;
    assert!(!tags.is_empty());

    let (status, body) = env
        .call(
            "POST",
            "views/rename",
            Some(json!({"source": {"namespace": ["s"], "name": "v1"},
                        "destination": {"namespace": ["s"], "name": "v2"}})),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    assert!(env.view("v1").await.is_none());
    assert!(env.view("v2").await.is_some());
    assert_eq!(env.assignments("v2").await, tags);
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v2").await, computed(&["pii"], &env.catalog));
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
    let version = env.tag_version().await;
    let reached = env.reached("POST /views/rename");
    assert_eq!(reached.len(), 1);
    // The rename reached lakekeeper after the plugin held the version that
    // carries v2's tags (the source's removal comes after, one bump later).
    let confirmed = plugin.report_of(version - 1).await;
    assert!(confirmed <= reached[0], "the rename did not wait");
}

/// The user's ruling on the brief: when the profile governing the view's
/// catalog has no classified column at all, the commit passes with no
/// EXPLAIN, and the lineage stays missing for the B3 sweep.
#[tokio::test]
async fn a_profile_with_no_classified_column_commits_without_explain() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, false).await else {
        return;
    };
    let before = env.tag_version().await;

    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Task 9 fix round 1 (amended ruling): the EXPLAIN runs best-effort, and
    // its lineage and columns are recorded, so a later classification of
    // `pii` re-tags the view in the base write itself.
    assert_eq!(
        env.runner.calls.load(Ordering::SeqCst),
        1,
        "one best-effort EXPLAIN"
    );
    assert!(env.view("v1").await.is_some());
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
    assert_eq!(
        env.columns("v1").await,
        Some(vec!["id".into(), "ssn".into()])
    );
    assert_eq!(env.tag_version().await, before);
}

/// Task 9 fix round 1 (amended ruling): on a profile with no classified
/// column, a lineage that cannot be computed never refuses the commit; it is
/// recorded `failed`, so the sweep a later classification triggers finds it.
#[tokio::test]
async fn a_profile_with_no_classified_column_never_refuses_and_records_a_failed_lineage() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, false).await else {
        return;
    };
    for (name, sql) in [
        ("vf", "SELECT id, ssn FROM pii WHERE FAIL"),
        ("ve", "SELECT id, ssn FROM pii WHERE false"),
    ] {
        let (status, body) = env.create(name, sql).await;
        assert_eq!(status, StatusCode::OK, "{name}: {body}");
        assert!(env.view(name).await.is_some());
        assert!(
            matches!(env.lineage(name).await, RecordedLineage::Failed { .. }),
            "{name}: {:?}",
            env.lineage(name).await
        );
    }
}

/// Wait until `check` holds, up to 10 s.
async fn eventually<F, Fut>(what: &str, check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..500 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never happened: {what}");
}

/// Backends of this database waiting on a lock.
async fn lock_waiters(db: &Db) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity \
          WHERE datname = current_database() AND wait_event_type = 'Lock'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Task 9 fix round 1, I1(a): an admin assigns, by hand, a coordinate the hold
/// derived while the hold waits for the plugins. The hold is then refused and
/// compensates: it removes its own derived rows, never the admin's.
#[tokio::test]
async fn compensation_never_removes_a_tag_an_admin_assigned_during_the_wait() {
    let _shared = shared().await;
    let Some(env) = env(Duration::from_secs(3), true).await else {
        return;
    };
    let tag = env.clearance.unwrap();
    let (app, prefix) = (env.app.clone(), env.prefix.clone());
    let create = tokio::spawn(async move {
        Env::send(
            &app,
            &prefix,
            "POST",
            "namespaces/s/views",
            Some(create_body("v1", "SELECT id, ssn FROM pii")),
        )
        .await
    });
    let e = &env;
    eventually("the hold's rows are committed", || async move {
        e.assignments("v1").await.len() == 2
    })
    .await;
    // `assign_catalog`'s upsert: a derived coordinate becomes manual.
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', 'v1', 'ssn') \
         ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) \
         DO UPDATE SET from_lineage = false WHERE catalog_tag_assignments.from_lineage",
    )
    .bind(tag)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .unwrap();

    let (status, body) = create.await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(message(&body).contains(UNCONFIRMED_REFUSAL), "{body}");
    assert_eq!(
        env.assignments("v1").await,
        BTreeSet::from([(tag, "ssn".to_string())]),
        "the admin's tag stays"
    );
    assert_eq!(env.derived_flags("v1").await, vec![false]);
}

/// Task 9 fix round 1, I1(b): while a replace waits, a base of the view's
/// PREVIOUS lineage is classified with the tag the hold inserted. The base
/// write's retag finds the rows present and adds none. When the replace is
/// refused, compensation restores the previous lineage AND its derived tags.
#[tokio::test]
async fn compensation_rederives_the_tags_the_previous_lineage_carries() {
    let _shared = shared().await;
    let Some(env) = env(Duration::from_secs(3), true).await else {
        return;
    };
    let tag = env.clearance.unwrap();
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.assignments("v1").await.is_empty());
    let view = env.view("v1").await.unwrap();
    let (app, prefix) = (env.app.clone(), env.prefix.clone());
    let replace = tokio::spawn(async move {
        Env::send(
            &app,
            &prefix,
            "POST",
            "namespaces/s/views/v1",
            Some(replace_body(&view, "SELECT id, ssn FROM pii")),
        )
        .await
    });
    let e = &env;
    eventually("the replace's rows are committed", || async move {
        e.assignments("v1").await.len() == 3
    })
    .await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', 'orders', 'id')",
    )
    .bind(tag)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .unwrap();

    let (status, body) = replace.await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(env.lineage("v1").await, computed(&["orders"], &env.catalog));
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "ssn"]),
        "the restored lineage derives the tag on every column of the old version"
    );
    assert_eq!(env.derived_flags("v1").await, vec![true, true]);
}

/// Task 9 fix round 1, I2: a base classification that commits after the hold
/// has read the view's tag set, but before the hold's lineage commits, must
/// still reach the view. Here the hold's write is stalled on a row lock the
/// test holds, the base write runs, then the stall is released.
#[tokio::test]
async fn a_base_classification_racing_a_held_commit_reaches_the_view() {
    let _serial = serial().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let x = env.clearance.unwrap();
    // A clearance-bearing tag not yet on any column.
    let y = new_tag(&env.db).await;
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(y)
    .bind(vec![unique("vh-role")])
    .execute(&env.db.pool)
    .await
    .unwrap();
    // Stall: an uncommitted row on a coordinate the hold will insert.
    let mut stall = env.db.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', 'v1', 'id')",
    )
    .bind(x)
    .bind(env.warehouse_id)
    .execute(&mut *stall)
    .await
    .unwrap();

    let (app, prefix) = (env.app.clone(), env.prefix.clone());
    let create = tokio::spawn(async move {
        Env::send(
            &app,
            &prefix,
            "POST",
            "namespaces/s/views",
            Some(create_body(
                "v1",
                "SELECT p.id, p.ssn FROM pii p JOIN orders o ON p.id = o.id",
            )),
        )
        .await
    });
    let db = &env.db;
    eventually("the hold's write waits on the stall", || async move {
        lock_waiters(db).await >= 1
    })
    .await;
    let repo = egret_bi::catalog_tags::CatalogTagRepo::new(env.db.clone());
    let wh = env.warehouse_id;
    let base =
        tokio::spawn(async move { repo.assign_catalog(y, wh, "s", "orders", "id", None).await });
    // The base write either completes, or waits behind the hold.
    let b = &base;
    eventually("the base write finished or waits", || async move {
        b.is_finished() || lock_waiters(db).await >= 2
    })
    .await;
    stall.rollback().await.unwrap();

    let (status, body) = create.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    base.await.unwrap().expect("the base write");
    let rows = env.assignments("v1").await;
    for tag in [x, y] {
        for column in ["id", "ssn"] {
            assert!(
                rows.contains(&(tag, column.to_string())),
                "{column} lacks {tag}: {rows:?}"
            );
        }
    }
}

// ── fix round 1 ─────────────────────────────────────────────────────────────

/// I1 / N3: a commit that makes a STORED version current is judged on that
/// stored version (loaded from the view's metadata) and held like any other.
/// `-1` with no version added before it cannot be resolved and is refused.
#[tokio::test]
async fn a_commit_switching_to_a_stored_version_is_held_on_that_version() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    // Version 1 reads the classified table, version 2 does not.
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = env.view("v1").await.unwrap();
    let (status, body) = env
        .call(
            "POST",
            "namespaces/s/views/v1",
            Some(replace_body(&view, "SELECT id, name FROM orders")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(env.lineage("v1").await, computed(&["orders"], &env.catalog));
    let calls = env.runner.calls.load(Ordering::SeqCst);

    let (status, body) = env
        .call(
            "POST",
            "namespaces/s/views/v1",
            Some(json!({"updates": [
                {"action": "set-current-view-version", "view-version-id": 1}
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.runner.calls.load(Ordering::SeqCst),
        calls + 1,
        "the stored version was EXPLAINed"
    );
    assert_eq!(
        current_sql(&env.view("v1").await.unwrap()),
        "SELECT id, ssn FROM pii"
    );
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
    let version = env.tag_version().await;
    let reached = env.reached("POST /namespaces/s/views/v1");
    let confirmed = plugin.report_of(version).await;
    assert!(
        confirmed <= *reached.last().unwrap(),
        "the switch did not wait"
    );

    // `-1` with nothing added before it.
    let (status, body) = env
        .call(
            "POST",
            "namespaces/s/views/v1",
            Some(json!({"updates": [
                {"action": "set-current-view-version", "view-version-id": -1}
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(message(&body).contains("DROP and CREATE"), "{body}");
}

/// I1 / N3: an explicit current id that no STORED version has (here the id
/// lakekeeper would give a version this request adds) cannot be loaded, and
/// is refused; the hold never judges the wrong added version.
#[tokio::test]
async fn a_commit_whose_current_cannot_be_loaded_is_refused() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = env.view("v1").await.unwrap();
    let uuid = view["metadata"]["view-uuid"].as_str().unwrap();
    let body = json!({
        "requirements": [{"type": "assert-view-uuid", "uuid": uuid}],
        "updates": [
            {"action": "add-schema", "schema": schema(1, &["id", "ssn"])},
            {"action": "add-view-version", "view-version": version(2, -1, "SELECT id, ssn FROM pii")},
            {"action": "add-view-version", "view-version": version(3, -1, "SELECT id, name FROM orders WHERE id > 1")},
            {"action": "set-current-view-version", "view-version-id": 2}
        ]
    });
    let (status, resp) = env.call("POST", "namespaces/s/views/v1", Some(body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{resp}");
    assert!(message(&resp).contains("cannot be loaded"), "{resp}");
    assert_eq!(
        current_sql(&env.view("v1").await.unwrap()),
        "SELECT id, name FROM orders"
    );
    assert!(env.assignments("v1").await.is_empty());
}

/// N1: `set-current-view-version -1` binds to the version added just before
/// it. In `[add A, set -1, add B]` A is current, so A is what is judged.
#[tokio::test]
async fn set_current_minus_one_binds_to_the_version_added_before_it() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = env.view("v1").await.unwrap();
    let uuid = view["metadata"]["view-uuid"].as_str().unwrap();
    let body = json!({
        "requirements": [{"type": "assert-view-uuid", "uuid": uuid}],
        "updates": [
            {"action": "add-schema", "schema": schema(1, &["id", "ssn"])},
            {"action": "add-view-version", "view-version": version(2, -1, "SELECT id, ssn FROM pii")},
            {"action": "set-current-view-version", "view-version-id": -1},
            {"action": "add-view-version", "view-version": version(3, -1, "SELECT id, name FROM orders WHERE id > 1")}
        ]
    });
    let (status, resp) = env.call("POST", "namespaces/s/views/v1", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(
        current_sql(&env.view("v1").await.unwrap()),
        "SELECT id, ssn FROM pii"
    );
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "ssn"])
    );
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
}

/// N2: `schema-id: -1` is the schema added just before the version, not the
/// last one in the request: the tags go on the columns the view really has.
#[tokio::test]
async fn schema_minus_one_is_the_schema_added_before_the_version() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = env.view("v1").await.unwrap();
    let uuid = view["metadata"]["view-uuid"].as_str().unwrap();
    let body = json!({
        "requirements": [{"type": "assert-view-uuid", "uuid": uuid}],
        "updates": [
            {"action": "add-schema", "schema": schema(1, &["id", "ssn"])},
            {"action": "add-view-version", "view-version": version(2, -1, "SELECT id, ssn FROM pii")},
            {"action": "add-schema", "schema": schema(2, &["x", "y"])},
            {"action": "set-current-view-version", "view-version-id": -1}
        ]
    });
    let (status, resp) = env.call("POST", "namespaces/s/views/v1", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "ssn"])
    );
}

/// Create the namespace `a`, then `a.b` (lakekeeper's `\u{1f}`).
async fn nested_namespace(env: &Env) {
    for ns in [json!(["a"]), json!(["a", "b"])] {
        let (status, body) = env
            .call_raw("POST", "namespaces", Some(json!({ "namespace": ns })))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

/// I2: a create in a multi-level namespace is refused on a classified
/// profile, and passes on a profile with no classified column.
#[tokio::test]
async fn a_multi_level_namespace_is_refused_only_on_a_classified_profile() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    nested_namespace(&env).await;
    let (status, body) = env
        .call(
            "POST",
            "namespaces/a%1Fb/views",
            Some(create_body("vn", "SELECT id, ssn FROM s.pii")),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(message(&body).contains("single-level schema"), "{body}");
    assert!(env.reached("POST /namespaces/a%1Fb/views").is_empty());

    let Some(plain) = env_with(Opts {
        classified: false,
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    nested_namespace(&plain).await;
    let (status, body) = plain
        .call(
            "POST",
            "namespaces/a%1Fb/views",
            Some(create_body("vn", "SELECT id, ssn FROM s.pii")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// I2: renaming a view out of a multi-level namespace (where it was never
/// held) into a single-level one is refused on a classified profile.
#[tokio::test]
async fn a_rename_out_of_a_multi_level_namespace_is_refused() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    nested_namespace(&env).await;
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/a%1Fb/views",
            Some(create_body("vn", "SELECT id, ssn FROM s.pii")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = env
        .call(
            "POST",
            "views/rename",
            Some(json!({"source": {"namespace": ["a", "b"], "name": "vn"},
                        "destination": {"namespace": ["s"], "name": "vn"}})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(message(&body).contains("single-level schema"), "{body}");
    assert!(env.view("vn").await.is_none(), "nothing arrived in s");
}

/// I3: the EXPLAIN is bounded (5 s), whatever the engine does.
#[tokio::test]
async fn a_slow_explain_is_refused_at_its_bound() {
    let _serial = serial().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let start = Instant::now();
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii -- SLOW15").await;
    let took = start.elapsed();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        message(&body).contains("did not answer within 5 s"),
        "{body}"
    );
    assert!(took < Duration::from_secs(7), "{took:?}");
    assert!(env.view("v1").await.is_none());
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
}

/// I3: one deadline from arrival covers the EXPLAIN and the wait together.
#[tokio::test]
async fn the_explain_and_the_wait_share_one_deadline() {
    let _serial = serial().await;
    let limit = Duration::from_secs(3);
    let Some(env) = env(limit, true).await else {
        return;
    };
    let start = Instant::now();
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii -- SLOW1").await;
    let took = start.elapsed();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(message(&body).contains(UNCONFIRMED_REFUSAL), "{body}");
    assert!(took >= limit, "{took:?}");
    assert!(
        took < limit + Duration::from_secs(1),
        "the deadline was not shared: {took:?}"
    );
    assert!(env.assignments("v1").await.is_empty());
}

/// I3 + I4: a view whose lock another hold has is refused at once, and a view
/// on a profile with no classified column takes no lock at all.
#[tokio::test]
async fn a_busy_view_lock_refuses_at_once_and_an_unclassified_view_takes_none() {
    let _serial = serial().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let Some(plain) = env_with(Opts {
        classified: false,
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    let mut other = env.db.pool.acquire().await.unwrap().detach();
    for (e, name) in [(&env, "v1"), (&plain, "v1")] {
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(view_lock_key(&e.prefix, "s", name))
            .execute(&mut other)
            .await
            .unwrap();
    }

    let start = Instant::now();
    let got = tokio::time::timeout(
        Duration::from_secs(10),
        env.create("v1", "SELECT id, ssn FROM pii"),
    )
    .await;
    let (status, body) = got.expect("the hold blocked on the lock");
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        message(&body).contains(
            "another change to this view is being held, or the lineage sweep is recomputing it"
        ),
        "{body}"
    );
    assert!(start.elapsed() < Duration::from_secs(2));

    let got = tokio::time::timeout(
        Duration::from_secs(10),
        plain.create("v1", "SELECT id, ssn FROM pii"),
    )
    .await;
    let (status, body) = got.expect("an unclassified view waited for a lock");
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::Connection::close(other).await.unwrap();
}

/// I4: when the concurrent holds are at their cap, the next is refused fast.
#[tokio::test]
async fn a_full_hold_semaphore_refuses_fast() {
    let _serial = serial().await;
    let Some(env) = env_with(Opts {
        limit: Duration::from_secs(5),
        max_holds: 1,
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    // No plugin polls, so the first hold waits out its 5 s.
    let (first, (second, took)) =
        tokio::join!(env.create("v1", "SELECT id, ssn FROM pii"), async {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            let start = Instant::now();
            let r = env.create("v2", "SELECT id, ssn FROM pii").await;
            (r, start.elapsed())
        });
    let (status, body) = second;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        message(&body).contains("classified view changes already"),
        "{body}"
    );
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert_eq!(first.0, StatusCode::CONFLICT, "{}", first.1);
    assert!(
        message(&first.1).contains(UNCONFIRMED_REFUSAL),
        "{}",
        first.1
    );
}

/// M1: no Trino engine is bound to the view's catalog, so no plugin can
/// confirm: refused at once, and nothing remains.
#[tokio::test]
async fn no_bound_trino_engine_refuses_at_once() {
    let _serial = serial().await;
    let Some(env) = env_with(Opts {
        engine_type: "starrocks",
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    let before = env.tag_version().await;
    let start = Instant::now();
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        env.tag_version().await,
        before,
        "refused before step 3: no write, no bump, no compensation"
    );
    let msg = message(&body);
    assert!(msg.contains("no Trino engine"), "{msg}");
    assert!(msg.contains("attach a Trino engine"), "{msg}");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert!(env.view("v1").await.is_none());
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
}

/// D3: when the view's tags would put two differently-masking tag policies on
/// one view column, the hold refuses and nothing remains.
#[tokio::test]
async fn tags_that_would_overlap_two_masks_on_a_view_column_are_refused() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let ssn_tag = env.clearance.unwrap();
    let id_tag = classify(&env.db, env.warehouse_id, "pii", "id").await;
    for (tag, mask) in [(ssn_tag, "MASK_NULL"), (id_tag, "MASK_HASH")] {
        // A tag policy addresses the tag by NAME (`download_tags` serves names).
        let key: String = sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
            .bind(tag)
            .fetch_one(&env.db.pool)
            .await
            .unwrap();
        let name = unique("vh-mask");
        let body = json!({
            "name": name, "service": env.tag_service, "serviceType": "tag",
            "policyType": 1, "isEnabled": true, "policyPriority": 0,
            "resources": {"tag": {"values": [key], "isExcludes": false, "isRecursive": false}},
            "dataMaskPolicyItems": [{
                "users": [], "groups": ["public"], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": mask}
            }],
            "policyItems": [], "denyPolicyItems": []
        });
        env.policy
            .create_policy(env.tag_service_id, &name, &body)
            .await
            .expect("hand-written tag mask");
    }
    let before = env.tag_version().await;

    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let msg = message(&body);
    assert!(
        msg.contains("the classification tags cannot be assigned"),
        "{msg}"
    );
    assert!(msg.contains("resolve the overlapping"), "{msg}");
    assert!(env.view("v1").await.is_none());
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
    assert_eq!(
        env.tag_version().await,
        before,
        "the refused write rolled back"
    );
}

/// Fix round 3 (a): a commit that adds a version but leaves the current one in
/// place changes nothing a reader sees. It passes untouched: no EXPLAIN, and
/// the recorded lineage stays that of the version that is still current.
#[tokio::test]
async fn adding_a_version_without_making_it_current_keeps_the_lineage() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let calls = env.runner.calls.load(Ordering::SeqCst);
    let view = env.view("v1").await.unwrap();
    let uuid = view["metadata"]["view-uuid"].as_str().unwrap();
    let body = json!({
        "requirements": [{"type": "assert-view-uuid", "uuid": uuid}],
        "updates": [
            {"action": "add-schema", "schema": schema(1, &["id", "name"])},
            {"action": "add-view-version", "view-version": version(2, -1, "SELECT id, name FROM orders")}
        ]
    });
    let (status, resp) = env.call("POST", "namespaces/s/views/v1", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(
        current_sql(&env.view("v1").await.unwrap()),
        "SELECT id, ssn FROM pii"
    );
    assert_eq!(env.runner.calls.load(Ordering::SeqCst), calls, "no EXPLAIN");
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "ssn"])
    );
}

/// Task 9 fix round 2, N1: a rename the hold refuses (here, no plugin
/// confirms) compensates. The destination keeps none of the rows the hold
/// copied onto it, the source's manual tag included, and the source keeps
/// its own.
#[tokio::test]
async fn a_refused_rename_leaves_nothing_on_the_destination() {
    let _shared = shared().await;
    let Some(env) = env(Duration::from_secs(2), true).await else {
        return;
    };
    for name in ["v1", "v2"] {
        let (status, body) = env
            .call_raw(
                "POST",
                "namespaces/s/views",
                Some(create_body(name, "SELECT id FROM orders")),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let manual = new_tag(&env.db).await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', 'v1', 'id')",
    )
    .bind(manual)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .unwrap();

    let (status, body) = env
        .call(
            "POST",
            "views/rename",
            Some(json!({"source": {"namespace": ["s"], "name": "v1"},
                        "destination": {"namespace": ["s"], "name": "v2"}})),
        )
        .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        env.assignments("v2").await.is_empty(),
        "{:?}",
        env.assignments("v2").await
    );
    assert_eq!(
        env.assignments("v1").await,
        BTreeSet::from([(manual, "id".to_string())])
    );
    assert_eq!(env.derived_flags("v1").await, vec![false]);
}

/// Task 9 fix round 2, N2: a best-effort record on an unclassified profile
/// takes the view's lock. While another writer holds it (here the test), the
/// hold does not EXPLAIN a version it might record out of order. The commit
/// goes through at once, with no wait (an unclassified view never waits on a
/// lock). Final review I-1: no task waits for the lock to free, either; the
/// other writer records, and the sweep repairs a lineage that may be stale.
#[tokio::test]
async fn a_best_effort_record_under_a_busy_view_lock_leaves_the_lineage_to_the_sweep() {
    let _serial = serial().await;
    let Some(env) = env(LIMIT, false).await else {
        return;
    };
    let mut other = env.db.pool.acquire().await.unwrap().detach();
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(view_lock_key(&env.prefix, "s", "v1"))
        .execute(&mut other)
        .await
        .unwrap();

    let (status, body) = tokio::time::timeout(
        Duration::from_secs(10),
        env.create("v1", "SELECT id, ssn FROM pii"),
    )
    .await
    .expect("an unclassified view waited for a lock");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.runner.calls.load(Ordering::SeqCst),
        0,
        "no EXPLAIN under a busy lock"
    );
    // The commit's one try at recording runs in the background, while the
    // other writer still holds the view.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        env.lineage("v1").await,
        RecordedLineage::Missing,
        "nothing is recorded while the other writer holds the view"
    );
    sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(view_lock_key(&env.prefix, "s", "v1"))
        .execute(&mut other)
        .await
        .unwrap();

    // Final review I-1: no task waits for the lock to free. The other writer
    // records, and what it records is left to the sweep.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        env.lineage("v1").await,
        RecordedLineage::Missing,
        "no background task waited for the view's lock"
    );
    sqlx::Connection::close(other).await.unwrap();
}

/// Final review I-1: a burst of best-effort commits beyond the cap. The
/// commit that holds the permit runs its EXPLAIN; every other commit passes
/// through at once, with no EXPLAIN and no lock connection of its own, and its
/// lineage is recorded `failed` (its view's lock is free) for the sweep, once
/// its record task gets a permit (re-review N-6).
#[tokio::test]
async fn a_best_effort_burst_beyond_the_cap_explains_once_and_never_waits() {
    let _serial = serial().await;
    let Some(env) = env_with(Opts {
        classified: false,
        max_holds: 1,
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    let names = ["b0", "b1", "b2", "b3"];
    let start = Instant::now();
    let (first, rest) = tokio::join!(
        env.create(names[0], "SELECT id, ssn FROM pii WHERE SLOW1"),
        async {
            // The first commit holds the permit across its 1.5 s EXPLAIN.
            tokio::time::sleep(Duration::from_millis(500)).await;
            let started = Instant::now();
            let sql = "SELECT id, ssn FROM pii WHERE SLOW1";
            let (a, b, c) = tokio::join!(
                env.create(names[1], sql),
                env.create(names[2], sql),
                env.create(names[3], sql)
            );
            (vec![a, b, c], started.elapsed())
        }
    );
    let (others, others_took) = rest;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    for (status, body) in &others {
        assert_eq!(*status, StatusCode::OK, "{body}");
    }
    assert_eq!(
        env.runner.calls.load(Ordering::SeqCst),
        1,
        "only the commit holding the permit runs an EXPLAIN"
    );
    assert!(
        others_took < Duration::from_millis(1200),
        "the commits past the cap waited: {others_took:?}"
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(
        env.lineage(names[0]).await,
        computed(&["pii"], &env.catalog)
    );
    let e = &env;
    for name in &names[1..] {
        eventually("a commit past the cap is recorded failed", || async move {
            matches!(e.lineage(name).await, RecordedLineage::Failed { .. })
        })
        .await;
    }
}

/// Final review I-2: a client that disconnects during the wait (hyper drops
/// the handler future) does not skip the compensation. The hold's work after
/// the lock runs in a task the request's cancellation cannot drop, so the
/// wait still times out, lakekeeper is never asked, and the rows and lineage
/// step 3 wrote are removed.
#[tokio::test]
async fn a_request_cancelled_during_the_wait_still_compensates() {
    let _serial = serial().await;
    let limit = Duration::from_secs(3);
    let Some(env) = env(limit, true).await else {
        return;
    };
    let before = env.tag_version().await;
    // No plugin polls, so the hold waits out its deadline. The request is
    // dropped the moment step 3's rows exist, i.e. mid-wait.
    // Boxed, so `drop` drops the handler future itself, as hyper does.
    let mut request = Box::pin(env.create("v1", "SELECT id, ssn FROM pii"));
    let written = async {
        while env.assignments("v1").await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        r = &mut request => panic!("the hold answered before step 3 wrote: {r:?}"),
        _ = written => {}
    }
    drop(request);

    let e = &env;
    eventually("the cancelled hold compensated", || async move {
        e.assignments("v1").await.is_empty() && e.lineage("v1").await == RecordedLineage::Missing
    })
    .await;
    assert!(env.view("v1").await.is_none(), "lakekeeper never committed");
    assert!(env.reached("POST /namespaces/s/views").is_empty());
    assert!(
        env.tag_version().await > before,
        "compensation bumped again"
    );
}

/// Task 9 fix round 2, N4: the hold's write waits for the view-tag lock no
/// longer than its deadline. With the lock held elsewhere for longer, the
/// hold refuses cleanly, in time, and writes nothing.
#[tokio::test]
async fn a_hold_that_cannot_take_the_view_tag_lock_in_time_refuses_cleanly() {
    let _serial = serial().await;
    let limit = Duration::from_secs(3);
    let Some(env) = env(limit, true).await else {
        return;
    };
    let before = env.tag_version().await;
    let mut blocker = env.db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('egret-view-tags', 0))")
        .execute(&mut *blocker)
        .await
        .unwrap();

    let start = Instant::now();
    let got = tokio::time::timeout(
        limit + Duration::from_secs(3),
        env.create("v1", "SELECT id, ssn FROM pii"),
    )
    .await;
    let took = start.elapsed();
    blocker.rollback().await.unwrap();

    let (status, body) = got.expect("the hold waited past its deadline");
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        took < limit + Duration::from_secs(2),
        "refused after {took:?}"
    );
    assert!(env.view("v1").await.is_none());
    assert!(env.assignments("v1").await.is_empty());
    assert_eq!(env.lineage("v1").await, RecordedLineage::Missing);
    assert_eq!(env.tag_version().await, before);
}

/// Task 9 fix round 2, N6: step 2's race. The hold reads an empty tag set,
/// then a base of the view is classified before the hold records the
/// lineage. The after-commit record re-tags under the lock, so the view
/// carries the tag on every column.
#[tokio::test]
async fn a_classification_during_a_step_two_commit_reaches_the_view() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let tag = env.clearance.unwrap();
    *env.on_reach.lock().unwrap() = Some((
        tag,
        env.warehouse_id,
        "orders".to_string(),
        "id".to_string(),
    ));

    let (status, body) = env.create("v1", "SELECT id, name FROM orders").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        env.on_reach.lock().unwrap().is_none(),
        "the classification ran"
    );
    assert_eq!(env.lineage("v1").await, computed(&["orders"], &env.catalog));
    assert_eq!(
        env.assignments("v1").await,
        env.expected_pii_tags(&["id", "ssn"])
    );
    assert_eq!(env.derived_flags("v1").await, vec![true, true]);
}

/// Re-review N-1: on a profile with no classified column, a client that
/// disconnects after lakekeeper commits the view but before the hold records
/// its lineage does not skip the record. The best-effort path's work after
/// its permit runs in a task the request's cancellation cannot drop.
#[tokio::test]
async fn a_best_effort_commit_cancelled_after_lakekeeper_commits_still_records() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, false).await else {
        return;
    };

    env.cancel_after_commit(
        "POST /namespaces/s/views",
        env.create("v1", "SELECT id, ssn FROM pii"),
    )
    .await;

    assert!(env.view("v1").await.is_some(), "lakekeeper committed");
    let e = &env;
    eventually("the cancelled best-effort commit recorded", || async move {
        e.lineage("v1").await == computed(&["pii"], &e.catalog)
    })
    .await;
    assert_eq!(
        env.columns("v1").await,
        Some(vec!["id".into(), "ssn".into()])
    );
    assert_eq!(env.runner.calls.load(Ordering::SeqCst), 1);
}

/// Re-review test gap: a classified rename whose client disconnects after
/// lakekeeper commits still drops what the source held.
#[tokio::test]
async fn a_rename_cancelled_after_lakekeeper_commits_still_moves_everything() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tags = env.assignments("v1").await;
    assert!(!tags.is_empty());

    env.cancel_after_commit(
        "POST /views/rename",
        env.call(
            "POST",
            "views/rename",
            Some(json!({"source": {"namespace": ["s"], "name": "v1"},
                        "destination": {"namespace": ["s"], "name": "v2"}})),
        ),
    )
    .await;

    let e = &env;
    eventually(
        "the cancelled rename dropped the source's rows",
        || async move {
            e.assignments("v1").await.is_empty()
                && e.lineage("v1").await == RecordedLineage::Missing
        },
    )
    .await;
    assert!(env.view("v1").await.is_none());
    assert!(env.view("v2").await.is_some());
    assert_eq!(env.assignments("v2").await, tags);
    assert_eq!(env.lineage("v2").await, computed(&["pii"], &env.catalog));
}

/// Re-review test gap: a classified drop whose client disconnects after
/// lakekeeper commits still forgets the view's lineage and derived tags.
#[tokio::test]
async fn a_drop_cancelled_after_lakekeeper_commits_still_forgets_the_view() {
    let _shared = shared().await;
    let Some(env) = env(LIMIT, true).await else {
        return;
    };
    let plugin = env.start_plugin();
    plugin.polled().await;
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!env.assignments("v1").await.is_empty());

    env.cancel_after_commit(
        "DELETE /namespaces/s/views/v1",
        env.call("DELETE", "namespaces/s/views/v1", None),
    )
    .await;

    let e = &env;
    eventually("the cancelled drop forgot the view", || async move {
        e.assignments("v1").await.is_empty() && e.lineage("v1").await == RecordedLineage::Missing
    })
    .await;
    assert!(env.view("v1").await.is_none());
}

/// How many backends hold the transaction-level advisory lock
/// `record_failed_if_free` takes on one of `keys`: the overflow record tasks
/// that hold a pooled connection right now.
async fn record_holders(conn: &mut sqlx::PgConnection, keys: &[String]) -> i64 {
    sqlx::query_scalar(
        "SELECT count(DISTINCT l.pid) FROM pg_locks l \
           JOIN (SELECT hashtextextended(k, 0) AS h FROM unnest($1::text[]) k) x \
             ON l.locktype = 'advisory' AND l.granted AND l.objsubid = 1 \
            AND l.classid = ((x.h >> 32) & 4294967295)::oid \
            AND l.objid = (x.h & 4294967295)::oid \
          WHERE l.database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .bind(keys)
    .fetch_one(conn)
    .await
    .unwrap()
}

/// Re-review N-6 (fix round 1): the overflow records are bounded by the
/// best-effort cap, and wait for a permit rather than skip. With the cap at 1
/// and the view-tag lock held here, ten commits past the cap each return at
/// once. When the permit frees, exactly `cap` record tasks hold a pooled
/// connection (each waiting on the view-tag lock); the others wait for a
/// permit holding none. Once the lock frees, every view is recorded `failed`.
#[tokio::test]
async fn overflow_records_never_hold_more_connections_than_the_cap() {
    let _serial = serial().await;
    const CAP: usize = 1;
    let Some(env) = env_with(Opts {
        classified: false,
        max_holds: CAP,
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/s/views",
            Some(create_body("dup", "SELECT id FROM orders")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The blocker and the sampler stay out of the hold's own pool.
    let dsn = egret_test_db::url().await.expect("database");
    let side = Db::connect(&dsn, 2).await.expect("connect");
    let mut sampler = side.pool.acquire().await.unwrap();
    let mut blocker = side.pool.begin().await.unwrap();
    egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut blocker)
        .await
        .expect("take the view-tag lock");
    let names: Vec<String> = (0..10).map(|i| format!("o{i}")).collect();
    let keys: Vec<String> = names
        .iter()
        .map(|n| view_lock_key(&env.prefix, "s", n))
        .collect();

    // The one permit is taken for ~1.5 s, then freed while the view-tag
    // lock is still held here.
    let rest = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut set = tokio::task::JoinSet::new();
        for name in &names {
            let (app, prefix) = (env.app.clone(), env.prefix.clone());
            let body = create_body(name, "SELECT id FROM orders");
            set.spawn(async move {
                let started = Instant::now();
                let got = Env::send(&app, &prefix, "POST", "namespaces/s/views", Some(body)).await;
                (got, started.elapsed())
            });
        }
        let answers: Vec<_> = set.join_all().await;
        // Sample until the freed permit has been taken, then 500 ms more, so
        // the view-tag lock (global; other tests want it) is held briefly.
        let mut most = 0;
        let mut until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            let now = record_holders(&mut sampler, &keys).await;
            if now > 0 && most == 0 {
                until = until.min(Instant::now() + Duration::from_millis(500));
            }
            most = most.max(now);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        blocker.rollback().await.unwrap();
        (answers, most)
    };
    let ((), (answers, most)) = tokio::join!(hold_the_permit_briefly(&env, "dup"), rest);

    assert_eq!(answers.len(), names.len());
    for ((status, body), took) in &answers {
        assert_eq!(*status, StatusCode::OK, "{body}");
        assert!(
            *took < Duration::from_millis(1200),
            "a commit past the cap waited {took:?}"
        );
    }
    assert_eq!(
        most, CAP as i64,
        "overflow record tasks holding a connection at once (cap {CAP})"
    );
    assert_eq!(
        env.runner.calls.load(Ordering::SeqCst),
        1,
        "only the permit holder ran an EXPLAIN"
    );
    // No record was skipped: each waited for the permit.
    let e = &env;
    for name in &names {
        eventually("an overflow record landed failed", || async move {
            matches!(e.lineage(name).await, RecordedLineage::Failed { .. })
        })
        .await;
    }
}

/// Take the one best-effort permit (cap 1) for about 1.5 s, then free it: a
/// create of `name`, which already exists, whose EXPLAIN is slow. lakekeeper
/// refuses it, so it records nothing and takes no view-tag lock.
async fn hold_the_permit_briefly(env: &Env, name: &str) {
    let (status, body) = env
        .create(name, "SELECT id, ssn FROM pii WHERE SLOW1")
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "the duplicate create: {body}");
}

/// Task 8 fix round 1 (c): a replace past the cap must not leave the view's
/// previous `computed` lineage in place. It is recorded `failed`, which the
/// classification sweep (`sweep_unjudged`) picks up; a stale `computed` one
/// it skips, so a classification of a table only the new definition reads
/// would reach the view only at the scheduled sweep.
#[tokio::test]
async fn an_over_cap_replace_replaces_the_stale_lineage_with_failed() {
    let _serial = serial().await;
    let Some(env) = env_with(Opts {
        classified: false,
        max_holds: 1,
        ..Opts::default()
    })
    .await
    else {
        return;
    };
    let (status, body) = env.create("v1", "SELECT id, ssn FROM pii").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(env.lineage("v1").await, computed(&["pii"], &env.catalog));
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/s/views",
            Some(create_body("dup", "SELECT id FROM orders")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = env.view("v1").await.unwrap();

    let ((), (status, body)) = tokio::join!(hold_the_permit_briefly(&env, "dup"), async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        env.call(
            "POST",
            "namespaces/s/views/v1",
            Some(replace_body(&view, "SELECT id, name FROM orders")),
        )
        .await
    });

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        current_sql(&env.view("v1").await.unwrap()),
        "SELECT id, name FROM orders"
    );
    assert_eq!(
        env.runner.calls.load(Ordering::SeqCst),
        2,
        "the replace past the cap ran no EXPLAIN"
    );
    let e = &env;
    eventually("the over-cap replace recorded failed", || async move {
        matches!(e.lineage("v1").await, RecordedLineage::Failed { .. })
    })
    .await;
}
