//! KI-319: a view renamed through egret's Iceberg catalog
//! (`POST /{prefix}/views/rename`) carries its grants, path masks, path row
//! filters and catalog grants by the rules platform #764 set for a table:
//! restrictions are copied to the new name before the commit and kept on the
//! old one, allows and catalog grants move after the commit, and a
//! restriction the new name would escape refuses the rename.
//!
//! Each test drives the REAL embedded lakekeeper (on its in-memory storage)
//! through [`front_catalog_router`] in the production order: a guard that
//! names a catalog client as the caller, the view hold, the table hook, then
//! lakekeeper. Fake Trino and StarRocks plugins poll egret's real Ranger
//! router; the engines' decisions are read from their downloads through
//! `egret-ranger-eval`.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-catalog-engine --test view_rename_db

#[path = "../../egret-perm-compile/tests/support/engine_eval.rs"]
mod engine_eval;
#[path = "common/plugins.rs"]
mod plugins;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, run_lakekeeper_migrations, table_hook, TableHook, ViewHold,
};
use egret_metastore::Db;
use egret_perm_compile::lineage::ExplainRunner;
use egret_policy::PolicyStore;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";
/// The hold's deadline in these tests.
const LIMIT: Duration = Duration::from_secs(10);
/// How often the fake plugins poll.
const POLL: Duration = Duration::from_millis(200);

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

/// No profile here has a classified column, so the hold never EXPLAINs a
/// rename; a create through the hold is never sent (views are created raw).
struct NoExplain;

#[async_trait::async_trait]
impl ExplainRunner for NoExplain {
    async fn explain_io(&self, _: &str, _: &str, _: &str) -> Result<Value, String> {
        Err("no engine in this test".into())
    }
}

/// `(kind, effect or rule kind, table, column)` of every grant and path rule.
type PermRow = (String, String, String, String);

/// What the catalog held when the rename reached lakekeeper.
#[derive(Debug, Clone)]
struct AtReach {
    at: Instant,
    perm: Vec<PermRow>,
    trino_policy_version: i64,
    starrocks_policy_version: i64,
    tag_version: i64,
}

type Reach = Arc<Mutex<Option<(String, Option<AtReach>)>>>;

/// Records, when the armed request reaches lakekeeper, what [`AtReach`] says.
async fn observe(
    State((reach, db, warehouse, trino, starrocks)): State<(Reach, Db, Uuid, i64, i64)>,
    req: Request,
    next: Next,
) -> Response {
    let what = format!("{} {}", req.method(), req.uri().path());
    let armed = reach
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|(w, seen)| *w == what && seen.is_none());
    if armed {
        let at = Instant::now();
        let version = |id: i64, column: &'static str| {
            let db = db.clone();
            async move {
                let sql = if column == "tag" {
                    "SELECT tag_version FROM policy.authz_services WHERE id = $1"
                } else {
                    "SELECT policy_version FROM policy.authz_services WHERE id = $1"
                };
                sqlx::query_scalar::<_, i64>(sql)
                    .bind(id)
                    .fetch_one(&db.pool)
                    .await
                    .expect("version")
            }
        };
        let seen = AtReach {
            at,
            perm: perm_rows(&db, warehouse).await,
            trino_policy_version: version(trino, "policy").await,
            starrocks_policy_version: version(starrocks, "policy").await,
            tag_version: version(trino, "tag").await,
        };
        if let Some(slot) = reach.lock().unwrap().as_mut() {
            slot.1 = Some(seen);
        }
    }
    next.run(req).await
}

async fn perm_rows(db: &Db, warehouse: Uuid) -> Vec<PermRow> {
    sqlx::query_as(
        "SELECT 'grant', g.effect::text, coalesce(g.table_name, ''), coalesce(g.column_name, '') \
           FROM perm.grants g JOIN registry.catalogs c ON c.id = g.catalog_id \
          WHERE c.warehouse_id = $1 \
         UNION ALL \
         SELECT 'rule', r.kind::text, coalesce(r.table_name, ''), coalesce(r.column_name, '') \
           FROM perm.rules r JOIN registry.catalogs c ON c.id = r.catalog_id \
          WHERE c.warehouse_id = $1 \
         ORDER BY 1, 2, 3, 4",
    )
    .bind(warehouse)
    .fetch_all(&db.pool)
    .await
    .expect("perm rows")
}

fn row(kind: &str, what: &str, table: &str, column: &str) -> PermRow {
    (kind.into(), what.into(), table.into(), column.into())
}

/// Names the catalog client in its slot as every request's caller, as the
/// production guard does with a verified catalog token.
struct ClientGuard(Arc<Mutex<String>>);

#[async_trait::async_trait]
impl egret_catalog_engine::CatalogGuard for ClientGuard {
    async fn authenticate(&self, _bearer: Option<&str>) -> Result<(), (StatusCode, String)> {
        Ok(())
    }

    async fn check(
        &self,
        _method: &str,
        _path: &str,
        _query: Option<&str>,
        _bearer: Option<&str>,
        _body: Option<&[u8]>,
    ) -> Result<egret_catalog_engine::CatalogCaller, (StatusCode, String)> {
        Ok(egret_catalog_engine::CatalogCaller {
            subject: self.0.lock().unwrap().clone(),
            audience: "catalog".into(),
            warehouse: String::new(),
            role: String::new(),
        })
    }

    async fn filter_listing(
        &self,
        _caller: &egret_catalog_engine::CatalogCaller,
        _listing: egret_catalog_engine::Listing,
        _body: &[u8],
    ) -> Result<Option<Vec<u8>>, (StatusCode, String)> {
        Ok(None)
    }
}

struct Env {
    db: Db,
    app: axum::Router,
    raw: axum::Router,
    prefix: String,
    warehouse_id: Uuid,
    catalog: String,
    catalog_id: Uuid,
    trino_id: i64,
    trino_name: String,
    starrocks_id: i64,
    starrocks_name: String,
    policy: PolicyStore,
    reach: Reach,
    /// The catalog client the guard names as the caller.
    client_id: String,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    lakekeeper_ready(&dsn).await;
    let (state, _registry) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("vrwh"))
        .await
        .expect("lakekeeper warehouse");
    let pool = &db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'VR') RETURNING id")
            .bind(unique("vr-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-vr-test','AKIATEST') RETURNING id",
    )
    .bind(unique("vr-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(unique("vr_wh"))
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
    let trino_name = unique("trino_vr");
    let tag_service = format!("{trino_name}_tag");
    policy
        .create_service(&tag_service, "tag", &json!({}))
        .await
        .expect("tag service");
    let trino = policy
        .create_service(&trino_name, "trino", &json!({"tagService": tag_service}))
        .await
        .expect("trino profile");
    let starrocks_name = unique("sr_vr");
    let starrocks = policy
        .create_service(&starrocks_name, "starrocks", &json!({}))
        .await
        .expect("starrocks profile");
    let mut engines = Vec::new();
    for (kind, profile, spec) in [
        ("trino", trino.id, json!({})),
        (
            "starrocks",
            starrocks.id,
            json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1.4"}}),
        ),
    ] {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
             VALUES ($1,$2,'egret',$3,$4) RETURNING id",
        )
        .bind(unique("vr-eng"))
        .bind(kind)
        .bind(spec)
        .bind(profile)
        .fetch_one(pool)
        .await
        .expect("engine");
        engines.push(id);
    }
    let catalog = unique("vr_cat");
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
    for engine in engines {
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
    // The renaming principal: a catalog client of an engine role, not an
    // admin (the guard has authorized the rename already).
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
         VALUES ($1, $2, 'engine') RETURNING id",
    )
    .bind(warehouse_id)
    .bind(unique("vr-role"))
    .fetch_one(pool)
    .await
    .expect("catalog role");
    let client_id = unique("trino-etl");
    sqlx::query(
        "INSERT INTO catalog.catalog_clients (name, consumer_type, workspace_id, warehouse_id, \
         catalog_role_id, client_id, client_secret_hash) \
         VALUES ('trino-etl', 'engine', $1, $2, $3, $4, 'x')",
    )
    .bind(ws)
    .bind(warehouse_id)
    .bind(role)
    .bind(&client_id)
    .execute(pool)
    .await
    .expect("catalog client");

    let reach: Reach = Arc::default();
    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let inner = catalog_engine_router(state).layer(axum::middleware::from_fn_with_state(
        (
            reach.clone(),
            db.clone(),
            warehouse_id,
            trino.id,
            starrocks.id,
        ),
        observe,
    ));
    let hook = Arc::new(TableHook::new(db.clone()).with_limit(LIMIT));
    let hold = Arc::new(ViewHold::new(db.clone(), Arc::new(NoExplain)).with_limit(LIMIT));
    let guard: Arc<dyn egret_catalog_engine::CatalogGuard> =
        Arc::new(ClientGuard(Arc::new(Mutex::new(client_id.clone()))));
    let app = front_catalog_router(table_hook::layer(inner, hook), Some(guard), Some(hold));
    let env = Env {
        db,
        app,
        raw,
        prefix,
        warehouse_id,
        catalog,
        catalog_id,
        trino_id: trino.id,
        trino_name,
        starrocks_id: starrocks.id,
        starrocks_name,
        policy,
        reach,
        client_id,
    };
    let (status, body) = env
        .send(
            &env.raw,
            "POST",
            "namespaces",
            Some(json!({"namespace": ["sales"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create namespace: {body}");
    Some(env)
}

fn view_body(name: &str) -> Value {
    json!({
        "name": name,
        "schema": {"type": "struct", "schema-id": 0, "fields": [
            {"id": 1, "name": "id", "type": "string", "required": false},
            {"id": 2, "name": "email", "type": "string", "required": false},
        ]},
        "view-version": {
            "version-id": 1,
            "schema-id": 0,
            "timestamp-ms": chrono::Utc::now().timestamp_millis(),
            "summary": {"engine-name": "trino", "engine-version": "483"},
            "default-namespace": ["sales"],
            "representations": [{"type": "sql", "sql": "SELECT id, email FROM orders",
                                 "dialect": "trino"}]
        },
        "properties": {}
    })
}

impl Env {
    async fn send(
        &self,
        app: &axum::Router,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = axum::http::Request::builder()
            .method(method)
            .uri(format!("/{}/{path}", self.prefix))
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

    /// A view `sales.{name}`, created with no hold in front.
    async fn create_view(&self, name: &str) {
        let (status, body) = self
            .send(
                &self.raw,
                "POST",
                "namespaces/sales/views",
                Some(view_body(name)),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create view {name}: {body}");
    }

    async fn view_exists(&self, name: &str) -> bool {
        let (status, body) = self
            .send(
                &self.raw,
                "GET",
                &format!("namespaces/sales/views/{name}"),
                None,
            )
            .await;
        match status {
            StatusCode::OK => true,
            StatusCode::NOT_FOUND => false,
            other => panic!("load view {name}: {other} {body}"),
        }
    }

    /// `ALTER VIEW sales.{from} RENAME TO sales.{to}` as Trino sends it.
    async fn rename(&self, from: &str, to: &str) -> (StatusCode, Value) {
        self.send(
            &self.app,
            "POST",
            "views/rename",
            Some(json!({
                "source": {"namespace": ["sales"], "name": from},
                "destination": {"namespace": ["sales"], "name": to},
            })),
        )
        .await
    }

    async fn grant(&self, effect: egret_perm::Effect, levels: &[&str], users: &[&str]) -> Uuid {
        let at = |i: usize| levels.get(i).map(|s| s.to_string());
        egret_perm::PermStore::new(self.db.pool.clone())
            .create_grant(&egret_perm::Grant {
                id: Uuid::nil(),
                effect,
                actions: vec![egret_perm::Action::Read],
                scope: egret_perm::Scope {
                    catalog_id: self.catalog_id,
                    schema: at(0),
                    table: at(1),
                    column: at(2),
                },
                principals: egret_perm::PrincipalSet {
                    include: users
                        .iter()
                        .map(|u| egret_perm::Principal {
                            kind: egret_perm::PrincipalKind::User,
                            r#ref: u.to_string(),
                        })
                        .collect(),
                    exclude: Vec::new(),
                },
                origin: egret_perm::GrantOrigin::Explicit,
            })
            .await
            .expect("grant")
    }

    async fn rule(&self, kind: egret_perm::RuleKind, levels: &[&str]) -> Uuid {
        let at = |i: usize| levels.get(i).map(|s| s.to_string());
        egret_perm::PermStore::new(self.db.pool.clone())
            .create_rule(&egret_perm::Rule {
                id: Uuid::nil(),
                name: unique("vr-rule"),
                kind,
                selector: egret_perm::Selector::Path(egret_perm::Scope {
                    catalog_id: self.catalog_id,
                    schema: at(0),
                    table: at(1),
                    column: at(2),
                }),
                exempt: egret_perm::PrincipalSet::default(),
            })
            .await
            .expect("rule")
    }

    /// View `v(id, email)`, readable to `u` and `w` everywhere in the
    /// catalog; `u` denied `sales.v`, `email` hash-masked, `v` filtered to
    /// `region = 'eu'`.
    async fn restricted_view(&self) {
        self.create_view("v").await;
        self.grant(egret_perm::Effect::Allow, &[], &["u", "w"])
            .await;
        self.grant(egret_perm::Effect::Deny, &["sales", "v"], &["u"])
            .await;
        self.rule(
            egret_perm::RuleKind::Mask {
                mask_type: "MASK_HASH".into(),
                value_expr: None,
            },
            &["sales", "v", "email"],
        )
        .await;
        self.rule(
            egret_perm::RuleKind::RowFilter {
                filter_expr: "region = 'eu'".into(),
            },
            &["sales", "v"],
        )
        .await;
        self.recompile_all().await;
    }

    /// Compile the model into both profiles (a store write does not).
    async fn recompile_all(&self) {
        let mut tx = self.db.pool.begin().await.expect("begin");
        let mut ids = vec![self.trino_id, self.starrocks_id];
        ids.sort_unstable();
        egret_perm_compile::recompile_services_in_tx(&mut tx, &ids)
            .await
            .expect("recompile");
        tx.commit().await.expect("commit");
    }

    async fn perm(&self) -> Vec<PermRow> {
        perm_rows(&self.db, self.warehouse_id).await
    }

    async fn start_plugins(&self) -> (plugins::Plugin, plugins::Plugin) {
        let trino = plugins::start_trino(self.policy.clone(), self.trino_name.clone(), POLL);
        let starrocks =
            plugins::start_starrocks(self.policy.clone(), self.starrocks_name.clone(), POLL);
        trino.polled().await;
        starrocks.polled().await;
        (trino, starrocks)
    }

    fn probe_reach(&self) {
        *self.reach.lock().unwrap() = Some((format!("POST /{}/views/rename", self.prefix), None));
    }

    fn reached(&self) -> Option<AtReach> {
        self.reach.lock().unwrap().take().and_then(|(_, seen)| seen)
    }

    fn resource(&self, view: &str, column: &str, middle: &str) -> Vec<(String, String)> {
        vec![
            ("catalog".into(), self.catalog.clone()),
            (middle.into(), "sales".into()),
            ("table".into(), view.into()),
            ("column".into(), column.into()),
        ]
    }

    async fn trino_reads(&self, user: &str, view: &str) -> bool {
        let r = self.resource(view, "id", "schema");
        let r: Vec<(&str, &str)> = r.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        engine_eval::allowed(
            &engine_eval::trino_decides(&self.db, self.trino_id, user, "select", &r).await,
        )
    }

    async fn starrocks_reads(&self, user: &str, view: &str) -> bool {
        let r = self.resource(view, "id", "database");
        let r: Vec<(&str, &str)> = r.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        engine_eval::allowed(
            &engine_eval::starrocks_decides(&self.db, self.starrocks_id, user, "select", &r).await,
        )
    }

    async fn starrocks_mask_of(&self, user: &str, view: &str) -> Option<String> {
        let r = self.resource(view, "email", "database");
        let r: Vec<(&str, &str)> = r.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        engine_eval::mask_type(
            &engine_eval::starrocks_mask(&self.db, self.starrocks_id, user, &r).await,
        )
    }

    async fn starrocks_filter_of(&self, user: &str, view: &str) -> Option<String> {
        let r = self.resource(view, "id", "database");
        let r: Vec<(&str, &str)> = r[..3]
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match engine_eval::starrocks_row_filter(&self.db, self.starrocks_id, user, &r).await {
            egret_ranger_eval::eval::RowFilterDecision::Filtered(f) => Some(f.filter_expr),
            egret_ranger_eval::eval::RowFilterDecision::NoFilter => None,
            other => panic!("undecidable row filter: {other:?}"),
        }
    }

    /// The grant and rule events naming this catalog: `(action, id, detail)`.
    async fn perm_events(&self) -> Vec<(String, String, Value)> {
        sqlx::query_as(
            "SELECT action, resource_id, detail FROM audit.audit_log \
              WHERE kind = 'mutation' AND action IN ('perm_grant:move', 'perm_grant:copy', \
                    'perm_rule:copy', 'perm_grant:left', 'ranger_policy:left') \
                AND (detail -> 'after' ->> 'catalog' = $1 OR detail -> 'before' ->> 'catalog' = $1 \
                     OR starts_with(detail -> 'after' ->> 'selector', $1 || '.')) \
              ORDER BY id",
        )
        .bind(&self.catalog)
        .fetch_all(&self.db.pool)
        .await
        .expect("events")
    }

    fn assert_refused(&self, status: StatusCode, body: &Value, names: &[&str]) {
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let message = body["error"]["message"].as_str().expect("message");
        assert!(message.starts_with("egret refused the view: "), "{message}");
        assert!(message.ends_with(". Nothing was renamed."), "{message}");
        for n in names {
            assert!(message.contains(n), "{n} not in {message}");
        }
    }
}

/// KI-319: the deny, the path mask and the path row filter of a view are
/// copied to the new name before the commit; the originals stay on the old
/// name, dormant, so a view created later under it is still restricted. Both
/// engines restrict both names.
#[tokio::test]
async fn a_renamed_view_keeps_its_restrictions_on_both_names() {
    let Some(env) = env().await else { return };
    env.restricted_view().await;
    let _plugins = env.start_plugins().await;
    assert!(!env.trino_reads("u", "v").await, "u denied before");

    let (status, body) = env.rename("v", "v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert!(env.view_exists("v2").await && !env.view_exists("v").await);
    assert_eq!(
        env.perm().await,
        vec![
            row("grant", "allow", "", ""),
            row("grant", "deny", "v", ""),
            row("grant", "deny", "v2", ""),
            row("rule", "mask", "v", "email"),
            row("rule", "mask", "v2", "email"),
            row("rule", "row_filter", "v", ""),
            row("rule", "row_filter", "v2", ""),
        ]
    );
    for view in ["v", "v2"] {
        assert!(
            !env.trino_reads("u", view).await,
            "Trino lets u read {view}"
        );
        assert!(
            env.trino_reads("w", view).await,
            "Trino refuses w on {view}"
        );
        assert!(
            !env.starrocks_reads("u", view).await,
            "StarRocks lets u read {view}"
        );
        assert_eq!(
            env.starrocks_mask_of("w", view).await.as_deref(),
            Some("MASK_HASH"),
            "{view}"
        );
        assert_eq!(
            env.starrocks_filter_of("w", view).await.as_deref(),
            Some("region = 'eu'"),
            "{view}"
        );
    }
    let copies: Vec<String> = env
        .perm_events()
        .await
        .into_iter()
        .map(|(action, _, d)| {
            assert_eq!(d["actor"]["client_id"], json!(env.client_id), "{d}");
            assert_eq!(d["rename"], json!({"from": "sales.v", "to": "sales.v2"}));
            action
        })
        .collect();
    assert_eq!(
        copies,
        vec!["perm_grant:copy", "perm_rule:copy", "perm_rule:copy"]
    );
}

/// KI-319, #764 D6: the rename reaches lakekeeper only once the copies exist
/// and both plugins have reported the policy version that serves them.
#[tokio::test]
async fn the_view_rename_reaches_lakekeeper_only_after_the_engines_serve_the_copies() {
    let Some(env) = env().await else { return };
    env.restricted_view().await;
    let (trino, starrocks) = env.start_plugins().await;
    env.probe_reach();

    let (status, body) = env.rename("v", "v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let reached = env.reached().expect("the rename reached lakekeeper");
    assert!(
        reached.perm.contains(&row("grant", "deny", "v2", "")),
        "{:?}",
        reached.perm
    );
    assert!(trino.policy_report_of(reached.trino_policy_version).await < reached.at);
    assert!(
        starrocks
            .policy_report_of(reached.starrocks_policy_version)
            .await
            < reached.at
    );
}

/// KI-319, #764 D2: an allow grant on the view moves to the new name after the
/// commit, keeping its id; it is not on the new name before the commit.
#[tokio::test]
async fn a_renamed_views_allow_moves_after_the_commit_and_keeps_its_id() {
    let Some(env) = env().await else { return };
    env.create_view("v").await;
    let allow = env
        .grant(egret_perm::Effect::Allow, &["sales", "v"], &["u"])
        .await;
    env.recompile_all().await;
    let _plugins = env.start_plugins().await;
    env.probe_reach();
    assert!(env.trino_reads("u", "v").await);

    let (status, body) = env.rename("v", "v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let reached = env.reached().expect("the rename reached lakekeeper");
    assert_eq!(reached.perm, vec![row("grant", "allow", "v", "")]);
    assert_eq!(env.perm().await, vec![row("grant", "allow", "v2", "")]);
    let table: String = sqlx::query_scalar("SELECT table_name FROM perm.grants WHERE id = $1")
        .bind(allow)
        .fetch_one(&env.db.pool)
        .await
        .expect("the allow keeps its id");
    assert_eq!(table, "v2");
    assert!(env.trino_reads("u", "v2").await, "u reads the renamed view");
    assert!(
        !env.trino_reads("u", "v").await,
        "nothing reaches the old name"
    );
    let events = env.perm_events().await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "perm_grant:move");
    assert_eq!(events[0].1, allow.to_string());
}

/// KI-319, #764 D8: a `table` catalog grant on the view's name (it covers the
/// view) moves to the new name after the commit, with its event.
#[tokio::test]
async fn a_views_catalog_grant_follows_the_rename() {
    let Some(env) = env().await else { return };
    env.create_view("v").await;
    let store = egret_catalog_engine::CatalogStore::new(env.db.clone());
    let role = store
        .create_role(env.warehouse_id, &unique("vr-reader"), "external", None)
        .await
        .expect("role");
    let grant = store
        .add_grant(
            role.id,
            &egret_catalog_engine::GrantInput {
                securable_type: "table".into(),
                namespace: Some(vec!["sales".into()]),
                table_name: Some("v".into()),
                privilege: "read".into(),
            },
        )
        .await
        .expect("catalog grant")
        .id;

    let (status, body) = env.rename("v", "v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let at: (Vec<String>, String) =
        sqlx::query_as("SELECT namespace, table_name FROM catalog.catalog_grants WHERE id = $1")
            .bind(grant)
            .fetch_one(&env.db.pool)
            .await
            .expect("catalog grant");
    assert_eq!(at, (vec!["sales".to_string()], "v2".to_string()));
    let events: Vec<(String, Value)> = sqlx::query_as(
        "SELECT action, detail FROM audit.audit_log \
          WHERE resource_type = 'catalog_grant' AND resource_id = $1 ORDER BY id",
    )
    .bind(grant.to_string())
    .fetch_all(&env.db.pool)
    .await
    .expect("events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "catalog_grant:move");
    assert_eq!(events[0].1["actor"]["client_id"], json!(env.client_id));
    assert_eq!(
        events[0].1["after"],
        json!({"namespace": ["sales"], "table": "v2"})
    );
}

/// KI-319, #764 G4: a deny that reaches the view only through a pattern and
/// would not reach the new name refuses the rename, naming it; nothing is
/// renamed, copied or moved.
#[tokio::test]
async fn a_pattern_restriction_the_new_name_escapes_refuses_the_view_rename() {
    let Some(env) = env().await else { return };
    env.create_view("v").await;
    let pattern = env
        .grant(egret_perm::Effect::Deny, &["sales", "v*"], &["u"])
        .await;
    let perm = env.perm().await;

    let (status, body) = env.rename("v", "archive").await;
    env.assert_refused(
        status,
        &body,
        &[&format!("deny grant {pattern} (sales.v*)"), "sales.archive"],
    );
    assert!(env.view_exists("v").await && !env.view_exists("archive").await);
    assert_eq!(env.perm().await, perm);

    // A name the pattern still covers goes through.
    let (status, body) = env.rename("v", "v_old").await;
    assert!(status.is_success(), "rename: {status} {body}");
}

/// KI-319, #764 D1: a rename lakekeeper refuses (the new name is taken)
/// deletes the copies; the originals stay.
#[tokio::test]
async fn a_failed_view_rename_deletes_the_copies() {
    let Some(env) = env().await else { return };
    env.restricted_view().await;
    env.create_view("v2").await;
    let _plugins = env.start_plugins().await;
    let before = env.perm().await;

    let (status, body) = env.rename("v", "v2").await;
    assert!(
        !status.is_success(),
        "rename onto a taken name: {status} {body}"
    );
    assert_eq!(env.perm().await, before);
}

/// KI-319 and the view hold: a view that carries classification tags and a
/// deny moves both under one hold, and reaches lakekeeper once Trino serves
/// the moved tags and both engines the copied deny.
#[tokio::test]
async fn a_tagged_view_moves_its_tags_and_restrictions_under_one_hold() {
    let Some(env) = env().await else { return };
    env.create_view("v").await;
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("vr-tag"))
        .bind(unique("VrTag"))
        .fetch_one(&env.db.pool)
        .await
        .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1, $2, 'sales', 'v', 'email')",
    )
    .bind(tag)
    .bind(env.warehouse_id)
    .execute(&env.db.pool)
    .await
    .expect("assign");
    env.grant(egret_perm::Effect::Deny, &["sales", "v"], &["u"])
        .await;
    let (trino, _starrocks) = env.start_plugins().await;
    env.probe_reach();

    let (status, body) = env.rename("v", "v2").await;
    assert!(status.is_success(), "rename: {status} {body}");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM catalog_tag_assignments WHERE warehouse_id = $1",
    )
    .bind(env.warehouse_id)
    .fetch_all(&env.db.pool)
    .await
    .expect("tags");
    assert_eq!(tables, vec!["v2".to_string()]);
    let reached = env.reached().expect("the rename reached lakekeeper");
    assert!(reached.perm.contains(&row("grant", "deny", "v2", "")));
    assert!(trino.report_of(reached.tag_version).await < reached.at);
    assert!(trino.policy_report_of(reached.trino_policy_version).await < reached.at);
}

/// KI-320 on a view: a hand-written StarRocks deny that names views only
/// through a pattern of its `view` level, and would not reach the new name,
/// refuses the rename, naming the policy.
#[tokio::test]
async fn a_handwritten_starrocks_view_pattern_refuses_a_view_rename() {
    let Some(env) = env().await else { return };
    env.create_view("v").await;
    let item = json!([{"users": ["u"], "accesses": [{"type": "select", "isAllowed": true}]}]);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator') RETURNING id",
    )
    .bind(env.starrocks_id)
    .bind(unique("vr-hand"))
    .bind(json!({
        "policyType": 0,
        "resources": {
            "catalog": {"values": [env.catalog]},
            "database": {"values": ["sales"]},
            "view": {"values": ["v*"]},
        },
        "denyPolicyItems": item,
    }))
    .fetch_one(&env.db.pool)
    .await
    .expect("policy");

    let (status, body) = env.rename("v", "archive").await;
    env.assert_refused(
        status,
        &body,
        &[
            &format!("{}/", env.starrocks_name),
            &format!("({id})"),
            "through a name pattern",
        ],
    );
    assert!(env.view_exists("v").await);
}
