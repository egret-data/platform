//! D2 (StarRocks whole-table classification, spec §5.2 and §5.5): the column
//! source read against the REAL embedded Lakekeeper (the drift guard C3), and
//! the table-commit hook.
//!
//! Each test drives Lakekeeper (on its in-memory storage) through
//! [`front_catalog_router`].
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-catalog-engine --test table_hook_db

#[path = "common/plugins.rs"]
mod plugins;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, load_table_detail, run_lakekeeper_migrations, set_delete_profile,
    table_hook, ApiContext, EngineState, TableHook, LAKEKEEPER_SCHEMA,
};
use egret_metastore::Db;
use egret_policy::PolicyStore;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

/// The hook's move deadline in these tests.
const LIMIT: Duration = Duration::from_secs(5);
/// How often the fake plugins poll.
const POLL: Duration = Duration::from_millis(200);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

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

struct Env {
    db: Db,
    state: ApiContext<EngineState>,
    /// Lakekeeper with no hook in front.
    raw: axum::Router,
    /// Lakekeeper behind the table hook, with [`observe`] between them, behind
    /// a [`StubGuard`] that names [`Env::caller`] as the request's caller.
    app: axum::Router,
    /// The same, with no guard at all.
    unguarded: axum::Router,
    /// The token subject the stub guard reports.
    caller: Arc<Mutex<String>>,
    prefix: String,
    warehouse_id: Uuid,
    /// The registry catalog over the warehouse.
    catalog: String,
    /// The StarRocks profile bound to the catalog.
    profile_id: i64,
    profile_name: String,
    /// The Trino profile bound to the catalog (with its tag service).
    trino_id: i64,
    trino_name: String,
    /// The Trino engine bound through it.
    trino_engine: Uuid,
    /// The StarRocks engine bound through the StarRocks profile.
    starrocks_engine: Uuid,
    policy: PolicyStore,
    on_commit: OnCommit,
    on_reach: OnReach,
    rows_at_reach: RowsAtReach,
    stall: Stall,
    fail_after_commit: FailAfterCommit,
    hook: Arc<TableHook>,
}

/// What the catalog held when a request reached lakekeeper: armed with
/// `(METHOD /path, warehouse, None)`, [`observe`] fills in the moment, the
/// tables the warehouse's classification rows name, the Trino profile's tag
/// version and the StarRocks profile's policy version.
type RowsAtReach = Arc<Mutex<Option<(String, Uuid, (i64, i64), Option<AtReach>)>>>;

#[derive(Debug, Clone)]
struct AtReach {
    at: Instant,
    tables: Vec<String>,
    tag_version: i64,
    policy_version: i64,
}

/// Armed with `(METHOD /path, signal)`: [`observe`] signals when that request
/// reaches lakekeeper, then holds it for [`STALL`] before forwarding it.
type Stall = Arc<Mutex<Option<(String, Arc<tokio::sync::Notify>)>>>;
const STALL: std::time::Duration = std::time::Duration::from_secs(2);

/// What the profile looked like when a request reached lakekeeper: armed with
/// `(METHOD /path, profile id, None)`, [`observe`] fills in its policies and
/// its last compile.
type Reached = (Value, Option<(String, chrono::DateTime<chrono::Utc>)>);
type OnReach = Arc<Mutex<Option<(String, i64, Option<Reached>)>>>;

/// A database write [`observe`] runs once lakekeeper has answered a matching
/// request 2xx, before the hook sees the answer: `(METHOD /path, SQL, tag)`.
type OnCommit = Arc<Mutex<Option<(String, &'static str, Uuid)>>>;

/// Armed with `METHOD /path`: [`observe`] answers that request 500 once
/// lakekeeper has committed it (an error after the commit, review finding 4).
type FailAfterCommit = Arc<Mutex<Option<String>>>;

/// Runs the armed [`OnCommit`] write after lakekeeper commits — a change the
/// hook's pre-commit work cannot have seen.
async fn observe(
    State((on_commit, on_reach, rows_at_reach, stall, fail_after, db)): State<(
        OnCommit,
        OnReach,
        RowsAtReach,
        Stall,
        FailAfterCommit,
        Db,
    )>,
    req: Request,
    next: Next,
) -> Response {
    let what = format!("{} {}", req.method(), req.uri().path());
    let at = Instant::now();
    let armed = rows_at_reach
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(w, _, _, seen)| *w == what && seen.is_none())
        .map(|(_, warehouse, profiles, _)| (*warehouse, *profiles));
    if let Some((warehouse, (trino, starrocks))) = armed {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT table_name FROM catalog_tag_assignments WHERE warehouse_id = $1 \
             ORDER BY 1",
        )
        .bind(warehouse)
        .fetch_all(&db.pool)
        .await
        .expect("rows at reach");
        let tag_version: i64 =
            sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
                .bind(trino)
                .fetch_one(&db.pool)
                .await
                .expect("tag version at reach");
        let policy_version: i64 =
            sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
                .bind(starrocks)
                .fetch_one(&db.pool)
                .await
                .expect("policy version at reach");
        if let Some(slot) = rows_at_reach.lock().unwrap().as_mut() {
            slot.3 = Some(AtReach {
                at,
                tables,
                tag_version,
                policy_version,
            });
        }
    }
    let stalled = {
        let mut slot = stall.lock().unwrap();
        match slot.as_ref() {
            Some((w, _)) if *w == what => slot.take().map(|(_, signal)| signal),
            _ => None,
        }
    };
    if let Some(signal) = stalled {
        signal.notify_one();
        tokio::time::sleep(STALL).await;
    }
    let probe = on_reach
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(w, _, seen)| *w == what && seen.is_none())
        .map(|(_, profile, _)| *profile);
    if let Some(profile) = probe {
        let policies: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(jsonb_build_array(name, policy_jsonb) ORDER BY name), \
                    '[]'::jsonb) \
               FROM policy.authz_policies WHERE service_id = $1",
        )
        .bind(profile)
        .fetch_one(&db.pool)
        .await
        .expect("policies at reach");
        let run: Option<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT outcome, attempted_at FROM perm.compile_runs WHERE service_id = $1",
        )
        .bind(profile)
        .fetch_optional(&db.pool)
        .await
        .expect("compile run at reach");
        if let Some(slot) = on_reach.lock().unwrap().as_mut() {
            slot.2 = Some((policies, run));
        }
    }
    let response = next.run(req).await;
    let fail = response.status().is_success()
        && fail_after.lock().unwrap().take_if(|w| *w == what).is_some();
    if fail {
        // Lakekeeper committed; the client is told it failed.
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if response.status().is_success() {
        let armed = {
            let mut slot = on_commit.lock().unwrap();
            match slot.as_ref() {
                Some((w, _, _)) if *w == what => slot.take(),
                _ => None,
            }
        };
        if let Some((_, sql, tag)) = armed {
            sqlx::query(sql)
                .bind(tag)
                .execute(&db.pool)
                .await
                .expect("on-commit write");
        }
    }
    response
}

/// A lakekeeper warehouse with namespace `sales`, and the egret warehouse over
/// it; the hook holds a move at most [`LIMIT`].
async fn env() -> Option<Env> {
    env_with_limit(LIMIT).await
}

async fn env_with_limit(limit: Duration) -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    lakekeeper_ready(&dsn).await;
    let (state, _registry) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("thwh"))
        .await
        .expect("lakekeeper warehouse");
    let pool = &db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'TH') RETURNING id")
            .bind(unique("th-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-th-test','AKIATEST') RETURNING id",
    )
    .bind(unique("th-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(unique("th_wh"))
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    let trino_name = unique("trino_th");
    let tag_service = format!("{trino_name}_tag");
    policy
        .create_service(&tag_service, "tag", &json!({}))
        .await
        .expect("tag service");
    let trino = policy
        .create_service(&trino_name, "trino", &json!({"tagService": tag_service}))
        .await
        .expect("trino profile");
    let trino_engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("th-trino"))
    .bind(trino.id)
    .fetch_one(pool)
    .await
    .expect("trino engine");
    let profile = policy
        .create_service(&unique("sr_th"), "starrocks", &json!({}))
        .await
        .expect("profile");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret', \
                 '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) \
         RETURNING id",
    )
    .bind(unique("th-eng"))
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("th_cat");
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
    for engine in [engine, trino_engine] {
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) \
             VALUES ($1,$2)",
        )
        .bind(engine)
        .bind(catalog_id)
        .execute(pool)
        .await
        .expect("attach");
    }

    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let on_commit: OnCommit = Arc::default();
    let on_reach: OnReach = Arc::default();
    let rows_at_reach: RowsAtReach = Arc::default();
    let stall: Stall = Arc::default();
    let fail_after_commit: FailAfterCommit = Arc::default();
    let inner = catalog_engine_router(state.clone()).layer(axum::middleware::from_fn_with_state(
        (
            on_commit.clone(),
            on_reach.clone(),
            rows_at_reach.clone(),
            stall.clone(),
            fail_after_commit.clone(),
            db.clone(),
        ),
        observe,
    ));
    let hook = Arc::new(TableHook::new(db.clone()).with_limit(limit));
    let hooked = table_hook::layer(inner, hook.clone());
    let caller: Arc<Mutex<String>> = Arc::new(Mutex::new(unique("no-such-client")));
    let guard: Arc<dyn egret_catalog_engine::CatalogGuard> = Arc::new(StubGuard(caller.clone()));
    let app = front_catalog_router(hooked.clone(), Some(guard), None);
    let unguarded = front_catalog_router(hooked, None, None);
    let env = Env {
        db,
        state,
        raw,
        app,
        unguarded,
        caller,
        prefix,
        warehouse_id,
        catalog,
        profile_id: profile.id,
        profile_name: profile.name,
        trino_id: trino.id,
        trino_name,
        trino_engine,
        starrocks_engine: engine,
        policy,
        on_commit,
        on_reach,
        rows_at_reach,
        stall,
        fail_after_commit,
        hook,
    };
    let (status, body) = env
        .call_raw("POST", "namespaces", Some(json!({"namespace": ["sales"]})))
        .await;
    assert_eq!(status, StatusCode::OK, "create namespace: {body}");
    Some(env)
}

/// A guard that lets every request through, by the subject in its slot, as
/// the production guard does with a verified catalog token. It reads a
/// rename's body, as the production guard does.
struct StubGuard(Arc<Mutex<String>>);

#[async_trait::async_trait]
impl egret_catalog_engine::CatalogGuard for StubGuard {
    async fn check(
        &self,
        _method: &str,
        path: &str,
        _query: Option<&str>,
        _bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<egret_catalog_engine::CatalogCaller, (StatusCode, String)> {
        // The mount hands the guard a rename's body and puts it back, so the
        // hook behind it moves classifications on the same bytes.
        assert_eq!(
            body.is_some(),
            path.ends_with("/tables/rename") || path.ends_with("/views/rename"),
            "the guard reads the body of a rename only: {path}"
        );
        Ok(egret_catalog_engine::CatalogCaller {
            subject: self.0.lock().unwrap().clone(),
            audience: "catalog".into(),
        })
    }
}

impl Env {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        send(&self.app, &self.prefix, method, path, body).await
    }

    async fn call_raw(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        send(&self.raw, &self.prefix, method, path, body).await
    }

    /// What the column source answers for `sales.{table}`.
    async fn source(&self, table: &str) -> Option<Vec<String>> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm_compile::table_columns::current_columns_via(
            &mut conn,
            self.warehouse_id,
            "sales",
            table,
        )
        .await
        .expect("column source")
        .map(|c| c.columns)
    }

    /// What the field source answers for `sales.{table}`: `(id, name)`.
    async fn source_fields(&self, table: &str) -> Option<Vec<(i64, String)>> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm_compile::table_columns::current_fields_via(
            &mut conn,
            self.warehouse_id,
            "sales",
            table,
        )
        .await
        .expect("field source")
        .map(|f| f.fields)
    }

    /// The current schema's top-level `(id, name)` pairs of `sales.{table}`,
    /// as Lakekeeper's Iceberg REST load returns them.
    async fn loaded_fields(&self, table: &str) -> Vec<(i64, String)> {
        let (status, body) = self
            .call_raw("GET", &format!("namespaces/sales/tables/{table}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "load {table}: {body}");
        let metadata = &body["metadata"];
        let current = &metadata["current-schema-id"];
        let schema = metadata["schemas"]
            .as_array()
            .expect("schemas")
            .iter()
            .find(|s| &s["schema-id"] == current)
            .expect("the current schema");
        egret_perm_compile::table_columns::top_level_fields(schema)
    }

    /// The top-level column names Lakekeeper itself loads for `sales.{table}`.
    async fn loaded(&self, table: &str) -> Vec<String> {
        load_table_detail(
            self.state.clone(),
            SYSTEM_USER,
            &self.prefix,
            &["sales".to_string()],
            table,
        )
        .await
        .expect("load table")
        .columns
        .into_iter()
        .map(|c| c.name)
        .collect()
    }
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

/// An Iceberg schema whose fields are `(id, name)`.
fn schema(id: i64, fields: &[(i64, &str)]) -> Value {
    let fields: Vec<Value> = fields
        .iter()
        .map(|(i, c)| json!({"id": i, "name": c, "type": "string", "required": false}))
        .collect();
    json!({"type": "struct", "schema-id": id, "fields": fields})
}

/// The commit that adds `schema` and makes it current.
fn schema_commit(schema: Value) -> Value {
    json!({
        "requirements": [],
        "updates": [
            {"action": "add-schema", "schema": schema},
            {"action": "set-current-schema", "schema-id": -1}
        ]
    })
}

/// C3, the drift guard: after every kind of schema change Lakekeeper accepts,
/// the column source answers exactly the top-level names Lakekeeper itself
/// loads (order included); a renamed or dropped table answers `None` under its
/// old name, and a view or a staged create is no table.
#[tokio::test]
async fn the_column_source_reads_what_lakekeeper_loads() {
    let Some(env) = env().await else { return };
    assert_eq!(
        egret_perm_compile::table_columns::LAKEKEEPER_SCHEMA,
        LAKEKEEPER_SCHEMA,
        "the column source's SQL names Lakekeeper's schema"
    );

    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({
                "name": "orders",
                "schema": schema(0, &[(1, "id"), (2, "email"), (3, "region")]),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create: {body}");
    assert_eq!(env.source("orders").await, Some(env.loaded("orders").await));
    assert_eq!(
        env.source_fields("orders").await,
        Some(env.loaded_fields("orders").await)
    );
    assert_eq!(
        env.source("orders").await,
        Some(vec!["id".into(), "email".into(), "region".into()])
    );

    let steps: [(&str, Vec<(i64, &str)>); 3] = [
        (
            "add phone",
            vec![(1, "id"), (2, "email"), (3, "region"), (4, "phone")],
        ),
        (
            "rename region to area",
            vec![(1, "id"), (2, "email"), (3, "area"), (4, "phone")],
        ),
        ("drop phone", vec![(1, "id"), (2, "email"), (3, "area")]),
    ];
    for (i, (what, fields)) in steps.iter().enumerate() {
        let (status, body) = env
            .call_raw(
                "POST",
                "namespaces/sales/tables/orders",
                Some(schema_commit(schema(i as i64 + 1, fields))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{what}: {body}");
        let want: Vec<String> = fields.iter().map(|(_, n)| n.to_string()).collect();
        assert_eq!(env.loaded("orders").await, want, "{what}: lakekeeper");
        assert_eq!(
            env.source("orders").await,
            Some(want),
            "{what}: column source"
        );
        let want: Vec<(i64, String)> = fields.iter().map(|(i, n)| (*i, n.to_string())).collect();
        assert_eq!(
            env.loaded_fields("orders").await,
            want,
            "{what}: lakekeeper"
        );
        assert_eq!(
            env.source_fields("orders").await,
            Some(want),
            "{what}: field source"
        );
    }
    // The rename kept field 3's id under its new name.
    assert!(env
        .source_fields("orders")
        .await
        .expect("orders")
        .contains(&(3, "area".to_string())));
    // The lookup folds case; the answer is the catalog's spelling.
    assert_eq!(env.source("ORDERS").await, env.source("orders").await);

    let (status, body) = env
        .call_raw(
            "POST",
            "tables/rename",
            Some(json!({
                "source": {"namespace": ["sales"], "name": "orders"},
                "destination": {"namespace": ["sales"], "name": "orders_v2"},
            })),
        )
        .await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.source("orders").await,
        None,
        "the old name after a rename"
    );
    assert_eq!(env.source_fields("orders").await, None);
    assert_eq!(
        env.source_fields("orders_v2").await,
        Some(env.loaded_fields("orders_v2").await)
    );
    assert_eq!(
        env.source("orders_v2").await,
        Some(env.loaded("orders_v2").await)
    );

    // A SOFT drop keeps the row with `deleted_at` set: the predicate that
    // matters is exercised only then.
    set_delete_profile(env.state.clone(), SYSTEM_USER, &env.prefix, Some(3600))
        .await
        .expect("soft delete profile");
    let (status, body) = env
        .call_raw("DELETE", "namespaces/sales/tables/orders_v2", None)
        .await;
    assert!(status.is_success(), "drop: {status} {body}");
    let soft: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM lakekeeper.tabular \
          WHERE warehouse_id::text = $1 AND name = 'orders_v2' AND deleted_at IS NOT NULL",
    )
    .bind(&env.prefix)
    .fetch_one(&env.db.pool)
    .await
    .expect("soft-deleted row");
    assert_eq!(
        soft, 1,
        "the drop must be soft for this step to test anything"
    );
    assert_eq!(env.source("orders_v2").await, None, "a dropped table");
    assert_eq!(
        env.source_fields("orders_v2").await,
        None,
        "a dropped table"
    );

    // A view of a table's name is no table.
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/views",
            Some(json!({
                "name": "v_orders",
                "schema": schema(0, &[(1, "id")]),
                "view-version": {
                    "version-id": 1, "schema-id": 0,
                    "timestamp-ms": 1_700_000_000_000_i64,
                    "summary": {"engine-name": "trino"},
                    "default-namespace": ["sales"],
                    "representations": [{"type": "sql", "sql": "SELECT 1 AS id", "dialect": "trino"}]
                },
                "properties": {}
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create view: {body}");
    assert_eq!(env.source("v_orders").await, None, "a view");
    assert_eq!(env.source_fields("v_orders").await, None, "a view");

    // A staged create has no metadata location until its committing update.
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({
                "name": "staged",
                "schema": schema(0, &[(1, "id")]),
                "stage-create": true,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "stage create: {body}");
    assert_eq!(env.source("staged").await, None, "a staged create");
}

// ── D2 Task 7: the table-commit hook, after-commit recompile (U2, C10, C12) ──

impl Env {
    /// A classification with a role-bearing clearance on the whole table
    /// `sales.{table}`, compiled into the profile.
    async fn classify_table(&self, table: &str) -> Uuid {
        let tag: Uuid =
            sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
                .bind(unique("th-tag"))
                .bind(unique("ThTag"))
                .fetch_one(&self.db.pool)
                .await
                .expect("tag");
        sqlx::query(
            "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
             column_name) VALUES ($1,$2,'sales',$3,'')",
        )
        .bind(tag)
        .bind(self.warehouse_id)
        .bind(table)
        .execute(&self.db.pool)
        .await
        .expect("assign");
        sqlx::query(
            "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
             VALUES ($1, $2, '{}')",
        )
        .bind(tag)
        .bind(vec![unique("th-role")])
        .execute(&self.db.pool)
        .await
        .expect("gate");
        self.recompile().await;
        tag
    }

    async fn recompile(&self) {
        let mut tx = self.db.pool.begin().await.expect("begin");
        egret_perm_compile::recompile_service_in_tx(&mut tx, self.profile_id)
            .await
            .expect("recompile");
        tx.commit().await.expect("commit");
    }

    /// The columns of `sales.{table}` the profile serves a clearance mask for
    /// under `tag`.
    async fn masked_columns(&self, tag: Uuid, table: &str) -> Vec<String> {
        let prefix = format!(
            "{}{tag}-mask-{}.sales.{table}.",
            egret_policy::COMPILER_CLEARANCE_PREFIX,
            self.catalog
        );
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM policy.authz_policies WHERE service_id = $1 AND name LIKE $2 || '%' \
             ORDER BY name",
        )
        .bind(self.profile_id)
        .bind(&prefix)
        .fetch_all(&self.db.pool)
        .await
        .expect("masks");
        names
            .into_iter()
            .map(|n| n[prefix.len()..].to_string())
            .collect()
    }

    async fn has_table_deny(&self, tag: Uuid, table: &str) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM policy.authz_policies \
              WHERE service_id = $1 AND name = $2)",
        )
        .bind(self.profile_id)
        .bind(format!(
            "{}{tag}-{}.sales.{table}.*",
            egret_policy::COMPILER_CLEARANCE_PREFIX,
            self.catalog
        ))
        .fetch_one(&self.db.pool)
        .await
        .expect("deny")
    }

    /// When the profile last compiled, and how (`perm.compile_runs`).
    async fn last_compile(&self) -> Option<(String, chrono::DateTime<chrono::Utc>)> {
        sqlx::query_as("SELECT outcome, attempted_at FROM perm.compile_runs WHERE service_id = $1")
            .bind(self.profile_id)
            .fetch_optional(&self.db.pool)
            .await
            .expect("compile run")
    }

    async fn create_table(&self, table: &str, fields: &[(i64, &str)]) {
        let (status, body) = self
            .call(
                "POST",
                "namespaces/sales/tables",
                Some(json!({"name": table, "schema": schema(0, fields)})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create {table}: {body}");
    }
}

/// U2: a column added to a table classified as a whole gets its NULL mask
/// before the Iceberg commit's response returns, with no other write.
#[tokio::test]
async fn adding_a_column_to_a_classified_table_masks_it_before_the_commit_returns() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let tag = env.classify_table("orders").await;
    assert_eq!(env.masked_columns(tag, "orders").await, vec!["email", "id"]);

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(schema_commit(schema(
                1,
                &[(1, "id"), (2, "email"), (3, "phone")],
            ))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "add phone: {body}");
    assert_eq!(
        env.masked_columns(tag, "orders").await,
        vec!["email", "id", "phone"]
    );
}

/// A dropped table has no current columns: the recompile reaps its masks and
/// keeps the `*` deny (U4), so a table recreated under the name stays denied.
#[tokio::test]
async fn a_drop_reaps_the_masks_and_keeps_the_deny() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let tag = env.classify_table("orders").await;
    assert_eq!(env.masked_columns(tag, "orders").await.len(), 2);

    let (status, body) = env
        .call("DELETE", "namespaces/sales/tables/orders", None)
        .await;
    assert!(status.is_success(), "drop: {status} {body}");
    assert!(env.masked_columns(tag, "orders").await.is_empty());
    assert!(env.has_table_deny(tag, "orders").await);
}

/// C12: a schema change on a table no whole-table classification is on runs
/// no compile (the profile's last compile is untouched); neither does an
/// append on the classified one.
#[tokio::test]
async fn a_commit_on_an_unclassified_table_runs_no_compile() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id")]).await;
    env.create_table("events", &[(1, "id")]).await;
    env.classify_table("orders").await;
    let before = env.last_compile().await.expect("compiled once");

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/events",
            Some(schema_commit(schema(1, &[(1, "id"), (2, "kind")]))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "events: {body}");
    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(json!({"requirements": [], "updates": [
                {"action": "set-properties", "updates": {"owner": "x"}}
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "orders properties: {body}");
    assert_eq!(env.last_compile().await, Some(before));
}

/// C10: the commit has happened, so a recompile that fails afterwards does not
/// undo it: the 2xx passes, and the failure is recorded for the Governance
/// Overview. The failure is caused after lakekeeper commits ([`observe`]): 501
/// per-column attachments of the tag push its clearance over the ceiling.
#[tokio::test]
async fn a_failed_recompile_still_returns_the_commit_and_is_recorded() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id")]).await;
    let tag = env.classify_table("orders").await;
    *env.on_commit.lock().unwrap() = Some((
        format!("POST /{}/namespaces/sales/tables/orders", env.prefix),
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) SELECT $1, a.warehouse_id, 'sales', 'wide', 'c' || gs::text \
         FROM catalog_tag_assignments a, generate_series(1, 501) gs \
         WHERE a.tag_id = $1 AND a.column_name = ''",
        tag,
    ));

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(schema_commit(schema(1, &[(1, "id"), (2, "phone")]))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "the commit stands: {body}");
    assert!(env.on_commit.lock().unwrap().is_none(), "the observer ran");
    assert_eq!(
        env.loaded("orders").await,
        vec!["id".to_string(), "phone".to_string()]
    );
    let (outcome, _) = env.last_compile().await.expect("a compile run");
    assert_eq!(outcome, "error");
    let error: String =
        sqlx::query_scalar("SELECT error FROM perm.compile_runs WHERE service_id = $1")
            .bind(env.profile_id)
            .fetch_one(&env.db.pool)
            .await
            .expect("error");
    assert!(error.contains("ceiling"), "{error}");
}

// ── D2 Task 8: the pre-commit dry run (C11, U5) ──

/// `n` fields `(1, "c1") .. (n, "cn")`.
fn fields(n: i64) -> Vec<(i64, String)> {
    (1..=n).map(|i| (i, format!("c{i}"))).collect()
}

fn refs(f: &[(i64, String)]) -> Vec<(i64, &str)> {
    f.iter().map(|(i, n)| (*i, n.as_str())).collect()
}

/// U5: a column add that would push a whole-table clearance over its ceiling
/// is refused BEFORE the commit with a 409 naming the table and the counts,
/// and the table keeps its schema; the profile is not wedged.
#[tokio::test]
async fn a_column_add_that_would_cross_the_ceiling_is_refused_before_commit() {
    let Some(env) = env().await else { return };
    // 1 deny + 498 masks = 499, one under the per-clearance ceiling of 500.
    let f = fields(498);
    env.create_table("wide", &refs(&f)).await;
    let tag = env.classify_table("wide").await;
    assert_eq!(env.masked_columns(tag, "wide").await.len(), 498);

    let grown = fields(500);
    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/wide",
            Some(schema_commit(schema(1, &refs(&grown)))),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().expect("message");
    assert!(
        message.starts_with("egret refused the table change: "),
        "{message}"
    );
    assert!(
        message.contains(&format!("{}.sales.wide", env.catalog)),
        "{message}"
    );
    assert!(message.contains("501"), "{message}");
    assert_eq!(env.loaded("wide").await.len(), 498, "nothing committed");
    assert_eq!(env.masked_columns(tag, "wide").await.len(), 498);
}

impl Env {
    /// Every policy the profile holds, as `[name, body]` pairs by name.
    async fn policies(&self) -> Value {
        sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(jsonb_build_array(name, policy_jsonb) ORDER BY name), \
                    '[]'::jsonb) \
               FROM policy.authz_policies WHERE service_id = $1",
        )
        .bind(self.profile_id)
        .fetch_one(&self.db.pool)
        .await
        .expect("policies")
    }
}

/// A column add within the ceiling commits, and the dry run before it wrote
/// nothing: when the request reaches lakekeeper ([`observe`] records the
/// profile's state then), the profile's policies and its last compile are as
/// they were; after the response the new column is masked.
#[tokio::test]
async fn a_column_add_within_the_ceiling_commits() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let tag = env.classify_table("orders").await;
    let before = (env.policies().await, env.last_compile().await);
    *env.on_reach.lock().unwrap() = Some((
        format!("POST /{}/namespaces/sales/tables/orders", env.prefix),
        env.profile_id,
        None,
    ));

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(schema_commit(schema(
                1,
                &[(1, "id"), (2, "email"), (3, "phone")],
            ))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let at_reach = env
        .on_reach
        .lock()
        .unwrap()
        .take()
        .and_then(|(_, _, seen)| seen)
        .expect("the observer saw the request reach lakekeeper");
    assert_eq!(at_reach, before, "the dry run left nothing behind");
    assert_eq!(
        env.masked_columns(tag, "orders").await,
        vec!["email", "id", "phone"]
    );
}

/// A rename onto a name that carries a whole-table classification, from a
/// source that carries none, takes #755's path: it is dry-run with the
/// source's columns under the destination name, and a destination whose
/// classification the moved columns would push over the ceiling is refused
/// before the commit (review finding 7).
#[tokio::test]
async fn a_rename_onto_a_classified_name_still_dry_runs() {
    let Some(env) = env().await else { return };
    // `big` (1 + 497) and the not-yet-existing `target` (1 + 0) carry tag B:
    // 499. Renaming `small` (5 columns) onto `target` would make it 504.
    let big = fields(497);
    env.create_table("big", &refs(&big)).await;
    let small = fields(5);
    env.create_table("small", &refs(&small)).await;
    let tag_b = env.classify_table("big").await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales','target','')",
    )
    .bind(tag_b)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign target");
    env.recompile().await;
    let (status, body) = env.rename("small", "target").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("ceiling"),
        "{body}"
    );
    assert_eq!(
        env.source("small").await.map(|c| c.len()),
        Some(5),
        "not renamed"
    );
    env.recompile().await;
}

// ── D2 review fix 1: an undrop goes through the hook ──

impl Env {
    /// Drop `sales.{table}` softly (so it can be undropped), returning its
    /// tabular id.
    async fn soft_drop(&self, table: &str) -> Uuid {
        set_delete_profile(self.state.clone(), SYSTEM_USER, &self.prefix, Some(3600))
            .await
            .expect("soft delete profile");
        let (status, body) = self
            .call("DELETE", &format!("namespaces/sales/tables/{table}"), None)
            .await;
        assert!(status.is_success(), "drop {table}: {status} {body}");
        sqlx::query_scalar(
            "SELECT tabular_id FROM lakekeeper.tabular \
              WHERE warehouse_id::text = $1 AND name = $2 AND deleted_at IS NOT NULL",
        )
        .bind(&self.prefix)
        .bind(table)
        .fetch_one(&self.db.pool)
        .await
        .expect("soft-deleted table")
    }

    /// The recovery page's undrop, as `egret-bin` wires it.
    async fn undrop(&self, id: Uuid) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        self.hook
            .undrop(
                self.state.clone(),
                SYSTEM_USER,
                &self.prefix,
                egret_catalog_engine::TabularKindName::Table,
                id,
            )
            .await
    }
}

/// Review finding 1: the recovery page undrops a table in process, past the
/// Iceberg mount. A table classified as a whole must get its NULL masks back
/// at once: with the deny alone a view over it reads real values (live spike).
#[tokio::test]
async fn an_undropped_classified_table_gets_its_masks_back() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let tag = env.classify_table("orders").await;
    let id = env.soft_drop("orders").await;
    assert!(
        env.masked_columns(tag, "orders").await.is_empty(),
        "the drop reaps them"
    );

    env.undrop(id).await.expect("undrop");
    assert_eq!(env.masked_columns(tag, "orders").await, vec!["email", "id"]);
}

/// U5 for an undrop: a table whose columns would push its classification
/// over the ceiling is not undropped, and the profile is not wedged.
#[tokio::test]
async fn an_undrop_that_would_cross_the_ceiling_is_refused() {
    let Some(env) = env().await else { return };
    let f = fields(490);
    env.create_table("wide", &refs(&f)).await;
    let small = fields(10);
    env.create_table("small", &refs(&small)).await;
    let tag = env.classify_table("wide").await;
    let id = env.soft_drop("wide").await;
    // While `wide` is dropped, the tag also goes on `small`: 1 + 0 + 1 + 10.
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales','small','')",
    )
    .bind(tag)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign small");
    env.recompile().await;

    let err = env
        .undrop(id)
        .await
        .expect_err("1 + 490 + 1 + 10 is over 500");
    let message = err.to_string();
    assert!(message.contains("ceiling"), "{message}");
    let still_dropped: bool = sqlx::query_scalar(
        "SELECT deleted_at IS NOT NULL FROM lakekeeper.tabular WHERE tabular_id = $1",
    )
    .bind(id)
    .fetch_one(&env.db.pool)
    .await
    .expect("tabular");
    assert!(still_dropped, "nothing was undropped");
    // Not wedged: the profile still compiles.
    env.recompile().await;
}

// ── D2 review fix 2: a register is checked like a create ──

/// U5 for a register: its schema is in the metadata file it points to, so a
/// register that would push a whole-table classification over its ceiling is
/// answered 409 with the compile's refusal, the table is not left registered,
/// and the profile still compiles.
#[tokio::test]
async fn a_register_that_would_cross_the_ceiling_is_refused_and_does_not_wedge() {
    let Some(env) = env().await else { return };
    // A metadata file with 520 columns: a table created and dropped (not
    // purged) past the hook.
    let f = fields(520);
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({"name": "wide", "schema": schema(0, &refs(&f))})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create: {body}");
    let location = body["metadata-location"]
        .as_str()
        .expect("location")
        .to_string();
    let (status, body) = env
        .call_raw(
            "DELETE",
            "namespaces/sales/tables/wide?purgeRequested=false",
            None,
        )
        .await;
    assert!(status.is_success(), "drop: {status} {body}");
    // The classification is on `wide` before it exists (U4: deny only).
    let tag = env.classify_table("wide").await;

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/register",
            Some(json!({"name": "wide", "metadata-location": location})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().expect("message");
    assert!(message.contains("ceiling"), "{message}");
    assert_eq!(
        env.source("wide").await,
        None,
        "the table is not left registered"
    );
    // Not wedged: the profile compiles, and its last compile is not a failure.
    env.recompile().await;
    assert_eq!(
        env.last_compile().await.map(|(o, _)| o).as_deref(),
        Some("ok")
    );
    assert!(env.has_table_deny(tag, "wide").await);
}

// ── D2 review fix 3: the dry run locks every profile first ──

impl Env {
    /// A second StarRocks profile with an engine attached to the same catalog.
    async fn second_profile(&self) -> i64 {
        let policy = PolicyStore::new(self.db.clone());
        let profile = policy
            .create_service(&unique("sr_th2"), "starrocks", &json!({}))
            .await
            .expect("profile");
        let engine: Uuid = sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
             VALUES ($1,'starrocks','egret', \
                     '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb,$2) \
             RETURNING id",
        )
        .bind(unique("th-eng2"))
        .bind(profile.id)
        .fetch_one(&self.db.pool)
        .await
        .expect("engine");
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) \
             SELECT $1, id FROM registry.catalogs WHERE name = $2",
        )
        .bind(engine)
        .bind(&self.catalog)
        .execute(&self.db.pool)
        .await
        .expect("attach");
        profile.id
    }
}

/// Review finding 4: with two bound StarRocks profiles, the dry run takes
/// both profile locks before any `users` row (one `recompile_services_in_tx`),
/// and the change reaches both profiles.
#[tokio::test]
async fn a_column_add_reaches_every_bound_starrocks_profile() {
    let Some(env) = env().await else { return };
    let second = env.second_profile().await;
    env.create_table("orders", &[(1, "id")]).await;
    let tag = env.classify_table("orders").await;

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(schema_commit(schema(1, &[(1, "id"), (2, "phone")]))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for profile in [env.profile_id, second] {
        let masks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name LIKE $2",
        )
        .bind(profile)
        .bind(format!(
            "{}{tag}-mask-{}.sales.orders.%",
            egret_policy::COMPILER_CLEARANCE_PREFIX,
            env.catalog
        ))
        .fetch_one(&env.db.pool)
        .await
        .expect("masks");
        assert_eq!(masks, 2, "profile {profile}");
    }
}

// ── D2 review fix 5: the other table requests through the hook ──

/// A multi-table commit (`POST /transactions/commit`) that adds a column to a
/// table classified as a whole masks it before the response returns.
#[tokio::test]
async fn a_multi_table_commit_adding_a_column_masks_it() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id")]).await;
    env.create_table("events", &[(1, "id")]).await;
    let tag = env.classify_table("orders").await;
    let (status, body) = env
        .call(
            "POST",
            "transactions/commit",
            Some(json!({"table-changes": [
                {
                    "identifier": {"namespace": ["sales"], "name": "orders"},
                    "requirements": [],
                    "updates": [
                        {"action": "add-schema", "schema": schema(1, &[(1, "id"), (2, "phone")])},
                        {"action": "set-current-schema", "schema-id": -1}
                    ]
                },
                {
                    "identifier": {"namespace": ["sales"], "name": "events"},
                    "requirements": [],
                    "updates": [{"action": "set-properties", "updates": {"k": "v"}}]
                }
            ]})),
        )
        .await;
    assert!(status.is_success(), "commit: {status} {body}");
    assert_eq!(env.masked_columns(tag, "orders").await, vec!["id", "phone"]);
}

/// A staged create commits nothing until its committing update, which is the
/// change the hook judges and recompiles for.
#[tokio::test]
async fn a_staged_create_is_masked_at_its_committing_update() {
    let Some(env) = env().await else { return };
    let tag = env.classify_table("orders").await;
    let (status, staged) = env
        .call(
            "POST",
            "namespaces/sales/tables",
            Some(json!({
                "name": "orders",
                "schema": schema(0, &[(1, "id"), (2, "email")]),
                "stage-create": true,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "stage: {staged}");
    assert!(
        env.masked_columns(tag, "orders").await.is_empty(),
        "nothing committed yet"
    );
    let metadata = &staged["metadata"];
    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(json!({
                "requirements": [{"type": "assert-create"}],
                "updates": [
                    {"action": "assign-uuid", "uuid": metadata["table-uuid"]},
                    {"action": "upgrade-format-version", "format-version": 2},
                    {"action": "add-schema", "schema": schema(0, &[(1, "id"), (2, "email")]),
                     "last-column-id": 2},
                    {"action": "set-current-schema", "schema-id": -1},
                    {"action": "add-spec", "spec": {"spec-id": 0, "fields": []}},
                    {"action": "set-default-spec", "spec-id": -1},
                    {"action": "add-sort-order", "sort-order": {"order-id": 0, "fields": []}},
                    {"action": "set-default-sort-order", "sort-order-id": -1},
                    {"action": "set-location", "location": metadata["location"]}
                ]
            })),
        )
        .await;
    assert!(
        status.is_success(),
        "commit the staged create: {status} {body}"
    );
    assert_eq!(env.masked_columns(tag, "orders").await, vec!["email", "id"]);
}

/// A recursive namespace drop takes every table in it: their masks are
/// reaped, and (rename spec C13) so is the `*` deny, because the drop, which
/// cannot be undropped, removes the classification rows under it.
#[tokio::test]
async fn a_namespace_drop_reaps_the_masks_of_its_tables() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let tag = env.classify_table("orders").await;
    assert_eq!(env.masked_columns(tag, "orders").await.len(), 2);
    let (status, body) = env
        .call("DELETE", "namespaces/sales?recursive=true", None)
        .await;
    assert!(status.is_success(), "drop namespace: {status} {body}");
    assert!(env.masked_columns(tag, "orders").await.is_empty());
    assert!(!env.has_table_deny(tag, "orders").await);
}

// ── D2 review fix 6: concurrent schema changes are serialized per profile ──

/// Review finding 3: two column adds on two tables of one classification,
/// each within the ceiling alone and over it together. Without serialization
/// both dry runs pass against the stored schema, both commit, and the profile
/// is wedged. The hook holds a per-profile lock from the dry run to the
/// recompile, so the second waits, dry-runs against the first's columns, and
/// is refused.
#[tokio::test]
async fn concurrent_column_adds_that_cross_the_ceiling_together_do_not_wedge() {
    let Some(env) = env().await else { return };
    let env = Arc::new(env);
    // Tag on `a` and `b`, 1 + 240 each: 482. Each add of 10 alone: 492.
    let a = fields(240);
    env.create_table("a", &refs(&a)).await;
    env.create_table("b", &refs(&a)).await;
    let tag = env.classify_table("a").await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales','b','')",
    )
    .bind(tag)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign b");
    env.recompile().await;
    let grown = fields(250);

    // A reaches lakekeeper (after its dry run) and is held there while B runs.
    let reached = Arc::new(tokio::sync::Notify::new());
    *env.stall.lock().unwrap() = Some((
        format!("POST /{}/namespaces/sales/tables/a", env.prefix),
        reached.clone(),
    ));
    let first = {
        let env = env.clone();
        let body = schema_commit(schema(1, &refs(&grown)));
        tokio::spawn(async move {
            env.call("POST", "namespaces/sales/tables/a", Some(body))
                .await
        })
    };
    reached.notified().await;
    let (status_b, body_b) = env
        .call(
            "POST",
            "namespaces/sales/tables/b",
            Some(schema_commit(schema(1, &refs(&grown)))),
        )
        .await;
    let (status_a, body_a) = first.await.expect("A");

    assert_eq!(status_a, StatusCode::OK, "A: {body_a}");
    assert_eq!(status_b, StatusCode::CONFLICT, "B: {body_b}");
    assert_eq!(env.loaded("b").await.len(), 240, "B did not commit");
    assert_eq!(
        env.last_compile().await.map(|(o, _)| o).as_deref(),
        Some("ok"),
        "the profile is not wedged"
    );
}

// ── Classifications follow a rename, Task 3: the move (R1, spec §5.1-§5.3) ──

/// A classification row: `(tag, schema, table, column)`.
type Row = (Uuid, String, String, String);

impl Env {
    async fn rename(&self, from: &str, to: &str) -> (StatusCode, Value) {
        self.call(
            "POST",
            "tables/rename",
            Some(json!({
                "source": {"namespace": ["sales"], "name": from},
                "destination": {"namespace": ["sales"], "name": to},
            })),
        )
        .await
    }

    /// A classification with a role-bearing clearance on `sales.{table}.{column}`
    /// (not recompiled: the caller does).
    async fn classify_column(&self, table: &str, column: &str) -> Uuid {
        let tag = self.new_gated_tag().await;
        self.assign(tag, table, column).await;
        tag
    }

    async fn new_gated_tag(&self) -> Uuid {
        let tag: Uuid =
            sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
                .bind(unique("th-tag"))
                .fetch_one(&self.db.pool)
                .await
                .expect("tag");
        sqlx::query(
            "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
             VALUES ($1, $2, '{}')",
        )
        .bind(tag)
        .bind(vec![unique("th-role")])
        .execute(&self.db.pool)
        .await
        .expect("gate");
        tag
    }

    async fn assign(&self, tag: Uuid, table: &str, column: &str) {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
             column_name) VALUES ($1,$2,'sales',$3,$4)",
        )
        .bind(tag)
        .bind(self.warehouse_id)
        .bind(table)
        .bind(column)
        .execute(&self.db.pool)
        .await
        .expect("assign");
    }

    /// Every classification row of the warehouse, sorted.
    async fn rows(&self) -> Vec<Row> {
        sqlx::query_as(
            "SELECT tag_id, schema_name, table_name, column_name FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 ORDER BY 1, 2, 3, 4",
        )
        .bind(self.warehouse_id)
        .fetch_all(&self.db.pool)
        .await
        .expect("rows")
    }

    async fn versions(&self) -> Vec<(i64, i64, i64)> {
        sqlx::query_as(
            "SELECT id, tag_version, policy_version FROM policy.authz_services \
              WHERE id = ANY($1) ORDER BY id",
        )
        .bind(vec![self.trino_id, self.profile_id])
        .fetch_all(&self.db.pool)
        .await
        .expect("versions")
    }

    async fn tag_version(&self) -> i64 {
        sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(self.trino_id)
            .fetch_one(&self.db.pool)
            .await
            .expect("tag version")
    }

    fn ranger(&self) -> axum::Router {
        egret_policy::ranger_router(egret_policy::RangerState {
            store: self.policy.clone(),
            auth_token: Arc::from(plugins::RANGER_TOKEN),
            mode: egret_policy::ranger::AuthMode::Required,
            verifier: None,
        })
    }

    async fn ranger_get(&self, uri: String) -> Value {
        let req = axum::http::Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {}", plugins::RANGER_TOKEN))
            .body(Body::empty())
            .unwrap();
        let resp = self.ranger().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).expect("download")
    }

    /// `(table, column)` of every resource the Trino plugin's tag download
    /// serves (a table-level row has no column: `""`).
    async fn served_trino_resources(&self) -> Vec<(String, String)> {
        let tags = self
            .ranger_get(format!(
                "/tags/download/{}?lastKnownVersion=-1",
                self.trino_name
            ))
            .await;
        let first = |r: &Value, level: &str| {
            r["resourceElements"][level]["values"][0]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        let mut out: Vec<(String, String)> = tags["serviceResources"]
            .as_array()
            .expect("serviceResources")
            .iter()
            .map(|r| (first(r, "table"), first(r, "column")))
            .collect();
        out.sort();
        out
    }

    /// The names of every policy the StarRocks plugin's download serves.
    async fn served_starrocks_policies(&self) -> Vec<String> {
        let download = self
            .ranger_get(format!(
                "/plugins/policies/download/{}?lastKnownVersion=-1",
                self.profile_name
            ))
            .await;
        download["policies"]
            .as_array()
            .expect("policies")
            .iter()
            .map(|p| p["name"].as_str().expect("name").to_string())
            .collect()
    }

    fn start_trino_plugin(&self) -> plugins::Plugin {
        plugins::start_trino(self.policy.clone(), self.trino_name.clone(), POLL)
    }

    /// The fake Trino and StarRocks plugins, each polled once.
    async fn start_plugins(&self) -> (plugins::Plugin, plugins::Plugin) {
        let trino = self.start_trino_plugin();
        let starrocks =
            plugins::start_starrocks(self.policy.clone(), self.profile_name.clone(), POLL);
        trino.polled().await;
        starrocks.polled().await;
        (trino, starrocks)
    }

    /// Arm [`observe`] to record what the catalog held when `what` reached
    /// lakekeeper.
    fn probe_reach(&self, what: &str) {
        *self.rows_at_reach.lock().unwrap() = Some((
            format!("{} /{}/{}", "POST", self.prefix, what),
            self.warehouse_id,
            (self.trino_id, self.profile_id),
            None,
        ));
    }

    fn reached(&self) -> AtReach {
        self.rows_at_reach
            .lock()
            .unwrap()
            .take()
            .and_then(|(_, _, _, seen)| seen)
            .expect("the observer saw the request reach lakekeeper")
    }

    /// `orders(id, email)` with tag A on the table and tag B on `email`, both
    /// compiled into every bound profile.
    async fn classified_orders(&self) -> (Uuid, Uuid) {
        self.create_table("orders", &[(1, "id"), (2, "email")])
            .await;
        let a = self.classify_table("orders").await;
        let b = self.classify_column("orders", "email").await;
        self.recompile_all().await;
        (a, b)
    }

    async fn recompile_all(&self) {
        let mut tx = self.db.pool.begin().await.expect("begin");
        let mut ids = vec![self.profile_id, self.trino_id];
        ids.sort_unstable();
        egret_perm_compile::recompile_services_in_tx(&mut tx, &ids)
            .await
            .expect("recompile");
        tx.commit().await.expect("commit");
    }
}

fn row(tag: Uuid, table: &str, column: &str) -> Row {
    (tag, "sales".into(), table.into(), column.into())
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort();
    rows
}

/// R1: a renamed table keeps its classifications, on both engines, and the old
/// name loses them.
#[tokio::test]
async fn a_renamed_table_keeps_its_classifications() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders_v2", ""), row(b, "orders_v2", "email")])
    );
    let served = env.served_trino_resources().await;
    assert!(
        served.contains(&("orders_v2".into(), String::new()))
            && served.contains(&("orders_v2".into(), "email".into())),
        "{served:?}"
    );
    assert!(served.iter().all(|(t, _)| t != "orders"), "{served:?}");
    let policies = env.served_starrocks_policies().await;
    let clearance = egret_policy::COMPILER_CLEARANCE_PREFIX;
    let c = &env.catalog;
    // One mask per column, named after the lower of its tags (D2 C5).
    for name in [
        format!("{clearance}{a}-{c}.sales.orders_v2.*"),
        format!("{clearance}{}-mask-{c}.sales.orders_v2.email", a.min(b)),
        format!("{clearance}{a}-mask-{c}.sales.orders_v2.id"),
    ] {
        assert!(policies.contains(&name), "{name} not in {policies:?}");
    }
    assert!(
        policies.iter().all(|p| !p.contains(".sales.orders.")),
        "{policies:?}"
    );
}

/// R1: the rename reaches lakekeeper only once the rows are on both names
/// and the Trino plugin has reported the tag version the copy left.
#[tokio::test]
async fn the_rename_reaches_lakekeeper_only_after_trino_serves_the_moved_tags() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let (plugin, _starrocks) = env.start_plugins().await;
    env.probe_reach("tables/rename");

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let reached = env.reached();
    assert_eq!(reached.tables, vec!["orders", "orders_v2"]);
    let reported = plugin.report_of(reached.tag_version).await;
    assert!(
        reported < reached.at,
        "the plugin reported the copy's version {} only after the rename reached lakekeeper",
        reached.tag_version
    );
}

/// R1: a move the overlap check refuses answers 409 and renames nothing. A
/// tag-addressed mask on tag B and a path mask on `sales.orders_v2.email`
/// that decides differently are a pair the move would complete.
#[tokio::test]
async fn a_rename_whose_move_is_refused_answers_409_and_does_not_rename() {
    let Some(env) = env().await else { return };
    let (_, b) = env.classified_orders().await;
    let b_key: String = sqlx::query_scalar("SELECT key FROM tags WHERE id = $1")
        .bind(b)
        .fetch_one(&env.db.pool)
        .await
        .expect("key");
    let catalog_id: Uuid = sqlx::query_scalar("SELECT id FROM registry.catalogs WHERE name = $1")
        .bind(&env.catalog)
        .fetch_one(&env.db.pool)
        .await
        .expect("catalog id");
    let perm = egret_perm::PermStore::new(env.db.pool.clone());
    let mask = |name: String, mask_type: &str, selector: egret_perm::Selector| egret_perm::Rule {
        id: Uuid::nil(),
        name,
        kind: egret_perm::RuleKind::Mask {
            mask_type: mask_type.into(),
            value_expr: None,
        },
        selector,
        exempt: egret_perm::PrincipalSet::default(),
    };
    perm.create_rule(&mask(
        unique("th-tag-mask"),
        "MASK",
        egret_perm::Selector::Tag { key: b_key },
    ))
    .await
    .expect("tag rule");
    perm.create_rule(&mask(
        unique("th-path-mask"),
        "MASK_NULL",
        egret_perm::Selector::Path(egret_perm::Scope {
            catalog_id,
            schema: Some("sales".into()),
            table: Some("orders_v2".into()),
            column: Some("email".into()),
        }),
    ))
    .await
    .expect("path rule");
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;
    let (rows, versions) = (env.rows().await, env.versions().await);

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().expect("message");
    assert!(
        message.starts_with(
            "egret refused the table change: the table's classifications cannot move to"
        ),
        "{message}"
    );
    // Live check F4: the compile's own advice is not followed by a second
    // "resolve it".
    assert!(message.ends_with(". Nothing was renamed."), "{message}");
    assert!(!message.contains("resolve it, then retry"), "{message}");
    assert!(env.source("orders").await.is_some(), "not renamed");
    assert!(env.source("orders_v2").await.is_none());
    assert_eq!(env.rows().await, rows);
    assert_eq!(env.versions().await, versions);
}

/// R1: a rename lakekeeper refuses (the destination exists) undoes the copy:
/// its answer is returned as it is, no row is left on the new name, and the
/// tag version moved twice (copy, compensation).
#[tokio::test]
async fn a_failed_rename_undoes_the_copy() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    env.create_table("orders_v2", &[(1, "id")]).await;
    let _plugins = env.start_plugins().await;
    let before = env.tag_version().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    // Lakekeeper answers a rename onto an existing table 404 ("Error getting
    // tabular from catalog"), not 409.
    assert!(!status.is_success(), "{status} {body}");
    assert!(
        !body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .starts_with("egret refused"),
        "lakekeeper's own answer: {body}"
    );
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "email")])
    );
    assert_eq!(env.tag_version().await, before + 2);
}

/// R4: only classification rows move; a path deny grant on `sales.orders`
/// stays where it is (spec §11 item 1, recorded, not fixed).
#[tokio::test]
async fn a_rename_moves_classifications_only() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let catalog_id: Uuid = sqlx::query_scalar("SELECT id FROM registry.catalogs WHERE name = $1")
        .bind(&env.catalog)
        .fetch_one(&env.db.pool)
        .await
        .expect("catalog id");
    let grant: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name) \
         VALUES ('deny', ARRAY['read'], $1, 'sales', 'orders') RETURNING id",
    )
    .bind(catalog_id)
    .fetch_one(&env.db.pool)
    .await
    .expect("deny grant");
    let _plugins = env.start_plugins().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let table: String = sqlx::query_scalar("SELECT table_name FROM perm.grants WHERE id = $1")
        .bind(grant)
        .fetch_one(&env.db.pool)
        .await
        .expect("grant");
    assert_eq!(table, "orders");
}

// ── Task 4: wait for StarRocks too; no Trino, silent engines (C4, C5, R5) ──

impl Env {
    fn start_starrocks_plugin(&self, poll: Duration) -> plugins::Plugin {
        plugins::start_starrocks(self.policy.clone(), self.profile_name.clone(), poll)
    }

    /// Unbind an engine from the catalog.
    async fn detach(&self, engine: Uuid) {
        sqlx::query("DELETE FROM registry.engine_catalog_attachments WHERE engine_id = $1")
            .bind(engine)
            .execute(&self.db.pool)
            .await
            .expect("detach");
    }
}

/// C5: the rename reaches lakekeeper only once the StarRocks plugin has
/// reported the policy version the copy left, and its download then masks
/// the new name.
#[tokio::test]
async fn the_rename_waits_for_the_starrocks_policy_version() {
    let Some(env) = env().await else { return };
    let (a, _) = env.classified_orders().await;
    let trino = env.start_trino_plugin();
    // Five times slower than Trino's: a rename that waited for Trino alone
    // reaches lakekeeper before StarRocks reports.
    let starrocks = env.start_starrocks_plugin(POLL * 5);
    trino.polled().await;
    starrocks.polled().await;
    env.probe_reach("tables/rename");

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let reached = env.reached();
    let reported = starrocks.report_of(reached.policy_version).await;
    assert!(
        reported < reached.at,
        "StarRocks reported the copy's policy version {} only after the rename reached \
         lakekeeper",
        reached.policy_version
    );
    let policies = env.served_starrocks_policies().await;
    let mask = format!(
        "{}{a}-mask-{}.sales.orders_v2.id",
        egret_policy::COMPILER_CLEARANCE_PREFIX,
        env.catalog
    );
    assert!(policies.contains(&mask), "{policies:?}");
}

/// C4: with no Trino engine bound, a classified rename is not refused and
/// waits on StarRocks alone.
#[tokio::test]
async fn a_rename_with_no_trino_bound_moves_without_a_trino_wait() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    env.detach(env.trino_engine).await;
    let starrocks = env.start_starrocks_plugin(POLL);
    starrocks.polled().await;
    env.probe_reach("tables/rename");

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders_v2", ""), row(b, "orders_v2", "email")])
    );
    let reached = env.reached();
    assert!(starrocks.report_of(reached.policy_version).await < reached.at);
}

/// R5: a bound engine whose plugins never confirm refuses the rename at the
/// deadline and undoes the copy.
#[tokio::test]
async fn a_rename_no_plugin_confirms_is_refused_and_undone() {
    let Some(env) = env_with_limit(Duration::from_secs(2)).await else {
        return;
    };
    let (a, b) = env.classified_orders().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().expect("message");
    assert!(
        message.starts_with(
            "egret refused the table change: engines did not confirm the moved \
             classifications within 2 s: "
        ),
        "{message}"
    );
    assert!(
        message.contains(egret_policy::NO_PLUGIN_POLLED),
        "{message}"
    );
    // Live check F9: every refusal of a held rename ends the same way.
    assert!(message.ends_with(". Nothing was renamed."), "{message}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "email")])
    );
    assert!(env.source("orders").await.is_some(), "not renamed");
}

/// C4, C5: with no engine bound at all, nothing can be waited for and the
/// move goes through at once.
#[tokio::test]
async fn a_rename_with_nothing_bound_moves_at_once() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    env.detach(env.trino_engine).await;
    env.detach(env.starrocks_engine).await;

    let started = Instant::now();
    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    // No wait: well inside the deadline a wait on a silent engine would
    // reach (the copy and the after-commit step alone take over a second
    // while the rest of the suite runs).
    assert!(started.elapsed() < LIMIT / 2, "{:?}", started.elapsed());
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders_v2", ""), row(b, "orders_v2", "email")])
    );
}

// ── Task 5: the rename defaults (C1-C3, C9, C10, C17, #755 interplay) ──

impl Env {
    async fn rename_across(
        &self,
        from: (&[&str], &str),
        to: (&[&str], &str),
    ) -> (StatusCode, Value) {
        self.call(
            "POST",
            "tables/rename",
            Some(json!({
                "source": {"namespace": from.0, "name": from.1},
                "destination": {"namespace": to.0, "name": to.1},
            })),
        )
        .await
    }

    /// Hold the view hold's name lock (C17) on `sales.{table}` from a
    /// connection of the test's own, as another hold would.
    async fn hold_name(&self, table: &str) -> sqlx::PgConnection {
        use sqlx::Connection as _;
        let mut conn = sqlx::PgConnection::connect(&egret_test_db::url().await.expect("url"))
            .await
            .expect("connect");
        let key = egret_catalog_engine::view_lock_key(&self.prefix, "sales", table);
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
            .bind(key)
            .fetch_one(&mut conn)
            .await
            .expect("lock");
        assert!(got, "the test's lock");
        conn
    }
}

/// C1: renaming onto a name that already carries rows unions them; a failed
/// rename's compensation leaves the rows that were there.
#[tokio::test]
async fn a_rename_onto_a_name_with_rows_unions_them() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let c = env.new_gated_tag().await;
    env.assign(c, "orders_v2", "").await;
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;

    // The failing variant first: a live `orders_v2` makes lakekeeper refuse.
    env.create_table("orders_v2", &[(1, "id")]).await;
    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(!status.is_success(), "{status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![
            row(a, "orders", ""),
            row(b, "orders", "email"),
            row(c, "orders_v2", ""),
        ])
    );
    let (status, body) = env
        .call_raw("DELETE", "namespaces/sales/tables/orders_v2", None)
        .await;
    assert!(status.is_success(), "drop: {status} {body}");

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![
            row(a, "orders_v2", ""),
            row(b, "orders_v2", "email"),
            row(c, "orders_v2", ""),
        ])
    );
}

/// C2: no row can be stored under a multi-level name, so a classified table
/// may not move into one.
#[tokio::test]
async fn a_classified_rename_into_a_multi_level_namespace_is_refused() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let rows = env.rows().await;
    let (status, body) = env
        .rename_across((&["sales"], "orders"), (&["sales", "eu"], "orders"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().expect("message");
    assert!(message.contains("more than one level"), "{message}");
    assert_eq!(env.rows().await, rows, "nothing written");
    assert!(env.source("orders").await.is_some(), "not renamed");
}

/// C3: a rename into another single-level namespace moves the rows.
#[tokio::test]
async fn a_cross_namespace_rename_moves_the_rows() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces",
            Some(json!({"namespace": ["archive"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "archive: {body}");
    let _plugins = env.start_plugins().await;

    let (status, body) = env
        .rename_across((&["sales"], "orders"), (&["archive"], "orders"))
        .await;
    assert!(status.is_success(), "rename: {status} {body}");
    let archived = |tag: Uuid, column: &str| -> Row {
        (tag, "archive".into(), "orders".into(), column.into())
    };
    assert_eq!(
        env.rows().await,
        sorted(vec![archived(a, ""), archived(b, "email")])
    );
}

/// C18: source rows are matched folded; the copies take the request's
/// destination spelling.
#[tokio::test]
async fn a_rename_into_a_name_spelled_differently_writes_the_destination_spelling() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let a = env.new_gated_tag().await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'Sales','Orders','Email')",
    )
    .bind(a)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign");
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(env.rows().await, vec![row(a, "orders_v2", "Email")]);
}

/// C10: a case-only rename changes no name any engine matches: no lock, no
/// row written, no tag version bumped.
#[tokio::test]
async fn a_case_only_rename_is_not_held() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let _plugins = env.start_plugins().await;
    let (rows, tag_version) = (env.rows().await, env.tag_version().await);
    let _held = env.hold_name("orders").await;

    let (status, body) = env.rename("orders", "Orders").await;
    let message = body["error"]["message"].as_str().unwrap_or("");
    assert!(!message.starts_with("egret refused"), "{status} {body}");
    assert_eq!(env.rows().await, rows);
    // No move bumped the tag version. #755's own after-commit recompile still
    // runs (the StarRocks policies are spelled as the catalog spells the
    // table), so the StarRocks policy version may move.
    assert_eq!(env.tag_version().await, tag_version);
}

/// R1: a source that carries no row is not held: nothing copied, no version
/// bumped.
#[tokio::test]
async fn an_unclassified_rename_is_not_held() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id")]).await;
    let versions = env.versions().await;
    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert!(env.rows().await.is_empty());
    assert_eq!(env.versions().await, versions);
}

/// C9: a view sent to the TABLE rename endpoint is not moved here (its rows
/// move with the view hold); lakekeeper answers it.
#[tokio::test]
async fn a_view_sent_to_the_table_rename_endpoint_is_not_moved() {
    let Some(env) = env().await else { return };
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/views",
            Some(json!({
                "name": "v",
                "schema": schema(0, &[(1, "id")]),
                "view-version": {
                    "version-id": 1, "schema-id": 0,
                    "timestamp-ms": 1_700_000_000_000_i64,
                    "summary": {"engine-name": "trino"},
                    "default-namespace": ["sales"],
                    "representations": [{"type": "sql", "sql": "SELECT 1 AS id", "dialect": "trino"}]
                },
                "properties": {}
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create view: {body}");
    let a = env.classify_column("v", "id").await;
    let versions = env.versions().await;

    let (status, body) = env.rename("v", "v2").await;
    let message = body["error"]["message"].as_str().unwrap_or("");
    assert!(!message.starts_with("egret refused"), "{status} {body}");
    assert_eq!(env.rows().await, vec![row(a, "v", "id")]);
    assert_eq!(env.versions().await, versions);
}

/// C17: a name held by another change refuses the rename at once.
#[tokio::test]
async fn a_held_name_refuses_a_second_rename_at_once() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let _held = env.hold_name("orders_v2").await;
    let started = Instant::now();
    let (status, body) = env.rename("orders", "orders_v2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("being held"),
        "{body}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(env.source("orders").await.is_some(), "not renamed");
}

/// C7: a column add on another whole-table-classified table of the warehouse,
/// sent while a rename waits, waits for the rename's profile locks: it reaches
/// lakekeeper only after the rename's after-commit step removed the old rows.
#[tokio::test]
async fn a_held_rename_and_a_column_add_on_the_same_profile_run_one_after_the_other() {
    let Some(env) = env().await else { return };
    let env = Arc::new(env);
    env.classified_orders().await;
    env.create_table("events", &[(1, "id")]).await;
    env.classify_table("events").await;
    let trino = env.start_trino_plugin();
    // Slow, so the rename waits long enough for the column add to arrive.
    let starrocks = env.start_starrocks_plugin(POLL * 5);
    trino.polled().await;
    starrocks.polled().await;

    let rename = {
        let env = env.clone();
        tokio::spawn(async move { env.rename("orders", "orders_v2").await })
    };
    // The copy has committed once the new name carries rows.
    for _ in 0..200 {
        if env.rows().await.iter().any(|r| r.2 == "orders_v2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!rename.is_finished(), "the rename is still held");
    env.probe_reach("namespaces/sales/tables/events");
    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/events",
            Some(schema_commit(schema(1, &[(1, "id"), (2, "kind")]))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "column add: {body}");
    let (status, body) = rename.await.expect("rename");
    assert!(status.is_success(), "rename: {status} {body}");
    let reached = env.reached();
    assert_eq!(
        reached.tables,
        vec!["events", "orders_v2"],
        "the column add reached lakekeeper after the rename finished"
    );
}

// ── Task 6: a column rename follows by field id (R3, C12) ──

impl Env {
    /// Trino's `ALTER TABLE … RENAME COLUMN`: a new schema with the same field
    /// ids, made current.
    async fn commit_schema(
        &self,
        table: &str,
        id: i64,
        fields: &[(i64, &str)],
    ) -> (StatusCode, Value) {
        self.call(
            "POST",
            &format!("namespaces/sales/tables/{table}"),
            Some(schema_commit(schema(id, fields))),
        )
        .await
    }

    /// The policies the StarRocks profile held when the armed request reached
    /// lakekeeper ([`OnReach`]).
    fn arm_policies_at_reach(&self, what: &str) {
        *self.on_reach.lock().unwrap() = Some((
            format!("POST /{}/{what}", self.prefix),
            self.profile_id,
            None,
        ));
    }

    fn policies_at_reach(&self) -> Vec<String> {
        let (policies, _) = self
            .on_reach
            .lock()
            .unwrap()
            .take()
            .and_then(|(_, _, seen)| seen)
            .expect("the observer saw the request reach lakekeeper");
        policies
            .as_array()
            .expect("policies")
            .iter()
            .map(|p| p[0].as_str().expect("name").to_string())
            .collect()
    }
}

/// R3: a renamed column keeps its classification, on Trino before the commit.
#[tokio::test]
async fn a_renamed_column_keeps_its_classification() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let (trino, _starrocks) = env.start_plugins().await;
    env.probe_reach("namespaces/sales/tables/orders");

    let (status, body) = env
        .commit_schema("orders", 1, &[(1, "id"), (2, "mail")])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "mail")])
    );
    let served = env.served_trino_resources().await;
    assert!(
        served.contains(&("orders".into(), "mail".into())),
        "{served:?}"
    );
    assert!(
        !served.contains(&("orders".into(), "email".into())),
        "{served:?}"
    );
    let reached = env.reached();
    assert!(trino.report_of(reached.tag_version).await < reached.at);
}

/// C12: a whole-table classification names its StarRocks masks per column,
/// so a column rename is held until StarRocks serves the new name's mask;
/// while held both names are masked, after it only the new one.
#[tokio::test]
async fn a_column_rename_on_a_whole_table_classification_waits_for_starrocks() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let a = env.classify_table("orders").await;
    env.recompile_all().await;
    let trino = env.start_trino_plugin();
    let starrocks = env.start_starrocks_plugin(POLL * 5);
    trino.polled().await;
    starrocks.polled().await;
    env.probe_reach("namespaces/sales/tables/orders");
    env.arm_policies_at_reach("namespaces/sales/tables/orders");

    let (status, body) = env
        .commit_schema("orders", 1, &[(1, "id"), (2, "mail")])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reached = env.reached();
    assert!(starrocks.report_of(reached.policy_version).await < reached.at);
    let prefix = format!(
        "{}{a}-mask-{}.sales.orders.",
        egret_policy::COMPILER_CLEARANCE_PREFIX,
        env.catalog
    );
    let at_reach = env.policies_at_reach();
    for column in ["mail", "email"] {
        assert!(
            at_reach.contains(&format!("{prefix}{column}")),
            "{column} at reach: {at_reach:?}"
        );
    }
    assert_eq!(env.masked_columns(a, "orders").await, vec!["id", "mail"]);
}

/// R3: two columns swapping names in one commit swap their classifications.
#[tokio::test]
async fn swapped_column_names_swap_their_classifications() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "a"), (3, "b")])
        .await;
    let b_tag = env.classify_column("orders", "a").await;
    let c_tag = env.classify_column("orders", "b").await;
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env
        .commit_schema("orders", 1, &[(1, "id"), (2, "b"), (3, "a")])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(b_tag, "orders", "b"), row(c_tag, "orders", "a")])
    );
}

/// R3: a multi-table commit moves each table's renamed columns.
#[tokio::test]
async fn a_multi_table_commit_moves_each_tables_renamed_columns() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    env.create_table("events", &[(1, "id"), (2, "kind")]).await;
    let b = env.classify_column("orders", "email").await;
    let k = env.classify_column("events", "kind").await;
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;

    let change = |table: &str, fields: &[(i64, &str)]| {
        json!({
            "identifier": {"namespace": ["sales"], "name": table},
            "requirements": [],
            "updates": [
                {"action": "add-schema", "schema": schema(1, fields)},
                {"action": "set-current-schema", "schema-id": -1}
            ]
        })
    };
    let (status, body) = env
        .call(
            "POST",
            "transactions/commit",
            Some(json!({"table-changes": [
                change("orders", &[(1, "id"), (2, "mail")]),
                change("events", &[(1, "id"), (2, "type")]),
            ]})),
        )
        .await;
    assert!(status.is_success(), "commit: {status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(b, "orders", "mail"), row(k, "events", "type")])
    );
}

/// R3: a dropped column's rows stay dormant on its name; nothing is held.
#[tokio::test]
async fn a_dropped_column_keeps_its_rows_dormant() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let _plugins = env.start_plugins().await;
    let tag_version = env.tag_version().await;
    let _held = env.hold_name("orders").await;

    let (status, body) = env.commit_schema("orders", 1, &[(1, "id")]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "email")])
    );
    assert_eq!(env.tag_version().await, tag_version);
}

/// C12: a commit that changes no schema takes no lock and writes nothing
/// (#755's `a_commit_on_an_unclassified_table_runs_no_compile`, extended).
#[tokio::test]
async fn an_append_is_not_held() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let (rows, versions) = (env.rows().await, env.versions().await);
    let _held = env.hold_name("orders").await;

    let (status, body) = env
        .call(
            "POST",
            "namespaces/sales/tables/orders",
            Some(json!({"requirements": [], "updates": [
                {"action": "set-properties", "updates": {"owner": "x"}}
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(env.rows().await, rows);
    assert_eq!(env.versions().await, versions);
}

/// C12: a column add that renames nothing takes #755's path: no copy, no
/// name lock, and the new column is masked after the commit.
#[tokio::test]
async fn a_column_add_without_a_rename_takes_the_755_path() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let a = env.classify_table("orders").await;
    let (rows, tag_version) = (env.rows().await, env.tag_version().await);
    let _held = env.hold_name("orders").await;

    let (status, body) = env
        .commit_schema("orders", 1, &[(1, "id"), (2, "email"), (3, "phone")])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(env.rows().await, rows);
    assert_eq!(env.tag_version().await, tag_version);
    assert_eq!(
        env.masked_columns(a, "orders").await,
        vec!["email", "id", "phone"]
    );
}

// ── Task 7: governance events by the real caller (C6, C6a, R7) ──

impl Env {
    /// A catalog client of the warehouse named `name`, returning
    /// `(id, client_id)`.
    async fn catalog_client(&self, name: &str) -> (Uuid, String) {
        let pool = &self.db.pool;
        let workspace: Uuid =
            sqlx::query_scalar("SELECT workspace_id FROM catalog.warehouses WHERE id = $1")
                .bind(self.warehouse_id)
                .fetch_one(pool)
                .await
                .expect("workspace");
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
             VALUES ($1, $2, 'engine') RETURNING id",
        )
        .bind(self.warehouse_id)
        .bind(unique("th-role"))
        .fetch_one(pool)
        .await
        .expect("catalog role");
        let client_id = unique(name);
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.catalog_clients (name, consumer_type, workspace_id, \
             warehouse_id, catalog_role_id, client_id, client_secret_hash) \
             VALUES ($1, 'engine', $2, $3, $4, $5, 'x') RETURNING id",
        )
        .bind(name)
        .bind(workspace)
        .bind(self.warehouse_id)
        .bind(role)
        .bind(&client_id)
        .fetch_one(pool)
        .await
        .expect("catalog client");
        (id, client_id)
    }

    /// `(actor_user_id, detail)` of every `tag:move` event on `sales.{table}`.
    async fn move_events(&self, table: &str) -> Vec<(Option<Uuid>, Value)> {
        sqlx::query_as(
            "SELECT actor_user_id, detail FROM audit.audit_log \
              WHERE action = 'tag:move' AND resource_type = 'catalog' AND resource_id = $1 \
              ORDER BY id",
        )
        .bind(format!("{}:sales.{table}", self.warehouse_id))
        .fetch_all(&self.db.pool)
        .await
        .expect("events")
    }
}

/// C6, R7: a completed move records one `tag:move` event, by the catalog
/// client whose token sent the rename; `kept` lists what the new name carried
/// already (C1).
#[tokio::test]
async fn a_completed_move_records_one_tag_move_event_by_its_caller() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let c = env.new_gated_tag().await;
    env.assign(c, "orders_v2", "").await;
    env.recompile_all().await;
    let (client, client_id) = env.catalog_client("trino-main").await;
    *env.caller.lock().unwrap() = client_id.clone();
    let _plugins = env.start_plugins().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let events = env.move_events("orders").await;
    assert_eq!(events.len(), 1, "{events:?}");
    let (actor, detail) = &events[0];
    assert_eq!(*actor, None);
    assert_eq!(
        detail["actor"],
        json!({"kind": "catalog_client", "id": client, "client_id": client_id,
               "name": "trino-main", "consumer_type": "engine"})
    );
    assert_eq!(
        detail["before"],
        json!({"schema": "sales", "table": "orders", "column": null})
    );
    assert_eq!(
        detail["after"],
        json!({"schema": "sales", "table": "orders_v2", "column": null})
    );
    assert_eq!(detail["rows"], json!(2));
    assert_eq!(detail["trigger"], json!("iceberg_rename"));
    let mut tags: Vec<String> = detail["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    tags.sort();
    let mut want = vec![a.to_string(), b.to_string()];
    want.sort();
    assert_eq!(tags, want);
    let kept = detail["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0]["tag"]["id"], json!(c));
}

/// C6a: a client deleted since keeps its `client_id`; its id and name are
/// unknown.
#[tokio::test]
async fn a_move_by_a_deleted_client_keeps_its_client_id() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let (client, client_id) = env.catalog_client("trino-main").await;
    sqlx::query("DELETE FROM catalog.catalog_clients WHERE id = $1")
        .bind(client)
        .execute(&env.db.pool)
        .await
        .expect("delete client");
    *env.caller.lock().unwrap() = client_id.clone();
    let _plugins = env.start_plugins().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let events = env.move_events("orders").await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].1["actor"],
        json!({"kind": "catalog_client", "id": null, "client_id": client_id,
               "name": null, "consumer_type": null})
    );
}

/// C6a: with no guard (no caller known) the move is egret's: no actor.
#[tokio::test]
async fn a_move_with_no_guard_is_by_egret() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let _plugins = env.start_plugins().await;

    let (status, body) = send(
        &env.unguarded,
        &env.prefix,
        "POST",
        "tables/rename",
        Some(json!({
            "source": {"namespace": ["sales"], "name": "orders"},
            "destination": {"namespace": ["sales"], "name": "orders_v2"},
        })),
    )
    .await;
    assert!(status.is_success(), "rename: {status} {body}");
    let events = env.move_events("orders").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, None);
    assert!(events[0].1.get("actor").is_none(), "{:?}", events[0].1);
}

/// C6: a refused or undone move records nothing.
#[tokio::test]
async fn a_refused_or_undone_move_records_nothing() {
    let Some(env) = env().await else { return };
    env.classified_orders().await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env
        .rename_across((&["sales"], "orders"), (&["sales", "eu"], "orders"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    env.create_table("orders_v2", &[(1, "id")]).await;
    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(!status.is_success(), "{status} {body}");
    assert!(env.move_events("orders").await.is_empty());
}

// ── Task 8: drops (R2, R9, C13) ──

impl Env {
    /// Record `columns` and a computed lineage reading `bases` for the view
    /// `{schema}.{name}` of the env's catalog, with the view's derived rows.
    async fn lineage_view(
        &self,
        schema: &str,
        name: &str,
        bases: &[(&str, &str)],
        columns: &[&str],
    ) -> egret_perm_compile::lineage::ViewRef {
        let view = egret_perm_compile::lineage::ViewRef {
            catalog: self.catalog.clone(),
            schema: schema.into(),
            name: name.into(),
        };
        let tables: std::collections::BTreeSet<egret_perm_compile::lineage::LineageTable> = bases
            .iter()
            .map(|(s, t)| (self.catalog.clone(), s.to_string(), t.to_string()))
            .collect();
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        let mut tx = self.db.pool.begin().await.expect("begin");
        egret_perm_compile::lineage::write_lineage(&mut tx, &view, &tables)
            .await
            .expect("lineage");
        egret_perm_compile::lineage::set_view_columns(&mut tx, &view, Some(&columns))
            .await
            .expect("columns");
        egret_perm_compile::retag_view_in_tx(&mut tx, &view)
            .await
            .expect("derive");
        tx.commit().await.expect("commit");
        view
    }

    /// Every classification row of the warehouse in any spelling of `schema`.
    async fn rows_in(&self, schema: &str) -> Vec<Row> {
        sqlx::query_as(
            "SELECT tag_id, schema_name, table_name, column_name FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 AND lower(schema_name) = lower($2) ORDER BY 1, 2, 3, 4",
        )
        .bind(self.warehouse_id)
        .bind(schema)
        .fetch_all(&self.db.pool)
        .await
        .expect("rows")
    }
}

/// C13: a recursive namespace drop (which cannot be undropped) deletes every
/// classification row under it in any spelling, the lineage of its views,
/// and the rows its tables gave views elsewhere; one `tag:purge` event, by
/// the caller.
#[tokio::test]
async fn a_recursive_namespace_drop_deletes_its_classifications() {
    let Some(env) = env().await else { return };
    let (a, _) = env.classified_orders().await;
    env.create_table("events", &[(1, "id")]).await;
    env.classify_table("events").await;
    let shouting = env.new_gated_tag().await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'SALES','Events','')",
    )
    .bind(shouting)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign");
    let in_sales = env
        .lineage_view("sales", "v_orders", &[("sales", "orders")], &["id"])
        .await;
    let elsewhere = env
        .lineage_view("other", "v_orders", &[("sales", "orders")], &["id"])
        .await;
    assert!(
        env.rows()
            .await
            .contains(&(a, "other".into(), "v_orders".into(), "id".into())),
        "the view elsewhere carries the base's classification"
    );
    let (client, client_id) = env.catalog_client("spark-etl").await;
    *env.caller.lock().unwrap() = client_id.clone();
    let tag_version = env.tag_version().await;

    let (status, body) = env
        .call("DELETE", "namespaces/sales?recursive=true", None)
        .await;
    assert!(status.is_success(), "drop namespace: {status} {body}");
    assert!(
        env.rows_in("sales").await.is_empty(),
        "{:?}",
        env.rows_in("sales").await
    );
    assert!(
        env.rows_in("other").await.is_empty(),
        "the view elsewhere is re-tagged"
    );
    let mut conn = env.db.pool.acquire().await.unwrap();
    assert_eq!(
        egret_perm_compile::lineage::recorded_lineage(&mut conn, &in_sales)
            .await
            .unwrap(),
        egret_perm_compile::lineage::RecordedLineage::Missing
    );
    assert_ne!(
        egret_perm_compile::lineage::recorded_lineage(&mut conn, &elsewhere)
            .await
            .unwrap(),
        egret_perm_compile::lineage::RecordedLineage::Missing,
        "a view elsewhere keeps its lineage"
    );
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT detail FROM audit.audit_log WHERE action = 'tag:purge' AND resource_id = $1",
    )
    .bind(format!("{}:sales", env.warehouse_id))
    .fetch_all(&env.db.pool)
    .await
    .expect("events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["actor"]["id"], json!(client));
    // orders (A, B on email), events, Events in another spelling, and the
    // sales view's derived rows (A, B on `id`).
    assert_eq!(events[0]["rows"], json!(6));
    assert_eq!(events[0]["views"], json!(1));
    assert!(env.tag_version().await > tag_version);
}

/// R9: a plain namespace drop removes only an empty namespace; a row placed
/// on it beforehand stays (the stale-row report shows it).
#[tokio::test]
async fn a_plain_namespace_drop_keeps_rows() {
    let Some(env) = env().await else { return };
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces",
            Some(json!({"namespace": ["empty_ns"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let a = env.new_gated_tag().await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'empty_ns','later','')",
    )
    .bind(a)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign");

    let (status, body) = env.call("DELETE", "namespaces/empty_ns", None).await;
    assert!(status.is_success(), "drop: {status} {body}");
    assert_eq!(env.rows_in("empty_ns").await.len(), 1);
}

/// R9, E35: lakekeeper refuses a plain drop of a namespace that still holds a
/// soft-deleted table, so a plain drop never removes a dropped table's rows.
/// If this ever passes the drop, R9's purge must be built.
#[tokio::test]
async fn a_plain_drop_of_a_namespace_with_a_dropped_table_is_refused() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    env.soft_drop("orders").await;

    let (status, body) = env.call("DELETE", "namespaces/sales", None).await;
    assert!(!status.is_success(), "{status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "email")])
    );
}

/// R2: a dropped table keeps its rows; an undrop brings its classifications
/// back on both engines.
#[tokio::test]
async fn a_dropped_table_keeps_its_rows_and_an_undrop_restores_them() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let id = env.soft_drop("orders").await;
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "email")])
    );

    env.undrop(id).await.expect("undrop");
    let served = env.served_trino_resources().await;
    assert!(
        served.contains(&("orders".into(), String::new())),
        "{served:?}"
    );
    assert!(
        served.contains(&("orders".into(), "email".into())),
        "{served:?}"
    );
    let policies = env.served_starrocks_policies().await;
    let mask = format!(
        "{}{a}-mask-{}.sales.orders.id",
        egret_policy::COMPILER_CLEARANCE_PREFIX,
        env.catalog
    );
    assert!(policies.contains(&mask), "{policies:?}");
}

// ── Task 9: the stale-row report's table states (C14) ──

impl Env {
    async fn states(
        &self,
        keys: &[(&str, &str)],
    ) -> Vec<egret_perm_compile::table_columns::TableState> {
        let keys: Vec<egret_perm_compile::table_columns::TableKey> = keys
            .iter()
            .map(|(s, t)| egret_perm_compile::table_columns::TableKey {
                warehouse_id: self.warehouse_id,
                schema: s.to_string(),
                table: t.to_string(),
            })
            .collect();
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm_compile::table_columns::table_states_via(&mut conn, &keys)
            .await
            .expect("table states")
    }
}

/// C14, the drift guard of the stale-row report: its states follow what
/// Lakekeeper holds, folded, through a rename and a soft drop.
#[tokio::test]
async fn table_states_follow_lakekeeper() {
    use egret_perm_compile::table_columns::TableState::{Dropped, Live, Missing};
    let Some(env) = env().await else { return };
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({"name": "orders", "schema": schema(0, &[(1, "id")])})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create: {body}");
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/views",
            Some(json!({
                "name": "v_orders",
                "schema": schema(0, &[(1, "id")]),
                "view-version": {
                    "version-id": 1, "schema-id": 0,
                    "timestamp-ms": 1_700_000_000_000_i64,
                    "summary": {"engine-name": "trino"},
                    "default-namespace": ["sales"],
                    "representations": [{"type": "sql", "sql": "SELECT 1 AS id", "dialect": "trino"}]
                },
                "properties": {}
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create view: {body}");
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces",
            Some(json!({"namespace": ["sales", "eu"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "nested namespace: {body}");
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales%1Feu/tables",
            Some(json!({"name": "deep", "schema": schema(0, &[(1, "id")])})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "nested table: {body}");
    assert_eq!(
        env.states(&[
            ("sales", "orders"),
            ("SALES", "Orders"),
            ("sales", "v_orders"),
            ("sales", "ghost"),
            ("eu", "deep"),
            ("sales", "deep"),
        ])
        .await,
        vec![Live, Live, Live, Missing, Missing, Missing]
    );

    let (status, body) = env
        .call_raw(
            "POST",
            "tables/rename",
            Some(json!({
                "source": {"namespace": ["sales"], "name": "orders"},
                "destination": {"namespace": ["sales"], "name": "orders_v2"},
            })),
        )
        .await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.states(&[("sales", "orders"), ("sales", "orders_v2")])
            .await,
        vec![Missing, Live]
    );

    set_delete_profile(env.state.clone(), SYSTEM_USER, &env.prefix, Some(3600))
        .await
        .expect("soft delete profile");
    let (status, body) = env
        .call_raw("DELETE", "namespaces/sales/tables/orders_v2", None)
        .await;
    assert!(status.is_success(), "drop: {status} {body}");
    assert_eq!(env.states(&[("sales", "orders_v2")]).await, vec![Dropped]);
}

// ── Task 10: columns that are not in their table (R8) ──

impl Env {
    async fn missing_columns(
        &self,
        table: &str,
        columns: &[&str],
    ) -> Vec<(String, Option<String>)> {
        let key = egret_perm_compile::table_columns::TableKey {
            warehouse_id: self.warehouse_id,
            schema: "sales".into(),
            table: table.into(),
        };
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm_compile::table_columns::missing_columns_via(&mut conn, &[(key, columns)])
            .await
            .expect("missing columns")
            .into_iter()
            .map(|m| (m.column, m.renamed_to))
            .collect()
    }
}

/// R8, the drift guard of the missing-columns report: a classified column the
/// current schema lacks is missing, with the field that last carried its name
/// as the rename candidate when that field is still there.
#[tokio::test]
async fn missing_columns_follow_the_schema_history() {
    let Some(env) = env().await else { return };
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({
                "name": "orders",
                "schema": schema(0, &[(1, "id"), (2, "email"), (3, "region")]),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create: {body}");
    for (id, fields) in [
        (1, vec![(1, "id"), (2, "mail"), (3, "region")]),
        (2, vec![(1, "id"), (2, "mail")]),
    ] {
        let (status, body) = env
            .call_raw(
                "POST",
                "namespaces/sales/tables/orders",
                Some(schema_commit(schema(id, &fields))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "schema {id}: {body}");
    }
    assert_eq!(
        env.missing_columns("orders", &["email", "region", "ghost", "ID"])
            .await,
        vec![
            ("email".to_string(), Some("mail".to_string())),
            ("region".to_string(), None),
            ("ghost".to_string(), None),
        ]
    );

    // A new `region` (field 4), renamed to `area`: the newest field that
    // carried the name is the candidate, a suggestion a steward confirms.
    for (id, fields) in [
        (3, vec![(1, "id"), (2, "mail"), (4, "region")]),
        (4, vec![(1, "id"), (2, "mail"), (4, "area")]),
    ] {
        let (status, body) = env
            .call_raw(
                "POST",
                "namespaces/sales/tables/orders",
                Some(schema_commit(schema(id, &fields))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "schema {id}: {body}");
    }
    assert_eq!(
        env.missing_columns("orders", &["region"]).await,
        vec![("region".to_string(), Some("area".to_string()))]
    );
}

// ── Review fix 1: a soft-deleted twin keeps the source name's rows ──

/// Review finding 1: `orders` dropped (soft) and created again share the
/// name's rows. Renaming the live one away copies them; it does not take them
/// from the dropped one, which an undrop brings back classified on both
/// engines.
#[tokio::test]
async fn a_rename_keeps_the_rows_a_dropped_twin_of_the_source_needs() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let old = env.soft_drop("orders").await;
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![
            row(a, "orders", ""),
            row(a, "orders_v2", ""),
            row(b, "orders", "email"),
            row(b, "orders_v2", "email"),
        ])
    );
    let events = env.move_events("orders").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1["source_kept"], json!(true));

    env.undrop(old).await.expect("undrop");
    let served = env.served_trino_resources().await;
    assert!(
        served.contains(&("orders".into(), String::new())),
        "{served:?}"
    );
    assert!(
        served.contains(&("orders".into(), "email".into())),
        "{served:?}"
    );
    let deny = format!(
        "{}{a}-{}.sales.orders.*",
        egret_policy::COMPILER_CLEARANCE_PREFIX,
        env.catalog
    );
    assert!(env.served_starrocks_policies().await.contains(&deny));
}

/// Review finding 1, column variant: a column renamed on the live table keeps
/// its old row while a dropped twin of the table (which still has the column)
/// can come back.
#[tokio::test]
async fn a_column_rename_keeps_the_rows_a_dropped_twin_needs() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    env.soft_drop("orders").await;
    env.create_table("orders", &[(1, "id"), (2, "email")]).await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env
        .commit_schema("orders", 1, &[(1, "id"), (2, "mail")])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![
            row(a, "orders", ""),
            row(b, "orders", "email"),
            row(b, "orders", "mail"),
        ])
    );
}

// ── Review fix 2: catalog-tag writes during a hold ──

/// Review finding 2: a classification write on a table whose rename is held
/// is refused (409, `table_change_held`) instead of landing on the name the
/// move is about to leave; in any spelling of the name. Once the rename has
/// finished, the write goes through.
#[tokio::test]
async fn a_catalog_tag_write_during_a_held_rename_is_refused() {
    let Some(env) = env().await else { return };
    let env = Arc::new(env);
    env.classified_orders().await;
    let pii = env.new_gated_tag().await;
    let trino = env.start_trino_plugin();
    let starrocks = env.start_starrocks_plugin(POLL * 5);
    trino.polled().await;
    starrocks.polled().await;
    let repo = egret_bi::catalog_tags::CatalogTagRepo::new(env.db.clone());

    let rename = {
        let env = env.clone();
        tokio::spawn(async move { env.rename("orders", "orders_v2").await })
    };
    for _ in 0..200 {
        if env.rows().await.iter().any(|r| r.2 == "orders_v2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!rename.is_finished(), "the rename is still held");
    let refused = repo
        .assign_catalog(pii, env.warehouse_id, "Sales", "ORDERS", "email", None)
        .await
        .expect_err("refused while held");
    let resp = refused.into_response();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "table_change_held", "{body}");
    assert!(!env.rows().await.iter().any(|r| r.0 == pii));

    let (status, body) = rename.await.expect("rename");
    assert!(status.is_success(), "rename: {status} {body}");
    repo.assign_catalog(pii, env.warehouse_id, "sales", "orders_v2", "email", None)
        .await
        .expect("free once the rename has finished");
}

// ── Review fix 3: a held multi-table commit dry-runs its other tables ──

/// Review finding 3: one table of a multi-table commit renames a classified
/// column (so the commit is held) and another grows a whole-table
/// classification past its ceiling. The hold's compile is the dry run for
/// both: the commit is refused 409 before it lands, and neither table
/// changes.
#[tokio::test]
async fn a_held_multi_table_commit_refuses_another_tables_ceiling() {
    let Some(env) = env().await else { return };
    let (_, b) = env.classified_orders().await;
    // 1 deny + 498 masks = 499; growing to 500 columns makes it 501.
    let wide = fields(498);
    env.create_table("wide", &refs(&wide)).await;
    env.classify_table("wide").await;
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;

    let grown = fields(500);
    let change = |table: &str, fields: &[(i64, &str)]| {
        json!({
            "identifier": {"namespace": ["sales"], "name": table},
            "requirements": [],
            "updates": [
                {"action": "add-schema", "schema": schema(1, fields)},
                {"action": "set-current-schema", "schema-id": -1}
            ]
        })
    };
    let (status, body) = env
        .call(
            "POST",
            "transactions/commit",
            Some(json!({"table-changes": [
                change("orders", &[(1, "id"), (2, "mail")]),
                change("wide", &refs(&grown)),
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("ceiling"), "{message}");
    assert_eq!(env.loaded("wide").await.len(), 498, "nothing committed");
    assert_eq!(
        env.loaded("orders").await,
        vec!["id".to_string(), "email".to_string()]
    );
    assert!(env.rows().await.contains(&row(b, "orders", "email")));
    assert!(!env.rows().await.iter().any(|r| r.3 == "mail"));
}

// ── Review fix 4: a non-2xx after the commit is not undone ──

/// Review finding 4: lakekeeper commits the rename, then the answer becomes an
/// error. The hook checks the catalog (the destination holds the source's
/// table id) and finishes the move instead of undoing it, so the renamed
/// table keeps its classifications.
#[tokio::test]
async fn an_error_after_a_committed_rename_finishes_the_move() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let _plugins = env.start_plugins().await;
    *env.fail_after_commit.lock().unwrap() = Some(format!("POST /{}/tables/rename", env.prefix));

    let (status, body) = env.rename("orders", "orders_v2").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        env.source("orders_v2").await.is_some(),
        "the rename committed"
    );
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders_v2", ""), row(b, "orders_v2", "email")])
    );
    assert_eq!(env.move_events("orders").await.len(), 1);
}

/// Review finding 4, column variant: the schema commit landed, then failed.
#[tokio::test]
async fn an_error_after_a_committed_column_rename_finishes_the_move() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    let _plugins = env.start_plugins().await;
    *env.fail_after_commit.lock().unwrap() = Some(format!(
        "POST /{}/namespaces/sales/tables/orders",
        env.prefix
    ));

    let (status, body) = env
        .commit_schema("orders", 1, &[(1, "id"), (2, "mail")])
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "mail")])
    );
}

// ── Live check F1: a rename onto a dropped table's name ──

/// Live check F1: lakekeeper refuses a rename onto a name a soft-deleted table
/// holds (pinned here without egret's hook), with a misleading 404, and a
/// classified rename used to learn that only after the whole hold. egret
/// refuses it at once with a 409 that names the cause; nothing is written.
#[tokio::test]
async fn a_classified_rename_onto_a_dropped_tables_name_is_refused_at_once() {
    let Some(env) = env().await else { return };
    let (a, b) = env.classified_orders().await;
    env.create_table("binned", &[(1, "id")]).await;
    env.soft_drop("binned").await;
    env.create_table("plain", &[(1, "id")]).await;
    let (status, body) = env
        .call_raw(
            "POST",
            "tables/rename",
            Some(json!({
                "source": {"namespace": ["sales"], "name": "plain"},
                "destination": {"namespace": ["sales"], "name": "binned"},
            })),
        )
        .await;
    assert!(
        !status.is_success(),
        "lakekeeper refuses it: {status} {body}"
    );
    let tag_version = env.tag_version().await;

    let started = Instant::now();
    let (status, body) = env.rename("orders", "binned").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(started.elapsed() < Duration::from_secs(1));
    let message = body["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("recycle bin"), "{message}");
    assert!(message.ends_with(". Nothing was renamed."), "{message}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(a, "orders", ""), row(b, "orders", "email")])
    );
    assert_eq!(env.tag_version().await, tag_version);
}

// ── Review fix 9: LOW-list tests ──

/// Review finding 7 (documented, over-tag): one commit drops field 3 (`b`) and
/// renames field 2 (`a`) to `b`. `a`'s rows follow field 2 to `b`; `b`'s rows
/// stay on the name `b` (R3 keeps a dropped field's rows dormant, by name), so
/// field 2 now carries both. Over-tags, never under-tags.
#[tokio::test]
async fn a_column_renamed_onto_a_dropped_fields_name_carries_both() {
    let Some(env) = env().await else { return };
    env.create_table("orders", &[(1, "id"), (2, "a"), (3, "b")])
        .await;
    let on_a = env.classify_column("orders", "a").await;
    let on_b = env.classify_column("orders", "b").await;
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;

    let (status, body) = env.commit_schema("orders", 1, &[(1, "id"), (2, "b")]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        env.rows().await,
        sorted(vec![row(on_a, "orders", "b"), row(on_b, "orders", "b")])
    );
}

/// The hold's work runs uncancellable: a client that goes away mid-hold does
/// not stop the wait, the forward or `finish`. The rename lands and the rows
/// end on the new name only.
#[tokio::test]
async fn a_client_that_cancels_mid_hold_does_not_stop_the_move() {
    let Some(env) = env().await else { return };
    let env = Arc::new(env);
    let (a, b) = env.classified_orders().await;
    let trino = env.start_trino_plugin();
    let starrocks = env.start_starrocks_plugin(POLL * 5);
    trino.polled().await;
    starrocks.polled().await;

    let rename = {
        let env = env.clone();
        tokio::spawn(async move { env.rename("orders", "orders_v2").await })
    };
    for _ in 0..200 {
        if env.rows().await.iter().any(|r| r.2 == "orders_v2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!rename.is_finished(), "the rename is still held");
    rename.abort();

    let moved = sorted(vec![row(a, "orders_v2", ""), row(b, "orders_v2", "email")]);
    for _ in 0..100 {
        if env.rows().await == moved {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(env.rows().await, moved);
    assert!(env.source("orders_v2").await.is_some(), "the rename landed");
}
